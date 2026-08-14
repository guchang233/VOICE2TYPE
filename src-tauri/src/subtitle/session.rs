//! 字幕会话：音频源 → 豆包流式 ASR → 句段/说话人跟踪 → 翻译 → 帧广播 + 转录
//!
//! 单源模式：A = 麦克风 或 系统扬声器；
//! 双音源同传模式（"dual"）：A = 系统扬声器（译文主源），B = 麦克风（副字幕）。
//! 翻译只作用于 A 源；转录同时记录 A/B 定稿句段。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::audio::processor::resample_and_convert;
use crate::config::{ConfigManager, STREAM_MODEL_DOUBAO};
use crate::streaming::{start_capture_with_prefs, AsrResponse, StreamingAsrClient};
use crate::subtitle::transcript::Transcript;
use crate::subtitle::translate::{
    resolve_engine, split_by_width, split_complete_sentences, TranslationPipeline,
    LINE_WIDTH_CHARS,
};
use crate::subtitle::window::scene_window_label;

const CHUNK_MS: u64 = 200;
const HISTORY_LIMIT: usize = 8;
/// 帧刷新节拍：合并发射间隔（毫秒）。所有帧状态变化只置脏标记，
/// 由合并发射器按此节拍统一上屏，防止高频事件击穿 WebView 消息队列。
const FRAME_FLUSH_MS: u64 = 80;
/// PCM 缓冲安全上限：超过约 5 秒样本（按 48kHz 单声道）则丢弃最旧，防止消费阻塞导致内存暴涨
const MAX_PCM_BUFFER_SAMPLES: usize = 5 * 48_000;
/// 说话人分轮：两次发声间隔超过该阈值视为新轮次
const SPEAKER_TURN_GAP_MS: u64 = 2500;
/// B 源（双源模式麦克风）固定说话人标签
const SOURCE_B_SPEAKER: &str = "麦克风";

// ==================== 会话状态 ====================

/// 单个音源的文本状态
#[derive(Clone, Default)]
pub struct SourceText {
    pub text: String,
    pub definite: String,
    pub indefinite: String,
    pub history: Vec<String>,
    pub tail: String,
    pub speaker: String,
}

/// 会话级状态（识别侧，所有场景共享；翻译状态在各场景流水线内）
pub struct SessionAsr {
    /// A 源全文（占位/状态提示用）
    pub text: String,
    pub is_final: bool,
    pub a: SourceText,
    pub b: SourceText,
    pub dual: bool,
}

/// 增量句段跟踪器：从 definite 流中切出完整句段并维护历史
struct LineTracker {
    history: VecDeque<String>,
    consumed_chars: usize,
}

impl LineTracker {
    fn new() -> Self {
        Self {
            history: VecDeque::new(),
            consumed_chars: 0,
        }
    }

    /// 喂入 definite 文本，返回本帧新定稿的行（标点断句 + 行宽强制切行）
    fn feed(&mut self, definite: &str) -> Vec<String> {
        let mut finalized = Vec::new();
        let chars: Vec<char> = definite.chars().collect();
        if self.consumed_chars > chars.len() {
            // ASR 文本回退（异常）：重置
            self.history.clear();
            self.consumed_chars = 0;
        }
        let new_part: String = chars[self.consumed_chars..].iter().collect();
        if new_part.is_empty() {
            return finalized;
        }
        let (complete, tail) = split_complete_sentences(&new_part);
        for s in complete {
            if self.history.len() >= HISTORY_LIMIT {
                self.history.pop_front();
            }
            self.history.push_back(s.clone());
            finalized.push(s);
        }
        self.consumed_chars += new_part.chars().count() - tail.chars().count();

        // 长行按行宽强制切行：大量文本自动换行，新行从底部出现、顶部行滚出
        if tail.chars().count() >= LINE_WIDTH_CHARS {
            let (lines, new_tail) = split_by_width(&tail, LINE_WIDTH_CHARS);
            for line in lines {
                if self.history.len() >= HISTORY_LIMIT {
                    self.history.pop_front();
                }
                self.history.push_back(line.clone());
                finalized.push(line);
            }
            self.consumed_chars += tail.chars().count() - new_tail.chars().count();
        }
        finalized
    }

    fn history(&self) -> Vec<String> {
        self.history.iter().cloned().collect()
    }

    fn tail(&self, definite: &str) -> String {
        let chars: Vec<char> = definite.chars().collect();
        let start = self.consumed_chars.min(chars.len());
        chars[start..].iter().collect()
    }
}

/// 说话人分轮跟踪器
struct SpeakerTracker {
    index: u32,
    label: String,
    last_text_at: Option<Instant>,
}

impl SpeakerTracker {
    fn new() -> Self {
        Self {
            index: 0,
            label: String::new(),
            last_text_at: None,
        }
    }

    fn feed(&mut self, has_text: bool) {
        if !has_text {
            return;
        }
        let new_turn = self
            .last_text_at
            .map_or(true, |t| t.elapsed() > Duration::from_millis(SPEAKER_TURN_GAP_MS));
        if new_turn {
            self.index = (self.index + 1).min(8);
            self.label = format!("说话人{}", self.index);
        }
        self.last_text_at = Some(Instant::now());
    }

    fn label(&self) -> &str {
        &self.label
    }
}

// ==================== 音源 Runner ====================

/// 一路音源（采集线程 + 豆包 WS + 音频泵 + 结果流）
struct SourceRunner {
    sample_rate: u32,
    result_rx: mpsc::Receiver<Result<AsrResponse>>,
    client: StreamingAsrClient,
    ws_task: JoinHandle<()>,
    pump_task: JoinHandle<()>,
    _stream_thread: std::thread::JoinHandle<()>,
    pcm_buffer: Arc<StdMutex<Vec<f32>>>,
    audio_seq: Arc<StdMutex<i32>>,
}

/// 启动一路音源：采集 → 豆包流式 ASR
async fn start_source(
    config: &Arc<ConfigManager>,
    audio_source: &str,
    device_name: &str,
    running: &Arc<AtomicBool>,
) -> Result<SourceRunner, String> {
    let pcm_buffer: Arc<StdMutex<Vec<f32>>> = Arc::new(StdMutex::new(Vec::new()));
    let audio_seq = Arc::new(StdMutex::new(2i32));

    let capture_buffer = pcm_buffer.clone();
    let running_thread = running.clone();
    let source_type = audio_source.to_string();
    let device = device_name.to_string();
    let (dm, sf, sr_pref, ch_pref) = config.effective_audio_prefs();

    let (stream_tx, stream_rx) = std::sync::mpsc::channel::<Result<u32, String>>();

    enum Capture {
        Cpal(cpal::Stream),
        Loopback(crate::audio::loopback::LoopbackCapture),
    }

    let stream_thread = std::thread::spawn(move || {
        let capture_result = if source_type == "system" {
            crate::audio::loopback::start_loopback_capture(capture_buffer, &dm)
                .map(|(sr, ch, h)| (sr, ch, Capture::Loopback(h)))
        } else {
            start_capture_with_prefs(
                capture_buffer,
                Some(&device),
                Some(&dm),
                Some(&sf),
                Some(&sr_pref),
                Some(&ch_pref),
            )
            .map(|(sr, ch, s)| (sr, ch, Capture::Cpal(s)))
        };

        match capture_result {
            Ok((sample_rate, _ch, handle)) => {
                let _ = stream_tx.send(Ok(sample_rate));
                while running_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                drop(handle);
            }
            Err(e) => {
                let _ = stream_tx.send(Err(format!("启动音频采集失败: {}", e)));
            }
        }
    });

    let sample_rate = stream_rx.recv().map_err(|_| "音频线程启动失败")??;

    let (result_tx, result_rx) = mpsc::channel(32);
    let (client, ws_task) = StreamingAsrClient::connect(config.clone(), result_tx)
        .await
        .map_err(|e| format!("连接豆包失败: {}", e))?;

    let pump_running = running.clone();
    let client_pump = client.clone();
    let pcm_buf_pump = pcm_buffer.clone();
    let seq_arc = audio_seq.clone();
    let pump_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(CHUNK_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if !pump_running.load(Ordering::Relaxed) {
                break;
            }
            let chunk_f32 = {
                let mut buf = match pcm_buf_pump.lock() {
                    Ok(b) => b,
                    Err(_) => break,
                };
                if buf.is_empty() {
                    continue;
                }
                // 安全上限：消费不及时则丢弃最旧样本，防止内存无限增长
                if buf.len() > MAX_PCM_BUFFER_SAMPLES {
                    let excess = buf.len() - MAX_PCM_BUFFER_SAMPLES;
                    buf.drain(..excess);
                }
                std::mem::take(&mut *buf)
            };
            if chunk_f32.is_empty() {
                continue;
            }
            let (pcm_i16, _) = resample_and_convert(&chunk_f32, sample_rate);
            if pcm_i16.is_empty() {
                continue;
            }
            let bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
            let seq = {
                let mut s = match seq_arc.lock() {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let cur = *s;
                *s += 1;
                cur
            };
            let _ = client_pump.send_audio(bytes, seq, false).await;
        }
    });

    Ok(SourceRunner {
        sample_rate,
        result_rx,
        client,
        ws_task,
        pump_task,
        _stream_thread: stream_thread,
        pcm_buffer,
        audio_seq,
    })
}

impl SourceRunner {
    /// 会话结束：发送最后一包、关闭连接、等待 WS 任务退出
    async fn finish(&mut self) {
        self.pump_task.abort();

        let rest: Vec<f32> = match self.pcm_buffer.lock() {
            Ok(mut buf) => std::mem::take(&mut *buf),
            Err(_) => Vec::new(),
        };
        let last_seq = *self.audio_seq.lock().unwrap();
        if !rest.is_empty() {
            let (pcm_i16, _) = resample_and_convert(&rest, self.sample_rate);
            let bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
            let _ = self.client.send_audio(bytes, last_seq, true).await;
        } else {
            let _ = self.client.send_audio(Vec::new(), last_seq, true).await;
        }
        self.client.close().await;

        let _ = tokio::time::timeout(Duration::from_secs(2), &mut self.ws_task).await;
    }
}

// ==================== 帧广播 ====================

pub type PipelineMap = HashMap<String, Option<Arc<TranslationPipeline>>>;

/// 向各场景字幕窗口广播一帧（窗口定向发送，避免主窗口反复接收无用数据）
#[allow(clippy::too_many_arguments)]
fn emit_frame(
    app: &AppHandle,
    scene_ids: &[String],
    text: Option<&str>,
    is_final: bool,
    a: &SourceText,
    b: &SourceText,
    dual: bool,
    translation_history: &[String],
    translation_current: &str,
) {
    for scene_id in scene_ids {
        let Some(window) = app.get_webview_window(&scene_window_label(scene_id)) else {
            continue;
        };
        // 隐藏窗口的 WebView 会被系统节流，持续投递事件会导致消息队列溢出（0x80070718）。
        // 隐藏窗口跳过，重新显示后由下一次刷新补上最新状态。
        if !window.is_visible().unwrap_or(false) {
            continue;
        }
        let _ = window.emit(
            "subtitle-text",
            serde_json::json!({
                "sceneId": scene_id,
                "text": text.unwrap_or(""),
                "isFinal": is_final,
                "definite": a.definite,
                "indefinite": a.indefinite,
                "history": a.history,
                "currentDefinite": a.tail,
                "speaker": a.speaker,
                "translationHistory": translation_history,
                "translationCurrent": translation_current,
                "dual": dual,
                "bText": b.text,
                "bDefinite": b.definite,
                "bIndefinite": b.indefinite,
                "bHistory": b.history,
                "bCurrentDefinite": b.tail,
                "bSpeaker": b.speaker,
            }),
        );
    }
}

/// 广播纯文本状态（连接提示/错误/清空），无历史与译文
pub fn push_status(app: &AppHandle, scene_ids: &[String], text: Option<&str>, is_final: bool) {
    let empty = SourceText::default();
    emit_frame(app, scene_ids, text, is_final, &empty, &empty, false, &[], "");
}

/// 按当前会话状态向所有场景发射完整帧（含每场景译文快照）
async fn emit_all(
    app: &AppHandle,
    scene_ids: &[String],
    asr_state: &Arc<Mutex<SessionAsr>>,
    pipelines: &Arc<Mutex<PipelineMap>>,
) {
    let st = asr_state.lock().await;
    let pps = pipelines.lock().await;
    for scene_id in scene_ids {
        let (t_history, t_current) = match pps.get(scene_id).and_then(|o| o.as_ref()) {
            Some(pl) => pl.snapshot().await,
            None => (Vec::new(), String::new()),
        };
        emit_frame(
            app,
            std::slice::from_ref(scene_id),
            Some(&st.text),
            st.is_final,
            &st.a,
            &st.b,
            st.dual,
            &t_history,
            &t_current,
        );
    }
}

// ==================== 会话 ====================

/// 运行字幕会话（单源或双音源同传）
pub async fn run_session(
    app: AppHandle,
    config: Arc<ConfigManager>,
    scene_ids: Vec<String>,
    running: Arc<AtomicBool>,
    stop_rx: &mut mpsc::Receiver<()>,
    transcript: Arc<StdMutex<Transcript>>,
) -> Result<(), String> {
    let subtitle_model = config.subtitle_model();
    if subtitle_model != STREAM_MODEL_DOUBAO {
        push_status(&app, &scene_ids, Some("请将字幕引擎设置为豆包云端流式识别"), false);
        wait_until_stop(running, stop_rx).await;
        return Ok(());
    }
    if config.get_doubao_api_key().is_empty() {
        push_status(&app, &scene_ids, Some("请先配置豆包 API Key"), false);
        wait_until_stop(running, stop_rx).await;
        return Ok(());
    }

    // 音源规划：dual = A 系统扬声器 + B 麦克风
    let raw_source = config.subtitle_audio_source();
    let dual = raw_source == "dual";
    let a_source = if dual { "system".to_string() } else { raw_source.clone() };
    let device_name = config.subtitle_input_device();

    // ===== 会话级状态 =====
    let asr_state: Arc<Mutex<SessionAsr>> = Arc::new(Mutex::new(SessionAsr {
        text: String::new(),
        is_final: false,
        a: SourceText::default(),
        b: SourceText::default(),
        dual,
    }));

    // ===== 每场景翻译流水线（仅 A 源）+ 合并发射脏标记 =====
    let dirty_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let cfg_snapshot = config.get_config();
    let translation_cache: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pipelines: Arc<Mutex<PipelineMap>> = Arc::new(Mutex::new(HashMap::new()));
    {
        let mut pps = pipelines.lock().await;
        for scene in &cfg_snapshot.subtitle.subtitle_scenes {
            if !scene_ids.contains(&scene.id) {
                continue;
            }
            let engine = resolve_engine(&scene.translation.engine);
            let pl = engine.map(|eng| {
                Arc::new(TranslationPipeline::new(
                    config.clone(),
                    Some(eng),
                    scene.translation.target_lang.clone(),
                    scene.translation.interim,
                    translation_cache.clone(),
                    dirty_flag.clone(),
                ))
            });
            pps.insert(scene.id.clone(), pl);
        }
    }

    // 合并发射器：按固定节拍发射最新帧状态（ASR 帧与译文落地都只置脏标记）。
    // 这是防止 PostMessage 队列溢出（0x80070718）导致卡死/内存暴涨的核心防线。
    {
        let app_d = app.clone();
        let scene_ids_d = scene_ids.clone();
        let asr_d = asr_state.clone();
        let pps_d = pipelines.clone();
        let transcript_d = transcript.clone();
        let running_d = running.clone();
        let dirty_d = dirty_flag.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(FRAME_FLUSH_MS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if dirty_d.swap(false, Ordering::Relaxed) {
                            sync_transcript_translations(&app_d, &transcript_d, &pps_d).await;
                            emit_all(&app_d, &scene_ids_d, &asr_d, &pps_d).await;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        if !running_d.load(Ordering::Relaxed) {
                            // 退出前补发最后一次状态
                            if dirty_d.swap(false, Ordering::Relaxed) {
                                sync_transcript_translations(&app_d, &transcript_d, &pps_d).await;
                                emit_all(&app_d, &scene_ids_d, &asr_d, &pps_d).await;
                            }
                            break;
                        }
                    }
                }
            }
        });
    }

    // 初始状态提示
    {
        let mut st = asr_state.lock().await;
        st.text = "正在连接语音服务...".to_string();
    }
    dirty_flag.store(true, Ordering::Relaxed);

    // ===== 启动音源 =====
    let mut runner_a = start_source(&config, &a_source, &device_name, &running).await?;
    let mut runner_b = if dual {
        Some(start_source(&config, "microphone", &device_name, &running).await?)
    } else {
        None
    };

    {
        let mut st = asr_state.lock().await;
        st.text = if dual {
            "同传模式已就绪：系统声音→译文，麦克风→副字幕".to_string()
        } else {
            "实时字幕已就绪，请开始说话".to_string()
        };
    }
    dirty_flag.store(true, Ordering::Relaxed);

    // ===== 消费循环 =====
    let mut tracker_a = LineTracker::new();
    let mut tracker_b = LineTracker::new();
    let mut speaker_a = SpeakerTracker::new();
    let app_work = app.clone();
    let asr_work = asr_state.clone();
    let pps_work = pipelines.clone();
    let transcript_work = transcript.clone();
    let dirty_work = dirty_flag.clone();

    loop {
        tokio::select! {
            _ = stop_rx.recv() => { break; }
            _ = tokio::time::sleep(Duration::from_millis(300)) => {
                if !running.load(Ordering::SeqCst) { break; }
            }
            msg_a = runner_a.result_rx.recv() => {
                match msg_a {
                    Some(Ok(resp)) => {
                        handle_frame("A", resp, &app_work, &asr_work, &pps_work, &transcript_work, &mut tracker_a, &mut tracker_b, &mut speaker_a).await;
                        dirty_work.store(true, Ordering::Relaxed);
                    }
                    Some(Err(e)) => log::error!("[字幕] A 源识别错误: {}", e),
                    None => {}
                }
            }
            msg_b = async {
                match runner_b.as_mut() {
                    Some(r) => r.result_rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match msg_b {
                    Some(Ok(resp)) => {
                        handle_frame("B", resp, &app_work, &asr_work, &pps_work, &transcript_work, &mut tracker_a, &mut tracker_b, &mut speaker_a).await;
                        dirty_work.store(true, Ordering::Relaxed);
                    }
                    Some(Err(e)) => log::error!("[字幕] B 源识别错误: {}", e),
                    None => {}
                }
            }
        }
    }

    // ===== 收尾 =====
    runner_a.finish().await;
    if let Some(rb) = runner_b.as_mut() {
        rb.finish().await;
    }

    running.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));

    Ok(())
}

/// 用主场景（default）翻译历史同步转录译文，并把变更增量推送给主窗口
async fn sync_transcript_translations(
    app: &AppHandle,
    transcript: &Arc<StdMutex<Transcript>>,
    pipelines: &Arc<Mutex<PipelineMap>>,
) {
    let history = {
        let pps = pipelines.lock().await;
        match pps.get("default").and_then(|o| o.as_ref()) {
            Some(pl) => pl.snapshot().await.0,
            None => Vec::new(),
        }
    };
    let changed = {
        if let Ok(mut tr) = transcript.lock() {
            tr.sync_translations(&history)
        } else {
            Vec::new()
        }
    };
    if !changed.is_empty() {
        let _ = app.emit(
            "subtitle-transcript-updated",
            serde_json::json!({ "type": "update", "updates": changed }),
        );
    }
}

/// 处理一帧 ASR 结果：更新源文本状态、转录记录、A 源逐场景翻译
#[allow(clippy::too_many_arguments)]
async fn handle_frame(
    source: &str,
    resp: AsrResponse,
    app: &AppHandle,
    asr_state: &Arc<Mutex<SessionAsr>>,
    pipelines: &Arc<Mutex<PipelineMap>>,
    transcript: &Arc<StdMutex<Transcript>>,
    tracker_a: &mut LineTracker,
    tracker_b: &mut LineTracker,
    speaker_a: &mut SpeakerTracker,
) {
    let d = resp.definite_text.trim().to_string();
    let i = resp.indefinite_text.trim().to_string();

    // 按源更新文本状态
    let finalized = {
        let mut st = asr_state.lock().await;
        let target = if source == "A" { &mut st.a } else { &mut st.b };
        target.text = resp.text.clone();
        target.definite = d.clone();
        target.indefinite = i.clone();
        let tracker = if source == "A" { tracker_a } else { tracker_b };
        let finalized = tracker.feed(&d);
        target.history = tracker.history();
        target.tail = tracker.tail(&d);
        if source == "B" {
            target.speaker = SOURCE_B_SPEAKER.to_string();
        } else {
            speaker_a.feed(!(d.is_empty() && i.is_empty()));
            target.speaker = speaker_a.label().to_string();
        }
        if source == "A" {
            st.text = resp.text.clone();
            st.is_final = resp.is_final;
        }
        finalized
    };

    // 转录记录（定稿句段）：只推送新增句段（增量），避免全量重发
    if !finalized.is_empty() {
        let speaker = if source == "A" {
            speaker_a.label().to_string()
        } else {
            SOURCE_B_SPEAKER.to_string()
        };
        let appended: Vec<_> = {
            let mut tr = transcript.lock().unwrap();
            finalized
                .iter()
                .map(|s| tr.push(source, &speaker, s))
                .collect()
        };
        let _ = app.emit(
            "subtitle-transcript-updated",
            serde_json::json!({ "type": "append", "segments": appended }),
        );
    }

    // A 源逐场景翻译
    if source == "A" {
        let mut pps = pipelines.lock().await;
        for pl in pps.values_mut() {
            if let Some(pl) = pl {
                pl.on_frame(&d, &i).await;
            }
        }
    }
}

/// 等待停止信号（无 ASR 会话的占位等待）
async fn wait_until_stop(running: Arc<AtomicBool>, stop_rx: &mut mpsc::Receiver<()>) {
    while running.load(Ordering::SeqCst) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = stop_rx.recv() => { break; }
        }
    }
}
