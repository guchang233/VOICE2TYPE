use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::ConfigManager;

static EMOJI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[\p{Emoji_Presentation}\p{Extended_Pictographic}]")
        .expect("emoji regex")
});
static WHITESPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").expect("whitespace regex"));
static PUNCT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\p{P}]").expect("punctuation regex"));

#[derive(Clone, Default)]
pub struct OutputHandler;

impl OutputHandler {
    pub fn new() -> Self {
        Self
    }

    pub async fn handle_output(&self, text: String, config: &ConfigManager) -> Result<()> {
        Self::paste_text(text, config)
    }

    /// 重新粘贴上一条识别结果（托盘菜单调用）
    pub fn repaste(text: String, config: &ConfigManager) -> Result<()> {
        Self::paste_text(text, config)
    }

    fn paste_text(text: String, config: &ConfigManager) -> Result<()> {
        match config.output_mode().as_str() {
            "clipboard" => Self::output_by_clipboard_sync(text),
            "inject" => Self::output_by_keyboard(text),
            mode => anyhow::bail!("Unknown output mode: {}", mode),
        }
    }

    fn output_by_clipboard_sync(text: String) -> Result<()> {
        #[cfg(target_os = "windows")]
        unsafe {
            let backup = crate::win_utils::get_clipboard_text();

            // 使用剪贴板粘贴兼容性最好，同时排除 Win+V 历史，尽量不打扰用户原来的剪贴板。
            crate::win_utils::set_clipboard_text(&text, true);
            crate::win_utils::paste_clipboard();

            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if let Some(old_text) = backup {
                    crate::win_utils::set_clipboard_text(&old_text, true);
                } else {
                    use windows::Win32::System::DataExchange::{
                        CloseClipboard, EmptyClipboard, OpenClipboard,
                    };
                    let _ = OpenClipboard(None);
                    let _ = EmptyClipboard();
                    let _ = CloseClipboard();
                }
            });
        }

        #[cfg(not(target_os = "windows"))]
        Self::output_by_keyboard(text)?;

        Ok(())
    }

    fn output_by_keyboard(text: String) -> Result<()> {
        #[cfg(target_os = "windows")]
        unsafe {
            crate::win_utils::send_unicode_text(&text);
        }

        #[cfg(not(target_os = "windows"))]
        {
            use anyhow::Context;
            use enigo::{Enigo, Settings};
            let mut enigo =
                Enigo::new(&Settings::default()).context("Failed to initialize keyboard output")?;
            enigo.text(&text).context("Failed to send text")?;
        }

        Ok(())
    }
}

pub fn post_process(text: &str, config: &ConfigManager) -> String {
    let mut result = text.trim().to_string();

    if !config.allow_emoji() {
        result = EMOJI_RE.replace_all(&result, "").to_string();
    }

    if !config.allow_punctuation() {
        result = strip_punctuation(&result);
    }

    result = WHITESPACE_RE.replace_all(&result, " ").to_string();
    result.trim().to_string()
}

fn is_punctuation(c: char) -> bool {
    let mut buf = [0u8; 4];
    PUNCT_RE.is_match(c.encode_utf8(&mut buf))
}

fn strip_punctuation(text: &str) -> String {
    let is_numeric_separator = |c: char| matches!(c, '.' | ':' | ',' | '-');

    let chars: Vec<char> = text.chars().collect();
    let mut cleaned = String::with_capacity(text.len());

    for (i, &c) in chars.iter().enumerate() {
        if !is_punctuation(c) {
            cleaned.push(c);
            continue;
        }

        let keep_inside_number = is_numeric_separator(c)
            && i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_ascii_digit()
            && chars[i + 1].is_ascii_digit();

        cleaned.push(if keep_inside_number { c } else { ' ' });
    }

    cleaned
}
