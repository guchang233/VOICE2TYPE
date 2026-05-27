use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
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

pub const AUDIO_SOURCE_SPEAKER: u8 = 0;
pub const AUDIO_SOURCE_MICROPHONE: u8 = 1;

pub static AUDIO_SOURCE: AtomicU8 = AtomicU8::new(AUDIO_SOURCE_SPEAKER);
pub static CURRENT_SAMPLE_RATE: AtomicU32 = AtomicU32::new(0);

#[cfg(target_os = "windows")]
pub fn start_loopback_capture(
    stop_flag: Arc<AtomicBool>,
    audio_tx: Sender<f32>,
    sample_rate_tx: Sender<u32>,
    error_tx: Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if let Err(e) = capture_loop_wrapper(stop_flag, audio_tx, sample_rate_tx) {
            let _ = error_tx.send(format!("{}", e));
            write_log(LogLevel::ERROR, &format!("[字幕] 音频捕获失败: {}", e), None);
        }
    })
}

#[cfg(target_os = "windows")]
fn capture_loop_wrapper(
    stop_flag: Arc<AtomicBool>,
    audio_tx: Sender<f32>,
    sample_rate_tx: Sender<u32>,
) -> Result<()> {
    let mut current_source = AUDIO_SOURCE.load(Ordering::SeqCst);

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            return Ok(());
        }

        match capture_session(&stop_flag, &audio_tx, &sample_rate_tx, current_source) {
            Ok(SessionEndReason::Stopped) => return Ok(()),
            Ok(SessionEndReason::SourceChanged(new_source)) => {
                current_source = new_source;
                write_log(LogLevel::INFO, "[字幕] 音频源已切换，重新初始化...", None);
            }
            Ok(SessionEndReason::DeviceChanged) => {
                write_log(LogLevel::INFO, "[字幕] 默认音频设备已变更，重新初始化...", None);
            }
            Err(e) => {
                write_log(LogLevel::ERROR, &format!("[字幕] 捕获会话错误: {}", e), None);
                thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
}

enum SessionEndReason {
    Stopped,
    SourceChanged(u8),
    DeviceChanged,
}

#[cfg(target_os = "windows")]
fn capture_session(
    stop_flag: &Arc<AtomicBool>,
    audio_tx: &Sender<f32>,
    sample_rate_tx: &Sender<u32>,
    source: u8,
) -> Result<SessionEndReason> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        let (data_flow, stream_flags) = if source == AUDIO_SOURCE_MICROPHONE {
            (eCapture, 0u32)
        } else {
            (eRender, AUDCLNT_STREAMFLAGS_LOOPBACK)
        };

        let device = enumerator.GetDefaultAudioEndpoint(data_flow, eConsole)?;

        let initial_device_id = get_device_id(&device)?;

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

        let source_name = if source == AUDIO_SOURCE_MICROPHONE { "麦克风" } else { "扬声器" };
        write_log(LogLevel::INFO, &format!("[字幕] WASAPI {} 模式: 采样率={}, 声道={}, 位深={}, wFormatTag={}, effectiveTag={}, effectiveBits={}",
            source_name, sample_rate, channels, bits_per_sample, format_tag, effective_format_tag, effective_bits), None);

        let buffer_duration = 10000000i64;

        let audio_client = try_initialize(&device, buffer_duration, std::ptr::null(), stream_flags)
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
                try_initialize(&device, buffer_duration, &simple_format as *const WAVEFORMATEX, stream_flags)
            })
            .or_else(|_| {
                write_log(LogLevel::WARN, "[字幕] 简化格式也失败，尝试原始mix format...", None);
                try_initialize(&device, buffer_duration, format_ptr, stream_flags)
            })
            .map_err(|e| {
                CoTaskMemFree(Some(format_ptr as *const _ as *mut _));
                e
            })?;

        let capture_client: IAudioCaptureClient = audio_client.GetService()?;

        let _ = sample_rate_tx.send(sample_rate);
        CURRENT_SAMPLE_RATE.store(sample_rate, Ordering::SeqCst);

        CoTaskMemFree(Some(format_ptr as *const _ as *mut _));

        audio_client.Start()?;
        write_log(LogLevel::INFO, &format!("[字幕] {} 音频捕获已启动", source_name), None);

        let mut last_device_check = std::time::Instant::now();
        let device_check_interval = std::time::Duration::from_secs(2);

        while !stop_flag.load(Ordering::Relaxed) {
            let current_audio_source = AUDIO_SOURCE.load(Ordering::SeqCst);
            if current_audio_source != source {
                let _ = audio_client.Stop();
                let _ = audio_client.Reset();
                CoUninitialize();
                return Ok(SessionEndReason::SourceChanged(current_audio_source));
            }

            if source == AUDIO_SOURCE_SPEAKER && last_device_check.elapsed() >= device_check_interval {
                last_device_check = std::time::Instant::now();
                if let Ok(current_device) = enumerator.GetDefaultAudioEndpoint(data_flow, eConsole) {
                    if let Ok(current_id) = get_device_id(&current_device) {
                        if current_id != initial_device_id {
                            let _ = audio_client.Stop();
                            let _ = audio_client.Reset();
                            CoUninitialize();
                            return Ok(SessionEndReason::DeviceChanged);
                        }
                    }
                }
            }

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

    Ok(SessionEndReason::Stopped)
}

#[cfg(target_os = "windows")]
unsafe fn get_device_id(device: &IMMDevice) -> Result<String> {
    let id_ptr = device.GetId()?;
    let wide = id_ptr.as_wide();
    let id = String::from_utf16_lossy(wide);
    CoTaskMemFree(Some(id_ptr.as_ptr() as *const _ as *mut _));
    Ok(id)
}

#[cfg(target_os = "windows")]
unsafe fn try_initialize(
    device: &IMMDevice,
    buffer_duration: i64,
    format: *const WAVEFORMATEX,
    stream_flags: u32,
) -> Result<IAudioClient> {
    let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    audio_client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        stream_flags,
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
