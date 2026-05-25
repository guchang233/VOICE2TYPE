use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::Foundation::*,
    Win32::Graphics::Gdi::*,
    Win32::System::LibraryLoader::GetModuleHandleW,
    Win32::UI::WindowsAndMessaging::*,
};

#[derive(Debug, Clone)]
pub struct SubtitleLine {
    pub text: String,
    pub timestamp: Instant,
}

#[derive(Clone)]
pub struct SubtitleWindow {
    tx: Sender<SubtitleMessage>,
}

pub enum SubtitleMessage {
    Show(String),
    Hide,
    SetClickThrough(bool),
    SetOpacity(f32),
    SetPosition(String),
    SetFontSize(u32),
    Shutdown,
}

impl SubtitleWindow {
    pub fn new(click_through: bool, opacity: f32, position: String, font_size: u32) -> Self {
        let (tx, rx) = channel();

        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            unsafe {
                run_subtitle_window(rx, click_through, opacity, position, font_size);
            }
        });

        Self { tx }
    }

    pub fn show(&self, text: String) {
        let _ = self.tx.send(SubtitleMessage::Show(text));
    }

    pub fn hide(&self) {
        let _ = self.tx.send(SubtitleMessage::Hide);
    }

    pub fn set_click_through(&self, enabled: bool) {
        let _ = self.tx.send(SubtitleMessage::SetClickThrough(enabled));
    }

    pub fn set_opacity(&self, opacity: f32) {
        let _ = self.tx.send(SubtitleMessage::SetOpacity(opacity));
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(SubtitleMessage::Shutdown);
    }
}

#[cfg(target_os = "windows")]
struct SubtitleState {
    lines: Vec<SubtitleLine>,
    visible: bool,
    click_through: bool,
    opacity: f32,
    position: String,
    font_size: u32,
    last_update: Instant,
    hide_after: Duration,
}

#[cfg(target_os = "windows")]
unsafe fn run_subtitle_window(
    rx: Receiver<SubtitleMessage>,
    click_through: bool,
    opacity: f32,
    position: String,
    font_size: u32,
) {
    let instance = GetModuleHandleW(None).unwrap();
    let class_name = w!("Voice2TypeSubtitleClass");

    let wc = WNDCLASSW {
        lpfnWndProc: Some(subtitle_wnd_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };

    RegisterClassW(&wc);

    let mut ex_style = WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW;
    if click_through {
        ex_style |= WS_EX_TRANSPARENT;
    }

    let screen_width = GetSystemMetrics(SM_CXSCREEN);
    let screen_height = GetSystemMetrics(SM_CYSCREEN);
    let initial_width = 800;
    let initial_height = 120;

    let x = (screen_width - initial_width) / 2;
    let y = if position == "top" { 60 } else { screen_height - initial_height - 80 };

    let hwnd = CreateWindowExW(
        ex_style,
        class_name,
        w!("Voice2Type Subtitle"),
        WS_POPUP,
        x,
        y,
        initial_width,
        initial_height,
        None,
        None,
        instance,
        None,
    );

    let mut state = SubtitleState {
        lines: Vec::new(),
        visible: false,
        click_through,
        opacity,
        position,
        font_size,
        last_update: Instant::now(),
        hide_after: Duration::from_secs(8),
    };

    let mut msg = MSG::default();
    loop {
        if let Ok(message) = rx.try_recv() {
            match message {
                SubtitleMessage::Show(text) => {
                    if !text.is_empty() {
                        state.lines.push(SubtitleLine {
                            text,
                            timestamp: Instant::now(),
                        });
                        if state.lines.len() > 3 {
                            state.lines.remove(0);
                        }
                        state.visible = true;
                        state.last_update = Instant::now();
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::Hide => {
                    state.visible = false;
                    state.lines.clear();
                    ShowWindow(hwnd, SW_HIDE);
                }
                SubtitleMessage::SetClickThrough(enabled) => {
                    state.click_through = enabled;
                    let mut new_ex_style = WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW;
                    if enabled {
                        new_ex_style |= WS_EX_TRANSPARENT;
                    }
                    SetWindowLongW(hwnd, GWL_EXSTYLE, new_ex_style.0 as i32);
                }
                SubtitleMessage::SetOpacity(op) => {
                    state.opacity = op.clamp(0.1, 1.0);
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetPosition(pos) => {
                    state.position = pos;
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetFontSize(size) => {
                    state.font_size = size;
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::Shutdown => {
                    ShowWindow(hwnd, SW_HIDE);
                    let _ = DestroyWindow(hwnd);
                    return;
                }
            }
        }

        if state.visible && state.last_update.elapsed() > state.hide_after {
            state.visible = false;
            state.lines.clear();
            ShowWindow(hwnd, SW_HIDE);
        }

        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_DESTROY {
                return;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn subtitle_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(target_os = "windows")]
unsafe fn draw_subtitle(hwnd: HWND, state: &SubtitleState) {
    if state.lines.is_empty() || !state.visible {
        ShowWindow(hwnd, SW_HIDE);
        return;
    }

    let screen_width = GetSystemMetrics(SM_CXSCREEN);
    let screen_height = GetSystemMetrics(SM_CYSCREEN);

    let padding = 20i32;
    let line_height = (state.font_size as f32 * 1.6) as i32;
    let num_lines = state.lines.len() as i32;
    let w = 800i32;
    let h = padding * 2 + line_height * num_lines + 10;

    let x = (screen_width - w) / 2;
    let y = if state.position == "top" {
        60
    } else {
        screen_height - h - 80
    };

    let _ = SetWindowPos(hwnd, HWND(0), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);

    let hdc_screen = GetDC(None);
    let hdc_mem = CreateCompatibleDC(hdc_screen);

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut p_bits: *mut c_void = std::ptr::null_mut();
    let hbitmap = CreateDIBSection(hdc_mem, &bmi, DIB_RGB_COLORS, &mut p_bits, None, 0).unwrap();
    let old_bitmap = SelectObject(hdc_mem, hbitmap);

    let pixels = std::slice::from_raw_parts_mut(p_bits as *mut u32, (w * h) as usize);

    for p in pixels.iter_mut() {
        *p = 0;
    }

    let radius = 12.0f32;
    let bg_r = 15u32;
    let bg_g = 15u32;
    let bg_b = 20u32;
    let bg_alpha = (state.opacity * 230.0) as u32;

    for y_px in 0..h {
        for x_px in 0..w {
            let idx = (y_px * w + x_px) as usize;

            let dx = if (x_px as f32) < radius {
                radius - x_px as f32
            } else if (x_px as f32) > (w as f32 - radius) {
                x_px as f32 - (w as f32 - radius)
            } else {
                0.0f32
            };

            let dy = if (y_px as f32) < radius {
                radius - y_px as f32
            } else if (y_px as f32) > (h as f32 - radius) {
                y_px as f32 - (h as f32 - radius)
            } else {
                0.0f32
            };

            let dist = (dx * dx + dy * dy).sqrt();

            let alpha_f = if dist <= radius {
                1.0f32
            } else if dist < radius + 2.0 {
                (radius + 2.0 - dist) / 2.0
            } else {
                0.0f32
            };

            if alpha_f > 0.0 {
                let pixel_alpha = (alpha_f * bg_alpha as f32) as u32;
                let premult_r = (bg_r * pixel_alpha) / 255;
                let premult_g = (bg_g * pixel_alpha) / 255;
                let premult_b = (bg_b * pixel_alpha) / 255;
                pixels[idx] = (pixel_alpha << 24) | (premult_r << 16) | (premult_g << 8) | premult_b;
            }
        }
    }

    SetBkMode(hdc_mem, TRANSPARENT);

    let font_height = state.font_size as i32;
    let font = CreateFontW(
        -font_height,
        0,
        0,
        0,
        FW_MEDIUM.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        w!("Microsoft YaHei"),
    );
    let old_font = SelectObject(hdc_mem, font);

    let now = Instant::now();
    for (i, line) in state.lines.iter().enumerate() {
        let age = now.duration_since(line.timestamp).as_secs_f32();
        let fade_factor = if i == state.lines.len() - 1 {
            1.0f32
        } else {
            0.5f32.max(1.0 - age * 0.1)
        };

        let _text_alpha = (255.0 * fade_factor) as u8;
        let text_color = if i == state.lines.len() - 1 {
            COLORREF(0x00FFFFFF)
        } else {
            COLORREF(0x00AAA0AA)
        };

        SetTextColor(hdc_mem, text_color);

        let y_pos = padding + i as i32 * line_height;
        let mut text_rect = RECT {
            left: padding + 10,
            top: y_pos,
            right: w - padding - 10,
            bottom: y_pos + line_height,
        };

        let mut text_wide: Vec<u16> = line.text.encode_utf16().chain(Some(0)).collect();
        DrawTextW(
            hdc_mem,
            &mut text_wide,
            &mut text_rect,
            DT_VCENTER | DT_SINGLELINE | DT_WORD_ELLIPSIS,
        );

        let text_left = text_rect.left.max(0);
        let text_right = text_rect.right.min(w);
        let text_top = text_rect.top.max(0);
        let text_bottom = text_rect.bottom.min(h);

        for ty in text_top..text_bottom {
            let row_offset = ty * w;
            for tx in text_left..text_right {
                let idx = (row_offset + tx) as usize;
                let val = pixels[idx];
                let r = (val >> 16) & 0xFF;
                let g = (val >> 8) & 0xFF;
                let b = val & 0xFF;
                let max_c = r.max(g).max(b);
                if max_c > 0 {
                    let current_a = (val >> 24) & 0xFF;
                    let target_a = current_a.max(max_c);
                    pixels[idx] = (target_a << 24) | (val & 0x00FFFFFF);
                }
            }
        }
    }

    SelectObject(hdc_mem, old_font);
    DeleteObject(font);

    let pt_src = POINT { x: 0, y: 0 };
    let size = SIZE { cx: w, cy: h };

    let global_alpha = (state.opacity * 255.0) as u8;

    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: global_alpha,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };

    let _ = UpdateLayeredWindow(
        hwnd,
        hdc_screen,
        None,
        Some(&size),
        hdc_mem,
        Some(&pt_src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );

    ShowWindow(hwnd, SW_SHOWNOACTIVATE);

    SelectObject(hdc_mem, old_bitmap);
    DeleteObject(hbitmap);
    DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
}

#[cfg(not(target_os = "windows"))]
impl SubtitleWindow {
    pub fn new(_click_through: bool, _opacity: f32, _position: String, _font_size: u32) -> Self {
        let (tx, _) = channel();
        Self { tx }
    }
}
