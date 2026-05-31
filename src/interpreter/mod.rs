pub mod chunker;
pub mod loopback;
pub mod pipeline;
pub mod subtitle_window;
pub mod text_segment;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use crate::config::ConfigManager;
use crate::utils::logger::{write_log, write_log_line, LogLevel};
use chunker::AudioChunker;
use subtitle_window::SubtitleWindow;
use text_segment::SentenceBuffer;

pub static AUDIO_SOURCE: AtomicU8 = AtomicU8::new(0);

pub struct InterpreterEngine {
    stop_flag: Arc<AtomicBool>,
    pub subtitle_window: SubtitleWindow,
    capture_handle: Option<std::thread::JoinHandle<()>>,
    chunker_handle: Option<std::thread::JoinHandle<()>>,
    pipeline_handle: Option<std::thread::JoinHandle<()>>,
}

impl InterpreterEngine {
    pub fn start(config: Arc<ConfigManager>) -> Result<Self, String> {
        let stop_flag = Arc::new(AtomicBool::new(false));

        let audio_source_val = if config.interpreter_audio_source() == "microphone" {
            loopback::AUDIO_SOURCE_MICROPHONE
        } else {
            loopback::AUDIO_SOURCE_SPEAKER
        };
        AUDIO_SOURCE.store(audio_source_val, Ordering::SeqCst);

        let audio_source = Arc::new(AtomicU8::new(audio_source_val));

        let subtitle_window = SubtitleWindow::new(
            config.interpreter_subtitle_click_through(),
            config.interpreter_subtitle_opacity(),
            config.interpreter_subtitle_position(),
            config.interpreter_subtitle_font_size(),
        );

        subtitle_window.set_original_font_size(config.interpreter_original_font_size());
        subtitle_window.set_original_color(config.interpreter_original_color());
        subtitle_window.set_translated_font_size(config.interpreter_translated_font_size());
        subtitle_window.set_translated_color(config.interpreter_translated_color());

        subtitle_window::SUBTITLE_ALWAYS_VISIBLE.store(config.interpreter_always_visible(), Ordering::SeqCst);

        write_log_line("[字幕] 正在初始化实时字幕引擎...", None);

        let (audio_tx, audio_rx): (std::sync::mpsc::Sender<f32>, std::sync::mpsc::Receiver<f32>) =
            std::sync::mpsc::channel();

        let (sample_rate_tx, sample_rate_rx) = std::sync::mpsc::channel::<u32>();
        let (error_tx, error_rx) = std::sync::mpsc::channel::<String>();

        let capture_stop = stop_flag.clone();
        let capture_audio_source = audio_source.clone();
        let capture_handle = loopback::start_loopback_capture(
            capture_stop,
            capture_audio_source,
            audio_tx,
            sample_rate_tx,
            error_tx,
        );

        let sample_rate = match sample_rate_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(rate) => rate,
            Err(_) => {
                stop_flag.store(true, Ordering::Relaxed);
                let _ = capture_handle.join();
                write_log(LogLevel::ERROR, "[字幕] 音频捕获初始化超时", None);
                return Err("音频捕获初始化超时".to_string());
            }
        };

        write_log(LogLevel::INFO, &format!("[字幕] 获取采样率: {}", sample_rate), None);

        if let Ok(err) = error_rx.try_recv() {
            stop_flag.store(true, Ordering::Relaxed);
            let _ = capture_handle.join();
            write_log(LogLevel::ERROR, &format!("[字幕] 音频捕获错误: {}", err), None);
            return Err(format!("音频捕获失败: {}", err));
        }

        let chunk_ms = config.interpreter_chunk_ms();
        let chunker_stop = stop_flag.clone();
        let (chunk_tx, chunk_rx): (
            std::sync::mpsc::Sender<Vec<u8>>,
            std::sync::mpsc::Receiver<Vec<u8>>,
        ) = std::sync::mpsc::channel();

        let chunker_handle = std::thread::spawn(move || {
            let mut sample_rate = sample_rate;
            let mut chunker = AudioChunker::new(sample_rate, chunk_ms);
            loop {
                if chunker_stop.load(Ordering::Relaxed) {
                    chunker.force_flush(&chunk_tx);
                    break;
                }

                let new_rate = loopback::CURRENT_SAMPLE_RATE.load(Ordering::SeqCst);
                if new_rate > 0 && new_rate != sample_rate {
                    chunker.force_flush(&chunk_tx);
                    sample_rate = new_rate;
                    chunker = AudioChunker::new(sample_rate, chunk_ms);
                    write_log(LogLevel::INFO, &format!("[字幕] 采样率更新: {}Hz", sample_rate), None);
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

        write_log_line("[字幕] 音频分块线程已启动", None);

        let pipeline_stop = stop_flag.clone();
        let pipeline_config = config.clone();
        let subtitle = subtitle_window.clone();

        let pipeline_handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async move {
                let mut sentence_buf = SentenceBuffer::new();

                let (async_chunk_tx, mut async_chunk_rx) =
                    tokio::sync::mpsc::channel::<Vec<u8>>(64);
                let stop_b = pipeline_stop.clone();
                let bridge = tokio::task::spawn_blocking(move || {
                    loop {
                        if stop_b.load(Ordering::Relaxed) { break; }
                        match chunk_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                            Ok(data) => {
                                if async_chunk_tx.blocking_send(data).is_err() { break; }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        }
                    }
                });

                let (result_tx, mut result_rx) =
                    tokio::sync::mpsc::channel::<(usize, String)>(32);
                let mut seq_counter: usize = 0;
                let mut next_expected: usize = 0;
                let mut pending_results: std::collections::BTreeMap<usize, String> =
                    std::collections::BTreeMap::new();
                let semaphore = Arc::new(tokio::sync::Semaphore::new(2));

                let mut idle_check = tokio::time::interval(std::time::Duration::from_millis(800));
                idle_check.tick().await;

                async fn finalize_and_show(
                    sentence: String,
                    config: &ConfigManager,
                    subtitle: &SubtitleWindow,
                ) {
                    subtitle.clear_interim();
                    if let Some(result) = pipeline::finalize_sentence(sentence, config).await {
                        if !result.original.is_empty() {
                            subtitle.show_bilingual(result.original, result.translated);
                        }
                    }
                }

                loop {
                    if pipeline_stop.load(Ordering::Relaxed) { break; }

                    tokio::select! {
                        Some(wav_data) = async_chunk_rx.recv() => {
                            let mut chunks = vec![wav_data];
                            while let Ok(extra) = async_chunk_rx.try_recv() {
                                chunks.push(extra);
                            }
                            let merged = merge_wav_chunks(chunks);

                            let seq = seq_counter;
                            seq_counter += 1;
                            let tx = result_tx.clone();
                            let cfg = pipeline_config.clone();
                            let sem = semaphore.clone();
                            tokio::spawn(async move {
                                let _permit = sem.acquire().await.unwrap();
                                if let Some(fragment) =
                                    pipeline::transcribe_chunk(merged, &cfg).await
                                {
                                    let _ = tx.send((seq, fragment)).await;
                                }
                            });
                        }

                        Some((seq, fragment)) = result_rx.recv() => {
                            pending_results.insert(seq, fragment);
                            while let Some(text) = pending_results.remove(&next_expected) {
                                next_expected += 1;
                                let sentences = sentence_buf.push_fragment(&text);
                                if !sentences.is_empty() {
                                    subtitle.clear_interim();
                                    for sentence in sentences {
                                        finalize_and_show(
                                            sentence, &pipeline_config, &subtitle,
                                        ).await;
                                    }
                                } else {
                                    subtitle.show_interim(
                                        sentence_buf.pending_text().to_string(),
                                    );
                                }
                            }
                        }

                        _ = idle_check.tick() => {
                            if let Some(tail) = sentence_buf.flush_if_idle() {
                                finalize_and_show(tail, &pipeline_config, &subtitle).await;
                            }
                        }
                    }
                }

                let _ = bridge.await;

                if let Some(tail) = sentence_buf.force_flush() {
                    finalize_and_show(tail, &pipeline_config, &subtitle).await;
                }
            });
        });

        write_log_line("[字幕] 翻译管道线程已启动", None);

        Ok(Self {
            stop_flag,
            subtitle_window,
            capture_handle: Some(capture_handle),
            chunker_handle: Some(chunker_handle),
            pipeline_handle: Some(pipeline_handle),
        })
    }

    pub fn update_subtitle_config(&self, config: &ConfigManager) {
        self.subtitle_window.set_original_font_size(config.interpreter_original_font_size());
        self.subtitle_window.set_original_color(config.interpreter_original_color());
        self.subtitle_window.set_translated_font_size(config.interpreter_translated_font_size());
        self.subtitle_window.set_translated_color(config.interpreter_translated_color());
        self.subtitle_window.set_click_through(config.interpreter_subtitle_click_through());
        self.subtitle_window.set_opacity(config.interpreter_subtitle_opacity());
        self.subtitle_window.set_font_size(config.interpreter_subtitle_font_size());
        self.subtitle_window.set_position(config.interpreter_subtitle_position());
        subtitle_window::SUBTITLE_ALWAYS_VISIBLE.store(config.interpreter_always_visible(), Ordering::SeqCst);
    }

    pub fn stop(&mut self) {
        write_log_line("[字幕] 正在停止实时字幕引擎...", None);
        self.stop_flag.store(true, Ordering::Relaxed);
        self.subtitle_window.hide();
        self.subtitle_window.shutdown();
        pipeline::reset_last_transcription();

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

fn find_data_offset(wav: &[u8]) -> Option<usize> {
    let mut i = 12usize;
    while i + 8 <= wav.len() {
        let id = &wav[i..i + 4];
        let size = u32::from_le_bytes(
            [wav[i + 4], wav[i + 5], wav[i + 6], wav[i + 7]],
        ) as usize;
        if id == b"data" {
            return Some(i + 8);
        }
        i += 8 + size;
        if i % 2 != 0 { i += 1; }
    }
    None
}

fn merge_wav_chunks(chunks: Vec<Vec<u8>>) -> Vec<u8> {
    if chunks.is_empty() { return Vec::new(); }
    if chunks.len() == 1 { return chunks.into_iter().next().unwrap(); }

    let first = &chunks[0];
    let data_off = match find_data_offset(first) {
        Some(off) if off < first.len() => off,
        _ => return chunks.into_iter().next().unwrap(),
    };

    let mut raw = Vec::new();
    for chunk in &chunks {
        if let Some(off) = find_data_offset(chunk) {
            if off < chunk.len() {
                raw.extend_from_slice(&chunk[off..]);
            }
        }
    }

    let mut result = first[..data_off].to_vec();
    result.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    result.extend(raw);
    let riff_size = (result.len() - 8) as u32;
    result[4..8].copy_from_slice(&riff_size.to_le_bytes());

    write_log(
        LogLevel::DEBUG,
        &format!("[字幕] 合并 {} 个音频块 → {} 字节", chunks.len(), result.len()),
        None,
    );

    result
}
