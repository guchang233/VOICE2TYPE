//! 文本后处理器 trait 与本地纠错实现。
//!
//! 定义统一的 [`PostProcessor`] trait，替代当前分散在
//! `output::handler::post_process` 和 `streaming::post_process` 中的硬编码逻辑。
//!
//! 已实现：
//! - [`LocalCorrector`]：包装现有后处理逻辑（emoji/标点过滤、空格归一）
//!
//! 预留接口（后续阶段实现）：
//! - `LlmCorrector`：调用 GPT / Claude / 本地 LLM 进行智能校对
//! - 热词替换、专业词库

use crate::config::ConfigManager;
use crate::output::handler;

/// 后处理上下文。携带当前环境信息，供后处理器做上下文相关决策。
///
/// 第六阶段将填充 `active_window` / `application` / `language` 字段，
/// 使后处理器能根据目标窗口（VSCode / Word / 聊天软件）调整输出风格。
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// 当前活动窗口标题。
    pub active_window: String,
    /// 当前活动应用进程名。
    pub application: String,
    /// 目标输出语言（zh / en / auto）。
    pub language: String,
}

/// 统一文本后处理器 trait。
///
/// 实现者负责对 ASR 识别出的原始文本做确定性清洗或智能校对。
/// 多个后处理器可串联（Pipeline）执行。
pub trait PostProcessor: Send + Sync {
    /// 处理文本，返回处理后的结果。
    fn process(&self, text: String, context: &Context) -> String;

    /// 处理器名称（用于日志）。
    fn name(&self) -> &str;
}

/// 本地确定性后处理器。
///
/// 包装现有的 [`crate::output::handler::post_process`] 逻辑：
/// - emoji 过滤（受 `allow_emoji` 配置控制）
/// - 标点过滤（受 `allow_punctuation` 配置控制）
/// - 空白字符归一
///
/// 后续阶段将扩展：错别字修正（"因该"→"应该"）、热词替换、专业词库。
pub struct LocalCorrector {
    config: std::sync::Arc<ConfigManager>,
}

impl LocalCorrector {
    pub fn new(config: std::sync::Arc<ConfigManager>) -> Self {
        Self { config }
    }
}

impl PostProcessor for LocalCorrector {
    fn process(&self, text: String, context: &Context) -> String {
        let _ = context; // 当前未使用上下文，后续阶段将根据 application 调整
        handler::post_process(&text, &self.config)
    }

    fn name(&self) -> &str {
        "local-corrector"
    }
}
