#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod audio;
mod config;
mod core;
mod gui;
mod indicator;
mod output;
mod update;
mod utils;
mod win_utils;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use chrono::Local;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dotenv::dotenv;
use tokio::sync::mpsc;

use api::client::ApiClient;
use audio::processor::{encode_wav_memory, resample_and_convert};
use config::ConfigManager;
use core::state::AppState;

use gui::Voice2TypeApp;
use indicator::{IndicatorState, StatusIndicator};
use native_windows_gui as nwg;
use output::handler::{post_process, OutputHandler};
use utils::hotkey::InputMessage;
use utils::logger::{
    init_log_pipe, log_set_enabled, start_log_viewer, viewer_main, write_log, write_log_line,
    LogLevel,
};

#[cfg(target_os = "windows")]
use once_cell::sync::{Lazy, OnceCell};

#[cfg(target_os = "windows")]
static INDICATOR: OnceCell<StatusIndicator> = OnceCell::new();

#[cfg(target_os = "windows")]
use std::sync::Mutex as StdMutex;

// 全局录音标志
static IS_RECORDING: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
static CONFIG_GLOBAL: OnceCell<Arc<ConfigManager>> = OnceCell::new();

#[cfg(target_os = "windows")]
static LOG_MENU_NEEDS_UNCHECK: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
pub fn request_uncheck_log_menu() {
    LOG_MENU_NEEDS_UNCHECK.store(true, Ordering::SeqCst);
}

#[cfg(target_os = "windows")]
pub fn should_uncheck_log_menu_and_reset() -> bool {
    if LOG_MENU_NEEDS_UNCHECK.load(Ordering::SeqCst) {
        LOG_MENU_NEEDS_UNCHECK.store(false, Ordering::SeqCst);
        return true;
    }
    false
}

#[cfg(target_os = "windows")]
static APP_MUTEX: Lazy<StdMutex<Option<windows::Win32::Foundation::HANDLE>>> =
    Lazy::new(|| StdMutex::new(None));

#[cfg(target_os = "windows")]
pub fn release_app_mutex() {
    use windows::Win32::Foundation::CloseHandle;
    if let Ok(mut guard) = APP_MUTEX.lock() {
        if let Some(handle) = guard.take() {
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }
}

fn main() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--log-viewer") {
            viewer_main();
            return Ok(());
        }

        // 如果是更新重启，先等待旧进程退出
        if args.iter().any(|a| a == "--restart") {
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }

        // 清理旧版本备份文件 (*.old)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(dir) = exe_path.parent() {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(ext) = path.extension() {
                            if ext == "old" {
                                let _ = std::fs::remove_file(path);
                            }
                        }
                    }
                }
            }
        }
    }
    // 0. 单例检查 (Single Instance Check)
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONEXCLAMATION, MB_OK};

        // 创建全局互斥体
        let mutex_name = "Global\\Voice2TypeAppMutex\0"
            .encode_utf16()
            .collect::<Vec<u16>>();

        let args: Vec<String> = std::env::args().collect();
        let is_restart = args.iter().any(|a| a == "--restart");
        let max_retries = if is_restart { 20 } else { 1 }; // 重启模式下尝试 20 次 (10秒)

        let mut handle = Ok(windows::Win32::Foundation::HANDLE(0));
        let mut acquired = false;

        for _ in 0..max_retries {
            handle = CreateMutexW(None, true, PCWSTR(mutex_name.as_ptr()));

            if let Ok(h) = handle {
                if GetLastError() != ERROR_ALREADY_EXISTS {
                    // 成功获取锁（是第一个实例）
                    acquired = true;
                    break;
                }
                // 虽然 CreateMutexW 成功返回句柄，但 ERROR_ALREADY_EXISTS 说明已经有实例存在
                // 关闭句柄，准备重试
                let _ = CloseHandle(h);
            }

            if is_restart {
                std::thread::sleep(std::time::Duration::from_millis(500));
            } else {
                break;
            }
        }

        if !acquired {
            let title = "错误\0".encode_utf16().collect::<Vec<u16>>();
            let msg = "程序已在运行中。\0".encode_utf16().collect::<Vec<u16>>();
            MessageBoxW(
                None,
                PCWSTR(msg.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONEXCLAMATION,
            );
            std::process::exit(0);
        }

        // 存储句柄以供显式释放
        if let Ok(h) = handle {
            *APP_MUTEX.lock().unwrap() = Some(h);
        }
    }

    // 1. 初始化环境与日志
    dotenv().ok();

    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::System::Console::GetConsoleWindow;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        // 仅在主线程处理窗口隐藏，日志启动移至异步线程以加快启动速度
        let temp_config = ConfigManager::new();
        if !temp_config.show_log() {
            let hwnd = GetConsoleWindow();
            if hwnd.0 != 0 {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    env_logger::init();

    // 2. 初始化 NWG
    nwg::init().expect("Failed to init Native Windows GUI");
    nwg::Font::set_global_family("Segoe UI").expect("Failed to set default font");

    // 3. 初始化配置
    let config_manager = Arc::new(ConfigManager::new());
    #[cfg(target_os = "windows")]
    {
        let _ = CONFIG_GLOBAL.set(config_manager.clone());
        // 初始化日志目录并清空旧日志
        let log_dir = config_manager.log_dir();
        if !log_dir.exists() {
            let _ = std::fs::create_dir_all(&log_dir);
        }
        let log_file = config_manager.log_file_path();
        let _ = std::fs::File::create(log_file); // 创建或清空文件
    }

    // 4. 启动API服务器
    #[cfg(feature = "api_server")]
    {
        use api::server::ApiServer;
        let api_server = ApiServer::new(config_manager.clone());
        api_server.start();
        write_log(LogLevel::INFO, "API服务器已启动", Some(&config_manager));
    }

    // 5. 启动逻辑线程 (Tokio Runtime)
    let cm_clone = config_manager.clone();
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        if let Err(e) = rt.block_on(async_main(cm_clone)) {
            write_log(LogLevel::ERROR, &format!("运行时错误: {}", e), None);
        }
    });

    // 5. 初始化 GUI
    let _ui = Voice2TypeApp::init(config_manager.clone());

    // 初始化状态指示器
    #[cfg(target_os = "windows")]
    if config_manager.enable_indicator() {
        let _ = INDICATOR.set(StatusIndicator::new());
    }

    // 6. 运行 GUI 事件循环 (主线程阻塞在此)
    nwg::dispatch_thread_events();

    // 7. 退出清理
    #[cfg(target_os = "windows")]
    {
        // 强制关闭日志子进程
        if let Some(cfg) = CONFIG_GLOBAL.get() {
            log_set_enabled(false, Some(cfg));
            let log_dir = cfg.log_dir();
            if log_dir.exists() {
                let _ = std::fs::remove_dir_all(log_dir);
            }
        } else {
            log_set_enabled(false, None);
        }
    }

    Ok(())
}

async fn async_main(config: Arc<ConfigManager>) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        if config.show_log() {
            init_log_pipe();
            start_log_viewer();
            // 给一点时间让管道连接，避免第一条日志丢失
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    write_log_line("Voice2Type已启动...", Some(&config));

    #[cfg(target_os = "windows")]
    if win_utils::is_admin() {
        write_log_line(
            "[系统] 程序当前以管理员权限运行 (兼容性最强)",
            Some(&config),
        );
    } else {
        write_log_line(
            "[警告]⚠️⚠️⚠️ 未检测到管理员权限！这将导致无法在管理员权限的游戏或程序中使用⚠️⚠️⚠️",
            Some(&config),
        );
        write_log_line(
            "[提示] 程序已配置为请求管理员权限，请确保在弹出的 UAC 窗口中选择“是”。",
            Some(&config),
        );
    }

    // 打印欢迎信息
    write_log_line(
        "--------------------------------------------------",
        Some(&config),
    );
    write_log_line(
        " 选中目标输入框，按住热键说话，松开后文字将自动输入",
        Some(&config),
    );
    write_log_line(" 若想取消录音，在按住热键时按下 ESC 即可", Some(&config));
    write_log_line(
        &format!(" 当前版本: {}", env!("CARGO_PKG_VERSION")),
        Some(&config),
    );
    write_log_line(" 若有任何问题，请联系作者", Some(&config));
    write_log_line(
        "--------------------------------------------------",
        Some(&config),
    );

    // 音频系统初始化
    let host = cpal::default_host();
    let device = host.default_input_device().context("未找到音频输入设备")?;
    let dev_config = device
        .default_input_config()
        .context("无法获取默认输入配置")?;
    let stream_config: cpal::StreamConfig = dev_config.clone().into();
    let sample_rate = stream_config.sample_rate.0;
    let channels = stream_config.channels;

    write_log_line(
        &format!("[音频] 采样率: {}Hz, 通道数: {}", sample_rate, channels),
        Some(&config),
    );

    let audio_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let audio_buffer_writer = audio_buffer.clone();

    // 创建输入流
    let config_clone = config.clone();
    let input_stream = device.build_input_stream(
        &stream_config,
        move |data: &[f32], _: &_| {
            if IS_RECORDING.load(Ordering::Relaxed) {
                if let Ok(mut buffer) = audio_buffer_writer.lock() {
                    let mut processed_data = Vec::new();

                    if channels > 1 {
                        // 混音到单声道
                        for frame in data.chunks(channels as usize) {
                            let sum: f32 = frame.iter().sum();
                            processed_data.push(sum / channels as f32);
                        }
                    } else {
                        processed_data.extend_from_slice(data);
                    }

                    // 将处理后的数据添加到缓冲区
                    buffer.extend_from_slice(&processed_data);
                }
            }
        },
        move |err| {
            write_log(
                LogLevel::ERROR,
                &format!("音频输入流错误: {}", err),
                Some(&config_clone),
            );
        },
        None,
    )?;

    input_stream.play()?;

    // 通信通道
    let (tx, mut rx) = mpsc::channel(100);

    // 启动热键监听线程
    let tx_clone = tx.clone();
    let config_for_hotkey = config.clone();
    utils::hotkey::start_hotkey_listener(tx_clone, config_for_hotkey);

    // 异步事件循环
    let mut current_state = AppState::Idle;

    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                match msg {
                    InputMessage::StartRecording => {
                        if current_state == AppState::Idle || current_state == AppState::Processing || current_state == AppState::Cancelled {
                            current_state = AppState::Recording;
                            write_log_line("--> [录音] 开始录音... (按 ESC 取消)", Some(&config));
                            audio_buffer.lock().unwrap().clear();
                            IS_RECORDING.store(true, Ordering::Relaxed);

                            #[cfg(target_os = "windows")]
                            if config.enable_indicator() {
                                if let Some(ind) = INDICATOR.get() {
                                    ind.set_state(IndicatorState::Recording);
                                }
                            }
                        }
                    }
                    InputMessage::CancelRecording => {
                        if current_state == AppState::Recording {
                            current_state = AppState::Cancelled;
                            IS_RECORDING.store(false, Ordering::Relaxed);
                            write_log_line("--> [取消] 录音已取消", Some(&config));
                            audio_buffer.lock().unwrap().clear();
                            #[cfg(target_os = "windows")]
                            if config.enable_indicator() {
                                if let Some(ind) = INDICATOR.get() {
                                    ind.set_state(IndicatorState::Cancelled);
                                    let ind = ind.clone();
                                    tokio::spawn(async move {
                                        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                                        if !IS_RECORDING.load(Ordering::Relaxed) {
                                             ind.set_state(IndicatorState::Hidden);
                                        }
                                    });
                                }
                            }
                        }
                    }
                    InputMessage::StopRecording => {
                        if current_state == AppState::Recording {
                            // 正常结束录音
                            IS_RECORDING.store(false, Ordering::Relaxed);
                            write_log_line("--> [处理] 正在转换音频...", Some(&config));
                            #[cfg(target_os = "windows")]
                            if config.enable_indicator() {
                                if let Some(ind) = INDICATOR.get() {
                                    ind.set_state(IndicatorState::Processing);
                                }
                            }

                            // 获取缓冲区中的音频数据
                            let audio_data = {
                                let mut lock = audio_buffer.lock().unwrap();
                                std::mem::take(&mut *lock)
                            };

                            let config_clone = config.clone();
                            let config_clone_for_log = config_clone.clone();

                            tokio::spawn(async move {
                                if let Err(e) = process_audio_and_type(audio_data, sample_rate, config_clone).await {
                                    write_log(LogLevel::ERROR, &format!("转写失败: {}", e), Some(&config_clone_for_log));
                                }
                            });

                            current_state = AppState::Idle;
                        } else if current_state == AppState::Cancelled {
                            // 如果之前被取消了，F2 松开时重置为 Idle
                            current_state = AppState::Idle;
                        }
                    }
                }
            }
        }
    }
}

async fn process_audio_and_type(
    audio_data: Vec<f32>,
    sample_rate: u32,
    config: Arc<ConfigManager>,
) -> Result<()> {
    if audio_data.is_empty() {
        write_log(
            LogLevel::WARN,
            "音频数据为空，已跳过本次转写",
            Some(&config),
        );
        return Ok(());
    }

    if config.get_api_key().is_empty() {
        write_log(
            LogLevel::ERROR,
            "API Key 未配置，请先在托盘菜单里填写",
            Some(&config),
        );
        #[cfg(target_os = "windows")]
        if config.enable_indicator() {
            if let Some(ind) = INDICATOR.get() {
                ind.set_state(IndicatorState::Error);
            }
        }
        return Ok(());
    }

    let start_time = std::time::Instant::now();

    // 录音线程只负责采集，这里集中做转码、识别和输出，方便排查耗时。
    let (processed_samples, new_rate) = resample_and_convert(&audio_data, sample_rate);
    let resample_duration = start_time.elapsed();

    let wav_data = encode_wav_memory(&processed_samples, new_rate)?;
    let encode_duration = start_time.elapsed().saturating_sub(resample_duration);

    write_log_line(
        &format!(
            "[性能] 原始: {}k, WAV: {}k, 采样率: {}Hz -> {}Hz, 重采样: {:?}, 编码: {:?}",
            audio_data.len() * 4 / 1024,
            wav_data.len() / 1024,
            sample_rate,
            new_rate,
            resample_duration,
            encode_duration
        ),
        Some(&config),
    );

    let api_client = ApiClient::new();
    let output = OutputHandler::new();
    let is_whisper_model = config.get_model_name().starts_with("whisper");
    let use_streaming = config.enable_streaming() && is_whisper_model;
    let upload_start = std::time::Instant::now();

    if use_streaming {
        write_log_line("[识别] 使用分段转写模式", Some(&config));
        let transcriptions = api_client
            .process_audio_streaming(wav_data, &config)
            .await?;
        write_log_line(
            &format!("[性能] API 请求耗时: {:?}", upload_start.elapsed()),
            Some(&config),
        );

        let final_text = post_process(&transcriptions.join(" "), &config);
        finish_transcription(final_text, &config, &output).await?;
    } else {
        let raw_text = api_client.process_audio(wav_data, &config).await?;
        write_log_line(
            &format!("[性能] API 请求耗时: {:?}", upload_start.elapsed()),
            Some(&config),
        );
        let final_text = post_process(&raw_text, &config);
        finish_transcription(final_text, &config, &output).await?;
    }

    Ok(())
}

async fn finish_transcription(
    final_text: String,
    config: &Arc<ConfigManager>,
    output: &OutputHandler,
) -> Result<()> {
    if final_text.is_empty() {
        write_log(LogLevel::WARN, "识别结果为空，未输出文本", Some(config));
        set_indicator_state(IndicatorState::Error, config);
        return Ok(());
    }

    write_log(
        LogLevel::INFO,
        &format!("[输出] {}", final_text),
        Some(config),
    );
    output.handle_output(final_text, config).await?;
    set_indicator_state(IndicatorState::Success, config);
    Ok(())
}

fn set_indicator_state(state: IndicatorState, config: &ConfigManager) {
    #[cfg(target_os = "windows")]
    if config.enable_indicator() {
        if let Some(ind) = INDICATOR.get() {
            ind.set_state(state);
        }
    }
}
