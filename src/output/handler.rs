use anyhow::{Context, Result};

use crate::config::ConfigManager;

/// 输出处理器
#[derive(Clone)]
pub struct OutputHandler;

impl OutputHandler {
    /// 创建新的输出处理器
    pub fn new() -> Self {
        Self
    }

    /// 处理文本输出
    pub async fn handle_output(&self, text: String, config: &ConfigManager) -> Result<()> {
        let mode = config.output_mode();
        
        match mode.as_str() {
            "clipboard" => {
                #[cfg(target_os = "windows")]
                unsafe {
                    // 1. 备份当前剪贴板 (仅限文本)
                    let backup = crate::win_utils::get_clipboard_text();
                    
                    // 2. 写入新内容并粘贴 (排除在历史记录之外，不污染 Win+V)
                    crate::win_utils::set_clipboard_text(&text, true);
                    crate::win_utils::paste_clipboard();
                    
                    // 3. 延迟还原剪贴板 (同样排除在历史记录之外，避免重复记录)
                    if let Some(old_text) = backup {
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            crate::win_utils::set_clipboard_text(&old_text, true);
                        });
                    } else {
                        // 如果原本就是空的，延迟清空
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            use windows::Win32::System::DataExchange::{OpenClipboard, EmptyClipboard, CloseClipboard};
                            let _ = OpenClipboard(None);
                            let _ = EmptyClipboard();
                            let _ = CloseClipboard();
                        });
                    }
                }
            },
            "inject" => {
                #[cfg(target_os = "windows")]
                unsafe {
                    crate::win_utils::send_unicode_text(&text);
                }
                
                #[cfg(not(target_os = "windows"))]
                {
                    use enigo::{Enigo, Settings};
                    let mut enigo = Enigo::new(&Settings::default()).unwrap();
                    let _ = enigo.text(&text);
                }
            },
            _ => {
                anyhow::bail!("Unknown output mode: {}", mode);
            }
        }

        Ok(())
    }

    /// 处理流式文本输出
    pub fn handle_streaming_output(&self, text: String) {
        #[cfg(target_os = "windows")]
        unsafe {
            crate::win_utils::send_unicode_text(&text);
        }
        
        #[cfg(not(target_os = "windows"))]
        {
            use enigo::{Enigo, Settings};
            let mut enigo = Enigo::new(&Settings::default()).unwrap();
            let _ = enigo.text(&text);
        }
    }
}

/// 文本后处理
pub fn post_process(text: &str, config: &ConfigManager) -> String {
    let mut result = text.to_string();

    // 1. 过滤 Emoji
    if !config.allow_emoji() {
        // 匹配 Emoji 和象形文字的正则
        if let Ok(re) = regex::Regex::new(r"[\p{Emoji_Presentation}\p{Extended_Pictographic}]") {
            result = re.replace_all(&result, "").to_string();
        }
    }

    // 2. 过滤标点
    if !config.allow_punctuation() {
        // 定义需要特殊处理的数字内部标点
        // 我们只保护 ASCII 数字分隔符：点、冒号、逗号、连字符
        let is_numeric_separator = |c: char| matches!(c, '.' | ':' | ',' | '-');
        
        // 匹配任何标点符号的正则
        if let Ok(punct_re) = regex::Regex::new(r"[\p{P}]") {
             let chars: Vec<char> = result.chars().collect();
             let mut new_result = String::with_capacity(result.len());
              
             for (i, &c) in chars.iter().enumerate() {
                 let s = c.to_string();
                 if punct_re.is_match(&s) {
                     // 是标点，检查是否需要保留
                     let mut preserve = false;
                     if is_numeric_separator(c) {
                         let prev_is_digit = i > 0 && chars[i-1].is_ascii_digit();
                         let next_is_digit = i + 1 < chars.len() && chars[i+1].is_ascii_digit();
                         if prev_is_digit && next_is_digit {
                             preserve = true;
                         }
                     }
                     
                     if preserve {
                         new_result.push(c);
                     } else {
                         new_result.push(' ');
                     }
                 } else {
                     new_result.push(c);
                 }
             }
             result = new_result;
        }
    }

    // 3. 合并空格 (如果标点被替换或已有多个空格)
    if let Ok(re) = regex::Regex::new(r"\s+") {
        result = re.replace_all(&result, " ").to_string();
    }

    // 4. 去除首尾空格
    result.trim().to_string()
}