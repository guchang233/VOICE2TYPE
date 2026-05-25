use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use tokio::sync::mpsc;
use windows::Win32::Foundation::{HRESULT, S_OK};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

use crate::audio::processor::resample_and_convert;

pub struct LoopbackCapture {
    running: Arc<AtomicBool>,
    _thread: Option<thread::JoinHandle<()>>,
}

impl LoopbackCapture {
    pub fn start(tx: mpsc::Sender<Vec<i16>>) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let thread = thread::spawn(move || {
            if let Err(e) = Self::capture_loop(running_clone, tx) {
                log::error!("Loopback capture error: {}", e);
            }
        });

        Ok(Self {
            running,
            _thread: Some(thread),
        })
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn capture_loop(running: Arc<AtomicBool>, tx: mpsc::Sender<Vec<i16>>) -> Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let enumerator: IMMDeviceEnumerator = CoCreateInstance(
                &MMDeviceEnumerator,
                None,
                CLSCTX_ALL,
            )?;

            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;

            let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

            let mut format = WAVEFORMATEX::default();
            let _ = audio_client.GetMixFormat(&mut format);

            let format_ptr = &format as *const WAVEFORMATEX;
            let mut latency = 0u64;
            audio_client.GetDevicePeriod(&mut latency, None)?;

            let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                100_000_000,
                0,
                format_ptr,
                None,
            )?;

            let capture_client: IAudioCaptureClient = audio_client.GetService()?;

            let frame_size = format.nBlockAlign as usize;
            let sample_rate = format.nSamplesPerSec;
            let channels = format.nChannels as usize;

            let buffer_frame_count = audio_client.GetBufferSize()?;

            audio_client.Start()?;

            while running.load(Ordering::SeqCst) {
                let mut buffer = Vec::with_capacity(buffer_frame_count as usize * frame_size);
                let mut num_frames_available = 0u32;
                let mut flags = 0u32;
                let mut device_position = 0u64;
                let mut qpc_position = 0u64;

                let hr = capture_client.GetBuffer(
                    Some(buffer.as_mut_ptr() as *mut _),
                    &mut num_frames_available,
                    &mut flags,
                    &mut device_position,
                    &mut qpc_position,
                );

                if hr != S_OK {
                    if hr == HRESULT(0x8000000A) {
                        thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    anyhow::bail!("GetBuffer failed: {:?}", hr);
                }

                if num_frames_available > 0 {
                    buffer.set_len(num_frames_available as usize * frame_size);

                    let float_samples = Self::convert_to_f32(&buffer, channels, format.wBitsPerSample);
                    let (converted, _) = resample_and_convert(&float_samples, sample_rate);

                    if let Err(e) = tx.try_send(converted) {
                        log::warn!("Failed to send audio: {}", e);
                    }
                }

                let _ = capture_client.ReleaseBuffer(num_frames_available);
            }

            let _ = audio_client.Stop();
            CoUninitialize();
        }

        Ok(())
    }

    fn convert_to_f32(buffer: &[u8], channels: usize, bits_per_sample: u16) -> Vec<f32> {
        let samples_per_channel = buffer.len() / (channels * (bits_per_sample as usize / 8));
        let mut result = Vec::with_capacity(samples_per_channel);

        match bits_per_sample {
            16 => {
                let samples: Vec<i16> = buffer
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect();

                for i in 0..samples_per_channel {
                    let mut sum = 0.0f32;
                    for ch in 0..channels {
                        sum += samples[i * channels + ch] as f32;
                    }
                    result.push(sum / channels as f32 / i16::MAX as f32);
                }
            }
            32 => {
                let samples: Vec<i32> = buffer
                    .chunks_exact(4)
                    .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();

                for i in 0..samples_per_channel {
                    let mut sum = 0.0f32;
                    for ch in 0..channels {
                        sum += samples[i * channels + ch] as f32;
                    }
                    result.push(sum / channels as f32 / i32::MAX as f32);
                }
            }
            _ => {}
        }

        result
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        self.stop();
    }
}