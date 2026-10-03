//! 字幕会话：音源 → 豆包流式 ASR → 切分定稿 → 状态板 / 转录 / 同传 → 信号。
//!
//! 每帧：切分器产出定稿行 → 状态板分配 ID → 写转录并通知主界面 → A 源交给同传；
//! 当前行变化交给同声预览。每 250ms 一次节拍，让无标点的尾句按静止时长定稿。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::config::{ConfigManager, SubtitleWindow, STREAM_MODEL_DOUBAO};
use crate::streaming::AsrResponse;
use crate::subtitle::audio::{start_source, AsrSource, SourceKind};
use crate::subtitle::captions::{CaptionStream, Piece};
use crate::subtitle::state::{Caption, SharedState, Source};
use crate::subtitle::transcript::Transcript;
use crate::subtitle::translate::{TranslationSink, Translator};

/// 连续识别错误熔断阈值
const MAX_CONSEC_ASR_ERRORS: usize = 8;
/// 切分节拍
const TICK: Duration = Duration::from_millis(250);
/// 会话结束时等待在途翻译的上限
const DRAIN_TIMEOUT: Duration = Duration::from_millis(1500);
/// 翻译附带的上文句数
const CONTEXT_SENTENCES: usize = 2;

pub struct SessionCtx {
    pub app: AppHandle,
    pub config: Arc<ConfigManager>,
    pub state: Arc<SharedState>,
    pub transcript: Arc<StdMutex<Transcript>>,
    pub running: Arc<AtomicBool>,
    /// 本次会话启用的窗口（决定翻译语言）
    pub windows: Vec<SubtitleWindow>,
}

/// 运行字幕会话（单源或同传双源），直到收到停止信号或熔断
pub async fn run_session(ctx: SessionCtx, stop_rx: &mut mpsc::Receiver<()>) -> Result<(), String> {
    let state = ctx.state.clone();

    // ===== 前置校验：给出可操作的提示，并保持窗口显示直到用户停止 =====
    let precheck = if ctx.config.subtitle_model() != STREAM_MODEL_DOUBAO {
        Some("请在「设置 → 语音识别模型」中把字幕模型设为豆包（云端）")
    } else if ctx.config.get_doubao_api_key().trim().is_empty() {
        Some("请先在「设置 → API 密钥」中填写豆包 Key")
    } else {
        None
    };
    if let Some(msg) = precheck {
        state.write().reset(false, [String::new(), String::new()], msg);
        state.bump();
        wait_until_stop(&ctx.running, stop_rx).await;
        return Ok(());
    }

    // ===== 音源规划 =====
    let raw_source = ctx.config.subtitle_audio_source();
    let dual = raw_source == "dual";
    let (a_kind, labels) = match raw_source.as_str() {
        "dual" => (SourceKind::System, ["对方".to_string(), "我".to_string()]),
        "system" => (SourceKind::System, ["系统声音".to_string(), String::new()]),
        _ => (SourceKind::Microphone, ["麦克风".to_string(), String::new()]),
    };
    let device = ctx.config.subtitle_input_device();

    state.write().reset(dual, labels.clone(), "正在连接语音服务…");
    state.bump();

    // ===== 同传：译文落地 → 回填转录 + 通知主界面 =====
    let sink: TranslationSink = {
        let transcript = ctx.transcript.clone();
        let app = ctx.app.clone();
        Arc::new(move |id: u64, lang: &str, text: &str| {
            let found = transcript.lock().unwrap_or_else(|e| e.into_inner()).set_translation(id, lang, text);
            if found {
                let _ = app.emit(
                    "subtitle-transcript-updated",
                    serde_json::json!({ "type": "translation", "id": id, "lang": lang, "text": text }),
                );
            }
        })
    };
    let translator = Translator::build(ctx.config.clone(), state.clone(), &ctx.windows, sink);

    // ===== 启动音源 =====
    let mut source_a = match start_source(&ctx.config, a_kind, &device, &ctx.running).await {
        Ok(s) => s,
        Err(e) => return Err(e),
    };
    let mut source_b: Option<AsrSource> = if dual {
        match start_source(&ctx.config, SourceKind::Microphone, &device, &ctx.running).await {
            Ok(s) => Some(s),
            Err(e) => {
                source_a.finish().await;
                return Err(e);
            }
        }
    } else {
        None
    };

    {
        let mut b = state.write();
        b.status = if dual {
            "同传模式就绪：系统声音 → 原文与译文，麦克风 → 副字幕".to_string()
        } else if translator.is_empty() {
            "实时字幕已就绪，请开始说话".to_string()
        } else {
            "实时字幕与同声传译已就绪，请开始说话".to_string()
        };
    }
    state.bump();

    let clock = Instant::now();
    let now_ms = || clock.elapsed().as_millis() as u64;
    let mut streams = [CaptionStream::new(), CaptionStream::new()];
    let mut errors = [0usize; 2];
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = stop_rx.recv() => break,
            _ = ticker.tick() => {
                if !ctx.running.load(Ordering::SeqCst) {
                    break;
                }
                let t = now_ms();
                let mut changed = false;
                for source in [Source::A, Source::B] {
                    let pieces = streams[source.idx()].tick(t);
                    if !pieces.is_empty() {
                        let live = streams[source.idx()].live();
                        commit(&ctx, &translator, source, pieces, live);
                        changed = true;
                    }
                }
                if changed {
                    state.bump();
                }
            }
            msg = source_a.results.recv() => {
                if !on_message(&ctx, &translator, Source::A, msg, &mut streams, &mut errors, now_ms()).await {
                    break;
                }
            }
            msg = async {
                match source_b.as_mut() {
                    Some(s) => s.results.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if !on_message(&ctx, &translator, Source::B, msg, &mut streams, &mut errors, now_ms()).await {
                    break;
                }
            }
        }
    }

    // ===== 收尾：关闭音源 → 最后一句定稿 → 等译文落地 =====
    source_a.finish().await;
    if let Some(b) = source_b.as_mut() {
        b.finish().await;
    }
    let t = now_ms();
    for source in [Source::A, Source::B] {
        let pieces = streams[source.idx()].flush(t);
        if !pieces.is_empty() {
            commit(&ctx, &translator, source, pieces, Default::default());
        }
    }
    state.bump();
    translator.drain(DRAIN_TIMEOUT).await;
    translator.shutdown();

    state.write().running = false;
    state.bump();
    Ok(())
}

/// 处理一路识别结果；返回 false 表示需要结束会话（熔断）
async fn on_message(
    ctx: &SessionCtx,
    translator: &Translator,
    source: Source,
    msg: Option<anyhow::Result<AsrResponse>>,
    streams: &mut [CaptionStream; 2],
    errors: &mut [usize; 2],
    now: u64,
) -> bool {
    match msg {
        Some(Ok(resp)) => {
            errors[source.idx()] = 0;
            let stream = &mut streams[source.idx()];
            let pieces = stream.feed(resp.definite_text.trim(), resp.indefinite_text.trim(), now);
            let live = stream.live();
            commit(ctx, translator, source, pieces, live);
            ctx.state.bump();
            true
        }
        Some(Err(e)) => {
            errors[source.idx()] += 1;
            let n = errors[source.idx()];
            log::error!("[字幕] {} 源识别错误（{}/{}）: {}", source.tag(), n, MAX_CONSEC_ASR_ERRORS, e);
            if n >= MAX_CONSEC_ASR_ERRORS {
                let name = if source == Source::A { "主音源" } else { "麦克风副音源" };
                trip_breaker(&ctx.state, name, &e.to_string()).await;
                return false;
            }
            true
        }
        // 结果通道关闭：WS 任务已退出（服务端断开），等同熔断
        None => {
            let name = if source == Source::A { "主音源" } else { "麦克风副音源" };
            trip_breaker(&ctx.state, name, "识别连接已关闭").await;
            false
        }
    }
}

/// 写入定稿行与当前行：状态板分配 ID → 转录 → 主界面增量 → 同传
fn commit(ctx: &SessionCtx, translator: &Translator, source: Source, pieces: Vec<Piece>, live: crate::subtitle::captions::LiveLine) {
    let live_text = live.text();
    let (captions, contexts, speaker, live_gen) = {
        let mut b = ctx.state.write();
        let captions: Vec<Caption> = pieces.into_iter().map(|p| b.push_caption(source, p)).collect();
        b.live[source.idx()] = live;
        if source == Source::A {
            if let Some(last) = captions.last() {
                b.settle_live_translation(last.id);
            } else if live_text.is_empty() {
                for view in b.translations.values_mut() {
                    view.live.clear();
                }
            }
        }
        let contexts: Vec<Vec<String>> = captions.iter().map(|c| b.context_before(c.id, CONTEXT_SENTENCES)).collect();
        (captions, contexts, b.labels[source.idx()].clone(), b.live_gen)
    };

    if !captions.is_empty() {
        let entries: Vec<_> = {
            let mut tr = ctx.transcript.lock().unwrap_or_else(|e| e.into_inner());
            captions.iter().map(|c| tr.push(c, &speaker)).collect()
        };
        let _ = ctx.app.emit("subtitle-transcript-updated", serde_json::json!({ "type": "append", "entries": entries }));
    }

    if source == Source::A && !translator.is_empty() {
        for (c, context) in captions.iter().zip(contexts) {
            translator.on_caption(c, context);
        }
        translator.on_live(live_gen, &live_text);
    }
}

/// 识别连接持续失败：把原因写进状态板（字幕窗口会显示），停留片刻后结束会话
async fn trip_breaker(state: &Arc<SharedState>, source_name: &str, err: &str) {
    let brief: String = err.chars().take(120).collect();
    log::error!("[字幕] {} 识别连接持续失败，停止会话: {}", source_name, brief);
    {
        let mut b = state.write();
        b.status = format!("识别服务连接中断（{}），字幕已停止：{}", source_name, brief);
        b.running = false;
    }
    state.bump();
    tokio::time::sleep(Duration::from_secs(3)).await;
}

async fn wait_until_stop(running: &Arc<AtomicBool>, stop_rx: &mut mpsc::Receiver<()>) {
    while running.load(Ordering::SeqCst) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = stop_rx.recv() => break,
        }
    }
}
