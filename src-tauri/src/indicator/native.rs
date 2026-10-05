use super::model::{IndicatorState, Model, Settings};
use super::render::{self, Palette, CARD_HEIGHT, CARD_WIDTH, PADDING};
use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Instant;
use windows::{
    core::{w, Error, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{
            LibraryLoader::GetModuleHandleW,
            Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
            Threading::{CreateEventW, GetCurrentThreadId, SetEvent, INFINITE},
        },
        UI::{
            HiDpi::{
                GetDpiForWindow, SetThreadDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            WindowsAndMessaging::*,
        },
    },
};

pub enum Command {
    State(IndicatorState),
    Configure(Settings),
    #[cfg(test)]
    Barrier(Sender<isize>),
}

/// An auto-reset event wakes the message-aware wait after commands or sender teardown.
struct WakeEvent(HANDLE);
// SAFETY: Windows event handles support cross-thread SetEvent/wait operations. Arc keeps
// the handle open until both the worker's wait and every sender have finished using it.
unsafe impl Send for WakeEvent {}
unsafe impl Sync for WakeEvent {}

impl WakeEvent {
    fn signal(&self) {
        unsafe {
            let _ = SetEvent(self.0);
        }
    }
}
impl Drop for WakeEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Clone)]
pub struct IndicatorSender {
    tx: Option<Sender<Command>>,
    wake: Arc<WakeEvent>,
}
impl IndicatorSender {
    pub fn send(&self, command: Command) {
        if self.tx.as_ref().is_some_and(|tx| tx.send(command).is_ok()) {
            self.wake.signal();
        }
    }
}
impl Drop for IndicatorSender {
    fn drop(&mut self) {
        // Disconnect before waking: the final sender must let the worker observe EOF.
        self.tx.take();
        self.wake.signal();
    }
}

pub fn start(settings: Settings) -> anyhow::Result<IndicatorSender> {
    let wake = Arc::new(WakeEvent(unsafe {
        CreateEventW(None, false, false, None)?
    }));
    let (tx, rx) = channel();
    let worker_wake = wake.clone();
    std::thread::Builder::new()
        .name("status-indicator".into())
        .spawn(move || {
            // SAFETY: all HWND/GDI objects are created, used and destroyed on this thread.
            if let Err(error) = unsafe { run(rx, worker_wake, settings) } {
                log::error!("状态浮层运行失败: {error:#}");
            }
        })?;
    Ok(IndicatorSender { tx: Some(tx), wake })
}

struct Window {
    hwnd: HWND,
    instance: HINSTANCE,
    class: Vec<u16>,
}
impl Window {
    unsafe fn new() -> anyhow::Result<Self> {
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let class: Vec<u16> = format!("Voice2Type.Status.{}", GetCurrentThreadId())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        anyhow::ensure!(
            RegisterClassW(&wc) != 0,
            "RegisterClassW: {}",
            Error::from_win32()
        );
        let mut window = Self {
            hwnd: HWND(0),
            instance,
            class,
        };
        window.hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            PCWSTR(window.class.as_ptr()),
            w!("Voice2Type 状态"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            instance,
            None,
        );
        anyhow::ensure!(
            window.hwnd.0 != 0,
            "CreateWindowExW: {}",
            Error::from_win32()
        );
        Ok(window)
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            if self.hwnd.0 != 0 {
                let _ = DestroyWindow(self.hwnd);
            }
            let _ = UnregisterClassW(PCWSTR(self.class.as_ptr()), self.instance);
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// An exact-size top-down DIB; its row stride is always width * 4 bytes.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    width: usize,
    height: usize,
}
impl Surface {
    unsafe fn new(width: usize, height: usize) -> anyhow::Result<Self> {
        let dc = CreateCompatibleDC(None);
        anyhow::ensure!(dc.0 != 0, "CreateCompatibleDC: {}", Error::from_win32());
        let mut surface = Self {
            dc,
            bitmap: HBITMAP(0),
            previous: HGDIOBJ(0),
            bits: std::ptr::null_mut(),
            width,
            height,
        };
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        surface.bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        anyhow::ensure!(!bits.is_null(), "CreateDIBSection returned no pixel buffer");
        surface.bits = bits.cast();
        surface.previous = SelectObject(dc, surface.bitmap);
        anyhow::ensure!(
            surface.previous.0 != 0 && surface.previous.0 != -1,
            "SelectObject failed"
        );
        Ok(surface)
    }
    unsafe fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: CreateDIBSection allocated width * height 32-bit pixels; this surface
        // exclusively owns them. GDI writes are flushed before accessing mask pixels.
        std::slice::from_raw_parts_mut(self.bits, self.width * self.height)
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if self.previous.0 != 0 && self.previous.0 != -1 {
                SelectObject(self.dc, self.previous);
            }
            if self.bitmap.0 != 0 {
                DeleteObject(self.bitmap);
            }
            DeleteDC(self.dc);
        }
    }
}

struct Font(HFONT);
impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

struct Frame {
    layer: Surface,
    mask: Surface,
    font: Font,
    scale: f32,
    label: &'static str,
}
impl Frame {
    unsafe fn new(dpi: u32) -> anyhow::Result<Self> {
        let scale = dpi as f32 / 96.0;
        let width = ((CARD_WIDTH + PADDING * 2.0) * scale).ceil() as usize;
        let height = ((CARD_HEIGHT + PADDING * 2.0) * scale).ceil() as usize;
        let layer = Surface::new(width, height)?;
        let mut mask = Surface::new(width, height)?;
        mask.pixels().fill(0);
        let font = Font(CreateFontW(
            -(13.0 * scale).round() as i32,
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
            ANTIALIASED_QUALITY.0 as u32,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI Variable Text"),
        ));
        anyhow::ensure!(font.0 .0 != 0, "CreateFontW: {}", Error::from_win32());
        SetBkMode(mask.dc, TRANSPARENT);
        SetTextColor(mask.dc, COLORREF(0x00ffffff));
        Ok(Self {
            layer,
            mask,
            font,
            scale,
            label: "",
        })
    }

    unsafe fn render(
        &mut self,
        state: IndicatorState,
        seconds: f32,
        palette: Palette,
    ) -> anyhow::Result<()> {
        let (width, height) = (self.layer.width, self.layer.height);
        render::paint(
            self.layer.pixels(),
            width,
            width,
            height,
            self.scale,
            palette,
            state,
            seconds,
        );
        let label = state.label();
        if label != self.label {
            self.mask.pixels().fill(0);
            let old_font = SelectObject(self.mask.dc, self.font.0);
            let mut rect = RECT {
                left: ((PADDING + 48.0) * self.scale).round() as i32,
                top: (PADDING * self.scale).round() as i32,
                right: ((PADDING + CARD_WIDTH - 14.0) * self.scale).round() as i32,
                bottom: ((PADDING + CARD_HEIGHT) * self.scale).round() as i32,
            };
            let mut text: Vec<u16> = label.encode_utf16().collect();
            let drawn = DrawTextW(
                self.mask.dc,
                &mut text,
                &mut rect,
                DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            // CPU access to DIB bits must follow GdiFlush, including on a cached text mask.
            GdiFlush();
            SelectObject(self.mask.dc, old_font);
            anyhow::ensure!(drawn > 0, "DrawTextW failed");
            self.label = label;
        }
        render::composite_text(self.layer.pixels(), self.mask.pixels(), palette.text);
        Ok(())
    }

    unsafe fn draw(
        &mut self,
        hwnd: HWND,
        model: &Model,
        palette: Palette,
        now: Instant,
        work: RECT,
    ) -> anyhow::Result<()> {
        if model.alpha == 0.0 {
            ShowWindow(hwnd, SW_HIDE);
            return Ok(());
        }
        self.render(model.displayed, model.elapsed(now).as_secs_f32(), palette)?;
        let (width, height) = (self.layer.width, self.layer.height);
        let size = SIZE {
            cx: width as i32,
            cy: height as i32,
        };
        let position = POINT {
            x: work.left + (work.right - work.left - size.cx) / 2,
            y: (work.top + (48.0 * self.scale).round() as i32
                - (PADDING * self.scale).round() as i32)
                .min(work.bottom - size.cy)
                .max(work.top),
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (model.alpha * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hwnd,
            None,
            Some(&position),
            Some(&size),
            self.layer.dc,
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )?;
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(())
    }
}

unsafe fn system_light() -> bool {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    RegGetValueW(
        HKEY_CURRENT_USER,
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("AppsUseLightTheme"),
        RRF_RT_REG_DWORD,
        None,
        Some((&mut value as *mut u32).cast()),
        Some(&mut size),
    ) == ERROR_SUCCESS
        && value != 0
}

unsafe fn work_area(monitor: HMONITOR) -> anyhow::Result<RECT> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    anyhow::ensure!(
        GetMonitorInfoW(monitor, &mut info).as_bool(),
        "GetMonitorInfoW: {}",
        Error::from_win32()
    );
    Ok(info.rcWork)
}

unsafe fn run(
    rx: Receiver<Command>,
    wake: Arc<WakeEvent>,
    settings: Settings,
) -> anyhow::Result<()> {
    // DPI awareness is thread-local; it does not change the main WebView's scaling.
    SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let window = Window::new()?;
    let hwnd = window.hwnd;
    let mut model = Model::new(settings, Instant::now());
    let mut monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
    let mut work = work_area(monitor)?;
    SetWindowPos(
        hwnd,
        None,
        work.left,
        work.top,
        1,
        1,
        SWP_NOACTIVATE | SWP_NOZORDER,
    )?;
    let mut dpi = GetDpiForWindow(hwnd).max(96);
    let mut frame = Frame::new(dpi)?;
    let mut light = system_light();
    let mut dirty = false;
    let mut last_frame = Instant::now();
    loop {
        let now = Instant::now();
        let mut choose_monitor = false;
        loop {
            match rx.try_recv() {
                Ok(Command::State(state)) => {
                    choose_monitor |= !model.state.active() && state.active();
                    model.set_state(state, now);
                    dirty = true;
                }
                Ok(Command::Configure(settings)) => {
                    model.configure(settings, now);
                    dirty = true;
                }
                #[cfg(test)]
                Ok(Command::Barrier(ready)) => {
                    let _ = ready.send(hwnd.0);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        let mut message = MSG::default();
        let mut refresh_environment = choose_monitor;
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            if message.message == WM_QUIT {
                return Ok(());
            }
            if matches!(
                message.message,
                WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE | WM_THEMECHANGED
            ) {
                refresh_environment = true;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if refresh_environment {
            if choose_monitor {
                monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONEAREST);
            }
            work = work_area(monitor).or_else(|_| {
                monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
                work_area(monitor)
            })?;
            // Move before querying DPI so each recording uses its target monitor's scale.
            SetWindowPos(
                hwnd,
                None,
                work.left,
                work.top,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE,
            )?;
            let next_dpi = GetDpiForWindow(hwnd).max(96);
            if next_dpi != dpi {
                frame = Frame::new(next_dpi)?;
                dpi = next_dpi;
            }
            light = system_light();
            dirty = true;
        }
        dirty |= model.tick(now);
        let active_frame = model.alpha > 0.0
            && model.state.active()
            && last_frame.elapsed() >= super::model::FRAME;
        if dirty || active_frame {
            frame.draw(
                hwnd,
                &model,
                Palette::for_theme(&model.settings.theme, light),
                now,
                work,
            )?;
            dirty = false;
            last_frame = Instant::now();
        }
        let timeout = model.wait(Instant::now()).map_or(INFINITE, |duration| {
            // Round up sub-millisecond deadlines; reserve INFINITE for true idle waits.
            duration
                .as_millis()
                .saturating_add(u128::from(duration.subsec_nanos() % 1_000_000 != 0))
                .min((INFINITE - 1) as u128) as u32
        });
        let result =
            MsgWaitForMultipleObjectsEx(Some(&[wake.0]), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        anyhow::ensure!(
            result != WAIT_FAILED,
            "MsgWaitForMultipleObjectsEx: {}",
            Error::from_win32()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn render_all_native_labels_and_optional_preview() {
        unsafe {
            let states = [
                IndicatorState::Recording,
                IndicatorState::Processing,
                IndicatorState::Success,
                IndicatorState::Error,
                IndicatorState::Cancelled,
            ];
            let mut frame = Frame::new(192).unwrap();
            let (width, height) = (frame.layer.width, frame.layer.height);
            let board_width = width * states.len();
            let board_height = height * 3;
            let mut board = vec![0u32; board_width * board_height];
            for (row, theme) in ["dark", "light", "eye-care"].iter().enumerate() {
                let palette = Palette::for_theme(theme, false);
                let background: u32 = match *theme {
                    "dark" => 0x000f0f12,
                    "light" => 0x00f4f4f7,
                    _ => 0x00e9e2d1,
                };
                for (column, state) in states.iter().enumerate() {
                    frame.render(*state, 0.3, palette).unwrap();
                    assert!(frame
                        .mask
                        .pixels()
                        .iter()
                        .any(|pixel| pixel & 0xffffff != 0));
                    for (index, pixel) in frame.layer.pixels().iter().enumerate() {
                        let inverse = 255 - (pixel >> 24);
                        let channel = |shift: u32| {
                            ((pixel >> shift) & 255)
                                + (((background >> shift) & 255) * inverse + 127) / 255
                        };
                        let target = (row * height + index / width) * board_width
                            + column * width
                            + index % width;
                        board[target] =
                            0xff000000 | (channel(16) << 16) | (channel(8) << 8) | channel(0);
                    }
                }
            }
            // Opt-in visual QA artifact; ordinary tests do not write workspace files.
            if let Some(path) = std::env::var_os("V2T_INDICATOR_PREVIEW") {
                let pixel_bytes = (board.len() * 4) as u32;
                let mut bmp = Vec::with_capacity(54 + pixel_bytes as usize);
                bmp.extend_from_slice(b"BM");
                bmp.extend_from_slice(&(54 + pixel_bytes).to_le_bytes());
                bmp.extend_from_slice(&[0; 4]);
                bmp.extend_from_slice(&54u32.to_le_bytes());
                bmp.extend_from_slice(&40u32.to_le_bytes());
                bmp.extend_from_slice(&(board_width as i32).to_le_bytes());
                bmp.extend_from_slice(&(-(board_height as i32)).to_le_bytes());
                bmp.extend_from_slice(&1u16.to_le_bytes());
                bmp.extend_from_slice(&32u16.to_le_bytes());
                bmp.extend_from_slice(&[0; 24]);
                for pixel in board {
                    bmp.extend_from_slice(&pixel.to_le_bytes());
                }
                std::fs::write(path, bmp).unwrap();
            }
        }
    }

    #[test]
    fn gdi_text_mask_renders_without_corrupting_alpha_surface() {
        unsafe {
            let mut frame = Frame::new(144).unwrap();
            let old_font = SelectObject(frame.mask.dc, frame.font.0);
            let mut rect = RECT {
                left: 72,
                top: 18,
                right: 234,
                bottom: 78,
            };
            let mut text: Vec<u16> = "识别失败".encode_utf16().collect();
            assert!(
                DrawTextW(
                    frame.mask.dc,
                    &mut text,
                    &mut rect,
                    DT_VCENTER | DT_SINGLELINE
                ) > 0
            );
            GdiFlush();
            SelectObject(frame.mask.dc, old_font);
            assert!(frame
                .mask
                .pixels()
                .iter()
                .any(|pixel| pixel & 0xffffff != 0));
            frame.layer.pixels().fill(0);
            render::composite_text(
                frame.layer.pixels(),
                frame.mask.pixels(),
                Palette::for_theme("light", false).text,
            );
            assert!(frame
                .layer
                .pixels()
                .iter()
                .all(|p| ((p >> 16) & 255) <= (p >> 24)));
        }
    }

    fn idle_worker() -> (
        IndicatorSender,
        HWND,
        Receiver<()>,
        std::thread::JoinHandle<()>,
    ) {
        let settings = Settings {
            enabled: true,
            theme: "dark".into(),
            fade: Duration::ZERO,
            success: Duration::ZERO,
            error: Duration::ZERO,
        };
        let wake = Arc::new(WakeEvent(unsafe {
            CreateEventW(None, false, false, None).unwrap()
        }));
        let (tx, rx) = channel();
        let sender = IndicatorSender {
            tx: Some(tx),
            wake: wake.clone(),
        };
        let (done_tx, done) = channel();
        let worker = std::thread::spawn(move || unsafe {
            run(rx, wake, settings).unwrap();
            done_tx.send(()).unwrap();
        });
        let (ready_tx, ready) = channel();
        sender.send(Command::Barrier(ready_tx));
        let hwnd = HWND(ready.recv_timeout(Duration::from_secs(5)).unwrap());
        (sender, hwnd, done, worker)
    }

    #[test]
    fn dropping_last_sender_wakes_idle_worker() {
        let (sender, _, done, worker) = idle_worker();
        drop(sender);
        done.recv_timeout(Duration::from_secs(5))
            .expect("idle worker must wake on disconnect");
        worker.join().unwrap();
    }

    #[test]
    fn hidden_window_processes_native_messages_without_state_commands() {
        let (sender, hwnd, done, worker) = idle_worker();
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            assert_ne!(style & WS_EX_NOACTIVATE.0, 0);
            assert_ne!(style & WS_EX_TRANSPARENT.0, 0);
            assert!(!IsWindowVisible(hwnd).as_bool());
            PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap();
        }
        done.recv_timeout(Duration::from_secs(5))
            .expect("hidden window must handle WM_CLOSE");
        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn frame_teardown_releases_selected_gdi_objects() {
        unsafe {
            let frame = Frame::new(144).unwrap();
            let layer_bitmap = frame.layer.bitmap;
            let mask_bitmap = frame.mask.bitmap;
            let font = frame.font.0;
            let dc = frame.layer.dc;
            // Query actual object data: type/required-size queries can succeed for
            // a stale GDI handle because the handle itself encodes its object type.
            let mut bitmap_info = BITMAP::default();
            let mut font_info = LOGFONTW::default();
            let bitmap_size = std::mem::size_of::<BITMAP>() as i32;
            let bitmap_buffer = Some((&mut bitmap_info as *mut BITMAP).cast());
            assert!(GetObjectW(layer_bitmap, bitmap_size, bitmap_buffer) > 0);
            drop(frame);
            GdiFlush();
            assert_eq!(GetObjectW(layer_bitmap, bitmap_size, bitmap_buffer), 0);
            assert_eq!(GetObjectW(mask_bitmap, bitmap_size, bitmap_buffer), 0);
            assert_eq!(
                GetObjectW(
                    font,
                    std::mem::size_of::<LOGFONTW>() as i32,
                    Some((&mut font_info as *mut LOGFONTW).cast())
                ),
                0
            );
            assert_eq!(GetCurrentObject(dc, OBJ_BITMAP).0, 0);
        }
    }

    #[test]
    fn layered_window_can_show_and_hide_without_taking_focus() {
        unsafe {
            let foreground = GetForegroundWindow();
            let window = Window::new().unwrap();
            let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTOPRIMARY);
            let work = work_area(monitor).unwrap();
            let now = Instant::now();
            let mut model = Model::new(
                Settings {
                    enabled: true,
                    theme: "dark".into(),
                    fade: Duration::ZERO,
                    success: Duration::from_secs(1),
                    error: Duration::from_secs(1),
                },
                now,
            );
            let mut frame = Frame::new(GetDpiForWindow(window.hwnd).max(96)).unwrap();
            let palette = Palette::for_theme("dark", false);
            model.set_state(IndicatorState::Recording, now);
            frame.draw(window.hwnd, &model, palette, now, work).unwrap();
            assert!(IsWindowVisible(window.hwnd).as_bool());
            assert_eq!(GetForegroundWindow(), foreground);
            model.set_state(IndicatorState::Hidden, now);
            frame.draw(window.hwnd, &model, palette, now, work).unwrap();
            assert!(!IsWindowVisible(window.hwnd).as_bool());
        }
    }
}
