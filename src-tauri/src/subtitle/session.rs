//! 字幕会话：音源 → 豆包流式 ASR → 快照更新 → 信号。
//!
//! 与旧实现的本质差异：**没有逐帧广播，也没有 80ms 合并发射器**。
//! 任何状态变化由 [`SharedState::bump`] 触发一次轻量信号（只含类型+版本号），
//! 字幕窗口收到信号后按需拉取快照 —— 信号-拉取协议。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::Result;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::config::{ConfigManager, STREAM_MODEL_DOUBAO, PRIMARY_WINDOW_ID};
use crate::streaming::AsrResponse;
use crate::subtitle::audio::{start_source, SourceKind};
use crate::subtitle::state::{SharedState, Source};
use crate::subtitle::transcript::Transcript;
use crate::subtitle::translate::TranslationHub;

/// 运行字幕会话（单源或双音源同传）
pub async fn run_session(
    app: AppHandle,
    config: Arc<ConfigManager>,
    state: Arc<SharedState>,
    hub: Arc<TranslationHub>,
    running: Arc<AtomicBool>,
    stop_rx: &mut mpsc::Receiver<()>,
    transcript: Arc<StdMutex<Transcript>>,
) -> Result<(), String> {
    // ===== 前置校验 =====
    if config.subtitle_model() != STREAM_MODEL_DOUBAO {
        {
            let mut snap = state.write();
            snap.reset(false, "请将字幕引擎设置为豆包云端流式识别");
        }
        state.bump();
        wait_until_stop(&running, stop_rx).await;
        return Ok(());
    }
    if config.get_doubao_api_key().is_empty() {
        {
            let mut snap = state.write();
            snap.reset(false, "请先配置豆包 API Key");
        }
        state.bump();
        wait_until_stop(&running, stop_rx).await;
        return Ok(());
    }

    // 音源规划：dual = A 系统扬声器 + B 麦克风；system 单独模式 A 也走扬声器
    let raw_source = config.subtitle_audio_source();
    let dual = raw_source == "dual";
    let a_kind = match raw_source.as_str() {
        "system" | "dual" => SourceKind::System,
        _ => SourceKind::Microphone,
    };
    let device = config.subtitle_input_device();

    // ===== 初始状态 =====
    {
        let mut snap = state.write();
        snap.reset(dual, "正在连接语音服务...");
    }
    state.bump();

    // ===== 转录译文同步器：周期把主窗口译文历史写入转录 =====
    let sync_task = {
        let app_d = app.clone();
        let state_d = state.clone();
        let transcript_d = transcript.clone();
        let running_d = running.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(1000));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if !running_d.load(Ordering::Relaxed) {
                    break;
                }
                sync_transcript(&app_d, &transcript_d, &state_d);
            }
        })
    };

    // ===== 启动音源 =====
    let mut source_a = start_source(&config, a_kind, &device, &running).await?;
    let mut source_b = if dual {
        Some(start_source(&config, SourceKind::Microphone, &device, &running).await?)
    } else {
        None
    };

    {
        let mut snap = state.write();
        snap.status = if dual {
            "同传模式已就绪：系统声音→译文，麦克风→副字幕".to_string()
        } else {
            "实时字幕已就绪，请开始说话".to_string()
        };
    }
    state.bump();

    // ===== 消费循环 =====
    let app_work = app.clone();
    let state_work = state.clone();
    let hub_work = hub.clone();
    let transcript_work = transcript.clone();

    loop {
        tokio::select! {
            _ = stop_rx.recv() => { break; }
            _ = tokio::time::sleep(Duration::from_millis(300)) => {
                if !running.load(Ordering::SeqCst) { break; }
            }
            msg_a = source_a.results.recv() => {
                match msg_a {
                    Some(Ok(resp)) => {
                        handle_frame(Source::A, resp, &app_work, &state_work, &hub_work, &transcript_work).await;
                        state_work.bump();
                    }
                    Some(Err(e)) => log::error!("[字幕] A 源识别错误: {}", e),
                    None => {}
                }
            }
            msg_b = async {
                match source_b.as_mut() {
                    Some(r) => r.results.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match msg_b {
                    Some(Ok(resp)) => {
                        handle_frame(Source::B, resp, &app_work, &state_work, &hub_work, &transcript_work).await;
                        state_work.bump();
                    }
                    Some(Err(e)) => log::error!("[字幕] B 源识别错误: {}", e),
                    None => {}
                }
            }
        }
    }

    // ===== 收尾 =====
    source_a.finish().await;
    if let Some(rb) = source_b.as_mut() {
        rb.finish().await;
    }

    running.store(false, Ordering::SeqCst);
    sync_task.abort();

    // 等待在途翻译任务落地（最多 ~800ms），补一次最终同步
    tokio::time::sleep(Duration::from_millis(800)).await;
    sync_transcript(&app, &transcript, &state);

    {
        let mut snap = state.write();
        snap.running = false;
    }
    state.bump();
    std::thread::sleep(Duration::from_millis(200));

    Ok(())
}

/// 处理一帧 ASR 结果：更新快照、记录转录、喂入翻译枢纽
async fn handle_frame(
    source: Source,
    resp: AsrResponse,
    app: &AppHandle,
    state: &Arc<SharedState>,
    hub: &Arc<TranslationHub>,
    transcript: &Arc<StdMutex<Transcript>>,
) {
    let d = resp.definite_text.trim().to_string();
    let i = resp.indefinite_text.trim().to_string();

    let finalized = {
        let mut snap = state.write();
        snap.apply_frame(source, &d, &i, &resp.text)
    };

    // 转录记录（定稿句段）：只推送新增句段（增量）
    if !finalized.is_empty() {
        let speaker = {
            let snap = state.read();
            if source == Source::A {
                snap.a.speaker.clone()
            } else {
                snap.b.speaker.clone()
            }
        };
        let source_tag = if source == Source::A { "A" } else { "B" };
        let appended: Vec<_> = {
            let mut tr = transcript.lock().unwrap();
            finalized
                .iter()
                .map(|s| tr.push(source_tag, &speaker, s))
                .collect()
        };
        let _ = app.emit(
            "subtitle-transcript-updated",
            serde_json::json!({ "type": "append", "segments": appended }),
        );
    }

    // A 源逐窗口翻译
    if source == Source::A {
        hub.on_frame(&d, &i).await;
    }
}

/// 用主窗口（primary）的译文历史同步转录译文，并把变更增量推送给主窗口
fn sync_transcript(
    app: &AppHandle,
    transcript: &Arc<StdMutex<Transcript>>,
    state: &Arc<SharedState>,
) {
    let history = {
        let snap = state.read();
        snap.translation
            .get(PRIMARY_WINDOW_ID)
            .map(|v| v.history.clone())
            .unwrap_or_default()
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

/// 等待停止信号（无 ASR 会话的占位等待）
async fn wait_until_stop(running: &Arc<AtomicBool>, stop_rx: &mut mpsc::Receiver<()>) {
    while running.load(Ordering::SeqCst) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = stop_rx.recv() => { break; }
        }
    }
}
