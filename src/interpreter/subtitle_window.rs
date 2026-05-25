use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, FillRect, GetDC,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, InvalidateRect, UpdateWindow,
    FW_NORMAL, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
    DEFAULT_QUALITY, DEFAULT_PITCH, FF_DONTCARE, TRANSPARENT,
    DrawTextW, DT_CENTER, DT_WORDBREAK, DT_NOCLIP, DT_NOPREFIX,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, LoadCursorW, RegisterClassExW, SetLayeredWindowAttributes,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    IDC_ARROW, MSG, SW_HIDE, SW_SHOW, WM_CREATE, WM_DESTROY, WM_PAINT,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    CREATESTRUCTW, GWLP_USERDATA, PostQuitMessage,
    SetWindowLongPtrW, GetWindowLongPtrW, WNDCLASSEXW,
    GetWindowRect, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
};
use windows::Win32::Foundation::COLORREF;
use windows::core::PCWSTR;

pub struct SubtitleWindowConfig {
    pub font_size: i32,
    pub opacity: f32,
    pub position: String,
    pub click_through: bool,
}

impl Clone for SubtitleWindowConfig {
    fn clone(&self) -> Self {
        Self {
            font_size: self.font_size,
            opacity: self.opacity,
            position: self.position.clone(),
            click_through: self.click_through,
        }
    }
}

impl Default for SubtitleWindowConfig {
    fn default() -> Self {
        Self {
            font_size: 22,
            opacity: 0.85,
            position: "bottom".to_string(),
            click_through: true,
        }
    }
}

struct SubtitleLine {
    text: String,
    age_ms: u64,
    alpha: f32,
}

pub struct SubtitleWindow {
    hwnd: HWND,
    lines: Arc<Mutex<Vec<SubtitleLine>>>,
    config: SubtitleWindowConfig,
    _thread: Option<thread::JoinHandle<()>>,
}

impl SubtitleWindow {
    pub fn new(config: SubtitleWindowConfig) -> Self {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();
        let config_clone = config.clone();

        let thread = thread::spawn(move || {
            Self::window_thread(lines_clone, config_clone);
        });

        let mut hwnd = HWND(0);
        for _ in 0..10 {
            thread::sleep(std::time::Duration::from_millis(50));
            if let Ok(handle) = get_hwnd_from_lines(&lines) {
                hwnd = handle;
                break;
            }
        }

        Self {
            hwnd,
            lines,
            config,
            _thread: Some(thread),
        }
    }

    pub fn update_subtitle(&self, text: &str) {
        if text.trim().is_empty() {
            return;
        }

        let mut lines = self.lines.lock().unwrap();
        let new_line = SubtitleLine {
            text: text.to_string(),
            age_ms: 0,
            alpha: 1.0,
        };

        lines.push(new_line);
        if lines.len() > 3 {
            lines.remove(0);
        }

        self.trigger_redraw();
    }

    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
        }
    }

    pub fn hide(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn trigger_redraw(&self) {
        unsafe {
            InvalidateRect(self.hwnd, None, true);
        }
    }

    fn window_thread(lines: Arc<Mutex<Vec<SubtitleLine>>>, config: SubtitleWindowConfig) {
        unsafe {
            let class_name: Vec<u16> = "Voice2TypeSubtitle\0".encode_utf16().collect();

            let wnd_class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(Self::window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap().into(),
                hIcon: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                hIconSm: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
            };

            RegisterClassExW(&wnd_class);

            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let screen_height = GetSystemMetrics(SM_CYSCREEN);
            let window_width = ((screen_width as i32) * 3 / 4);
            let window_height = 150;

            let (x, y) = match config.position.as_str() {
                "top" => {
                    let x = ((screen_width as i32 - window_width) / 2);
                    (x, 50)
                }
                "bottom" | _ => {
                    let x = ((screen_width as i32 - window_width) / 2);
                    (x, screen_height as i32 - 200)
                }
            };

            let mut ex_style = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
            if config.click_through {
                ex_style |= WS_EX_TRANSPARENT;
            }

            let hwnd = CreateWindowExW(
                ex_style,
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                x, y, window_width, window_height,
                None, None, None, None,
            );

            let _ = SetLayeredWindowAttributes(
                hwnd,
                COLORREF(0),
                (config.opacity * 255.0) as u8,
                windows::Win32::UI::WindowsAndMessaging::LAYERED_WINDOW_ATTRIBUTES_FLAGS(2),
            );

            let user_data = Box::new((lines, config));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(user_data) as isize);

            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);

            let mut msg = MSG::default();
            loop {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 == 0 {
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_CREATE => {
                    let create_struct = &*(lparam.0 as *const CREATESTRUCTW);
                    let user_data = create_struct.lpCreateParams as *mut (Arc<Mutex<Vec<SubtitleLine>>>, SubtitleWindowConfig);
                    if !user_data.is_null() {
                        let data = Box::from_raw(user_data);
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
                    }
                    LRESULT(0)
                }
                WM_PAINT => {
                    Self::paint(hwnd);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                    if ptr != 0 {
                        let _ = Box::from_raw(ptr as *mut (Arc<Mutex<Vec<SubtitleLine>>>, SubtitleWindowConfig));
                    }
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    unsafe fn paint(hwnd: HWND) {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 {
            return;
        }

        let data = &*(ptr as *const (Arc<Mutex<Vec<SubtitleLine>>>, SubtitleWindowConfig));
        let lines = data.0.lock().unwrap();
        let config = &data.1;

        let hdc = GetDC(hwnd);
        if hdc.is_invalid() {
            return;
        }

        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect);
        let client_rect = RECT {
            left: 0,
            top: 0,
            right: rect.right - rect.left,
            bottom: rect.bottom - rect.top,
        };

        let brush = CreateSolidBrush(COLORREF(0x141414));
        FillRect(hdc, &client_rect, brush);
        DeleteObject(brush);

        if lines.is_empty() {
            ReleaseDC(hwnd, hdc);
            return;
        }

        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(0xFFFFFF));

        let font_name: Vec<u16> = "Microsoft YaHei\0".encode_utf16().collect();
        let font = CreateFontW(
            config.font_size,
            0, 0, 0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            DEFAULT_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 as u32) | (FF_DONTCARE.0 as u32),
            PCWSTR(font_name.as_ptr()),
        );

        let old_font = SelectObject(hdc, font);

        let line_height = config.font_size + 10;
        let total_height = lines.len() as i32 * line_height;
        let start_y = (client_rect.bottom - total_height) / 2;

        for (i, line) in lines.iter().enumerate() {
            let mut text: Vec<u16> = line.text.encode_utf16().chain(std::iter::once(0)).collect();

            let mut line_rect = RECT {
                left: 20,
                top: start_y + i as i32 * line_height,
                right: client_rect.right - 20,
                bottom: start_y + (i + 1) as i32 * line_height,
            };

            DrawTextW(
                hdc,
                &mut text,
                &mut line_rect,
                DT_CENTER | DT_WORDBREAK | DT_NOCLIP | DT_NOPREFIX,
            );
        }

        SelectObject(hdc, old_font);
        DeleteObject(font);
        ReleaseDC(hwnd, hdc);
    }
}

impl Drop for SubtitleWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

fn get_hwnd_from_lines(_lines: &Arc<Mutex<Vec<SubtitleLine>>>) -> Result<HWND, ()> {
    Err(())
}
