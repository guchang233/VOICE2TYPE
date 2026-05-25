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
    // or when other state entry animations are running (up to 400ms after state starts)
    let elapsed = state.state_start_time.elapsed().as_millis();
    let is_animating = elapsed < 400;
    let is_pulsing = state.target_state == IndicatorState::Recording || state.target_state == IndicatorState::Processing;
    if (is_pulsing || is_animating) && state.current_alpha > 0.01 {
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

    // Compute dynamic dark color wave components outside the double loop for high-speed performance
    let c_t = state.pulse_time;
    
    let sin_1 = (c_t * 1.2).sin();
    let cos_1 = (c_t * 0.5).cos();
    let r1 = 10.0 + sin_1 * 12.0 + cos_1 * 6.0;
    
    let cos_2 = (c_t * 0.8).cos();
    let sin_2 = (c_t * 1.5).sin();
    let g1 = 10.0 + cos_2 * 8.0 + sin_2 * 4.0;
    
    let sin_3 = (c_t * 0.9).sin();
    let cos_3 = (c_t * 1.1).cos();
    let b1 = 11.0 + sin_3 * 16.0 + cos_3 * 8.0;

    let sin_4 = (c_t * 0.7 + 2.0).sin();
    let cos_4 = (c_t * 1.3).cos();
    let r2 = 10.0 + sin_4 * 8.0 + cos_4 * 8.0;

    let cos_5 = (c_t * 1.1 + 1.0).cos();
    let g2 = 12.0 + cos_5 * 105.0 * 0.1;

    let sin_5 = (c_t * 0.5 + 3.0).sin();
    let b2 = 14.0 + sin_5 * 18.0;

    let blend_r = ((state.current_color >> 16) & 0xFF) as f32;
    let blend_g = ((state.current_color >> 8) & 0xFF) as f32;
    let blend_b = (state.current_color & 0xFF) as f32;

    let br1 = (r1 + blend_r * 0.08).min(255.0) as u32;
    let bg1 = (g1 + blend_g * 0.08).min(255.0) as u32;
    let bb1 = (b1 + blend_b * 0.08).min(255.0) as u32;

    let br2 = (r2 + blend_r * 0.04).min(255.0) as u32;
    let bg2 = (g2 + blend_g * 0.04).min(255.0) as u32;
    let bb2 = (b2 + blend_b * 0.04).min(255.0) as u32;

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
                
                let u = x as f32 / w as f32;
                let br = (br1 as f32 + u * (br2 as f32 - br1 as f32)) as u32;
                let bg = (bg1 as f32 + u * (bg2 as f32 - bg1 as f32)) as u32;
                let bb = (bb1 as f32 + u * (bb2 as f32 - bb1 as f32)) as u32;

                let (br, bg, bb) = if is_inner_border {
                    ((br + 42).min(255), (bg + 42).min(255), (bb + 44).min(255))
                } else {
                    (br, bg, bb)
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

    // --- Procedural Dynamic Vector Icons with Subpixel Antialiasing ---
    // Shake vibration to icon_cx for high-end mechanical feel under user feedback
    let shake_offset = if state.target_state == IndicatorState::Error {
        let elapsed_ms = state.state_start_time.elapsed().as_millis();
        if elapsed_ms < 300 {
            let freq = 0.15f32; // Fast energetic frequency
            let amplitude = 4.5f32; // 4.5px total maximum horizontal jitter
            let t_damp = elapsed_ms as f32 / 300.0;
            let damp = (1.0 - t_damp).powi(2); // Dampen down quadratically
            ((elapsed_ms as f32 * freq).sin() * amplitude * damp)
        } else {
            0.0
        }
    } else {
         0.0
    };
    let icon_cx = 22.0f32 + shake_offset;
    let icon_cy = h as f32 / 2.0;
    let dot_color = state.current_color;
    let dr = (dot_color >> 16) & 0xFF;
    let dg = (dot_color >> 8) & 0xFF;
    let db = dot_color & 0xFF;

    let pulse_time = state.pulse_time;
    let elapsed_ms = state.state_start_time.elapsed().as_millis();
    let is_pulsing = state.target_state == IndicatorState::Recording || state.target_state == IndicatorState::Processing;

    // Pulse wave intensity matching the oscillator format
    let pulse_intensity = if is_pulsing {
        (pulse_time.sin() + 1.0) / 2.0
    } else {
        0.0
    };

    // Soft surrounding glow halo matching active tasks
    let halo_max_r = 13.0f32;
    let halo_min_r = 7.0f32;
    let halo_r = halo_min_r + pulse_intensity * (halo_max_r - halo_min_r);
    let halo_alpha_base = if is_pulsing {
        (65.0 * (1.0 - pulse_intensity * 0.4)) as u32
    } else {
        0
    };

    // Unified Procedural Shape Evaluator
    let get_icon_alpha = |x: f32, y: f32| -> f32 {
        let dx = x - icon_cx;
        let dy = y - icon_cy;
        let dist = (dx * dx + dy * dy).sqrt();

        match &state.target_state {
            IndicatorState::Hidden => 0.0,
            IndicatorState::Recording => {
                // Hyper-premium Siri/Copilot style 5-bar voice spectrum live equalizer visualizer
                let t = pulse_time;
                let h0 = 4.0 + 5.0 * (t * 3.0 + 0.5).sin().abs();
                let h1 = 5.0 + 10.0 * (t * 4.2 + 1.2).sin().abs();
                let h2 = 6.0 + 15.0 * (t * 2.5 + 2.1).sin().abs();
                let h3 = 5.0 + 11.0 * (t * 3.8 + 0.3).sin().abs();
                let h4 = 4.0 + 6.0 * (t * 2.9 + 1.8).sin().abs();

                let draw_pill = |px: f32, py: f32, p_cx: f32, p_cy: f32, p_h: f32| -> f32 {
                    let half_len = ((p_h - 2.0) / 2.0).max(0.0);
                    let b_dx = px - p_cx;
                    let b_dy = py - p_cy;
                    let b_dist = if b_dy.abs() <= half_len {
                        b_dx.abs()
                    } else {
                        let sign = if b_dy > 0.0 { 1.0 } else { -1.0 };
                        (b_dx.powi(2) + (b_dy - sign * half_len).powi(2)).sqrt()
                    };
                    (1.0 - b_dist + 0.5).clamp(0.0, 1.0)
                };

                let a0 = draw_pill(x, y, icon_cx - 8.0, icon_cy, h0);
                let a1 = draw_pill(x, y, icon_cx - 4.0, icon_cy, h1);
                let a2 = draw_pill(x, y, icon_cx, icon_cy, h2);
                let a3 = draw_pill(x, y, icon_cx + 4.0, icon_cy, h3);
                let a4 = draw_pill(x, y, icon_cx + 8.0, icon_cy, h4);

                a0.max(a1).max(a2).max(a3).max(a4)
            }
            IndicatorState::Processing => {
                // Hyper-premium orbital double-swept swirling light trails (Apple Neural Engine style triple comet swirling loop)
                let r_ring = 6.2;
                let t_ring = 1.8;
                let ring_dist = (dist - r_ring).abs();
                let ring_alpha = (t_ring / 2.0 - ring_dist + 0.5).clamp(0.0, 1.0);
                if ring_alpha <= 0.0 {
                    return 0.0;
                }

                let angle = dy.atan2(dx);
                // Map to 0..2PI
                let angle = if angle < 0.0 { angle + 2.0 * std::f32::consts::PI } else { angle };

                // Swirling speed
                let rot = pulse_time * 3.5;

                // Helper to compute tail intensity for a comet head angle moving clockwise
                let get_comet_intensity = |head_angle: f32| -> f32 {
                    let mut tail_dist = head_angle - angle;
                    while tail_dist < 0.0 { tail_dist += 2.0 * std::f32::consts::PI; }
                    let tail_dist = tail_dist % (2.0 * std::f32::consts::PI);

                    let max_tail = 1.6; // approx 90 degrees trail length
                    if tail_dist < max_tail {
                        let f = tail_dist / max_tail;
                        (1.0 - f).powf(1.8) // Exponential fade
                    } else {
                        0.0
                    }
                };

                let head1 = rot;
                let head2 = rot + 2.0 * std::f32::consts::PI / 3.0;
                let head3 = rot + 4.0 * std::f32::consts::PI / 3.0;

                let i1 = get_comet_intensity(head1);
                let i2 = get_comet_intensity(head2);
                let i3 = get_comet_intensity(head3);

                ring_alpha * i1.max(i2).max(i3)
            }
            IndicatorState::Success => {
                // Elegant checkmark rendering with an animated writing effect + expanding sonic ripple
                let t_prog = (elapsed_ms as f32 / 300.0).clamp(0.0, 1.0);

                let p0 = (icon_cx - 5.0, icon_cy + 0.5);
                let p1 = (icon_cx - 1.5, icon_cy + 4.0);
                let p2 = (icon_cx + 5.5, icon_cy - 4.0);

                let dist_to_segment = |px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32| -> f32 {
                    let l2 = (x2 - x1).powi(2) + (y2 - y1).powi(2);
                    if l2 == 0.0 {
                        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
                    }
                    let t_val = ((px - x1) * (x2 - x1) + (py - y1) * (y2 - y1)) / l2;
                    let t_val = t_val.clamp(0.0, 1.0);
                    let proj_x = x1 + t_val * (x2 - x1);
                    let proj_y = y1 + t_val * (y2 - y1);
                    ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt()
                };

                let stroke_width = 1.8;
                let check_alpha = if t_prog < 0.4 {
                    let f = t_prog / 0.4;
                    let cur_x = p0.0 + f * (p1.0 - p0.0);
                    let cur_y = p0.1 + f * (p1.1 - p0.1);
                    let d = dist_to_segment(x, y, p0.0, p0.1, cur_x, cur_y);
                    (stroke_width / 2.0 - d + 0.5).clamp(0.0, 1.0)
                } else {
                    let d1 = dist_to_segment(x, y, p0.0, p0.1, p1.0, p1.1);
                    let a1 = (stroke_width / 2.0 - d1 + 0.5).clamp(0.0, 1.0);

                    let f2 = (t_prog - 0.4) / 0.6;
                    let cur_x = p1.0 + f2 * (p2.0 - p1.0);
                    let cur_y = p1.1 + f2 * (p2.1 - p1.1);
                    let d2 = dist_to_segment(x, y, p1.0, p1.1, cur_x, cur_y);
                    let a2 = (stroke_width / 2.0 - d2 + 0.5).clamp(0.0, 1.0);

                    a1.max(a2)
                };

                // Add an expanding shockwave ripple (Apple Pay style feedback)
                let ripple_alpha = {
                    let ripple_duration = 500.0f32;
                    let t_ripple = (elapsed_ms as f32 / ripple_duration).clamp(0.0, 1.0);
                    let ripple_max_r = 18.0f32;
                    let ripple_r = 4.0 + t_ripple * ripple_max_r;
                    let dist_to_ripple = (dist - ripple_r).abs();
                    // Sharp micro-edge boundary wave
                    let thickness = 1.0f32;
                    let alpha_profile = (thickness / 2.0 - dist_to_ripple + 0.5).clamp(0.0, 1.0);
                    let fade = (1.0 - t_ripple).powi(2);
                    alpha_profile * fade * 0.7
                };

                check_alpha.max(ripple_alpha)
            }
            IndicatorState::Error => {
                // Cross mark with sequential branch drawing animations + explosive feedback ripple
                let t_prog = (elapsed_ms as f32 / 250.0).clamp(0.0, 1.0);

                let p1a = (icon_cx - 4.5, icon_cy - 4.5);
                let p1b = (icon_cx + 4.5, icon_cy + 4.5);
                let p2a = (icon_cx - 4.5, icon_cy + 4.5);
                let p2b = (icon_cx + 4.5, icon_cy - 4.5);

                let dist_to_segment = |px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32| -> f32 {
                    let l2 = (x2 - x1).powi(2) + (y2 - y1).powi(2);
                    if l2 == 0.0 {
                        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
                    }
                    let t_val = ((px - x1) * (x2 - x1) + (py - y1) * (y2 - y1)) / l2;
                    let t_val = t_val.clamp(0.0, 1.0);
                    let proj_x = x1 + t_val * (x2 - x1);
                    let proj_y = y1 + t_val * (y2 - y1);
                    ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt()
                };

                let stroke_width = 1.8;
                let cross_alpha = if t_prog < 0.5 {
                    let f = t_prog / 0.5;
                    let cur_x = p1a.0 + f * (p1b.0 - p1a.0);
                    let cur_y = p1a.1 + f * (p1b.1 - p1a.1);
                    let d = dist_to_segment(x, y, p1a.0, p1a.1, cur_x, cur_y);
                    (stroke_width / 2.0 - d + 0.5).clamp(0.0, 1.0)
                } else {
                    let d1 = dist_to_segment(x, y, p1a.0, p1a.1, p1b.0, p1b.1);
                    let a1 = (stroke_width / 2.0 - d1 + 0.5).clamp(0.0, 1.0);

                    let f2 = (t_prog - 0.5) / 0.5;
                    let cur_x = p2a.0 + f2 * (p2b.0 - p2a.0);
                    let cur_y = p2a.1 + f2 * (p2b.1 - p2a.1);
                    let d2 = dist_to_segment(x, y, p2a.0, p2a.1, cur_x, cur_y);
                    let a2 = (stroke_width / 2.0 - d2 + 0.5).clamp(0.0, 1.0);

                    a1.max(a2)
                };

                // Add an explosive warning shockwave ripple
                let ripple_alpha = {
                    let t_ripple = (elapsed_ms as f32 / 550.0).clamp(0.0, 1.0);
                    let ripple_max_r = 20.0f32;
                    let ripple_r = 3.0 + t_ripple * ripple_max_r;
                    let dist_to_ripple = (dist - ripple_r).abs();
                    let thickness = 1.0f32;
                    let alpha_profile = (thickness / 2.0 - dist_to_ripple + 0.5).clamp(0.0, 1.0);
                    let fade = (1.0 - t_ripple).powf(1.8);
                    alpha_profile * fade * 0.65
                };

                cross_alpha.max(ripple_alpha)
            }
            IndicatorState::Cancelled => {
                // Cancelled minus dash drawing animation with elastic slow-down rotation (spins 270 degrees into horizontal focus)
                let t_prog = (elapsed_ms as f32 / 350.0).clamp(0.0, 1.0);
                
                let t_inv = 1.0 - t_prog;
                let angle = t_inv.powi(3) * (std::f32::consts::PI * 1.5); 

                let dx_rot = dx * angle.cos() + dy * angle.sin();
                let dy_rot = -dx * angle.sin() + dy * angle.cos();

                let cur_len = -5.0 + t_prog * 10.0; // Line writes horizontally while spinning for liquid flow effect
                let segment_dist = if dx_rot <= -5.0 {
                    ((dx_rot + 5.0).powi(2) + dy_rot.powi(2)).sqrt()
                } else if dx_rot >= cur_len {
                    ((dx_rot - cur_len).powi(2) + dy_rot.powi(2)).sqrt()
                } else {
                    dy_rot.abs()
                };

                let stroke_width = 1.8;
                (stroke_width / 2.0 - segment_dist + 0.5).clamp(0.0, 1.0)
            }
        }
    };

    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - icon_cx;
            let dy = y as f32 - icon_cy;
            let dist = (dx * dx + dy * dy).sqrt();

            // Evaluate procedural custom icon body alpha (expanded range to allow shockwave display up to 24.0px radius)
            let icon_alpha_f = if dist <= 24.0 {
                get_icon_alpha(x as f32, y as f32)
            } else {
                0.0
            };

            // Evaluate soft atmospheric halo glow around active state icons
            let halo_alpha_f = if is_pulsing && dist > 5.0 && dist <= halo_r {
                let d_factor = (halo_r - dist) / (halo_r - 5.0);
                d_factor.clamp(0.0, 1.0) * (halo_alpha_base as f32 / 255.0)
            } else {
                0.0
            };

            if icon_alpha_f > 0.0 || halo_alpha_f > 0.0 {
                let idx = (y * w + x) as usize;
                let bg_val = pixels[idx];
                let bg_a = (bg_val >> 24) & 0xFF;

                let dot_alpha = 235u32; // Crisp vivid foreground icon body opacity
                let effective_alpha = (icon_alpha_f * (dot_alpha as f32 / 255.0)) + (1.0 - icon_alpha_f) * halo_alpha_f;
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
        left: 40,
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
