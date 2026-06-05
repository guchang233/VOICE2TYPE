//! 音频采集：打开默认麦克风，本地重采样为 16kHz mono i16。
//!
//! 返回 cpal::Stream 由调用方持有（！Send），因此必须在 LocalSet 中运行。

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// 启动采集，返回 (原始采样率, 原始通道数, cpal::Stream)。
/// `buffer` 中累积原始 f32 样本，调用方需定期 drain 并重采样。
pub fn start_capture(
    buffer: Arc<Mutex<Vec<f32>>>,
) -> Result<(u32, u16, cpal::Stream)> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no input device found")?;
    let name = device.name().unwrap_or_else(|_| "unknown".to_string());

    let mut supported = device
        .supported_input_configs()
        .context("no supported input config")?;
    let range = supported.next().context("no input config range")?;
    let config: cpal::StreamConfig = cpal::StreamConfig {
        channels: range.channels(),
        sample_rate: cpal::SampleRate(range.max_sample_rate().0.min(48_000)),
        buffer_size: cpal::BufferSize::Default,
    };

    let src_rate = config.sample_rate.0;
    let channels = config.channels;

    crate::utils::logger::write_log_line(
        &format!(
            "[音频] 设备: {}, 采样率: {}Hz, 通道数: {}",
            name, src_rate, channels
        ),
        None,
    );

    let stream = device.build_input_stream(
        &config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if let Ok(mut buf) = buffer.lock() {
                buf.extend_from_slice(data);
            }
        },
        |err| {
            crate::utils::logger::write_log_line(
                &format!("[音频] 采集错误: {}", err),
                None,
            );
        },
        None,
    )?;

    stream.play()?;
    Ok((src_rate, channels, stream))
}

/// 本地重采样到 16kHz mono i16。
/// 简易实现：线性插值重采样 + 取平均混音。
pub fn resample_to_16k_mono(samples: &[f32], src_rate: u32, channels: u16) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }

    let factor = src_rate as f64 / 16_000.0;
    let ch = channels as usize;
    let n_input = samples.len() / ch;
    let n_output = (n_input as f64 / factor).ceil() as usize;

    let mut out = Vec::with_capacity(n_output);

    for i in 0..n_output {
        let src_idx = (i as f64 * factor) as usize;
        let next_idx = (src_idx + 1).min(n_input - 1);

        let t = (i as f64 * factor) - src_idx as f64;

        let mut mono = 0.0f32;
        for c in 0..ch {
            let a = samples[src_idx * ch + c];
            let b = samples[next_idx * ch + c];
            mono += a + (b - a) * t as f32;
        }
        mono /= ch as f32;

        // clamp to i16
        let clamped = (mono * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
        out.push(clamped);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_empty() {
        let r = resample_to_16k_mono(&[], 48000, 1);
        assert!(r.is_empty());
    }

    #[test]
    fn resample_stereo() {
        // 48000Hz stereo: 480 samples = 120 frames per channel = 2.5ms
        // At 16kHz: ~40 samples
        let samples: Vec<f32> = (0..480).map(|i| (i as f32 / 480.0 - 0.5) * 2.0).collect();
        let r = resample_to_16k_mono(&samples, 48000, 2);
        assert!(!r.is_empty());
        // For 480 samples at 2ch (240 frames), 48000→16000 gives ~80 frames
        assert!(r.len() >= 60 && r.len() <= 100);
    }
}