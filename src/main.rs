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
#[cfg(not(target_os = "windows"))]
use enigo::{Enigo, Settings};
use rdev::{listen, EventType, Key};
use tokio::sync::mpsc;
use regex::Regex;

use config::ConfigManager;
use gui::Voice2TypeApp;
use native_windows_gui as nwg;
// use native_windows_gui::NativeUi; 

#[cfg(target_os = "windows")]
use once_cell::sync::{OnceCell, Lazy};

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
        let temp_config = ConfigManager::new();
        if temp_config.show_log() {
            init_log_pipe();
            start_log_viewer();
        } else {
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
    let _ = CONFIG_GLOBAL.set(config_manager.clone());

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
    write_log_line("Voice2Type已启动...");

    // 打印欢迎信息
    write_log_line("--------------------------------------------------");
    write_log_line(" 选中目标输入框，按住 F2 说话，松开 F2 后即文字将直接注入对话框");
    write_log_line(" 若想取消，在按住 F2 时按下 ESC 即可");
    write_log_line(" 若游戏内无法正常输出，尝试以管理员模式启动这个程序");
    write_log_line(&format!(" 当前版本: {}", env!("CARGO_PKG_VERSION")));
    write_log_line(" 若有任何问题，请联系QQ 57262494");
    write_log_line("--------------------------------------------------");

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
            write_log_line(&format!("音频输入流错误: {}", err));
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
        write_log_line("[错误] API Key 未配置！请在托盘菜单中设置。");
        return Ok(());
    }

    let wav_data = encode_wav_memory(&audio_data, sample_rate)?;

    let client = reqwest::Client::new();
    let form = reqwest::multipart::Form::new()
        .text("model", config.get_model_name())
        .part("file", reqwest::multipart::Part::bytes(wav_data)
            .file_name("recording.wav")
            .mime_str("audio/wav")?);

    let resp = client.post(&config.get_api_url())
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await?;

    if !resp.status().is_success() {
        let error_text = resp.text().await?;
        write_log_line(&format!("API 错误: {}", error_text));
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

    write_log_line(&format!("[输出] {}", final_text));

    let mode = config.output_mode();
    if mode == "clipboard" {
        #[cfg(target_os = "windows")]
        unsafe {
            set_clipboard_text(&final_text);
            paste_clipboard();
        }
    } else {
        #[cfg(target_os = "windows")]
        unsafe {
            send_unicode_text(&final_text);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let mut enigo = Enigo::new(&Settings::default()).unwrap();
            let _ = enigo.text(&final_text);
        }
    }

    Ok(())
}

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
                        std::thread::sleep(Duration::from_millis(500));
                    }
                });
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn write_log_line(s: &str) {
    use windows::Win32::Storage::FileSystem::WriteFile;
    unsafe {
        if let Some(handle) = *LOG_PIPE_HANDLE.lock().unwrap() {
            let mut buf = Vec::with_capacity(s.len() + 2);
            buf.extend_from_slice(s.as_bytes());
            buf.extend_from_slice(b"\r\n");
            let mut written = 0u32;
            let _ = WriteFile(handle, Some(&buf), Some(&mut written), None);
        }
    }
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
        crate::gui::show_console_with_redirect();
    }
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
#[cfg(target_os = "windows")]
unsafe fn send_unicode_text(text: &str) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY
    };

    let mut inputs = Vec::with_capacity(text.len() * 2);

    for c in text.encode_utf16() {
        // Key Down
        let input_down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: c,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        inputs.push(input_down);

        // Key Up
        let input_up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: c,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        inputs.push(input_up);
    }

    if !inputs.is_empty() {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(target_os = "windows")]
unsafe fn set_clipboard_text(text: &str) {
    use windows::Win32::System::DataExchange::{OpenClipboard, EmptyClipboard, SetClipboardData, CloseClipboard};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let _ = OpenClipboard(None);
    let _ = EmptyClipboard();
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let size_bytes = utf16.len() * std::mem::size_of::<u16>();
    if let Ok(hglobal) = GlobalAlloc(GMEM_MOVEABLE, size_bytes) {
        let ptr = GlobalLock(hglobal);
        let ptr_u16 = ptr as *mut u16;
        if !ptr_u16.is_null() {
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr_u16, utf16.len());
        }
        let _ = GlobalUnlock(hglobal);
        let _ = SetClipboardData(CF_UNICODETEXT, HANDLE(hglobal.0 as isize));
        // 注意：成功后由系统接管 hglobal，不再释放
    }
    let _ = CloseClipboard();
}
#[cfg(target_os = "windows")]
unsafe fn paste_clipboard() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, KEYBD_EVENT_FLAGS
    };
    let vk_ctrl = VIRTUAL_KEY(0x11);
    let vk_v = VIRTUAL_KEY(0x56);
    let down_ctrl = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk_ctrl, wScan: 0, dwFlags: KEYBD_EVENT_FLAGS(0), time: 0, dwExtraInfo: 0 } } };
    let down_v = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk_v, wScan: 0, dwFlags: KEYBD_EVENT_FLAGS(0), time: 0, dwExtraInfo: 0 } } };
    let up_v = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk_v, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 } } };
    let up_ctrl = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk_ctrl, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 } } };
    let inputs = [down_ctrl, down_v, up_v, up_ctrl];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
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
