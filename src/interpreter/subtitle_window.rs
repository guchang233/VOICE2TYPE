use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use windows::Win32::Foundation::{HWND, LRESULT, POINT, RECT, WPARAM, LPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteDC, DeleteObject, GetDC, GetSystemMetrics,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, TEXTMETRICW,
    DrawTextW, DT_CENTER, DT_NOCLIP, DT_NOPREFIX, DT_WORDBREAK,
    SM_CXSCREEN, SM_CYSCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, GetWindowRect, InvalidateRect, LoadCursorW,
    RegisterClassW, SetLayeredWindowAttributes, SetWindowPos,
    ShowWindow, TranslateMessage, UpdateWindow, CS_HREDRAW,
    CS_VREDRAW, CW_USEDEFAULT, IDC_ARROW, MSG, SW_HIDE, SW_SHOW,
    ULW_ALPHA, VA_NOTIFY, WM_CREATE, WM_DESTROY, WM_PAINT,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    HWND_TOPMOST, SWP_NOSIZE, SWP_NOMOVE,
};

pub struct SubtitleWindowConfig {
    pub font_size: i32,
    pub opacity: f32,
    pub position: String,
    pub click_through: bool,
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
            let class_name: Vec<u16> = "Voice2TypeSubtitle".encode_utf16().chain(std::iter::once(0)).collect();

            let wnd_class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(Self::window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap(),
                hIcon: None,
                hCursor: LoadCursorW(None, IDC_ARROW),
                hbrBackground: None,
                lpszMenuName: None,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                hIconSm: None,
            };

            RegisterClassExW(&wnd_class);

            let screen_width = GetSystemMetrics(SM_CXSCREEN);
            let screen_height = GetSystemMetrics(SM_CYSCREEN);
            let window_width = (screen_width * 3 / 4) as i32;
            let window_height = 150;

            let (x, y) = match config.position.as_str() {
                "top" => ((screen_width - window_width as u32) / 2) as i32,
                "bottom" => ((screen_width - window_width as u32) / 2) as i32,
                _ => ((screen_width - window_width as u32) / 2) as i32,
            };

            let (_, y) = match config.position.as_str() {
                "top" => (x, 50),
                "bottom" => (x, screen_height as i32 - 200),
                _ => (x, screen_height as i32 - 200),
            };

            let mut ex_style = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
            if config.click_through {
                ex_style |= WS_EX_TRANSPARENT;
            }

            let hwnd = CreateWindowExW(
                ex_style,
                PCWSTR(class_name.as_ptr()),
                PCWSTR("".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>().as_ptr()),
                WS_POPUP,
                x, y, window_width, window_height,
                None, None, None, None,
            );

            SetLayeredWindowAttributes(
                hwnd,
                0,
                (config.opacity * 255.0) as u8,
                ULW_ALPHA,
            );

            let user_data = Box::new((lines, config));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(user_data) as i32);

            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
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
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as i32);
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

        let brush = CreateSolidBrush(windows::Win32::Graphics::Gdi::RGB(20, 20, 20));
        FillRect(hdc, &client_rect, brush);
        DeleteObject(brush);

        if lines.is_empty() {
            ReleaseDC(hwnd, hdc);
            return;
        }

        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, windows::Win32::Graphics::Gdi::RGB(255, 255, 255));

        let font = CreateFontW(
            config.font_size,
            0, 0, 0,
            FW_NORMAL,
            false, false, false,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            DEFAULT_PITCH | FF_DONTCARE,
            PCWSTR("Microsoft YaHei".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>().as_ptr()),
        );

        let old_font = SelectObject(hdc, font);

        let line_height = config.font_size + 10;
        let total_height = lines.len() as i32 * line_height;
        let start_y = (client_rect.bottom - total_height) / 2;

        for (i, line) in lines.iter().enumerate() {
            let text: Vec<u16> = line.text.encode_utf16().chain(std::iter::once(0)).collect();

            let alpha = line.alpha;
            SetTextColor(hdc, windows::Win32::Graphics::Gdi::RGB(
                (255.0 * alpha) as u8,
                (255.0 * alpha) as u8,
                (255.0 * alpha) as u8,
            ));

            let mut line_rect = RECT {
                left: 20,
                top: start_y + i as i32 * line_height,
                right: client_rect.right - 20,
                bottom: start_y + (i + 1) as i32 * line_height,
            };

            DrawTextW(
                hdc,
                PCWSTR(text.as_ptr()),
                -1,
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

use windows::Win32::UI::WindowsAndMessaging::{
    CreateFontW, FillRect, GWLP_USERDATA, PCWSTR, PostQuitMessage,
    SetWindowLongPtrW, GetWindowLongPtrW,
};
use windows::Win32::Graphics::Gdi::{
    FW_NORMAL, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
    DEFAULT_QUALITY, DEFAULT_PITCH, FF_DONTCARE, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW;
use windows::Win32::Foundation::LONG;