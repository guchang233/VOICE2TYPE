//! 流式模式专用热键：低级键盘钩子吞掉热键，避免在输入框内触发 F6 等副作用

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use tokio::sync::mpsc;

use crate::config::ConfigManager;

#[derive(Debug, Clone, Copy)]
pub enum StreamingInputMessage {
    Start,
    Stop,
    Cancel,
}

static SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
pub fn start_streaming_hotkey_listener(
    tx: mpsc::Sender<StreamingInputMessage>,
    config: Arc<ConfigManager>,
) {
    thread::spawn(move || {
        if let Err(e) = run_hook_thread(tx, config) {
            eprintln!("流式热键监听错误: {}", e);
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn start_streaming_hotkey_listener(
    _tx: mpsc::Sender<StreamingInputMessage>,
    _config: Arc<ConfigManager>,
) {
}

#[cfg(target_os = "windows")]
fn run_hook_thread(
    tx: mpsc::Sender<StreamingInputMessage>,
    config: Arc<ConfigManager>,
) -> Result<(), String> {
    use std::sync::OnceLock;
    use std::sync::Mutex as StdMutex;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
        WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    static TX: OnceLock<StdMutex<Option<mpsc::Sender<StreamingInputMessage>>>> = OnceLock::new();
    static CFG: OnceLock<Arc<ConfigManager>> = OnceLock::new();
    static TOGGLE_ON: AtomicBool = AtomicBool::new(false);
    static HOTKEY_DOWN: AtomicBool = AtomicBool::new(false);

    TX.set(StdMutex::new(Some(tx))).map_err(|_| "tx init")?;
    CFG.set(config).map_err(|_| "cfg init")?;

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code != HC_ACTION as i32 {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let cfg = CFG.get().expect("cfg");
        let tx = TX
            .get()
            .and_then(|m| m.lock().ok())
            .and_then(|g| g.clone());

        let kb = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode;
        let target = cfg.streaming_hotkey();
        let is_down =
            wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;
        let is_up = wparam.0 == WM_KEYUP as usize || wparam.0 == WM_SYSKEYUP as usize;
        let mode = cfg.streaming_trigger_mode();

        if vk == VK_ESCAPE.0 as u32 && is_down && SESSION_ACTIVE.load(Ordering::SeqCst) {
            if let Some(tx) = tx.as_ref() {
                let _ = tx.try_send(StreamingInputMessage::Cancel);
            }
            return LRESULT(1);
        }

        if vk != target {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        // 吞掉流式热键，不送入当前输入框
        if mode == "hold" {
            if is_down && !HOTKEY_DOWN.swap(true, Ordering::SeqCst) {
                SESSION_ACTIVE.store(true, Ordering::SeqCst);
                if let Some(tx) = tx.as_ref() {
                    let _ = tx.try_send(StreamingInputMessage::Start);
                }
            } else if is_up && HOTKEY_DOWN.swap(false, Ordering::SeqCst) {
                SESSION_ACTIVE.store(false, Ordering::SeqCst);
                if let Some(tx) = tx.as_ref() {
                    let _ = tx.try_send(StreamingInputMessage::Stop);
                }
            }
            return LRESULT(1);
        }

        // toggle：仅在按下沿切换
        if is_down && !HOTKEY_DOWN.swap(true, Ordering::SeqCst) {
            if TOGGLE_ON.load(Ordering::SeqCst) {
                TOGGLE_ON.store(false, Ordering::SeqCst);
                SESSION_ACTIVE.store(false, Ordering::SeqCst);
                if let Some(tx) = tx.as_ref() {
                    let _ = tx.try_send(StreamingInputMessage::Stop);
                }
            } else {
                TOGGLE_ON.store(true, Ordering::SeqCst);
                SESSION_ACTIVE.store(true, Ordering::SeqCst);
                if let Some(tx) = tx.as_ref() {
                    let _ = tx.try_send(StreamingInputMessage::Start);
                }
            }
            return LRESULT(1);
        }
        if is_up {
            HOTKEY_DOWN.store(false, Ordering::SeqCst);
            return LRESULT(1);
        }

        LRESULT(1)
    }

    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(hook_proc),
            GetModuleHandleW(None).map_err(|e| e.to_string())?,
            0,
        )
        .map_err(|e| e.to_string())?
    };

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() } {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(hook);
    }
    Ok(())
}
