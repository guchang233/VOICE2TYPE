use anyhow::Result;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HANDLE;
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, KEYBD_EVENT_FLAGS
};

#[cfg(target_os = "windows")]
pub fn is_admin() -> bool {
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_ok() {
            let mut elevation = TOKEN_ELEVATION::default();
            let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
            if GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut _),
                size,
                &mut size,
            ).is_ok() {
                return elevation.TokenIsElevated != 0;
            }
        }
    }
    false
}

#[cfg(target_os = "windows")]
pub unsafe fn send_unicode_text(text: &str) {
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
pub unsafe fn get_clipboard_text() -> Option<String> {
    use windows::Win32::System::DataExchange::{OpenClipboard, GetClipboardData, CloseClipboard, IsClipboardFormatAvailable};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows::Win32::Foundation::HGLOBAL;
    
    const CF_UNICODETEXT: u32 = 13;

    if !OpenClipboard(None).is_ok() {
        return None;
    }

    let mut result = None;
    if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
        if let Ok(h_mem) = GetClipboardData(CF_UNICODETEXT) {
            if h_mem.0 != 0 {
                let ptr = GlobalLock(HGLOBAL(h_mem.0 as *mut _));
                if !ptr.is_null() {
                    let wide_slice = std::slice::from_raw_parts(ptr as *const u16, 2048);
                    let len = wide_slice.iter().position(|&c| c == 0).unwrap_or(2048);
                    result = Some(String::from_utf16_lossy(&wide_slice[..len]));
                    let _ = GlobalUnlock(HGLOBAL(h_mem.0 as *mut _));
                }
            }
        }
    }

    let _ = CloseClipboard();
    result
}

#[cfg(target_os = "windows")]
pub unsafe fn set_clipboard_text(text: &str, exclude_from_history: bool) {
    use windows::Win32::System::DataExchange::{OpenClipboard, EmptyClipboard, SetClipboardData, CloseClipboard, RegisterClipboardFormatW};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::core::w;

    let _ = OpenClipboard(None);
    let _ = EmptyClipboard();

    // 1. 设置文本内容
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let size_bytes = utf16.len() * std::mem::size_of::<u16>();
    if let Ok(hglobal) = GlobalAlloc(GMEM_MOVEABLE, size_bytes) {
        let ptr = GlobalLock(hglobal);
        let ptr_u16 = ptr as *mut u16;
        if !ptr_u16.is_null() {
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr_u16, utf16.len());
        }
        let _ = GlobalUnlock(hglobal);
        let _ = SetClipboardData(13, HANDLE(hglobal.0 as isize)); // CF_UNICODETEXT = 13
    }

    // 2. 如果需要，排除在历史记录之外 (Win+V)
    if exclude_from_history {
        let format_name = w!("CanIncludeInClipboardHistory");
        let format_id = RegisterClipboardFormatW(format_name);
        if format_id != 0 {
            if let Ok(h_meta) = GlobalAlloc(GMEM_MOVEABLE, 4) {
                let ptr = GlobalLock(h_meta);
                if !ptr.is_null() {
                    *(ptr as *mut u32) = 0; // 0 = 不包含在历史记录中
                }
                let _ = GlobalUnlock(h_meta);
                let _ = SetClipboardData(format_id, HANDLE(h_meta.0 as isize));
            }
        }
        
        // 同时排除在云剪贴板同步之外
        let format_sync = w!("CanUploadToCloudClipboard");
        let format_sync_id = RegisterClipboardFormatW(format_sync);
        if format_sync_id != 0 {
            if let Ok(h_meta) = GlobalAlloc(GMEM_MOVEABLE, 4) {
                let ptr = GlobalLock(h_meta);
                if !ptr.is_null() {
                    *(ptr as *mut u32) = 0; // 0 = 不同步
                }
                let _ = GlobalUnlock(h_meta);
                let _ = SetClipboardData(format_sync_id, HANDLE(h_meta.0 as isize));
            }
        }
    }

    let _ = CloseClipboard();
}

#[cfg(target_os = "windows")]
pub unsafe fn paste_clipboard() {
    let vk_ctrl = VIRTUAL_KEY(0x11);
    let vk_v = VIRTUAL_KEY(0x56);
    let down_ctrl = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk_ctrl, wScan: 0, dwFlags: KEYBD_EVENT_FLAGS(0), time: 0, dwExtraInfo: 0 } } };
    let down_v = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk_v, wScan: 0, dwFlags: KEYBD_EVENT_FLAGS(0), time: 0, dwExtraInfo: 0 } } };
    let up_v = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk_v, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 } } };
    let up_ctrl = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk_ctrl, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 } } };
    let inputs = [down_ctrl, down_v, up_v, up_ctrl];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

#[cfg(target_os = "windows")]
pub unsafe fn show_console_with_redirect() {
    use windows::Win32::System::Console::{AllocConsole, GetConsoleWindow};
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOW};
    use std::ffi::CString;

    // 1. Check if console already exists
    let hwnd = GetConsoleWindow();
    if hwnd.0 != 0 {
        ShowWindow(hwnd, SW_SHOW);
        return;
    }

    // 2. Allocate new console
    let _ = AllocConsole();

    // 3. Redirect stdout/stderr (CRITICAL for println!)
    // "CONOUT$" is the special filename for the console output buffer
    let conout = CString::new("CONOUT$").unwrap();
    let mode = CString::new("w").unwrap();

    #[cfg(target_os = "windows")]
    unsafe {
        // Only valid on MSVC toolchain, GNU might use different symbols
        extern "C" {
            #[link_name = "__acrt_iob_func"]
            fn __acrt_iob_func(idx: u32) -> *mut libc::FILE;
        }

        // stdout = 1, stderr = 2
        let stdout_ptr = __acrt_iob_func(1);
        let stderr_ptr = __acrt_iob_func(2);

        libc::freopen(conout.as_ptr(), mode.as_ptr(), stdout_ptr);
        libc::freopen(conout.as_ptr(), mode.as_ptr(), stderr_ptr);
    }
    
    // Optional: Update Rust's own buffering if needed, but usually freopen is enough
    println!("Console allocated and stdout redirected successfully!");
}

#[cfg(target_os = "windows")]
pub fn is_autostart_enabled() -> bool {
    use windows::Win32::System::Registry::{RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, HKEY, KEY_READ, REG_VALUE_TYPE, REG_SZ};
    use windows::core::PCWSTR;
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0".encode_utf16().collect();
    unsafe {
        let mut hkey: HKEY = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), 0, KEY_READ, &mut hkey).is_err() {
            return false;
        }
        let name: Vec<u16> = "Voice2Type\0".encode_utf16().collect();
        let mut data_len: u32 = 0;
        let mut ty = REG_VALUE_TYPE(0);
        // First call to get required length
        let _ = RegQueryValueExW(hkey, PCWSTR(name.as_ptr()), None, Some(&mut ty), None, Some(&mut data_len));
        if ty != REG_SZ || data_len == 0 {
            return false;
        }
        
        // Second call to get actual data
        let mut data = vec![0u8; data_len as usize];
        if RegQueryValueExW(hkey, PCWSTR(name.as_ptr()), None, None, Some(data.as_mut_ptr()), Some(&mut data_len)).is_err() {
            return false;
        }
        
        // Convert data to string
        let data_u16 = std::slice::from_raw_parts(data.as_ptr() as *const u16, data_len as usize / 2);
        let reg_path = String::from_utf16_lossy(data_u16).trim_matches('"').to_string();
        
        // Get current executable path
        if let Ok(current_exe) = std::env::current_exe() {
            let current_path = current_exe.display().to_string();
            // Compare paths
            return reg_path == current_path;
        }
        
        false
    }
}

#[cfg(target_os = "windows")]
pub unsafe fn set_autostart(enabled: bool) -> Result<()> {
    use windows::Win32::System::Registry::{
        RegCreateKeyExW, RegSetValueExW, RegDeleteValueW, HKEY_CURRENT_USER, HKEY, REG_OPTION_NON_VOLATILE,
        KEY_SET_VALUE, KEY_WRITE, REG_SZ
    };
    use windows::core::PCWSTR;
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0".encode_utf16().collect();
    let mut hkey: HKEY = HKEY::default();
    let rc = RegCreateKeyExW(
        HKEY_CURRENT_USER,
        PCWSTR(subkey.as_ptr()),
        0,
        None,
        REG_OPTION_NON_VOLATILE,
        KEY_SET_VALUE | KEY_WRITE,
        None,
        &mut hkey,
        None
    );
    if rc.is_err() {
        anyhow::bail!("RegCreateKeyExW failed");
    }

    let name: Vec<u16> = "Voice2Type\0".encode_utf16().collect();
    if enabled {
        if let Ok(exe) = std::env::current_exe() {
            let quoted = format!("\"{}\"", exe.display());
            let data: Vec<u16> = quoted.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes: &[u8] = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
            let rv = RegSetValueExW(hkey, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes));
            if rv.is_err() {
                anyhow::bail!("RegSetValueExW failed");
            }
        }
    } else {
        let rd = RegDeleteValueW(hkey, PCWSTR(name.as_ptr()));
        if rd.is_err() {
            anyhow::bail!("RegDeleteValueW failed");
        }
    }
    Ok(())
}
