use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

use crate::streaming::audio::{pick_best_input_config, push_samples_mono};

pub struct Recorder {
    stream: Option<cpal::Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<u32>>,
    is_recording: Arc<Mutex<bool>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
            sample_rate: Arc::new(Mutex::new(0)),
            is_recording: Arc::new(Mutex::new(false)),
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
        if *self.is_recording.lock().unwrap() {
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
        *self.sample_rate.lock().unwrap() = sample_rate;

        self.buffer.lock().unwrap().clear();

        let buffer_clone = self.buffer.clone();
        let is_recording_clone = self.is_recording.clone();
        let dm = downmix_pref.to_string();

        let err_fn = |err| {
            log::error!("[整段录音] 音频流错误: {}", err);
        };

        let stream = match sample_format {
            SampleFormat::F32 => {
                let buf = buffer_clone.clone();
                let irc = is_recording_clone.clone();
                let dm = dm.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if *irc.lock().unwrap() {
                            push_samples_mono(buf.clone(), data, channels, &dm);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::I16 => {
                let buf = buffer_clone.clone();
                let irc = is_recording_clone.clone();
                let dm = dm.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if *irc.lock().unwrap() {
                            let f: Vec<f32> =
                                data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                            push_samples_mono(buf.clone(), &f, channels, &dm);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::U16 => {
                let buf = buffer_clone.clone();
                let irc = is_recording_clone.clone();
                let dm = dm.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        if *irc.lock().unwrap() {
                            let f: Vec<f32> = data
                                .iter()
                                .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                                .collect();
                            push_samples_mono(buf.clone(), &f, channels, &dm);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::I32 => {
                let buf = buffer_clone.clone();
                let irc = is_recording_clone.clone();
                let dm = dm.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i32], _: &cpal::InputCallbackInfo| {
                        if *irc.lock().unwrap() {
                            let f: Vec<f32> =
                                data.iter().map(|&s| s as f32 / i32::MAX as f32).collect();
                            push_samples_mono(buf.clone(), &f, channels, &dm);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::U32 => {
                let buf = buffer_clone.clone();
                let irc = is_recording_clone.clone();
                let dm = dm.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[u32], _: &cpal::InputCallbackInfo| {
                        if *irc.lock().unwrap() {
                            let f: Vec<f32> = data
                                .iter()
                                .map(|&s| (s as f32 / u32::MAX as f32) * 2.0 - 1.0)
                                .collect();
                            push_samples_mono(buf.clone(), &f, channels, &dm);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::I8 => {
                let buf = buffer_clone.clone();
                let irc = is_recording_clone.clone();
                let dm = dm.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i8], _: &cpal::InputCallbackInfo| {
                        if *irc.lock().unwrap() {
                            let f: Vec<f32> =
                                data.iter().map(|&s| s as f32 / i8::MAX as f32).collect();
                            push_samples_mono(buf.clone(), &f, channels, &dm);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::U8 => {
                // 兜底：理论上 pick_best_input_config 已排除 U8
                let buf = buffer_clone.clone();
                let irc = is_recording_clone.clone();
                let dm = dm.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[u8], _: &cpal::InputCallbackInfo| {
                        if *irc.lock().unwrap() {
                            let f: Vec<f32> = data
                                .iter()
                                .map(|&s| (s as f32 / u8::MAX as f32) * 2.0 - 1.0)
                                .collect();
                            push_samples_mono(buf.clone(), &f, channels, &dm);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            sample_format => anyhow::bail!("Unsupported sample format: {:?}", sample_format),
        };

        stream.play()?;
        self.stream = Some(stream);
        *self.is_recording.lock().unwrap() = true;

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
        if !*self.is_recording.lock().unwrap() {
            anyhow::bail!("Not recording");
        }

        if let Some(stream) = self.stream.take() {
            drop(stream);
        }

        *self.is_recording.lock().unwrap() = false;

        let mut buffer = self.buffer.lock().unwrap();
        let data = buffer.clone();
        buffer.clear();

        log::info!("[整段录音] 停止，样本数(mono): {}", data.len());
        Ok(data)
    }

    pub fn cancel(&mut self) -> Result<()> {
        if !*self.is_recording.lock().unwrap() {
            return Ok(());
        }

        if let Some(stream) = self.stream.take() {
            drop(stream);
        }

        *self.is_recording.lock().unwrap() = false;
        self.buffer.lock().unwrap().clear();

        log::info!("[整段录音] 取消");
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        *self.is_recording.lock().unwrap()
    }

    pub fn sample_rate(&self) -> u32 {
        *self.sample_rate.lock().unwrap()
    }
}

unsafe impl Send for Recorder {}
unsafe impl Sync for Recorder {}
