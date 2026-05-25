use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

#[cfg(target_os = "windows")]
use windows::{
    core::*, Win32::Foundation::*, Win32::Graphics::Gdi::*,
    Win32::System::LibraryLoader::GetModuleHandleW, Win32::UI::WindowsAndMessaging::*,
};

#[derive(Debug, Clone, PartialEq)]
pub enum IndicatorState {
    Hidden,
    Recording,
    Processing,
    Success,
    Error,
    Cancelled,
}

#[derive(Clone)]
pub struct StatusIndicator {
    tx: Sender<IndicatorState>,
}

impl StatusIndicator {
    pub fn new(fade_duration: u64, error_duration: u64, success_duration: u64) -> Self {
        let (tx, rx) = channel();

        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            unsafe {
                create_and_run_window(rx, fade_duration, error_duration, success_duration);
            }
        });

        Self { tx }
    }

    pub fn set_state(&self, state: IndicatorState) {
        let _ = self.tx.send(state);
    }
}

#[cfg(target_os = "windows")]
struct WindowState {
    target_state: IndicatorState,

    // Animation state
    current_alpha: f32, // 0.0 to 1.0
    target_alpha: f32,

    current_color: u32, // RGB
    target_color: u32,

    text: String,

    // Layout (Fluid Dynamic Island Width Transition)
    width: i32,
    height: i32,
    current_width: f32,

    // Breathing pulse variable for active recording state
    pulse_time: f32,

    // State timing
    state_start_time: std::time::Instant, // 当前状态的开始时间

    // Duration settings
    fade_duration: u64,    // 淡出动画时间（毫秒）
    error_duration: u64,   // 错误状态持续时间（毫秒）
    success_duration: u64, // 成功状态持续时间（毫秒）
}

#[cfg(target_os = "windows")]
unsafe fn create_and_run_window(
    rx: Receiver<IndicatorState>,
    fade_duration: u64,
    error_duration: u64,
    success_duration: u64,
) {
    let instance = GetModuleHandleW(None).unwrap();
    let class_name = w!("Voice2TypeIndicatorClass");

    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
        hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
        style: CS_HREDRAW | CS_VREDRAW,
        ..Default::default()
    };

    RegisterClassW(&wc);

    let screen_width = GetSystemMetrics(SM_CXSCREEN);
    let initial_width = 200;
    let initial_height = 40;
    let x = (screen_width - initial_width) / 2;
    let y = 60; // Slightly lower than top to look like a floating island

    // WS_EX_TRANSPARENT makes it click-through
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
        class_name,
        w!("Voice2Type Indicator"),
        WS_POPUP | WS_VISIBLE,
        x,
        y,
        initial_width,
        initial_height,
        None,
        None,
        instance,
        None,
    );

    // Initial state
    let mut state = WindowState {
        target_state: IndicatorState::Hidden,
        current_alpha: 0.0,
        target_alpha: 0.0,
        current_color: 0x000000,
        target_color: 0x000000,
        text: String::new(),
        width: initial_width,
        height: initial_height,
        current_width: initial_width as f32,
        pulse_time: 0.0,
        state_start_time: std::time::Instant::now(),
        fade_duration,
        error_duration,
        success_duration,
    };

    // Store state pointer in window user data?
    // Simplified: Just keep it in the loop since we process messages manually-ish or use static/global if needed.
    // But wnd_proc needs to be static.
    // For this simple single-window thread, we can handle logic in the loop and just use DefWindowProc for the basics.
    // However, WM_PAINT/TIMER might be dispatched.
    // Let's keep logic in the main loop as much as possible, using PeekMessage or MsgWaitForMultipleObjects.

    // Setup Timer for animation (approx 60fps -> 16ms)
    SetTimer(hwnd, 1, 16, None);

    let mut msg = MSG::default();
    loop {
        // Check for channel messages (non-blocking)
        if let Ok(new_state) = rx.try_recv() {
            if new_state != state.target_state {
                state.target_state = new_state.clone();
                state.state_start_time = std::time::Instant::now(); // 重置状态开始时间
                update_targets(&mut state, &new_state);
            }
        }

        // 检查错误状态和成功状态是否需要自动隐藏
        if state.target_state == IndicatorState::Error {
            let elapsed = state.state_start_time.elapsed();
            if elapsed >= std::time::Duration::from_millis(state.error_duration) {
                state.target_state = IndicatorState::Hidden;
                state.state_start_time = std::time::Instant::now();
                update_targets(&mut state, &IndicatorState::Hidden);
            }
        } else if state.target_state == IndicatorState::Success {
            let elapsed = state.state_start_time.elapsed();
            if elapsed >= std::time::Duration::from_millis(state.success_duration) {
                state.target_state = IndicatorState::Hidden;
                state.state_start_time = std::time::Instant::now();
                update_targets(&mut state, &IndicatorState::Hidden);
            }
        }

        // Handle Windows messages
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);

            // Handle Timer manually if dispatch doesn't cover it well (it does)
            if msg.message == WM_TIMER && msg.wParam.0 == 1 {
                if update_animation(&mut state) {
                    draw_window(hwnd, &state);
                }
            }
        }

        // Sleep a bit to avoid CPU spin if no messages
        // MsgWaitForMultipleObjects would be better but Sleep(1) is okay for this simple thread
        thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(target_os = "windows")]
fn update_targets(state: &mut WindowState, new_state: &IndicatorState) {
    match new_state {
        IndicatorState::Hidden => {
            state.target_alpha = 0.0;
            // Keep color/text and width same for safe fade out
        }
        IndicatorState::Recording => {
            state.target_alpha = 1.0;
            state.target_color = 0x34C759; // Premium iOS Emerald Green
            state.text = "聆听中".to_string();
            state.width = 170;
        }
        IndicatorState::Processing => {
            state.target_alpha = 1.0;
            state.target_color = 0xFF9500; // Premium iOS Vivid Orange
            state.text = "处理中".to_string();
            state.width = 170;
        }
        IndicatorState::Success => {
            state.target_alpha = 1.0;
            state.target_color = 0x007AFF; // Premium iOS Royal Blue
            state.text = "已输出".to_string();
            state.width = 195;
        }
        IndicatorState::Error => {
            state.target_alpha = 1.0;
            state.target_color = 0xFF3B30; // Premium iOS Coral Red
            state.text = "错误".to_string();
            state.width = 155;
        }
        IndicatorState::Cancelled => {
            state.target_alpha = 1.0;
            state.target_color = 0x8E8E93; // Premium iOS Muted Neutral Gray
            state.text = "已取消".to_string();
            state.width = 170;
        }
    }
}

#[cfg(target_os = "windows")]
fn update_animation(state: &mut WindowState) -> bool {
    let mut changed = false;

    // Lerp alpha
    let alpha_diff = state.target_alpha - state.current_alpha;
    if alpha_diff.abs() > 0.01 {
        // 基于fade_duration计算平滑因子，确保动画时间符合配置
        let smooth_factor = 16.0 / state.fade_duration as f32 * 10.0;
        state.current_alpha += alpha_diff * smooth_factor.min(1.0);
        changed = true;
    } else {
        state.current_alpha = state.target_alpha;
    }

    // Lerp width (Elastic stretching for Dynamic Island morphing effect)
    let w_diff = state.width as f32 - state.current_width;
    if w_diff.abs() > 0.5 {
        let smooth_factor = 16.0 / state.fade_duration as f32 * 10.0;
        state.current_width += w_diff * smooth_factor.min(1.0);
        changed = true;
    } else {
        state.current_width = state.width as f32;
    }

    // Advance breathing oscillator (For glowing indicator dot waves)
    state.pulse_time += 0.08;
    if state.pulse_time > 2.0 * std::f32::consts::PI {
        state.pulse_time -= 2.0 * std::f32::consts::PI;
    }

    // Force animation frame updates while active recording/processing states are pulsing
    let is_pulsing = state.target_state == IndicatorState::Recording || state.target_state == IndicatorState::Processing;
    if is_pulsing && state.current_alpha > 0.01 {
        changed = true;
    }

    // Lerp color
    let r_target = (state.target_color >> 16) & 0xFF;
    let g_target = (state.target_color >> 8) & 0xFF;
    let b_target = state.target_color & 0xFF;

    let r_curr = (state.current_color >> 16) & 0xFF;
    let g_curr = (state.current_color >> 8) & 0xFF;
    let b_curr = state.current_color & 0xFF;

    let lerp = |c: u32, t: u32| -> u32 {
        if c < t {
            c + ((t - c) as f32 * 0.1).ceil() as u32
        } else {
            c - ((c - t) as f32 * 0.1).ceil() as u32
        }
    };

    let r_new = lerp(r_curr, r_target);
    let g_new = lerp(g_curr, g_target);
    let b_new = lerp(b_curr, b_target);

    let new_color = (r_new << 16) | (g_new << 8) | b_new;
    if new_color != state.current_color {
        state.current_color = new_color;
        changed = true;
    }

    // If alpha is 0, we can skip drawing, but we need to ensure we draw the "Hidden" state at least once
    if state.current_alpha < 0.01 && state.target_alpha == 0.0 {
        return changed; // If changed became true above (just finished fading), return true.
    }

    changed
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn wnd_proc(
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
unsafe fn draw_window(hwnd: HWND, state: &WindowState) {
    if state.current_alpha <= 0.01 {
        ShowWindow(hwnd, SW_HIDE);
        return;
    }

    let w = state.current_width as i32;
    let h = state.height;

    // Center the layered window dynamically horizontally on the screen as it stretches (Apple Dynamic Island style)
    let screen_width = GetSystemMetrics(SM_CXSCREEN);
    let x_pos = (screen_width - w) / 2;
    let _ = SetWindowPos(
        hwnd,
        HWND(0),
        x_pos,
        60, // Elegant top offset
        w,
        h,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );

    let hdc_screen = GetDC(None);
    let hdc_mem = CreateCompatibleDC(hdc_screen);

    // Create 32-bit DIB
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // Top-down
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

    // Cast bits to slice
    let pixels = std::slice::from_raw_parts_mut(p_bits as *mut u32, (w * h) as usize);

    // Clear to transparent
    for p in pixels.iter_mut() {
        *p = 0;
    }

    // Constants (Strict Apple Design Guidelines: Full Pill Capsule shape)
    let radius = h as f32 / 2.0; // 20px - perfectly rounded pill capsule rounded corners
    let padding = 18.0; // Compact elegant interior margins
    let dot_radius = 4.0; // 8px diameter precise micro dot
    let dot_x = padding + dot_radius;
    let dot_y = h as f32 / 2.0;

    // Background Color: Apple high-contrast deep black with subtle opacity
    let bg_alpha = 232u32; // 91% opacity for ultra premium frosted presence

    // Draw Background (Rounded Rect with Fine macOS Metal Border definition)
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;

            let cx = w as f32 / 2.0;
            let cy = h as f32 / 2.0;
            let px = (x as f32 - cx).abs();
            let py = (y as f32 - cy).abs();

            let qw = cx - radius;
            let qh = cy - radius;

            let dx = (px - qw).max(0.0);
            let dy = (py - qh).max(0.0);
            let dist = (dx * dx + dy * dy).sqrt();

            let alpha_f = (radius - dist + 0.5).clamp(0.0, 1.0);

            if alpha_f > 0.0 {
                // Apply a hairline thin high-contrast border matching charcoal macOS windows
                let is_inner_border = dist >= radius - 1.2 && dist <= radius;
                
                let (br, bg, bb) = if is_inner_border {
                    (52u32, 52u32, 55u32) // Soft silver-metallic edge highlight
                } else {
                    (10u32, 10u32, 11u32) // Pure deep charcoal velvet obsidian black
                };

                let pixel_alpha = (alpha_f * bg_alpha as f32) as u32;
                
                // Pre-multiplied alpha values
                let premult_r = (br * pixel_alpha) / 255;
                let premult_g = (bg * pixel_alpha) / 255;
                let premult_b = (bb * pixel_alpha) / 255;

                pixels[idx] = (pixel_alpha << 24) | (premult_r << 16) | (premult_g << 8) | premult_b;
            }
        }
    }

    // Dynamic Pulsing Breathing Glow for Indicator Dot (Active state fluid loops)
    let dot_color = state.current_color;
    let dot_alpha = 220u32; // Bright vibrant indicator core
    let dr = (dot_color >> 16) & 0xFF;
    let dg = (dot_color >> 8) & 0xFF;
    let db = dot_color & 0xFF;

    let pulse_time = state.pulse_time;
    let is_pulsing = state.target_state == IndicatorState::Recording || state.target_state == IndicatorState::Processing;
    
    // Wave intensity between 0.0 and 1.0 in a standard sine wave format
    let pulse_intensity = if is_pulsing {
        (pulse_time.sin() + 1.0) / 2.0
    } else {
        0.0
    };

    // Rescale dot slightly as it pulses
    let core_r = if is_pulsing {
        dot_radius - 0.5 + (pulse_intensity * 0.5)
    } else {
        dot_radius
    };

    // Create a larger floating halo glow around the dot (soft ambient lighting)
    let halo_max_r = dot_radius * 2.2;
    let halo_r = core_r + (pulse_intensity * (halo_max_r - core_r));
    let halo_alpha_base = (70.0 * (1.0 - pulse_intensity * 0.45)) as u32; // Naturally fades as it expands

    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - dot_x;
            let dy = y as f32 - dot_y;
            let dist = (dx * dx + dy * dy).sqrt();

            let core_alpha_f = (core_r - dist + 0.5).clamp(0.0, 1.0);
            
            let halo_alpha_f = if is_pulsing && dist > core_r {
                let d_factor = (halo_r - dist) / (halo_r - core_r);
                d_factor.clamp(0.0, 1.0) * (halo_alpha_base as f32 / 255.0)
            } else {
                0.0
            };

            if core_alpha_f > 0.0 || halo_alpha_f > 0.0 {
                let idx = (y * w + x) as usize;
                let bg_val = pixels[idx];
                let bg_a = (bg_val >> 24) & 0xFF;

                let effective_alpha = (core_alpha_f * (dot_alpha as f32 / 255.0)) + (1.0 - core_alpha_f) * halo_alpha_f;
                let final_a = (effective_alpha * 255.0).clamp(0.0, 255.0) as u32;

                if final_a > 0 {
                    let r = (dr * final_a) / 255;
                    let g = (dg * final_a) / 255;
                    let b = (db * final_a) / 255;

                    let src_a_f = final_a as f32 / 255.0;
                    let blend_r = r + (((bg_val >> 16) & 0xFF) as f32 * (1.0 - src_a_f)) as u32;
                    let blend_g = g + (((bg_val >> 8) & 0xFF) as f32 * (1.0 - src_a_f)) as u32;
                    let blend_b = b + (((bg_val) & 0xFF) as f32 * (1.0 - src_a_f)) as u32;
                    let blend_a = (final_a as f32 + bg_a as f32 * (1.0 - src_a_f)) as u32;

                    pixels[idx] = (blend_a << 24) | (blend_r << 16) | (blend_g << 8) | blend_b;
                }
            }
        }
    }

    // Draw Smooth Text using Windows Cleartype GDI
    SetBkMode(hdc_mem, TRANSPARENT);
    SetTextColor(hdc_mem, COLORREF(0x00FFFFFF)); // Bright white text

    let font_height = 16; // Perfectly sized SF-Pro style display font height
    let font = CreateFontW(
        -font_height,
        0,
        0,
        0,
        FW_SEMIBOLD.0 as i32, // Elegant Apple Typography (Medium/SemiBold weight)
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        w!("Microsoft YaHei UI"), // Adaptable fallback with best clear-type GDI rendering in China UI Windows environments
    );
    let old_font = SelectObject(hdc_mem, font);

    let mut text_rect = RECT {
        left: (dot_x + dot_radius + 14.0) as i32,
        top: 0,
        right: w,
        bottom: h,
    };

    let mut text_wide: Vec<u16> = state.text.encode_utf16().chain(Some(0)).collect();
    DrawTextW(
        hdc_mem,
        &mut text_wide,
        &mut text_rect,
        DT_VCENTER | DT_SINGLELINE,
    );

    // Alpha channel fix for smooth font rendering
    for y in text_rect.top..text_rect.bottom {
        for x in text_rect.left..text_rect.right {
            if x >= 0 && x < w && y >= 0 && y < h {
                let idx = (y * w + x as i32) as usize;
                let val = pixels[idx];
                let r = (val >> 16) & 0xFF;
                let g = (val >> 8) & 0xFF;
                let b = val & 0xFF;

                let max_c = r.max(g).max(b);
                if max_c > 0 {
                    let existing_bg_pixel = pixels[idx];
                    let current_a = (existing_bg_pixel >> 24) & 0xFF;
                    let font_alpha = max_c;
                    let target_a = current_a.max(font_alpha);
                    pixels[idx] = (target_a << 24) | (val & 0x00FFFFFF);
                }
            }
        }
    }

    SelectObject(hdc_mem, old_font);
    DeleteObject(font);

    // Final Update
    let pt_src = POINT { x: 0, y: 0 };
    let size = SIZE { cx: w, cy: h };

    // Global alpha for fade-in/out animation
    let global_alpha = (state.current_alpha * 255.0) as u8;

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
