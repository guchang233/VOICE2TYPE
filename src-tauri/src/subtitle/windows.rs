//! 字幕窗口管理：label 映射、窗口构建、事件挂接、几何持久化。
//!
//! 设计要点：
//! - 窗口生命周期保持稳定（不销毁重建），OBS 采集源绑定的是窗口 HWND，
//!   重建窗口会让 OBS 采集源指向已销毁的 HWND 而变黑；
//! - OBS 兼容依赖「启动时 GDI 回退环境变量 + 运行时不透明黑底背景」，
//!   透明窗口在 OBS 窗口采集（BitBlt）下会整片变黑；
//! - 拖动采用前端手动拖动（见 subtitle.html），避开 Windows 模态移动循环
//!   在透明窗口上造成的内容闪烁/消失（tauri #14764）。

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

/// 设置窗口背景（完整窗口始终不透明，保证渲染与 OBS 采集稳定）：
/// - OBS 模式：纯黑底；
/// - 普通模式：深灰底（与页面底色一致）。
/// ⚠️ 禁止 `set_background_color(None)`：Windows 上会被替换成不透明白。
pub fn apply_background(window: &WebviewWindow, obs_mode: bool) {
    let color = if obs_mode {
        Color(0, 0, 0, 255)
    } else {
        Color(0x17, 0x17, 0x17, 255)
    };
    let _ = window.set_background_color(Some(color));
}

/// 将窗口控制配置应用到窗口（位置/尺寸/置顶/穿透/OBS 背景）
pub fn apply_window_props(window: &WebviewWindow, win: &SubtitleWindow) {
    let _ = window.set_always_on_top(win.always_on_top);
    let _ = window.set_ignore_cursor_events(win.click_through);
    apply_background(window, win.obs_mode);
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

/// 构建字幕窗口（完整窗口：带标题栏、不透明、出现在任务栏，可被 OBS 稳定采集）。
pub fn build_window(
    app: &AppHandle,
    win: &SubtitleWindow,
    config: &Arc<ConfigManager>,
) -> Option<WebviewWindow> {
    let label = window_label(&win.id);
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("subtitle.html".into()))
        .title("Voice2Type 实时字幕")
        .decorations(true)
        .resizable(true)
        .transparent(false)
        .always_on_top(win.always_on_top)
        .skip_taskbar(false)
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

/// 确保字幕窗口存在（已存在直接返回；不存在/已销毁则按配置构建）
pub fn ensure_window(
    app: &AppHandle,
    win: &SubtitleWindow,
    config: &Arc<ConfigManager>,
) -> Option<WebviewWindow> {
    let label = window_label(&win.id);
    if let Some(window) = app.get_webview_window(&label) {
        return Some(window);
    }
    build_window(app, win, config)
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

/// 为字幕窗口挂接事件：
/// - 关闭（标题栏 X）→ 仅隐藏窗口（不改变启用状态，完整窗口随时可再显示）；
/// - 移动/缩放 → 保存几何。
pub fn attach_window_events(window: &WebviewWindow, window_id: &str, config: &Arc<ConfigManager>) {
    let win = window.clone();
    let id = window_id.to_string();
    let config = config.clone();

    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = win.hide();
        }
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
            debounce_save_geometry(&win, &id, &config);
        }
        _ => {}
    });
}
