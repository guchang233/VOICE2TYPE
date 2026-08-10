//! 豆包大模型流式语音识别（与录音文件识别模式隔离）

pub(crate) mod audio;
pub(crate) mod client;
mod output;
pub mod polish;
mod post_process;
pub(crate) mod protocol;
pub(crate) mod session;

pub use client::StreamingAsrClient;
pub use protocol::AsrResponse;
