//! Streaming ASR output: replace session text without touching pre-existing content.

use std::sync::{Arc, Mutex};

use crate::config::{ConfigManager, STREAMING_POST_AI};
use crate::streaming::post_process;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Controls::{EM_GETSEL, EM_REPLACESEL, EM_SETSEL};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, IsWindow, SendMessageW};

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct EditAnchor {
    hwnd: HWND,
    start: u32,
}

pub struct StreamingOutput {
    last_displayed: Mutex<String>,
    last_raw: Mutex<String>,
    last_ai_polished_src: Mutex<String>,
    #[cfg(target_os = "windows")]
    anchor: Mutex<Option<EditAnchor>>,
}

impl StreamingOutput {
    pub fn new() -> Self {
        Self {
            last_displayed: Mutex::new(String::new()),
            last_raw: Mutex::new(String::new()),
            last_ai_polished_src: Mutex::new(String::new()),
            #[cfg(target_os = "windows")]
            anchor: Mutex::new(None),
        }
    }

    pub fn reset(&self) {
        if let Ok(mut last) = self.last_displayed.lock() {
            *last = String::new();
        }
        if let Ok(mut last) = self.last_raw.lock() {
            *last = String::new();
        }
        if let Ok(mut last) = self.last_ai_polished_src.lock() {
            *last = String::new();
        }
        #[cfg(target_os = "windows")]
        if let Ok(mut anchor) = self.anchor.lock() {
            *anchor = None;
        }
    }

    pub fn begin_session(&self) {
        #[cfg(target_os = "windows")]
        {
            if let Some(a) = capture_edit_anchor() {
                if let Ok(mut anchor) = self.anchor.lock() {
                    *anchor = Some(a);
                }
            }
        }
    }

    pub fn apply_full_text(self: &Arc<Self>, raw_text: &str, config: &Arc<ConfigManager>, is_final: bool) {
        let trimmed = raw_text.trim();
        if trimmed.is_empty() {
            return;
        }

        if let Ok(mut last_raw) = self.last_raw.lock() {
            if *last_raw == trimmed && !is_final {
                return;
            }
            *last_raw = trimmed.to_string();
        } else {
            return;
        }

        let display = post_process::process_streaming_text(trimmed, config);
        self.replace_display(&display);

        if config.streaming_post_process_mode() == STREAMING_POST_AI && is_final {
            self.spawn_ai_polish(trimmed, config.clone());
        }
    }

    pub fn finalize_ai_polish(self: &Arc<Self>, config: &Arc<ConfigManager>) {
        if config.streaming_post_process_mode() != STREAMING_POST_AI {
            return;
        }
        let raw = match self.last_raw.lock() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        if raw.is_empty() {
            return;
        }
        self.spawn_ai_polish(&raw, config.clone());
    }

    fn spawn_ai_polish(self: &Arc<Self>, raw: &str, config: Arc<ConfigManager>) {
        {
            let Ok(mut guard) = self.last_ai_polished_src.lock() else {
                return;
            };
            if *guard == raw {
                return;
            }
            *guard = raw.to_string();
        }

        let output = Arc::clone(self);
        let raw = raw.to_string();
        tokio::spawn(async move {
            match crate::streaming::polish::polish_with_ai(&raw, &config).await {
                Ok(polished) => {
                    let cleaned = post_process::process_none(&polished, &config);
                    output.replace_display(&cleaned);
                }
                Err(e) => {
                    crate::utils::logger::write_log(
                        crate::utils::logger::LogLevel::WARN,
                        &format!("AI polish failed, keeping raw: {}", e),
                        Some(&config),
                    );
                }
            }
        });
    }

    fn replace_display(&self, display: &str) {
        let Ok(mut last) = self.last_displayed.lock() else {
            return;
        };
        if *last == display {
            return;
        }

        // 公共前缀增量替换：只删除变化的尾部、只输入新增的字符
        let (common_utf16, delete_count, new_suffix) = compute_incremental(&last, display);

        let ok = replace_session_text(common_utf16, delete_count, new_suffix, self);
        if !ok {
            #[cfg(target_os = "windows")]
            fallback_backspace_type(delete_count, new_suffix);
        }
        *last = display.to_string();
    }
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count().min(16_384) as u32
}

/// 计算新旧文本的公共前缀，返回 (公共前缀 UTF-16 长度, 需删除的 UTF-16 数量, 新文本后缀)
fn compute_incremental<'a>(prev: &'a str, new: &'a str) -> (u32, u32, &'a str) {
    // 按字符（Unicode scalar value）计算公共前缀的字节长度
    let mut common_bytes = 0usize;
    for (a, b) in prev.chars().zip(new.chars()) {
        if a == b {
            common_bytes += a.len_utf8();
        } else {
            break;
        }
    }
    let common_utf16 = prev[..common_bytes].encode_utf16().count() as u32;
    let prev_utf16 = utf16_len(prev);
    let delete_count = prev_utf16.saturating_sub(common_utf16);
    let new_suffix = &new[common_bytes..];
    (common_utf16, delete_count, new_suffix)
}

fn replace_session_text(
    common_utf16: u32,
    delete_count: u32,
    new_suffix: &str,
    output: &StreamingOutput,
) -> bool {
    #[cfg(target_os = "windows")]
    {
        let anchor = match output.anchor.lock() {
            Ok(g) => g.clone(),
            Err(_) => return false,
        };
        if let Some(a) = anchor {
            return replace_via_edit(&a, common_utf16, delete_count, new_suffix);
        }
    }
    let _ = (common_utf16, delete_count, new_suffix, output);
    false
}

#[cfg(target_os = "windows")]
fn capture_edit_anchor() -> Option<EditAnchor> {
    unsafe {
        let hwnd = crate::win_utils::get_focused_hwnd_cross_thread()?;
        if !IsWindow(hwnd).as_bool() || !is_edit_like(hwnd) {
            return None;
        }
        let ret = SendMessageW(hwnd, EM_GETSEL, WPARAM(0), LPARAM(0));
        let start = (ret.0 & 0xFFFF) as u32;
        Some(EditAnchor { hwnd, start })
    }
}

#[cfg(target_os = "windows")]
fn is_edit_like(hwnd: HWND) -> bool {
    unsafe {
        let mut buf = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut buf);
        if len == 0 {
            return false;
        }
        let name = String::from_utf16_lossy(&buf[..len as usize]);
        name == "Edit"
            || name.starts_with("RichEdit")
            || name == "RICHEDIT50W"
            || name.contains("Edit")
    }
}

#[cfg(target_os = "windows")]
fn replace_via_edit(anchor: &EditAnchor, common_utf16: u32, delete_count: u32, new_suffix: &str) -> bool {
    unsafe {
        if !IsWindow(anchor.hwnd).as_bool() {
            return false;
        }
        // 只选中变化的尾部 [anchor.start + common, anchor.start + common + delete_count]
        let sel_start = anchor.start.saturating_add(common_utf16);
        let sel_end = sel_start.saturating_add(delete_count);
        SendMessageW(
            anchor.hwnd,
            EM_SETSEL,
            WPARAM(sel_start as usize),
            LPARAM(sel_end as isize),
        );
        let wide: Vec<u16> = new_suffix.encode_utf16().chain(std::iter::once(0)).collect();
        let replaced = SendMessageW(
            anchor.hwnd,
            EM_REPLACESEL,
            WPARAM(1),
            LPARAM(wide.as_ptr() as isize),
        );
        replaced.0 != 0
    }
}

#[cfg(target_os = "windows")]
fn fallback_backspace_type(delete_count: u32, new_suffix: &str) {
    unsafe {
        if delete_count > 0 {
            crate::win_utils::send_backspaces(delete_count as usize);
        }
        if !new_suffix.is_empty() {
            crate::win_utils::send_unicode_text(new_suffix);
        }
    }
}
