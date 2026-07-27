use std::sync::{Arc, Mutex};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use anyhow::{Context, Result};

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

    pub fn start(&mut self, device_name: Option<&str>) -> Result<()> {
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

        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate().0;
        *self.sample_rate.lock().unwrap() = sample_rate;

        self.buffer.lock().unwrap().clear();

        let buffer_clone = self.buffer.clone();
        let is_recording_clone = self.is_recording.clone();

        let err_fn = |err| {
            log::error!("Audio stream error: {}", err);
        };

        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if *is_recording_clone.lock().unwrap() {
                            buffer_clone.lock().unwrap().extend_from_slice(data);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::I16 => {
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if *is_recording_clone.lock().unwrap() {
                            let mut buf = buffer_clone.lock().unwrap();
                            for &sample in data {
                                buf.push(sample as f32 / i16::MAX as f32);
                            }
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::U16 => {
                device.build_input_stream(
                    &config.into(),
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        if *is_recording_clone.lock().unwrap() {
                            let mut buf = buffer_clone.lock().unwrap();
                            for &sample in data {
                                let normalized = (sample as f32 / u16::MAX as f32) * 2.0 - 1.0;
                                buf.push(normalized);
                            }
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

        log::info!("Recording started on device: {}, sample_rate: {}", device.name().unwrap_or_default(), sample_rate);
        Ok(())
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

        log::info!("Recording stopped, captured {} samples", data.len());
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

        log::info!("Recording cancelled");
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
