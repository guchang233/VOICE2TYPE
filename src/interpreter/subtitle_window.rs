//! 实时字幕浮窗 - 重写版
//!
//! 视觉设计：
//!   - 深色半透明圆角背景（radius=16）
//!   - 当前行：译文大字（白色）+ 原文小字（灰色）
//!   - 历史行：上方渐出，单行显示，60% 透明度
//!   - 按钮区：右上角 5 个图标按钮，悬停时轻微高亮
//!
//! 按钮（右→左）：
//!   [👁 隐藏/显示] [🔒 锁定] [🎤/🔊 音频源] [⚙ 设置] [✥ 拖拽]

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
pub static SUBTITLE_ALWAYS_VISIBLE: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
thread_local! {
    static SUBTITLE_TX:         RefCell<Option<Sender<SubtitleMessage>>> = RefCell::new(None);
    static SUBTITLE_LOCKED:     Cell<bool> = Cell::new(false);
    static SUBTITLE_USER_HIDDEN:Cell<bool> = Cell::new(false);
    static SUBTITLE_USER_MOVED: Cell<bool> = Cell::new(false);
    static SUBTITLE_HOVER_BTN:  Cell<i32>  = Cell::new(-1); // 当前悬停的按钮 index，-1=无
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
    ShowInterim { text: String },
    ClearInterim,
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
    ToggleAudioSource,
    Shutdown,
}

impl SubtitleWindow {
    pub fn new(click_through: bool, opacity: f32, position: String, font_size: u32) -> Self {
        let (tx, rx) = channel();
        let tx_clone = tx.clone();
        let _click_through = click_through;
        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            unsafe { run_subtitle_window(rx, tx_clone, opacity, position, font_size); }
        });
        Self { tx }
    }

    pub fn show_bilingual(&self, original: String, translated: Option<String>) {
        let _ = self.tx.send(SubtitleMessage::ShowBilingual { original, translated });
    }
    pub fn show_interim(&self, text: String) {
        let _ = self.tx.send(SubtitleMessage::ShowInterim { text });
    }
    pub fn clear_interim(&self) {
        let _ = self.tx.send(SubtitleMessage::ClearInterim);
    }
    pub fn hide(&self) { let _ = self.tx.send(SubtitleMessage::Hide); }
    pub fn set_click_through(&self, _e: bool) {}
    pub fn set_opacity(&self, v: f32)             { let _ = self.tx.send(SubtitleMessage::SetOpacity(v)); }
    pub fn set_font_size(&self, v: u32)           { let _ = self.tx.send(SubtitleMessage::SetFontSize(v)); }
    pub fn set_original_font_size(&self, v: u32)  { let _ = self.tx.send(SubtitleMessage::SetOriginalFontSize(v)); }
    pub fn set_original_color(&self, v: String)   { let _ = self.tx.send(SubtitleMessage::SetOriginalColor(v)); }
    pub fn set_translated_font_size(&self, v: u32){ let _ = self.tx.send(SubtitleMessage::SetTranslatedFontSize(v)); }
    pub fn set_translated_color(&self, v: String) { let _ = self.tx.send(SubtitleMessage::SetTranslatedColor(v)); }
    pub fn set_position(&self, v: String)         { let _ = self.tx.send(SubtitleMessage::SetPosition(v)); }
    pub fn set_locked(&self, v: bool)             { let _ = self.tx.send(SubtitleMessage::SetLocked(v)); }
    pub fn toggle_visibility(&self)               { let _ = self.tx.send(SubtitleMessage::ToggleVisibility); }
    pub fn shutdown(&self)                        { let _ = self.tx.send(SubtitleMessage::Shutdown); }
}

// ── 内部状态 ─────────────────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
struct SubtitleState {
    lines: Vec<SubtitleLine>,
    interim_text: String,
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
    audio_source: String,   // "speaker" | "microphone"
    last_update: Instant,
    hide_after: Duration,
}

// ── 颜色工具 ─────────────────────────────────────────────────────────────────
fn parse_hex_color(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    if h.len() == 6 {
        let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(255);
        (r, g, b)
    } else {
        (255, 255, 255)
    }
}

// ── 按钮布局（右上角，从右到左） ─────────────────────────────────────────────
const BTN_SIZE:   i32 = 22;
const BTN_MARGIN: i32 = 8;
const BTN_GAP:    i32 = 3;
const HOVER_TIMER_ID: usize = 1001;

/// 返回 5 个按钮的 (x1,y1,x2,y2)：[eye, lock, audio, settings, drag]
fn btn_layout(w: i32) -> [(i32, i32, i32, i32); 5] {
    let y1 = BTN_MARGIN;
    let y2 = BTN_MARGIN + BTN_SIZE;
    let r0 = w - BTN_MARGIN;
    let make = |right: i32| (right - BTN_SIZE, y1, right, y2);
    let b0 = make(r0);
    let b1 = make(b0.0 - BTN_GAP);
    let b2 = make(b1.0 - BTN_GAP);
    let b3 = make(b2.0 - BTN_GAP);
    let b4 = make(b3.0 - BTN_GAP);
    [b0, b1, b2, b3, b4]
}

fn pt_in_btn(x: i32, y: i32, btn: &(i32, i32, i32, i32)) -> bool {
    x >= btn.0 && x <= btn.2 && y >= btn.1 && y <= btn.3
}

// ── 主事件循环 ───────────────────────────────────────────────────────────────
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
    let class_name = w!("Voice2TypeSubtitleV2");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(subtitle_wnd_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };
    RegisterClassW(&wc);

    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);
    let win_w = (screen_w * 3 / 4).min(900);
    let win_h = 100i32;
    let x = (screen_w - win_w) / 2;
    let y = if position == "top" { 60 } else { screen_h - win_h - 80 };

    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
        class_name,
        w!("Voice2Type Subtitle"),
        WS_POPUP,
        x, y, win_w, win_h,
        None, None, instance, None,
    );

    let _ = SetTimer(hwnd, HOVER_TIMER_ID, 50, None);

    let initial_audio_source = if super::AUDIO_SOURCE.load(Ordering::SeqCst)
        == super::loopback::AUDIO_SOURCE_MICROPHONE
    { "microphone".to_string() } else { "speaker".to_string() };

    let mut state = SubtitleState {
        lines: Vec::new(),
        interim_text: String::new(),
        visible: false,
        opacity,
        position,
        font_size,
        original_font_size: 18,
        original_color: "#888888".to_string(),
        translated_font_size: 24,
        translated_color: "#FFFFFF".to_string(),
        locked: false,
        user_hidden: false,
        user_moved: false,
        audio_source: initial_audio_source,
        last_update: Instant::now(),
        hide_after: Duration::from_secs(8),
    };

    let mut msg = MSG::default();
    loop {
        // ── 处理消息队列 ────────────────────────────────────────────────────
        while let Ok(message) = rx.try_recv() {
            match message {
                SubtitleMessage::ShowBilingual { original, translated } => {
                    if !original.is_empty() {
                        state.lines.push(SubtitleLine { original, translated });
                        while state.lines.len() > 2 { state.lines.remove(0); }
                        state.interim_text.clear();
                        state.visible = true;
                        state.last_update = Instant::now();
                        SUBTITLE_VISIBLE.store(true, Ordering::SeqCst);
                        if !state.user_hidden { draw_subtitle(hwnd, &state); }
                    }
                }
                SubtitleMessage::ShowInterim { text } => {
                    if !text.is_empty() {
                        state.interim_text = text;
                        state.visible = true;
                        state.last_update = Instant::now();
                        SUBTITLE_VISIBLE.store(true, Ordering::SeqCst);
                        if !state.user_hidden { draw_subtitle(hwnd, &state); }
                    }
                }
                SubtitleMessage::ClearInterim => {
                    if !state.interim_text.is_empty() {
                        state.interim_text.clear();
                        if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                    }
                }
                SubtitleMessage::Hide => {
                    state.visible = false;
                    state.lines.clear();
                    state.interim_text.clear();
                    SUBTITLE_VISIBLE.store(false, Ordering::SeqCst);
                    ShowWindow(hwnd, SW_HIDE);
                }
                SubtitleMessage::SetOpacity(v) => {
                    state.opacity = v.clamp(0.1, 1.0);
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::SetPosition(v) => {
                    state.position = v;
                    state.user_moved = false;
                    SUBTITLE_USER_MOVED.with(|tl| tl.set(false));
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::SetFontSize(v) => {
                    state.font_size = v;
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::SetOriginalFontSize(v) => {
                    state.original_font_size = v;
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::SetOriginalColor(v) => {
                    state.original_color = v;
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::SetTranslatedFontSize(v) => {
                    state.translated_font_size = v;
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::SetTranslatedColor(v) => {
                    state.translated_color = v;
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::ToggleLock => {
                    state.locked = !state.locked;
                    SUBTITLE_LOCKED.with(|tl| tl.set(state.locked));
                    update_click_through(hwnd, state.locked);
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::SetLocked(v) => {
                    state.locked = v;
                    SUBTITLE_LOCKED.with(|tl| tl.set(v));
                    update_click_through(hwnd, v);
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::OpenSettings => {
                    SETTINGS_REQUESTED.store(true, Ordering::SeqCst);
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
                SubtitleMessage::ToggleAudioSource => {
                    state.audio_source = if state.audio_source == "speaker" {
                        super::AUDIO_SOURCE.store(
                            super::loopback::AUDIO_SOURCE_MICROPHONE, Ordering::SeqCst,
                        );
                        "microphone".to_string()
                    } else {
                        super::AUDIO_SOURCE.store(
                            super::loopback::AUDIO_SOURCE_SPEAKER, Ordering::SeqCst,
                        );
                        "speaker".to_string()
                    };
                    if state.visible && !state.user_hidden { draw_subtitle(hwnd, &state); }
                }
                SubtitleMessage::Shutdown => {
                    ShowWindow(hwnd, SW_HIDE);
                    let _ = DestroyWindow(hwnd);
                    return;
                }
            }
        }

        // ── 自动隐藏 ────────────────────────────────────────────────────────
        if state.visible && !state.user_hidden && !SUBTITLE_ALWAYS_VISIBLE.load(Ordering::SeqCst) && state.interim_text.is_empty() && state.last_update.elapsed() > state.hide_after {
            state.visible = false;
            state.lines.clear();
            SUBTITLE_VISIBLE.store(false, Ordering::SeqCst);
            ShowWindow(hwnd, SW_HIDE);
        }

        // ── Win32 消息循环 ──────────────────────────────────────────────────
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_DESTROY { return; }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        thread::sleep(Duration::from_millis(16));
    }
}

// ── 窗口透传切换 ─────────────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
unsafe fn update_click_through(hwnd: HWND, through: bool) {
    let mut ex = (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & !(WS_EX_TRANSPARENT.0 as u32);
    if through { ex |= WS_EX_TRANSPARENT.0 as u32; }
    SetWindowLongW(hwnd, GWL_EXSTYLE, ex as i32);
}

// ── WndProc ──────────────────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
unsafe extern "system" fn subtitle_wnd_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }

        WM_ENTERSIZEMOVE => { SUBTITLE_USER_MOVED.with(|tl| tl.set(true)); LRESULT(0) }

        WM_TIMER => {
            if wparam.0 == HOVER_TIMER_ID {
                let locked = SUBTITLE_LOCKED.with(|tl| tl.get());
                if IsWindowVisible(hwnd).as_bool() {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let _ = ScreenToClient(hwnd, &mut pt);
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let btns = btn_layout(rect.right);

                    let hovered: i32 = (0..5_usize)
                        .find(|&i| pt_in_btn(pt.x, pt.y, &btns[i]))
                        .map(|i| i as i32)
                        .unwrap_or(-1);

                    let prev = SUBTITLE_HOVER_BTN.with(|tl| tl.get());
                    if hovered != prev {
                        SUBTITLE_HOVER_BTN.with(|tl| tl.set(hovered));
                        // 锁定态：仅 unlock 按钮（index=1）可穿透给 WM_LBUTTONUP
                        if locked {
                            let unlock_hovered = hovered == 1;
                            update_click_through(hwnd, !unlock_hovered);
                        }
                    }
                }
            }
            LRESULT(0)
        }

        WM_NCHITTEST => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let lx = x - wr.left;
            let ly = y - wr.top;
            let w = wr.right - wr.left;
            let btns = btn_layout(w);

            if SUBTITLE_LOCKED.with(|tl| tl.get()) {
                // 锁定时：unlock 按钮（b1）通过悬停计时器已暂时取消透传，其余透传
                if pt_in_btn(lx, ly, &btns[1]) && SUBTITLE_HOVER_BTN.with(|tl| tl.get()) == 1 {
                    LRESULT(HTCLIENT as isize)
                } else {
                    LRESULT(HTTRANSPARENT as isize)
                }
            } else {
                if pt_in_btn(lx, ly, &btns[4]) {
                    LRESULT(HTCAPTION as isize)
                } else if (0..4).any(|i| pt_in_btn(lx, ly, &btns[i])) {
                    LRESULT(HTCLIENT as isize)
                } else {
                    LRESULT(HTCAPTION as isize)
                }
            }
        }

        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let btns = btn_layout(rect.right);

            SUBTITLE_TX.with(|tx| {
                if let Some(sender) = tx.borrow().as_ref() {
                    if pt_in_btn(x, y, &btns[0]) {
                        let _ = sender.send(SubtitleMessage::ToggleVisibility);
                    } else if pt_in_btn(x, y, &btns[1]) {
                        let _ = sender.send(SubtitleMessage::ToggleLock);
                    } else if pt_in_btn(x, y, &btns[2]) {
                        let _ = sender.send(SubtitleMessage::ToggleAudioSource);
                    } else if pt_in_btn(x, y, &btns[3]) {
                        let _ = sender.send(SubtitleMessage::OpenSettings);
                    }
                    // btns[4] = drag，不需要发消息（由 HTCAPTION 处理）
                }
            });
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── 渲染 ─────────────────────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
unsafe fn draw_subtitle(hwnd: HWND, state: &SubtitleState) {
    let has_content = !state.lines.is_empty() || !state.interim_text.is_empty();
    if !has_content || !state.visible {
        ShowWindow(hwnd, SW_HIDE);
        return;
    }

    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);

    // ── 窗口宽度 ─────────────────────────────────────────────────────────────
    let pad_h = 18i32; // 水平内边距
    let pad_v = 12i32; // 垂直内边距
    let line_gap = 2i32;  // 同一对内：译/原行间距
    let pair_gap = 6i32;  // 历史行与当前行之间的额外间距
    // ── 预先计算各行尺寸（单行显示，宽度随最长句自适应）────────────────────
    // 临时 DC 用于 DT_CALCRECT
    let hdc_screen = GetDC(None);
    let hdc_tmp = CreateCompatibleDC(hdc_screen);

    struct RowSpec {
        text: String,
        font_size: i32,
        color: (u8, u8, u8),
        alpha_mul: f32, // 1.0 = 当前，0.55 = 历史
        height: i32,
        text_width: i32,
    }

    let mut rows: Vec<RowSpec> = Vec::new();
    let mut max_text_px = 0i32;
    let n = state.lines.len();

    for (i, line) in state.lines.iter().enumerate() {
        let is_current = i == n - 1;
        let alpha_mul = if is_current { 1.0f32 } else { 0.55f32 };

        if let Some(ref trans) = line.translated {
            // 译文行
            let (cr, cg, cb) = parse_hex_color(&state.translated_color);
            let fs = if is_current { state.translated_font_size as i32 } else { (state.translated_font_size as f32 * 0.85) as i32 };
            let tw = calc_text_width(hdc_tmp, trans, fs);
            let h = single_line_height(fs);
            max_text_px = max_text_px.max(tw);
            rows.push(RowSpec { text: trans.clone(), font_size: fs, color: (cr, cg, cb), alpha_mul, height: h, text_width: tw });

            // 原文行
            let (or, og, ob) = parse_hex_color(&state.original_color);
            let fs2 = if is_current { state.original_font_size as i32 } else { (state.original_font_size as f32 * 0.85) as i32 };
            let tw2 = calc_text_width(hdc_tmp, &line.original, fs2);
            let h2 = single_line_height(fs2);
            max_text_px = max_text_px.max(tw2);
            rows.push(RowSpec { text: line.original.clone(), font_size: fs2, color: (or, og, ob), alpha_mul: alpha_mul * 0.8, height: h2, text_width: tw2 });
        } else {
            // 无翻译：只显示原文
            let (cr, cg, cb) = (255u8, 255u8, 255u8);
            let fs = if is_current { state.font_size as i32 } else { (state.font_size as f32 * 0.85) as i32 };
            let tw = calc_text_width(hdc_tmp, &line.original, fs);
            let h = single_line_height(fs);
            max_text_px = max_text_px.max(tw);
            rows.push(RowSpec { text: line.original.clone(), font_size: fs, color: (cr, cg, cb), alpha_mul, height: h, text_width: tw });
        }

        // 历史行与当前行之间加额外间距
        if !is_current && i + 1 < n {
            if let Some(last) = rows.last_mut() {
                last.height += pair_gap;
            }
        } else if i + 1 < n {
            // 同一 pair 的行间距
            if let Some(last) = rows.last_mut() {
                last.height += line_gap;
            }
        }
    }

    // ── Interim 行（正在转写中的文本）──────────────────────────────────────
    if !state.interim_text.is_empty() {
        let interim_display = format!("{}▌", state.interim_text);
        let fs = state.font_size as i32;
        let tw = calc_text_width(hdc_tmp, &interim_display, fs);
        let h = single_line_height(fs);
        max_text_px = max_text_px.max(tw);
        // interim 用略暗的白色 + 0.9 透明度
        rows.push(RowSpec {
            text: interim_display,
            font_size: fs,
            color: (200, 200, 200),
            alpha_mul: 0.9,
            height: h,
            text_width: tw,
        });
    }

    DeleteDC(hdc_tmp);
    ReleaseDC(None, hdc_screen);

    let screen_max_w = (screen_w * 3 / 4).min(900);
    let min_w = 280i32;
    let w = (max_text_px + pad_h * 2)
        .clamp(min_w, screen_max_w);
    let text_w = w - pad_h * 2;

    let total_text_h: i32 = rows.iter().map(|r| r.height + line_gap).sum::<i32>() - line_gap;
    let btn_area = BTN_MARGIN * 2 + BTN_SIZE;
    let content_h = total_text_h + pad_v * 2 + btn_area;
    let h = content_h;

    // ── 定位窗口 ─────────────────────────────────────────────────────────────
    let user_moved = SUBTITLE_USER_MOVED.with(|tl| tl.get());
    if user_moved {
        let mut cur = RECT::default();
        let _ = GetWindowRect(hwnd, &mut cur);
        let _ = SetWindowPos(hwnd, HWND(0), cur.left, cur.top, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    } else {
        let x = (screen_w - w) / 2;
        let y = if state.position == "top" { 60 } else { screen_h - h - 80 };
        let _ = SetWindowPos(hwnd, HWND(0), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }

    // ── 创建 DIB ──────────────────────────────────────────────────────────────
    let hdc_screen = GetDC(None);
    let hdc_mem = CreateCompatibleDC(hdc_screen);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize:   std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth:  w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut p_bits: *mut c_void = std::ptr::null_mut();
    let hbmp = CreateDIBSection(hdc_mem, &bmi, DIB_RGB_COLORS, &mut p_bits, None, 0).unwrap();
    let old_bmp = SelectObject(hdc_mem, hbmp);
    let pixels = std::slice::from_raw_parts_mut(p_bits as *mut u32, (w * h) as usize);
    for p in pixels.iter_mut() { *p = 0; }

    // ── 绘制背景：深色圆角矩形（预乘 ARGB）──────────────────────────────────
    let bg_alpha = (state.opacity * 210.0) as u32; // 背景整体透明度
    let radius = 16.0f32;
    // 背景色：#0C0C12（深蓝黑）
    let bg_r = 12u32; let bg_g = 12u32; let bg_b = 18u32;

    for py in 0..h {
        for px in 0..w {
            let dx = if (px as f32) < radius { radius - px as f32 }
                     else if (px as f32) > (w as f32 - 1.0 - radius) { px as f32 - (w as f32 - 1.0 - radius) }
                     else { 0.0 };
            let dy = if (py as f32) < radius { radius - py as f32 }
                     else if (py as f32) > (h as f32 - 1.0 - radius) { py as f32 - (h as f32 - 1.0 - radius) }
                     else { 0.0 };
            let dist = (dx * dx + dy * dy).sqrt();
            // 2px AA 边缘
            let coverage = if dist <= radius - 1.0 { 1.0f32 }
                           else if dist < radius + 1.0 { (radius + 1.0 - dist) / 2.0 }
                           else { 0.0 };
            if coverage > 0.0 {
                let a = (coverage * bg_alpha as f32) as u32;
                let r = bg_r * a / 255;
                let g = bg_g * a / 255;
                let b_val = bg_b * a / 255;
                pixels[(py * w + px) as usize] = (a << 24) | (r << 16) | (g << 8) | b_val;
            }
        }
    }

    // ── 文字渲染 DC ──────────────────────────────────────────────────────────
    let hdc_text = CreateCompatibleDC(hdc_screen);
    let mut p_tbits: *mut c_void = std::ptr::null_mut();
    let text_bmp = CreateDIBSection(hdc_text, &bmi, DIB_RGB_COLORS, &mut p_tbits, None, 0).unwrap();
    let old_tbmp = SelectObject(hdc_text, text_bmp);
    SetBkMode(hdc_text, TRANSPARENT);

    let mut cur_y = pad_v + BTN_MARGIN * 2 + BTN_SIZE;

    for row in &rows {
        // 清空文字 DC（纯黑背景）
        let tpixels = std::slice::from_raw_parts_mut(p_tbits as *mut u32, (w * h) as usize);
        for p in tpixels.iter_mut() { *p = 0xFF000000; }

        let font = CreateFontW(
            -row.font_size, 0, 0, 0,
            FW_MEDIUM.0 as i32, 0, 0, 0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            DEFAULT_PITCH.0 as u32,
            w!("Microsoft YaHei"),
        );
        let old_font = SelectObject(hdc_text, font);
        let (cr, cg, cb) = row.color;
        // GDI DrawText 使用 BGR
        SetTextColor(hdc_text, COLORREF((cb as u32) << 16 | (cg as u32) << 8 | cr as u32));

        let mut rect = RECT {
            left: pad_h,
            top: cur_y,
            right: pad_h + text_w,
            bottom: cur_y + row.height,
        };
        let mut wide: Vec<u16> = row.text.encode_utf16().chain(Some(0)).collect();
        let draw_flags = if row.text_width > text_w {
            DT_SINGLELINE | DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX
        } else {
            DT_SINGLELINE | DT_LEFT | DT_VCENTER | DT_NOPREFIX
        };
        DrawTextW(hdc_text, &mut wide, &mut rect, draw_flags);

        SelectObject(hdc_text, old_font);
        DeleteObject(font);

        // 将文字 DC 的亮度通道 alpha-composite 到主 pixels
        let tpixels = std::slice::from_raw_parts(p_tbits as *const u32, (w * h) as usize);
        let row_end = (cur_y + row.height).min(h);
        for ty in cur_y.max(0)..row_end {
            let row_off = ty * w;
            for tx in pad_h.max(0)..(w - pad_h).min(w) {
                let idx = (row_off + tx) as usize;
                let tv = tpixels[idx];
                let tr = (tv >> 16) & 0xFF;
                let tg = (tv >> 8) & 0xFF;
                let tb = tv & 0xFF;
                let brightness = tr.max(tg).max(tb);
                if brightness > 8 {
                    // 应用行的历史淡化
                    let text_a = ((brightness as f32 * row.alpha_mul) as u32).min(255);
                    blend_pixel(&mut pixels[idx], cr, cg, cb, text_a);
                }
            }
        }

        cur_y += row.height + line_gap;
    }

    // ── 按钮渲染 ─────────────────────────────────────────────────────────────
    let btns = btn_layout(w);
    let hover_idx = SUBTITLE_HOVER_BTN.with(|tl| tl.get());

    // 按钮背景
    for (i, btn) in btns.iter().enumerate() {
        if i == 4 && state.locked { continue; } // 锁定时不显示拖拽按钮
        let is_hover = hover_idx == i as i32;
        let btn_bg_a: u32 = if is_hover { 200 } else { 120 };
        fill_rounded_rect(pixels, w, h, btn.0, btn.1, btn.2, btn.3, 5.0, 10, 10, 15, btn_bg_a);
    }

    // 按钮图标
    let icon_font = CreateFontW(
        -13, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        w!("Segoe UI Symbol"),
    );
    let old_ifont = SelectObject(hdc_text, icon_font);
    SetTextColor(hdc_text, COLORREF(0x00FFFFFF));

    let eye_icon  = if state.user_hidden { "\u{25CB}" } else { "\u{25C9}" };
    let lock_icon = if state.locked { "\u{1F512}" } else { "\u{1F513}" };
    let audio_icon = if state.audio_source == "microphone" { "\u{1F3A4}" } else { "\u{1F50A}" };
    let icons = [eye_icon, lock_icon, audio_icon, "\u{2699}", "\u{2725}"];

    let tpixels = std::slice::from_raw_parts_mut(p_tbits as *mut u32, (w * h) as usize);
    for p in tpixels.iter_mut() { *p = 0xFF000000; }

    for (i, &icon) in icons.iter().enumerate() {
        if i == 4 && state.locked { continue; }
        let mut rect = RECT { left: btns[i].0, top: btns[i].1, right: btns[i].2, bottom: btns[i].3 };
        let mut wide: Vec<u16> = icon.encode_utf16().chain(Some(0)).collect();
        DrawTextW(hdc_text, &mut wide, &mut rect, DT_VCENTER | DT_SINGLELINE | DT_CENTER);
    }

    let tpixels = std::slice::from_raw_parts(p_tbits as *const u32, (w * h) as usize);
    for (i, btn) in btns.iter().enumerate() {
        if i == 4 && state.locked { continue; }
        for ty in btn.1.max(0)..btn.3.min(h) {
            for tx in btn.0.max(0)..btn.2.min(w) {
                let idx = (ty * w + tx) as usize;
                let tv = tpixels[idx];
                let brightness = ((tv >> 16) & 0xFF).max((tv >> 8) & 0xFF).max(tv & 0xFF);
                if brightness > 8 {
                    blend_pixel(&mut pixels[idx], 255, 255, 255, brightness as u32);
                }
            }
        }
    }

    SelectObject(hdc_text, old_ifont);
    DeleteObject(icon_font);
    SelectObject(hdc_text, old_tbmp);
    DeleteObject(text_bmp);
    DeleteDC(hdc_text);

    // ── UpdateLayeredWindow ──────────────────────────────────────────────────
    let pt_src = POINT { x: 0, y: 0 };
    let size = SIZE { cx: w, cy: h };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let _ = UpdateLayeredWindow(
        hwnd, hdc_screen, None, Some(&size),
        hdc_mem, Some(&pt_src), COLORREF(0), Some(&blend), ULW_ALPHA,
    );
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);

    SelectObject(hdc_mem, old_bmp);
    DeleteObject(hbmp);
    DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
}

// ── 渲染辅助 ─────────────────────────────────────────────────────────────────

/// 将 (r,g,b,text_a) 预乘 alpha-composite 到 pixels[idx] 上
#[cfg(target_os = "windows")]
#[inline]
fn blend_pixel(pixel: &mut u32, r: u8, g: u8, b: u8, text_a: u32) {
    let bg = *pixel;
    let bg_a = (bg >> 24) & 0xFF;
    let bg_r = if bg_a > 0 { ((bg >> 16) & 0xFF) * 255 / bg_a } else { 0 };
    let bg_g = if bg_a > 0 { ((bg >>  8) & 0xFF) * 255 / bg_a } else { 0 };
    let bg_b = if bg_a > 0 { ( bg        & 0xFF) * 255 / bg_a } else { 0 };
    let out_a = (text_a + (255 - text_a) * bg_a / 255).min(255);
    if out_a == 0 { return; }
    let out_r = (r as u32 * text_a + bg_r * (255 - text_a) * bg_a / 255) / out_a;
    let out_g = (g as u32 * text_a + bg_g * (255 - text_a) * bg_a / 255) / out_a;
    let out_b = (b as u32 * text_a + bg_b * (255 - text_a) * bg_a / 255) / out_a;
    let pr = out_r * out_a / 255;
    let pg = out_g * out_a / 255;
    let pb = out_b * out_a / 255;
    *pixel = (out_a << 24) | (pr << 16) | (pg << 8) | pb;
}

#[cfg(target_os = "windows")]
fn single_line_height(font_size: i32) -> i32 {
    font_size + 8
}

/// 测量单行文字像素宽度
#[cfg(target_os = "windows")]
unsafe fn calc_text_width(hdc: HDC, text: &str, font_size: i32) -> i32 {
    let font = CreateFontW(
        -font_size, 0, 0, 0, FW_MEDIUM.0 as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32, w!("Microsoft YaHei"),
    );
    let old_font = SelectObject(hdc, font);
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE::default();
    let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
    SelectObject(hdc, old_font);
    DeleteObject(font);
    size.cx + 4
}

/// 填充带圆角的矩形（AA 边缘，预乘 ARGB）
#[cfg(target_os = "windows")]
fn fill_rounded_rect(
    pixels: &mut [u32], w: i32, h: i32,
    x1: i32, y1: i32, x2: i32, y2: i32,
    radius: f32,
    fill_r: u32, fill_g: u32, fill_b: u32,
    fill_a: u32,
) {
    let rx1 = x1 as f32 + radius;
    let ry1 = y1 as f32 + radius;
    let rx2 = x2 as f32 - radius;
    let ry2 = y2 as f32 - radius;

    for py in y1.max(0)..y2.min(h) {
        for px in x1.max(0)..x2.min(w) {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            let dx = if fx < rx1 { rx1 - fx } else if fx > rx2 { fx - rx2 } else { 0.0 };
            let dy = if fy < ry1 { ry1 - fy } else if fy > ry2 { fy - ry2 } else { 0.0 };
            let dist = (dx * dx + dy * dy).sqrt();
            let cov = if dist <= radius - 1.0 { 1.0f32 }
                      else if dist < radius + 1.0 { (radius + 1.0 - dist) / 2.0 }
                      else { 0.0 };
            if cov > 0.0 {
                let a = (cov * fill_a as f32) as u32;
                let r = fill_r * a / 255;
                let g = fill_g * a / 255;
                let b = fill_b * a / 255;
                let idx = (py * w + px) as usize;
                let bg = pixels[idx];
                let bg_a = (bg >> 24) & 0xFF;
                let out_a = (a + (255 - a) * bg_a / 255).min(255);
                if out_a > 0 {
                    let pr = (r + (bg >> 16 & 0xFF) * (255 - a) / 255) * out_a / 255;
                    let pg = (g + (bg >>  8 & 0xFF) * (255 - a) / 255) * out_a / 255;
                    let pb = (b + (bg       & 0xFF) * (255 - a) / 255) * out_a / 255;
                    pixels[idx] = (out_a << 24) | (pr << 16) | (pg << 8) | pb;
                }
            }
        }
    }
}

// ── non-Windows stub ─────────────────────────────────────────────────────────
#[cfg(not(target_os = "windows"))]
impl SubtitleWindow {
    pub fn new(_ct: bool, _op: f32, _pos: String, _fs: u32) -> Self {
        let (tx, _) = channel();
        Self { tx }
    }
}