//! 字幕窗口管理：label 映射、窗口构建、事件挂接、几何持久化。
//!
//! 设计要点：
//! - 窗口生命周期保持稳定（不销毁重建），OBS 采集源绑定的是窗口 HWND，
//!   重建窗口会让 OBS 采集源指向已销毁的 HWND 而变黑；
//! - 窗口统一为不透明黑底的完整窗口，渲染与 OBS 采集稳定；
//!   透明窗口在 OBS 窗口采集（BitBlt）下会整片变黑；
//! - 拖动采用前端手动拖动（见 subtitle.html），避开 Windows 模态移动循环
//!   在透明窗口上造成的内容闪烁/消失（tauri #14764）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
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

// 注意：**全模块不再调用 `set_background_color`**，且 `apply_window_props`
// **不再重复设置位置/尺寸**。经验证据表明：运行时提交窗口属性（背景色、
// 甚至是 resize/位置回跳）会触发部分机器上 WebView2 内容不渲染（黑框）。
// - 背景完全由页面自身 CSS（body { background: #0b0b0b }）绘制；
// - 位置/尺寸只在窗口**创建**时设置一次（build_window），之后由用户
//   拖动/缩放自然决定，关闭重开不再回跳或缩放。

/// 将运行时安全的窗口属性应用到窗口（仅置顶/穿透）。
/// 不触碰位置/尺寸（避免触发渲染异常，且尊重用户手动调整的结果）。
pub fn apply_window_props(window: &WebviewWindow, win: &SubtitleWindow) {
    let _ = window.set_always_on_top(win.always_on_top);
    let _ = window.set_ignore_cursor_events(win.click_through);
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
/// - 关闭（标题栏 X）→ 隐藏窗口，并广播窗口状态给主窗口（设置面板同步显示「窗口已关闭」）；
/// - 移动/缩放 → 保存几何。
pub fn attach_window_events(window: &WebviewWindow, window_id: &str, config: &Arc<ConfigManager>) {
    use tauri::Emitter;

    let win = window.clone();
    let id = window_id.to_string();
    let config = config.clone();

    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = win.hide();
            // 同步 UI：广播窗口已关闭（hidden）状态
            let _ = win.app_handle().emit(
                "subtitle-window-state",
                serde_json::json!({ "windowId": id, "visible": false }),
            );
        }
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
            debounce_save_geometry(&win, &id, &config);
        }
        _ => {}
    });
}
