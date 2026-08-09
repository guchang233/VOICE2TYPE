use chrono::Local;
use once_cell::sync::Lazy;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::CloseHandle;
#[cfg(target_os = "windows")]
use windows::Win32::Storage::FileSystem::WriteFile;

/// 日志级别
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    DEBUG,
    INFO,
    WARN,
    ERROR,
}

/// AppHandle 持有者，用于向前端推送日志事件
/// 在 setup() 中通过 set_app_handle() 设置
static APP_HANDLE: Lazy<Mutex<Option<AppHandle>>> = Lazy::new(|| Mutex::new(None));

/// 日志文件句柄缓存，避免每次写日志都重新打开文件
static LOG_FILE_HANDLE: Lazy<Mutex<Option<(std::path::PathBuf, std::fs::File)>>> =
    Lazy::new(|| Mutex::new(None));

#[cfg(target_os = "windows")]
pub static LOG_PIPE_HANDLE: Lazy<Mutex<Option<windows::Win32::Foundation::HANDLE>>> =
    Lazy::new(|| Mutex::new(None));

#[cfg(target_os = "windows")]
pub static LOG_VIEWER_CHILD: Lazy<Mutex<Option<std::process::Child>>> =
    Lazy::new(|| Mutex::new(None));

/// 自定义日志记录器，实现 log::Log trait
/// 捕获所有 log::info!/warn!/error!/debug! 调用，统一处理：
/// 1. 写入本地日志文件
/// 2. 通过 Tauri 事件推送到前端日志视图
/// 3. 写入命名管道（供外部日志查看器子进程使用，仅 Windows）
pub struct TauriLogger;

static LOGGER: TauriLogger = TauriLogger;

impl log::Log for TauriLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let level = match record.level() {
            log::Level::Error => "ERROR",
            log::Level::Warn => "WARN",
            log::Level::Info => "INFO",
            _ => "DEBUG",
        };

        let message = format!("{}", record.args());
        let time_str = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();

        // source：取模块路径最后一段作为来源标签
        let target = record.target();
        let source = target.rsplit("::").next().unwrap_or(target);

        let log_entry = format!("[{}][{}] {}", time_str, level, message);

        // 1. 写入本地日志文件
        write_to_file(&log_entry);

        // 2. 通过 Tauri 事件推送到前端
        if let Some(handle) = APP_HANDLE.lock().unwrap().as_ref() {
            let _ = handle.emit(
                "backend-log",
                serde_json::json!({
                    "level": level.to_lowercase(),
                    "message": message,
                    "source": source,
                    "time": time_str,
                }),
            );
        }

        // 3. 写入命名管道 (供外部日志查看器子进程使用)
        #[cfg(target_os = "windows")]
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

    fn flush(&self) {}
}

/// 写入日志到文件（带句柄缓存）
fn write_to_file(log_entry: &str) {
    let Some(cfg) = crate::CONFIG_GLOBAL.get() else {
        return;
    };
    let log_file = cfg.log_file_path();
    let mut guard = LOG_FILE_HANDLE.lock().unwrap();
    let file_ok = if let Some((ref path, _)) = *guard {
        path == &log_file
    } else {
        false
    };

    if !file_ok {
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&log_file) {
            *guard = Some((log_file.clone(), file));
        }
    }

    if let Some((_, ref mut file)) = *guard {
        let _ = writeln!(file, "{}", log_entry);
        let _ = file.flush();
    }
}

/// 初始化自定义日志记录器（替代 env_logger）
pub fn init_logger() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

/// 设置 AppHandle，启用向前端推送日志事件
/// 在 Tauri setup() 中调用
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.lock().unwrap() = Some(handle);
}

/// 写入日志（路由到 log 宏，由 TauriLogger 统一处理）
pub fn write_log(level: LogLevel, s: &str) {
    let lvl = match level {
        LogLevel::DEBUG => log::Level::Debug,
        LogLevel::INFO => log::Level::Info,
        LogLevel::WARN => log::Level::Warn,
        LogLevel::ERROR => log::Level::Error,
    };
    log::log!(lvl, "{}", s);
}

/// 写入 INFO 级别日志行
pub fn write_log_line(s: &str) {
    log::info!("{}", s);
}

/// 初始化日志管道
#[cfg(target_os = "windows")]
pub fn init_log_pipe() {
    use std::thread;
    use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW};
    let name: Vec<u16> = "\\\\.\\pipe\\voice2type_log\0".encode_utf16().collect();
    thread::spawn(move || unsafe {
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
                                if let Some(handle) = LOG_PIPE_HANDLE.lock().unwrap().take() {
                                    unsafe {
                                        let _ = CloseHandle(handle);
                                    }
                                }
                                if let Some(cfg) = crate::CONFIG_GLOBAL.get() {
                                    cfg.set_show_log(false);
                                    cfg.save_or_notify();
                                }
                                *LOG_VIEWER_CHILD.lock().unwrap() = None;
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

/// 设置日志启用状态
#[cfg(target_os = "windows")]
pub fn log_set_enabled(enabled: bool, _config: Option<&crate::config::ConfigManager>) {
    if enabled {
        crate::LOG_MENU_NEEDS_UNCHECK.store(false, std::sync::atomic::Ordering::SeqCst);
        if LOG_PIPE_HANDLE.lock().unwrap().is_none() {
            init_log_pipe();
            start_log_viewer();
        }
    } else {
        // 停止子进程
        {
            let mut guard = LOG_VIEWER_CHILD.lock().unwrap();
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        // 关闭管道
        {
            let mut guard = LOG_PIPE_HANDLE.lock().unwrap();
            if let Some(handle) = guard.take() {
                unsafe {
                    let _ = CloseHandle(handle);
                }
            }
        }
        // 关闭并释放缓存的文件句柄（下次写日志会自动重新打开）
        {
            let mut guard = LOG_FILE_HANDLE.lock().unwrap();
            *guard = None;
        }
        // 重置 LOG_MENU_NEEDS_UNCHECK 标志
        crate::LOG_MENU_NEEDS_UNCHECK.store(false, std::sync::atomic::Ordering::SeqCst);
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
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
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
