//! 文本格式化 trait 与实现。
//!
//! 负责在后处理完成后对文本做格式化输出：
//! - 中文标点自动补全
//! - 自动换行
//! - Markdown 格式
//! - 代码模式（在 IDE 中输出代码块）
//!
//! ## 当前实现
//!
//! - [`TextFormatter`]：通用格式化器
//!   - 中文标点归一（半角→全角）
//!   - 自动换行（按指定宽度）
//!   - 代码模式（包裹 ``` 代码块）
//!
//! 后续阶段将根据 [`Context::application`] 分派到不同格式化器：
//! - VSCode → 代码格式
//! - Word → 正式文本
//! - 聊天软件 → 自然语言

use super::processor::Context;

/// 输出格式类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// 自然语言（聊天、文档）
    Plain,
    /// 代码（IDE 中输出）
    Code,
    /// Markdown 格式
    Markdown,
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Plain
    }
}

/// 统一文本格式化 trait。
pub trait Formatter: Send + Sync {
    /// 将后处理后的文本格式化为最终输出形式。
    fn format(&self, text: &str, context: &Context) -> String;

    /// 格式化器名称。
    fn name(&self) -> &str;
}

/// 通用文本格式化器。
///
/// 支持中文标点归一、自动换行、代码模式。
/// 后续阶段将根据 `Context.application` 自动选择格式。
pub struct TextFormatter {
    /// 自动换行宽度（字符数）。0 表示不换行。
    pub wrap_width: usize,
    /// 输出格式。
    pub output_format: OutputFormat,
}

impl Default for TextFormatter {
    fn default() -> Self {
        Self {
            wrap_width: 0, // 默认不换行
            output_format: OutputFormat::Plain,
        }
    }
}

impl TextFormatter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置自动换行宽度。
    pub fn with_wrap_width(mut self, width: usize) -> Self {
        self.wrap_width = width;
        self
    }

    /// 设置输出格式。
    pub fn with_format(mut self, format: OutputFormat) -> Self {
        self.output_format = format;
        self
    }

    /// 中文标点归一：将常见半角标点替换为全角。
    ///
    /// 仅在中文上下文中生效（文本含中文字符时）。
    /// 保留数字间的标点（如 3.14、1,000）。
    fn normalize_chinese_punctuation(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let has_cjk = chars.iter().any(|c| is_cjk_char(*c));
        if !has_cjk {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        for (i, &c) in chars.iter().enumerate() {
            // 保留数字间的标点
            if is_numeric_separator_context(&chars, i, c) {
                result.push(c);
                continue;
            }
            result.push(half_to_full_punct(c));
        }
        result
    }

    /// 自动换行：按指定宽度插入换行符。
    ///
    /// 在宽度边界处寻找最近的空格或中文标点断行。
    fn wrap_text(text: &str, width: usize) -> String {
        if width == 0 {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len() + text.len() / width);
        let mut current_len = 0usize;

        for line in text.lines() {
            if current_len > 0 {
                result.push('\n');
                current_len = 0;
            }

            let chars: Vec<char> = line.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let remaining = &chars[i..];
                let take = remaining.len().min(width - current_len);
                let chunk: String = remaining[..take].iter().collect();
                result.push_str(&chunk);
                current_len += take;
                i += take;

                if i < chars.len() {
                    // 寻找断行点：优先空格，其次标点
                    let break_at = find_break_point(&chars[i..], width - current_len);
                    if break_at > 0 {
                        result.push_str(&chars[i..i + break_at].iter().collect::<String>());
                        i += break_at;
                    }
                    result.push('\n');
                    current_len = 0;
                }
            }
        }

        result
    }

    /// 代码模式：用 ``` 包裹文本。
    fn format_as_code(text: &str) -> String {
        if text.contains("```") {
            // 已含代码块，不重复包裹
            return text.to_string();
        }
        format!("```\n{}\n```", text)
    }

    /// Markdown 格式：保持原样（Markdown 已是目标格式）。
    fn format_as_markdown(text: &str) -> String {
        text.to_string()
    }
}

impl Formatter for TextFormatter {
    fn format(&self, text: &str, context: &Context) -> String {
        let mut result = text.to_string();

        // 根据格式类型处理
        let format = if context.application.contains_ignore_case("code")
            || context.application.contains_ignore_case("vscode")
            || context.application.contains_ignore_case("idea")
        {
            OutputFormat::Code
        } else {
            self.output_format
        };

        match format {
            OutputFormat::Plain => {
                // 直接透传：handler::post_process 已处理标点/emoji/空格
                // 不做额外的标点归一，避免破坏 URL、版本号、时间等
            }
            OutputFormat::Code => {
                result = Self::format_as_code(&result);
            }
            OutputFormat::Markdown => {
                result = Self::format_as_markdown(&result);
            }
        }

        // 自动换行（代码模式除外）
        if format != OutputFormat::Code && self.wrap_width > 0 {
            result = Self::wrap_text(&result, self.wrap_width);
        }

        result
    }

    fn name(&self) -> &str {
        "text-formatter"
    }
}

/// 判断字符是否为 CJK 中日韩文字或全角标点。
fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF   // CJK 统一汉字
        | 0x3400..=0x4DBF // CJK 扩展 A
        | 0x3000..=0x303F // CJK 符号和标点
        | 0x3040..=0x309F // 平假名
        | 0x30A0..=0x30FF // 片假名
        | 0xFF00..=0xFFEF // 全角形式（含全角标点，如 ，。！？）
    )
}

/// 判断当前位置是否为数字间分隔符（应保留半角）。
fn is_numeric_separator_context(chars: &[char], i: usize, c: char) -> bool {
    if !matches!(c, '.' | ',' | ':' | '-' | '/') {
        return false;
    }
    i > 0 && i + 1 < chars.len() && chars[i - 1].is_ascii_digit() && chars[i + 1].is_ascii_digit()
}

/// 半角标点转全角（中文语境）。
fn half_to_full_punct(c: char) -> char {
    match c {
        ',' => '，',
        '.' => '。',
        '!' => '！',
        '?' => '？',
        ':' => '：',
        ';' => '；',
        '(' => '（',
        ')' => '）',
        _ => c,
    }
}

/// 在剩余文本中寻找合适的断行点。
fn find_break_point(chars: &[char], max_width: usize) -> usize {
    if chars.is_empty() || max_width == 0 {
        return 0;
    }
    // 优先在空格处断行
    for (i, &c) in chars.iter().take(max_width).enumerate() {
        if c == ' ' || c == '\t' {
            return i + 1;
        }
    }
    // 其次在标点处断行
    for (i, &c) in chars.iter().take(max_width).enumerate() {
        if matches!(c, '，' | '。' | '！' | '？' | '：' | '；' | ',' | '.') {
            return i + 1;
        }
    }
    0
}

/// 字符串忽略大小写包含判断（辅助 trait）。
trait ContainsIgnoreCase {
    fn contains_ignore_case(&self, needle: &str) -> bool;
}

impl ContainsIgnoreCase for str {
    fn contains_ignore_case(&self, needle: &str) -> bool {
        self.to_lowercase().contains(&needle.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_no_wrap_when_width_zero() {
        let formatter = TextFormatter::new();
        let ctx = Context::default();
        let result = formatter.format("这是一段很长的文本", &ctx);
        assert_eq!(result, "这是一段很长的文本");
    }

    #[test]
    fn wrap_text_wraps_at_width() {
        let formatter = TextFormatter::new().with_wrap_width(5);
        let ctx = Context::default();
        let result = formatter.format("一二三四五六七八九十", &ctx);
        assert!(result.contains('\n'));
    }

    #[test]
    fn format_as_code_wraps_with_backticks() {
        let result = TextFormatter::format_as_code("fn main() {}");
        assert!(result.starts_with("```\n"));
        assert!(result.ends_with("\n```"));
    }

    #[test]
    fn format_as_code_no_double_wrap() {
        let result = TextFormatter::format_as_code("```\nalready\n```");
        assert_eq!(result, "```\nalready\n```");
    }

    #[test]
    fn formatter_plain_passthrough_preserves_url() {
        // Plain 模式不应修改任何标点，URL 应保持完整
        let formatter = TextFormatter::new();
        let ctx = Context::default();
        let result = formatter.format("访问 http://example.com 查看详情", &ctx);
        assert_eq!(result, "访问 http://example.com 查看详情");
    }

    #[test]
    fn formatter_code_format_when_vscode_context() {
        let formatter = TextFormatter::new();
        let ctx = Context {
            application: "vscode".to_string(),
            ..Default::default()
        };
        let result = formatter.format("fn main() {}", &ctx);
        assert!(result.contains("```"));
    }

    #[test]
    fn formatter_markdown_passthrough() {
        let formatter = TextFormatter::new().with_format(OutputFormat::Markdown);
        let ctx = Context::default();
        let input = "# 标题\n\n正文";
        let result = formatter.format(input, &ctx);
        assert_eq!(result, input);
    }

    #[test]
    fn formatter_name() {
        let formatter = TextFormatter::new();
        assert_eq!(formatter.name(), "text-formatter");
    }

    #[test]
    fn output_format_default_is_plain() {
        assert_eq!(OutputFormat::default(), OutputFormat::Plain);
    }
}
