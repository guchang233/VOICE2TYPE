//! 豆包大模型流式语音识别（与录音文件识别模式隔离）

mod client;
mod hotkey;
mod output;
mod polish;
mod post_process;
mod protocol;
mod session;

pub use hotkey::{start_streaming_hotkey_listener, StreamingInputMessage};
pub use session::{StreamingSession, IS_STREAMING};
