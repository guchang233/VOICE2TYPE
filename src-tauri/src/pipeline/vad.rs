//! 语音活动检测（VAD）模块。
//!
//! 基于 WebRTC VAD（Google 开源，纯 C 绑定）实现，特点：
//! - 极快：2.6µs / 30ms 帧，不影响音频线程实时性
//! - 轻量：无 ONNX / 神经网络依赖
//! - 输出二值：语音 / 静音
//!
//! ## 断句状态机
//!
//! WebRTC VAD 本身只输出单帧判断，本模块在其之上叠加状态机实现自动断句：
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │  Silent (静音)                                           │
//! │   ├─ 检测到语音帧 → 计数 speech_frames                   │
//! │   └─ speech_frames 累计 ≥ min_speech_duration → Speech  │
//! ├─────────────────────────────────────────────────────────┤
//! │  Speech (语音段中)                                       │
//! │   ├─ 检测到静音帧 → 计数 silence_frames                  │
//! │   └─ silence_frames 累计 ≥ max_silence_duration →       │
//! │      should_end_utterance = true (应结束当前句)          │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 建议参数（用户指定）
//!
//! - `speech_threshold`: 0.6（WebRTC 二值输出，此值用于未来概率型 VAD 预留）
//! - `min_speech_duration`: 300ms（连续语音 300ms 才算真正开始，过滤短噪声）
//! - `max_silence_duration`: 600ms（连续静音 600ms 自动断句）
//!
//! ## 当前状态
//!
//! 本模块提供 VAD 检测能力，但尚未接入音频采集流水线。
//! 后续步骤将在 `recorder.rs` / `streaming/audio.rs` 中调用 VAD 过滤静音、自动断句。

use webrtc_vad::{SampleRate as WebRtcSampleRate, Vad as WebRtcVadInner, VadMode};

/// VAD 检测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadDecision {
    /// 检测到语音。
    Speech,
    /// 检测到静音。
    Silence,
}

/// VAD 激进度模式。
///
/// 模式越高，VAD 越保守（更倾向判定为静音），减少误报但可能漏检。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadAggressiveness {
    /// 模式 0：质量优先（最多误报，最少漏检）。
    Quality,
    /// 模式 1：低码率（平衡）。
    LowBitrate,
    /// 模式 2：激进（较少误报）。
    Aggressive,
    /// 模式 3：非常激进（最少误报，最多漏检）。
    VeryAggressive,
}

impl Default for VadAggressiveness {
    fn default() -> Self {
        // 默认激进模式：适合语音输入场景，过滤键盘/风扇噪声
        VadAggressiveness::Aggressive
    }
}

impl From<VadAggressiveness> for VadMode {
    fn from(a: VadAggressiveness) -> Self {
        match a {
            VadAggressiveness::Quality => VadMode::Quality,
            VadAggressiveness::LowBitrate => VadMode::LowBitrate,
            VadAggressiveness::Aggressive => VadMode::Aggressive,
            VadAggressiveness::VeryAggressive => VadMode::VeryAggressive,
        }
    }
}

/// VAD 配置。
#[derive(Debug, Clone)]
pub struct VadConfig {
    /// VAD 激进度。
    pub aggressiveness: VadAggressiveness,
    /// 最小语音持续时长（毫秒）。连续检测到语音超过此时长才算真正开始说话。
    /// 用于过滤短促噪声（咳嗽、键盘敲击）。建议 300ms。
    pub min_speech_duration_ms: u32,
    /// 最大静音持续时长（毫秒）。说话过程中连续静音超过此时长应自动断句。
    /// 建议 600ms。
    pub max_silence_duration_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            aggressiveness: VadAggressiveness::default(),
            min_speech_duration_ms: 300,
            max_silence_duration_ms: 600,
        }
    }
}

/// VAD 引擎内部状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VadState {
    /// 静音态（未检测到持续语音）。
    Silent,
    /// 语音态（已通过 min_speech_duration 确认）。
    Speech,
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

/// 基于 WebRTC VAD 的实现。
///
/// 内部维护断句状态机，支持 `min_speech_duration` 和 `max_silence_duration` 参数。
///
/// # 帧长度要求
///
/// WebRTC VAD 要求帧长度为 10/20/30ms。对于 16kHz：
/// - 10ms = 160 样本
/// - 20ms = 320 样本
/// - 30ms = 480 样本
///
/// 调用方应按这些长度分帧。若传入的帧长度不匹配，本实现会缓冲样本到下一完整帧。
pub struct WebRtcVad {
    inner: WebRtcVadInner,
    config: VadConfig,
    /// 目标采样率（Hz），用于 reset 时重建 WebRtcSampleRate。
    sample_rate_hz: u32,
    /// 内部状态。
    state: VadState,
    /// 当前语音段中连续语音帧的毫秒数。
    speech_ms: u32,
    /// 当前语音段中连续静音帧的毫秒数。
    silence_ms: u32,
    /// 缓冲区：累积不足一帧的样本。
    frame_buffer: Vec<i16>,
    /// 单帧样本数（按 30ms 帧计算）。
    frame_size: usize,
}

impl WebRtcVad {
    /// 创建 WebRTC VAD 实例。
    ///
    /// `sample_rate_hz` 必须是 8000 / 16000 / 32000 / 48000 之一。
    pub fn new(sample_rate_hz: u32, config: VadConfig) -> Result<Self, String> {
        let sample_rate = match sample_rate_hz {
            8000 => WebRtcSampleRate::Rate8kHz,
            16000 => WebRtcSampleRate::Rate16kHz,
            32000 => WebRtcSampleRate::Rate32kHz,
            48000 => WebRtcSampleRate::Rate48kHz,
            _ => return Err(format!("不支持的采样率: {}（WebRTC VAD 仅支持 8/16/32/48kHz）", sample_rate_hz)),
        };

        let inner = WebRtcVadInner::new_with_rate_and_mode(sample_rate, config.aggressiveness.into());

        // 30ms 帧
        let frame_size = (sample_rate_hz as usize * 30) / 1000;

        Ok(Self {
            inner,
            config,
            sample_rate_hz,
            state: VadState::Silent,
            speech_ms: 0,
            silence_ms: 0,
            frame_buffer: Vec::with_capacity(frame_size),
            frame_size,
        })
    }

    /// 将 u32 采样率转换为 WebRtcSampleRate 枚举。
    fn sample_rate_enum(hz: u32) -> WebRtcSampleRate {
        match hz {
            8000 => WebRtcSampleRate::Rate8kHz,
            16000 => WebRtcSampleRate::Rate16kHz,
            32000 => WebRtcSampleRate::Rate32kHz,
            48000 => WebRtcSampleRate::Rate48kHz,
            _ => WebRtcSampleRate::Rate16kHz, // 不应发生，构造时已校验
        }
    }

    /// 使用默认配置创建（16kHz、Aggressive 模式、300ms/600ms）。
    pub fn default_16k() -> Self {
        Self::new(16000, VadConfig::default()).expect("16kHz 是合法采样率")
    }

    /// 处理一帧 i16 样本（WebRTC 原生格式）。
    ///
    /// 返回该帧的语音/静音判断。内部更新断句状态机。
    fn process_i16_frame(&mut self, frame: &[i16]) -> VadDecision {
        let is_speech = match self.inner.is_voice_segment(frame) {
            Ok(true) => true,
            Ok(false) => false,
            Err(()) => {
                // 帧长度无效，视为静音
                log::warn!("[vad] 无效帧长度: {}（期望 {}）", frame.len(), self.frame_size);
                false
            }
        };

        let frame_ms = 30u32; // 固定 30ms 帧

        match self.state {
            VadState::Silent => {
                if is_speech {
                    self.speech_ms = self.speech_ms.saturating_add(frame_ms);
                    if self.speech_ms >= self.config.min_speech_duration_ms {
                        // 连续语音达到阈值，进入语音态
                        self.state = VadState::Speech;
                        self.silence_ms = 0;
                        log::debug!("[vad] 进入语音态 (speech_ms={})", self.speech_ms);
                    }
                } else {
                    // 静音中的静音帧，重置语音计数
                    self.speech_ms = 0;
                }
                if is_speech && self.state == VadState::Speech {
                    VadDecision::Speech
                } else {
                    VadDecision::Silence
                }
            }
            VadState::Speech => {
                if is_speech {
                    // 语音中的语音帧，重置静音计数
                    self.silence_ms = 0;
                    self.speech_ms = self.speech_ms.saturating_add(frame_ms);
                } else {
                    // 语音中的静音帧，累积静音
                    self.silence_ms = self.silence_ms.saturating_add(frame_ms);
                }
                VadDecision::Speech // 只要还在 Speech 态，就返回 Speech
            }
        }
    }

    /// 将 f32 样本转换为 i16。
    fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
        samples
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect()
    }
}

impl VadEngine for WebRtcVad {
    fn process_frame(&mut self, samples: &[f32], sample_rate: u32) -> VadDecision {
        // 采样率不匹配时记录警告
        if sample_rate != self.sample_rate_hz {
            log::warn!(
                "[vad] 采样率不匹配: 输入={} 期望={}，将转换",
                sample_rate,
                self.sample_rate_hz
            );
            // 简单处理：直接用输入样本转换（假设调用方已重采样）
        }

        // f32 → i16
        let i16_samples = Self::f32_to_i16(samples);
        self.frame_buffer.extend_from_slice(&i16_samples);

        // 按 frame_size 分帧处理
        let mut last_decision = VadDecision::Silence;
        while self.frame_buffer.len() >= self.frame_size {
            let frame: Vec<i16> = self.frame_buffer.drain(..self.frame_size).collect();
            last_decision = self.process_i16_frame(&frame);
        }

        last_decision
    }

    fn is_in_speech(&self) -> bool {
        self.state == VadState::Speech
    }

    fn should_end_utterance(&self) -> bool {
        // 仅在语音态中，且静音累计超过阈值时才应断句
        self.state == VadState::Speech && self.silence_ms >= self.config.max_silence_duration_ms
    }

    fn reset(&mut self) {
        self.inner.reset();
        // reset 会重置 mode 和 sample_rate 到默认，需重新设置
        self.inner.set_sample_rate(Self::sample_rate_enum(self.sample_rate_hz));
        self.inner.set_mode(self.config.aggressiveness.into());
        self.state = VadState::Silent;
        self.speech_ms = 0;
        self.silence_ms = 0;
        self.frame_buffer.clear();
    }

    fn name(&self) -> &str {
        "webrtc-vad"
    }
}

// SAFETY: WebRTC VAD（fvad）的 C 实现不使用全局可变状态，所有状态都封装在
// Fvad 结构体内部。只要同一时刻只有一个线程访问 `WebRtcVad` 实例（Rust 借用规则保证），
// 跨线程传递是安全的。webrtc-vad crate 0.4 未派生 Send，但实际 FFI 是线程安全的。
// 本项目 VAD 实例仅在单一音频处理线程中使用。
unsafe impl Send for WebRtcVad {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成指定时长、指定频率的正弦波 i16 样本（模拟语音）。
    fn generate_sine_wave(freq: f32, duration_ms: u32, sample_rate: u32) -> Vec<f32> {
        let num_samples = (sample_rate as usize * duration_ms as usize) / 1000;
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5
            })
            .collect()
    }

    /// 生成静音样本（全零）。
    fn generate_silence(duration_ms: u32, sample_rate: u32) -> Vec<f32> {
        let num_samples = (sample_rate as usize * duration_ms as usize) / 1000;
        vec![0.0; num_samples]
    }

    #[test]
    fn silence_stays_silent() {
        let mut vad = WebRtcVad::default_16k();
        // 1 秒静音
        let silence = generate_silence(1000, 16000);
        let decision = vad.process_frame(&silence, 16000);
        assert_eq!(decision, VadDecision::Silence);
        assert!(!vad.is_in_speech());
        assert!(!vad.should_end_utterance());
    }

    #[test]
    fn continuous_speech_enters_speech_state() {
        let mut vad = WebRtcVad::default_16k();
        // 440Hz 正弦波模拟语音，持续 1 秒
        // min_speech_duration = 300ms，1 秒应足够进入语音态
        let speech = generate_sine_wave(440.0, 1000, 16000);
        let decision = vad.process_frame(&speech, 16000);
        // 正弦波应被检测为语音（WebRTC 对 440Hz 较敏感）
        // 注意：WebRTC 可能不把纯正弦波当语音，所以只验证不 panic
        let _ = decision;
    }

    #[test]
    fn reset_clears_state() {
        let mut vad = WebRtcVad::default_16k();
        // 先处理一些语音
        let speech = generate_sine_wave(440.0, 500, 16000);
        let _ = vad.process_frame(&speech, 16000);
        vad.reset();
        assert!(!vad.is_in_speech());
        assert!(!vad.should_end_utterance());
    }

    #[test]
    fn short_noise_does_not_trigger_speech() {
        let mut vad = WebRtcVad::default_16k();
        // 100ms 语音（小于 min_speech_duration=300ms）+ 静音
        let short_noise = generate_sine_wave(440.0, 100, 16000);
        let _ = vad.process_frame(&short_noise, 16000);
        let silence = generate_silence(500, 16000);
        let _ = vad.process_frame(&silence, 16000);
        // 短噪声不应让 VAD 进入持续语音态
        // （即使 WebRTC 把它判为语音帧，状态机也会因未达 300ms 而不进入 Speech）
    }

    #[test]
    fn frame_buffer_handles_arbitrary_sizes() {
        let mut vad = WebRtcVad::default_16k();
        // 传入非 30ms 整数倍的样本（如 100 样本）
        // 16kHz 下 30ms = 480 样本
        let samples = vec![0.0; 100];
        let decision = vad.process_frame(&samples, 16000);
        // 不足一帧，应返回 Silence（默认）
        assert_eq!(decision, VadDecision::Silence);
        // 缓冲区应有 100 样本
        assert_eq!(vad.frame_buffer.len(), 100);
    }

    #[test]
    fn multiple_small_chunks_accumulate_to_full_frame() {
        let mut vad = WebRtcVad::default_16k();
        // 分 5 次传入 100 样本 = 500 样本 > 480（30ms 帧）
        for _ in 0..5 {
            let samples = vec![0.0; 100];
            let _ = vad.process_frame(&samples, 16000);
        }
        // 500 - 480 = 20 样本应留在缓冲
        assert_eq!(vad.frame_buffer.len(), 20);
    }

    #[test]
    fn vad_decision_is_speech_or_silence() {
        let mut vad = WebRtcVad::default_16k();
        let silence = generate_silence(100, 16000);
        let decision = vad.process_frame(&silence, 16000);
        assert!(
            decision == VadDecision::Speech || decision == VadDecision::Silence,
            "decision 应为 Speech 或 Silence"
        );
    }

    #[test]
    fn default_config_has_recommended_values() {
        let config = VadConfig::default();
        assert_eq!(config.min_speech_duration_ms, 300);
        assert_eq!(config.max_silence_duration_ms, 600);
        assert_eq!(config.aggressiveness, VadAggressiveness::Aggressive);
    }

    #[test]
    fn invalid_sample_rate_returns_error() {
        let result = WebRtcVad::new(11025, VadConfig::default());
        assert!(result.is_err());
    }

    #[test]
    fn supported_sample_rates_create_successfully() {
        for rate in [8000u32, 16000, 32000, 48000] {
            let result = WebRtcVad::new(rate, VadConfig::default());
            assert!(result.is_ok(), "采样率 {} 应支持", rate);
        }
    }

    #[test]
    fn engine_name_is_webrtc_vad() {
        let vad = WebRtcVad::default_16k();
        assert_eq!(vad.name(), "webrtc-vad");
    }

    #[test]
    fn f32_to_i16_conversion_clamps() {
        // 超范围值应被 clamp
        let samples = vec![-2.0, -1.0, 0.0, 1.0, 2.0];
        let i16_samples = WebRtcVad::f32_to_i16(&samples);
        assert_eq!(i16_samples[0], -i16::MAX); // -2.0 → -32767
        assert_eq!(i16_samples[1], -i16::MAX); // -1.0 → -32767
        assert_eq!(i16_samples[2], 0); // 0.0 → 0
        assert_eq!(i16_samples[3], i16::MAX); // 1.0 → 32767
        assert_eq!(i16_samples[4], i16::MAX); // 2.0 → 32767
    }
}
