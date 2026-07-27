use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Mutex};

use crate::audio::processor::resample_and_convert;
use crate::config::{ConfigManager, STREAM_MODEL_DOUBAO};
use crate::streaming::{AsrResponse, StreamingAsrClient};

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

            let _ = app.emit("subtitle-config", serde_json::json!({
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
            }));
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
                    let _ = app_handle.emit("subtitle-text", serde_json::json!({
                        "text": "请先配置豆包 API Key",
                        "isFinal": false
                    }));
                    while running.load(Ordering::SeqCst) {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                            _ = stop_rx.recv() => { break; }
                        }
                    }
                } else {
                    if let Err(e) = run_doubao_streaming(app_handle.clone(), config.clone(), running.clone(), &mut stop_rx).await {
                        log::error!("Subtitle doubao streaming error: {}", e);
                        let _ = app_handle.emit("subtitle-text", serde_json::json!({
                            "text": format!("错误: {}", e),
                            "isFinal": false
                        }));
                    }
                }
            } else {
                let _ = app_handle.emit("subtitle-text", serde_json::json!({
                    "text": "请将字幕引擎设置为豆包云端流式识别",
                    "isFinal": false
                }));
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
            let _ = app_handle.emit("subtitle-text", serde_json::json!({
                "text": "",
                "isFinal": true
            }));
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
    let _ = app.emit("subtitle-text", serde_json::json!({
        "text": "正在连接语音服务...",
        "isFinal": false
    }));

    let pcm_buffer: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let audio_seq = Arc::new(std::sync::Mutex::new(2i32));

    let buffer_for_thread = pcm_buffer.clone();
    let buffer_for_later = pcm_buffer.clone();
    let running_thread = running.clone();
    let (stream_tx, stream_rx) = std::sync::mpsc::channel::<Result<u32, String>>();

    let _stream_thread = std::thread::spawn(move || {
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => {
                let _ = stream_tx.send(Err("未找到音频输入设备".to_string()));
                return;
            }
        };
        let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());

        let supported_configs = match device.supported_input_configs() {
            Ok(c) => c,
            Err(e) => {
                let _ = stream_tx.send(Err(format!("无支持的音频配置: {}", e)));
                return;
            }
        };
        let range = match supported_configs.into_iter().next() {
            Some(r) => r,
            None => {
                let _ = stream_tx.send(Err("无支持的音频配置范围".to_string()));
                return;
            }
        };

        let stream_config = cpal::StreamConfig {
            channels: range.channels(),
            sample_rate: cpal::SampleRate(range.max_sample_rate().0.min(48_000)),
            buffer_size: cpal::BufferSize::Default,
        };

        let sample_rate = stream_config.sample_rate.0;
        let ch = stream_config.channels;
        let sample_format = range.sample_format();

        log::info!("[字幕] 音频设备: {}, {}Hz, {}ch", device_name, sample_rate, ch);

        let err_fn = |err| {
            log::error!("[字幕] 音频采集错误: {}", err);
        };

        let ch_usize = ch as usize;
        let ch_f32 = ch as f32;

        let stream = match sample_format {
            SampleFormat::F32 => {
                let buf = buffer_for_thread.clone();
                let is_cap = running_thread.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if !is_cap.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(mut b) = buf.lock() {
                            if ch_usize == 1 {
                                b.extend_from_slice(data);
                            } else {
                                for chunk in data.chunks(ch_usize) {
                                    if chunk.len() == ch_usize {
                                        let mono: f32 = chunk.iter().sum::<f32>() / ch_f32;
                                        b.push(mono);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let buf = buffer_for_thread.clone();
                let is_cap = running_thread.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if !is_cap.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(mut b) = buf.lock() {
                            if ch_usize == 1 {
                                for &sample in data {
                                    b.push(sample as f32 / i16::MAX as f32);
                                }
                            } else {
                                for chunk in data.chunks(ch_usize) {
                                    if chunk.len() == ch_usize {
                                        let sum: f32 = chunk.iter().map(|&s| s as f32 / i16::MAX as f32).sum();
                                        b.push(sum / ch_f32);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let buf = buffer_for_thread.clone();
                let is_cap = running_thread.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        if !is_cap.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(mut b) = buf.lock() {
                            if ch_usize == 1 {
                                for &sample in data {
                                    b.push((sample as f32 / u16::MAX as f32) * 2.0 - 1.0);
                                }
                            } else {
                                for chunk in data.chunks(ch_usize) {
                                    if chunk.len() == ch_usize {
                                        let sum: f32 = chunk.iter().map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0).sum();
                                        b.push(sum / ch_f32);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I32 => {
                let buf = buffer_for_thread.clone();
                let is_cap = running_thread.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i32], _: &cpal::InputCallbackInfo| {
                        if !is_cap.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(mut b) = buf.lock() {
                            if ch_usize == 1 {
                                for &sample in data {
                                    b.push(sample as f32 / i32::MAX as f32);
                                }
                            } else {
                                for chunk in data.chunks(ch_usize) {
                                    if chunk.len() == ch_usize {
                                        let sum: f32 = chunk.iter().map(|&s| s as f32 / i32::MAX as f32).sum();
                                        b.push(sum / ch_f32);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U32 => {
                let buf = buffer_for_thread.clone();
                let is_cap = running_thread.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u32], _: &cpal::InputCallbackInfo| {
                        if !is_cap.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(mut b) = buf.lock() {
                            if ch_usize == 1 {
                                for &sample in data {
                                    b.push((sample as f32 / u32::MAX as f32) * 2.0 - 1.0);
                                }
                            } else {
                                for chunk in data.chunks(ch_usize) {
                                    if chunk.len() == ch_usize {
                                        let sum: f32 = chunk.iter().map(|&s| (s as f32 / u32::MAX as f32) * 2.0 - 1.0).sum();
                                        b.push(sum / ch_f32);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I8 => {
                let buf = buffer_for_thread.clone();
                let is_cap = running_thread.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i8], _: &cpal::InputCallbackInfo| {
                        if !is_cap.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(mut b) = buf.lock() {
                            if ch_usize == 1 {
                                for &sample in data {
                                    b.push(sample as f32 / i8::MAX as f32);
                                }
                            } else {
                                for chunk in data.chunks(ch_usize) {
                                    if chunk.len() == ch_usize {
                                        let sum: f32 = chunk.iter().map(|&s| s as f32 / i8::MAX as f32).sum();
                                        b.push(sum / ch_f32);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U8 => {
                let buf = buffer_for_thread.clone();
                let is_cap = running_thread.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u8], _: &cpal::InputCallbackInfo| {
                        if !is_cap.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(mut b) = buf.lock() {
                            if ch_usize == 1 {
                                for &sample in data {
                                    b.push((sample as f32 / u8::MAX as f32) * 2.0 - 1.0);
                                }
                            } else {
                                for chunk in data.chunks(ch_usize) {
                                    if chunk.len() == ch_usize {
                                        let sum: f32 = chunk.iter().map(|&s| (s as f32 / u8::MAX as f32) * 2.0 - 1.0).sum();
                                        b.push(sum / ch_f32);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            _ => {
                let _ = stream_tx.send(Err(format!("不支持的样本格式: {:?}", sample_format)));
                return;
            }
        };

        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                let _ = stream_tx.send(Err(format!("构建音频流失败: {}", e)));
                return;
            }
        };

        if let Err(e) = stream.play() {
            let _ = stream_tx.send(Err(format!("启动音频流失败: {}", e)));
            return;
        }

        let _ = stream_tx.send(Ok(sample_rate));

        while running_thread.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }

        drop(stream);
    });

    let sample_rate = stream_rx.recv().map_err(|_| "音频线程启动失败")??;

    let (result_tx, mut result_rx) = mpsc::channel(32);
    let (client, ws_task) = StreamingAsrClient::connect(config.clone(), result_tx)
        .await
        .map_err(|e| format!("连接豆包失败: {}", e))?;

    let _ = app.emit("subtitle-text", serde_json::json!({
        "text": "实时字幕已就绪，请开始说话",
        "isFinal": false
    }));

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
                    let _ = app_result.emit("subtitle-text", serde_json::json!({
                        "text": display,
                        "isFinal": is_final
                    }));
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
                // 取走 buffer 中全部样本，避免积压（同 streaming/session.rs 的修复）
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

    let rest: Vec<f32> = {
        match buffer_for_later.lock() {
            Ok(mut buf) => std::mem::take(&mut *buf),
            Err(_) => Vec::new(),
        }
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
