//! 音频采集抽象层。
//!
//! 第二阶段将基于 lock-free 缓冲重构采集流程：
//! ```text
//! Audio callback → Ring Buffer → ASR 处理线程
//! ```
//! 保证音频线程永远快速返回，ASR 处理异步运行。
//!
//! 当前采集实现位于：
//! - [`crate::recorder::Recorder`]（批量模式）
//! - [`crate::streaming::audio::start_capture`]（流式模式）
//!
//! 本阶段仅建立 trait 骨架。

use anyhow::Result;

/// 音频采集器 trait。
///
/// 抽象麦克风采集，使采集实现可替换（cpal / WASAPI / 其他）。
pub trait AudioCapture: Send {
    /// 开始采集。
    ///
    /// `device_name` 为 None 或空字符串时使用默认输入设备。
    /// 返回采集到的实际采样率。
    fn start(&mut self, device_name: Option<&str>) -> Result<u32>;

    /// 停止采集。
    fn stop(&mut self) -> Result<()>;

    /// 是否正在采集。
    fn is_capturing(&self) -> bool;

    /// 当前采样率。
    fn sample_rate(&self) -> u32;
}
