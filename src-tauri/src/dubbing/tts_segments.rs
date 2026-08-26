//! 逐段 TTS 合成与配音时间轴拼装。
//!
//! 每个字幕分段调用 Fish Audio 合成 WAV，解码为统一规格（24kHz 单声道 s16），
//! 若合成时长超出原分段时长则自动提高语速重试一次；
//! 随后按「原起始时间 + 不重叠前移」规则流式写入配音音轨 WAV。

use std::io::BufWriter;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::config::TtsConfig;
use crate::tts::client::FishTtsClient;

/// 配音音轨统一采样率（人声 24k 足够，兼顾体积）
pub const TRACK_SAMPLE_RATE: u32 = 24_000;
/// 允许的轻微超时比例（超过才触发语速重试）
const FIT_TOLERANCE: f64 = 1.05;

/// 单段 TTS 结果：24kHz mono s16 原始样本与实际时长
pub struct SegmentAudio {
    pub samples: Vec<i16>,
    pub duration_ms: u64,
}

/// 将用户 TTS 配置克隆为「WAV 输出」版本（时间轴拼装需要可解码的 PCM）
pub fn wav_tts_config(base: &TtsConfig) -> TtsConfig {
    let mut cfg = base.clone();
    cfg.format = "wav".to_string();
    // 采样率交给 Fish 默认值，由本地重采样归一化
    cfg.sample_rate = 0;
    cfg
}

/// 判断是否为音色失效类错误（Fish Audio: Reference not found）
fn is_reference_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("reference not found")
        || lower.contains("reference_id")
            && (lower.contains("not found") || lower.contains("invalid"))
}

fn brief_err(msg: &str) -> String {
    msg.chars().take(120).collect()
}

/// 单次合成（含音色失效自动回退：显式选择的音色报 Reference not found 时，
/// 清除 reference_id 改用模型默认音色重试一次）
async fn synth_with_fallback(
    client: &FishTtsClient,
    cfg: &TtsConfig,
    text: &str,
    speed: f32,
) -> Result<Vec<u8>> {
    let mut c = cfg.clone();
    c.speed = speed;
    match client.synthesize(text, &c).await {
        Ok(bytes) => Ok(bytes),
        Err(e) if is_reference_error(&e.to_string()) && !c.reference_id.is_empty() => {
            log::warn!(
                "[dubbing] 所选音色不可用（{}），本段回退默认音色",
                brief_err(&e.to_string())
            );
            c.reference_id = String::new();
            client.synthesize(text, &c).await
        }
        Err(e) => Err(e),
    }
}

/// 合成单个分段并尽量贴合目标时长。
/// 超长时按比例提高 `prosody.speed` 重试一次（Fish 语速范围 0.5–2.0）。
pub async fn synthesize_segment(
    client: &FishTtsClient,
    cfg: &TtsConfig,
    text: &str,
    slot_ms: u64,
    base_speed: f32,
) -> Result<SegmentAudio> {
    let mut attempt_speed = base_speed;
    let mut best: Option<SegmentAudio> = None;

    for _ in 0..2 {
        let bytes = synth_with_fallback(client, cfg, text, attempt_speed).await?;
        let audio = decode_wav_to_track(&bytes)?;
        let duration = audio.duration_ms;

        // 保留时长更短的版本（超长段落重试后取更优者）
        if best.as_ref().is_none_or(|b| duration < b.duration_ms) {
            best = Some(audio);
        }
        if duration as f64 <= slot_ms as f64 * FIT_TOLERANCE {
            break;
        }

        // 计算需要的语速提升：时长比即语速比（近似反比关系）
        let ratio = duration as f64 / slot_ms.max(1) as f64;
        let next = (attempt_speed * ratio as f32).clamp(0.5, 2.0);
        if (next - attempt_speed).abs() < 0.01 {
            break; // 已到语速上限
        }
        log::info!(
            "[dubbing] 段落超长（{}ms / {}ms），语速 {} -> {} 重试",
            duration,
            slot_ms,
            attempt_speed,
            next
        );
        attempt_speed = next;
    }

    best.ok_or_else(|| anyhow!("TTS 合成失败：无有效音频"))
}

/// 解码 WAV 字节为音轨规格（24kHz mono s16）
pub fn decode_wav_to_track(bytes: &[u8]) -> Result<SegmentAudio> {
    let cursor = std::io::Cursor::new(bytes);
    let reader = hound::WavReader::new(cursor).context("TTS 返回的不是有效 WAV")?;
    let spec = reader.spec();
    let spec_desc = format!(
        "{:?}/{}bit/{}Hz/{}ch",
        spec.sample_format, spec.bits_per_sample, spec.sample_rate, spec.channels
    );

    let src_rate = spec.sample_rate as f64;
    let channels = spec.channels as usize;

    // 统一转为 f32 单声道。
    // 注意：Fish Audio 流式返回的 WAV 头部 data 块大小字段不可靠（常为 0xFFFFFF00），
    // hound 按头部长度读到 EOF 会报 IoError，因此把「样本流提前结束」视为正常结束。
    let mut mono: Vec<f64> = Vec::with_capacity(reader.duration() as usize / channels.max(1));
    let mut frame: Vec<f64> = Vec::with_capacity(channels);
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for s in reader.into_samples::<f32>() {
                let v = match s {
                    Ok(v) => v as f64,
                    Err(_) => break, // 数据真实结束（头部尺寸字段不可信）
                };
                frame.push(v);
                if frame.len() == channels {
                    mono.push(frame.iter().sum::<f64>() / channels as f64);
                    frame.clear();
                }
            }
        }
        hound::SampleFormat::Int => {
            let max_val = (1i64 << (spec.bits_per_sample.saturating_sub(1))) as f64;
            for s in reader.into_samples::<i32>() {
                let v = match s {
                    Ok(v) => v as f64 / max_val,
                    Err(_) => break, // 同上
                };
                frame.push(v);
                if frame.len() == channels {
                    mono.push(frame.iter().sum::<f64>() / channels as f64);
                    frame.clear();
                }
            }
        }
    }
    if mono.is_empty() {
        return Err(anyhow!("WAV 解码结果为空（{}）", spec_desc));
    }
    if !frame.is_empty() {
        // 尾部残缺帧：按已有声道均值补入
        mono.push(frame.iter().sum::<f64>() / frame.len() as f64);
    }

    // 线性插值重采样到 TRACK_SAMPLE_RATE
    let dst_rate = TRACK_SAMPLE_RATE as f64;
    let out_len = ((mono.len() as f64) * dst_rate / src_rate).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * src_rate / dst_rate;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(mono.len().saturating_sub(1));
        let frac = pos - i0 as f64;
        let v = mono[i0] * (1.0 - frac) + mono[i1] * frac;
        out.push(v.clamp(-1.0, 1.0) * i16::MAX as f64);
    }

    let duration_ms = (out.len() as f64 / dst_rate * 1000.0) as u64;
    Ok(SegmentAudio {
        samples: out.into_iter().map(|v| v as i16).collect(),
        duration_ms,
    })
}

/// 配音音轨流式写入器：按时间轴顺序写入样本，避免整条音轨驻留内存。
pub struct TimelineWriter {
    writer: hound::WavWriter<BufWriter<std::fs::File>>,
    cursor_samples: u64,
}

impl TimelineWriter {
    pub fn create(path: &Path) -> Result<Self> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: TRACK_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(path, spec).context("创建配音音轨文件失败")?;
        Ok(Self {
            writer,
            cursor_samples: 0,
        })
    }

    pub fn cursor_ms(&self) -> u64 {
        (self.cursor_samples as f64 / TRACK_SAMPLE_RATE as f64 * 1000.0) as u64
    }

    /// 在指定绝对时间写入一段音频；若与前一段重叠则顺延到当前游标之后。
    /// 返回该段实际的起始毫秒。
    pub fn write_at(&mut self, start_ms: u64, audio: &SegmentAudio) -> Result<u64> {
        let target = ms_to_samples(start_ms);
        let start = target.max(self.cursor_samples);
        if start > self.cursor_samples {
            self.write_silence(start - self.cursor_samples)?;
        }
        for &s in &audio.samples {
            self.writer.write_sample(s as i32)?;
        }
        self.cursor_samples += audio.samples.len() as u64;
        Ok(samples_to_ms(start))
    }

    fn write_silence(&mut self, n: u64) -> Result<()> {
        const CHUNK: u64 = 4800;
        let mut left = n;
        while left > 0 {
            let step = left.min(CHUNK);
            for _ in 0..step {
                self.writer.write_sample(0)?;
            }
            left -= step;
        }
        self.cursor_samples += n;
        Ok(())
    }

    /// 补齐静音至总时长并落盘
    pub fn finish(mut self, total_ms: u64) -> Result<()> {
        let total = ms_to_samples(total_ms);
        if total > self.cursor_samples {
            self.write_silence(total - self.cursor_samples)?;
        }
        // finalize 更新 WAV 头，BufWriter 在 Drop 时自动 flush
        self.writer.finalize().context("写配音音轨失败")?;
        Ok(())
    }
}

fn ms_to_samples(ms: u64) -> u64 {
    (ms as f64 * TRACK_SAMPLE_RATE as f64 / 1000.0).round() as u64
}

fn samples_to_ms(samples: u64) -> u64 {
    (samples as f64 / TRACK_SAMPLE_RATE as f64 * 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_reference_errors() {
        assert!(is_reference_error(
            r#"Fish Audio TTS 错误 400 Bad Request: {"message":"Reference not found","status":400}"#
        ));
        assert!(is_reference_error("reference_id invalid"));
        assert!(!is_reference_error("Fish Audio TTS 错误 401 Unauthorized"));
        assert!(!is_reference_error("network timeout"));
    }

    /// 构造指定时长/采样率/声道的正弦波 WAV 字节
    fn make_wav(rate: u32, channels: u16, dur_ms: usize) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let path = std::env::temp_dir().join(format!("v2t_make_{}.wav", uuid::Uuid::new_v4()));
        {
            let mut writer = hound::WavWriter::create(&path, spec).unwrap();
            let n = rate as usize * dur_ms / 1000;
            for i in 0..n {
                let v = ((i as f32 / rate as f32 * 440.0 * 2.0 * std::f32::consts::PI).sin()
                    * 8000.0) as i32;
                for _ in 0..channels {
                    writer.write_sample(v).unwrap();
                }
            }
            writer.finalize().unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        bytes
    }

    #[test]
    fn decodes_and_resamples() {
        // 44100Hz 立体声 500ms -> 24000Hz mono
        let bytes = make_wav(44_100, 2, 500);
        let audio = decode_wav_to_track(&bytes).unwrap();
        assert!((490..=520).contains(&audio.duration_ms));
        assert_eq!(audio.samples.len(), 12_000); // 24k * 0.5s
    }

    #[test]
    fn timeline_no_overlap_and_pad() {
        let path = std::env::temp_dir().join(format!("v2t_test_{}.wav", uuid::Uuid::new_v4()));
        {
            let mut w = TimelineWriter::create(&path).unwrap();
            let audio = SegmentAudio {
                samples: vec![100; 2400],
                duration_ms: 100,
            }; // 100ms @24k
               // 两段间隔 200ms：第二段应从 200ms 开始
            w.write_at(0, &audio).unwrap();
            w.write_at(200, &audio).unwrap();
            assert_eq!(w.cursor_ms(), 300);
            w.finish(1000).unwrap();
        }
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, TRACK_SAMPLE_RATE);
        assert_eq!(reader.duration(), 24_000); // 1000ms
        let samples: Vec<i32> = reader.samples::<i32>().map(|s| s.unwrap()).collect();
        // [0..2400]=声音, [2400..4800]=静音, [4800..7200]=声音, 其余静音
        assert!(samples[..2399].iter().any(|&s| s != 0));
        assert!(samples[2500..4700].iter().all(|&s| s == 0));
        assert!(samples[4900..7100].iter().any(|&s| s != 0));
        assert!(samples[10_000..].iter().all(|&s| s == 0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn timeline_overlap_shifts_forward() {
        let path = std::env::temp_dir().join(format!("v2t_test_{}.wav", uuid::Uuid::new_v4()));
        {
            let mut w = TimelineWriter::create(&path).unwrap();
            let audio = SegmentAudio {
                samples: vec![100; 4800],
                duration_ms: 200,
            }; // 200ms
            w.write_at(0, &audio).unwrap();
            // 第二段起始 100ms 与上一段重叠 -> 应顺延到 200ms 起
            let actual_start = w.write_at(100, &audio).unwrap();
            assert_eq!(actual_start, 200);
            w.finish(400).unwrap();
        }
        let _ = std::fs::remove_file(&path);
    }

    /// 真实 API 集成验证。需要有效 Key，手动运行：
    /// FISH_KEY=sk-xxx cargo test --release -- --ignored --nocapture tts_fallback_e2e
    #[tokio::test]
    #[ignore]
    async fn tts_fallback_e2e() {
        let key = std::env::var("FISH_KEY").expect("设置 FISH_KEY 环境变量");
        let client = FishTtsClient::new();

        // 场景 A（用户实际遇到的情况）：未选择音色 → 不携带 reference_id，直接成功
        let mut cfg = crate::config::TtsConfig::default();
        cfg.fish_api_key = key.clone();
        cfg.format = "wav".into();
        let a = synthesize_segment(&client, &cfg, "你好，世界。", 10_000, 1.0).await;
        match a {
            Ok(audio) => println!(
                "场景A（无音色）成功：{} 样本，{}ms",
                audio.samples.len(),
                audio.duration_ms
            ),
            Err(e) => panic!("场景A 失败: {}", e),
        }

        // 场景 B：显式失效音色 → 自动清除 reference_id 回退默认音色后成功
        let mut cfg_b = crate::config::TtsConfig::default();
        cfg_b.fish_api_key = key;
        cfg_b.format = "wav".into();
        cfg_b.reference_id = "00a1b221-6137-4b73-ad62-b0cbce134167".into();
        let b = synthesize_segment(&client, &cfg_b, "你好，世界。", 10_000, 1.0).await;
        match b {
            Ok(audio) => println!(
                "场景B（失效音色回退）成功：{} 样本，{}ms",
                audio.samples.len(),
                audio.duration_ms
            ),
            Err(e) => panic!("场景B 失败（回退未生效）: {}", e),
        }
    }
}
