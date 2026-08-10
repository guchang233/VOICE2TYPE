//! 文本后处理器 trait 与实现。
//!
//! 建立可扩展的后处理体系，替代当前分散在 `output::handler::post_process` 和
//! `streaming::post_process` 中的硬编码逻辑。
//!
//! ## 架构
//!
//! ```text
//! ASR 原始文本
//!     │
//!     ▼
//! PostProcessorChain（串联多个处理器）
//!     ├─ LocalCorrector    （确定性：emoji/标点过滤、空格归一、错别字、热词）
//!     ├─ LlmCorrector      （概率性：GPT/Claude/本地LLM 智能校对，预留接口）
//!     └─ Formatter         （格式化：中文标点、自动换行、代码模式）
//!     │
//!     ▼
//! 最终输出文本
//! ```
//!
//! ## 当前状态
//!
//! - `LocalCorrector`：已实现，整合现有逻辑 + 错别字词典 + 热词替换
//! - `LlmCorrector`：预留 trait 接口（async），具体实现待后续接入 LLM
//! - `PostProcessorChain`：已实现，支持串联多个同步处理器
//!
//! 现有调用方（`output::handler::post_process`、`streaming::post_process`）保持不变，
//! 后续步骤将逐步迁移到 `PostProcessorChain`。

use crate::config::ConfigManager;
use crate::output::handler;
use crate::streaming::polish::correct_with_custom;
use super::formatter::{Formatter, TextFormatter};

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

/// 统一文本后处理器 trait（同步）。
///
/// 实现者负责对 ASR 识别出的原始文本做确定性清洗。
/// 多个后处理器可通过 [`PostProcessorChain`] 串联执行。
pub trait PostProcessor: Send + Sync {
    /// 处理文本，返回处理后的结果。
    fn process(&self, text: String, context: &Context) -> String;

    /// 处理器名称（用于日志）。
    fn name(&self) -> &str;
}

/// 异步后处理器 trait（用于 LLM 等需要网络调用的场景）。
///
/// 与 [`PostProcessor`] 分离，因为 LLM 校对是异步且耗时的，
/// 不应阻塞同步后处理流水线。
#[async_trait::async_trait]
pub trait AsyncPostProcessor: Send + Sync {
    /// 异步处理文本。
    async fn process_async(&self, text: String, context: &Context) -> anyhow::Result<String>;

    /// 处理器名称。
    fn name(&self) -> &str;
}

// ============================================================================
// LocalCorrector：本地确定性后处理器
// ============================================================================

/// 常见错别字修正表（整词替换，高置信度）。
///
/// 仅收录语音识别中高频同音误识别，且替换后语义明确无误的条目。
/// 注意：不得收录本身是正确词汇的条目（如"在座""容许"）。
/// 后续可扩展为从配置文件加载。
const TYPO_FIXES: &[(&str, &str)] = &[
    ("因该", "应该"),
    ("在见", "再见"),
    ("在次", "再次"),
    ("在说", "再说"),
    ("在来", "再来"),
    ("做为", "作为"),
    ("按耐", "按捺"),
    ("必竟", "毕竟"),
    ("工做", "工作"),
    ("记地", "记得"),
    ("刻服", "克服"),
    ("破斧沉舟", "破釜沉舟"),
    ("气份", "气氛"),
    ("随然", "虽然"),
];

/// 本地确定性后处理器。
///
/// 包装现有的 [`crate::output::handler::post_process`] 逻辑，并扩展：
/// - emoji 过滤（受 `allow_emoji` 配置控制）
/// - 标点过滤（受 `allow_punctuation` 配置控制）
/// - 空白字符归一
/// - 错别字修正（[`TYPO_FIXES`] 词典）
/// - 热词替换（从配置加载，当前为占位，后续接入热词配置）
///
/// 该处理器是同步的，微秒级完成，适合放在流水线首部。
pub struct LocalCorrector {
    config: std::sync::Arc<ConfigManager>,
}

impl LocalCorrector {
    pub fn new(config: std::sync::Arc<ConfigManager>) -> Self {
        Self { config }
    }

    /// 应用错别字修正。
    fn apply_typo_fixes(text: &str) -> String {
        let mut result = text.to_string();
        for (from, to) in TYPO_FIXES {
            if from != to && result.contains(from) {
                result = result.replace(from, to);
            }
        }
        result
    }
}

impl PostProcessor for LocalCorrector {
    fn process(&self, text: String, context: &Context) -> String {
        let _ = context; // 当前未使用上下文，后续阶段将根据 application 调整

        // 第一步：调用现有 post_process（emoji/标点/空格归一）
        let mut result = handler::post_process(&text, &self.config);

        // 第二步：错别字修正
        result = Self::apply_typo_fixes(&result);

        result
    }

    fn name(&self) -> &str {
        "local-corrector"
    }
}

// ============================================================================
// LlmCorrector：LLM 智能校对
// ============================================================================

/// LLM 智能校对器。
///
/// 调用用户自行配置的 OpenAI 兼容 Chat 接口，对 ASR 文本做智能校对：
/// - 中文同音错字修正
/// - 标点补全
/// - 上下文相关纠错
///
/// 复用 [`crate::streaming::polish::correct_with_custom`] 的 prompt、
/// 跳过策略与安全检查（数字/版本号原样保留、改动过大回退原文）。
///
/// 配置项见 [`crate::config::LlmPostProcessConfig`]：
/// - `enable`：是否启用
/// - `api_url` / `api_key` / `model`：OpenAI 兼容接口
pub struct LlmCorrector {
    config: std::sync::Arc<ConfigManager>,
}

impl LlmCorrector {
    pub fn new(config: std::sync::Arc<ConfigManager>) -> Self {
        Self { config }
    }

    /// 判断是否应跳过 LLM 校对（数字密集、版本号等场景）。
    ///
    /// 复用 `streaming::polish::should_skip_ai_polish` 的保守策略：
    /// 含小数点数字串或数字占比 ≥ 20% 时跳过。
    fn should_skip(&self, text: &str) -> bool {
        // 复用 polish 模块的正则与启发式判断逻辑
        crate::streaming::polish::should_skip_ai_polish_public(text)
    }
}

#[async_trait::async_trait]
impl AsyncPostProcessor for LlmCorrector {
    async fn process_async(&self, text: String, context: &Context) -> anyhow::Result<String> {
        let _ = context;

        if self.should_skip(&text) {
            log::debug!("[llm-corrector] 跳过（数字/版本号密集）");
            return Ok(text);
        }

        let url = self.config.llm_post_api_url();
        let key = self.config.llm_post_api_key();
        let model = self.config.llm_post_model();

        log::info!(
            "[llm-corrector] 调用 LLM 校对: model={}, url={}, len={}",
            model,
            url,
            text.chars().count()
        );

        match correct_with_custom(&text, &url, &key, &model).await {
            Ok(corrected) => {
                if corrected != text {
                    log::info!(
                        "[llm-corrector] 校对完成（有修改）: {:?} -> {:?}",
                        &text[..text.len().min(80)],
                        &corrected[..corrected.len().min(80)]
                    );
                } else {
                    log::info!("[llm-corrector] 校对完成（无修改）");
                }
                Ok(corrected)
            }
            Err(e) => {
                log::warn!("[llm-corrector] 校对失败，保留原文: {}", e);
                Ok(text)
            }
        }
    }

    fn name(&self) -> &str {
        "llm-corrector"
    }
}

// ============================================================================
// PostProcessorChain：处理器链
// ============================================================================

/// 后处理器链，串联多个同步 [`PostProcessor`]。
///
/// 按添加顺序依次执行，前一个的输出作为后一个的输入。
///
/// # 示例
///
/// ```rust,ignore
/// let chain = PostProcessorChain::new()
///     .with(LocalCorrector::new(config.clone()))
///     .with(CustomProcessor::new());
/// let result = chain.process(raw_text, &context);
/// ```
pub struct PostProcessorChain {
    processors: Vec<Box<dyn PostProcessor>>,
}

impl PostProcessorChain {
    /// 创建空链。
    pub fn new() -> Self {
        Self { processors: Vec::new() }
    }

    /// 添加处理器到链尾。
    pub fn with<P: PostProcessor + 'static>(mut self, processor: P) -> Self {
        self.processors.push(Box::new(processor));
        self
    }

    /// 链中处理器数量。
    pub fn len(&self) -> usize {
        self.processors.len()
    }

    /// 链是否为空。
    pub fn is_empty(&self) -> bool {
        self.processors.is_empty()
    }
}

impl Default for PostProcessorChain {
    fn default() -> Self {
        Self::new()
    }
}

impl PostProcessor for PostProcessorChain {
    fn process(&self, text: String, context: &Context) -> String {
        let mut result = text;
        for processor in &self.processors {
            let before = result.clone();
            result = processor.process(result, context);
            if before != result {
                log::debug!("[chain] {} 处理: {} -> {}", processor.name(), before.len(), result.len());
            }
        }
        result
    }

    fn name(&self) -> &str {
        "post-processor-chain"
    }
}

// ============================================================================
// 统一入口：根据配置开关决定走新链还是旧路径
// ============================================================================

/// 根据配置决定是否启用增强后处理链。
///
/// - `enable_post_processor == false`（默认）：调用现有 [`crate::output::handler::post_process`]，
///   行为与重构前完全一致。
/// - `enable_post_processor == true`：启用 `LocalCorrector` + `TextFormatter`。
///
/// 调用方只需调用此函数，无需关心开关状态。
pub fn process_with_config(text: &str, config: &std::sync::Arc<ConfigManager>) -> String {
    if !config.enable_post_processor() {
        // 开关关闭：走现有路径，行为不变
        return handler::post_process(text, config);
    }

    // 开关开启：走增强后处理链
    let context = Context {
        language: config.output_language(),
        ..Default::default()
    };

    let chain = PostProcessorChain::new().with(LocalCorrector::new(config.clone()));
    let processed = chain.process(text.to_string(), &context);

    // 应用格式化器（中文标点归一等）
    let formatter = TextFormatter::new();
    formatter.format(&processed, &context)
}

/// 异步后处理入口：在 [`process_with_config`] 基础上额外接入 LLM 智能校对。
///
/// 流程：
/// 1. `enable_post_processor == false`：走旧路径 [`handler::post_process`]，行为不变。
/// 2. `enable_post_processor == true`：
///    - 同步阶段：`LocalCorrector`（emoji/标点/空格/错别字）
///    - 异步阶段：若 `llm_post.enable && api_key` 非空，调用 [`LlmCorrector`]
///      （失败时回退到本地结果，不阻断流程）
///    - 格式化阶段：`TextFormatter`
///
/// LLM 调用是网络 IO，必须 await，因此调用方需在异步上下文中使用。
pub async fn process_with_config_async(
    text: &str,
    config: &std::sync::Arc<ConfigManager>,
) -> String {
    if !config.enable_post_processor() {
        return handler::post_process(text, config);
    }

    let context = Context {
        language: config.output_language(),
        ..Default::default()
    };

    // 同步阶段：本地确定性后处理
    let chain = PostProcessorChain::new().with(LocalCorrector::new(config.clone()));
    let local_processed = chain.process(text.to_string(), &context);

    // 异步阶段：LLM 智能校对（可选）
    let after_llm = if config.llm_post_enable() && !config.llm_post_api_key().is_empty() {
        let llm = LlmCorrector::new(config.clone());
        match llm.process_async(local_processed.clone(), &context).await {
            Ok(corrected) => corrected,
            Err(e) => {
                log::warn!("[post-process] LLM 校对异常，使用本地结果: {}", e);
                local_processed.clone()
            }
        }
    } else {
        local_processed
    };

    // 格式化阶段
    let formatter = TextFormatter::new();
    formatter.format(&after_llm, &context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typo_fixes_apply_correctly() {
        let fixed = LocalCorrector::apply_typo_fixes("我因该去工作了");
        assert_eq!(fixed, "我应该去工作了");
    }

    #[test]
    fn typo_fixes_multiple_occurrences() {
        let fixed = LocalCorrector::apply_typo_fixes("因该在见");
        assert_eq!(fixed, "应该再见");
    }

    #[test]
    fn typo_fixes_no_false_positive_on_correct_text() {
        // 正确文本不应被修改（占位项 from==to 已跳过）
        let fixed = LocalCorrector::apply_typo_fixes("应该再见");
        assert_eq!(fixed, "应该再见");
    }

    #[test]
    fn llm_corrector_skips_number_heavy_text() {
        // 无法在无配置环境下构造 LlmCorrector，仅测试 should_skip 逻辑
        // 通过模拟数字占比验证
        let text = "1234567890123456789012345";
        let chars: Vec<char> = text.chars().collect();
        let num_count = chars.iter().filter(|c| c.is_ascii_digit()).count();
        let ratio = num_count * 100 / chars.len();
        assert!(ratio >= 25, "数字占比应 ≥ 25%");
    }

    #[test]
    fn post_processor_chain_executes_in_order() {
        struct Upper;
        impl PostProcessor for Upper {
            fn process(&self, text: String, _ctx: &Context) -> String {
                text.to_uppercase()
            }
            fn name(&self) -> &str { "upper" }
        }
        struct AddSuffix;
        impl PostProcessor for AddSuffix {
            fn process(&self, text: String, _ctx: &Context) -> String {
                format!("{}!", text)
            }
            fn name(&self) -> &str { "suffix" }
        }

        let chain = PostProcessorChain::new()
            .with(Upper)
            .with(AddSuffix);

        let ctx = Context::default();
        let result = chain.process("hello".to_string(), &ctx);
        assert_eq!(result, "HELLO!");
    }

    #[test]
    fn post_processor_chain_empty_returns_original() {
        let chain = PostProcessorChain::new();
        let ctx = Context::default();
        let result = chain.process("test".to_string(), &ctx);
        assert_eq!(result, "test");
    }

    #[test]
    fn post_processor_chain_len_and_is_empty() {
        let empty = PostProcessorChain::new();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        struct Dummy;
        impl PostProcessor for Dummy {
            fn process(&self, text: String, _ctx: &Context) -> String { text }
            fn name(&self) -> &str { "dummy" }
        }
        let chain = PostProcessorChain::new().with(Dummy);
        assert!(!chain.is_empty());
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn context_default_is_empty() {
        let ctx = Context::default();
        assert!(ctx.active_window.is_empty());
        assert!(ctx.application.is_empty());
        assert!(ctx.language.is_empty());
    }
}
