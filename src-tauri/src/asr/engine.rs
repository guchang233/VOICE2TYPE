//! 统一 ASR 引擎接口（会话式批量识别）。
//!
//! 该 trait 面向"一段完整录音 → 一段完整文本"的识别场景。
//! 调用流程：
//! 1. `start()` 初始化会话
//! 2. `push_audio()` 持续推送音频帧（f32 归一化样本）
//! 3. `stop()` 结束会话并返回完整识别文本
//!
//! 适配器内部负责重采样、格式转换、模型加载等细节。

use anyhow::Result;
use async_trait::async_trait;

/// 统一 ASR 引擎接口。
///
/// 设计说明：
/// - `push_audio` 接收 `&[f32]`（-1.0..1.0 归一化样本），引擎内部完成重采样。
///   若上游已重采样到引擎期望采样率，`push_audio` 几乎零开销。
/// - 引擎期望的输入采样率通过 `set_input_sample_rate` 设置（非 trait 方法，
///   由具体适配器提供），默认 16kHz。
/// - `stop` 返回 `Result<String>`，错误显式传递而非 panic。
#[async_trait]
pub trait AsrEngine: Send {
    /// 开始一次识别会话。重复调用应重置内部状态。
    async fn start(&mut self) -> Result<()>;

    /// 推送音频数据（f32 归一化样本，-1.0..1.0）。
    /// 可多次调用，引擎内部累积。
    async fn push_audio(&mut self, audio: &[f32]) -> Result<()>;

    /// 结束会话并返回完整识别文本。
    /// 调用后引擎内部缓冲将被清空，可再次 `start` 新会话。
    async fn stop(&mut self) -> Result<String>;

    /// 引擎名称，用于日志（如 "whisper-local"、"cloud-groq"）。
    fn name(&self) -> &str;
}
