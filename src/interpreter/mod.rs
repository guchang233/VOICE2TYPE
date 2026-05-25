use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::config::ConfigManager;

mod chunker;
mod loopback;
mod pipeline;
mod subtitle_window;

pub use self::chunker::ChunkerConfig;
pub use self::pipeline::PipelineConfig;
pub use self::subtitle_window::SubtitleWindowConfig;

pub struct InterpreterEngine {
    running: Arc<AtomicBool>,
    config: Arc<ConfigManager>,
}

impl InterpreterEngine {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);

        let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel(100);
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel(100);
        let (result_tx, mut result_rx) = tokio::sync::mpsc::channel(100);

        let loopback = loopback::LoopbackCapture::start(audio_tx)?;

        let chunker_config = ChunkerConfig {
            chunk_ms: self.config.interpreter_chunk_ms(),
            overlap_ms: self.config.interpreter_overlap_ms(),
            silence_thresh: self.config.interpreter_silence_thresh(),
            min_silence_ms: self.config.interpreter_min_silence_ms(),
            sample_rate: 16000,
        };

        let mut chunker = chunker::Chunker::new(chunker_config);

        let subtitle_config = subtitle_window::SubtitleWindowConfig {
            font_size: self.config.interpreter_subtitle_font_size(),
            opacity: self.config.interpreter_subtitle_opacity(),
            position: self.config.interpreter_subtitle_position(),
            click_through: self.config.interpreter_subtitle_click_through(),
        };

        let subtitle_window = Arc::new(subtitle_window::SubtitleWindow::new(subtitle_config));

        let pipeline_config = PipelineConfig {
            use_translation: self.config.interpreter_use_translation(),
            source_language: self.config.interpreter_source_language(),
            target_language: self.config.interpreter_target_language(),
        };

        let pipeline = Arc::new(pipeline::Pipeline::new(pipeline_config));

        let running_clone = self.running.clone();
        let config_clone = self.config.clone();
        let chunk_tx_clone = chunk_tx.clone();

        tokio::spawn(async move {
            while running_clone.load(Ordering::SeqCst) {
                tokio::select! {
                    Some(audio) = audio_rx.recv() => {
                        chunker.process(&audio, &chunk_tx_clone);
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                }
            }
            chunker.flush(&chunk_tx_clone);
        });

        let running_clone2 = self.running.clone();
        let config_clone2 = self.config.clone();
        let pipeline_clone = pipeline.clone();
        let tx_clone = result_tx.clone();

        tokio::spawn(async move {
            while running_clone2.load(Ordering::SeqCst) {
                tokio::select! {
                    Some(chunk) = chunk_rx.recv() => {
                        let pipeline_inner = pipeline_clone.clone();
                        let cfg = config_clone2.clone();
                        let result_sender = tx_clone.clone();

                        tokio::spawn(async move {
                            match pipeline_inner.process_audio(chunk, &cfg).await {
                                Ok(text) => {
                                    let _ = result_sender.send(text).await;
                                }
                                Err(e) => {
                                    log::warn!("Pipeline error: {}", e);
                                }
                            }
                        });
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                }
            }
        });

        let running_clone3 = self.running.clone();
        let subtitle_clone = subtitle_window.clone();

        tokio::spawn(async move {
            while running_clone3.load(Ordering::SeqCst) {
                tokio::select! {
                    Some(text) = result_rx.recv() => {
                        subtitle_clone.update_subtitle(&text);
                        subtitle_clone.show();
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                }
            }
            subtitle_clone.hide();
        });

        let _ = loopback;

        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
