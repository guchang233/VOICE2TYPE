//! 字幕窗口管理：label 映射、动态创建/销毁、窗口属性、事件挂接、几何持久化。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use tauri::window::Color;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Position, Size, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::config::{ConfigManager, SubtitleWindow, PRIMARY_WINDOW_ID};

/// 窗口几何保存节流：每个窗口每 800ms 最多触发一次持久化
static LAST_GEO_SAVE: Lazy<StdMutex<HashMap<String, Instant>>> =
    Lazy::new(|| StdMutex::new(HashMap::new()));

/// 窗口 ID → label 映射（主窗口复用静态 "subtitle"，其余动态命名）
pub fn window_label(window_id: &str) -> String {
    if window_id == PRIMARY_WINDOW_ID {
        "subtitle".to_string()
    } else {
        format!("subtitle-w-{}", window_id)
    }
}

/// 将窗口控制配置应用到窗口（位置/尺寸/置顶/穿透/OBS 背景）
pub fn apply_window_props(window: &WebviewWindow, win: &SubtitleWindow) {
    let _ = window.set_always_on_top(win.always_on_top);
    let _ = window.set_ignore_cursor_events(win.click_through);
    // OBS 捕捉兼容：OBS 模式开启时把窗口设为不透明黑底。
    // 透明窗口在 OBS 窗口采集（BitBlt）下常表现为黑屏/静止画面；
    // 改为不透明黑底后窗口本身即是一张可采集的画面，切换即时生效、无需重启应用。
    // （启动期的 WEBVIEW2 环境变量回退 GDI 仍保留，作为双保险。）
    if win.obs_mode {
        let _ = window.set_background_color(Some(Color(0, 0, 0, 255)));
    } else {
        let _ = window.set_background_color(None);
    }
    if win.x >= 0 && win.y >= 0 {
        let _ = window.set_position(Position::Logical(LogicalPosition {
            x: win.x as f64,
            y: win.y as f64,
        }));
    }
    if win.width > 0 && win.height > 0 {
        let _ = window.set_size(Size::Logical(LogicalSize {
            width: win.width as f64,
            height: win.height as f64,
        }));
    }
}

/// 窗口几何变化 → 防抖持久化到配置
fn debounce_save_geometry(window: &WebviewWindow, window_id: &str, config: &Arc<ConfigManager>) {
    let due = {
        let mut map = match LAST_GEO_SAVE.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        match map.get(window_id) {
            Some(t) if t.elapsed() < Duration::from_millis(800) => false,
            _ => {
                map.insert(window_id.to_string(), Instant::now());
                true
            }
        }
    };
    if !due {
        return;
    }
    let win = window.clone();
    let config = config.clone();
    let id = window_id.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let pos = win.outer_position().ok();
        let size = win.outer_size().ok();
        if let (Some(p), Some(s)) = (pos, size) {
            config.update_subtitle_window_rect(&id, p.x, p.y, s.width, s.height);
            let _ = config.save();
        }
    });
}

/// 为字幕窗口挂接事件：关闭 → 停用窗口；移动/缩放 → 保存几何
pub fn attach_window_events(window: &WebviewWindow, window_id: &str, config: &Arc<ConfigManager>) {
    let win = window.clone();
    let id = window_id.to_string();
    let config = config.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = win.hide();
            config.set_subtitle_window_enabled(&id, false);
            let _ = config.save();
        }
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
            debounce_save_geometry(&win, &id, &config);
        }
        _ => {}
    });
}

/// 确保字幕窗口存在（主窗口复用静态 subtitle 窗口，其余动态创建）
pub async fn ensure_window(
    app: &AppHandle,
    win: &SubtitleWindow,
    config: &Arc<ConfigManager>,
) -> Option<WebviewWindow> {
    let label = window_label(&win.id);
    if let Some(window) = app.get_webview_window(&label) {
        return Some(window);
    }
    if win.id == PRIMARY_WINDOW_ID {
        // 静态窗口在 tauri.conf.json 中定义；这里仅兜底
        return app.get_webview_window("subtitle");
    }

    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("subtitle.html".into()))
        .title("Voice2Type 实时字幕")
        .decorations(false)
        .resizable(true)
        .transparent(true)
        .always_on_top(win.always_on_top)
        .skip_taskbar(true)
        .visible(false);
    if win.width > 0 && win.height > 0 {
        builder = builder.inner_size(win.width as f64, win.height as f64);
    }
    if win.x >= 0 && win.y >= 0 {
        builder = builder.position(win.x as f64, win.y as f64);
    }
    let window = match builder.build() {
        Ok(w) => w,
        Err(e) => {
            log::error!("[字幕] 创建窗口 {} 失败: {}", label, e);
            return None;
        }
    };
    attach_window_events(&window, &win.id, config);
    Some(window)
}
