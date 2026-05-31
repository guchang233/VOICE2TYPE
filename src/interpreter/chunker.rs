use std::sync::mpsc::Sender;

use crate::audio::processor::{encode_wav_memory, resample_and_convert};
use crate::utils::logger::{write_log, LogLevel};

// 每帧 160 个样本（约 3ms@48kHz）计算一次 RMS，用于 VAD 判断
const VAD_FRAME_SIZE: usize = 160;

// 低于此 RMS 视为静音（0.0~1.0 归一化幅度）
const SILENCE_RMS_THRESHOLD: f32 = 0.015;

// 连续静音多少毫秒后触发切割
const SILENCE_DURATION_MS: u64 = 150;

const FLUSH_TIMEOUT_MS: u64 = 300;

const MIN_CHUNK_SECS: f64 = 0.3;

pub struct AudioChunker {
    // 原始样本缓冲区（native sample rate，未 resample）
    buffer: Vec<f32>,
    sample_rate: u32,
    // 超过此长度强制切割（对应 config 里的 chunk_ms）
    max_chunk_samples: usize,
    // VAD：正在积累的当前帧
    vad_frame: Vec<f32>,
    // 连续静音帧计数
    silent_frames: usize,
    // 达到此帧数则认为出现了足够长的静音
    silent_frames_needed: usize,
    // 最后一次收到样本的时刻，用于 flush_if_stale
    last_data_time: std::time::Instant,
}

impl AudioChunker {
    pub fn new(sample_rate: u32, chunk_ms: u64) -> Self {
        let max_chunk_samples = (sample_rate as f64 * chunk_ms as f64 / 1000.0) as usize;
        let silent_frames_needed =
            (SILENCE_DURATION_MS as f64 / 1000.0 * sample_rate as f64 / VAD_FRAME_SIZE as f64)
                .ceil() as usize;

        Self {
            buffer: Vec::with_capacity(max_chunk_samples),
            sample_rate,
            max_chunk_samples,
            vad_frame: Vec::with_capacity(VAD_FRAME_SIZE),
            silent_frames: 0,
            silent_frames_needed,
            last_data_time: std::time::Instant::now(),
        }
    }

    pub fn push_sample(&mut self, sample: f32, chunk_tx: &Sender<Vec<u8>>) {
        self.buffer.push(sample);
        self.last_data_time = std::time::Instant::now();

        // 积累 VAD 帧，满帧后判断静音
        self.vad_frame.push(sample);
        if self.vad_frame.len() >= VAD_FRAME_SIZE {
            let sum_sq: f32 = self.vad_frame.iter().map(|s| s * s).sum();
            let rms = (sum_sq / self.vad_frame.len() as f32).sqrt();
            if rms < SILENCE_RMS_THRESHOLD {
                self.silent_frames += 1;
            } else {
                self.silent_frames = 0;
            }
            self.vad_frame.clear();
        }

        let min_chunk = (self.sample_rate as f64 * MIN_CHUNK_SECS) as usize;

        // 条件 1：检测到足够长的静音 → 在静音开始处切割
        if self.buffer.len() >= min_chunk && self.silent_frames >= self.silent_frames_needed {
            // split_point 指向静音段的起点（不含静音本身）
            let silent_samples = (self.silent_frames * VAD_FRAME_SIZE)
                .min(self.buffer.len().saturating_sub(min_chunk));
            let split_point = self.buffer.len() - silent_samples;

            if split_point >= min_chunk {
                let chunk: Vec<f32> = self.buffer.drain(..split_point).collect();
                self.silent_frames = 0;
                self.vad_frame.clear();
                send_wav_chunk(chunk, self.sample_rate, chunk_tx);
                return;
            }
        }

        // 条件 2：缓冲区达到最大长度 → 强制切割，不保留重叠
        if self.buffer.len() >= self.max_chunk_samples {
            let chunk: Vec<f32> = self.buffer.drain(..).collect();
            self.silent_frames = 0;
            self.vad_frame.clear();
            send_wav_chunk(chunk, self.sample_rate, chunk_tx);
        }
    }

    /// 被主循环每 100ms 调用一次；若缓冲区有足够数据且长时间没有新样本则 flush
    pub fn flush_if_stale(&mut self, chunk_tx: &Sender<Vec<u8>>) {
        let min_chunk = (self.sample_rate as f64 * MIN_CHUNK_SECS) as usize;
        if self.last_data_time.elapsed().as_millis() >= FLUSH_TIMEOUT_MS as u128
            && self.buffer.len() >= min_chunk
        {
            self.force_flush(chunk_tx);
        }
    }

    /// 停止时调用，把剩余缓冲区全部发出
    pub fn force_flush(&mut self, chunk_tx: &Sender<Vec<u8>>) {
        let min_chunk = (self.sample_rate as f64 * MIN_CHUNK_SECS) as usize;
        if self.buffer.len() >= min_chunk {
            let chunk: Vec<f32> = self.buffer.drain(..).collect();
            self.silent_frames = 0;
            self.vad_frame.clear();
            send_wav_chunk(chunk, self.sample_rate, chunk_tx);
        } else {
            self.buffer.clear();
            self.silent_frames = 0;
            self.vad_frame.clear();
        }
    }
}

fn send_wav_chunk(samples: Vec<f32>, sample_rate: u32, chunk_tx: &Sender<Vec<u8>>) {
    let (processed, new_rate) = resample_and_convert(&samples, sample_rate);
    if let Ok(wav_data) = encode_wav_memory(&processed, new_rate) {
        write_log(
            LogLevel::DEBUG,
            &format!(
                "[字幕] 产出音频块: {:.2}s ({}样本 @{}Hz → {}样本 @{}Hz)",
                samples.len() as f64 / sample_rate as f64,
                samples.len(),
                sample_rate,
                processed.len(),
                new_rate,
            ),
            None,
        );
        let _ = chunk_tx.send(wav_data);
    }
}