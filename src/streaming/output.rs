//! 流式识别结果注入：选区替换覆盖本会话已输出内容（不碰输入框原有文字）

use std::sync::{Arc, Mutex};

use crate::config::{ConfigManager, STREAMING_POST_AI};
use crate::streaming::post_process;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Controls::{EM_GETSEL, EM_REPLACESEL, EM_SETSEL};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

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
        *self.last_displayed.lock().unwrap() = String::new();
        *self.last_raw.lock().unwrap() = String::new();
        *self.last_ai_polished_src.lock().unwrap() = String::new();
        #[cfg(target_os = "windows")]
        {
            *self.anchor.lock().unwrap() = None;
        }
    }

    pub fn begin_session(&self) {
        #[cfg(target_os = "windows")]
        {
            if let Some(a) = capture_edit_anchor() {
                *self.anchor.lock().unwrap() = Some(a);
            }
        }
    }

    /// 服务端全量文本 → 按后处理模式显示；AI 模式在结束时异步润色并覆盖
    pub fn apply_full_text(self: &Arc<Self>, raw_text: &str, config: &Arc<ConfigManager>, is_final: bool) {
        let trimmed = raw_text.trim();
        if trimmed.is_empty() {
            return;
        }

        {
            let mut last_raw = self.last_raw.lock().unwrap();
            if *last_raw == trimmed && !is_final {
                return;
            }
            *last_raw = trimmed.to_string();
        }

        let display = post_process::process_streaming_text(trimmed, config);
        self.replace_display(&display);

        if config.streaming_post_process_mode() == STREAMING_POST_AI && is_final {
            self.spawn_ai_polish(trimmed, config.clone());
        }
    }

    /// 会话结束时补一次 AI 润色（防止未收到 is_final 尾包）
    pub fn finalize_ai_polish(self: &Arc<Self>, config: &Arc<ConfigManager>) {
        if config.streaming_post_process_mode() != STREAMING_POST_AI {
            return;
        }
        let raw = self.last_raw.lock().unwrap().clone();
        if raw.is_empty() {
            return;
        }
        self.spawn_ai_polish(&raw, config.clone());
    }

    fn spawn_ai_polish(self: &Arc<Self>, raw: &str, config: Arc<ConfigManager>) {
        {
            let mut guard = self.last_ai_polished_src.lock().unwrap();
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
                        &format!("AI 润色失败，保留原文: {}", e),
                        Some(&config),
                    );
                }
            }
        });
    }

    fn replace_display(&self, display: &str) {
        let mut last = self.last_displayed.lock().unwrap();
        if *last == display {
            return;
        }

        let prev_utf16 = utf16_len(&last);
        let ok = replace_session_text(prev_utf16, display, self);
        if !ok {
            #[cfg(target_os = "windows")]
            fallback_backspace_type(prev_utf16, display);
        }
        *last = display.to_string();
    }
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn replace_session_text(prev_utf16: u32, new_text: &str, output: &StreamingOutput) -> bool {
    #[cfg(target_os = "windows")]
    {
        let anchor = output.anchor.lock().unwrap().clone();
        if let Some(a) = anchor {
            return replace_via_edit(&a, prev_utf16, new_text);
        }
    }
    let _ = (prev_utf16, new_text, output);
    false
}

#[cfg(target_os = "windows")]
fn capture_edit_anchor() -> Option<EditAnchor> {
    unsafe {
        let hwnd = GetFocus();
        if hwnd.0 == 0 {
            return None;
        }
        let ret = SendMessageW(hwnd, EM_GETSEL, WPARAM(0), LPARAM(0));
        let start = (ret.0 & 0xFFFF) as u32;
        Some(EditAnchor { hwnd, start })
    }
}

#[cfg(target_os = "windows")]
fn replace_via_edit(anchor: &EditAnchor, prev_utf16: u32, new_text: &str) -> bool {
    unsafe {
        let end = anchor.start.saturating_add(prev_utf16);
        SendMessageW(
            anchor.hwnd,
            EM_SETSEL,
            WPARAM(anchor.start as usize),
            LPARAM(end as isize),
        );
        let wide: Vec<u16> = new_text.encode_utf16().chain(std::iter::once(0)).collect();
        SendMessageW(
            anchor.hwnd,
            EM_REPLACESEL,
            WPARAM(1),
            LPARAM(wide.as_ptr() as isize),
        );
        true
    }
}

#[cfg(target_os = "windows")]
fn fallback_backspace_type(prev_utf16: u32, new_text: &str) {
    unsafe {
        if prev_utf16 > 0 {
            crate::win_utils::send_backspaces(prev_utf16 as usize);
        }
        if !new_text.is_empty() {
            crate::win_utils::send_unicode_text(new_text);
        }
    }
}
