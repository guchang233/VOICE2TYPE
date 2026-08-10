use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Mutex};

use crate::audio::processor::resample_and_convert;
use crate::config::{ConfigManager, STREAM_MODEL_DOUBAO};
use crate::streaming::{start_capture_with_prefs, AsrResponse, StreamingAsrClient};

const CHUNK_MS: u64 = 200;
const TARGET_RATE: u32 = 16_000;

pub struct SubtitleService {
    app_handle: Arc<Mutex<Option<AppHandle>>>,
    running: Arc<AtomicBool>,
    stop_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

impl SubtitleService {
    pub fn new() -> Self {
        Self {
            app_handle: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            stop_tx: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.lock().await = Some(handle);
    }

    pub async fn start(&self, config: Arc<ConfigManager>) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Err("字幕已在运行".to_string());
        }

        let handle = self.app_handle.lock().await;
        let app = handle.as_ref().ok_or("App handle not ready")?.clone();
        drop(handle);

        if let Some(window) = app.get_webview_window("subtitle") {
            let cfg = config.get_config();
            let sub_cfg = &cfg.subtitle;

            let _ = window.show();
            let _ = window.set_always_on_top(true);

            if sub_cfg.subtitle_window_x >= 0 && sub_cfg.subtitle_window_y >= 0 {
                let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition {
                    x: sub_cfg.subtitle_window_x as f64,
                    y: sub_cfg.subtitle_window_y as f64,
                }));
            }
            if sub_cfg.subtitle_window_width > 0 && sub_cfg.subtitle_window_height > 0 {
                let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
                    width: sub_cfg.subtitle_window_width as f64,
                    height: sub_cfg.subtitle_window_height as f64,
                }));
            }

            let _ = app.emit(
                "subtitle-config",
                serde_json::json!({
                    "fontFamily": sub_cfg.subtitle_font_family,
                    "fontSize": sub_cfg.subtitle_font_size,
                    "fontWeight": sub_cfg.subtitle_font_weight,
                    "bold": sub_cfg.subtitle_font_weight >= 700,
                    "italic": sub_cfg.subtitle_italic,
                    "fontColor": sub_cfg.subtitle_font_color,
                    "textAlign": sub_cfg.subtitle_text_align,
                    "letterSpacing": sub_cfg.subtitle_letter_spacing,
                    "lineHeight": sub_cfg.subtitle_line_height,
                    "textShadowColor": sub_cfg.subtitle_text_shadow_color,
                    "textShadowStrength": sub_cfg.subtitle_text_shadow_strength,
                    "bgColor": sub_cfg.subtitle_bg_color,
                    "bgOpacity": sub_cfg.subtitle_bg_opacity,
                    "blur": sub_cfg.subtitle_blur,
                    "borderRadius": sub_cfg.subtitle_border_radius,
                    "borderColor": sub_cfg.subtitle_border_color,
                    "borderWidth": sub_cfg.subtitle_border_width,
                    "paddingX": sub_cfg.subtitle_padding_x,
                    "paddingY": sub_cfg.subtitle_padding_y,
                    "maxLines": sub_cfg.subtitle_max_lines,
                    "interimColor": sub_cfg.subtitle_interim_color,
                    "interimOpacity": sub_cfg.subtitle_interim_opacity,
                }),
            );
        }

        self.running.store(true, Ordering::SeqCst);

        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        {
            let mut tx = self.stop_tx.lock().await;
            *tx = Some(stop_tx);
        }

        let running = self.running.clone();
        let app_handle = app.clone();

        tokio::spawn(async move {
            let subtitle_model = config.subtitle_model();

            if subtitle_model == STREAM_MODEL_DOUBAO {
                if config.get_doubao_api_key().is_empty() {
                    let _ = app_handle.emit(
                        "subtitle-text",
                        serde_json::json!({
                            "text": "请先配置豆包 API Key",
                            "isFinal": false
                        }),
                    );
                    while running.load(Ordering::SeqCst) {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                            _ = stop_rx.recv() => { break; }
                        }
                    }
                } else {
                    if let Err(e) =
                        run_doubao_streaming(app_handle.clone(), config.clone(), running.clone(), &mut stop_rx)
                            .await
                    {
                        log::error!("Subtitle doubao streaming error: {}", e);
                        let _ = app_handle.emit(
                            "subtitle-text",
                            serde_json::json!({
                                "text": format!("错误: {}", e),
                                "isFinal": false
                            }),
                        );
                    }
                }
            } else {
                let _ = app_handle.emit(
                    "subtitle-text",
                    serde_json::json!({
                        "text": "请将字幕引擎设置为豆包云端流式识别",
                        "isFinal": false
                    }),
                );
                while running.load(Ordering::SeqCst) {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                        _ = stop_rx.recv() => { break; }
                    }
                }
            }

            running.store(false, Ordering::SeqCst);

            if let Some(window) = app_handle.get_webview_window("subtitle") {
                let _ = window.hide();
            }
            let _ = app_handle.emit(
                "subtitle-text",
                serde_json::json!({
                    "text": "",
                    "isFinal": true
                }),
            );
        });

        Ok(())
    }

    pub async fn stop(&self) -> Result<(), String> {
        self.running.store(false, Ordering::SeqCst);

        let mut tx = self.stop_tx.lock().await;
        if let Some(sender) = tx.take() {
            let _ = sender.send(()).await;
        }

        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

async fn run_doubao_streaming(
    app: AppHandle,
    config: Arc<ConfigManager>,
    running: Arc<AtomicBool>,
    stop_rx: &mut mpsc::Receiver<()>,
) -> Result<(), String> {
    let _ = app.emit(
        "subtitle-text",
        serde_json::json!({
            "text": "正在连接语音服务...",
            "isFinal": false
        }),
    );

    let pcm_buffer: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let audio_seq = Arc::new(std::sync::Mutex::new(2i32));

    let buffer_for_later = pcm_buffer.clone();
    let running_thread = running.clone();

    let (stream_tx, stream_rx) = std::sync::mpsc::channel::<Result<u32, String>>();

    // 在 move 进 thread 之前读取音源 + 有效音频偏好
    let subtitle_device_name = config.subtitle_input_device();
    let (dm, sf, sr_pref, ch_pref) = config.effective_audio_prefs();

    let _stream_thread = std::thread::spawn(move || {
        // 直接复用 streaming 模块的 start_capture_with_prefs（含配置选取 / U8 过滤 / 下混策略）
        match start_capture_with_prefs(
            pcm_buffer,
            Some(&subtitle_device_name),
            Some(&dm),
            Some(&sf),
            Some(&sr_pref),
            Some(&ch_pref),
        ) {
            Ok((sample_rate, _ch, stream)) => {
                let _ = stream_tx.send(Ok(sample_rate));
                // 保持存活直到 running=false
                while running_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                // 显式 drop stream 后线程退出
                drop(stream);
            }
            Err(e) => {
                let _ = stream_tx.send(Err(format!("启动字幕音频采集失败: {}", e)));
            }
        }
    });

    let sample_rate = stream_rx.recv().map_err(|_| "音频线程启动失败")??;

    let (result_tx, mut result_rx) = mpsc::channel(32);
    let (client, ws_task) = StreamingAsrClient::connect(config.clone(), result_tx)
        .await
        .map_err(|e| format!("连接豆包失败: {}", e))?;

    let _ = app.emit(
        "subtitle-text",
        serde_json::json!({
            "text": "实时字幕已就绪，请开始说话",
            "isFinal": false
        }),
    );

    let app_result = app.clone();
    let result_task = tokio::spawn(async move {
        let mut final_text = String::new();
        while let Some(msg) = result_rx.recv().await {
            match msg {
                Ok(AsrResponse { text, is_final }) => {
                    let display = if is_final {
                        let t = text.clone();
                        final_text = t.clone();
                        t
                    } else {
                        if final_text.is_empty() {
                            text
                        } else {
                            format!("{} {}", final_text, text)
                        }
                    };
                    let _ = app_result.emit(
                        "subtitle-text",
                        serde_json::json!({
                            "text": display,
                            "isFinal": is_final
                        }),
                    );
                    if is_final {
                        final_text.clear();
                    }
                }
                Err(e) => {
                    log::error!("[字幕] 识别错误: {}", e);
                }
            }
        }
    });

    let pump_running = running.clone();
    let client_pump = client.clone();
    let pcm_buf_pump = buffer_for_later.clone();
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

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(300)) => {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
            }
            _ = stop_rx.recv() => {
                break;
            }
        }
    }

    pump_task.abort();

    let rest: Vec<f32> = match buffer_for_later.lock() {
        Ok(mut buf) => std::mem::take(&mut *buf),
        Err(_) => Vec::new(),
    };

    let last_seq = *audio_seq.lock().unwrap();
    if !rest.is_empty() {
        let (pcm_i16, _) = resample_and_convert(&rest, sample_rate);
        let bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
        let _ = client.send_audio(bytes, last_seq, true).await;
    } else {
        let _ = client.send_audio(Vec::new(), last_seq, true).await;
    }
    client.close().await;

    running.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));

    let _ = tokio::time::timeout(Duration::from_secs(2), ws_task).await;
    result_task.abort();

    Ok(())
}
