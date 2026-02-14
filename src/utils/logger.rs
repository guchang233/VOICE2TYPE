use chrono::Local;
use once_cell::sync::Lazy;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::CloseHandle;
#[cfg(target_os = "windows")]
use windows::Win32::Storage::FileSystem::WriteFile;
#[cfg(target_os = "windows")]
use windows::core::PCWSTR;

/// 日志级别
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    DEBUG,
    INFO,
    WARN,
    ERROR,
}

#[cfg(target_os = "windows")]
pub static LOG_PIPE_HANDLE: Lazy<Mutex<Option<windows::Win32::Foundation::HANDLE>>> = Lazy::new(|| Mutex::new(None));

#[cfg(target_os = "windows")]
pub static LOG_VIEWER_CHILD: Lazy<Mutex<Option<std::process::Child>>> = Lazy::new(|| Mutex::new(None));

/// 初始化日志管道
#[cfg(target_os = "windows")]
pub fn init_log_pipe() {
    use std::thread;
    use windows::Win32::System::Pipes::{CreateNamedPipeW, ConnectNamedPipe};
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

/// 启动日志查看器
#[cfg(target_os = "windows")]
pub fn start_log_viewer() {
    use std::process::Command;
    if LOG_VIEWER_CHILD.lock().unwrap().is_none() {
        if let Ok(exe) = std::env::current_exe() {
            if let Ok(child) = Command::new(exe).arg("--log-viewer").spawn() {
                *LOG_VIEWER_CHILD.lock().unwrap() = Some(child);
                std::thread::spawn(|| {
                    use std::time::Duration;
                    loop {
                        let exited = {
                            if let Ok(mut guard) = LOG_VIEWER_CHILD.try_lock() {
                                if let Some(ch) = guard.as_mut() {
                                    ch.try_wait().map(|o| o.is_some()).unwrap_or(false)
                                } else {
                                    // 已被外部关闭
                                    true
                                }
                            } else {
                                // 无法获取锁，继续检查
                                false
                            }
                        };
                        if exited {
                            // 子进程已退出：关闭日志并同步 UI 勾选
                            #[cfg(target_os = "windows")]
                            {
                                if let Ok(mut guard) = LOG_PIPE_HANDLE.try_lock() {
                                    if let Some(handle) = guard.take() {
                                        unsafe { let _ = CloseHandle(handle); }
                                    }
                                }
                                if let Some(cfg) = crate::CONFIG_GLOBAL.get() {
                                    cfg.set_show_log(false);
                                    let _ = cfg.save();
                                }
                                if let Ok(mut guard) = LOG_VIEWER_CHILD.try_lock() {
                                    *guard = None;
                                }
                                crate::request_uncheck_log_menu();
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

/// 写入日志
#[cfg(target_os = "windows")]
pub fn write_log(level: LogLevel, s: &str, config: Option<&crate::config::ConfigManager>) {
    let level_str = match level {
        LogLevel::DEBUG => "DEBUG",
        LogLevel::INFO => "INFO ",
        LogLevel::WARN => "WARN ",
        LogLevel::ERROR => "ERROR",
    };
    
    let time_str = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let log_entry = format!("[{}][{}] {}", time_str, level_str, s);

    // 1. 写入本地文件
    if let Some(cfg) = config {
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

/// 写入日志行
#[cfg(target_os = "windows")]
pub fn write_log_line(s: &str, config: Option<&crate::config::ConfigManager>) {
    write_log(LogLevel::INFO, s, config);
}

/// 设置日志启用状态
#[cfg(target_os = "windows")]
pub fn log_set_enabled(enabled: bool, config: Option<&crate::config::ConfigManager>) {
    if enabled {
        crate::LOG_MENU_NEEDS_UNCHECK.store(false, std::sync::atomic::Ordering::SeqCst);
        if LOG_PIPE_HANDLE.lock().unwrap().is_none() {
            init_log_pipe();
            start_log_viewer();
        }
    } else {
        // 停止子进程
        if let Ok(mut guard) = LOG_VIEWER_CHILD.try_lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        // 关闭管道
        if let Ok(mut guard) = LOG_PIPE_HANDLE.try_lock() {
            if let Some(handle) = guard.take() {
                unsafe { let _ = CloseHandle(handle); }
            }
        }
        // 更新配置
        if let Some(cfg) = config {
            cfg.set_show_log(false);
            let _ = cfg.save();
        }
        // 请求取消日志菜单的勾选
        crate::request_uncheck_log_menu();
    }
}

/// 日志查看器主函数
#[cfg(target_os = "windows")]
pub fn viewer_main() {
    unsafe {
        crate::win_utils::show_console_with_redirect();
    }

    let config = crate::config::ConfigManager::new();
    let log_file = config.log_file_path();

    // 1. 先读取历史日志文件
    if let Ok(history) = std::fs::read_to_string(&log_file) {
        print!("{}", history);
    }

    // 2. 连接管道读取实时日志
    use windows::Win32::Storage::FileSystem::{CreateFileW, ReadFile, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
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
