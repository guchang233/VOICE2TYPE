#![windows_subsystem = "windows"]

mod api;
mod audio;
mod config;
mod gui;
mod history;
mod indicator;
mod notify;
mod output;
mod streaming;
mod update;
mod utils;
mod whisper_local;
mod win_utils;

/// 单次录音最长秒数，超出后自动结束并转写。
const MAX_RECORDING_SECS: u32 = 90;

#[derive(Debug, Clone, Copy, PartialEq)]
enum AppState {
    Idle,
    Recording,
    Processing,
    Cancelled,
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dotenv::dotenv;
use tokio::sync::mpsc;

use api::client::ApiClient;
use audio::processor::{encode_wav_memory, resample_and_convert};
use config::ConfigManager;

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
pub static INDICATOR: OnceCell<StatusIndicator> = OnceCell::new();

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

        if args.iter().any(|a| a == "--restart") {
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }

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
    env_logger::init();

    // 2. 初始化 NWG
    nwg::init().expect("Failed to init Native Windows GUI");
    nwg::Font::set_global_family("DengXian").expect("Failed to set default font");

    // 3. 初始化配置
    let config_manager = Arc::new(ConfigManager::new());

    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::System::Console::GetConsoleWindow;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        if !config_manager.show_log() {
            let hwnd = GetConsoleWindow();
            if hwnd.0 != 0 {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }
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

    history::init(config_manager.config_dir());

    // 5. 启动逻辑线程 (Tokio Runtime)
    let cm_clone = config_manager.clone();
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
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
        let _ = INDICATOR.set(StatusIndicator::new(
            config_manager.indicator_fade_duration(),
            config_manager.indicator_error_duration(),
            config_manager.indicator_success_duration(),
        ));
    }

    // 6. 运行 GUI 事件循环 (主线程阻塞在此)
    nwg::dispatch_thread_events();

    // 7. 退出清理
    #[cfg(target_os = "windows")]
    {
        // 强制关闭日志子进程
        if let Some(cfg) = CONFIG_GLOBAL.get() {
            log_set_enabled(false, Some(cfg));
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
    write_log_line(
        " 流式识别：默认按住 F6 边说边出字（菜单「流式语音识别」可改）",
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
    let device = select_input_device(&host, &config)?;
    let device_name = device.name().unwrap_or_else(|_| "未知设备".to_string());
    let dev_config = device
        .default_input_config()
        .context("无法获取默认输入配置")?;
    let stream_config: cpal::StreamConfig = dev_config.clone().into();
    let sample_rate = stream_config.sample_rate.0;
    let channels = stream_config.channels;

    write_log_line(
        &format!(
            "[音频] 设备: {}, 采样率: {}Hz, 通道数: {}",
            device_name, sample_rate, channels
        ),
        Some(&config),
    );

    let audio_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let audio_buffer_writer = audio_buffer.clone();
    let channels_usize = channels.max(1) as usize;
    let sample_rate = sample_rate.max(1);

    let mut streaming_session = streaming::StreamingSession::new(sample_rate);
    let streaming_pcm = streaming_session.pcm_buffer();

    // 创建输入流
    let config_clone = config.clone();
    let input_stream = device.build_input_stream(
        &stream_config,
        move |data: &[f32], _: &_| {
            let streaming = streaming::IS_STREAMING.load(Ordering::Relaxed);
            let recording = IS_RECORDING.load(Ordering::Relaxed);
            if !streaming && !recording {
                return;
            }

            let mono: Vec<f32> = if channels_usize > 1 && channels > 1 {
                let scale = 1.0 / channels as f32;
                data.chunks(channels_usize)
                    .map(|frame| frame.iter().sum::<f32>() * scale)
                    .collect()
            } else {
                data.to_vec()
            };

            if streaming {
                if let Ok(mut buffer) = streaming_pcm.lock() {
                    buffer.extend_from_slice(&mono);
                }
            }

            if !recording {
                return;
            }
            if let Ok(mut buffer) = audio_buffer_writer.lock() {
                buffer.extend_from_slice(&mono);
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

    // 启动热键监听线程（录音文件识别）
    let tx_clone = tx.clone();
    let config_for_hotkey = config.clone();
    utils::hotkey::start_hotkey_listener(tx_clone, config_for_hotkey);

    // 流式识别热键与事件
    let (stream_tx, mut stream_rx) = mpsc::channel(32);
    streaming::start_streaming_hotkey_listener(stream_tx, config.clone());

    // 异步事件循环
    let app_state = Arc::new(Mutex::new(AppState::Idle));
    let mut recording_tick = tokio::time::interval(std::time::Duration::from_secs(1));
    recording_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut processing_task: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        tokio::select! {
            Some(stream_msg) = stream_rx.recv() => {
                use streaming::StreamingInputMessage;
                match stream_msg {
                    StreamingInputMessage::Start => {
                        let file_busy = {
                            let st = app_state.lock().unwrap();
                            *st == AppState::Recording || *st == AppState::Processing
                        };
                        if file_busy {
                            notify::queue_tray_message(
                                "流式识别",
                                "录音文件识别进行中，请结束后再使用流式识别。",
                            );
                            continue;
                        }
                        if let Err(e) = streaming_session.start(config.clone()).await {
                            write_log(LogLevel::ERROR, &format!("流式启动失败: {}", e), Some(&config));
                            notify::queue_tray_message("流式识别", &e.to_string());
                        }
                    }
                    StreamingInputMessage::Stop => {
                        streaming_session.stop(&config).await;
                    }
                    StreamingInputMessage::Cancel => {
                        streaming_session.cancel(&config).await;
                    }
                }
            }
            Some(msg) = rx.recv() => {
                match msg {
                    InputMessage::StartRecording => {
                        if streaming::IS_STREAMING.load(Ordering::Relaxed) {
                            notify::queue_tray_message(
                                "Voice2Type",
                                "流式识别进行中，请结束后再录音。",
                            );
                            continue;
                        }
                        let is_processing = {
                            let st = app_state.lock().unwrap();
                            *st == AppState::Processing
                        };

                        if is_processing {
                            if let Some(task) = processing_task.take() {
                                task.abort();
                                write_log_line("--> [打断] 检测到 F2 按键，已打断正在进行的转写任务并开始新录音", Some(&config));
                            }
                        } else {
                            let can_start = {
                                let st = app_state.lock().unwrap();
                                matches!(*st, AppState::Idle | AppState::Cancelled)
                            };
                            if !can_start {
                                notify::queue_tray_message(
                                    "Voice2Type",
                                    "正在转写上一段录音，请稍候再录。",
                                );
                                continue;
                            }
                        }

                        {
                            let mut st = app_state.lock().unwrap();
                            *st = AppState::Recording;
                        }
                        write_log_line("--> [录音] 开始录音... (按 ESC 取消)", Some(&config));
                        {
                            let mut buf = audio_buffer.lock().unwrap();
                            buf.clear();
                            buf.reserve(sample_rate as usize * 30);
                        }
                        IS_RECORDING.store(true, Ordering::Relaxed);

                        #[cfg(target_os = "windows")]
                        if config.enable_indicator() {
                            if let Some(ind) = INDICATOR.get() {
                                ind.set_state(IndicatorState::Recording);
                            }
                        }
                    }
                    InputMessage::CancelRecording => {
                        let is_recording = *app_state.lock().unwrap() == AppState::Recording;
                        if is_recording {
                            *app_state.lock().unwrap() = AppState::Cancelled;
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
                        let state_now = *app_state.lock().unwrap();
                        if state_now == AppState::Recording {
                            processing_task = Some(begin_audio_processing(
                                &audio_buffer,
                                &app_state,
                                sample_rate,
                                config.clone(),
                                None,
                            ));
                        } else if state_now == AppState::Cancelled {
                            *app_state.lock().unwrap() = AppState::Idle;
                        }
                    }
                }
            }
            _ = recording_tick.tick() => {
                let should_stop = {
                    let st = *app_state.lock().unwrap();
                    if st != AppState::Recording {
                        false
                    } else {
                        let max_samples = sample_rate as usize * MAX_RECORDING_SECS as usize;
                        audio_buffer.lock().unwrap().len() >= max_samples
                    }
                };
                if should_stop {
                    write_log_line(
                        &format!(
                            "--> [录音] 已达最长 {} 秒，自动结束并转写",
                            MAX_RECORDING_SECS
                        ),
                        Some(&config),
                    );
                    notify::queue_tray_message(
                        "Voice2Type",
                        &format!("录音已达 {} 秒上限，正在转写…", MAX_RECORDING_SECS),
                    );
                    processing_task = Some(begin_audio_processing(
                        &audio_buffer,
                        &app_state,
                        sample_rate,
                        config.clone(),
                        Some(format!(
                            "录音已达 {} 秒上限，正在转写…",
                            MAX_RECORDING_SECS
                        )),
                    ));
                }
            }
        }
    }
}

fn begin_audio_processing(
    audio_buffer: &Arc<Mutex<Vec<f32>>>,
    app_state: &Arc<Mutex<AppState>>,
    sample_rate: u32,
    config: Arc<ConfigManager>,
    limit_notice: Option<String>,
) -> tokio::task::JoinHandle<()> {
    IS_RECORDING.store(false, Ordering::Relaxed);
    *app_state.lock().unwrap() = AppState::Processing;
    if let Some(msg) = limit_notice {
        write_log_line(&format!("--> [处理] {}", msg), Some(&config));
    } else {
        write_log_line("--> [处理] 正在转换音频...", Some(&config));
    }
    #[cfg(target_os = "windows")]
    if config.enable_indicator() {
        if let Some(ind) = INDICATOR.get() {
            ind.set_state(IndicatorState::Processing);
        }
    }

    let audio_data = {
        let mut lock = audio_buffer.lock().unwrap();
        std::mem::take(&mut *lock)
    };

    let state_clone = app_state.clone();
    let config_clone = config.clone();
    tokio::spawn(async move {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(90),
            process_audio_and_type(audio_data, sample_rate, config_clone)
        ).await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                write_log(
                    LogLevel::ERROR,
                    &format!("转写失败: {}", e),
                    Some(&config),
                );
                notify::queue_tray_message("转写失败", &e.to_string());
                set_indicator_state(IndicatorState::Error, &config);
            }
            Err(_) => {
                write_log(LogLevel::ERROR, "转写超时 (90秒)", Some(&config));
                notify::queue_tray_message("转写失败", "转写超时");
                set_indicator_state(IndicatorState::Error, &config);
            }
        }

        let mut st = state_clone.lock().unwrap();
        if *st == AppState::Processing {
            *st = AppState::Idle;
        }
    })
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
        notify::queue_tray_message("Voice2Type", "录音太短或未采集到声音");
        set_indicator_state(IndicatorState::Error, &config);
        return Ok(());
    }

    if !config.is_local_whisper() && config.get_api_key().is_empty() {
        let msg = "API Key 未配置，请在托盘菜单「配置 -> API Key」中填写";
        write_log(LogLevel::ERROR, msg, Some(&config));
        notify::queue_tray_message("Voice2Type", msg);
        set_indicator_state(IndicatorState::Error, &config);
        return Ok(());
    }

    if config.is_local_whisper() {
        match whisper_local::LocalWhisper::status(&config) {
            whisper_local::LocalWhisperStatus::Ready => {}
            whisper_local::LocalWhisperStatus::NotConfigured
            | whisper_local::LocalWhisperStatus::InvalidDirectory
            | whisper_local::LocalWhisperStatus::MissingExecutable
            | whisper_local::LocalWhisperStatus::MissingModel => {
                let msg = whisper_local::LocalWhisper::status_message(&config);
                write_log(LogLevel::ERROR, &msg, Some(&config));
                notify::queue_tray_message("本地 Whisper 未就绪", &msg);
                set_indicator_state(IndicatorState::Error, &config);
                return Ok(());
            }
        }
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

    let output = OutputHandler::new();
    let transcribe_start = std::time::Instant::now();

    let raw_text = if config.is_local_whisper() {
        write_log_line("[处理] 调用本地 Whisper...", Some(&config));
        let cfg = config.clone();
        let wav = wav_data;
        let result = tokio::task::spawn_blocking(move || whisper_local::LocalWhisper::transcribe_sync(&wav, &cfg))
            .await
            .context("本地 Whisper 任务异常")??;
        write_log_line(&format!("[处理] 本地 Whisper 完成: {}", result.trim()), Some(&config));
        result
    } else {
        write_log_line(&format!("[处理] 调用 API: {}", config.get_api_url()), Some(&config));
        let api_client = ApiClient::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            api_client.process_audio(wav_data, &config)
        ).await
        .map_err(|_| anyhow::anyhow!("API 请求超时 (60秒)"))??;
        write_log_line(&format!("[处理] API 完成: {}", result.trim()), Some(&config));
        result
    };

    let label = if config.is_local_whisper() {
        "本地 Whisper"
    } else {
        "API"
    };
    write_log_line(
        &format!("[性能] {} 耗时: {:?}", label, transcribe_start.elapsed()),
        Some(&config),
    );
    let final_text = post_process(&raw_text, &config);
    finish_transcription(final_text, &config, &output).await?;

    Ok(())
}

async fn finish_transcription(
    final_text: String,
    config: &Arc<ConfigManager>,
    output: &OutputHandler,
) -> Result<()> {
    if final_text.is_empty() {
        write_log(LogLevel::WARN, "识别结果为空，未输出文本", Some(config));
        notify::queue_tray_message("Voice2Type", "识别结果为空，请靠近麦克风再试");
        set_indicator_state(IndicatorState::Error, config);
        return Ok(());
    }

    write_log(
        LogLevel::INFO,
        &format!("[输出] {}", final_text),
        Some(config),
    );
    history::push(final_text.clone());
    output.handle_output(final_text, config).await?;
    set_indicator_state(IndicatorState::Success, config);
    Ok(())
}

fn select_input_device(
    host: &cpal::Host,
    config: &ConfigManager,
) -> Result<cpal::Device> {
    let wanted = config.input_device();
    if wanted.is_empty() {
        return host
            .default_input_device()
            .context("未找到音频输入设备");
    }

    for device in host.input_devices()? {
        if device.name().ok().as_deref() == Some(wanted.as_str()) {
            return Ok(device);
        }
    }

    write_log(
        LogLevel::WARN,
        &format!("未找到麦克风「{}」，使用系统默认设备", wanted),
        Some(config),
    );
    host.default_input_device()
        .context("未找到音频输入设备")
}

fn set_indicator_state(state: IndicatorState, config: &ConfigManager) {
    #[cfg(target_os = "windows")]
    if config.enable_indicator() {
        if let Some(ind) = INDICATOR.get() {
            ind.set_state(state);
        }
    }
}
