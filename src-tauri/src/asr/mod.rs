//! 统一 ASR（自动语音识别）抽象层。
//!
//! 本模块定义了跨后端统一的 ASR 接口，使项目可以在不修改调用方的前提下
//! 切换底层语音识别引擎。当前已实现的适配器：
//!
//! - [`whisper::WhisperAsrEngine`]：本地 whisper.cpp（通过 whisper-cli 子进程）
//! - [`remote::RemoteAsrEngine`]：云端兼容 OpenAI 接口的批量 ASR（硅基流动 / Groq / 自定义）
//!
//! 两条接口：
//! - [`engine::AsrEngine`]：会话式批量识别（start → push_audio → stop 返回完整文本）
//! - [`streaming::StreamingAsrEngine`]：流式识别（push_audio 过程中实时返回中间结果）
//!
//! 未来可通过实现这两个 trait 接入：
//! FunASR、SenseVoice、Deepgram、Azure Speech 等。

pub mod engine;
pub mod remote;
pub mod streaming;
pub mod whisper;

pub use engine::AsrEngine;
pub use streaming::{AsrEvent, StreamingAsrEngine};
