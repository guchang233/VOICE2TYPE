//! 流式 ASR 引擎接口。
//!
//! 与 [`crate::asr::engine::AsrEngine`]（会话式批量识别）不同，
//! 流式引擎在 `push_audio` 过程中实时通过事件回调返回中间识别结果，
//! 适合实时字幕、边说边写等场景。
//!
//! 当前流式实现（豆包大模型 WebSocket）位于 [`crate::streaming`] 模块，
//! 后续阶段将为其实现 `StreamingAsrEngine` 适配器。

use anyhow::Result;
use async_trait::async_trait;

/// 流式 ASR 事件。
#[derive(Debug, Clone)]
pub enum AsrEvent {
    /// 中间识别结果（partial，可能随后被修正）。
    Partial { text: String },
    /// 最终识别结果（final，该句已确定）。
    Final { text: String },
    /// 引擎错误。
    Error { message: String },
}

/// 流式 ASR 引擎接口。
///
/// 事件回调机制：实现者通过内部 channel 或 callback 将 [`AsrEvent`] 推送给调用方。
/// 具体订阅方式由适配器提供（如 `subscribe() -> Receiver<AsrEvent>`）。
#[async_trait]
pub trait StreamingAsrEngine: Send {
    /// 开始流式会话（建立连接、发送首包）。
    async fn start(&mut self) -> Result<()>;

    /// 推送音频帧。引擎应尽快返回，识别结果异步通过事件回调送达。
    async fn push_audio(&mut self, audio: &[f32]) -> Result<()>;

    /// 结束会话（发送尾包、关闭连接）。已产生的 Final 事件仍然有效。
    async fn stop(&mut self) -> Result<()>;

    /// 引擎名称。
    fn name(&self) -> &str;
}
