use native_windows_gui as nwg;
use native_windows_gui::NativeUi;
use native_windows_derive::NwgUi;
use std::cell::RefCell;
use std::sync::Arc;
use crate::config::ConfigManager;
use semver::Version;
use serde::Deserialize;

#[cfg(target_os = "windows")]
use windows::Win32::System::Console::GetConsoleWindow;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::ShowWindow;

use self::voice2_type_app_ui::Voice2TypeAppUi;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_OWNER: &str = "guchang233"; // 如果不同，请替换为您的实际用户名
const REPO_NAME: &str = "VOICE2TYPE";

#[derive(Default, NwgUi)]
pub struct Voice2TypeApp {
    // 1x1 像素透明锚点窗口
    // Flags: POPUP (无边框/标题), VISIBLE (使其存在), 禁用任务栏 (工具窗口)
    // 注意: 我们故意移除了 VISIBLE 以防止它出现在任务栏中。
    // NWG 仍然会创建窗口句柄，这对于托盘图标来说足够了。
    // ex_flags: 0x80 = WS_EX_TOOLWINDOW (防止显示在任务栏)
    #[nwg_control(size: (1, 1), position: (0, 0), flags: "POPUP", ex_flags: 0x80)]
    #[nwg_events( OnWindowClose: [Voice2TypeApp::quit] )]
    pub window: nwg::Window,

    // 系统托盘
    #[nwg_control(icon: Some(&data.icon), tip: Some("Voice2Type 助手"))]
    #[nwg_events(MousePressLeftUp: [Voice2TypeApp::show_menu], OnContextMenu: [Voice2TypeApp::show_menu])]
    pub tray: nwg::TrayNotification,

    #[nwg_resource(source_file: Some("icon.ico"))]
    pub icon: nwg::Icon,

    // 托盘菜单
    #[nwg_control(parent: window, popup: true)]
    pub tray_menu: nwg::Menu,

    #[nwg_control(parent: tray_menu, text: "设置")]
    pub settings_menu: nwg::Menu,

    #[nwg_control(parent: settings_menu, text: "允许输出 Emoji", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_emoji])]
    pub allow_emoji_item: nwg::MenuItem,

    #[nwg_control(parent: settings_menu, text: "允许输出标点", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_punctuation])]
    pub allow_punct_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu, text: "语言")]
    pub lang_menu: nwg::Menu,

    #[nwg_control(parent: lang_menu, text: "中文", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_lang_zh])]
    pub lang_zh_item: nwg::MenuItem,

    #[nwg_control(parent: lang_menu, text: "English", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_lang_en])]
    pub lang_en_item: nwg::MenuItem,

    // 配置子菜单
    #[nwg_control(parent: tray_menu, text: "配置")]
    pub config_menu: nwg::Menu,

    #[nwg_control(parent: config_menu, text: "API Key...")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_config_window])]
    pub config_api_item: nwg::MenuItem,

    #[nwg_control(parent: config_menu, text: "编辑配置")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::open_config_dir])]
    pub config_file_item: nwg::MenuItem,

    #[nwg_control(parent: config_menu, text: "显示日志", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_log])]
    pub log_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    pub sep_update: nwg::MenuSeparator,

    #[nwg_control(parent: tray_menu, text: "检查更新")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::check_update])]
    pub update_item: nwg::MenuItem,

    // 其他项
    #[nwg_control(parent: tray_menu, text: "关于")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_about])]
    pub about_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    pub sep: nwg::MenuSeparator,

    #[nwg_control(parent: tray_menu, text: "退出")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::quit])]
    pub quit_item: nwg::MenuItem,

    // --- 配置窗口 ---
    #[nwg_control(size: (400, 150), position: (300, 300), title: "Voice2Type API 配置", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_config_window])]
    pub config_window: nwg::Window, // 初始时我们希望它隐藏，在 init 中处理

    #[nwg_layout(parent: config_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub config_layout: nwg::GridLayout,

    #[nwg_control(parent: config_window, text: "API Key:")]
    #[nwg_layout_item(layout: config_layout, row: 0, col: 0)]
    pub api_label: nwg::Label,

    #[nwg_control(parent: config_window, text: "")] // 将在 init 中设置
    #[nwg_layout_item(layout: config_layout, row: 0, col: 1, col_span: 2)]
    pub api_input: nwg::TextInput,

    #[nwg_control(parent: config_window, text: "保存", position: (290, 70), size: (90, 30))]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_config])]
    pub save_btn: nwg::Button,
    
    // 状态
    pub config_manager: RefCell<Option<Arc<ConfigManager>>>,
}

impl Voice2TypeApp {
    pub fn init(config_manager: Arc<ConfigManager>) -> Voice2TypeAppUi {
        // 如果图标文件不存在，NWG 可能会 panic 或编译失败。
        // 这里假设图标在构建时已正确打包。
        
        let app_ui = Voice2TypeApp::build_ui(Default::default()).expect("无法构建 UI");
        let app = &app_ui;
        
        // 初始化状态
        *app.config_manager.borrow_mut() = Some(config_manager.clone());
        
        // Set initial values
        app.allow_emoji_item.set_checked(config_manager.allow_emoji());
        app.allow_punct_item.set_checked(config_manager.allow_punctuation());
        
        let show_log = config_manager.show_log();
        app.log_item.set_checked(show_log);
        Self::set_console_visibility(show_log);

        let lang = config_manager.language();
        // app.update_ui_text(&lang); // Removed dynamic update
        
        if lang == "en" {
            app.lang_en_item.set_checked(true);
            app.lang_zh_item.set_checked(false);
        } else {
            app.lang_zh_item.set_checked(true);
            app.lang_en_item.set_checked(false);
        }

        // 初始隐藏配置窗口
        app.config_window.set_visible(false);

        // 确保主锚点窗口也不可见
        app.window.set_visible(false);
        
        // 设置输入框文本
        app.api_input.set_text(&config_manager.get_api_key());

        // 检查 API Key 是否为空 (首次运行)
        if config_manager.get_api_key().is_empty() {
            let msg = "哎呀，还没检测到 API Key 呢！\n\n为了能听懂您说话，咱得去整一个 SiliconFlow 的 API Key。\n\n点击“确定”后，我会帮您打开配置窗口，顺便再帮您打开注册页面。\n去注册个账号，白嫖一个免费的 Key 填进来呗？";
            nwg::simple_message("温馨提示", msg);
            let _ = open::that("https://cloud.siliconflow.cn/account/ak");
            app.config_window.set_visible(true);
        }
        
        app_ui
    }

    fn set_lang_zh(&self) {
        if !self.lang_zh_item.checked() {
            self.lang_zh_item.set_checked(true);
            self.lang_en_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_language("zh".to_string());
                let _ = mgr.save();
            }
            // self.update_ui_text("zh"); // nwg不支持动态修改Menu文本
            nwg::simple_message("语言已切换", "语言已切换为中文 (重启程序后生效)");
        }
    }

    fn set_lang_en(&self) {
        if !self.lang_en_item.checked() {
            self.lang_en_item.set_checked(true);
            self.lang_zh_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_language("en".to_string());
                let _ = mgr.save();
            }
            // self.update_ui_text("en"); // nwg不支持动态修改Menu文本
            nwg::simple_message("Language Changed", "Language changed to English (Please restart the app)");
        }
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
            let hwnd = GetConsoleWindow();
            if hwnd.0 != 0 {
                if visible {
                    ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOW);
                } else {
                    ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
                }
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

    fn open_config_dir(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            let path = mgr.config_path();
            if let Some(parent) = path.parent() {
                let _ = open::that(parent);
            } else {
                // 如果没有父目录，尝试打开文件本身或当前目录
                let _ = open::that(".");
            }
        }
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
        nwg::simple_message("已保存", "API Key 已成功保存！");
        self.config_window.set_visible(false);
    }

    fn check_update(&self) {
        let current_version = CURRENT_VERSION.to_string();
        std::thread::spawn(move || {
            match Self::fetch_latest_version() {
                Ok(latest_version_str) => {
                    let current = Version::parse(&current_version).unwrap_or_else(|_| Version::new(0, 0, 0));
                    // 移除 'v' 前缀
                    let clean_latest = latest_version_str.trim_start_matches('v');
                    let latest = Version::parse(clean_latest).unwrap_or_else(|_| Version::new(0, 0, 0));

                    if latest > current {
                         nwg::simple_message("发现新版本", &format!("新版本 {} 已发布！\n当前版本: {}\n\n请前往 GitHub 下载。", latest_version_str, current_version));
                         let _ = open::that(format!("https://github.com/{}/{}/releases/latest", REPO_OWNER, REPO_NAME));
                    } else {
                         nwg::simple_message("无更新", &format!("当前已是最新版本 ({})。", current_version));
                    }
                },
                Err(e) => {
                     nwg::simple_message("检查更新失败", &format!("无法检查更新。\n错误: {}", e));
                }
            }
        });
    }

    fn fetch_latest_version() -> anyhow::Result<String> {
        #[derive(Deserialize)]
        struct Release {
            tag_name: String,
        }

        let client = reqwest::blocking::Client::builder()
            .user_agent("Voice2Type-App")
            .build()?;
        
        let url = format!("https://api.github.com/repos/{}/{}/releases/latest", REPO_OWNER, REPO_NAME);
        let resp = client.get(&url).send()?;
        
        if !resp.status().is_success() {
             anyhow::bail!("GitHub API request failed: {}", resp.status());
        }

        let release: Release = resp.json()?;
        Ok(release.tag_name)
    }

    fn show_about(&self) {
        let _ = open::that("https://github.com/guchang233/VOICE2TYPE");
    }

    fn quit(&self) {
        nwg::stop_thread_dispatch();
        std::process::exit(0);
    }
}
