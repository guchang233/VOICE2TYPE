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

const AUDCLNT_STREAMFLAGS_LOOPBACK: u32 = 0x00020000;

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

        let format_ptr = device.Activate::<IAudioClient>(CLSCTX_ALL, None)?.GetMixFormat()?;
        let format = &*format_ptr;

        let format_tag = (*format).wFormatTag;
        let sample_rate = (*format).nSamplesPerSec;
        let channels = (*format).nChannels as u32;
        let bits_per_sample = (*format).wBitsPerSample;

        let effective_format_tag = if format_tag == 0xFFFE {
            let extensible = format as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE;
            let sub_format = (*extensible).SubFormat;
            if sub_format == windows::core::GUID::from_values(0x00000003, 0x0000, 0x0010, [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]) {
                3u16
            } else {
                1u16
            }
        } else {
            format_tag
        };

        let effective_bits = if effective_format_tag == 3 { 32u16 } else { 16u16 };

        write_log(LogLevel::INFO, &format!("[字幕] WASAPI mix format: 采样率={}, 声道={}, 位深={}, wFormatTag={}, effectiveTag={}, effectiveBits={}",
            sample_rate, channels, bits_per_sample, format_tag, effective_format_tag, effective_bits), None);

        let buffer_duration = 10000000i64;

        let audio_client = try_initialize(&device, buffer_duration, std::ptr::null())
            .or_else(|_| {
                write_log(LogLevel::WARN, "[字幕] NULL格式初始化失败，尝试简化格式...", None);
                let simple_format = WAVEFORMATEX {
                    wFormatTag: effective_format_tag,
                    nChannels: channels as u16,
                    nSamplesPerSec: sample_rate,
                    nAvgBytesPerSec: sample_rate * (channels * effective_bits as u32 / 8),
                    nBlockAlign: (channels * effective_bits as u32 / 8) as u16,
                    wBitsPerSample: effective_bits,
                    cbSize: 0,
                };
                try_initialize(&device, buffer_duration, &simple_format as *const WAVEFORMATEX)
            })
            .or_else(|_| {
                write_log(LogLevel::WARN, "[字幕] 简化格式也失败，尝试原始mix format...", None);
                try_initialize(&device, buffer_duration, format_ptr)
            })
            .map_err(|e| {
                CoTaskMemFree(Some(format_ptr as *const _ as *mut _));
                e
            })?;

        let capture_client: IAudioCaptureClient = audio_client.GetService()?;

        let _ = sample_rate_tx.send(sample_rate);

        CoTaskMemFree(Some(format_ptr as *const _ as *mut _));

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

            if effective_bits == 32 {
                let samples = std::slice::from_raw_parts(data_ptr as *const f32, num_samples);
                for frame in samples.chunks(channels as usize) {
                    let mono: f32 = frame.iter().sum::<f32>() / channels as f32;
                    if audio_tx.send(mono).is_err() {
                        break;
                    }
                }
            } else if effective_bits == 16 {
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

#[cfg(target_os = "windows")]
unsafe fn try_initialize(
    device: &IMMDevice,
    buffer_duration: i64,
    format: *const WAVEFORMATEX,
) -> Result<IAudioClient> {
    let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    audio_client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
        buffer_duration,
        0,
        format,
        None,
    )?;
    Ok(audio_client)
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
