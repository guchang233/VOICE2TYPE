pub mod chunker;
pub mod loopback;
pub mod pipeline;
pub mod subtitle_window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::config::ConfigManager;
use chunker::AudioChunker;
use subtitle_window::SubtitleWindow;

pub struct InterpreterEngine {
    stop_flag: Arc<AtomicBool>,
    subtitle_window: SubtitleWindow,
    capture_handle: Option<std::thread::JoinHandle<()>>,
    chunker_handle: Option<std::thread::JoinHandle<()>>,
    pipeline_handle: Option<std::thread::JoinHandle<()>>,
}

impl InterpreterEngine {
    pub fn start(config: Arc<ConfigManager>) -> Result<Self, String> {
        let stop_flag = Arc::new(AtomicBool::new(false));

        let subtitle_window = SubtitleWindow::new(
            config.interpreter_subtitle_click_through(),
            config.interpreter_subtitle_opacity(),
            config.interpreter_subtitle_position(),
            config.interpreter_subtitle_font_size(),
        );

        let (audio_tx, audio_rx): (std::sync::mpsc::Sender<f32>, std::sync::mpsc::Receiver<f32>) =
            std::sync::mpsc::channel();

        let (sample_rate_tx, sample_rate_rx) = std::sync::mpsc::channel::<u32>();
        let (error_tx, error_rx) = std::sync::mpsc::channel::<String>();

        let capture_stop = stop_flag.clone();
        let capture_handle = loopback::start_loopback_capture(
            capture_stop,
            audio_tx,
            sample_rate_tx,
            error_tx,
        );

        let sample_rate = match sample_rate_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(rate) => rate,
            Err(_) => {
                stop_flag.store(true, Ordering::Relaxed);
                let _ = capture_handle.join();
                return Err("音频捕获初始化超时".to_string());
            }
        };

        if let Ok(err) = error_rx.try_recv() {
            stop_flag.store(true, Ordering::Relaxed);
            let _ = capture_handle.join();
            return Err(format!("音频捕获失败: {}", err));
        }

        let chunk_ms = config.interpreter_chunk_ms();
        let chunker_stop = stop_flag.clone();
        let (chunk_tx, chunk_rx): (
            std::sync::mpsc::Sender<Vec<u8>>,
            std::sync::mpsc::Receiver<Vec<u8>>,
        ) = std::sync::mpsc::channel();

        let chunker_handle = std::thread::spawn(move || {
            let mut chunker = AudioChunker::new(sample_rate, chunk_ms);
            loop {
                if chunker_stop.load(Ordering::Relaxed) {
                    chunker.force_flush(&chunk_tx);
                    break;
                }

                match audio_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(sample) => {
                        chunker.push_sample(sample, &chunk_tx);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        chunker.flush_if_stale(&chunk_tx);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        chunker.force_flush(&chunk_tx);
                        break;
                    }
                }
            }
        });

        let pipeline_stop = stop_flag.clone();
        let pipeline_config = config.clone();
        let subtitle = subtitle_window.clone();

        let pipeline_handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async move {
                loop {
                    if pipeline_stop.load(Ordering::Relaxed) {
                        break;
                    }

                    match chunk_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(wav_data) => {
                            if let Some(text) =
                                pipeline::process_chunk(wav_data, &pipeline_config).await
                            {
                                if !text.is_empty() {
                                    subtitle.show(text);
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        });

        Ok(Self {
            stop_flag,
            subtitle_window,
            capture_handle: Some(capture_handle),
            chunker_handle: Some(chunker_handle),
            pipeline_handle: Some(pipeline_handle),
        })
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        self.subtitle_window.hide();

        if let Some(handle) = self.capture_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.chunker_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.pipeline_handle.take() {
            let _ = handle.join();
        }
    }
}
