#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod api;
mod config;
mod core;
mod gui;
mod output;
mod utils;
mod win_utils;
mod indicator;
mod update;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dotenv::dotenv;
use tokio::sync::mpsc;
use chrono::Local;

use audio::processor::{resample_and_convert, encode_wav_memory};
use audio::vad::AudioSegmenter;
use api::client::HTTP_CLIENT;
use config::ConfigManager;
use core::state::AppState;

/// 流式处理状态
#[derive(Debug, Clone)]
struct StreamingState {
    pub last_result: String,
    pub output_sent: bool,
    // API调用节流相关字段
    pub processing: bool, // 是否有正在进行的API调用
}

impl Default for StreamingState {
    fn default() -> Self {
        Self {
            last_result: String::new(),
            output_sent: false,
            processing: false,
        }
    }
}

impl StreamingState {
    /// 创建新的流式处理状态
    pub fn new() -> Self {
        Default::default()
    }
    
    /// 更新最后结果
    pub fn update_last_result(&mut self, result: &str) {
        self.last_result = result.to_string();
    }
    
    /// 重置流式处理状态
    pub fn reset(&mut self) {
        self.last_result.clear();
        self.output_sent = false;
        self.processing = false;
    }
}
use gui::Voice2TypeApp;
use indicator::{StatusIndicator, IndicatorState};
use output::handler::{OutputHandler, post_process};
use utils::hotkey::InputMessage;
use utils::logger::{LogLevel, write_log, write_log_line, log_set_enabled, init_log_pipe, start_log_viewer, viewer_main};
use native_windows_gui as nwg;

#[cfg(target_os = "windows")]
use once_cell::sync::{OnceCell, Lazy};

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
static APP_MUTEX: Lazy<StdMutex<Option<windows::Win32::Foundation::HANDLE>>> = Lazy::new(|| StdMutex::new(None));

#[cfg(target_os = "windows")]
pub fn release_app_mutex() {
    use windows::Win32::Foundation::CloseHandle;
    if let Ok(mut guard) = APP_MUTEX.lock() {
        if let Some(handle) = guard.take() {
            unsafe { let _ = CloseHandle(handle); }
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
        use windows::Win32::System::Threading::CreateMutexW;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, CloseHandle};
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONEXCLAMATION};
        use windows::core::PCWSTR;

        // 创建全局互斥体
        let mutex_name = "Global\\Voice2TypeAppMutex\0".encode_utf16().collect::<Vec<u16>>();
        
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
             MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONEXCLAMATION);
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
        write_log_line("[系统] 程序当前以管理员权限运行 (兼容性最强)", Some(&config));
    } else {
        write_log_line("[警告]⚠️⚠️⚠️ 未检测到管理员权限！这将导致无法在管理员权限的游戏或程序中使用⚠️⚠️⚠️", Some(&config));
        write_log_line("[提示] 程序已配置为请求管理员权限，请确保在弹出的 UAC 窗口中选择“是”。", Some(&config));
    }

    // 打印欢迎信息
    write_log_line("--------------------------------------------------", Some(&config));
    write_log_line(" 选中目标输入框，按住热键说话，松开后文字将自动输入", Some(&config));
    write_log_line(" 若想取消录音，在按住热键时按下 ESC 即可", Some(&config));
    write_log_line(&format!(" 当前版本: {}", env!("CARGO_PKG_VERSION")), Some(&config));
    write_log_line(" 若有任何问题，请联系作者", Some(&config));
    write_log_line("--------------------------------------------------", Some(&config));

    // 音频系统初始化
    let host = cpal::default_host();
    let device = host.default_input_device().context("未找到音频输入设备")?;
    let dev_config = device.default_input_config().context("无法获取默认输入配置")?;
    let stream_config: cpal::StreamConfig = dev_config.clone().into();
    let sample_rate = stream_config.sample_rate.0;
    let channels = stream_config.channels;

    write_log_line(&format!("[音频] 采样率: {}Hz, 通道数: {}", sample_rate, channels), Some(&config));

    let audio_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let audio_buffer_writer = audio_buffer.clone();
    
    // 音频分片器
    let segmenter: Arc<Mutex<AudioSegmenter>> = Arc::new(Mutex::new(AudioSegmenter::new(sample_rate)));
    
    // 流式处理状态
    let streaming_state: Arc<Mutex<StreamingState>> = Arc::new(Mutex::new(StreamingState::new()));

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
            write_log(LogLevel::ERROR, &format!("音频输入流错误: {}", err), Some(&config_clone));
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
    
    // 流式处理定时器
    let streaming_timer = tokio::time::interval(std::time::Duration::from_millis(500));
    tokio::pin!(streaming_timer);

    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                match msg {
                    InputMessage::StartRecording => {
                        if current_state == AppState::Idle || current_state == AppState::Processing || current_state == AppState::Cancelled {
                            current_state = AppState::Recording;
                            write_log_line("--> [录音] 开始录音... (按 ESC 取消)", Some(&config)); 
                            audio_buffer.lock().unwrap().clear();
                            segmenter.lock().unwrap().reset();
                            streaming_state.lock().unwrap().reset();
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
                            segmenter.lock().unwrap().reset();
                            streaming_state.lock().unwrap().reset();
                            
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
            _ = streaming_timer.tick() => {
                if current_state == AppState::Recording && config.enable_streaming() {
                    // 获取音频数据
                    let audio_data = {
                        let mut lock = audio_buffer.lock().unwrap();
                        std::mem::take(&mut *lock)
                    };
                    
                    if !audio_data.is_empty() {
                        // 使用VAD进行音频分片
                        let mut segmenter_lock = segmenter.lock().unwrap();
                        let need_segment = segmenter_lock.process_audio(&audio_data);
                        
                        if need_segment {
                            let chunk_data = segmenter_lock.take_current_segment();
                            drop(segmenter_lock);
                            
                            if !chunk_data.is_empty() {
                                let mut streaming = streaming_state.lock().unwrap();
                                
                                // 检查是否有正在进行的API调用
                                if streaming.processing {
                                    // 如果正在处理，跳过此次触发，避免并发请求
                                    write_log(LogLevel::DEBUG, "流式处理: 跳过，已有请求在处理中", Some(&config));
                                } else {
                                    streaming.processing = true; // 标记为正在处理
                                    
                                    let config_clone = config.clone();
                                    let config_clone_for_log = config_clone.clone();
                                    let streaming_state_clone1 = streaming_state.clone();
                                    let streaming_state_clone2 = streaming_state.clone();
                                    
                                    drop(streaming);
                                    
                                    tokio::spawn(async move {
                                        if let Err(e) = process_audio_streaming(chunk_data, sample_rate, config_clone, streaming_state_clone1).await {
                                            write_log(LogLevel::ERROR, &format!("流式转写失败: {}", e), Some(&config_clone_for_log));
                                        }
                                        // 处理完成后，重置processing状态
                                        let mut streaming = streaming_state_clone2.lock().unwrap();
                                        streaming.processing = false;
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn process_audio_and_type(audio_data: Vec<f32>, sample_rate: u32, config: Arc<ConfigManager>) -> Result<()> {
    if audio_data.is_empty() {
        write_log(LogLevel::WARN, "音频数据为空", Some(&config));
        return Ok(());
    }

    // 检查 API Key
    let api_key = config.get_api_key();
    if api_key.is_empty() {
        write_log(LogLevel::ERROR, "[错误] API Key 未配置！请在托盘菜单中设置。", Some(&config));
        #[cfg(target_os = "windows")]
        if config.enable_indicator() {
            if let Some(ind) = INDICATOR.get() {
                ind.set_state(IndicatorState::Error);
            }
        }
        return Ok(());
    }

    let start_time = std::time::Instant::now();

    // 1. 降采样和格式转换 (f32 -> i16, Target 16kHz)
    let (processed_samples, new_rate) = resample_and_convert(&audio_data, sample_rate);
    
    let resample_duration = start_time.elapsed();
    
    // 2. 编码 WAV (16-bit)
    let wav_data = encode_wav_memory(&processed_samples, 16000)?;

    let encode_duration = start_time.elapsed() - resample_duration;

    write_log_line(&format!(
        "[性能] 原始大小: {}k, 处理后大小: {}k, 降采样耗时: {:?}, 编码耗时: {:?}", 
        audio_data.len() * 4 / 1024, 
        wav_data.len() / 1024,
        resample_duration,
        encode_duration
    ), Some(&config));

    // 确保使用明确的语言参数
    let language = if config.output_language() == "auto" {
        "zh".to_string() // 默认使用中文
    } else {
        config.output_language()
    };

    write_log_line(&format!(
        "[API] 模型: {}, 语言: {}", 
        config.get_model_name(),
        language
    ), Some(&config));

    let form = reqwest::multipart::Form::new()
        .text("model", config.get_model_name())
        .text("language", language)
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
    write_log_line(&format!("[性能] API 请求耗时: {:?}", upload_duration), Some(&config));

    if !resp.status().is_success() {
        let error_text = resp.text().await?;
        write_log(LogLevel::ERROR, &format!("API 错误: {}", error_text), Some(&config));
        
        #[cfg(target_os = "windows")]
        if config.enable_indicator() {
            if let Some(ind) = INDICATOR.get() {
                ind.set_state(IndicatorState::Error);
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
            write_log(LogLevel::ERROR, &format!("解析响应失败: {}", e), Some(&config));
             #[cfg(target_os = "windows")]
            if config.enable_indicator() {
                if let Some(ind) = INDICATOR.get() {
                    ind.set_state(IndicatorState::Error);
                }
            }
            return Ok(());
        }
    };

    let raw_text = result.text.trim();

    if raw_text.is_empty() {
        write_log(LogLevel::WARN, "识别结果为空 (未检测到有效语音)", Some(&config));
        #[cfg(target_os = "windows")]
        if config.enable_indicator() {
            if let Some(ind) = INDICATOR.get() {
                ind.set_state(IndicatorState::Error); // 用 Error 状态表示空结果
            }
        }
        return Ok(());
    }

    // 后处理
    let final_text = post_process(raw_text, &config);
    if final_text.is_empty() {
         write_log(LogLevel::WARN, "后处理后结果为空", Some(&config));
         return Ok(());
    }

    write_log(LogLevel::INFO, &format!("[输出] {}", final_text), Some(&config));
    
    #[cfg(target_os = "windows")]
    if config.enable_indicator() {
        if let Some(ind) = INDICATOR.get() {
            ind.set_state(IndicatorState::Success);
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
                let config_clone = config.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    win_utils::set_clipboard_text(&old_text, true);
                    write_log_line("[系统] 剪贴板内容已还原 (已从历史记录中排除)", Some(&config_clone));
                });
            } else {
                let config_clone = config.clone();
                // 如果原本就是空的，延迟清空
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    use windows::Win32::System::DataExchange::{OpenClipboard, EmptyClipboard, CloseClipboard};
                    let _ = OpenClipboard(None);
                    let _ = EmptyClipboard();
                    let _ = CloseClipboard();
                    write_log_line("[系统] 剪贴板已清空", Some(&config_clone));
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
            use enigo::{Enigo, Settings};
            let mut enigo = Enigo::new(&Settings::default()).unwrap();
            let _ = enigo.text(&final_text);
        }
    }

    Ok(())
}

async fn process_audio_streaming(audio_data: Vec<f32>, sample_rate: u32, config: Arc<ConfigManager>, streaming_state: Arc<Mutex<StreamingState>>) -> Result<()> {
    if audio_data.is_empty() {
        return Ok(());
    }

    // 检查 API Key
    let api_key = config.get_api_key();
    if api_key.is_empty() {
        return Ok(());
    }

    // 1. 降采样和格式转换 (f32 -> i16, Target 16kHz)
    let (processed_samples, new_rate) = resample_and_convert(&audio_data, sample_rate);
    
    // 2. 编码 WAV (16-bit)
    let wav_data = encode_wav_memory(&processed_samples, 16000)?;

    // 确保使用明确的语言参数
    let language = if config.output_language() == "auto" {
        "zh".to_string() // 默认使用中文
    } else {
        config.output_language()
    };

    let form = reqwest::multipart::Form::new()
        .text("model", config.get_model_name())
        .text("language", language)
        .part("file", reqwest::multipart::Part::bytes(wav_data)
            .file_name("recording.wav")
            .mime_str("audio/wav")?);

    let resp = HTTP_CLIENT.post(&config.get_api_url())
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await?;
    
    if !resp.status().is_success() {
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
        Err(_) => return Ok(()),
    };

    let raw_text = result.text.trim();

    if raw_text.is_empty() {
        return Ok(());
    }

    // 后处理
    let final_text = post_process(raw_text, &config);
    if final_text.is_empty() {
        return Ok(());
    }

    // 处理流式结果
    let mut streaming = streaming_state.lock().unwrap();
    let last_result = streaming.last_result.clone();
    
    // 计算新增的文本
    let new_text = if final_text.starts_with(&last_result) {
        &final_text[last_result.len()..]
    } else {
        &final_text
    };
    
    if !new_text.is_empty() {
        write_log(LogLevel::INFO, &format!("[流式输出] {}", new_text), Some(&config));
        
        // 实时输出新增文本
        #[cfg(target_os = "windows")]
        unsafe {
            win_utils::send_unicode_text(new_text);
        }
        
        streaming.update_last_result(&final_text);
        streaming.output_sent = true;
    }

    Ok(())
}






