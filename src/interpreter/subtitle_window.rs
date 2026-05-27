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
pub static SUBTITLE_VISIBLE: AtomicBool = AtomicBool::new(true);

#[cfg(target_os = "windows")]
thread_local! {
    static SUBTITLE_TX: RefCell<Option<Sender<SubtitleMessage>>> = RefCell::new(None);
    static SUBTITLE_LOCKED: Cell<bool> = Cell::new(false);
    static SUBTITLE_USER_HIDDEN: Cell<bool> = Cell::new(false);
    static SUBTITLE_USER_MOVED: Cell<bool> = Cell::new(false);
    static SUBTITLE_HOVER_UNLOCK: Cell<bool> = Cell::new(false);
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
    SetOpacity(f32),
    SetPosition(String),
    SetFontSize(u32),
    SetOriginalFontSize(u32),
    SetOriginalColor(String),
    SetTranslatedFontSize(u32),
    SetTranslatedColor(String),
    ToggleLock,
    OpenSettings,
    SetLocked(bool),
    ToggleVisibility,
    Shutdown,
}

impl SubtitleWindow {
    pub fn new(click_through: bool, opacity: f32, position: String, font_size: u32) -> Self {
        let (tx, rx) = channel();
        let tx_for_wnd = tx.clone();

        let _click_through = click_through;
        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            unsafe {
                run_subtitle_window(rx, tx_for_wnd, opacity, position, font_size);
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

    pub fn set_click_through(&self, _enabled: bool) {
    }

    pub fn set_opacity(&self, opacity: f32) {
        let _ = self.tx.send(SubtitleMessage::SetOpacity(opacity));
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

    pub fn toggle_visibility(&self) {
        let _ = self.tx.send(SubtitleMessage::ToggleVisibility);
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(SubtitleMessage::Shutdown);
    }
}

#[cfg(target_os = "windows")]
struct SubtitleState {
    lines: Vec<SubtitleLine>,
    visible: bool,
    opacity: f32,
    position: String,
    font_size: u32,
    original_font_size: u32,
    original_color: String,
    translated_font_size: u32,
    translated_color: String,
    locked: bool,
    user_hidden: bool,
    user_moved: bool,
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

const BTN_SIZE: i32 = 22;
const BTN_MARGIN: i32 = 6;
const BTN_GAP: i32 = 3;
const HOVER_TIMER_ID: usize = 1001;

fn btn_layout(w: i32) -> [(i32, i32, i32, i32); 4] {
    let y1 = BTN_MARGIN;
    let y2 = BTN_MARGIN + BTN_SIZE;
    let r0 = w - BTN_MARGIN;
    let r1 = r0 - BTN_SIZE;
    let r2 = r1 - BTN_GAP - BTN_SIZE;
    let r3 = r2 - BTN_GAP - BTN_SIZE;
    let r4 = r3 - BTN_GAP - BTN_SIZE;
    [
        (r1, y1, r0, y2),
        (r2, y1, r1 - BTN_GAP, y2),
        (r3, y1, r2 - BTN_GAP, y2),
        (r4, y1, r3 - BTN_GAP, y2),
    ]
}

#[cfg(target_os = "windows")]
unsafe fn run_subtitle_window(
    rx: Receiver<SubtitleMessage>,
    tx_for_wnd: Sender<SubtitleMessage>,
    opacity: f32,
    position: String,
    font_size: u32,
) {
    SUBTITLE_TX.with(|tl| *tl.borrow_mut() = Some(tx_for_wnd));
    SUBTITLE_LOCKED.with(|tl| tl.set(false));
    SUBTITLE_USER_HIDDEN.with(|tl| tl.set(false));
    SUBTITLE_USER_MOVED.with(|tl| tl.set(false));

    let instance = GetModuleHandleW(None).unwrap();
    let class_name = w!("Voice2TypeSubtitleClass");

    let wc = WNDCLASSW {
        lpfnWndProc: Some(subtitle_wnd_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };

    RegisterClassW(&wc);

    let ex_style = WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW;

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

    let _ = SetTimer(hwnd, HOVER_TIMER_ID, 50, None);

    let mut state = SubtitleState {
        lines: Vec::new(),
        visible: false,
        opacity,
        position,
        font_size,
        original_font_size: 18,
        original_color: "#AAAAAA".to_string(),
        translated_font_size: 24,
        translated_color: "#FFFFFF".to_string(),
        locked: false,
        user_hidden: false,
        user_moved: false,
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
                        let max_lines = 3;
                        while state.lines.len() > max_lines {
                            state.lines.remove(0);
                        }
                        state.visible = true;
                        state.last_update = Instant::now();
                        SUBTITLE_VISIBLE.store(true, Ordering::SeqCst);
                        if !state.user_hidden {
                            draw_subtitle(hwnd, &state);
                        }
                    }
                }
                SubtitleMessage::Hide => {
                    state.visible = false;
                    state.lines.clear();
                    SUBTITLE_VISIBLE.store(false, Ordering::SeqCst);
                    ShowWindow(hwnd, SW_HIDE);
                }
                SubtitleMessage::SetOpacity(op) => {
                    state.opacity = op.clamp(0.1, 1.0);
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetPosition(pos) => {
                    state.position = pos;
                    state.user_moved = false;
                    SUBTITLE_USER_MOVED.with(|tl| tl.set(false));
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetFontSize(size) => {
                    state.font_size = size;
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetOriginalFontSize(size) => {
                    state.original_font_size = size;
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetOriginalColor(color) => {
                    state.original_color = color;
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetTranslatedFontSize(size) => {
                    state.translated_font_size = size;
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::SetTranslatedColor(color) => {
                    state.translated_color = color;
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::ToggleLock => {
                    state.locked = !state.locked;
                    SUBTITLE_LOCKED.with(|tl| tl.set(state.locked));
                    update_window_transparency(hwnd, state.locked);
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::OpenSettings => {
                    SETTINGS_REQUESTED.store(true, Ordering::SeqCst);
                }
                SubtitleMessage::SetLocked(locked) => {
                    state.locked = locked;
                    SUBTITLE_LOCKED.with(|tl| tl.set(locked));
                    update_window_transparency(hwnd, locked);
                    if state.visible && !state.user_hidden {
                        draw_subtitle(hwnd, &state);
                    }
                }
                SubtitleMessage::ToggleVisibility => {
                    state.user_hidden = !state.user_hidden;
                    SUBTITLE_USER_HIDDEN.with(|tl| tl.set(state.user_hidden));
                    SUBTITLE_VISIBLE.store(!state.user_hidden, Ordering::SeqCst);
                    if state.user_hidden {
                        ShowWindow(hwnd, SW_HIDE);
                    } else if state.visible {
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

        if state.visible && !state.user_hidden && state.last_update.elapsed() > state.hide_after {
            state.visible = false;
            state.lines.clear();
            SUBTITLE_VISIBLE.store(false, Ordering::SeqCst);
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
unsafe fn update_window_transparency(hwnd: HWND, locked: bool) {
    let mut ex_style = (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & !(WS_EX_TRANSPARENT.0 as u32);
    if locked {
        ex_style |= WS_EX_TRANSPARENT.0 as u32;
    }
    SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style as i32);
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
        WM_ENTERSIZEMOVE => {
            SUBTITLE_USER_MOVED.with(|tl| tl.set(true));
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == HOVER_TIMER_ID {
                let locked = SUBTITLE_LOCKED.with(|tl| tl.get());
                if locked && IsWindowVisible(hwnd).as_bool() {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let _ = ScreenToClient(hwnd, &mut pt);

                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let w = rect.right;

                    let btns = btn_layout(w);
                    let in_unlock = pt.x >= btns[1].0 && pt.x <= btns[1].2
                        && pt.y >= btns[1].1 && pt.y <= btns[1].3;

                    let was_hovering = SUBTITLE_HOVER_UNLOCK.with(|tl| tl.get());
                    if in_unlock && !was_hovering {
                        SUBTITLE_HOVER_UNLOCK.with(|tl| tl.set(true));
                        update_window_transparency(hwnd, false);
                    } else if !in_unlock && was_hovering {
                        SUBTITLE_HOVER_UNLOCK.with(|tl| tl.set(false));
                        update_window_transparency(hwnd, true);
                    }
                } else if !locked {
                    SUBTITLE_HOVER_UNLOCK.with(|tl| tl.set(false));
                }
            }
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

            let btns = btn_layout(w);

            let in_btn_0 = local_x >= btns[0].0 && local_x <= btns[0].2 && local_y >= btns[0].1 && local_y <= btns[0].3;
            let in_btn_1 = local_x >= btns[1].0 && local_x <= btns[1].2 && local_y >= btns[1].1 && local_y <= btns[1].3;
            let in_btn_2 = local_x >= btns[2].0 && local_x <= btns[2].2 && local_y >= btns[2].1 && local_y <= btns[2].3;
            let in_btn_3 = local_x >= btns[3].0 && local_x <= btns[3].2 && local_y >= btns[3].1 && local_y <= btns[3].3;

            if SUBTITLE_LOCKED.get() {
                if SUBTITLE_HOVER_UNLOCK.get() && in_btn_1 {
                    LRESULT(HTCLIENT as isize)
                } else {
                    LRESULT(HTTRANSPARENT as isize)
                }
            } else if in_btn_0 || in_btn_1 || in_btn_2 || in_btn_3 {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTCAPTION as isize)
            }
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let w = rect.right;

            let btns = btn_layout(w);

            let in_drag = local_x_in_btn(x, &btns[3]) && local_y_in_btn(y, &btns[3]);

            if in_drag && !SUBTITLE_LOCKED.get() {
                let _ = PostMessageW(hwnd, WM_NCLBUTTONDOWN, WPARAM(HTCAPTION as usize), lparam);
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let w = rect.right;

            let btns = btn_layout(w);

            SUBTITLE_TX.with(|tx| {
                if let Some(sender) = tx.borrow().as_ref() {
                    if local_x_in_btn(x, &btns[0]) && local_y_in_btn(y, &btns[0]) {
                        let _ = sender.send(SubtitleMessage::ToggleVisibility);
                    } else if local_x_in_btn(x, &btns[1]) && local_y_in_btn(y, &btns[1]) {
                        let _ = sender.send(SubtitleMessage::ToggleLock);
                    } else if local_x_in_btn(x, &btns[2]) && local_y_in_btn(y, &btns[2]) {
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
fn local_x_in_btn(x: i32, btn: &(i32, i32, i32, i32)) -> bool {
    x >= btn.0 && x <= btn.2
}

#[cfg(target_os = "windows")]
fn local_y_in_btn(y: i32, btn: &(i32, i32, i32, i32)) -> bool {
    y >= btn.1 && y <= btn.3
}

#[cfg(target_os = "windows")]
fn draw_button_bg(pixels: &mut [u32], w: i32, h: i32, x1: i32, y1: i32, x2: i32, y2: i32, alpha: u32) {
    for by in y1..y2 {
        for bx in x1..x2 {
            if bx >= 0 && bx < w && by >= 0 && by < h {
                let idx = (by * w + bx) as usize;
                let premult = alpha * 40 / 255;
                pixels[idx] = (alpha << 24) | (premult << 16) | (premult << 8) | premult;
            }
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn draw_icon(pixels: &mut [u32], p_text_bits: *const u32, w: i32, h: i32, x1: i32, y1: i32, x2: i32, y2: i32) {
    let text_pixels = std::slice::from_raw_parts(p_text_bits, (w * h) as usize);
    for by in y1.max(0)..y2.min(h) {
        let row_offset = by * w;
        for bx in x1.max(0)..x2.min(w) {
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
        if let Some(ref translated_text) = line.translated {
            let (tr, tg, tb) = parse_hex_color(&state.translated_color);
            row_specs.push(((state.translated_font_size as f32 * 1.5) as i32, translated_text.clone(), state.translated_font_size, (tr, tg, tb)));
            let (or, og, ob) = parse_hex_color(&state.original_color);
            row_specs.push(((state.original_font_size as f32 * 1.4) as i32, line.original.clone(), state.original_font_size, (or, og, ob)));
        } else {
            let (cr, cg, cb) = (255u8, 255u8, 255u8);
            row_specs.push(((state.font_size as f32 * 1.6) as i32, line.original.clone(), state.font_size, (cr, cg, cb)));
        }
    }

    let total_height: i32 = row_specs.iter().map(|(h, _, _, _)| *h).sum();
    let h = padding * 2 + total_height + 10;

    let user_moved = SUBTITLE_USER_MOVED.with(|tl| tl.get());
    if user_moved {
        let mut cur_rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut cur_rect);
        let _ = SetWindowPos(hwnd, HWND(0), cur_rect.left, cur_rect.top, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    } else {
        let x = (screen_width - w) / 2;
        let y = if state.position == "top" {
            60
        } else {
            screen_height - h - 80
        };
        let _ = SetWindowPos(hwnd, HWND(0), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }

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

    let btns = btn_layout(w);

    draw_button_bg(pixels, w, h, btns[0].0, btns[0].1, btns[0].2, btns[0].3, 160);
    draw_button_bg(pixels, w, h, btns[1].0, btns[1].1, btns[1].2, btns[1].3, 160);
    draw_button_bg(pixels, w, h, btns[2].0, btns[2].1, btns[2].2, btns[2].3, 160);
    if !state.locked {
        draw_button_bg(pixels, w, h, btns[3].0, btns[3].1, btns[3].2, btns[3].3, 120);
    }

    let btn_font = CreateFontW(
        -13,
        0, 0, 0,
        FW_NORMAL.0 as i32,
        0, 0, 0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        w!("Segoe UI Symbol"),
    );
    let old_btn_font = SelectObject(hdc_text, btn_font);

    let eye_text = if state.user_hidden { "\u{25CB}" } else { "\u{25C9}" };
    let lock_text = if state.locked { "\u{1F512}" } else { "\u{1F513}" };
    let settings_text = "\u{2699}";
    let drag_text = "\u{2725}";

    {
        let text_pixels = std::slice::from_raw_parts_mut(p_text_bits as *mut u32, (w * h) as usize);
        for p in text_pixels.iter_mut() {
            *p = 0xFF000000;
        }

        SetTextColor(hdc_text, COLORREF(0x00FFFFFF));

        let draw_btn_text = |text: &str, idx: usize| {
            let mut wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let mut rect = RECT {
                left: btns[idx].0,
                top: btns[idx].1,
                right: btns[idx].2,
                bottom: btns[idx].3,
            };
            DrawTextW(hdc_text, &mut wide, &mut rect, DT_VCENTER | DT_SINGLELINE | DT_CENTER);
        };

        draw_btn_text(eye_text, 0);
        draw_btn_text(lock_text, 1);
        draw_btn_text(settings_text, 2);
        draw_btn_text(drag_text, 3);

        let p_text = p_text_bits as *const u32;
        draw_icon(pixels, p_text, w, h, btns[0].0, btns[0].1, btns[0].2, btns[0].3);
        draw_icon(pixels, p_text, w, h, btns[1].0, btns[1].1, btns[1].2, btns[1].3);
        draw_icon(pixels, p_text, w, h, btns[2].0, btns[2].1, btns[2].2, btns[2].3);
        if !state.locked {
            draw_icon(pixels, p_text, w, h, btns[3].0, btns[3].1, btns[3].2, btns[3].3);
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
