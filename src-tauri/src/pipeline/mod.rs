//! 语音处理 Pipeline 模块。
//!
//! 包含 ASR 识别后的文本处理流水线：
//! - [`processor::PostProcessor`]：后处理 trait（错别字修正、热词替换、emoji/标点过滤）
//! - [`formatter::Formatter`]：文本格式化 trait（中文标点、自动换行、Markdown、代码模式）
//! - [`vad::VadEngine`]：语音活动检测 trait（第三阶段实现）
//!
//! 当前已有的后处理逻辑（`output::handler::post_process` 和
//! `streaming::post_process::process_streaming_text`）将在后续阶段迁移为
//! `PostProcessor` 的具体实现，本阶段仅建立 trait 骨架与适配入口。

pub mod formatter;
pub mod processor;
pub mod vad;

pub use formatter::{Formatter, TextFormatter};
pub use processor::{Context, LocalCorrector, PostProcessor};
pub use vad::{VadAggressiveness, VadConfig, VadDecision, VadEngine, WebRtcVad};
