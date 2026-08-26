//! 逐段 TTS 合成与配音时间轴拼装。
//!
//! 每个字幕分段调用 Fish Audio 合成 WAV，解码为统一规格（24kHz 单声道 s16）；
//! 去除首尾静音后，用 ffmpeg `atempo` 变时基（不变调）把每段**精确**拉伸/压缩到
//! 原分段时长，保证每段都从原起始时间开口且时长完全贴合，一次直出；
//! atempo 失败时才回退为尾部截断，避免级联后移。

use std::io::BufWriter;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::config::TtsConfig;
use crate::tts::client::FishTtsClient;

/// 配音音轨统一采样率（人声 24k 足够，兼顾体积）
pub const TRACK_SAMPLE_RATE: u32 = 24_000;
/// 已贴合判定带宽：实测时长落在槽位时长 ±2% 内时跳过拉伸，避免无谓音质损耗
const FIT_BAND: f64 = 0.02;

/// 单段贴合统计：拉伸方式与回退情况
#[derive(Debug, Default)]
pub struct FitStats {
    /// 经 ffmpeg atempo 精确拉伸/压缩到槽位时长
    pub stretched: bool,
    /// 拉伸失败回退为尾部截断（保底不级联）
    pub truncated: bool,
}

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

/// 预估合成初始语速：按文本粗估自然时长与槽位时长的比值做温和预调，
/// 让后续 atempo 拉伸因子尽量接近 1（因子越极端音质损耗越大）。
pub fn estimate_fit_speed(text: &str, slot_ms: u64, base_speed: f32) -> f32 {
    if slot_ms == 0 {
        return base_speed;
    }
    let mut cjk = 0usize;
    let mut latin = 0usize;
    for c in text.chars() {
        match c {
            '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}' => cjk += 1,
            _ if c.is_alphanumeric() => latin += 1,
            _ => {}
        }
    }
    // 粗估：中日韩 ~4.8 字/秒，拉丁 ~13 字符/秒（含标点停顿的 TTS 自然语速）
    let est_ms = (cjk as f64 / 4.8 + latin as f64 / 13.0) * 1000.0;
    if est_ms < 200.0 {
        return base_speed;
    }
    let ratio = est_ms / slot_ms as f64;
    (base_speed as f64 * ratio).clamp(0.8, 1.5) as f32
}

/// 合成单个分段：单次调用（含音色失效回退）→ 解码到音轨规格 → 去首尾静音。
/// 时长精确贴合由 [`fit_to_slot`] 完成，不在此重试，保证一次直出。
pub async fn synthesize_segment(
    client: &FishTtsClient,
    cfg: &TtsConfig,
    text: &str,
    speed: f32,
) -> Result<SegmentAudio> {
    let bytes = synth_with_fallback(client, cfg, text, speed).await?;
    let mut audio = decode_wav_to_track(&bytes)?;
    trim_silence(&mut audio);
    Ok(audio)
}

/// 将音轨规格样本写出为 WAV 文件（24kHz mono s16）
pub fn write_track_wav(path: &Path, audio: &SegmentAudio) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TRACK_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).context("创建临时 WAV 失败")?;
    for &s in &audio.samples {
        writer.write_sample(s as i32)?;
    }
    writer.finalize().context("写入临时 WAV 失败")?;
    Ok(())
}

/// 把单段音频**精确**贴合到槽位时长（音画同步核心）：
/// ffmpeg `atempo` 变时基（不变调）拉伸/压缩到目标时长；
/// 失败时回退为尾部截断，保证后续段不被级联后移。
pub fn fit_to_slot(
    ff: &Path,
    temp_dir: &Path,
    index: usize,
    mut audio: SegmentAudio,
    slot_ms: u64,
) -> (SegmentAudio, FitStats) {
    let mut stats = FitStats::default();
    if slot_ms == 0 || audio.duration_ms == 0 {
        return (audio, stats);
    }
    let ratio = audio.duration_ms as f64 / slot_ms as f64;
    if (1.0 - FIT_BAND..=1.0 + FIT_BAND).contains(&ratio) {
        return (audio, stats); // 已在贴合带内，无需处理
    }

    let in_path = temp_dir.join(format!("seg_{:04}_in.wav", index));
    let out_path = temp_dir.join(format!("seg_{:04}_fit.wav", index));
    let res = (|| -> Result<SegmentAudio> {
        write_track_wav(&in_path, &audio)?;
        super::ffmpeg::time_stretch_wav(ff, &in_path, &out_path, ratio)?;
        let bytes = std::fs::read(&out_path).context("读取变速结果失败")?;
        decode_wav_to_track(&bytes)
    })();
    let _ = std::fs::remove_file(&in_path);
    let _ = std::fs::remove_file(&out_path);

    match res {
        Ok(mut fitted) => {
            stats.stretched = true;
            // 变速结果可能仍有个位数毫秒尾差，超出槽位的部分修掉（通常不会发生）
            let limit = ms_to_samples(slot_ms);
            if fitted.samples.len() as u64 > limit {
                fitted.samples.truncate(limit as usize);
                fitted.duration_ms = samples_to_ms(limit);
            }
            log::debug!(
                "[dubbing] 段 {} 精确贴合：{}ms -> {}ms（tempo {:.3}）",
                index + 1,
                audio.duration_ms,
                slot_ms,
                ratio
            );
            (fitted, stats)
        }
        Err(e) => {
            log::warn!(
                "[dubbing] 段 {} atempo 贴合失败（{}），回退尾部截断",
                index + 1,
                brief_err(&e.to_string())
            );
            let limit = ms_to_samples(slot_ms);
            if audio.samples.len() as u64 > limit {
                audio.samples.truncate(limit as usize);
                audio.duration_ms = samples_to_ms(limit);
                stats.truncated = true;
            }
            (audio, stats)
        }
    }
}

/// 去除首尾静音（10ms 帧能量检测，阈值相对峰值），让人声起点对齐分段起点。
/// 单侧最多修剪 500ms，避免异常样本删过头。
pub fn trim_silence(audio: &mut SegmentAudio) {
    const FRAME: usize = (TRACK_SAMPLE_RATE / 100) as usize; // 10ms
    const MAX_TRIM: usize = FRAME * 50; // 单侧上限 500ms
    let s = &audio.samples;
    if s.len() < FRAME * 4 {
        return;
    }
    let peak = s.iter().map(|&v| (v as i64).unsigned_abs()).max().unwrap_or(0);
    if peak == 0 {
        return; // 纯静音段不修剪，保留时长占位
    }
    // 平方能量阈值：(peak * 0.008)^2，用 u64 比较避免浮点
    let thr = ((peak as u64) * 8 / 1000).pow(2);
    let frame_loud = |start: usize| {
        let end = (start + FRAME).min(s.len());
        let mut sum: u64 = 0;
        for &v in &s[start..end] {
            let a = (v as i64).unsigned_abs();
            sum += a * a;
        }
        sum / (end - start) as u64 > thr
    };
    let frames = s.len() / FRAME;
    let mut lead = 0usize;
    while lead < frames.min(MAX_TRIM / FRAME) && !frame_loud(lead * FRAME) {
        lead += 1;
    }
    let mut tail = 0usize;
    while tail < frames.saturating_sub(lead).min(MAX_TRIM / FRAME)
        && !frame_loud((frames - 1 - tail) * FRAME)
    {
        tail += 1;
    }
    let cut_head = lead * FRAME;
    let cut_tail = tail * FRAME;
    if cut_head + cut_tail == 0 {
        return;
    }
    let new_len = s.len() - cut_head - cut_tail;
    if new_len < FRAME {
        return; // 修剪后过短，保留原样
    }
    audio.samples = audio.samples[cut_head..cut_head + new_len].to_vec();
    audio.duration_ms = samples_to_ms(audio.samples.len() as u64);
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

    #[test]
    fn trims_leading_and_trailing_silence() {
        // 100ms 静音 + 200ms 响音 + 150ms 静音 @24k → 修剪后约 200ms
        let frame = (TRACK_SAMPLE_RATE / 100) as usize;
        let mut samples = vec![0i16; frame * 10];
        samples.extend(vec![8000i16; frame * 20]);
        samples.extend(vec![0i16; frame * 15]);
        let mut audio = SegmentAudio {
            samples,
            duration_ms: 450,
        };
        trim_silence(&mut audio);
        assert!((195..=205).contains(&audio.duration_ms));
    }

    #[test]
    fn trim_keeps_pure_silence_placeholder() {
        // 纯静音段不修剪，保留时长占位（避免破坏时间轴）
        let mut audio = SegmentAudio {
            samples: vec![0i16; 24_000],
            duration_ms: 1000,
        };
        trim_silence(&mut audio);
        assert_eq!(audio.duration_ms, 1000);
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
        let a = synthesize_segment(&client, &cfg, "你好，世界。", 1.0).await;
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
        let b = synthesize_segment(&client, &cfg_b, "你好，世界。", 1.0).await;
        match b {
            Ok(audio) => println!(
                "场景B（失效音色回退）成功：{} 样本，{}ms",
                audio.samples.len(),
                audio.duration_ms
            ),
            Err(e) => panic!("场景B 失败（回退未生效）: {}", e),
        }
    }

    #[test]
    fn estimate_speed_clamped_to_band() {
        // 10 个中文字自然时长 ≈ 2083ms，槽位 1000ms → 比值 2.08 被限幅到 1.5×
        let s = estimate_fit_speed("一二三四五六七八九十", 1000, 1.0);
        assert!((1.49..=1.51).contains(&s));
        // 槽位远长于自然时长 → 减速下限 0.8×
        let s2 = estimate_fit_speed("你好", 60_000, 1.0);
        assert!((0.79..=0.81).contains(&s2));
        // 空文本/零槽位 → 基准语速
        assert_eq!(estimate_fit_speed("", 1000, 1.2), 1.2);
        assert_eq!(estimate_fit_speed("你好", 0, 1.2), 1.2);
    }

    #[test]
    fn fit_skips_within_band() {
        // 时长偏差 ±2% 内不处理（无需 ffmpeg）
        let td = std::env::temp_dir();
        let audio = SegmentAudio {
            samples: vec![100i16; 24_240],
            duration_ms: 1010,
        };
        let (out, stats) =
            fit_to_slot(std::path::Path::new("__no_such_ffmpeg__"), &td, 0, audio, 1000);
        assert!(!stats.stretched && !stats.truncated);
        assert_eq!(out.duration_ms, 1010);
    }

    #[test]
    fn fit_falls_back_to_truncation_without_ffmpeg() {
        // ffmpeg 不可用时回退尾部截断，保证不超出槽位（不级联后移）
        let td = std::env::temp_dir();
        let audio = SegmentAudio {
            samples: vec![100i16; 24_000], // 1000ms
            duration_ms: 1000,
        };
        let (out, stats) =
            fit_to_slot(std::path::Path::new("__no_such_ffmpeg__"), &td, 0, audio, 500);
        assert!(stats.truncated && !stats.stretched);
        assert!((495..=505).contains(&out.duration_ms));
    }
}
