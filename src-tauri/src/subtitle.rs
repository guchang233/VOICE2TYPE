//! 实时字幕：多场景自定义字幕窗口 + 豆包流式 ASR + 同声传译
//!
//! 架构：
//! - 每个「场景」对应一个独立字幕窗口（默认场景使用静态 subtitle 窗口，
//!   其余场景运行时动态创建 `subtitle-scene-{id}` 窗口）；
//! - 一次字幕会话（单路音源 → 豆包流式 ASR → 同声传译）广播到所有启用场景；
//! - `subtitle-text` / `subtitle-config` / `subtitle-obs-mode` 事件携带 `sceneId`，
//!   字幕窗口按自身场景 ID 过滤。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Position, Size, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};
use tokio::sync::{mpsc, Mutex};

use crate::audio::processor::resample_and_convert;
use crate::config::{ConfigManager, SubtitleSceneConfig, STREAM_MODEL_DOUBAO};
use crate::streaming::{start_capture_with_prefs, AsrResponse, StreamingAsrClient};
use crate::translate::TranslationPipeline;

const CHUNK_MS: u64 = 200;

/// 主场景 ID（静态 subtitle 窗口）
pub const DEFAULT_SCENE_ID: &str = "default";

/// 窗口几何保存节流：每个场景每 800ms 最多触发一次持久化
static LAST_GEO_SAVE: Lazy<StdMutex<HashMap<String, Instant>>> =
    Lazy::new(|| StdMutex::new(HashMap::new()));

/// 场景 ID → 窗口 label 映射
pub fn scene_window_label(scene_id: &str) -> String {
    if scene_id == DEFAULT_SCENE_ID {
        "subtitle".to_string()
    } else {
        format!("subtitle-scene-{}", scene_id)
    }
}

// ==================== 配置/窗口工具函数 ====================

/// 构建场景样式配置事件负载
pub fn build_config_payload(scene: &SubtitleSceneConfig) -> serde_json::Value {
    let s = &scene.style;
    serde_json::json!({
        "sceneId": scene.id,
        "sceneName": scene.name,
        "fontFamily": s.font_family,
        "fontSize": s.font_size,
        "fontWeight": s.font_weight,
        "bold": s.font_weight >= 700,
        "italic": s.italic,
        "fontColor": s.font_color,
        "textAlign": s.text_align,
        "letterSpacing": s.letter_spacing,
        "lineHeight": s.line_height,
        "textShadowColor": s.text_shadow_color,
        "textShadowStrength": s.text_shadow_strength,
        "bgColor": s.bg_color,
        "bgOpacity": s.bg_opacity,
        "blur": s.blur,
        "borderRadius": s.border_radius,
        "borderColor": s.border_color,
        "borderWidth": s.border_width,
        "paddingX": s.padding_x,
        "paddingY": s.padding_y,
        "maxLines": s.max_lines,
        "interimColor": s.interim_color,
        "interimOpacity": s.interim_opacity,
        "showOriginal": s.show_original,
        "showTranslation": s.show_translation,
        "showSpeaker": s.show_speaker,
        "showTimestamp": s.show_timestamp,
        "layout": s.layout,
        "translationFontSize": s.translation_font_size,
        "translationFontColor": s.translation_font_color,
        "translationFontWeight": s.translation_font_weight,
        "translationOpacity": s.translation_opacity,
        "translationPrefix": s.translation_prefix,
        "speakerColor": s.speaker_color,
        "speakerFontSize": s.speaker_font_size,
        "speakerPrefix": s.speaker_prefix,
        "timestampColor": s.timestamp_color,
        "timestampFontSize": s.timestamp_font_size,
        "timestampFormat": s.timestamp_format,
        "customElements": s.custom_elements,
        "elementOrder": s.element_order,
        "preset": s.preset,
        // 同声传译配置（供窗口端展示/调试）
        "translationEngine": scene.translation.engine,
        "translationTargetLang": scene.translation.target_lang,
    })
}

/// 推送场景样式配置事件（app 级广播，窗口按 sceneId 过滤）
pub fn push_scene_config(app: &AppHandle, scene: &SubtitleSceneConfig) {
    let _ = app.emit("subtitle-config", build_config_payload(scene));
}

/// 将场景窗口控制配置应用到窗口（位置/尺寸/置顶/穿透）
pub fn apply_scene_window_props(window: &WebviewWindow, scene: &SubtitleSceneConfig) {
    let _ = window.set_always_on_top(scene.window.always_on_top);
    let _ = window.set_ignore_cursor_events(scene.window.click_through);
    if scene.window.x >= 0 && scene.window.y >= 0 {
        let _ = window.set_position(Position::Logical(LogicalPosition {
            x: scene.window.x as f64,
            y: scene.window.y as f64,
        }));
    }
    if scene.window.width > 0 && scene.window.height > 0 {
        let _ = window.set_size(Size::Logical(LogicalSize {
            width: scene.window.width as f64,
            height: scene.window.height as f64,
        }));
    }
}

/// 窗口几何变化 → 防抖持久化到场景配置
fn debounce_save_window_geometry(window: &WebviewWindow, scene_id: &str, config: &Arc<ConfigManager>) {
    let due = {
        let mut map = match LAST_GEO_SAVE.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        match map.get(scene_id) {
            Some(t) if t.elapsed() < Duration::from_millis(800) => false,
            _ => {
                map.insert(scene_id.to_string(), Instant::now());
                true
            }
        }
    };
    if !due {
        return;
    }
    let win = window.clone();
    let config = config.clone();
    let id = scene_id.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let pos = win.outer_position().ok();
        let size = win.outer_size().ok();
        if let (Some(p), Some(s)) = (pos, size) {
            config.update_subtitle_scene_window(&id, p.x, p.y, s.width, s.height);
            let _ = config.save();
        }
    });
}

/// 为字幕窗口挂接事件：关闭 → 停用场景；移动/缩放 → 保存几何
pub fn attach_scene_window_events(window: &WebviewWindow, scene_id: &str, config: &Arc<ConfigManager>) {
    let win = window.clone();
    let id = scene_id.to_string();
    let config = config.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = win.hide();
            config.set_subtitle_scene_enabled(&id, false);
            let _ = config.save();
        }
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
            debounce_save_window_geometry(&win, &id, &config);
        }
        _ => {}
    });
}

/// 确保场景窗口存在（默认场景复用静态 subtitle 窗口，其余动态创建）
pub async fn ensure_scene_window(
    app: &AppHandle,
    scene: &SubtitleSceneConfig,
    config: &Arc<ConfigManager>,
) -> Option<WebviewWindow> {
    let label = scene_window_label(&scene.id);
    if let Some(window) = app.get_webview_window(&label) {
        return Some(window);
    }
    if scene.id == DEFAULT_SCENE_ID {
        // 静态窗口应在 tauri.conf.json 中定义；这里仅兜底
        return app.get_webview_window("subtitle");
    }

    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("subtitle.html".into()))
        .title("Voice2Type 实时字幕")
        .decorations(false)
        .resizable(true)
        .always_on_top(scene.window.always_on_top)
        .skip_taskbar(true)
        .visible(false);
    if scene.window.width > 0 && scene.window.height > 0 {
        builder = builder.inner_size(scene.window.width as f64, scene.window.height as f64);
    }
    if scene.window.x >= 0 && scene.window.y >= 0 {
        builder = builder.position(scene.window.x as f64, scene.window.y as f64);
    }
    let window = match builder.build() {
        Ok(w) => w,
        Err(e) => {
            log::error!("[字幕] 创建场景窗口 {} 失败: {}", label, e);
            return None;
        }
    };
    attach_scene_window_events(&window, &scene.id, config);
    Some(window)
}

// ==================== SubtitleService ====================

pub struct SubtitleService {
    app_handle: Arc<Mutex<Option<AppHandle>>>,
    config: Arc<ConfigManager>,
    running: Arc<AtomicBool>,
    stop_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    /// 会话代际：每次 start 递增，旧会话收尾时不误伤新会话的窗口
    session_gen: Arc<std::sync::atomic::AtomicU64>,
}

impl SubtitleService {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        Self {
            app_handle: Arc::new(Mutex::new(None)),
            config,
            running: Arc::new(AtomicBool::new(false)),
            stop_tx: Arc::new(Mutex::new(None)),
            session_gen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub async fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.lock().await = Some(handle);
    }

    /// 应用启动时为主场景（静态 subtitle 窗口）挂接事件
    pub fn init_default_scene_window(&self, app: &AppHandle) {
        if let Some(window) = app.get_webview_window("subtitle") {
            attach_scene_window_events(&window, DEFAULT_SCENE_ID, &self.config);
        }
    }

    pub async fn start(&self, config: Arc<ConfigManager>) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Err("字幕已在运行".to_string());
        }

        let handle = self.app_handle.lock().await;
        let app = handle.as_ref().ok_or("App handle not ready")?.clone();
        drop(handle);

        let cfg = config.get_config();
        let scenes: Vec<SubtitleSceneConfig> = cfg
            .subtitle
            .subtitle_scenes
            .iter()
            .filter(|s| s.enabled)
            .cloned()
            .collect();
        if scenes.is_empty() {
            return Err("没有启用的字幕场景，请先在字幕设置中启用至少一个场景".to_string());
        }
        let scene_ids: Vec<String> = scenes.iter().map(|s| s.id.clone()).collect();

        // 1. 确保窗口存在、应用窗口属性、推送配置、显示
        for scene in &scenes {
            if let Some(window) = ensure_scene_window(&app, scene, &config).await {
                apply_scene_window_props(&window, scene);
                push_scene_config(&app, scene);
                let _ = window.show();
            }
        }

        self.running.store(true, Ordering::SeqCst);
        let session_gen = self.session_gen.fetch_add(1, Ordering::SeqCst) + 1;

        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        {
            let mut tx = self.stop_tx.lock().await;
            *tx = Some(stop_tx);
        }

        let running = self.running.clone();
        let app_handle = app.clone();
        let scene_ids_task = scene_ids.clone();
        let config_task = config.clone();
        let gen_flag = self.session_gen.clone();

        tokio::spawn(async move {
            if let Err(e) = run_session(
                app_handle.clone(),
                config_task.clone(),
                scene_ids_task.clone(),
                running.clone(),
                &mut stop_rx,
            )
            .await
            {
                log::error!("[字幕] 会话错误: {}", e);
                emit_frame(
                    &app_handle,
                    &scene_ids_task,
                    Some(&format!("错误: {}", e)),
                    false,
                    "",
                    "",
                    "",
                    "",
                );
            }

            running.store(false, Ordering::SeqCst);

            // 只有最新会话才执行收尾（旧会话收尾不隐藏新会话刚显示的窗口）
            if gen_flag.load(Ordering::SeqCst) == session_gen {
                for scene_id in &scene_ids_task {
                    if let Some(window) = app_handle.get_webview_window(&scene_window_label(scene_id)) {
                        let _ = window.hide();
                    }
                }
                emit_frame(&app_handle, &scene_ids_task, Some(""), true, "", "", "", "");
            }
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

// ==================== 帧广播 ====================

/// 向所有场景广播一帧字幕（每个场景各发一条，窗口按 sceneId 过滤）
#[allow(clippy::too_many_arguments)]
fn emit_frame(
    app: &AppHandle,
    scene_ids: &[String],
    text: Option<&str>,
    is_final: bool,
    definite: &str,
    indefinite: &str,
    translation: &str,
    translation_interim: &str,
) {
    for scene_id in scene_ids {
        let _ = app.emit(
            "subtitle-text",
            serde_json::json!({
                "sceneId": scene_id,
                "text": text.unwrap_or(""),
                "isFinal": is_final,
                "definite": definite,
                "indefinite": indefinite,
                "translation": translation,
                "translationInterim": translation_interim,
            }),
        );
    }
}

// ==================== 字幕会话 ====================

/// 运行一路字幕会话：音频采集 → 豆包流式 ASR → 同声传译 → 场景广播
async fn run_session(
    app: AppHandle,
    config: Arc<ConfigManager>,
    scene_ids: Vec<String>,
    running: Arc<AtomicBool>,
    stop_rx: &mut mpsc::Receiver<()>,
) -> Result<(), String> {
    let subtitle_model = config.subtitle_model();
    if subtitle_model != STREAM_MODEL_DOUBAO {
        emit_frame(
            &app,
            &scene_ids,
            Some("请将字幕引擎设置为豆包云端流式识别"),
            false,
            "",
            "",
            "",
            "",
        );
        wait_until_stop(running, stop_rx).await;
        return Ok(());
    }
    if config.get_doubao_api_key().is_empty() {
        emit_frame(
            &app,
            &scene_ids,
            Some("请先配置豆包 API Key"),
            false,
            "",
            "",
            "",
            "",
        );
        wait_until_stop(running, stop_rx).await;
        return Ok(());
    }

    // ===== 每场景翻译流水线（引擎关闭则无） =====
    let cfg_snapshot = config.get_config();
    let translation_cache: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut pipelines: HashMap<String, Option<TranslationPipeline>> = HashMap::new();
    for scene in &cfg_snapshot.subtitle.subtitle_scenes {
        if !scene_ids.contains(&scene.id) {
            continue;
        }
        let pl = if scene.translation.engine == "llm" {
            Some(TranslationPipeline::new(
                config.clone(),
                scene.translation.engine.clone(),
                scene.translation.target_lang.clone(),
                scene.translation.interim,
                translation_cache.clone(),
            ))
        } else {
            None
        };
        pipelines.insert(scene.id.clone(), pl);
    }

    emit_frame(
        &app,
        &scene_ids,
        Some("正在连接语音服务..."),
        false,
        "",
        "",
        "",
        "",
    );

    let pcm_buffer: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let audio_seq = Arc::new(std::sync::Mutex::new(2i32));

    let buffer_for_later = pcm_buffer.clone();
    let running_thread = running.clone();

    let (stream_tx, stream_rx) = std::sync::mpsc::channel::<Result<u32, String>>();

    // 在 move 进 thread 之前读取音源 + 有效音频偏好
    let subtitle_device_name = config.subtitle_input_device();
    let audio_source = config.subtitle_audio_source();
    let (dm, sf, sr_pref, ch_pref) = config.effective_audio_prefs();

    // 统一两种采集方式的句柄（cpal 麦克风流 / WASAPI loopback），drop 时停止采集
    enum SubtitleCapture {
        Cpal(cpal::Stream),
        Loopback(crate::audio::loopback::LoopbackCapture),
    }

    let _stream_thread = std::thread::spawn(move || {
        let capture_result = if audio_source == "system" {
            crate::audio::loopback::start_loopback_capture(pcm_buffer, &dm)
                .map(|(sr, ch, h)| (sr, ch, SubtitleCapture::Loopback(h)))
        } else {
            // 麦克风：复用 streaming 模块的 start_capture_with_prefs
            start_capture_with_prefs(
                pcm_buffer,
                Some(&subtitle_device_name),
                Some(&dm),
                Some(&sf),
                Some(&sr_pref),
                Some(&ch_pref),
            )
            .map(|(sr, ch, s)| (sr, ch, SubtitleCapture::Cpal(s)))
        };

        match capture_result {
            Ok((sample_rate, _ch, handle)) => {
                let _ = stream_tx.send(Ok(sample_rate));
                // 保持存活直到 running=false
                while running_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                // 显式 drop 句柄后线程退出
                drop(handle);
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

    emit_frame(
        &app,
        &scene_ids,
        Some("实时字幕已就绪，请开始说话"),
        false,
        "",
        "",
        "",
        "",
    );

    let app_result = app.clone();
    let result_task = tokio::spawn(async move {
        while let Some(msg) = result_rx.recv().await {
            match msg {
                Ok(AsrResponse {
                    text,
                    is_final,
                    definite_text,
                    indefinite_text,
                }) => {
                    // 逐场景翻译并广播
                    for (scene_id, pipeline) in pipelines.iter_mut() {
                        let (fin, prov) = match pipeline {
                            Some(pl) => pl.on_frame(&definite_text, &indefinite_text).await,
                            None => (String::new(), String::new()),
                        };
                        emit_frame(
                            &app_result,
                            std::slice::from_ref(scene_id),
                            Some(&text),
                            is_final,
                            &definite_text,
                            &indefinite_text,
                            &fin,
                            &prov,
                        );
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

/// 等待停止信号（无 ASR 会话的占位等待）
async fn wait_until_stop(running: Arc<AtomicBool>, stop_rx: &mut mpsc::Receiver<()>) {
    while running.load(Ordering::SeqCst) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = stop_rx.recv() => { break; }
        }
    }
}
