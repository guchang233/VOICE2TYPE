//! 字幕场景窗口管理：动态创建/销毁、窗口属性、事件挂接、配置推送

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Position, Size, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};

use crate::config::{ConfigManager, SubtitleSceneConfig};

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
        "paddingX": s.padding_x,
        "paddingY": s.padding_y,
        "maxLines": s.max_lines,
        "autoFit": scene.window.auto_fit,
        "interimColor": s.interim_color,
        "interimOpacity": s.interim_opacity,
        "showOriginal": s.show_original,
        "showTranslation": s.show_translation,
        "showSpeaker": s.show_speaker,
        "showTimestamp": s.show_timestamp,
        "showOriginalSecondary": s.show_original_secondary,
        "layout": s.layout,
        "containerAlignX": s.container_align_x,
        "containerAlignY": s.container_align_y,
        "boxMaxWidth": s.box_max_width,
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
        // OBS 兼容模式（窗口端据此切换黑底/透明与模糊）
        "obsMode": scene.window.obs_mode,
        // 同声传译配置（供窗口端展示/调试）
        "translationEngine": scene.translation.engine,
        "translationTargetLang": scene.translation.target_lang,
    })
}

/// 推送场景样式配置事件（窗口定向发送，避免广播到无关窗口）
pub fn push_scene_config(app: &AppHandle, scene: &SubtitleSceneConfig) {
    if let Some(window) = app.get_webview_window(&scene_window_label(&scene.id)) {
        let _ = window.emit("subtitle-config", build_config_payload(scene));
    }
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
        .transparent(true)
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
