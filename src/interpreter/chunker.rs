use tokio::sync::mpsc;

pub struct ChunkerConfig {
    pub chunk_ms: usize,
    pub overlap_ms: usize,
    pub silence_thresh: f32,
    pub min_silence_ms: usize,
    pub sample_rate: u32,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            chunk_ms: 3000,
            overlap_ms: 800,
            silence_thresh: 0.01,
            min_silence_ms: 400,
            sample_rate: 16000,
        }
    }
}

pub struct Chunker {
    config: ChunkerConfig,
    buffer: Vec<i16>,
    silence_start: Option<usize>,
}

impl Chunker {
    pub fn new(config: ChunkerConfig) -> Self {
        Self {
            config,
            buffer: Vec::new(),
            silence_start: None,
        }
    }

    pub fn process(&mut self, data: &[i16], tx: &mpsc::Sender<Vec<i16>>) {
        self.buffer.extend_from_slice(data);

        let chunk_samples = (self.config.chunk_ms * self.config.sample_rate as usize) / 1000;
        let overlap_samples = (self.config.overlap_ms * self.config.sample_rate as usize) / 1000;
        let min_silence_samples = (self.config.min_silence_ms * self.config.sample_rate as usize) / 1000;

        while self.buffer.len() >= chunk_samples {
            let rms = self.calculate_rms(&self.buffer[..chunk_samples]);

            if rms < self.config.silence_thresh {
                if self.silence_start.is_none() {
                    self.silence_start = Some(0);
                }
            } else {
                self.silence_start = None;
            }

            if let Some(start) = self.silence_start {
                let silence_samples = self.buffer.len().min(chunk_samples) - start;
                if silence_samples >= min_silence_samples {
                    let chunk = self.buffer[..chunk_samples].to_vec();
                    if let Err(e) = tx.try_send(chunk) {
                        log::warn!("Chunker send error: {}", e);
                    }
                    self.buffer.drain(0..chunk_samples - overlap_samples);
                    self.silence_start = None;
                    continue;
                }
            }

            if self.buffer.len() >= chunk_samples {
                let chunk = self.buffer[..chunk_samples].to_vec();
                if let Err(e) = tx.try_send(chunk) {
                    log::warn!("Chunker send error: {}", e);
                }
                self.buffer.drain(0..chunk_samples - overlap_samples);
                self.silence_start = None;
            } else {
                break;
            }
        }
    }

    fn calculate_rms(&self, data: &[i16]) -> f32 {
        if data.is_empty() {
            return 0.0;
        }

        let sum_squared: f64 = data.iter().map(|&x| (x as f64).powi(2)).sum();
        let mean_squared = sum_squared / data.len() as f64;
        (mean_squared.sqrt() / i16::MAX as f64) as f32
    }

    pub fn flush(&mut self, tx: &mpsc::Sender<Vec<i16>>) {
        if !self.buffer.is_empty() {
            if let Err(e) = tx.try_send(self.buffer.clone()) {
                log::warn!("Chunker flush error: {}", e);
            }
            self.buffer.clear();
        }
    }
}