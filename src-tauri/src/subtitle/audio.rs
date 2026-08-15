//! 单路字幕音源：音频采集线程 + 豆包流式 ASR 客户端 + 音频泵。
//!
//! 职责单一：启动后持续把采集到的 PCM 分块送入豆包 WebSocket，
//! 识别结果经 `results` 通道流出；会话层只负责消费结果与更新快照。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::audio::processor::resample_and_convert;
use crate::config::ConfigManager;
use crate::streaming::{start_capture_with_prefs, AsrResponse, StreamingAsrClient};

/// 音频泵节拍：每 200ms 取走采集缓冲并送入 WebSocket
const CHUNK_MS: u64 = 200;
/// PCM 缓冲安全上限：超过约 5 秒样本（按 48kHz 单声道）则丢弃最旧，防止内存暴涨
const MAX_PCM_BUFFER_SAMPLES: usize = 5 * 48_000;

/// 音源类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// 麦克风输入设备
    Microphone,
    /// 系统扬声器回放（WASAPI loopback）
    System,
}

/// 一路运行中的音源。
pub struct AsrSource {
    pub sample_rate: u32,
    /// ASR 结果流（会话层消费）
    pub results: mpsc::Receiver<Result<AsrResponse>>,
    client: StreamingAsrClient,
    ws_task: JoinHandle<()>,
    pump_task: JoinHandle<()>,
    _capture_thread: std::thread::JoinHandle<()>,
    buffer: Arc<StdMutex<Vec<f32>>>,
    seq: Arc<StdMutex<i32>>,
}

/// 启动一路音源：采集 → 豆包流式 ASR。
pub async fn start_source(
    config: &Arc<ConfigManager>,
    kind: SourceKind,
    device_name: &str,
    running: &Arc<AtomicBool>,
) -> Result<AsrSource, String> {
    let buffer: Arc<StdMutex<Vec<f32>>> = Arc::new(StdMutex::new(Vec::new()));
    let seq = Arc::new(StdMutex::new(2i32));

    // ===== 采集线程：只负责把样本写进共享缓冲，等待 running 复位后释放 =====
    let capture_buffer = buffer.clone();
    let running_thread = running.clone();
    let device = device_name.to_string();
    let (dm, sf, sr_pref, ch_pref) = config.effective_audio_prefs();

    let (rate_tx, rate_rx) = std::sync::mpsc::channel::<Result<u32, String>>();

    // 持有采集句柄：变体字段仅凭 Drop 释放采集资源，不读取内容。
    #[allow(dead_code)]
    enum Capture {
        Cpal(cpal::Stream),
        Loopback(crate::audio::loopback::LoopbackCapture),
    }

    let capture_thread = std::thread::spawn(move || {
        let capture_result = if kind == SourceKind::System {
            crate::audio::loopback::start_loopback_capture(capture_buffer, &dm)
                .map(|(sr, _ch, h)| (sr, Capture::Loopback(h)))
        } else {
            start_capture_with_prefs(
                capture_buffer,
                Some(&device),
                Some(&dm),
                Some(&sf),
                Some(&sr_pref),
                Some(&ch_pref),
            )
            .map(|(sr, _ch, s)| (sr, Capture::Cpal(s)))
        };

        match capture_result {
            Ok((sample_rate, handle)) => {
                let _ = rate_tx.send(Ok(sample_rate));
                while running_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                drop(handle);
            }
            Err(e) => {
                let _ = rate_tx.send(Err(format!("启动音频采集失败: {}", e)));
            }
        }
    });

    let sample_rate = rate_rx.recv().map_err(|_| "音频线程启动失败")??;

    // ===== 豆包流式 ASR 客户端 =====
    let (result_tx, result_rx) = mpsc::channel(32);
    let (client, ws_task) = StreamingAsrClient::connect(config.clone(), result_tx)
        .await
        .map_err(|e| format!("连接豆包失败: {}", e))?;

    // ===== 音频泵：按固定节拍把缓冲送入 WebSocket =====
    let pump_running = running.clone();
    let pump_client = client.clone();
    let pump_buffer = buffer.clone();
    let pump_seq = seq.clone();
    let pump_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(CHUNK_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if !pump_running.load(Ordering::Relaxed) {
                break;
            }
            let chunk_f32 = {
                let mut buf = match pump_buffer.lock() {
                    Ok(b) => b,
                    Err(_) => break,
                };
                if buf.is_empty() {
                    continue;
                }
                // 安全上限：消费不及时则丢弃最旧样本，防止内存无限增长
                if buf.len() > MAX_PCM_BUFFER_SAMPLES {
                    let excess = buf.len() - MAX_PCM_BUFFER_SAMPLES;
                    buf.drain(..excess);
                }
                std::mem::take(&mut *buf)
            };
            // 静音保活：豆包流式 ASR 在长时间收不到数据包时会主动结束会话
            // （45000081: waiting next packet timeout）。同传模式的主源（系统扬声器）
            // 与说话停顿期间的麦克风都会出现这种情况——会话一旦被服务端终止，
            // 后续语音就再也无法识别。这里在无真实音频时持续发送静音帧，
            // 让会话在静音/停顿期间保持存活。
            if chunk_f32.is_empty() {
                let silence_len = (sample_rate / 5) as usize; // 200ms 静音
                let silence: Vec<f32> = vec![0.0f32; silence_len];
                let (pcm_i16, _) = resample_and_convert(&silence, sample_rate);
                if pcm_i16.is_empty() {
                    continue;
                }
                let bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
                let seq_no = {
                    let mut s = match pump_seq.lock() {
                        Ok(s) => s,
                        Err(_) => break,
                    };
                    let cur = *s;
                    *s += 1;
                    cur
                };
                let _ = pump_client.send_audio(bytes, seq_no, false).await;
                continue;
            }
            let (pcm_i16, _) = resample_and_convert(&chunk_f32, sample_rate);
            if pcm_i16.is_empty() {
                continue;
            }
            let bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
            let seq_no = {
                let mut s = match pump_seq.lock() {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let cur = *s;
                *s += 1;
                cur
            };
            let _ = pump_client.send_audio(bytes, seq_no, false).await;
        }
    });

    Ok(AsrSource {
        sample_rate,
        results: result_rx,
        client,
        ws_task,
        pump_task,
        _capture_thread: capture_thread,
        buffer,
        seq,
    })
}

impl AsrSource {
    /// 会话结束：发送最后一包（含残余缓冲）、关闭连接、等待 WS 任务退出。
    pub async fn finish(&mut self) {
        self.pump_task.abort();

        let rest: Vec<f32> = match self.buffer.lock() {
            Ok(mut buf) => std::mem::take(&mut *buf),
            Err(_) => Vec::new(),
        };
        let last_seq = *self.seq.lock().unwrap();
        if !rest.is_empty() {
            let (pcm_i16, _) = resample_and_convert(&rest, self.sample_rate);
            let bytes: Vec<u8> = pcm_i16.iter().flat_map(|s| s.to_le_bytes()).collect();
            let _ = self.client.send_audio(bytes, last_seq, true).await;
        } else {
            let _ = self.client.send_audio(Vec::new(), last_seq, true).await;
        }
        self.client.close().await;

        let _ = tokio::time::timeout(Duration::from_secs(2), &mut self.ws_task).await;
    }
}
