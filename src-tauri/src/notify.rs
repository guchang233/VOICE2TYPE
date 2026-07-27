use std::sync::Mutex;

use once_cell::sync::Lazy;

/// 后台线程写入，GUI 主线程在 on_tick 中弹出托盘气泡。
pub static PENDING_TRAY_MESSAGES: Lazy<Mutex<Vec<(String, String)>>> =
    Lazy::new(|| Mutex::new(Vec::new()));

pub fn queue_tray_message(title: impl Into<String>, message: impl Into<String>) {
    if let Ok(mut pending) = PENDING_TRAY_MESSAGES.lock() {
        pending.push((title.into(), message.into()));
    }
}
