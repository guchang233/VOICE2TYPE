use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::Foundation::*,
    Win32::Graphics::Gdi::*,
    Win32::UI::WindowsAndMessaging::*,
    Win32::System::LibraryLoader::GetModuleHandleW,
};

#[derive(Debug, Clone, PartialEq)]
pub enum IndicatorState {
    Hidden,
    Recording,
    Processing,
    Success,
    Error,
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
unsafe fn create_and_run_window(rx: Receiver<IndicatorState>) {
    let instance = GetModuleHandleW(None).unwrap();
    let class_name = w!("Voice2TypeIndicatorClass");

    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
        hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
        ..Default::default()
    };

    RegisterClassW(&wc);

    // 计算屏幕中心位置
    let screen_width = GetSystemMetrics(SM_CXSCREEN);
    let window_width = 200;
    let window_height = 40;
    let x = (screen_width - window_width) / 2;
    let y = 10; // 顶部距离

    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
        class_name,
        w!("Voice2Type Indicator"),
        WS_POPUP | WS_VISIBLE,
        x, y, window_width, window_height,
        None,
        None,
        instance,
        None,
    );

    // 初始状态隐藏
    // SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
    
    let mut current_state = IndicatorState::Hidden;
    
    // 简单的消息循环，结合 Receiver 检查
    // 由于 Windows 消息循环是阻塞的，我们需要一种方式来处理 rx 消息
    // 可以使用 SetTimer 定期检查 rx
    SetTimer(hwnd, 1, 16, None); // ~60fps check

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        if msg.message == WM_TIMER && msg.wParam.0 == 1 {
            // 检查状态更新
            if let Ok(new_state) = rx.try_recv() {
                if new_state != current_state {
                    current_state = new_state;
                    update_window_visuals(hwnd, &current_state, window_width, window_height);
                }
            }
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(target_os = "windows")]
unsafe fn update_window_visuals(hwnd: HWND, state: &IndicatorState, width: i32, height: i32) {
    if *state == IndicatorState::Hidden {
        ShowWindow(hwnd, SW_HIDE);
        return;
    }

    // 准备 GDI 绘图
    let hdc_screen = GetDC(None);
    let hdc_mem = CreateCompatibleDC(hdc_screen);
    let hbitmap = CreateCompatibleBitmap(hdc_screen, width, height);
    let old_bitmap = SelectObject(hdc_mem, hbitmap);

    // 绘制背景
    // 为了实现圆角和透明，我们需要用特定的颜色填充背景，然后将其设为透明色
    // 但 UpdateLayeredWindow 支持 per-pixel alpha，这更适合现代化 UI
    
    // 这里使用简单的 GDI 绘图和 UpdateLayeredWindow
    
    let (bg_color, text) = match state {
        IndicatorState::Recording => (0x000000FF, "正在听..."), // 红色 (BGR)
        IndicatorState::Processing => (0x0000FFFF, "转换中..."), // 黄色
        IndicatorState::Success => (0x0000FF00, "已完成"), // 绿色
        IndicatorState::Error => (0x00FF0000, "错误"), // 蓝色 (错误)
        IndicatorState::Hidden => (0, ""),
    };

    // 绘制背景 (纯色圆角矩形)
    // 使用 GDI 绘制比较麻烦，这里简化为填充矩形
    let brush = CreateSolidBrush(COLORREF(bg_color));
    let rect = RECT { left: 0, top: 0, right: width, bottom: height };
    FillRect(hdc_mem, &rect, brush);
    DeleteObject(brush);

    // 绘制文字
    SetBkMode(hdc_mem, TRANSPARENT);
    SetTextColor(hdc_mem, COLORREF(0x00FFFFFF)); // 白色文字
    
    // 创建字体
    let font_height = -16;
    let font = CreateFontW(
        font_height, 0, 0, 0, FW_BOLD.0 as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32, DEFAULT_PITCH.0 as u32, w!("Microsoft YaHei UI")
    );
    let old_font = SelectObject(hdc_mem, font);

    let mut text_rect = rect;
    let mut text_wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    DrawTextW(hdc_mem, &mut text_wide, &mut text_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

    SelectObject(hdc_mem, old_font);
    DeleteObject(font);

    // 更新分层窗口
    let pt_src = POINT { x: 0, y: 0 };
    let size = SIZE { cx: width, cy: height };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 230, // 稍微透明一点
        AlphaFormat: 0, // AC_SRC_ALPHA 需要 32-bit bitmap with alpha channel，GDI 默认不支持自动 alpha，所以这里用 SourceConstantAlpha
    };
    
    // 注意：如果想要真正的圆角透明，需要 AlphaFormat = AC_SRC_ALPHA 并且自己处理 Bitmap 的 Alpha 通道
    // 为了简单起见，这里先做成半透明矩形。如果需要圆角，可以用 SetWindowRgn
    
    // 应用圆角 Region
    let rgn = CreateRoundRectRgn(0, 0, width, height, 20, 20);
    SetWindowRgn(hwnd, rgn, true);
    // 注意：SetWindowRgn 和 UpdateLayeredWindow 有时会有冲突，但在这种模式下（非 per-pixel alpha）应该可以

    UpdateLayeredWindow(
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

    // 清理
    SelectObject(hdc_mem, old_bitmap);
    DeleteObject(hbitmap);
    DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
}
