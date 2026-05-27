use std::sync::mpsc::Sender;

use crate::audio::processor::{encode_wav_memory, resample_and_convert};
use crate::utils::logger::{write_log, LogLevel};

const SILENCE_RMS_THRESHOLD: f32 = 0.015;
const SILENCE_DURATION_MS: u64 = 400;
const FLUSH_TIMEOUT_MS: u64 = 800;
const OVERLAP_RATIO: f64 = 0.3;
const VAD_FRAME_SIZE: usize = 160;
const MIN_CHUNK_SECS: f64 = 0.5;

pub struct AudioChunker {
    buffer: Vec<f32>,
    sample_rate: u32,
    chunk_samples: usize,
    overlap_samples: usize,
    silent_frames_needed: usize,
    silent_frames: usize,
    vad_frame: Vec<f32>,
    last_data_time: std::time::Instant,
}

impl AudioChunker {
    pub fn new(sample_rate: u32, chunk_ms: u64) -> Self {
        let chunk_samples = (sample_rate as f64 * chunk_ms as f64 / 1000.0) as usize;
        let overlap_samples = (chunk_samples as f64 * OVERLAP_RATIO) as usize;
        let silent_frames_needed =
            (SILENCE_DURATION_MS as f64 / 1000.0 * sample_rate as f64 / VAD_FRAME_SIZE as f64) as usize;

        Self {
            buffer: Vec::with_capacity(chunk_samples * 2),
            sample_rate,
            chunk_samples,
            overlap_samples,
            silent_frames_needed,
            silent_frames: 0,
            vad_frame: Vec::with_capacity(VAD_FRAME_SIZE),
            last_data_time: std::time::Instant::now(),
        }
    }

    pub fn push_sample(&mut self, sample: f32, chunk_tx: &Sender<Vec<u8>>) {
        self.buffer.push(sample);
        self.last_data_time = std::time::Instant::now();

        self.vad_frame.push(sample);
        if self.vad_frame.len() >= VAD_FRAME_SIZE {
            let rms = (self.vad_frame.iter().map(|s| s * s).sum::<f32>()
                / self.vad_frame.len() as f32)
                .sqrt();
            if rms < SILENCE_RMS_THRESHOLD {
                self.silent_frames += 1;
            } else {
                self.silent_frames = 0;
            }
            self.vad_frame.clear();
        }

        let min_chunk = (self.sample_rate as f64 * MIN_CHUNK_SECS) as usize;

        if self.buffer.len() >= min_chunk
            && self.silent_frames >= self.silent_frames_needed
        {
            let silent_samples = self.silent_frames * VAD_FRAME_SIZE;
            let split_point = if self.buffer.len() > silent_samples {
                self.buffer.len() - silent_samples
            } else {
                self.buffer.len()
            };
            if split_point >= min_chunk {
                let chunk: Vec<f32> = self.buffer.drain(..split_point).collect();
                self.silent_frames = 0;
                send_wav_chunk(chunk, self.sample_rate, chunk_tx);
            }
        }

        if self.buffer.len() >= self.chunk_samples {
            let advance = self.chunk_samples - self.overlap_samples;
            let chunk: Vec<f32> = self.buffer[..self.chunk_samples].to_vec();
            self.buffer.drain(..advance);
            self.silent_frames = 0;
            send_wav_chunk(chunk, self.sample_rate, chunk_tx);
        }
    }

    pub fn flush_if_stale(&mut self, chunk_tx: &Sender<Vec<u8>>) {
        let min_chunk = (self.sample_rate as f64 * MIN_CHUNK_SECS) as usize;
        if self.last_data_time.elapsed().as_millis() >= FLUSH_TIMEOUT_MS as u128
            && self.buffer.len() >= min_chunk
        {
            self.force_flush(chunk_tx);
        }
    }

    pub fn force_flush(&mut self, chunk_tx: &Sender<Vec<u8>>) {
        let min_chunk = (self.sample_rate as f64 * MIN_CHUNK_SECS) as usize;
        if self.buffer.len() >= min_chunk {
            let chunk: Vec<f32> = self.buffer.drain(..).collect();
            self.silent_frames = 0;
            send_wav_chunk(chunk, self.sample_rate, chunk_tx);
        } else {
            self.buffer.clear();
            self.silent_frames = 0;
        }
    }
}

fn send_wav_chunk(samples: Vec<f32>, sample_rate: u32, chunk_tx: &Sender<Vec<u8>>) {
    let (processed, new_rate) = resample_and_convert(&samples, sample_rate);
    if let Ok(wav_data) = encode_wav_memory(&processed, new_rate) {
        let _ = chunk_tx.send(wav_data);
        write_log(LogLevel::DEBUG, &format!("[字幕] 产出音频块: {} 样本, {}Hz", samples.len(), sample_rate), None);
    }
}
