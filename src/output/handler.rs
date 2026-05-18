use anyhow::Result;

use crate::config::ConfigManager;

#[derive(Clone, Default)]
pub struct OutputHandler;

impl OutputHandler {
    pub fn new() -> Self {
        Self
    }

    pub async fn handle_output(&self, text: String, config: &ConfigManager) -> Result<()> {
        match config.output_mode().as_str() {
            "clipboard" => self.output_by_clipboard(text).await,
            "inject" => self.output_by_keyboard(text),
            mode => anyhow::bail!("Unknown output mode: {}", mode),
        }
    }

    async fn output_by_clipboard(&self, text: String) -> Result<()> {
        #[cfg(target_os = "windows")]
        unsafe {
            let backup = crate::win_utils::get_clipboard_text();

            // 使用剪贴板粘贴兼容性最好，同时排除 Win+V 历史，尽量不打扰用户原来的剪贴板。
            crate::win_utils::set_clipboard_text(&text, true);
            crate::win_utils::paste_clipboard();

            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
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
        self.output_by_keyboard(text)?;

        Ok(())
    }

    fn output_by_keyboard(&self, text: String) -> Result<()> {
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
        if let Ok(re) = regex::Regex::new(r"[\p{Emoji_Presentation}\p{Extended_Pictographic}]") {
            result = re.replace_all(&result, "").to_string();
        }
    }

    if !config.allow_punctuation() {
        result = strip_punctuation(&result);
    }

    if let Ok(re) = regex::Regex::new(r"\s+") {
        result = re.replace_all(&result, " ").to_string();
    }

    result.trim().to_string()
}

fn strip_punctuation(text: &str) -> String {
    let is_numeric_separator = |c: char| matches!(c, '.' | ':' | ',' | '-');
    let Ok(punct_re) = regex::Regex::new(r"[\p{P}]") else {
        return text.to_string();
    };

    let chars: Vec<char> = text.chars().collect();
    let mut cleaned = String::with_capacity(text.len());

    for (i, &c) in chars.iter().enumerate() {
        if !punct_re.is_match(&c.to_string()) {
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
