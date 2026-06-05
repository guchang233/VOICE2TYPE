//! 流式识别会话：采集 → WebSocket → 增量输出

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    output: Arc<StreamingOutput>,
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
            output: Arc::new(StreamingOutput::new()),
        }
    }

    pub fn pcm_buffer(&self) -> Arc<Mutex<Vec<f32>>> {
        self.pcm_buffer.clone()
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

        let (result_tx, mut result_rx) = mpsc::channel(32);
        let (client, ws_task) = StreamingAsrClient::connect(config.clone(), result_tx).await?;
        self.client = Some(Arc::new(client));
        self.ws_task = Some(ws_task);

        let output = self.output.clone();
        let cfg = config.clone();
        tokio::spawn(async move {
            while let Some(msg) = result_rx.recv().await {
                match msg {
                    Ok(resp) => {
                        output.apply_full_text(&resp.text, &cfg, resp.is_final);
                    }
                    Err(e) => {
                        write_log(LogLevel::ERROR, &format!("流式识别: {}", e), Some(&cfg));
                    }
                }
            }
        });

        IS_STREAMING.store(true, Ordering::SeqCst);
        write_log_line("--> [流式] 开始实时识别… (ESC 取消)", Some(&config));

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

                let chunk_f32 = {
                    let Ok(mut buf) = pcm_buf.lock() else {
                        break;
                    };
                    if buf.is_empty() {
                        continue;
                    }
                    let take = if rate == 0 {
                        buf.len()
                    } else {
                        ((buf.len() as u64 * TARGET_RATE as u64) / rate as u64).max(1) as usize
                    };
                    let take = take.min(buf.len());
                    buf.drain(..take).collect::<Vec<_>>()
                };

                if chunk_f32.is_empty() {
                    continue;
                }

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
            let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
        }

        self.client = None;
        tokio::time::sleep(Duration::from_millis(80)).await;
        if !cancelled {
            self.output.finalize_ai_polish(config);
            tokio::time::sleep(Duration::from_millis(1200)).await;
        }
        self.output.reset();

        if cancelled {
            write_log_line("--> [流式] 已取消", Some(config));
            #[cfg(target_os = "windows")]
            if config.streaming_enable_indicator() {
                if let Some(ind) = crate::INDICATOR.get() {
                    ind.set_state(IndicatorState::Cancelled);
                }
            }
        } else {
            write_log_line("--> [流式] 识别结束", Some(config));
            #[cfg(target_os = "windows")]
            if config.streaming_enable_indicator() {
                if let Some(ind) = crate::INDICATOR.get() {
                    ind.set_state(IndicatorState::Success);
                }
            }
        }
    }
}
