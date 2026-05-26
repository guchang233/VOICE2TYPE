use std::sync::mpsc::Sender;

use crate::audio::processor::{encode_wav_memory, resample_and_convert};
use crate::utils::logger::{write_log, LogLevel};

const SILENCE_RMS_THRESHOLD: f32 = 0.01;
const SILENCE_DURATION_MS: u64 = 400;
const FLUSH_TIMEOUT_MS: u64 = 500;
const OVERLAP_RATIO: f64 = 0.3;

pub struct AudioChunker {
    buffer: Vec<f32>,
    sample_rate: u32,
    chunk_samples: usize,
    overlap_samples: usize,
    silence_samples_needed: usize,
    consecutive_silence: usize,
    last_data_time: std::time::Instant,
}

impl AudioChunker {
    pub fn new(sample_rate: u32, chunk_ms: u64) -> Self {
        let chunk_samples = (sample_rate as f64 * chunk_ms as f64 / 1000.0) as usize;
        let overlap_samples = (chunk_samples as f64 * OVERLAP_RATIO) as usize;
        let silence_samples_needed =
            (sample_rate as f64 * SILENCE_DURATION_MS as f64 / 1000.0) as usize;

        Self {
            buffer: Vec::with_capacity(chunk_samples * 2),
            sample_rate,
            chunk_samples,
            overlap_samples,
            silence_samples_needed,
            consecutive_silence: 0,
            last_data_time: std::time::Instant::now(),
        }
    }

    pub fn push_sample(&mut self, sample: f32, chunk_tx: &Sender<Vec<u8>>) {
        self.buffer.push(sample);
        self.last_data_time = std::time::Instant::now();

        if sample.abs() < SILENCE_RMS_THRESHOLD {
            self.consecutive_silence += 1;
        } else {
            self.consecutive_silence = 0;
        }

        if self.buffer.len() >= self.chunk_samples / 2
            && self.consecutive_silence >= self.silence_samples_needed
        {
            let split_point = self.buffer.len() - self.consecutive_silence;
            if split_point > self.sample_rate as usize / 5 {
                let chunk: Vec<f32> = self.buffer.drain(..split_point).collect();
                self.consecutive_silence = 0;
                send_wav_chunk(chunk, self.sample_rate, chunk_tx);
            }
        }

        if self.buffer.len() >= self.chunk_samples {
            let advance = self.chunk_samples - self.overlap_samples;
            let chunk: Vec<f32> = self.buffer[..self.chunk_samples].to_vec();
            self.buffer.drain(..advance);
            self.consecutive_silence = 0;
            send_wav_chunk(chunk, self.sample_rate, chunk_tx);
        }
    }

    pub fn flush_if_stale(&mut self, chunk_tx: &Sender<Vec<u8>>) {
        if self.last_data_time.elapsed().as_millis() >= FLUSH_TIMEOUT_MS as u128
            && self.buffer.len() >= self.sample_rate as usize / 5
        {
            self.force_flush(chunk_tx);
        }
    }

    pub fn force_flush(&mut self, chunk_tx: &Sender<Vec<u8>>) {
        if self.buffer.len() >= self.sample_rate as usize / 10 {
            let chunk: Vec<f32> = self.buffer.drain(..).collect();
            self.consecutive_silence = 0;
            send_wav_chunk(chunk, self.sample_rate, chunk_tx);
        } else {
            self.buffer.clear();
            self.consecutive_silence = 0;
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
