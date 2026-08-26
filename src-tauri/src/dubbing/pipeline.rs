//! 视频配音流水线：节点化编排，分两阶段执行以支持字幕人工编辑。
//!
//! ```text
//! 阶段一（识别）：[视频输入]→[提取节点]──▶(cap2)──▶[识别节点] ──返回带词级时间戳的字幕
//! 阶段二（生成）：[编辑后字幕]→[合成节点]→[混流节点]──输出视频+SRT
//! ```
//!
//! - 两阶段之间前端可在「字幕编辑」节点修改文本/时间、按词级时间戳重新分段
//! - 有界通道限制在途数据量；识别期间逐块推送增量转写预览
//! - 进度权重：阶段一 extract 5-15% / asr 15-45%；阶段二 tts 5-85% / mux 85-100%

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use crate::config::{ConfigManager, TtsConfig};
use crate::dubbing::{asr_ali, ffmpeg, transcribe, tts_segments, DubSegment};
use crate::tts::client::FishTtsClient;

const CANCELLED_SENTINEL: &str = "__cancelled__";

static DUBBING_RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL_DUBBING: AtomicBool = AtomicBool::new(false);

/// 音频分块时长（秒）：64kbps 单声道下每块约 4.8MB，远低于各云厂商上传限额
const DEFAULT_CHUNK_SECONDS: u64 = 600;
/// 提取 → 识别 通道容量（在途分块数）
const CHUNK_CHANNEL_CAP: usize = 2;

// ===================== 选项结构 =====================

/// 阶段一选项：识别引擎与其参数
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PrepareOptions {
    /// `ali-dashscope`（默认）/ `global-compat`（跟随整段识别配置）
    #[serde(default)]
    pub asr_provider: Option<String>,
    /// 词级时间戳（阿里专有，开启后前端可精确重新分段）
    #[serde(default)]
    pub ali_enable_words: Option<bool>,
    /// 逆文本规范化（数字转阿拉伯数字等）
    #[serde(default)]
    pub ali_enable_itn: Option<bool>,
    /// 语种提示（空/auto = 自动检测）
    #[serde(default)]
    pub ali_language: Option<String>,
    /// 音频分块时长（秒）
    #[serde(default)]
    pub chunk_seconds: Option<u64>,
}

/// Fish Audio 合成参数覆盖（未提供的字段沿用「语音合成」页保存值）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TtsOverrides {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub reference_title: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub volume: Option<f32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub latency: Option<String>,
    #[serde(default)]
    pub chunk_length: Option<u32>,
    #[serde(default)]
    pub normalize: Option<bool>,
}

impl TtsOverrides {
    fn apply_to(&self, cfg: &mut TtsConfig) {
        if let Some(v) = &self.model {
            cfg.model = v.clone();
        }
        if let Some(v) = &self.reference_id {
            cfg.reference_id = v.clone();
        }
        if let Some(v) = self.speed {
            cfg.speed = v;
        }
        if let Some(v) = self.volume {
            cfg.volume = v;
        }
        if let Some(v) = self.temperature {
            cfg.temperature = v;
        }
        if let Some(v) = self.top_p {
            cfg.top_p = v;
        }
        if let Some(v) = &self.latency {
            cfg.latency = v.clone();
        }
        if let Some(v) = self.chunk_length {
            cfg.chunk_length = v;
        }
        if let Some(v) = self.normalize {
            cfg.normalize = v;
        }
    }
}

/// 阶段二选项：输出目录与合成参数
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GenerateOptions {
    #[serde(default)]
    pub output_dir: Option<String>,
    #[serde(default)]
    pub tts: Option<TtsOverrides>,
}

/// 提取 → 识别 节点间的分块消息
struct ChunkMsg {
    index: usize,
    path: PathBuf,
}

// ===================== 运行锁与取消 =====================

pub fn is_running() -> bool {
    DUBBING_RUNNING.load(Ordering::SeqCst)
}

pub fn request_cancel() {
    CANCEL_DUBBING.store(true, Ordering::SeqCst);
}

/// 供 ffmpeg 下载等子模块轮询取消标志
pub fn is_cancel_requested() -> bool {
    CANCEL_DUBBING.load(Ordering::SeqCst)
}

fn is_cancelled() -> bool {
    CANCEL_DUBBING.load(Ordering::SeqCst)
}

fn check_cancel() -> Result<()> {
    if is_cancelled() {
        return Err(anyhow!("{}", CANCELLED_SENTINEL));
    }
    Ok(())
}

fn acquire_slot() -> Result<(), String> {
    if DUBBING_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("已有配音任务正在进行".to_string());
    }
    CANCEL_DUBBING.store(false, Ordering::SeqCst);
    Ok(())
}

// ===================== 进度事件 =====================

fn emit_progress(
    app: &tauri::AppHandle,
    stage: &str,
    label: &str,
    percent: u32,
    message: &str,
    current: Option<usize>,
    total: Option<usize>,
) {
    let _ = app.emit(
        "dubbing-progress",
        serde_json::json!({
            "stage": stage,
            "label": label,
            "status": "running",
            "percent": percent.clamp(0, 99),
            "message": message,
            "current": current,
            "total": total,
        }),
    );
}

fn emit_done(app: &tauri::AppHandle, payload: serde_json::Value) {
    let _ = app.emit(
        "dubbing-progress",
        serde_json::json!({
            "stage": "done",
            "label": "完成",
            "status": "done",
            "percent": 100,
            "message": "完成",
            "result": payload,
        }),
    );
}

fn emit_error(app: &tauri::AppHandle, message: &str) {
    let _ = app.emit(
        "dubbing-progress",
        serde_json::json!({
            "stage": "error",
            "label": "失败",
            "status": "error",
            "percent": 100,
            "message": message,
        }),
    );
}

fn emit_cancelled(app: &tauri::AppHandle) {
    let _ = app.emit(
        "dubbing-progress",
        serde_json::json!({
            "stage": "cancelled",
            "label": "已取消",
            "status": "cancelled",
            "percent": 0,
            "message": "任务已取消",
        }),
    );
}

/// 包装任务收尾：释放运行锁并按结果发送终态事件
async fn finish_job(app: tauri::AppHandle, result: Result<serde_json::Value>) {
    DUBBING_RUNNING.store(false, Ordering::SeqCst);
    match result {
        Ok(payload) => emit_done(&app, payload),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains(CANCELLED_SENTINEL) {
                log::info!("[dubbing] 任务已取消");
                emit_cancelled(&app);
            } else {
                log::error!("[dubbing] 任务失败: {}", msg);
                emit_error(&app, &msg);
            }
        }
    }
}

/// 任务临时目录守卫：销毁时清理中间产物（分块音频、配音音轨等）
struct TempGuard(PathBuf);

impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ===================== 阶段一：识别字幕 =====================

/// 启动识别任务（提取音轨 + 云端 ASR），完成后经 done 事件返回分段。
pub fn spawn_prepare(
    app: tauri::AppHandle,
    config: Arc<ConfigManager>,
    video_path: String,
    options: PrepareOptions,
) -> Result<(), String> {
    acquire_slot()?;
    let app2 = app.clone();
    // 同步命令运行在主线程事件循环中，必须用 tauri::async_runtime::spawn
    tauri::async_runtime::spawn(async move {
        let result = run_prepare(&app2, config, video_path, options).await;
        finish_job(app2, result).await;
    });
    Ok(())
}

async fn run_prepare(
    app: &tauri::AppHandle,
    config: Arc<ConfigManager>,
    video_path: String,
    options: PrepareOptions,
) -> Result<serde_json::Value> {
    let video = PathBuf::from(&video_path);
    if !video.is_file() {
        return Err(anyhow!("视频文件不存在: {}", video_path));
    }
    let chunk_seconds = options
        .chunk_seconds
        .unwrap_or(DEFAULT_CHUNK_SECONDS)
        .clamp(60, 3600);

    // ===== 准备节点：ffmpeg 就绪 =====
    emit_progress(
        app,
        "prepare",
        "准备工具",
        0,
        "正在定位 ffmpeg...",
        None,
        None,
    );
    let models_dir = config.current_models_dir();
    let ff = ffmpeg::ensure_ffmpeg(&models_dir, Some(app))
        .await
        .map_err(|e| anyhow!("ffmpeg 不可用且自动下载失败：{}", e))?;
    check_cancel()?;

    // ===== 提取节点 =====
    emit_progress(
        app,
        "extract",
        "提取音轨",
        5,
        "正在探测视频信息...",
        None,
        None,
    );
    let duration_secs = spawn_ffmpeg(ff.clone(), video.clone(), |ff, video| {
        ffmpeg::probe_duration(ff, video)
    })
    .await?;
    let est_chunks = ((duration_secs / chunk_seconds as f64).ceil() as usize).max(1);
    check_cancel()?;
    emit_progress(
        app,
        "extract",
        "提取音轨",
        7,
        &format!("视频时长 {}，正在提取音轨...", fmt_duration(duration_secs)),
        None,
        None,
    );

    let temp_dir = config
        .config_dir()
        .join("dubbing_tmp")
        .join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .context("创建配音工作目录失败")?;
    let _temp_guard = TempGuard(temp_dir.clone());

    // 识别节点先启动等待上游消息，随后由提取节点喂数据
    let (chunk_tx, chunk_rx) = mpsc::channel::<ChunkMsg>(CHUNK_CHANNEL_CAP);

    let asr_backend = resolve_asr_backend(&config, options.asr_provider.as_deref())?;
    log::info!(
        "[dubbing] ASR 引擎: {}",
        match &asr_backend {
            AsrBackend::AliDashScope { .. } => "阿里云百炼 (qwen3-asr-flash-filetrans)",
            AsrBackend::GlobalCompat => "全局整段识别配置",
        }
    );

    let ali_opts = asr_ali::AliAsrOptions {
        enable_itn: options.ali_enable_itn.unwrap_or(true),
        enable_words: options.ali_enable_words.unwrap_or(true),
        language: options.ali_language.unwrap_or_default(),
    };

    let asr_app = app.clone();
    let asr_config = config.clone();
    let asr_node = tauri::async_runtime::spawn(asr_node(
        chunk_rx,
        asr_app,
        asr_config,
        asr_backend,
        ali_opts,
        est_chunks,
        chunk_seconds,
    ));

    // 提取节点（本地 ffmpeg，速度快）：产出分块后按序喂给识别节点
    let extract_result = {
        let ff2 = ff.clone();
        let v2 = video.clone();
        let td = temp_dir.clone();
        tokio::task::spawn_blocking(move || {
            ffmpeg::extract_audio_chunks(&ff2, &v2, &td, chunk_seconds)
        })
        .await
        .map_err(|e| anyhow!("提取音轨任务执行失败: {}", e))?
        .and_then(|_| collect_chunk_paths(&temp_dir))
    };

    let chunks = match extract_result {
        Ok(paths) => paths,
        Err(e) => {
            drop(chunk_tx);
            let _ = asr_node.await;
            return Err(e);
        }
    };
    emit_progress(app, "extract", "提取音轨", 15, "音轨提取完成", None, None);

    for (i, path) in chunks.into_iter().enumerate() {
        if chunk_tx.send(ChunkMsg { index: i, path }).await.is_err() {
            break;
        }
    }
    drop(chunk_tx);

    // 识别节点直接返回分段（含词级时间戳，供前端编辑/重新分段）
    let (processed_chunks, segments) = asr_node
        .await
        .map_err(|e| anyhow!("识别节点异常退出: {}", e))??;

    if is_cancelled() {
        return Err(anyhow!("{}", CANCELLED_SENTINEL));
    }

    log::info!(
        "[dubbing] 识别完成：{} 块音频 / {} 个分段",
        processed_chunks,
        segments.len()
    );

    Ok(serde_json::json!({ "phase": "prepare", "segments": segments }))
}

// ===================== 识别节点 =====================

#[derive(Debug, Clone)]
enum AsrBackend {
    /// 阿里云百炼录音文件转写（句/词级毫秒时间戳，支持超长音频）
    AliDashScope { api_key: String },
    /// OpenAI 兼容 /v1/audio/transcriptions（verbose_json → srt 回退）
    GlobalCompat,
}

fn resolve_asr_backend(config: &ConfigManager, provider: Option<&str>) -> Result<AsrBackend> {
    let provider = provider.unwrap_or_default();
    let ali_key = config.get_dashscope_api_key();
    match provider {
        crate::config::DUBBING_ASR_GLOBAL => Ok(AsrBackend::GlobalCompat),
        _ => {
            if !ali_key.is_empty() {
                Ok(AsrBackend::AliDashScope { api_key: ali_key })
            } else if provider.is_empty() || provider == crate::config::DUBBING_ASR_ALI {
                log::warn!("[dubbing] 未配置阿里云百炼 Key，回退为全局整段识别配置");
                Ok(AsrBackend::GlobalCompat)
            } else {
                Err(anyhow!("未知配音引擎: {}", provider))
            }
        }
    }
}

async fn asr_node(
    mut rx: mpsc::Receiver<ChunkMsg>,
    app: tauri::AppHandle,
    config: Arc<ConfigManager>,
    backend: AsrBackend,
    ali_opts: asr_ali::AliAsrOptions,
    est_chunks: usize,
    chunk_seconds: u64,
) -> Result<(usize, Vec<DubSegment>)> {
    let mut processed: usize = 0;
    let mut cumulative: usize = 0;
    let mut collected: Vec<DubSegment> = Vec::new();

    while let Some(msg) = rx.recv().await {
        if is_cancelled() {
            return Err(anyhow!("{}", CANCELLED_SENTINEL));
        }
        processed += 1;
        let pct = 15 + ((processed * 30 / est_chunks.max(1)).min(30) as u32);
        emit_progress(
            &app,
            "asr",
            "识别字幕",
            pct,
            &format!("识别第 {} 段音频...", processed),
            Some(processed),
            None,
        );

        let chunk_name = format!("chunk_{:04}.mp3", msg.index);
        let mut part = match &backend {
            AsrBackend::AliDashScope { api_key } => {
                asr_ali::transcribe_chunk(&msg.path, api_key, &chunk_name, &ali_opts, is_cancelled)
                    .await?
            }
            AsrBackend::GlobalCompat => transcribe::transcribe_file(&msg.path, &config).await?,
        };

        // 分块内相对时间 → 全局绝对时间，并统一重排索引
        let offset_ms = (msg.index as u64) * chunk_seconds * 1000;
        for seg in part.iter_mut() {
            seg.start_ms += offset_ms;
            seg.end_ms += offset_ms;
            seg.index = cumulative + seg.index;
        }
        cumulative += part.len();

        // 实时推送增量转写结果给前端预览
        let _ = app.emit(
            "dubbing-transcript",
            serde_json::json!({ "added": &part, "completed": processed, "cumulative": cumulative }),
        );
        collected.extend(part);
    }

    Ok((processed, collected))
}

// ===================== 阶段二：生成配音 =====================

/// 启动生成任务（逐段 TTS + 时间轴拼装 + 替换原声导出）。
pub fn spawn_generate(
    app: tauri::AppHandle,
    config: Arc<ConfigManager>,
    video_path: String,
    segments: Vec<DubSegment>,
    options: GenerateOptions,
) -> Result<(), String> {
    acquire_slot()?;
    if segments.is_empty() {
        return Err("字幕为空，请先执行「识别字幕」或手动添加分段".to_string());
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = run_generate(&app2, config, video_path, segments, options).await;
        finish_job(app2, result).await;
    });
    Ok(())
}

async fn run_generate(
    app: &tauri::AppHandle,
    config: Arc<ConfigManager>,
    video_path: String,
    mut segments: Vec<DubSegment>,
    options: GenerateOptions,
) -> Result<serde_json::Value> {
    let video = PathBuf::from(&video_path);
    if !video.is_file() {
        return Err(anyhow!("视频文件不存在: {}", video_path));
    }
    for (i, seg) in segments.iter_mut().enumerate() {
        seg.index = i;
    }
    let seg_count = segments.len();

    // ===== 准备：ffmpeg 就绪（混流需要）=====
    emit_progress(
        app,
        "prepare",
        "准备工具",
        0,
        "正在定位 ffmpeg...",
        None,
        None,
    );
    let models_dir = config.current_models_dir();
    let ff = ffmpeg::ensure_ffmpeg(&models_dir, Some(app))
        .await
        .map_err(|e| anyhow!("ffmpeg 不可用且自动下载失败：{}", e))?;
    check_cancel()?;

    let temp_dir = config
        .config_dir()
        .join("dubbing_tmp")
        .join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .context("创建配音工作目录失败")?;
    let temp_guard = TempGuard(temp_dir.clone());

    // ===== 合成节点：TTS + 时间轴拼装 =====
    let track_wav = temp_dir.join("dub_track.wav");
    let mut tts_cfg = config.tts_config();
    if let Some(ov) = &options.tts {
        ov.apply_to(&mut tts_cfg);
    }
    let duration_hint = segments.last().map(|s| s.end_ms).unwrap_or(0);
    emit_progress(
        app,
        "tts",
        "语音合成",
        5,
        &format!("共 {} 个分段待合成", seg_count),
        None,
        Some(seg_count),
    );

    let (ok_n, fail_n, fit_n, cut_n) =
        run_tts_segments(app, &ff, &temp_dir, &segments, &track_wav, &tts_cfg).await?;
    check_cancel()?;
    log::info!(
        "[dubbing] TTS 完成：成功 {} 段，失败 {} 段，atempo 精确贴合 {} 段，截断回退 {} 段",
        ok_n,
        fail_n,
        fit_n,
        cut_n
    );

    // ===== 混流节点 =====
    let output = resolve_output_path(&video, options.output_dir.as_deref())?;
    emit_progress(
        app,
        "mux",
        "合成视频",
        90,
        "正在替换音轨并导出...",
        None,
        None,
    );
    {
        let ff2 = ff.clone();
        let v2 = video.clone();
        let tw = track_wav.clone();
        let out2 = output.clone();
        tokio::task::spawn_blocking(move || ffmpeg::mux_replace_audio(&ff2, &v2, &tw, &out2))
            .await
            .map_err(|e| anyhow!("混流任务执行失败: {}", e))??;
    }

    // 导出 SRT 字幕文件（与输出视频同目录同名）
    let srt_path = output.with_extension("srt");
    if let Err(e) = std::fs::write(&srt_path, transcribe::segments_to_srt(&segments)) {
        log::warn!("[dubbing] SRT 导出失败: {}", e);
    }

    drop(temp_guard);

    let _ = duration_hint;
    Ok(serde_json::json!({
        "phase": "generate",
        "output": output.to_string_lossy(),
        "subtitle": srt_path.to_string_lossy(),
        "segments": seg_count,
        "failed_segments": fail_n,
        "fitted_segments": fit_n,
        "truncated_segments": cut_n,
    }))
}

/// 顺序执行逐段 TTS：预估语速单次合成 → atempo 精确拉伸到槽位时长 → 按原起点写入。
/// 返回（成功/失败/拉伸贴合/截断回退）计数。
async fn run_tts_segments(
    app: &tauri::AppHandle,
    ff: &std::path::Path,
    temp_dir: &std::path::Path,
    segments: &[DubSegment],
    track_path: &PathBuf,
    tts_cfg: &TtsConfig,
) -> Result<(usize, usize, usize, usize)> {
    let client = FishTtsClient::new();
    let base_speed = tts_cfg.speed.clamp(0.5, 2.0);
    let cfg = tts_segments::wav_tts_config(tts_cfg);

    let mut writer = tts_segments::TimelineWriter::create(track_path)?;
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut stretched = 0usize;
    let mut truncated = 0usize;
    let mut consec_fail = 0usize;
    const MAX_INITIAL_CONSEC_FAILS: usize = 3;
    let total = segments.len();

    for seg in segments {
        if is_cancelled() {
            return Err(anyhow!("{}", CANCELLED_SENTINEL));
        }
        let pct = 5 + ((ok * 80 / total.max(1)).min(83) as u32);
        let preview: String = seg.text.chars().take(24).collect();
        emit_progress(
            app,
            "tts",
            "语音合成",
            pct,
            &format!("合成 {}/{}: {}...", ok + failed + 1, total, preview),
            Some(ok + failed),
            Some(total),
        );

        let slot = seg.duration_ms();
        let speed = tts_segments::estimate_fit_speed(&seg.text, slot, base_speed);
        match tts_segments::synthesize_segment(&client, &cfg, &seg.text, speed).await {
            Ok(raw) => {
                // atempo 精确拉伸/压缩到槽位时长（同步子进程，放到阻塞线程）
                let (audio, fit) = {
                    let ff2 = ff.to_path_buf();
                    let td = temp_dir.to_path_buf();
                    let idx = seg.index;
                    tokio::task::spawn_blocking(move || {
                        tts_segments::fit_to_slot(&ff2, &td, idx, raw, slot)
                    })
                    .await
                    .map_err(|e| anyhow!("贴合任务执行失败: {}", e))?
                };
                writer.write_at(seg.start_ms, &audio)?;
                ok += 1;
                if fit.stretched {
                    stretched += 1;
                }
                if fit.truncated {
                    truncated += 1;
                }
                consec_fail = 0;
            }
            Err(e) => {
                log::warn!("[dubbing] 第 {} 段合成失败，保留静音: {}", seg.index + 1, e);
                failed += 1;
                consec_fail += 1;
                if ok == 0 && consec_fail >= MAX_INITIAL_CONSEC_FAILS {
                    return Err(anyhow!(
                        "前 {} 段语音合成全部失败（最后错误：{}）。请检查「语音合成」页的音色选择与 Fish Audio API Key 后重试",
                        consec_fail,
                        brief_msg(&e.to_string())
                    ));
                }
            }
        }
    }

    // 总时长取最后一段结束时间与游标的较大者（贴合后两者应几乎相等）
    let last_end = segments.last().map(|s| s.end_ms).unwrap_or(0);
    let total_ms = writer.cursor_ms().max(last_end);
    writer.finish(total_ms)?;
    Ok((ok, failed, stretched, truncated))
}

// ===================== 辅助 =====================

fn brief_msg(msg: &str) -> String {
    msg.chars().take(160).collect()
}

/// 运行一个同步 ffmpeg 子命令封装（避免阻塞异步运行时）
async fn spawn_ffmpeg<T, F>(ff: PathBuf, video: PathBuf, f: F) -> Result<T>
where
    F: FnOnce(&PathBuf, &PathBuf) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || f(&ff, &video))
        .await
        .map_err(|e| anyhow!("ffmpeg 任务执行失败: {}", e))?
}

/// 收集提取节点产出的分块路径（按序）
fn collect_chunk_paths(dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut chunks: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mp3"))
                .unwrap_or(false)
        })
        .collect();
    chunks.sort();
    if chunks.is_empty() {
        return Err(anyhow!("音轨提取结果为空，该视频可能没有音频轨道"));
    }
    Ok(chunks)
}

/// 计算输出路径：<output_dir|视频目录>/<stem>_dubbed.mp4（重名追加序号）
fn resolve_output_path(video: &std::path::Path, output_dir: Option<&str>) -> Result<PathBuf> {
    let parent = match output_dir.filter(|s| !s.trim().is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => video
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("无法确定输出目录"))?,
    };
    std::fs::create_dir_all(&parent).context("创建输出目录失败")?;
    let stem = video
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video");
    let mut out = parent.join(format!("{}_dubbed.mp4", stem));
    let mut n = 1;
    while out.exists() {
        n += 1;
        out = parent.join(format!("{}_dubbed_{}.mp4", stem, n));
    }
    Ok(out)
}

fn fmt_duration(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}
