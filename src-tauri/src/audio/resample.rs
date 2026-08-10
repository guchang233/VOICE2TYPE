//! 重采样模块。
//!
//! 重导出 [`crate::audio::processor`] 中的重采样函数，作为新架构的入口。
//! 第二阶段将引入更高效的重采样实现（如 rubato 库），届时只需替换本模块内部。

pub use crate::audio::processor::{encode_wav_memory, resample_and_convert};

/// 重采样目标采样率（Whisper 与多数 ASR 引擎的标准采样率）。
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
