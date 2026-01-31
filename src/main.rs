#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod gui;

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dotenv::dotenv;
use enigo::{Enigo, Keyboard, Settings};
use rdev::{listen, EventType, Key};
use tokio::sync::mpsc;
use regex::Regex;

use config::ConfigManager;
use gui::Voice2TypeApp;
use native_windows_gui as nwg;
// use native_windows_gui::NativeUi; 

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

fn main() -> Result<()> {
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
             std::mem::forget(handle);
        }
    }

    // 1. 初始化环境与日志
    dotenv().ok();
    
    // 如果是 Windows GUI 模式，标准输出可能不可用。
    // 如果配置中开启了日志，我们需要手动分配控制台并重定向输出。
    #[cfg(target_os = "windows")]
    unsafe {
        use crate::gui::show_console_with_redirect;
        // 临时加载配置以检查是否需要显示日志
        let temp_config = ConfigManager::new();
        if temp_config.show_log() {
            show_console_with_redirect();
        } else {
            // 确保初始状态下没有残留的控制台窗口
            use windows::Win32::System::Console::GetConsoleWindow;
            use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
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

    // 4. 启动逻辑线程 (Tokio Runtime)
    let cm_clone = config_manager.clone();
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        
        if let Err(e) = rt.block_on(async_main(cm_clone)) {
            eprintln!("运行时错误: {}", e);
        }
    });

    // 5. 初始化 GUI
    let _ui = Voice2TypeApp::init(config_manager.clone());

    // 6. 运行 GUI 事件循环 (主线程阻塞在此)
    nwg::dispatch_thread_events();

    Ok(())
}

async fn async_main(config: Arc<ConfigManager>) -> Result<()> {
    println!("Voice2Type已启动...");

    // 打印欢迎信息
    println!("--------------------------------------------------");
    println!(" 选中目标输入框，按住 F2 说话，松开 F2 后即文字将直接注入对话框");
    println!(" 若想取消，在按住 F2 时按下 ESC 即可");
    println!(" 当前版本: {}", env!("CARGO_PKG_VERSION"));
    println!("--------------------------------------------------");

    // 音频系统初始化
    let host = cpal::default_host();
    let device = host.default_input_device().context("未找到音频输入设备")?;
    let dev_config = device.default_input_config().context("无法获取默认输入配置")?;
    let stream_config: cpal::StreamConfig = dev_config.clone().into();
    let sample_rate = stream_config.sample_rate.0;

    let audio_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let audio_buffer_writer = audio_buffer.clone();

    // 创建输入流
    let input_stream = device.build_input_stream(
        &stream_config,
        move |data: &[f32], _: &_| {
            if IS_RECORDING.load(Ordering::Relaxed) {
                if let Ok(mut buffer) = audio_buffer_writer.lock() {
                    buffer.extend_from_slice(data);
                }
            }
        },
        move |err| {
            eprintln!("音频输入流错误: {}", err);
        },
        None,
    )?;

    input_stream.play()?;

    // 通信通道
    let (tx, mut rx) = mpsc::channel(100);

    // 启动热键监听线程
    let tx_clone = tx.clone();
    thread::spawn(move || {
        let mut is_f2_pressed = false;
        if let Err(error) = listen(move |event| {
            match event.event_type {
                EventType::KeyPress(Key::F2) => {
                    if !is_f2_pressed {
                        is_f2_pressed = true;
                        let _ = tx_clone.blocking_send(InputMessage::StartRecording);
                    }
                }
                EventType::KeyRelease(Key::F2) => {
                    if is_f2_pressed {
                        is_f2_pressed = false;
                        let _ = tx_clone.blocking_send(InputMessage::StopRecording);
                    }
                }
                EventType::KeyPress(Key::Escape) => {
                    if is_f2_pressed {
                        // 如果 F2 正被按住，此时按下了 ESC -> 取消
                        let _ = tx_clone.blocking_send(InputMessage::CancelRecording);
                    }
                }
                _ => {}
            }
        }) {
            eprintln!("全局按键监听错误: {:?}", error);
        }
    });

    // 异步事件循环
    let mut current_state = AppState::Idle;

    while let Some(msg) = rx.recv().await {
        match msg {
            InputMessage::StartRecording => {
                if current_state == AppState::Idle || current_state == AppState::Processing || current_state == AppState::Cancelled {
                    current_state = AppState::Recording;
                    println!("--> [录音] 开始录音... (按 ESC 取消)"); 
                    audio_buffer.lock().unwrap().clear();
                    IS_RECORDING.store(true, Ordering::Relaxed);
                }
            }
            InputMessage::CancelRecording => {
                if current_state == AppState::Recording {
                    current_state = AppState::Cancelled;
                    IS_RECORDING.store(false, Ordering::Relaxed);
                    println!("--> [取消] 录音已取消");
                    audio_buffer.lock().unwrap().clear();
                }
            }
            InputMessage::StopRecording => {
                if current_state == AppState::Recording {
                    // 正常结束录音
                    IS_RECORDING.store(false, Ordering::Relaxed);
                    println!("--> [处理] 正在转换音频...");

                    let buffer_clone = audio_buffer.clone();
                    let config_clone = config.clone();
                    
                    tokio::spawn(async move {
                        if let Err(e) = process_audio_and_type(buffer_clone, sample_rate, config_clone).await {
                            eprintln!("转写失败: {}", e);
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

async fn process_audio_and_type(buffer: Arc<Mutex<Vec<f32>>>, sample_rate: u32, config: Arc<ConfigManager>) -> Result<()> {
    let audio_data = {
        let lock = buffer.lock().unwrap();
        lock.clone()
    };

    if audio_data.is_empty() {
        return Ok(());
    }

    // 检查 API Key
    let api_key = config.get_api_key();
    if api_key.is_empty() {
        // 我们可以通知 GUI 显示错误，但简单记录或忽略也行
        eprintln!("[错误] API Key 未配置！请在托盘菜单中设置。");
        return Ok(());
    }

    let wav_data = encode_wav_memory(&audio_data, sample_rate)?;

    let client = reqwest::Client::new();
    let form = reqwest::multipart::Form::new()
        .text("model", "FunAudioLLM/SenseVoiceSmall")
        .part("file", reqwest::multipart::Part::bytes(wav_data)
            .file_name("recording.wav")
            .mime_str("audio/wav")?);

    let resp = client.post("https://api.siliconflow.cn/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await?;

    if !resp.status().is_success() {
        let error_text = resp.text().await?;
        eprintln!("API 错误: {}", error_text);
        return Ok(());
    }

    #[derive(serde::Deserialize)]
    struct SiliconFlowResponse {
        text: String,
    }

    let result: SiliconFlowResponse = resp.json().await?;
    let raw_text = result.text.trim();

    if raw_text.is_empty() {
        return Ok(());
    }

    // 后处理
    let final_text = post_process(raw_text, &config);
    if final_text.is_empty() {
        return Ok(());
    }

    println!("[输出] {}", final_text);

    // 打字
    tokio::task::block_in_place(|| {
        let mut enigo = Enigo::new(&Settings::default()).unwrap();
        let _ = enigo.text(&final_text);
    });

    Ok(())
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
        // 用空格替换标点
        // \p{P} 匹配任何标点符号
        if let Ok(re) = Regex::new(r"[\p{P}]") {
            result = re.replace_all(&result, " ").to_string();
        }
    }

    // 3. 合并空格 (如果标点被替换或已有多个空格)
    if let Ok(re) = Regex::new(r"\s+") {
        result = re.replace_all(&result, " ").to_string();
    }

    result.trim().to_string()
}

fn encode_wav_memory(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
    }

    Ok(cursor.into_inner())
}
