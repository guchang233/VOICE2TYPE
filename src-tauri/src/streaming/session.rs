//! 流式识别会话：采集 → WebSocket → 增量输出

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::audio::processor::resample_and_convert;
use crate::config::ConfigManager;
use crate::indicator::IndicatorState;
use crate::streaming::client::StreamingAsrClient;
use crate::streaming::output::StreamingOutput;
use crate::utils::logger::{write_log, write_log_line, LogLevel};

/// 由主音频回调写入；会话运行时置 true
pub static IS_STREAMING: AtomicBool = AtomicBool::new(false);

const CHUNK_MS: u32 = 200;
const TARGET_RATE: u32 = 16_000;

pub struct StreamingSession {
    sample_rate: u32,
    pcm_buffer: Arc<Mutex<Vec<f32>>>,
    audio_seq: Arc<Mutex<i32>>,
    client: Option<Arc<StreamingAsrClient>>,
    ws_task: Option<JoinHandle<()>>,
    pump_task: Option<JoinHandle<()>>,
    consumer_task: Option<JoinHandle<()>>,
    output: Arc<StreamingOutput>,
    start_instant: Option<Instant>,
    partial_count: Arc<AtomicU64>,
    final_count: Arc<AtomicU64>,
    first_packet_ms: Arc<AtomicU64>,
}

impl StreamingSession {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            pcm_buffer: Arc::new(Mutex::new(Vec::new())),
            // 首包 full client request 占用 sequence=1，音频从 2 开始
            audio_seq: Arc::new(Mutex::new(2)),
            client: None,
            ws_task: None,
            pump_task: None,
            consumer_task: None,
            output: Arc::new(StreamingOutput::new()),
            start_instant: None,
            partial_count: Arc::new(AtomicU64::new(0)),
            final_count: Arc::new(AtomicU64::new(0)),
            first_packet_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn pcm_buffer(&self) -> Arc<Mutex<Vec<f32>>> {
        self.pcm_buffer.clone()
    }

    /// 在 `start()` 之前调用：把音频采集得到的真实采样率写入会话，
    /// 供 pump_task 与 finish() 中的重采样使用。
    pub fn set_sample_rate(&mut self, rate: u32) {
        self.sample_rate = rate;
    }

    /// 当前会话是否处于运行中（已建立 WebSocket 连接）。
    pub fn is_running(&self) -> bool {
        self.client.is_some()
    }

    pub async fn start(&mut self, config: Arc<ConfigManager>) -> Result<()> {
        if self.client.is_some() {
            return Ok(());
        }

        if config.get_doubao_api_key().is_empty() {
            anyhow::bail!("豆包 API Key 未配置");
        }

        self.pcm_buffer.lock().unwrap().clear();
        self.output.reset();
        self.output.begin_session();
        *self.audio_seq.lock().unwrap() = 2;
        self.start_instant = Some(Instant::now());
        self.partial_count.store(0, Ordering::SeqCst);
        self.final_count.store(0, Ordering::SeqCst);
        self.first_packet_ms.store(0, Ordering::SeqCst);

        let (result_tx, mut result_rx) = mpsc::channel(32);
        let (client, ws_task) = StreamingAsrClient::connect(config.clone(), result_tx).await?;
        self.client = Some(Arc::new(client));
        self.ws_task = Some(ws_task);

        let output = self.output.clone();
        let cfg = config.clone();
        let partial_count = self.partial_count.clone();
        let final_count = self.final_count.clone();
        let first_packet_ms = self.first_packet_ms.clone();
        let start_time = self.start_instant.unwrap_or_else(Instant::now);
        let consumer = tokio::spawn(async move {
            while let Some(msg) = result_rx.recv().await {
                match msg {
                    Ok(resp) => {
                        if first_packet_ms.load(Ordering::SeqCst) == 0 {
                            first_packet_ms.store(
                                start_time.elapsed().as_millis() as u64,
                                Ordering::SeqCst,
                            );
                        }
                        if resp.is_final {
                            final_count.fetch_add(1, Ordering::SeqCst);
                        } else {
                            partial_count.fetch_add(1, Ordering::SeqCst);
                        }
                        output.apply_full_text(&resp.text, &cfg, resp.is_final);
                    }
                    Err(e) => {
                        write_log(LogLevel::ERROR, &format!("流式识别: {}", e));
                    }
                }
            }
        });
        self.consumer_task = Some(consumer);

        IS_STREAMING.store(true, Ordering::SeqCst);
        write_log_line("--> [流式] 开始实时识别… (ESC 取消)");

        #[cfg(target_os = "windows")]
        if config.streaming_enable_indicator() {
            if let Some(ind) = crate::INDICATOR.get() {
                ind.set_state(IndicatorState::Recording);
            }
        }

        let client_pump = self.client.as_ref().unwrap().clone();
        let pcm_buf = self.pcm_buffer.clone();
        let seq_arc = self.audio_seq.clone();
        let rate = self.sample_rate;

        let pump = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_millis(CHUNK_MS as u64));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;
                if !IS_STREAMING.load(Ordering::Relaxed) {
                    break;
                }

                // 取走当前 buffer 中全部积累的样本（采集回调一直在写入）
                // 之前的计算 `buf.len() * TARGET_RATE / rate` 会只取走 1/3，
                // 导致 buffer 以 3 倍速积压，音频直到松开才被发送。
                let chunk_f32 = {
                    let Ok(mut buf) = pcm_buf.lock() else {
                        break;
                    };
                    if buf.is_empty() {
                        continue;
                    }
                    std::mem::take(&mut *buf)
                };

                if chunk_f32.is_empty() {
                    continue;
                }

                // buffer 中已是单声道样本（start_capture 回调做了 downmix）
                let (pcm_i16, _) = resample_and_convert(&chunk_f32, rate);
                if pcm_i16.is_empty() {
                    continue;
                }
                let bytes: Vec<u8> = pcm_i16
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect();

                let seq = {
                    let Ok(mut s) = seq_arc.lock() else {
                        break;
                    };
                    let cur = *s;
                    *s += 1;
                    cur
                };

                let _ = client_pump.send_audio(bytes, seq, false).await;
            }
        });

        self.pump_task = Some(pump);
        Ok(())
    }

    pub async fn stop(&mut self, config: &Arc<ConfigManager>) {
        self.finish(config, false).await;
    }

    pub async fn cancel(&mut self, config: &Arc<ConfigManager>) {
        self.finish(config, true).await;
    }

    async fn finish(&mut self, config: &Arc<ConfigManager>, cancelled: bool) {
        if self.client.is_none() {
            return;
        }

        IS_STREAMING.store(false, Ordering::SeqCst);

        if let Some(pump) = self.pump_task.take() {
            pump.abort();
        }

        // 发送最后一包
        if let Some(client) = &self.client {
            let rest: Vec<f32> = {
                let mut buf = self.pcm_buffer.lock().unwrap();
                std::mem::take(&mut *buf)
            };
            if !cancelled {
                let last_seq = *self.audio_seq.lock().unwrap();
                if !rest.is_empty() {
                    let (pcm_i16, _) = resample_and_convert(&rest, self.sample_rate);
                    let bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
                    let _ = client.send_audio(bytes, last_seq, true).await;
                } else {
                    let _ = client.send_audio(Vec::new(), last_seq, true).await;
                }
            }
            client.close().await;
        }

        if let Some(task) = self.ws_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
        }

        // 等待消费任务处理完最后的 ASR 帧，最多 500ms 后强制终止
        if let Some(task) = self.consumer_task.take() {
            let abort_handle = task.abort_handle();
            if tokio::time::timeout(Duration::from_millis(500), task).await.is_err() {
                // 超时则 abort，防止松手后仍在逐字输出
                abort_handle.abort();
            }
        }

        self.client = None;
        tokio::time::sleep(Duration::from_millis(30)).await;
        if !cancelled {
            self.output.finalize_ai_polish(config);
        }
        self.output.reset();

        if cancelled {
            write_log_line("--> [流式] 已取消");
            #[cfg(target_os = "windows")]
            if config.streaming_enable_indicator() {
                if let Some(ind) = crate::INDICATOR.get() {
                    ind.set_state(IndicatorState::Cancelled);
                }
            }
        } else {
            write_log_line("--> [流式] 识别结束");
            #[cfg(target_os = "windows")]
            if config.streaming_enable_indicator() {
                if let Some(ind) = crate::INDICATOR.get() {
                    ind.set_state(IndicatorState::Success);
                }
            }
        }
    }
}
