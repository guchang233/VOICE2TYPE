//! 音频采集：打开指定麦克风，按偏好选择配置，本地重采样为 16kHz mono i16。
//!
//! 返回 cpal::Stream 由调用方持有（！Send），因此必须在 LocalSet 中运行。

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, SupportedStreamConfigRange};
use std::sync::{Arc, Mutex};

/// 启动采集（简单签名，向后兼容；使用系统默认偏好）。
pub fn start_capture(
    buffer: Arc<Mutex<Vec<f32>>>,
    device_name: Option<&str>,
) -> Result<(u32, u16, cpal::Stream)> {
    start_capture_with_prefs(buffer, device_name, None, None, None, None)
}

/// 启动采集（带偏好）。
///
/// 偏好参数：
/// - `downmix_pref`：`average` | `strongest` | `first_channel`
/// - `sample_fmt_pref`：`auto` | `f32` | `i16` | `i32`（U8 一律被过滤）
/// - `sample_rate_pref`：`auto` | `16000` | `44100` | `48000`
/// - `channels_pref`：`auto` | `mono` | `stereo`
///
/// 任一参数传 `None` 即走默认（auto / strongest）。
#[allow(clippy::too_many_arguments)]
pub fn start_capture_with_prefs(
    buffer: Arc<Mutex<Vec<f32>>>,
    device_name: Option<&str>,
    downmix_pref: Option<&str>,
    sample_fmt_pref: Option<&str>,
    sample_rate_pref: Option<&str>,
    channels_pref: Option<&str>,
) -> Result<(u32, u16, cpal::Stream)> {
    let downmix = downmix_pref.unwrap_or("strongest").to_string();
    let sf_pref = sample_fmt_pref.unwrap_or("auto").to_string();
    let sr_pref = sample_rate_pref.unwrap_or("auto").to_string();
    let ch_pref = channels_pref.unwrap_or("auto").to_string();

    let host = cpal::default_host();
    let device = match device_name {
        Some(name) if !name.is_empty() => host
            .input_devices()?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .or_else(|| host.default_input_device())
            .context("no input device found")?,
        _ => host
            .default_input_device()
            .context("no input device found")?,
    };
    let name = device.name().unwrap_or_else(|_| "unknown".to_string());

    // ============== 选取最优 StreamConfig ==============
    // 优先级 1：尝试 default_input_config，检查是否满足偏好/质量阈值，不满足再走 fallback 链
    let (config, sample_format) =
        pick_best_input_config(&device, &sf_pref, &sr_pref, &ch_pref)?;

    let src_rate = config.sample_rate.0;
    let channels = config.channels;

    crate::utils::logger::write_log_line(
        &format!(
            "[音频] 设备: {}, 采样率: {}Hz, 通道数: {}, 样本格式: {:?}, 下混: {}",
            name, src_rate, channels, sample_format, downmix
        ),
    );

    let err_fn = |err| {
        crate::utils::logger::write_log_line(&format!("[音频] 采集错误: {}", err));
    };

    // 为每个样本格式分支，统一调用 push_samples_mono（会按 downmix 策略做混音）
    let stream = match sample_format {
        SampleFormat::F32 => {
            let buf = buffer.clone();
            let dm = downmix.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    push_samples_mono(buf.clone(), data, channels, &dm);
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::I16 => {
            let buf = buffer.clone();
            let dm = downmix.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let f: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    push_samples_mono(buf.clone(), &f, channels, &dm);
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::U16 => {
            let buf = buffer.clone();
            let dm = downmix.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let f: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                        .collect();
                    push_samples_mono(buf.clone(), &f, channels, &dm);
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::I32 => {
            let buf = buffer.clone();
            let dm = downmix.clone();
            device.build_input_stream(
                &config,
                move |data: &[i32], _: &cpal::InputCallbackInfo| {
                    let f: Vec<f32> = data.iter().map(|&s| s as f32 / i32::MAX as f32).collect();
                    push_samples_mono(buf.clone(), &f, channels, &dm);
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::U32 => {
            let buf = buffer.clone();
            let dm = downmix.clone();
            device.build_input_stream(
                &config,
                move |data: &[u32], _: &cpal::InputCallbackInfo| {
                    let f: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 / u32::MAX as f32) * 2.0 - 1.0)
                        .collect();
                    push_samples_mono(buf.clone(), &f, channels, &dm);
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::I8 => {
            let buf = buffer.clone();
            let dm = downmix.clone();
            device.build_input_stream(
                &config,
                move |data: &[i8], _: &cpal::InputCallbackInfo| {
                    let f: Vec<f32> = data.iter().map(|&s| s as f32 / i8::MAX as f32).collect();
                    push_samples_mono(buf.clone(), &f, channels, &dm);
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::U8 => {
            // 理论上 pick_best_input_config 已经过滤 U8；这里仍然兜底处理
            let buf = buffer.clone();
            let dm = downmix.clone();
            device.build_input_stream(
                &config,
                move |data: &[u8], _: &cpal::InputCallbackInfo| {
                    let f: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 / u8::MAX as f32) * 2.0 - 1.0)
                        .collect();
                    push_samples_mono(buf.clone(), &f, channels, &dm);
                },
                err_fn,
                None,
            )?
        }
        other => anyhow::bail!("不支持的样本格式: {:?}", other),
    };

    stream.play()?;
    Ok((src_rate, channels, stream))
}

// ======================================================================
// 公共辅助：配置选取 / 下混策略，subtitle.rs / recorder.rs 直接复用
// ======================================================================

/// 在给定设备上，按用户偏好挑选「最优」的 `(StreamConfig, SampleFormat)`。
///
/// 策略：
/// 1. 先尝试 `device.default_input_config()`（通常是厂家推荐配置），
///    若其样本格式为 U8（低质量）或不满足强偏好，才回退。
/// 2. 回退时从 `supported_input_configs()` 过滤掉 U8，再按：
///    采样格式匹配 → 声道匹配 → 采样率最接近 选出最优范围，
///    再从该范围内选取不超过偏好/48000 的最高采样率。
pub fn pick_best_input_config(
    device: &cpal::Device,
    sample_fmt_pref: &str,
    sample_rate_pref: &str,
    channels_pref: &str,
) -> Result<(cpal::StreamConfig, SampleFormat)> {
    // --- 步骤 1：尝试 default_input_config ---
    if let Ok(default_cfg) = device.default_input_config() {
        let fmt = default_cfg.sample_format();
        // 默认配置不是 U8，且不与强偏好冲突 → 用该默认配置的声道/格式，
        // 采样率若可在用户偏好中选取则再选一次（但默认配置仅有单一 sample_rate 值）
        if fmt != SampleFormat::U8 && is_default_acceptable(&default_cfg, sample_fmt_pref, channels_pref) {
            let fixed_sr = default_cfg.sample_rate().0;
            let sr = choose_sample_rate(fixed_sr, fixed_sr, sample_rate_pref);
            let ch = default_cfg.channels();
            return Ok((
                cpal::StreamConfig {
                    channels: ch,
                    sample_rate: SampleRate(sr),
                    buffer_size: cpal::BufferSize::Default,
                },
                fmt,
            ));
        }
    }

    // --- 步骤 2：从 supported_configs 中按偏好挑选 ---
    let supported: Vec<SupportedStreamConfigRange> = device
        .supported_input_configs()
        .context("no supported input config")?
        // 永远过滤掉 U8（8bit 无符号，音质极差，ASR 准确率低）
        .filter(|r| r.sample_format() != SampleFormat::U8)
        .collect();
    if supported.is_empty() {
        anyhow::bail!("无可用的非 U8 音频输入配置");
    }

    let best_range = select_best_range(&supported, sample_fmt_pref, channels_pref)
        .context("无匹配偏好的配置范围")?;
    let fmt = best_range.sample_format();
    let min_sr = best_range.min_sample_rate().0;
    let max_sr = best_range.max_sample_rate().0;
    let sr = choose_sample_rate(min_sr, max_sr, sample_rate_pref);
    let ch = best_range.channels();

    Ok((
        cpal::StreamConfig {
            channels: ch,
            sample_rate: SampleRate(sr),
            buffer_size: cpal::BufferSize::Default,
        },
        fmt,
    ))
}

/// 把交错的多声道 f32 样本下混为单声道后追加到 buffer。
/// `downmix`：`average` | `strongest` | `first_channel`
pub fn push_samples_mono(
    buffer: Arc<Mutex<Vec<f32>>>,
    data: &[f32],
    channels: u16,
    downmix: &str,
) {
    if let Ok(mut buf) = buffer.lock() {
        if channels == 1 {
            buf.extend_from_slice(data);
            return;
        }
        let ch = channels as usize;
        match downmix {
            "first_channel" => {
                for frame in data.chunks_exact(ch) {
                    buf.push(frame[0]);
                }
            }
            "strongest" => {
                // 计算每声道 RMS（本回调块内），选 RMS 最大者整段采用
                let n_frames = data.len() / ch;
                if n_frames == 0 {
                    return;
                }
                let mut rms = vec![0.0f64; ch];
                for frame in data.chunks_exact(ch) {
                    for (c, &s) in frame.iter().enumerate() {
                        let v = s as f64;
                        rms[c] += v * v;
                    }
                }
                let best = rms
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                for frame in data.chunks_exact(ch) {
                    buf.push(frame[best]);
                }
            }
            _ => {
                // average（旧行为，兜底）
                for frame in data.chunks_exact(ch) {
                    let mono: f32 = frame.iter().sum::<f32>() / ch as f32;
                    buf.push(mono);
                }
            }
        }
    }
}

// ====== 内部辅助函数 ======

fn is_default_acceptable(
    default_cfg: &cpal::SupportedStreamConfig,
    sample_fmt_pref: &str,
    channels_pref: &str,
) -> bool {
    // 采样格式偏好检查：auto 通过；f32/i16/i32 严格相等才通过
    match sample_fmt_pref {
        "f32" if default_cfg.sample_format() != SampleFormat::F32 => return false,
        "i16" if default_cfg.sample_format() != SampleFormat::I16 => return false,
        "i32" if default_cfg.sample_format() != SampleFormat::I32 => return false,
        _ => {}
    }
    // 声道偏好检查
    let actual_ch = default_cfg.channels();
    match channels_pref {
        "mono" if actual_ch != 1 => return false,
        "stereo" if actual_ch < 2 => return false,
        _ => {}
    }
    true
}

fn select_best_range(
    supported: &[SupportedStreamConfigRange],
    sample_fmt_pref: &str,
    channels_pref: &str,
) -> Option<SupportedStreamConfigRange> {
    use std::cmp::Ordering;

    let fmt_score = |fmt: SampleFormat| -> i32 {
        // 越大越优；auto 模式时用这个评分
        match fmt {
            SampleFormat::F32 => 100,
            SampleFormat::I16 => 90,
            SampleFormat::I32 => 80,
            SampleFormat::U16 => 50,
            SampleFormat::I8 => 30,
            _ => 0, // U8 已经被过滤
        }
    };

    let mut candidates: Vec<SupportedStreamConfigRange> = supported.to_vec();

    // 若有强样本格式偏好（非 auto），先筛出匹配的；筛空则放弃此约束
    if sample_fmt_pref != "auto" {
        let target_fmt = match sample_fmt_pref {
            "f32" => SampleFormat::F32,
            "i16" => SampleFormat::I16,
            "i32" => SampleFormat::I32,
            _ => SampleFormat::F32,
        };
        let filtered: Vec<_> = candidates
            .iter()
            .filter(|r| r.sample_format() == target_fmt)
            .cloned()
            .collect();
        if !filtered.is_empty() {
            candidates = filtered;
        }
    }

    // 声道偏好筛选
    if channels_pref != "auto" {
        let want_ch: Option<u16> = match channels_pref {
            "mono" => Some(1),
            "stereo" => Some(2),
            _ => None,
        };
        if let Some(want) = want_ch {
            let filtered: Vec<_> = candidates
                .iter()
                .filter(|r| {
                    if want == 1 {
                        r.channels() == 1
                    } else {
                        r.channels() >= want
                    }
                })
                .cloned()
                .collect();
            if !filtered.is_empty() {
                candidates = filtered;
            }
        }
    }

    // 综合打分：采样格式分 + （声道数 2→+5，多声道不惩罚）+ 最高采样率上限偏好
    candidates.into_iter().max_by(|a, b| {
        let sa = fmt_score(a.sample_format())
            + if a.channels() >= 2 { 5 } else { 0 }
            + a.max_sample_rate().0.min(48_000) as i32 / 1000;
        let sb = fmt_score(b.sample_format())
            + if b.channels() >= 2 { 5 } else { 0 }
            + b.max_sample_rate().0.min(48_000) as i32 / 1000;
        sa.cmp(&sb).then_with(|| {
            // 同分更倾向通道数少的（避免不必要的多声道）
            b.channels().cmp(&a.channels()).then(Ordering::Equal)
        })
    })
}

fn choose_sample_rate(min_sr: u32, max_sr: u32, pref: &str) -> u32 {
    let want: Option<u32> = match pref {
        "16000" => Some(16_000),
        "44100" => Some(44_100),
        "48000" => Some(48_000),
        _ => None,
    };
    let cap = max_sr.min(48_000);
    if let Some(w) = want {
        if w >= min_sr && w <= max_sr {
            return w;
        }
        // 超出范围 → 夹到 [min_sr, cap]
        return w.max(min_sr).min(cap);
    }
    // auto：优先最高但不超过 48kHz
    cap.max(min_sr)
}

/// 本地重采样到 16kHz mono i16。
/// 注意：输入 samples 必须已经是单声道（push_samples_mono 保证）。
pub fn resample_to_16k_mono(samples: &[f32], src_rate: u32, _channels: u16) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }

    let factor = src_rate as f64 / 16_000.0;
    let n_input = samples.len();
    let n_output = (n_input as f64 / factor).ceil() as usize;

    let mut out = Vec::with_capacity(n_output);

    for i in 0..n_output {
        let src_idx = (i as f64 * factor) as usize;
        let next_idx = (src_idx + 1).min(n_input - 1);
        let t = (i as f64 * factor) - src_idx as f64;
        let a = samples[src_idx];
        let b = samples[next_idx];
        let mono = a + (b - a) * t as f32;

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
    fn resample_mono() {
        // 48000Hz mono: 480 samples = 10ms → 160 samples @ 16kHz
        let samples: Vec<f32> = (0..480)
            .map(|i| (i as f32 / 480.0 - 0.5) * 2.0)
            .collect();
        let r = resample_to_16k_mono(&samples, 48000, 1);
        assert!(!r.is_empty());
        assert!(r.len() >= 150 && r.len() <= 170);
    }

    #[test]
    fn downmix_strategies_produce_same_frame_count() {
        // 2 声道 10 帧
        let data: Vec<f32> = (0..20).map(|i| i as f32 / 20.0 - 0.5).collect();
        let check = |strategy: &str| {
            let buf = Arc::new(Mutex::new(Vec::new()));
            push_samples_mono(buf.clone(), &data, 2, strategy);
            let r = buf.lock().unwrap().clone();
            assert_eq!(r.len(), 10, "strategy {} frame count mismatch", strategy);
            r
        };
        let a = check("average");
        let s = check("strongest");
        let f = check("first_channel");
        // first_channel 必须等于 data[0], data[2], ...
        for (i, &v) in f.iter().enumerate() {
            assert!((v - data[i * 2]).abs() < 1e-6);
        }
        // 简单断言：三种策略都产生非 NaN 有限值
        for v in a.iter().chain(s.iter()).chain(f.iter()) {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn choose_sample_rate_basic() {
        // auto：用上限夹到 48k
        assert_eq!(choose_sample_rate(8000, 192_000, "auto"), 48_000);
        assert_eq!(choose_sample_rate(8000, 44_100, "auto"), 44_100);
        // 指定值：范围内命中
        assert_eq!(choose_sample_rate(8000, 48_000, "16000"), 16_000);
        // 指定值：超出范围 → 夹到边界
        assert_eq!(choose_sample_rate(22_000, 44_100, "16000"), 22_000);
        assert_eq!(choose_sample_rate(8000, 32_000, "48000"), 32_000);
    }
}
