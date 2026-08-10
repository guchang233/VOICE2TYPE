//! 语音活动检测（VAD）trait。
//!
//! 第三阶段将实现具体 VAD 引擎（webrtc-vad 或 Silero VAD），
//! 用于自动判断用户是否正在讲话、过滤静音、自动断句。
//!
//! 建议参数（第三阶段实现时使用）：
//! - `speech_threshold`: 0.6
//! - `min_speech_duration`: 300ms
//! - `max_silence_duration`: 600ms
//!
//! 本阶段仅建立 trait 骨架。

/// VAD 检测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadDecision {
    /// 检测到语音。
    Speech,
    /// 检测到静音。
    Silence,
}

/// 语音活动检测引擎 trait。
///
/// 输入为 f32 音频帧，输出为该帧是否包含语音的判断。
/// 实现者内部维护状态（如静音持续时长），以支持自动断句。
pub trait VadEngine: Send {
    /// 处理一帧音频，返回语音/静音判断。
    ///
    /// `samples` 为 f32 归一化样本（-1.0..1.0），`sample_rate` 为该帧采样率。
    fn process_frame(&mut self, samples: &[f32], sample_rate: u32) -> VadDecision;

    /// 当前是否处于语音段中。
    fn is_in_speech(&self) -> bool;

    /// 静音是否已超过断句阈值（应结束当前语音段）。
    fn should_end_utterance(&self) -> bool;

    /// 重置内部状态。
    fn reset(&mut self);

    /// 引擎名称。
    fn name(&self) -> &str;
}
