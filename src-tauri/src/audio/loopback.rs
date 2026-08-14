//! Windows WASAPI loopback 采集：捕获系统扬声器（渲染端点）正在播放的音频。
//!
//! 使用 WASAPI 共享模式 + LOOPBACK 标志，无需虚拟声卡即可获取系统音频。
//! 所有 COM 操作（初始化、采集、释放）均在同一个工作线程内完成，
//! 避免跨线程传递 COM 对象。返回的 [`LoopbackCapture`] 在 drop 时停止线程。

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::streaming::push_samples_mono;

/// loopback 采集句柄：drop 时停止采集线程。
pub struct LoopbackCapture {
    stop_flag: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// IEEE_FLOAT SubFormat GUID: 00000003-0000-0010-8000-00aa00389b71
#[cfg(windows)]
const SUBTYPE_IEEE_FLOAT: windows::core::GUID = windows::core::GUID::from_u128(
    0x00000003_0000_0010_8000_00aa00389b71,
);

/// 启动 WASAPI loopback 采集。
///
/// 捕获默认渲染设备（扬声器）的输出，下混为 mono f32 后追加到 `buffer`。
/// `downmix`：`average` | `strongest` | `first_channel`
///
/// 返回 `(sample_rate, channels, LoopbackCapture)`。COM 初始化与采集循环
/// 均在工作线程内执行，通过 channel 传回采样率或错误。
#[cfg(windows)]
pub fn start_loopback_capture(
    buffer: Arc<Mutex<Vec<f32>>>,
    downmix: &str,
) -> Result<(u32, u16, LoopbackCapture)> {
    use std::ffi::c_void;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = stop_flag.clone();
    let dm = downmix.to_string();

    let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<(u32, u16)>>();

    let thread = std::thread::spawn(move || {
        // ===== 所有 COM 操作在此线程内完成，不跨线程 =====
        let init_result = unsafe {
            // COM 初始化（MTA）
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let co_initialized = hr.is_ok();

            let init = (|| -> Result<(u32, u16, IAudioClient, IAudioCaptureClient)> {
                // 创建设备枚举器
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                        .context("创建 IMMDeviceEnumerator 失败")?;

                // 获取默认渲染设备（扬声器）
                let device = enumerator
                    .GetDefaultAudioEndpoint(eRender, eMultimedia)
                    .context("获取默认扬声器设备失败")?;

                // 激活 IAudioClient
                let audio_client: IAudioClient = device
                    .Activate(CLSCTX_ALL, None)
                    .context("激活 IAudioClient 失败")?;

                // 获取 mix format（通常 48kHz f32 立体声）
                let format_ptr = audio_client
                    .GetMixFormat()
                    .context("获取音频格式失败")?;

                let wfx = &*format_ptr;
                // 拷贝字段到局部变量，避免 packed struct 引用问题
                let sample_rate = wfx.nSamplesPerSec;
                let channels = wfx.nChannels;
                let bits_per_sample = wfx.wBitsPerSample;
                let format_tag = wfx.wFormatTag;

                // 验证格式为 32-bit float
                let is_float = format_tag == 3 // WAVE_FORMAT_IEEE_FLOAT
                    || (format_tag == 0xFFFE // WAVE_FORMAT_EXTENSIBLE
                        && {
                            let ext_ptr = format_ptr as *const WAVEFORMATEXTENSIBLE;
                            let sub = std::ptr::read_unaligned(std::ptr::addr_of!((*ext_ptr).SubFormat));
                            sub == SUBTYPE_IEEE_FLOAT
                        });

                if !is_float || bits_per_sample != 32 {
                    CoTaskMemFree(Some(format_ptr as *const c_void));
                    anyhow::bail!(
                        "系统音频格式不支持（需要 32-bit float，当前: {}bit, tag={}）",
                        bits_per_sample,
                        format_tag
                    );
                }

                // 初始化 AudioClient（共享模式 + LOOPBACK）
                audio_client
                    .Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK,
                        0,
                        0,
                        format_ptr,
                        None,
                    )
                    .context("Initialize AudioClient 失败")?;

                // 获取 CaptureClient
                let capture_client: IAudioCaptureClient = audio_client
                    .GetService()
                    .context("获取 IAudioCaptureClient 失败")?;

                // 释放 format（Initialize 后不再需要）
                CoTaskMemFree(Some(format_ptr as *const c_void));

                // 启动采集
                audio_client.Start().context("启动采集失败")?;

                crate::utils::logger::write_log_line(&format!(
                    "[音频-loopback] 扬声器: 采样率 {}Hz, 通道 {}, 下混: {}",
                    sample_rate, channels, dm
                ));

                Ok((sample_rate, channels, audio_client, capture_client))
            })();

            match init {
                Ok((sample_rate, channels, audio_client, capture_client)) => {
                    // 通知主线程采样率
                    let _ = init_tx.send(Ok((sample_rate, channels)));
                    (Some((audio_client, capture_client)), co_initialized, channels)
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                    (None, co_initialized, 0u16)
                }
            }
        };

        let (com_objs, co_initialized, ch) = init_result;

        // ===== 采集循环 =====
        if let Some((audio_client, capture_client)) = com_objs {
            unsafe {
                loop {
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }

                    let mut packet_size = capture_client.GetNextPacketSize().unwrap_or(0);

                    while packet_size > 0 {
                        let mut data_ptr: *mut u8 = std::ptr::null_mut();
                        let mut num_frames = 0u32;
                        let mut flags: u32 = 0;

                        if capture_client
                            .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                            .is_err()
                        {
                            break;
                        }

                        // 非 SILENT 且有数据 → 读取 f32 样本并下混
                        // AUDCLNT_BUFFERFLAGS_SILENT = 0x2
                        if (flags & 0x2) == 0 && !data_ptr.is_null() && num_frames > 0 {
                            let len = num_frames as usize * ch as usize;
                            let samples =
                                std::slice::from_raw_parts(data_ptr as *const f32, len);
                            push_samples_mono(buffer.clone(), samples, ch, &dm);
                        }

                        let _ = capture_client.ReleaseBuffer(num_frames);
                        packet_size = capture_client.GetNextPacketSize().unwrap_or(0);
                    }

                    std::thread::sleep(Duration::from_millis(10));
                }

                let _ = audio_client.Stop();
            }
        }

        if co_initialized {
            unsafe {
                CoUninitialize();
            }
        }
    });

    let (sample_rate, channels) = init_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("loopback 线程启动失败"))??;

    Ok((
        sample_rate,
        channels,
        LoopbackCapture {
            stop_flag,
            thread: Some(thread),
        },
    ))
}

#[cfg(not(windows))]
pub fn start_loopback_capture(
    _buffer: Arc<Mutex<Vec<f32>>>,
    _downmix: &str,
) -> Result<(u32, u16, LoopbackCapture)> {
    anyhow::bail!("系统音频捕获仅支持 Windows 平台")
}
