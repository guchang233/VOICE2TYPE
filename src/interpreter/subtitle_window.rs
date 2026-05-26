use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
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

pub static SETTINGS_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
thread_local! {
    static SUBTITLE_TX: RefCell<Option<Sender<SubtitleMessage>>> = RefCell::new(None);
    static SUBTITLE_LOCKED: Cell<bool> = Cell::new(false);
}

#[derive(Debug, Clone)]
pub struct SubtitleLine {
    pub original: String,
    pub translated: Option<String>,
}

#[derive(Clone)]
pub struct SubtitleWindow {
    tx: Sender<SubtitleMessage>,
}

pub enum SubtitleMessage {
    ShowBilingual { original: String, translated: Option<String> },
    Hide,
    SetClickThrough(bool),
    SetOpacity(f32),
    SetPosition(String),
    SetFontSize(u32),
    SetBilingualMode(bool),
    SetOriginalFontSize(u32),
    SetOriginalColor(String),
    SetTranslatedFontSize(u32),
    SetTranslatedColor(String),
    ToggleLock,
    OpenSettings,
    SetLocked(bool),
    Shutdown,
}

impl SubtitleWindow {
    pub fn new(click_through: bool, opacity: f32, position: String, font_size: u32) -> Self {
        let (tx, rx) = channel();
        let tx_for_wnd = tx.clone();

        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            unsafe {
                run_subtitle_window(rx, tx_for_wnd, click_through, opacity, position, font_size);
            }
        });

        Self { tx }
    }

    pub fn show_bilingual(&self, original: String, translated: Option<String>) {
        let _ = self.tx.send(SubtitleMessage::ShowBilingual { original, translated });
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

    pub fn set_bilingual_mode(&self, mode: bool) {
        let _ = self.tx.send(SubtitleMessage::SetBilingualMode(mode));
    }

    pub fn set_original_font_size(&self, size: u32) {
        let _ = self.tx.send(SubtitleMessage::SetOriginalFontSize(size));
    }

    pub fn set_original_color(&self, color: String) {
        let _ = self.tx.send(SubtitleMessage::SetOriginalColor(color));
    }

    pub fn set_translated_font_size(&self, size: u32) {
        let _ = self.tx.send(SubtitleMessage::SetTranslatedFontSize(size));
    }

    pub fn set_translated_color(&self, color: String) {
        let _ = self.tx.send(SubtitleMessage::SetTranslatedColor(color));
    }

    pub fn set_font_size(&self, size: u32) {
        let _ = self.tx.send(SubtitleMessage::SetFontSize(size));
    }

    pub fn set_position(&self, pos: String) {
        let _ = self.tx.send(SubtitleMessage::SetPosition(pos));
    }

    pub fn set_locked(&self, locked: bool) {
        let _ = self.tx.send(SubtitleMessage::SetLocked(locked));
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
    bilingual_mode: bool,
    original_font_size: u32,
    original_color: String,
    translated_font_size: u32,
    translated_color: String,
    locked: bool,
    last_update: Instant,
    hide_after: Duration,
}

fn parse_hex_color(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255);
        (r, g, b)
    } else {
        (255, 255, 255)
    }
}

#[cfg(target_os = "windows")]
unsafe fn run_subtitle_window(
    rx: Receiver<SubtitleMessage>,
    tx_for_wnd: Sender<SubtitleMessage>,
    click_through: bool,
    opacity: f32,
    position: String,
    font_size: u32,
) {
    SUBTITLE_TX.with(|tl| *tl.borrow_mut() = Some(tx_for_wnd));
    SUBTITLE_LOCKED.with(|tl| tl.set(false));

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
    let initial_width = (screen_width * 4 / 5).min(1000);
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
        bilingual_mode: false,
        original_font_size: 18,
        original_color: "#AAAAAA".to_string(),
        translated_font_size: 24,
        translated_color: "#FFFFFF".to_string(),
        locked: false,
        last_update: Instant::now(),
        hide_after: Duration::from_secs(8),
    };

    let mut msg = MSG::default();
    loop {
        if let Ok(message) = rx.try_recv() {
            match message {
                SubtitleMessage::ShowBilingual { original, translated } => {
                    if !original.is_empty() {
                        state.lines.push(SubtitleLine {
                            original,
                            translated,
                        });
                        let max_lines = if state.bilingual_mode { 2 } else { 3 };
                        while state.lines.len() > max_lines {
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
                SubtitleMessage::SetBilingualMode(mode) => {
                    state.bilingual_mode = mode;
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetOriginalFontSize(size) => {
                    state.original_font_size = size;
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetOriginalColor(color) => {
                    state.original_color = color;
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetTranslatedFontSize(size) => {
                    state.translated_font_size = size;
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetTranslatedColor(color) => {
                    state.translated_color = color;
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::ToggleLock => {
                    state.locked = !state.locked;
                    SUBTITLE_LOCKED.with(|tl| tl.set(state.locked));
                    let mut new_ex_style = WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW;
                    if state.locked || state.click_through {
                        new_ex_style |= WS_EX_TRANSPARENT;
                    }
                    SetWindowLongW(hwnd, GWL_EXSTYLE, new_ex_style.0 as i32);
                    if state.visible {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::OpenSettings => {
                    SETTINGS_REQUESTED.store(true, Ordering::SeqCst);
                }
                SubtitleMessage::SetLocked(locked) => {
                    state.locked = locked;
                    SUBTITLE_LOCKED.with(|tl| tl.set(locked));
                    let mut new_ex_style = WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW;
                    if state.locked || state.click_through {
                        new_ex_style |= WS_EX_TRANSPARENT;
                    }
                    SetWindowLongW(hwnd, GWL_EXSTYLE, new_ex_style.0 as i32);
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
        WM_NCHITTEST => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);

            let local_x = x - rect.left;
            let local_y = y - rect.top;
            let w = rect.right - rect.left;

            let btn_size = 24i32;
            let btn_margin = 8i32;

            let lock_x1 = w - btn_margin - btn_size;
            let lock_x2 = w - btn_margin;
            let lock_y1 = btn_margin;
            let lock_y2 = btn_margin + btn_size;

            let settings_x1 = w - btn_margin - btn_size * 2 - 4;
            let settings_x2 = w - btn_margin - btn_size - 4;

            let in_button = (local_x >= lock_x1 && local_x <= lock_x2 && local_y >= lock_y1 && local_y <= lock_y2)
                || (local_x >= settings_x1 && local_x <= settings_x2 && local_y >= lock_y1 && local_y <= lock_y2);

            if in_button {
                LRESULT(HTCLIENT as isize)
            } else if SUBTITLE_LOCKED.get() {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTCAPTION as isize)
            }
        }
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let w = rect.right;

            let btn_size = 24i32;
            let btn_margin = 8i32;

            let lock_x1 = w - btn_margin - btn_size;
            let lock_x2 = w - btn_margin;
            let lock_y1 = btn_margin;
            let lock_y2 = btn_margin + btn_size;

            let settings_x1 = w - btn_margin - btn_size * 2 - 4;
            let settings_x2 = w - btn_margin - btn_size - 4;
            let settings_y1 = btn_margin;
            let settings_y2 = btn_margin + btn_size;

            SUBTITLE_TX.with(|tx| {
                if let Some(sender) = tx.borrow().as_ref() {
                    if x >= lock_x1 && x <= lock_x2 && y >= lock_y1 && y <= lock_y2 {
                        let _ = sender.send(SubtitleMessage::ToggleLock);
                    } else if x >= settings_x1 && x <= settings_x2 && y >= settings_y1 && y <= settings_y2 {
                        let _ = sender.send(SubtitleMessage::OpenSettings);
                    }
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(target_os = "windows")]
fn draw_button_bg(pixels: &mut [u32], w: i32, h: i32, x: i32, y: i32, size: i32, alpha: u32) {
    for by in y..(y + size) {
        for bx in x..(x + size) {
            if bx >= 0 && bx < w && by >= 0 && by < h {
                let idx = (by * w + bx) as usize;
                let premult = alpha * 40 / 255;
                pixels[idx] = (alpha << 24) | (premult << 16) | (premult << 8) | premult;
            }
        }
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
    let w = (screen_width * 4 / 5).min(1000);

    let mut row_specs: Vec<(i32, String, u32, (u8, u8, u8))> = Vec::new();

    for line in &state.lines {
        if state.bilingual_mode && line.translated.is_some() {
            let (tr, tg, tb) = parse_hex_color(&state.translated_color);
            row_specs.push(((state.translated_font_size as f32 * 1.5) as i32, line.translated.as_ref().unwrap().clone(), state.translated_font_size, (tr, tg, tb)));
            let (or, og, ob) = parse_hex_color(&state.original_color);
            row_specs.push(((state.original_font_size as f32 * 1.4) as i32, line.original.clone(), state.original_font_size, (or, og, ob)));
        } else {
            let display_text = line.translated.as_deref().unwrap_or(&line.original);
            let fs = if line.translated.is_some() { state.translated_font_size } else { state.font_size };
            let (cr, cg, cb) = if line.translated.is_some() {
                parse_hex_color(&state.translated_color)
            } else {
                (255, 255, 255)
            };
            row_specs.push(((fs as f32 * 1.6) as i32, display_text.to_string(), fs, (cr, cg, cb)));
        }
    }

    let total_height: i32 = row_specs.iter().map(|(h, _, _, _)| *h).sum();
    let h = padding * 2 + total_height + 10;

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

    let hdc_text = CreateCompatibleDC(hdc_screen);
    let mut p_text_bits: *mut c_void = std::ptr::null_mut();
    let text_bmp = CreateDIBSection(hdc_text, &bmi, DIB_RGB_COLORS, &mut p_text_bits, None, 0).unwrap();
    let old_text_bmp = SelectObject(hdc_text, text_bmp);

    SetBkMode(hdc_text, TRANSPARENT);

    let mut current_y = padding;

    for (row_h, text, font_size, (cr, cg, cb)) in &row_specs {
        let text_pixels = std::slice::from_raw_parts_mut(p_text_bits as *mut u32, (w * h) as usize);
        for p in text_pixels.iter_mut() {
            *p = 0xFF000000;
        }

        let font = CreateFontW(
            -(*font_size as i32),
            0, 0, 0,
            FW_MEDIUM.0 as i32,
            0, 0, 0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            DEFAULT_PITCH.0 as u32,
            w!("Microsoft YaHei"),
        );
        let old_font = SelectObject(hdc_text, font);

        SetTextColor(hdc_text, COLORREF((*cb as u32) << 16 | (*cg as u32) << 8 | (*cr as u32)));

        let mut text_rect = RECT {
            left: padding + 10,
            top: current_y,
            right: w - padding - 10,
            bottom: current_y + row_h,
        };

        let mut text_wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        DrawTextW(hdc_text, &mut text_wide, &mut text_rect, DT_VCENTER | DT_SINGLELINE | DT_WORD_ELLIPSIS);

        SelectObject(hdc_text, old_font);
        DeleteObject(font);

        let text_pixels = std::slice::from_raw_parts(p_text_bits as *const u32, (w * h) as usize);
        for ty in current_y.max(0)..(current_y + row_h).min(h) {
            let row_offset = ty * w;
            for tx in (padding + 10).max(0)..(w - padding - 10).min(w) {
                let idx = (row_offset + tx) as usize;
                let text_val = text_pixels[idx];
                let tr = (text_val >> 16) & 0xFF;
                let tg = (text_val >> 8) & 0xFF;
                let tb = text_val & 0xFF;
                let brightness = tr.max(tg).max(tb);
                if brightness > 10 {
                    let text_alpha = brightness as u32;
                    let bg_val = pixels[idx];
                    let bg_a = (bg_val >> 24) & 0xFF;
                    let bg_r_val = ((bg_val >> 16) & 0xFF) * 255 / bg_a.max(1);
                    let bg_g_val = ((bg_val >> 8) & 0xFF) * 255 / bg_a.max(1);
                    let bg_b_val = (bg_val & 0xFF) * 255 / bg_a.max(1);

                    let out_a = (text_alpha + ((255 - text_alpha) * bg_a as u32) / 255).min(255);
                    let out_r = (*cr as u32 * text_alpha + bg_r_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);
                    let out_g = (*cg as u32 * text_alpha + bg_g_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);
                    let out_b = (*cb as u32 * text_alpha + bg_b_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);

                    let premult_r = out_r * out_a / 255;
                    let premult_g = out_g * out_a / 255;
                    let premult_b = out_b * out_a / 255;
                    pixels[idx] = (out_a << 24) | (premult_r << 16) | (premult_g << 8) | premult_b;
                }
            }
        }

        current_y += row_h;
    }

    let btn_size = 24i32;
    let btn_margin = 8i32;

    let lock_x = w - btn_margin - btn_size;
    let lock_y = btn_margin;
    let settings_x = w - btn_margin - btn_size * 2 - 4;
    let settings_y = btn_margin;

    draw_button_bg(pixels, w, h, lock_x, lock_y, btn_size, 160);
    draw_button_bg(pixels, w, h, settings_x, settings_y, btn_size, 160);

    let btn_font = CreateFontW(
        -14,
        0, 0, 0,
        FW_MEDIUM.0 as i32,
        0, 0, 0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        w!("Microsoft YaHei"),
    );
    let old_btn_font = SelectObject(hdc_text, btn_font);

    let lock_text = if state.locked { "\u{25A3}" } else { "\u{25A1}" };
    let settings_text = "\u{2699}";

    {
        let text_pixels = std::slice::from_raw_parts_mut(p_text_bits as *mut u32, (w * h) as usize);
        for p in text_pixels.iter_mut() {
            *p = 0xFF000000;
        }

        SetTextColor(hdc_text, COLORREF(0x00FFFFFF));

        let mut lock_wide: Vec<u16> = lock_text.encode_utf16().chain(Some(0)).collect();
        let mut lock_rect = RECT {
            left: lock_x,
            top: lock_y,
            right: lock_x + btn_size,
            bottom: lock_y + btn_size,
        };
        DrawTextW(hdc_text, &mut lock_wide, &mut lock_rect, DT_VCENTER | DT_SINGLELINE | DT_CENTER);

        let mut settings_wide: Vec<u16> = settings_text.encode_utf16().chain(Some(0)).collect();
        let mut settings_rect = RECT {
            left: settings_x,
            top: settings_y,
            right: settings_x + btn_size,
            bottom: settings_y + btn_size,
        };
        DrawTextW(hdc_text, &mut settings_wide, &mut settings_rect, DT_VCENTER | DT_SINGLELINE | DT_CENTER);

        let text_pixels = std::slice::from_raw_parts(p_text_bits as *const u32, (w * h) as usize);
        for by in lock_y..(lock_y + btn_size).min(h) {
            let row_offset = by * w;
            for bx in lock_x..(lock_x + btn_size).min(w) {
                let idx = (row_offset + bx) as usize;
                let text_val = text_pixels[idx];
                let tb = text_val & 0xFF;
                let brightness = tb;
                if brightness > 10 {
                    let text_alpha = brightness as u32;
                    let bg_val = pixels[idx];
                    let bg_a = (bg_val >> 24) & 0xFF;
                    let bg_r_val = ((bg_val >> 16) & 0xFF) * 255 / bg_a.max(1);
                    let bg_g_val = ((bg_val >> 8) & 0xFF) * 255 / bg_a.max(1);
                    let bg_b_val = (bg_val & 0xFF) * 255 / bg_a.max(1);
                    let out_a = (text_alpha + ((255 - text_alpha) * bg_a as u32) / 255).min(255);
                    let out_r = (255u32 * text_alpha + bg_r_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);
                    let out_g = (255u32 * text_alpha + bg_g_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);
                    let out_b = (255u32 * text_alpha + bg_b_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);
                    let premult_r = out_r * out_a / 255;
                    let premult_g = out_g * out_a / 255;
                    let premult_b = out_b * out_a / 255;
                    pixels[idx] = (out_a << 24) | (premult_r << 16) | (premult_g << 8) | premult_b;
                }
            }
        }
        for by in settings_y..(settings_y + btn_size).min(h) {
            let row_offset = by * w;
            for bx in settings_x..(settings_x + btn_size).min(w) {
                let idx = (row_offset + bx) as usize;
                let text_val = text_pixels[idx];
                let tb = text_val & 0xFF;
                let brightness = tb;
                if brightness > 10 {
                    let text_alpha = brightness as u32;
                    let bg_val = pixels[idx];
                    let bg_a = (bg_val >> 24) & 0xFF;
                    let bg_r_val = ((bg_val >> 16) & 0xFF) * 255 / bg_a.max(1);
                    let bg_g_val = ((bg_val >> 8) & 0xFF) * 255 / bg_a.max(1);
                    let bg_b_val = (bg_val & 0xFF) * 255 / bg_a.max(1);
                    let out_a = (text_alpha + ((255 - text_alpha) * bg_a as u32) / 255).min(255);
                    let out_r = (255u32 * text_alpha + bg_r_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);
                    let out_g = (255u32 * text_alpha + bg_g_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);
                    let out_b = (255u32 * text_alpha + bg_b_val * (255 - text_alpha) * bg_a as u32 / 255) / out_a.max(1);
                    let premult_r = out_r * out_a / 255;
                    let premult_g = out_g * out_a / 255;
                    let premult_b = out_b * out_a / 255;
                    pixels[idx] = (out_a << 24) | (premult_r << 16) | (premult_g << 8) | premult_b;
                }
            }
        }
    }

    SelectObject(hdc_text, old_btn_font);
    DeleteObject(btn_font);

    SelectObject(hdc_text, old_text_bmp);
    DeleteObject(text_bmp);
    DeleteDC(hdc_text);

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
