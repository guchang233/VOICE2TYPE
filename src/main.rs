#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod gui;
mod win_utils;
mod indicator;

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dotenv::dotenv;
#[cfg(not(target_os = "windows"))]
use enigo::{Enigo, Settings};
use tokio::sync::mpsc;
use regex::Regex;
use chrono::Local;

use config::ConfigManager;
use gui::Voice2TypeApp;
use indicator::{StatusIndicator, IndicatorState};
use native_windows_gui as nwg;

#[cfg(target_os = "windows")]
use once_cell::sync::{OnceCell, Lazy};

#[cfg(target_os = "windows")]
static INDICATOR: OnceCell<StatusIndicator> = OnceCell::new();

#[cfg(target_os = "windows")]
use std::sync::Mutex as StdMutex;

// 定义应用状态
#[derive(Debug, Clone, PartialEq)]
enum AppState {
    Idle,
    Recording,
    Processing,
    Cancelled, // 新增：取消状态
}

// 定义主线程与监听线程的通信消息
#[derive(Debug)]
enum InputMessage {
    StartRecording,
    StopRecording,
    CancelRecording, // 新增：取消录音消息
}

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

fn main() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--log-viewer") {
            viewer_main();
            return Ok(());
        }
    }
    // 0. 单例检查 (Single Instance Check)
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::System::Threading::CreateMutexW;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONEXCLAMATION};
        use windows::core::PCWSTR;

        // 创建全局互斥体
        let mutex_name = "Global\\Voice2TypeAppMutex\0".encode_utf16().collect::<Vec<u16>>();
        // CreateMutexW returns Result<HANDLE, Error> in windows 0.54+
        let result = CreateMutexW(None, true, PCWSTR(mutex_name.as_ptr()));

        if let Ok(handle) = result {
             if GetLastError() == ERROR_ALREADY_EXISTS {
                 let title = "Voice2Type\0".encode_utf16().collect::<Vec<u16>>();
                 let msg = "程序已在运行中！\nProgram is already running.\0".encode_utf16().collect::<Vec<u16>>();
                 MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONEXCLAMATION);
                 std::process::exit(0);
             }
             // 保持互斥体句柄直到进程结束
             let _ = handle;
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

    // 4. 启动逻辑线程 (Tokio Runtime)
    let cm_clone = config_manager.clone();
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        
        if let Err(e) = rt.block_on(async_main(cm_clone)) {
            write_log(LogLevel::ERROR, &format!("运行时错误: {}", e));
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
        log_set_enabled(false);

        if let Some(cfg) = CONFIG_GLOBAL.get() {
            let log_dir = cfg.log_dir();
            if log_dir.exists() {
                let _ = std::fs::remove_dir_all(log_dir);
            }
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

    write_log_line("Voice2Type已启动...");
    
    #[cfg(target_os = "windows")]
    if win_utils::is_admin() {
        write_log_line("[系统] 程序当前以管理员权限运行 (兼容性最强)");
    } else {
        write_log_line("[警告]⚠️⚠️⚠️ 未检测到管理员权限！这将导致无法在管理员权限的游戏或程序中使用⚠️⚠️⚠️");
        write_log_line("[提示] 程序已配置为请求管理员权限，请确保在弹出的 UAC 窗口中选择“是”。");
    }

    // 打印欢迎信息
    write_log_line("--------------------------------------------------");
    write_log_line(" 选中目标输入框，按住热键说话，松开后文字将自动输入");
    write_log_line(" 若想取消录音，在按住热键时按下 ESC 即可");
    write_log_line(&format!(" 当前版本: {}", env!("CARGO_PKG_VERSION")));
    write_log_line(" 若有任何问题，请联系作者");
    write_log_line("--------------------------------------------------");

    // 音频系统初始化
    let host = cpal::default_host();
    let device = host.default_input_device().context("未找到音频输入设备")?;
    let dev_config = device.default_input_config().context("无法获取默认输入配置")?;
    let stream_config: cpal::StreamConfig = dev_config.clone().into();
    let sample_rate = stream_config.sample_rate.0;
    let channels = stream_config.channels;

    write_log_line(&format!("[音频] 采样率: {}Hz, 通道数: {}", sample_rate, channels));

    let audio_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let audio_buffer_writer = audio_buffer.clone();

    // 创建输入流
    let input_stream = device.build_input_stream(
        &stream_config,
        move |data: &[f32], _: &_| {
            if IS_RECORDING.load(Ordering::Relaxed) {
                if let Ok(mut buffer) = audio_buffer_writer.lock() {
                    if channels > 1 {
                        // 混音到单声道
                        for frame in data.chunks(channels as usize) {
                            let sum: f32 = frame.iter().sum();
                            buffer.push(sum / channels as f32);
                        }
                    } else {
                        buffer.extend_from_slice(data);
                    }
                }
            }
        },
        move |err| {
            write_log(LogLevel::ERROR, &format!("音频输入流错误: {}", err));
        },
        None,
    )?;

    input_stream.play()?;

    // 通信通道
    let (tx, mut rx) = mpsc::channel(100);

    // 启动热键监听线程
    let tx_clone = tx.clone();
    let config_for_hotkey = config.clone();
    #[cfg(target_os = "windows")]
    thread::spawn(move || {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VIRTUAL_KEY};
        use std::time::Duration;
        
        let vk_esc = VIRTUAL_KEY(0x1B); // ESC
        let mut is_pressed = false;

        loop {
            let current_vk = config_for_hotkey.hotkey();
            let vk_main = VIRTUAL_KEY(current_vk as u16);
            
            // GetAsyncKeyState 的最高位表示按键当前是否按下
            let main_down = unsafe { (GetAsyncKeyState(vk_main.0 as i32) as u16 & 0x8000) != 0 };
            let esc_down = unsafe { (GetAsyncKeyState(vk_esc.0 as i32) as u16 & 0x8000) != 0 };

            if main_down {
                if !is_pressed {
                    is_pressed = true;
                    write_log_line(&format!("检测到主热键(VK:0x{:X}) 按下", current_vk));
                    let _ = tx_clone.blocking_send(InputMessage::StartRecording);
                }
                
                if esc_down {
                    write_log_line("检测到 ESC 按下 (取消录音)");
                    let _ = tx_clone.blocking_send(InputMessage::CancelRecording);
                    // 等待 ESC 松开，防止连续触发
                    while unsafe { (GetAsyncKeyState(vk_esc.0 as i32) as u16 & 0x8000) != 0 } {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            } else {
                if is_pressed {
                    is_pressed = false;
                    write_log_line(&format!("检测到主热键(VK:0x{:X}) 松开", current_vk));
                    let _ = tx_clone.blocking_send(InputMessage::StopRecording);
                }
            }

            thread::sleep(Duration::from_millis(15));
        }
    });

    #[cfg(not(target_os = "windows"))]
    thread::spawn(move || {
        let mut is_f2_pressed = false;
        if let Err(error) = listen(move |event| {
            match event.event_type {
                EventType::KeyPress(Key::F2) => {
                    if !is_f2_pressed {
                        is_f2_pressed = true;
                        write_log_line("检测到 F2 按下");
                        let _ = tx_clone.blocking_send(InputMessage::StartRecording);
                    }
                }
                EventType::KeyRelease(Key::F2) => {
                    if is_f2_pressed {
                        is_f2_pressed = false;
                        write_log_line("检测到 F2 松开");
                        let _ = tx_clone.blocking_send(InputMessage::StopRecording);
                    }
                }
                EventType::KeyPress(Key::Escape) => {
                    if is_f2_pressed {
                        // 如果 F2 正被按住，此时按下了 ESC -> 取消
                        write_log_line("检测到 ESC 按下 (取消录音)");
                        let _ = tx_clone.blocking_send(InputMessage::CancelRecording);
                    }
                }
                _ => {}
            }
        }) {
            write_log_line(&format!("全局按键监听错误: {:?}", error));
        }
    });

    // 异步事件循环
    let mut current_state = AppState::Idle;

    while let Some(msg) = rx.recv().await {
        match msg {
            InputMessage::StartRecording => {
                if current_state == AppState::Idle || current_state == AppState::Processing || current_state == AppState::Cancelled {
                    current_state = AppState::Recording;
                    write_log_line("--> [录音] 开始录音... (按 ESC 取消)"); 
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
                    write_log_line("--> [取消] 录音已取消");
                    audio_buffer.lock().unwrap().clear();
                    
                    #[cfg(target_os = "windows")]
                    if config.enable_indicator() {
                        if let Some(ind) = INDICATOR.get() {
                            ind.set_state(IndicatorState::Hidden);
                        }
                    }
                }
            }
            InputMessage::StopRecording => {
                if current_state == AppState::Recording {
                    // 正常结束录音
                    IS_RECORDING.store(false, Ordering::Relaxed);
                    write_log_line("--> [处理] 正在转换音频...");

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
                    let config_clone = config.clone();
                    
                    tokio::spawn(async move {
                        if let Err(e) = process_audio_and_type(audio_data, sample_rate, config_clone).await {
                            write_log(LogLevel::ERROR, &format!("转写失败: {}", e));
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

    Ok(())
}

async fn process_audio_and_type(audio_data: Vec<f32>, sample_rate: u32, config: Arc<ConfigManager>) -> Result<()> {
    if audio_data.is_empty() {
        write_log(LogLevel::WARN, "音频数据为空");
        return Ok(());
    }

    // 检查 API Key
    let api_key = config.get_api_key();
    if api_key.is_empty() {
        write_log(LogLevel::ERROR, "[错误] API Key 未配置！请在托盘菜单中设置。");
        #[cfg(target_os = "windows")]
        if config.enable_indicator() {
            if let Some(ind) = INDICATOR.get() {
                ind.set_state(IndicatorState::Error);
                let ind = ind.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                    if !IS_RECORDING.load(Ordering::Relaxed) {
                         ind.set_state(IndicatorState::Hidden);
                    }
                });
            }
        }
        return Ok(());
    }

    let start_time = std::time::Instant::now();

    // 1. 降采样和格式转换 (f32 -> i16, Target 16kHz)
    let (processed_samples, new_rate) = resample_and_convert(&audio_data, sample_rate);
    
    let resample_duration = start_time.elapsed();
    
    // 2. 编码 WAV (16-bit)
    let wav_data = encode_wav_memory(&processed_samples, new_rate)?;

    let encode_duration = start_time.elapsed() - resample_duration;

    write_log_line(&format!(
        "[性能] 原始大小: {}k, 处理后大小: {}k, 降采样耗时: {:?}, 编码耗时: {:?}", 
        audio_data.len() * 4 / 1024, 
        wav_data.len() / 1024,
        resample_duration,
        encode_duration
    ));

    let form = reqwest::multipart::Form::new()
        .text("model", config.get_model_name())
        .part("file", reqwest::multipart::Part::bytes(wav_data)
            .file_name("recording.wav")
            .mime_str("audio/wav")?);

    let upload_start = std::time::Instant::now();

    let resp = HTTP_CLIENT.post(&config.get_api_url())
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await?;
    
    let upload_duration = upload_start.elapsed();
    write_log_line(&format!("[性能] API 请求耗时: {:?}", upload_duration));

    if !resp.status().is_success() {
        let error_text = resp.text().await?;
        write_log(LogLevel::ERROR, &format!("API 错误: {}", error_text));
        
        #[cfg(target_os = "windows")]
        if config.enable_indicator() {
            if let Some(ind) = INDICATOR.get() {
                ind.set_state(IndicatorState::Error);
                let ind = ind.clone();
                // 延迟隐藏
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                    if !IS_RECORDING.load(Ordering::Relaxed) {
                         ind.set_state(IndicatorState::Hidden);
                    }
                });
            }
        }
        return Ok(());
    }

    #[derive(serde::Deserialize)]
    struct SiliconFlowResponse {
        text: String,
    }

    // Handle JSON parse error
    let result_json: Result<SiliconFlowResponse, _> = resp.json().await;
    let result = match result_json {
        Ok(r) => r,
        Err(e) => {
            write_log(LogLevel::ERROR, &format!("解析响应失败: {}", e));
             #[cfg(target_os = "windows")]
            if config.enable_indicator() {
                if let Some(ind) = INDICATOR.get() {
                    ind.set_state(IndicatorState::Error);
                    let ind = ind.clone();
                     tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                        if !IS_RECORDING.load(Ordering::Relaxed) {
                             ind.set_state(IndicatorState::Hidden);
                        }
                    });
                }
            }
            return Ok(());
        }
    };

    let raw_text = result.text.trim();

    if raw_text.is_empty() {
        write_log(LogLevel::WARN, "识别结果为空 (未检测到有效语音)");
        #[cfg(target_os = "windows")]
        if config.enable_indicator() {
            if let Some(ind) = INDICATOR.get() {
                ind.set_state(IndicatorState::Error); // 用 Error 状态表示空结果
                let ind = ind.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                    if !IS_RECORDING.load(Ordering::Relaxed) {
                         ind.set_state(IndicatorState::Hidden);
                    }
                });
            }
        }
        return Ok(());
    }

    // 后处理
    let final_text = post_process(raw_text, &config);
    if final_text.is_empty() {
         write_log(LogLevel::WARN, "后处理后结果为空");
         return Ok(());
    }

    write_log(LogLevel::INFO, &format!("[输出] {}", final_text));
    
    #[cfg(target_os = "windows")]
    if config.enable_indicator() {
        if let Some(ind) = INDICATOR.get() {
            ind.set_state(IndicatorState::Success);
            let ind = ind.clone();
             tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                if !IS_RECORDING.load(Ordering::Relaxed) {
                     ind.set_state(IndicatorState::Hidden);
                }
            });
        }
    }

    let mode = config.output_mode();
    if mode == "clipboard" {
        #[cfg(target_os = "windows")]
        unsafe {
            // 1. 备份当前剪贴板 (仅限文本)
            let backup = win_utils::get_clipboard_text();
            
            // 2. 写入新内容并粘贴 (排除在历史记录之外，不污染 Win+V)
            win_utils::set_clipboard_text(&final_text, true);
            win_utils::paste_clipboard();
            
            // 3. 延迟还原剪贴板 (同样排除在历史记录之外，避免重复记录)
            if let Some(old_text) = backup {
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    win_utils::set_clipboard_text(&old_text, true);
                    write_log_line("[系统] 剪贴板内容已还原 (已从历史记录中排除)");
                });
            } else {
                // 如果原本就是空的，延迟清空
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    use windows::Win32::System::DataExchange::{OpenClipboard, EmptyClipboard, CloseClipboard};
                    let _ = OpenClipboard(None);
                    let _ = EmptyClipboard();
                    let _ = CloseClipboard();
                    write_log_line("[系统] 剪贴板已清空");
                });
            }
        }
    } else {
        #[cfg(target_os = "windows")]
        unsafe {
            win_utils::send_unicode_text(&final_text);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let mut enigo = Enigo::new(&Settings::default()).unwrap();
            let _ = enigo.text(&final_text);
        }
    }

    Ok(())
}

fn resample_and_convert(input: &[f32], input_rate: u32) -> (Vec<i16>, u32) {
    let target_rate = 16000;
    
    // 如果原始采样率小于等于目标采样率，不做降采样，直接转换
    if input_rate <= target_rate {
        let output: Vec<i16> = input.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        return (output, input_rate);
    }

    // 计算降采样比率 (简单的整数比率)
    // 例如 48000 / 16000 = 3
    // 如果不是整数倍，这里会有点误差，但对于语音识别通常可接受
    let ratio = (input_rate as f32 / target_rate as f32).round() as usize;
    if ratio <= 1 {
         let output: Vec<i16> = input.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        return (output, input_rate);
    }

    let est_capacity = input.len() / ratio + 1;
    let mut output = Vec::with_capacity(est_capacity);

    // 均值池化 (Average Pooling) 降采样，充当简单的低通滤波
    for chunk in input.chunks(ratio) {
        let sum: f32 = chunk.iter().sum();
        let avg = sum / chunk.len() as f32;
        let sample_i16 = (avg.clamp(-1.0, 1.0) * 32767.0) as i16;
        output.push(sample_i16);
    }
    
    // 计算实际的新采样率
    let actual_new_rate = input_rate / ratio as u32;
    (output, actual_new_rate)
}

#[cfg(target_os = "windows")]
static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| reqwest::Client::new());

#[cfg(target_os = "windows")]
static LOG_PIPE_HANDLE: Lazy<StdMutex<Option<windows::Win32::Foundation::HANDLE>>> = Lazy::new(|| StdMutex::new(None));

#[cfg(target_os = "windows")]
static LOG_VIEWER_CHILD: Lazy<StdMutex<Option<std::process::Child>>> = Lazy::new(|| StdMutex::new(None));

#[cfg(target_os = "windows")]
fn init_log_pipe() {
    use std::thread;
    use windows::Win32::System::Pipes::{CreateNamedPipeW, ConnectNamedPipe};
    use windows::core::PCWSTR;
    let name: Vec<u16> = "\\\\.\\pipe\\voice2type_log\0".encode_utf16().collect();
    thread::spawn(move || {
        unsafe {
            let handle = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0x00000002),
                windows::Win32::System::Pipes::NAMED_PIPE_MODE(0),
                1,
                4096,
                4096,
                0,
                None,
            );
            *LOG_PIPE_HANDLE.lock().unwrap() = Some(handle);
            let _ = ConnectNamedPipe(handle, None);
        }
    });
}

#[cfg(target_os = "windows")]
fn start_log_viewer() {
    use std::process::Command;
    if LOG_VIEWER_CHILD.lock().unwrap().is_none() {
        if let Ok(exe) = std::env::current_exe() {
            if let Ok(child) = Command::new(exe).arg("--log-viewer").spawn() {
                *LOG_VIEWER_CHILD.lock().unwrap() = Some(child);
                std::thread::spawn(|| {
                    use std::time::Duration;
                    loop {
                        let exited = {
                            let mut guard = LOG_VIEWER_CHILD.lock().unwrap();
                            if let Some(ch) = guard.as_mut() {
                                ch.try_wait().map(|o| o.is_some()).unwrap_or(false)
                            } else {
                                // 已被外部关闭
                                true
                            }
                        };
                        if exited {
                            // 子进程已退出：关闭日志并同步 UI 勾选
                            #[cfg(target_os = "windows")]
                            {
                                use windows::Win32::Foundation::CloseHandle;
                                if let Some(handle) = LOG_PIPE_HANDLE.lock().unwrap().take() {
                                    unsafe { let _ = CloseHandle(handle); }
                                }
                                if let Some(cfg) = CONFIG_GLOBAL.get() {
                                    cfg.set_show_log(false);
                                    let _ = cfg.save();
                                }
                                *LOG_VIEWER_CHILD.lock().unwrap() = None;
                                request_uncheck_log_menu();
                            }
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                });
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    DEBUG,
    INFO,
    WARN,
    ERROR,
}

#[cfg(target_os = "windows")]
fn write_log(level: LogLevel, s: &str) {
    use windows::Win32::Storage::FileSystem::WriteFile;
    use std::fs::OpenOptions;
    use std::io::Write;

    let level_str = match level {
        LogLevel::DEBUG => "DEBUG",
        LogLevel::INFO => "INFO ",
        LogLevel::WARN => "WARN ",
        LogLevel::ERROR => "ERROR",
    };
    
    let time_str = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let log_entry = format!("[{}][{}] {}", time_str, level_str, s);

    // 1. 写入本地文件
    if let Some(cfg) = CONFIG_GLOBAL.get() {
        let log_file = cfg.log_file_path();
        if let Ok(mut file) = OpenOptions::new().append(true).open(log_file) {
            let _ = writeln!(file, "{}", log_entry);
        }
    }

    // 2. 写入命名管道 (供实时查看器使用)
    unsafe {
        if let Some(handle) = *LOG_PIPE_HANDLE.lock().unwrap() {
            let mut buf = Vec::with_capacity(log_entry.len() + 2);
            buf.extend_from_slice(log_entry.as_bytes());
            buf.extend_from_slice(b"\r\n");
            let mut written = 0u32;
            let _ = WriteFile(handle, Some(&buf), Some(&mut written), None);
        }
    }
}

#[cfg(target_os = "windows")]
fn write_log_line(s: &str) {
    write_log(LogLevel::INFO, s);
}

#[cfg(target_os = "windows")]
pub fn log_set_enabled(enabled: bool) {
    use windows::Win32::Foundation::CloseHandle;
    if enabled {
        LOG_MENU_NEEDS_UNCHECK.store(false, Ordering::SeqCst);
        if LOG_PIPE_HANDLE.lock().unwrap().is_none() {
            init_log_pipe();
            start_log_viewer();
        }
    } else {
        // 停止子进程
        if let Some(mut child) = LOG_VIEWER_CHILD.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // 关闭管道
        if let Some(handle) = LOG_PIPE_HANDLE.lock().unwrap().take() {
            unsafe { let _ = CloseHandle(handle); }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn write_log_line(_s: &str) {}

#[cfg(target_os = "windows")]
fn viewer_main() {
    unsafe {
        win_utils::show_console_with_redirect();
    }

    let config = ConfigManager::new();
    let log_file = config.log_file_path();

    // 1. 先读取历史日志文件
    if let Ok(history) = std::fs::read_to_string(&log_file) {
        print!("{}", history);
    }

    // 2. 连接管道读取实时日志
    use windows::Win32::Storage::FileSystem::{CreateFileW, ReadFile, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::core::PCWSTR;
    let name: Vec<u16> = "\\\\.\\pipe\\voice2type_log\0".encode_utf16().collect();
    unsafe {
        let h = CreateFileW(
            PCWSTR(name.as_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        );
        let h = match h {
            Ok(h) => h,
            Err(_) => return,
        };
        let mut buf = vec![0u8; 4096];
        loop {
            let mut read = 0u32;
            let ok = ReadFile(h, Some(&mut buf), Some(&mut read), None).is_ok();
            if !ok || read == 0 {
                break;
            }
            if let Ok(text) = String::from_utf8(buf[..read as usize].to_vec()) {
                print!("{}", text);
            }
        }
    }
}

fn post_process(text: &str, config: &ConfigManager) -> String {
    let mut result = text.to_string();

    // 1. 过滤 Emoji
    if !config.allow_emoji() {
        // 匹配 Emoji 和象形文字的正则
        if let Ok(re) = Regex::new(r"[\p{Emoji_Presentation}\p{Extended_Pictographic}]") {
            result = re.replace_all(&result, "").to_string();
        }
    }

    // 2. 过滤标点
    if !config.allow_punctuation() {
        // 定义需要特殊处理的数字内部标点
        // 我们只保护 ASCII 数字分隔符：点、冒号、逗号、连字符
        let is_numeric_separator = |c: char| matches!(c, '.' | ':' | ',' | '-');
        
        // 匹配任何标点符号的正则
        if let Ok(punct_re) = Regex::new(r"[\p{P}]") {
             let chars: Vec<char> = result.chars().collect();
             let mut new_result = String::with_capacity(result.len());
             
             for (i, &c) in chars.iter().enumerate() {
                 let s = c.to_string();
                 if punct_re.is_match(&s) {
                     // 是标点，检查是否需要保留
                     let mut preserve = false;
                     if is_numeric_separator(c) {
                         let prev_is_digit = i > 0 && chars[i-1].is_ascii_digit();
                         let next_is_digit = i + 1 < chars.len() && chars[i+1].is_ascii_digit();
                         if prev_is_digit && next_is_digit {
                             preserve = true;
                         }
                     }
                     
                     if preserve {
                         new_result.push(c);
                     } else {
                         new_result.push(' ');
                     }
                 } else {
                     new_result.push(c);
                 }
             }
             result = new_result;
        }
    }

    // 3. 合并空格 (如果标点被替换或已有多个空格)
    if let Ok(re) = Regex::new(r"\s+") {
        result = re.replace_all(&result, " ").to_string();
    }

    result.trim().to_string()
}

fn encode_wav_memory(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    // 预分配内存：Header (约44 bytes) + Data (samples * 2 bytes)
    // 这样可以避免 Vec 扩容带来的内存拷贝
    let expected_size = 44 + samples.len() * 2;
    let mut cursor = Cursor::new(Vec::with_capacity(expected_size));
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
    }

    Ok(cursor.into_inner())
}
