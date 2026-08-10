//! 豆包大模型流式语音识别（与录音文件识别模式隔离）

pub(crate) mod audio;
pub(crate) mod client;
mod output;
pub mod polish;
mod post_process;
pub(crate) mod protocol;
pub(crate) mod session;

pub use audio::{pick_best_input_config, push_samples_mono, start_capture, start_capture_with_prefs};
pub use client::StreamingAsrClient;
pub use protocol::AsrResponse;
