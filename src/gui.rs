use native_windows_gui as nwg;
use native_windows_gui::NativeUi;
use native_windows_derive::NwgUi;
use std::cell::RefCell;
use std::sync::Arc;
use crate::config::ConfigManager;

#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AllocConsole, FreeConsole, GetConsoleWindow};
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::ShowWindow;

use self::voice2_type_app_ui::Voice2TypeAppUi;

#[derive(Default, NwgUi)]
pub struct Voice2TypeApp {
    // Shared state
    pub config_manager: RefCell<Option<Arc<ConfigManager>>>,

    // 1x1 Pixel Transparent Anchor Window
    // Flags: POPUP (no border/title), VISIBLE (so it exists), disabled taskbar (tool window)
    // Note: We intentionally remove VISIBLE to prevent it from showing in the taskbar.
    // NWG still creates the window handle, which is all we need for the tray icon.
    #[nwg_control(size: (1, 1), position: (0, 0), flags: "POPUP")]
    #[nwg_events( OnWindowClose: [Voice2TypeApp::quit] )]
    pub window: nwg::Window,

    // System Tray
    #[nwg_resource(source_system: Some(nwg::OemIcon::Information))] 
    pub icon: nwg::Icon,

    #[nwg_control(icon: Some(&data.icon), tip: Some("Voice2Type Assistant"))]
    #[nwg_events(MousePressLeftUp: [Voice2TypeApp::show_menu], OnContextMenu: [Voice2TypeApp::show_menu])]
    pub tray: nwg::TrayNotification,

    #[nwg_control(parent: window, popup: true)]
    pub tray_menu: nwg::Menu,

    // Settings Submenu
    #[nwg_control(parent: tray_menu, text: "设置 (Settings)")]
    pub settings_menu: nwg::Menu,

    #[nwg_control(parent: settings_menu, text: "允许输出 Emoji", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_emoji])]
    pub allow_emoji_item: nwg::MenuItem,

    #[nwg_control(parent: settings_menu, text: "允许输出标点", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_punctuation])]
    pub allow_punct_item: nwg::MenuItem,

    // Config Submenu
    #[nwg_control(parent: tray_menu, text: "配置 (Config)")]
    pub config_menu: nwg::Menu,

    #[nwg_control(parent: config_menu, text: "API Key...")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_config_window])]
    pub config_api_item: nwg::MenuItem,

    #[nwg_control(parent: config_menu, text: "显示日志 (Show Log)", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_log])]
    pub log_item: nwg::MenuItem,

    // Other Items
    #[nwg_control(parent: tray_menu, text: "关于 (About)")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_about])]
    pub about_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    pub sep: nwg::MenuSeparator,

    #[nwg_control(parent: tray_menu, text: "退出 (Quit)")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::quit])]
    pub quit_item: nwg::MenuItem,

    // --- Config Window ---
    #[nwg_control(size: (400, 150), position: (300, 300), title: "Voice2Type API Config", flags: "WINDOW|VISIBLE", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_config_window])]
    pub config_window: nwg::Window, // Initially we might want it hidden, handled in init

    #[nwg_control(parent: config_window, text: "SiliconFlow API Key:", position: (10, 10), size: (380, 20))]
    pub api_label: nwg::Label,

    #[nwg_control(parent: config_window, text: "", position: (10, 35), size: (370, 25), password: Some('*'))]
    pub api_input: nwg::TextInput,

    #[nwg_control(parent: config_window, text: "保存 (Save)", position: (290, 70), size: (90, 30))]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_config])]
    pub save_btn: nwg::Button,
}

impl Voice2TypeApp {
    pub fn init(config_manager: Arc<ConfigManager>) -> Voice2TypeAppUi {
        // Need to create a dummy icon file if it doesn't exist, otherwise nwg might panic or fail to compile if include_bytes fails
        // For now, let's assume we handle the icon logic in build or assume it exists. 
        // Actually, include_bytes! requires the file to exist at compile time.
        // I will create a dummy icon.ico before compiling.
        
        let app_ui = Voice2TypeApp::build_ui(Default::default()).expect("Failed to build UI");
        let app = &app_ui;
        
        // Init state
        *app.config_manager.borrow_mut() = Some(config_manager.clone());
        
        // Set initial values
        app.allow_emoji_item.set_checked(config_manager.allow_emoji());
        app.allow_punct_item.set_checked(config_manager.allow_punctuation());
        
        let show_log = config_manager.show_log();
        app.log_item.set_checked(show_log);
        Self::set_console_visibility(show_log);

        // Hide config window initially
        app.config_window.set_visible(false);
        
        // Set input text
        app.api_input.set_text(&config_manager.get_api_key());
        
        app_ui
    }

    fn show_menu(&self) {
        let (x, y) = nwg::GlobalCursor::position();
        self.tray_menu.popup(x, y);
    }

    fn toggle_emoji(&self) {
        let new_state = !self.allow_emoji_item.checked();
        self.allow_emoji_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_allow_emoji(new_state);
            let _ = mgr.save();
        }
    }

    fn toggle_punctuation(&self) {
        let new_state = !self.allow_punct_item.checked();
        self.allow_punct_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_allow_punctuation(new_state);
            let _ = mgr.save();
        }
    }

    fn toggle_log(&self) {
        let new_state = !self.log_item.checked();
        self.log_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_show_log(new_state);
            let _ = mgr.save();
        }
        Self::set_console_visibility(new_state);
    }

    fn set_console_visibility(visible: bool) {
        #[cfg(target_os = "windows")]
        unsafe {
            if visible {
                let hwnd = GetConsoleWindow();
                if hwnd.0 == 0 {
                    // No console attached, allocate one
                    AllocConsole();
                } else {
                    // Console exists, ensure it is visible
                    ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOW);
                }
            } else {
                // Hide console: FreeConsole is more reliable than ShowWindow(SW_HIDE)
                // It completely detaches the console window.
                FreeConsole();
            }
        }
    }

    fn show_config_window(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            self.api_input.set_text(&mgr.get_api_key());
        }
        self.config_window.set_visible(true);
        self.config_window.set_focus();
    }

    fn hide_config_window(&self) {
        self.config_window.set_visible(false);
    }

    fn save_config(&self) {
        let key = self.api_input.text();
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_api_key(key);
            let _ = mgr.save();
        }
        nwg::simple_message("Saved", "API Key saved successfully!");
        self.config_window.set_visible(false);
    }

    fn show_about(&self) {
        let _ = open::that("https://github.com/guchang233/VOICE2TYPE");
    }

    fn quit(&self) {
        nwg::stop_thread_dispatch();
        std::process::exit(0);
    }
}
