use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

use crate::streaming::audio::{build_capture_stream, pick_best_input_config};

pub struct Recorder {
    stream: Option<cpal::Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<u32>>,
    /// 音频回调热路径读取，用 AtomicBool 避免每帧加锁
    is_recording: Arc<AtomicBool>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
            sample_rate: Arc::new(Mutex::new(0)),
            is_recording: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 启动录音（带偏好参数）。偏好与 `start_capture_with_prefs` 保持一致。
    pub fn start_with_prefs(
        &mut self,
        device_name: Option<&str>,
        downmix_pref: &str,
        sample_fmt_pref: &str,
        sample_rate_pref: &str,
        channels_pref: &str,
    ) -> Result<()> {
        if self.is_recording.load(Ordering::Relaxed) {
            anyhow::bail!("Already recording");
        }

        let host = cpal::default_host();
        let device = if let Some(name) = device_name {
            if name.is_empty() {
                host.default_input_device()
            } else {
                host.input_devices()?
                    .find(|d| d.name().map(|n| n == name).unwrap_or(false))
                    .or_else(|| host.default_input_device())
            }
        } else {
            host.default_input_device()
        }.context("No input device available")?;

        let (config, sample_format) =
            pick_best_input_config(&device, sample_fmt_pref, sample_rate_pref, channels_pref)?;

        let sample_rate = config.sample_rate.0;
        let channels = config.channels;
        *self.sample_rate.lock().unwrap_or_else(|e| e.into_inner()) = sample_rate;

        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();

        let is_recording_clone = self.is_recording.clone();
        let dm = downmix_pref.to_string();

        let err_fn = |err| {
            log::error!("[整段录音] 音频流错误: {}", err);
        };

        // 各样本格式分支共用同一宏：回调内单次遍历完成「格式转换 + 下混」，
        // 无中间 Vec 分配、无 Arc 克隆、无热路径加锁（实时音频回调安全）。
        // 整段录音模式不限缓冲上限（max = 0）。
        let stream = match sample_format {
            SampleFormat::F32 => build_capture_stream!(
                device, config, f32, |s: f32| s,
                is_recording_clone.load(Ordering::Relaxed),
                self.buffer, channels, dm, err_fn, 0usize
            )?,
            SampleFormat::I16 => build_capture_stream!(
                device, config, i16, |s: i16| s as f32 / i16::MAX as f32,
                is_recording_clone.load(Ordering::Relaxed),
                self.buffer, channels, dm, err_fn, 0usize
            )?,
            SampleFormat::U16 => build_capture_stream!(
                device, config, u16, |s: u16| (s as f32 / u16::MAX as f32) * 2.0 - 1.0,
                is_recording_clone.load(Ordering::Relaxed),
                self.buffer, channels, dm, err_fn, 0usize
            )?,
            SampleFormat::I32 => build_capture_stream!(
                device, config, i32, |s: i32| s as f32 / i32::MAX as f32,
                is_recording_clone.load(Ordering::Relaxed),
                self.buffer, channels, dm, err_fn, 0usize
            )?,
            SampleFormat::U32 => build_capture_stream!(
                device, config, u32, |s: u32| (s as f32 / u32::MAX as f32) * 2.0 - 1.0,
                is_recording_clone.load(Ordering::Relaxed),
                self.buffer, channels, dm, err_fn, 0usize
            )?,
            SampleFormat::I8 => build_capture_stream!(
                device, config, i8, |s: i8| s as f32 / i8::MAX as f32,
                is_recording_clone.load(Ordering::Relaxed),
                self.buffer, channels, dm, err_fn, 0usize
            )?,
            SampleFormat::U8 => {
                // 兜底：理论上 pick_best_input_config 已排除 U8
                build_capture_stream!(
                    device, config, u8, |s: u8| (s as f32 / u8::MAX as f32) * 2.0 - 1.0,
                    is_recording_clone.load(Ordering::Relaxed),
                    self.buffer, channels, dm, err_fn, 0usize
                )?
            }
            sample_format => anyhow::bail!("Unsupported sample format: {:?}", sample_format),
        };

        stream.play()?;
        self.stream = Some(stream);
        self.is_recording.store(true, Ordering::SeqCst);

        log::info!(
            "[整段录音] 设备: {}, 采样率: {}Hz, {}ch, 格式: {:?}, 下混: {}",
            device.name().unwrap_or_default(),
            sample_rate,
            channels,
            sample_format,
            downmix_pref
        );
        Ok(())
    }

    /// 简单签名（向后兼容）：传 auto 偏好
    pub fn start(&mut self, device_name: Option<&str>) -> Result<()> {
        self.start_with_prefs(device_name, "strongest", "auto", "auto", "auto")
    }

    pub fn stop(&mut self) -> Result<Vec<f32>> {
        if !self.is_recording.load(Ordering::Relaxed) {
            anyhow::bail!("Not recording");
        }

        if let Some(stream) = self.stream.take() {
            drop(stream);
        }

        self.is_recording.store(false, Ordering::SeqCst);

        // mem::replace 换出整段数据，同时给新会话预置 1 秒容量，
        // 避免整体 clone（长录音可达数十 MB）且下次录音从零增长
        let capacity = self.sample_rate() as usize;
        let data = {
            let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut *buffer, Vec::with_capacity(capacity))
        };

        log::info!("[整段录音] 停止，样本数(mono): {}", data.len());
        Ok(data)
    }

    pub fn cancel(&mut self) -> Result<()> {
        if !self.is_recording.load(Ordering::Relaxed) {
            return Ok(());
        }

        if let Some(stream) = self.stream.take() {
            drop(stream);
        }

        self.is_recording.store(false, Ordering::SeqCst);
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();

        log::info!("[整段录音] 取消");
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::Relaxed)
    }

    pub fn sample_rate(&self) -> u32 {
        *self.sample_rate.lock().unwrap_or_else(|e| e.into_inner())
    }
}

unsafe impl Send for Recorder {}
unsafe impl Sync for Recorder {}
