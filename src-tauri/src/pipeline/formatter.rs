//! 文本格式化 trait。
//!
//! 负责在后处理完成后对文本做格式化输出：
//! - 中文标点自动补全
//! - 自动换行
//! - Markdown 格式
//! - 代码模式（在 IDE 中输出代码块）
//!
//! 第五阶段将实现具体逻辑，本阶段仅建立 trait 骨架。

use super::processor::Context;

/// 统一文本格式化 trait。
pub trait Formatter: Send + Sync {
    /// 将后处理后的文本格式化为最终输出形式。
    fn format(&self, text: &str, context: &Context) -> String;

    /// 格式化器名称。
    fn name(&self) -> &str;
}

/// 默认文本格式化器（透传，不做任何转换）。
///
/// 后续阶段将根据 `Context.application` 分派到不同格式化器：
/// - VSCode → 代码格式
/// - Word → 正式文本
/// - 聊天软件 → 自然语言
pub struct TextFormatter;

impl Formatter for TextFormatter {
    fn format(&self, text: &str, _context: &Context) -> String {
        text.to_string()
    }

    fn name(&self) -> &str {
        "text-formatter"
    }
}
