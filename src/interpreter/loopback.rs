use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

#[cfg(target_os = "windows")]
use std::sync::mpsc::Sender;

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::Media::Audio::*,
    Win32::System::Com::*,
};

#[cfg(target_os = "windows")]
use crate::utils::logger::{write_log, LogLevel};

const AUDCLNT_STREAMFLAGS_LOOPBACK: u32 = 0x00010000;

pub struct LoopbackCapture {
    pub sample_rate: u32,
    pub stop_flag: Arc<AtomicBool>,
    pub audio_tx: Sender<f32>,
}

#[cfg(target_os = "windows")]
pub fn start_loopback_capture(
    stop_flag: Arc<AtomicBool>,
    audio_tx: Sender<f32>,
    sample_rate_tx: Sender<u32>,
    error_tx: Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if let Err(e) = capture_loop(stop_flag, audio_tx, sample_rate_tx) {
            let _ = error_tx.send(format!("{}", e));
            write_log(LogLevel::ERROR, &format!("[字幕] 音频捕获失败: {}", e), None);
        }
    })
}

#[cfg(target_os = "windows")]
fn capture_loop(
    stop_flag: Arc<AtomicBool>,
    audio_tx: Sender<f32>,
    sample_rate_tx: Sender<u32>,
) -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;

        let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

        let format_ptr = audio_client.GetMixFormat()?;
        let format = &*format_ptr;

        let sample_rate = (*format).nSamplesPerSec;
        let channels = (*format).nChannels as u32;
        let bits_per_sample = (*format).wBitsPerSample;
        let _block_align = (*format).nBlockAlign;

        let _ = sample_rate_tx.send(sample_rate);
        write_log(LogLevel::INFO, &format!("[字幕] WASAPI 初始化成功: 采样率={}, 声道={}, 位深={}", sample_rate, channels, bits_per_sample), None);

        let buffer_duration = 10000000;
        audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            buffer_duration,
            0,
            format_ptr,
            None,
        )?;

        let _render_client: IAudioRenderClient = audio_client.GetService()?;
        let capture_client: IAudioCaptureClient = audio_client.GetService()?;

        audio_client.Start()?;
        write_log(LogLevel::INFO, "[字幕] 音频捕获已启动", None);

        while !stop_flag.load(Ordering::Relaxed) {
            let packet_size = match capture_client.GetNextPacketSize() {
                Ok(size) => size,
                Err(_) => break,
            };

            if packet_size == 0 {
                thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }

            let mut data_ptr: *mut u8 = std::ptr::null_mut();
            let mut num_frames: u32 = 0;
            let mut flags: u32 = 0;

            let hr = capture_client.GetBuffer(
                &mut data_ptr,
                &mut num_frames,
                &mut flags,
                None,
                None,
            );

            if hr.is_err() {
                continue;
            }

            let num_samples = num_frames as usize * channels as usize;

            if bits_per_sample == 32 {
                let samples = std::slice::from_raw_parts(data_ptr as *const f32, num_samples);
                for frame in samples.chunks(channels as usize) {
                    let mono: f32 = frame.iter().sum::<f32>() / channels as f32;
                    if audio_tx.send(mono).is_err() {
                        break;
                    }
                }
            } else if bits_per_sample == 16 {
                let samples = std::slice::from_raw_parts(data_ptr as *const i16, num_samples);
                for frame in samples.chunks(channels as usize) {
                    let mono: f32 = frame.iter()
                        .map(|&s| s as f32 / 32768.0)
                        .sum::<f32>() / channels as f32;
                    if audio_tx.send(mono).is_err() {
                        break;
                    }
                }
            }

            let _ = capture_client.ReleaseBuffer(num_frames);
        }

        let _ = audio_client.Stop();
        let _ = audio_client.Reset();
        CoUninitialize();
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn start_loopback_capture(
    _stop_flag: Arc<AtomicBool>,
    _audio_tx: Sender<f32>,
    _sample_rate_tx: Sender<u32>,
    _error_tx: Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(|| {})
}
