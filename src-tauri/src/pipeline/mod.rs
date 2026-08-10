//! 文本处理流水线：后处理与格式化。
//!
//! 本模块提供可扩展的文本后处理体系：
//!
//! - [`processor::PostProcessor`]：同步后处理器 trait
//! - [`processor::AsyncPostProcessor`]：异步后处理器 trait（LLM 场景）
//! - [`processor::LocalCorrector`]：本地确定性后处理（emoji/标点/空格/错别字）
//! - [`processor::LlmCorrector`]：LLM 智能校对（预留接口）
//! - [`processor::PostProcessorChain`]：处理器链
//! - [`formatter::Formatter`]：格式化器 trait
//! - [`formatter::TextFormatter`]：通用格式化（中文标点/换行/代码模式）
//! - [`vad::VadEngine`]：语音活动检测 trait
//! - [`vad::WebRtcVad`]：WebRTC VAD 实现
//!
//! ## 开关控制
//!
//! 通过配置项 `enable_post_processor` 控制是否启用新后处理链：
//! - `false`（默认）：使用现有 `output::handler::post_process`，行为不变
//! - `true`：启用 `PostProcessorChain` + `TextFormatter`
//!
//! 参见 [`processor::process_with_config`] 统一入口。

pub mod formatter;
pub mod processor;
pub mod vad;

pub use formatter::{Formatter, OutputFormat, TextFormatter};
pub use processor::{
    process_with_config, process_with_config_async, AsyncPostProcessor, Context, LocalCorrector,
    LlmCorrector, PostProcessor, PostProcessorChain,
};
pub use vad::{VadAggressiveness, VadConfig, VadDecision, VadEngine, WebRtcVad};
