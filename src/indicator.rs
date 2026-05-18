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
    pub fn new() -> Self {
        let (tx, rx) = channel();

        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            unsafe {
                create_and_run_window(rx);
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

    // Layout
    width: i32,
    height: i32,

    // State timing
    state_start_time: std::time::Instant, // 当前状态的开始时间

    // Duration settings
    fade_duration: u64,    // 淡出动画时间（毫秒）
    error_duration: u64,   // 错误状态持续时间（毫秒）
    success_duration: u64, // 成功状态持续时间（毫秒）
}

#[cfg(target_os = "windows")]
unsafe fn create_and_run_window(rx: Receiver<IndicatorState>) {
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
        state_start_time: std::time::Instant::now(),
        // Default duration settings
        fade_duration: 300,     // 默认 300 毫秒
        error_duration: 5000,   // 默认 5000 毫秒
        success_duration: 5000, // 默认 5000 毫秒
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
            // Keep color/text same for fade out
        }
        IndicatorState::Recording => {
            state.target_alpha = 1.0;
            state.target_color = 0x00FF00; // Green
            state.text = "聆听中".to_string();
        }
        IndicatorState::Processing => {
            state.target_alpha = 1.0;
            state.target_color = 0xFFD600; // Yellow
            state.text = "处理中".to_string();
        }
        IndicatorState::Success => {
            state.target_alpha = 1.0;
            state.target_color = 0x2196F3; // Blue
            state.text = "已输出".to_string();
        }
        IndicatorState::Error => {
            state.target_alpha = 1.0;
            state.target_color = 0xFF0000; // Red
            state.text = "错误".to_string();
        }
        IndicatorState::Cancelled => {
            state.target_alpha = 1.0;
            state.target_color = 0xFFD700; // Gold/Yellow
            state.text = "已取消".to_string();
        }
    }

    // Recalculate width based on text
    // Fixed width for now for stability, or dynamic:
    // state.width = 160;
}

#[cfg(target_os = "windows")]
fn update_animation(state: &mut WindowState) -> bool {
    let mut changed = false;

    // Lerp alpha
    let alpha_diff = state.target_alpha - state.current_alpha;
    if alpha_diff.abs() > 0.01 {
        // 基于fade_duration计算平滑因子，确保动画时间符合配置
        let smooth_factor = 16.0 / state.fade_duration as f32 * 10.0;
        state.current_alpha += alpha_diff * smooth_factor;
        changed = true;
    } else {
        state.current_alpha = state.target_alpha;
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

    let w = state.width;
    let h = state.height;

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
    // pixels.fill(0); // Already zero initialized? Usually yes but safer to fill.
    for p in pixels.iter_mut() {
        *p = 0;
    }

    // Constants
    let radius = 16.0; // Corner radius
    let padding = 20.0;
    let dot_radius = 5.0; // 10px diameter
    let dot_x = padding + dot_radius;
    let dot_y = h as f32 / 2.0;

    // Background Color (Black #000000 with some opacity)
    // User asked for black background. Opacity is not strictly defined for bg, but "semi-transparent" implied?
    // User: "悬浮窗整体背景色设置为黑色... 指示灯...透明度70-80%... 窗口...淡入淡出"
    // Let's make the background opaque black for high contrast as requested, but the whole window fades.
    // Wait, "半透明效果" in technical requirements implies the window itself might be semi-transparent.
    // Let's use 220 alpha for background to look "modern".
    let bg_alpha = 220u32;

    // Draw Background (Rounded Rect)
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;

            // Signed distance to rounded rect
            // Rect: 0,0, w, h. Radius r.
            // Symmetry: map x,y to top-left quadrant relative to center
            let cx = w as f32 / 2.0;
            let cy = h as f32 / 2.0;
            let px = (x as f32 - cx).abs();
            let py = (y as f32 - cy).abs();

            let qw = w as f32 / 2.0 - radius;
            let qh = h as f32 / 2.0 - radius;

            let dx = (px - qw).max(0.0);
            let dy = (py - qh).max(0.0);
            let dist = (dx * dx + dy * dy).sqrt();

            // Alpha based on distance (Anti-aliasing)
            // if dist > radius -> outside.
            // alpha = clamp(radius - dist + 0.5, 0, 1)
            let alpha_f = (radius - dist + 0.5).clamp(0.0, 1.0);

            if alpha_f > 0.0 {
                let pixel_alpha = (alpha_f * bg_alpha as f32) as u32;
                // Pre-multiplied alpha (Black is 0,0,0 so just Alpha channel matters)
                pixels[idx] = pixel_alpha << 24;
            }
        }
    }

    // Draw Indicator Dot
    // Color: state.current_color
    // Alpha: 70-80% -> ~190
    let dot_color = state.current_color;
    let dot_alpha = 190u32;
    let dr = (dot_color >> 16) & 0xFF;
    let dg = (dot_color >> 8) & 0xFF;
    let db = dot_color & 0xFF;

    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - dot_x;
            let dy = y as f32 - dot_y;
            let dist = (dx * dx + dy * dy).sqrt();

            let alpha_f = (dot_radius - dist + 0.5).clamp(0.0, 1.0);
            if alpha_f > 0.0 {
                let idx = (y * w + x) as usize;
                let bg_val = pixels[idx];
                let _bg_a = (bg_val >> 24) & 0xFF;

                // Simple blend over black background
                let final_a = (alpha_f * dot_alpha as f32) as u32;

                // Premultiply
                let r = (dr * final_a) / 255;
                let g = (dg * final_a) / 255;
                let b = (db * final_a) / 255;

                // Composite over existing background (Painter's algorithm)
                // Src: (r,g,b, final_a), Dst: (0,0,0, bg_a)
                // OutA = SrcA + DstA * (1 - SrcA)
                // OutC = SrcC + DstC * (1 - SrcA)

                // Simplified: just set it since dot is on top and opaque-ish
                pixels[idx] = (final_a << 24) | (r << 16) | (g << 8) | b;
            }
        }
    }

    // Draw Text using GDI
    // We draw to the DC, then fix up alpha
    SetBkMode(hdc_mem, TRANSPARENT);
    SetTextColor(hdc_mem, COLORREF(0x00FFFFFF)); // White

    let font_height = 18; // 14pt approx
    let font = CreateFontW(
        -font_height,
        0,
        0,
        0,
        FW_BOLD.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        w!("Microsoft YaHei UI"),
    );
    let old_font = SelectObject(hdc_mem, font);

    let mut text_rect = RECT {
        left: (dot_x + dot_radius + 15.0) as i32,
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

    // Fix up alpha for text
    // GDI draws RGB but often leaves Alpha=0 for text.
    // We assume white text. If pixel is White (or gray for AA), we bump Alpha.
    // Since background is Black (0,0,0), any non-zero channel implies text or dot.
    // Dot region is known, we can skip it or just process everything.
    // Text area: text_rect.
    for y in text_rect.top..text_rect.bottom {
        for x in text_rect.left..text_rect.right {
            if x >= 0 && x < w && y >= 0 && y < h {
                let idx = (y * w + x as i32) as usize;
                let val = pixels[idx];
                let r = (val >> 16) & 0xFF;
                let g = (val >> 8) & 0xFF;
                let b = val & 0xFF;

                // If it has color (white text), ensure it's visible
                // Simple heuristic: max(r,g,b) > 0 -> it's text (or background/dot)
                // Background is black (0,0,0) with Alpha.
                // Wait, background pixels are (0,0,0) with Alpha=bg_alpha.
                // Text pixels drawn by GDI will be (255,255,255) with Alpha=0 (usually).
                // So if we see R/G/B > 0, it is text (since background is black).
                // (Dot is also colored, but we already drew it with alpha).

                if r > 0 || g > 0 || b > 0 {
                    // Check if it's the dot (we know dot area) or just trust the loop order?
                    // Text is drawn AFTER dot.
                    // GDI overwrites buffer.
                    // If GDI wrote White (255,255,255), we need to set Alpha=255.
                    // AA pixels will be Gray (v,v,v). We set Alpha=v?
                    // Yes, for white text on transparent, Alpha should roughly equal Luma.
                    // But we want it on top of the black background we already drew?
                    // Actually GDI drawing blends with the *existing* buffer if we didn't clear it?
                    // No, standard GDI operations on DIBSection are read-modify-write.
                    // But Text drawing might ignore alpha.

                    // Let's assume text pixels are meant to be opaque white.
                    // We just take the max channel as the alpha for the text part.
                    let max_c = r.max(g).max(b);
                    if max_c > 0 {
                        // This is text.
                        // Set Alpha to max_c (so full white = 255 alpha, gray = partial)
                        // And keep the RGB (pre-multiplied logic holds since R=G=B for white/gray)
                        pixels[idx] = (max_c << 24) | (val & 0x00FFFFFF);
                    }
                } else {
                    // It's black. Could be background.
                    // If it was background, we already set alpha.
                    // If GDI drew black text... we use white text.
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
