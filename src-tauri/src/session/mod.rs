//! 录音会话状态管理。
//!
//! 第七阶段将把现有的 `bool recording` 替换为状态机，
//! 所有录音状态变化必须经过状态机，方便支持：
//! - 按住说话（hold）
//! - 点击开始/停止（toggle）
//! - 自动监听模式（VAD 驱动）
//!
//! 本阶段仅建立状态机骨架，现有 [`crate::recorder::Recorder`] 保持不变。

pub mod recorder;

pub use recorder::RecorderState;
