use native_windows_gui as nwg;
use native_windows_gui::NativeUi;
use native_windows_derive::NwgUi;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use crate::config::ConfigManager;
use crate::update;

// 使用 UI 线程的 Timer 轮询同步托盘菜单勾选状态，避免跨线程持有句柄

#[cfg(target_os = "windows")]

use self::voice2_type_app_ui::Voice2TypeAppUi;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default, NwgUi)]
pub struct Voice2TypeApp {
    // 1x1 像素透明锚点窗口
    #[nwg_control(size: (1, 1), position: (0, 0), flags: "POPUP", ex_flags: 0x80)]
    #[nwg_events( OnWindowClose: [Voice2TypeApp::quit] )]
    pub window: nwg::Window,

    // 系统托盘
    #[nwg_control(icon: Some(&data.icon), tip: Some("Voice2Type 助手"))]
    #[nwg_events(MousePressLeftUp: [Voice2TypeApp::show_menu], OnContextMenu: [Voice2TypeApp::show_menu])]
    pub tray: nwg::TrayNotification,

    #[nwg_resource(source_bin: Some(include_bytes!("../icon.ico")))]
    pub icon: nwg::Icon,

    // 托盘菜单 (Root)
    #[nwg_control(parent: window, popup: true)]
    pub tray_menu: nwg::Menu,

    // 设置 (Top Level)
    #[nwg_control(parent: tray_menu, text: "设置")]
    pub settings_menu: nwg::Menu,

    // --- 设置 -> 触发模式 ---
    #[nwg_control(parent: settings_menu, text: "触发模式")]
    pub trigger_menu: nwg::Menu,

    #[nwg_control(parent: trigger_menu, text: "按住输入", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_trigger_hold])]
    pub trigger_hold_item: nwg::MenuItem,

    #[nwg_control(parent: trigger_menu, text: "按下输入", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_trigger_toggle])]
    pub trigger_toggle_item: nwg::MenuItem,

    // --- 设置 -> 通用 ---
    #[nwg_control(parent: settings_menu, text: "通用")]
    pub general_menu: nwg::Menu,

    #[nwg_control(parent: general_menu, text: "开机自启动", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_autostart])]
    pub autostart_item: nwg::MenuItem,

    #[nwg_control(parent: general_menu, text: "状态浮窗", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_indicator])]
    pub indicator_item: nwg::MenuItem,

    #[nwg_control(parent: general_menu)]
    pub sep_general: nwg::MenuSeparator,

    #[nwg_control(parent: general_menu, text: "允许输出标点", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_punctuation])]
    pub allow_punct_item: nwg::MenuItem,

    #[nwg_control(parent: general_menu, text: "允许输出表情", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_emoji])]
    pub allow_emoji_item: nwg::MenuItem,

    // --- 设置 -> 输出模式 ---
    #[nwg_control(parent: settings_menu, text: "输出方式")]
    pub output_menu: nwg::Menu,

    #[nwg_control(parent: output_menu, text: "注入(不推荐)", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_output_inject])]
    pub output_inject_item: nwg::MenuItem,

    #[nwg_control(parent: output_menu, text: "剪贴板(推荐)", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_output_clipboard])]
    pub output_clipboard_item: nwg::MenuItem,

    // --- 设置 -> 语言 ---
    #[nwg_control(parent: settings_menu, text: "语言")]
    pub lang_menu: nwg::Menu,

    #[nwg_control(parent: lang_menu, text: "界面语言")]
    pub interface_lang_menu: nwg::Menu,

    #[nwg_control(parent: interface_lang_menu, text: "中文", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_lang_zh])]
    pub lang_zh_item: nwg::MenuItem,

    #[nwg_control(parent: interface_lang_menu, text: "English", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_lang_en])]
    pub lang_en_item: nwg::MenuItem,

    #[nwg_control(parent: lang_menu, text: "输出语言")]
    pub output_lang_menu: nwg::Menu,

    #[nwg_control(parent: output_lang_menu, text: "自动 (Auto)", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_out_lang_auto])]
    pub output_lang_auto_item: nwg::MenuItem,

    #[nwg_control(parent: output_lang_menu, text: "中文 (Zh)", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_out_lang_zh])]
    pub output_lang_zh_item: nwg::MenuItem,

    #[nwg_control(parent: output_lang_menu, text: "English (En)", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_out_lang_en])]
    pub output_lang_en_item: nwg::MenuItem,

    // --- 设置 -> 配置 ---
    #[nwg_control(parent: settings_menu, text: "配置")]
    pub config_menu: nwg::Menu,

    #[nwg_control(parent: config_menu, text: "模型设置")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_config_window])]
    pub config_api_item: nwg::MenuItem,

    #[nwg_control(parent: config_menu, text: "热键绑定")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_hotkey_window])]
    pub hotkey_settings_item: nwg::MenuItem,

    #[nwg_control(parent: config_menu, text: "高级")]
    pub advanced_menu: nwg::Menu,

    #[nwg_control(parent: advanced_menu, text: "流式间隔设置")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_streaming_window])]
    pub streaming_interval_item: nwg::MenuItem,

    #[nwg_control(parent: advanced_menu, text: "VAD参数设置")]
    pub vad_settings_item: nwg::MenuItem,

    #[nwg_control(parent: advanced_menu, text: "指示器参数设置")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_indicator_window])]
    pub indicator_settings_item: nwg::MenuItem,

    #[nwg_control(parent: config_menu, text: "配置目录")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::open_config_dir])]
    pub config_file_item: nwg::MenuItem,

    // --- 设置 -> 调试 ---
    #[nwg_control(parent: settings_menu, text: "调试")]
    pub debug_menu: nwg::Menu,

    #[nwg_control(parent: debug_menu, text: "显示日志", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_log])]
    pub log_item: nwg::MenuItem,

    #[nwg_control(parent: debug_menu, text: "日志目录")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::open_log_dir])]
    pub log_dir_item: nwg::MenuItem,

    #[nwg_control(parent: debug_menu, text: "不稳定功能")]
    pub unstable_menu: nwg::Menu,

    #[nwg_control(parent: unstable_menu, text: "启用流式输出", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_streaming])]
    pub streaming_item: nwg::MenuItem,

    #[nwg_control(parent: unstable_menu, text: "启用VAD智能切片", check: true)]
    pub vad_item: nwg::MenuItem,

    // --- Root Level Items ---
    #[nwg_control(parent: tray_menu)]
    pub sep_update: nwg::MenuSeparator,

    // --- 关于父菜单 ---
    #[nwg_control(parent: tray_menu, text: "关于")]
    pub about_parent_menu: nwg::Menu,

    #[nwg_control(parent: about_parent_menu, text: "项目信息")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_about])]
    pub about_item: nwg::MenuItem,

    #[nwg_control(parent: about_parent_menu, text: "版本检测")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_update_window])]
    pub update_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    pub sep: nwg::MenuSeparator,

    #[nwg_control(parent: tray_menu, text: "退出")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::quit])]
    pub quit_item: nwg::MenuItem,

    // --- 模型配置窗口 ---
    #[nwg_control(size: (480, 240), position: (300, 300), title: "模型配置", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_config_window])]
    pub config_window: nwg::Window,

    #[nwg_layout(parent: config_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub config_layout: nwg::GridLayout,

    #[nwg_control(parent: config_window, text: "API Key:")]
    #[nwg_layout_item(layout: config_layout, row: 0, col: 0)]
    pub api_label: nwg::Label,

    #[nwg_control(parent: config_window, text: "")]
    #[nwg_layout_item(layout: config_layout, row: 0, col: 1, col_span: 2)]
    pub api_input: nwg::TextInput,

    #[nwg_control(parent: config_window, text: "API URL:")]
    #[nwg_layout_item(layout: config_layout, row: 1, col: 0)]
    pub url_label: nwg::Label,

    #[nwg_control(parent: config_window, text: "")]
    #[nwg_layout_item(layout: config_layout, row: 1, col: 1, col_span: 2)]
    pub url_input: nwg::TextInput,

    #[nwg_control(parent: config_window, text: "模型:")]
    #[nwg_layout_item(layout: config_layout, row: 2, col: 0)]
    pub model_label: nwg::Label,

    #[nwg_control(parent: config_window, text: "")]
    #[nwg_layout_item(layout: config_layout, row: 2, col: 1, col_span: 2)]
    pub model_input: nwg::TextInput,

    #[nwg_control(parent: config_window, text: "保存")]
    #[nwg_layout_item(layout: config_layout, row: 3, col: 2)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_config])]
    pub save_btn: nwg::Button,
    
    #[nwg_control(parent: config_window, text: "重置")]
    #[nwg_layout_item(layout: config_layout, row: 3, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::reset_ai_config])]
    pub reset_btn: nwg::Button,

    // --- 热键设置窗口 ---
    #[nwg_control(size: (300, 150), position: (350, 350), title: "热键绑定", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_hotkey_window])]
    pub hotkey_window: nwg::Window,

    #[nwg_layout(parent: hotkey_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub hotkey_layout: nwg::GridLayout,

    #[nwg_control(parent: hotkey_window, text: "选择热键:")]
    #[nwg_layout_item(layout: hotkey_layout, row: 0, col: 0)]
    pub hotkey_win_label: nwg::Label,

    #[nwg_control(parent: hotkey_window)]
    #[nwg_layout_item(layout: hotkey_layout, row: 0, col: 1, col_span: 2)]
    pub hotkey_win_combo: nwg::ComboBox<String>,

    #[nwg_control(parent: hotkey_window, text: "保存")]
    #[nwg_layout_item(layout: hotkey_layout, row: 1, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_hotkey_config])]
    pub hotkey_save_btn: nwg::Button,

    // --- 版本检测窗口 ---
    #[nwg_control(size: (450, 400), position: (300, 300), title: "版本检测", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_update_window])]
    pub update_window: nwg::Window,

    #[nwg_layout(parent: update_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub update_layout: nwg::GridLayout,

    #[nwg_control(parent: update_window, text: "当前版本:")]
    #[nwg_layout_item(layout: update_layout, row: 0, col: 0)]
    pub current_ver_label: nwg::Label,

    #[nwg_control(parent: update_window, text: "")]
    #[nwg_layout_item(layout: update_layout, row: 0, col: 1, col_span: 2)]
    pub current_ver_val: nwg::Label,

    #[nwg_control(parent: update_window, text: "最新版本:")]
    #[nwg_layout_item(layout: update_layout, row: 1, col: 0)]
    pub latest_ver_label: nwg::Label,

    #[nwg_control(parent: update_window, text: "未检测")]
    #[nwg_layout_item(layout: update_layout, row: 1, col: 1, col_span: 2)]
    pub latest_ver_val: nwg::Label,

    #[nwg_control(parent: update_window, text: "更新日志:")]
    #[nwg_layout_item(layout: update_layout, row: 2, col: 0, col_span: 3)]
    pub changelog_label: nwg::Label,

    #[nwg_control(parent: update_window, text: "", flags: "VISIBLE", readonly: true)]
    #[nwg_layout_item(layout: update_layout, row: 3, col: 0, col_span: 3, row_span: 3)]
    pub changelog_text: nwg::TextBox,

    #[nwg_control(parent: update_window, text: "检测新版本")]
    #[nwg_layout_item(layout: update_layout, row: 6, col: 0)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::do_check_update])]
    pub check_update_btn: nwg::Button,


    #[nwg_control(parent: update_window, text: "忽略此版本", enabled: false)]
    #[nwg_layout_item(layout: update_layout, row: 6, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::ignore_update])]
    pub ignore_btn: nwg::Button,

    #[nwg_control(parent: update_window, text: "立即更新", enabled: false)]
    #[nwg_layout_item(layout: update_layout, row: 6, col: 2)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::start_update])]
    pub start_update_btn: nwg::Button,

    #[nwg_control(parent: update_window, range: 0..100, pos: 0)]
    #[nwg_layout_item(layout: update_layout, row: 7, col: 0, col_span: 3)]
    pub update_progress: nwg::ProgressBar,

    // --- 流式设置窗口 ---
    #[nwg_control(size: (300, 180), position: (350, 350), title: "流式输出设置", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_streaming_window])]
    pub streaming_window: nwg::Window,

    #[nwg_layout(parent: streaming_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub streaming_layout: nwg::GridLayout,

    #[nwg_control(parent: streaming_window, text: "流式间隔 (毫秒):")]
    #[nwg_layout_item(layout: streaming_layout, row: 0, col: 0)]
    pub streaming_label: nwg::Label,

    #[nwg_control(parent: streaming_window, text: "2000")]
    #[nwg_layout_item(layout: streaming_layout, row: 0, col: 1, col_span: 2)]
    pub streaming_input: nwg::TextInput,

    #[nwg_control(parent: streaming_window, text: "保存")]
    #[nwg_layout_item(layout: streaming_layout, row: 1, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_streaming_config])]
    pub streaming_save_btn: nwg::Button,

    // --- VAD设置窗口 ---
    #[nwg_control(size: (400, 250), position: (350, 350), title: "VAD参数设置", flags: "WINDOW", icon: Some(&data.icon))]
    pub vad_window: nwg::Window,

    #[nwg_layout(parent: vad_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub vad_layout: nwg::GridLayout,

    #[nwg_control(parent: vad_window, text: "能量阈值:")]
    #[nwg_layout_item(layout: vad_layout, row: 0, col: 0)]
    pub vad_energy_label: nwg::Label,

    #[nwg_control(parent: vad_window, text: "0.01")]
    #[nwg_layout_item(layout: vad_layout, row: 0, col: 1, col_span: 2)]
    pub vad_energy_input: nwg::TextInput,

    #[nwg_control(parent: vad_window, text: "静音帧阈值:")]
    #[nwg_layout_item(layout: vad_layout, row: 1, col: 0)]
    pub vad_silence_label: nwg::Label,

    #[nwg_control(parent: vad_window, text: "10")]
    #[nwg_layout_item(layout: vad_layout, row: 1, col: 1, col_span: 2)]
    pub vad_silence_input: nwg::TextInput,

    #[nwg_control(parent: vad_window, text: "帧大小:")]
    #[nwg_layout_item(layout: vad_layout, row: 2, col: 0)]
    pub vad_frame_label: nwg::Label,

    #[nwg_control(parent: vad_window, text: "1024")]
    #[nwg_layout_item(layout: vad_layout, row: 2, col: 1, col_span: 2)]
    pub vad_frame_input: nwg::TextInput,

    #[nwg_control(parent: vad_window, text: "保存")]
    #[nwg_layout_item(layout: vad_layout, row: 3, col: 1)]
    pub vad_save_btn: nwg::Button,



    // --- 指示器参数设置窗口 ---
    #[nwg_control(size: (400, 300), position: (350, 350), title: "指示器参数设置", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_indicator_window])]
    pub indicator_window: nwg::Window,

    #[nwg_layout(parent: indicator_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub indicator_layout: nwg::GridLayout,

    #[nwg_control(parent: indicator_window, text: "淡出动画时间 (毫秒):")]
    #[nwg_layout_item(layout: indicator_layout, row: 0, col: 0)]
    pub indicator_fade_label: nwg::Label,

    #[nwg_control(parent: indicator_window, text: "300")]
    #[nwg_layout_item(layout: indicator_layout, row: 0, col: 1, col_span: 2)]
    pub indicator_fade_input: nwg::TextInput,

    #[nwg_control(parent: indicator_window, text: "错误状态持续时间 (毫秒):")]
    #[nwg_layout_item(layout: indicator_layout, row: 1, col: 0)]
    pub indicator_error_label: nwg::Label,

    #[nwg_control(parent: indicator_window, text: "5000")]
    #[nwg_layout_item(layout: indicator_layout, row: 1, col: 1, col_span: 2)]
    pub indicator_error_input: nwg::TextInput,

    #[nwg_control(parent: indicator_window, text: "成功状态持续时间 (毫秒):")]
    #[nwg_layout_item(layout: indicator_layout, row: 2, col: 0)]
    pub indicator_success_label: nwg::Label,

    #[nwg_control(parent: indicator_window, text: "5000")]
    #[nwg_layout_item(layout: indicator_layout, row: 2, col: 1, col_span: 2)]
    pub indicator_success_input: nwg::TextInput,

    #[nwg_control(parent: indicator_window, text: "保存")]
    #[nwg_layout_item(layout: indicator_layout, row: 3, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_indicator_config])]
    pub indicator_save_btn: nwg::Button,

    // --- 关于窗口 ---
    #[nwg_control(size: (300, 320), position: (400, 400), title: "关于 Voice2Type", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_about_window])]
    pub about_window: nwg::Window,

    #[nwg_layout(parent: about_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub about_layout: nwg::GridLayout,

    #[nwg_control(parent: about_window, text: "Voice2Type", font: Some(&data.font_bold))]
    #[nwg_layout_item(layout: about_layout, row: 0, col: 0, col_span: 2)]
    pub app_name_label: nwg::Label,

    #[nwg_control(parent: about_window, text: concat!("当前版本: ", env!("CARGO_PKG_VERSION")))]
    #[nwg_layout_item(layout: about_layout, row: 1, col: 0, col_span: 2)]
    pub version_label: nwg::Label,

    #[nwg_control(parent: about_window, text: "最新版本: 未检测")]
    #[nwg_layout_item(layout: about_layout, row: 2, col: 0, col_span: 2)]
    pub about_latest_ver_label: nwg::Label,

    #[nwg_control(parent: about_window, text: "开源项目地址:")]
    #[nwg_layout_item(layout: about_layout, row: 3, col: 0, col_span: 2)]
    pub repo_label: nwg::Label,

    #[nwg_control(parent: about_window, text: "访问 GitHub 仓库")]
    #[nwg_layout_item(layout: about_layout, row: 4, col: 0, col_span: 2)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::open_github])]
    pub github_btn: nwg::Button,
    
    #[nwg_control(parent: about_window, text: "作者：guchang233")]
    #[nwg_layout_item(layout: about_layout, row: 5, col: 0, col_span: 2)]
    pub author_label: nwg::Label,

    #[nwg_resource(family: "Segoe UI", size: 24, weight: 700)]
    pub font_bold: nwg::Font,

    pub update_info: RefCell<Option<update::UpdateInfo>>,

    #[nwg_control]
    #[nwg_events( OnNotice: [Voice2TypeApp::on_check_notice] )]
    pub check_notice: nwg::Notice,

    #[nwg_control]
    #[nwg_events( OnNotice: [Voice2TypeApp::on_progress_notice] )]
    pub progress_notice: nwg::Notice,

    // Thread communication state
    pub check_result: RefCell<Option<Arc<Mutex<Option<anyhow::Result<Option<update::UpdateInfo>>>>>>>,
    pub progress_data: RefCell<Option<Arc<Mutex<(u64, u64)>>>>,

    // 状态
    pub config_manager: RefCell<Option<Arc<ConfigManager>>>,

    // UI 线程轮询同步日志菜单状态
    #[nwg_control(interval: std::time::Duration::from_millis(100), active: false)]
    #[nwg_events( OnTimerTick: [Voice2TypeApp::on_tick] )]
    pub timer: nwg::AnimationTimer,
}

impl Voice2TypeApp {
    pub fn init(config_manager: Arc<ConfigManager>) -> Voice2TypeAppUi {
        // 如果图标文件不存在，NWG 可能会 panic 或编译失败。
        // 这里假设图标在构建时已正确打包。
        
        let app_ui = Voice2TypeApp::build_ui(Default::default()).expect("无法构建 UI");
        let app = &app_ui;
        
        // 初始化状态
        *app.config_manager.borrow_mut() = Some(config_manager.clone());
        *app.check_result.borrow_mut() = Some(Arc::new(Mutex::new(None)));
        *app.progress_data.borrow_mut() = Some(Arc::new(Mutex::new((0, 0))));
        
        // Set initial values
        app.allow_emoji_item.set_checked(config_manager.allow_emoji());
        app.allow_punct_item.set_checked(config_manager.allow_punctuation());
        
        let show_log = config_manager.show_log();
        app.log_item.set_checked(show_log);

        let lang = config_manager.language();
        
        if lang == "en" {
            app.lang_en_item.set_checked(true);
            app.lang_zh_item.set_checked(false);
        } else {
            app.lang_zh_item.set_checked(true);
            app.lang_en_item.set_checked(false);
        }

        // Output Language settings
        let out_lang = config_manager.output_language();
        match out_lang.as_str() {
            "zh" => {
                app.output_lang_zh_item.set_checked(true);
                app.output_lang_auto_item.set_checked(false);
                app.output_lang_en_item.set_checked(false);
            },
            "en" => {
                app.output_lang_en_item.set_checked(true);
                app.output_lang_auto_item.set_checked(false);
                app.output_lang_zh_item.set_checked(false);
            },
            _ => { // "auto" or others
                app.output_lang_auto_item.set_checked(true);
                app.output_lang_zh_item.set_checked(false);
                app.output_lang_en_item.set_checked(false);
            }
        }

        let mode = config_manager.output_mode();
        if mode == "inject" {
            app.output_inject_item.set_checked(true);
            app.output_clipboard_item.set_checked(false);
        } else {
            app.output_inject_item.set_checked(false);
            app.output_clipboard_item.set_checked(true);
        }

        #[cfg(target_os = "windows")]
        {
            let enabled = crate::win_utils::is_autostart_enabled();
            app.autostart_item.set_checked(enabled || config_manager.autostart_enabled());
            if let Some(mgr) = &*app.config_manager.borrow() {
                mgr.set_autostart_enabled(app.autostart_item.checked());
                let _ = mgr.save();
            }
        }

        app.indicator_item.set_checked(config_manager.enable_indicator());
        app.streaming_item.set_checked(config_manager.enable_streaming());
        
        // 设置触发模式初始状态
        let trigger_mode = config_manager.trigger_mode();
        if trigger_mode == "hold" {
            app.trigger_hold_item.set_checked(true);
            app.trigger_toggle_item.set_checked(false);
        } else if trigger_mode == "toggle" {
            app.trigger_hold_item.set_checked(false);
            app.trigger_toggle_item.set_checked(true);
        } else {
            // 默认使用hold模式
            app.trigger_hold_item.set_checked(true);
            app.trigger_toggle_item.set_checked(false);
        }

        // 初始隐藏窗口
        app.config_window.set_visible(false);
        app.hotkey_window.set_visible(false);
        app.streaming_window.set_visible(false);
        app.vad_window.set_visible(false);
        app.about_window.set_visible(false);
        app.update_window.set_visible(false);
        app.current_ver_val.set_text(CURRENT_VERSION);

        // 确保主锚点窗口也不可见
        app.window.set_visible(false);
        
        // 设置输入框文本
        app.api_input.set_text(&config_manager.get_api_key());
        app.url_input.set_text(&config_manager.get_api_url());
        app.model_input.set_text(&config_manager.get_model_name());

        // 初始化热键下拉框
        let hotkeys = vec![
            ("F1", 0x70), ("F2", 0x71), ("F3", 0x72), ("F4", 0x73), 
            ("F5", 0x74), ("F6", 0x75), ("F7", 0x76), ("F8", 0x77), 
            ("F9", 0x78), ("F10", 0x79), ("F11", 0x7A), ("F12", 0x7B),
            ("CAPS LOCK", 0x14), ("LEFT CTRL", 0xA2), ("RIGHT CTRL", 0xA3),
            ("LEFT ALT", 0xA4), ("RIGHT ALT", 0xA5), ("V", 0x56)
        ];
        
        let mut selected_index = 1; // 默认 F2
        let current_vk = config_manager.hotkey();
        
        for (i, (name, vk)) in hotkeys.iter().enumerate() {
            app.hotkey_win_combo.push(name.to_string());
            if *vk == current_vk {
                selected_index = i;
            }
        }
        app.hotkey_win_combo.set_selection(Some(selected_index));

        app.timer.start();

        // 检查 API Key 是否为空 (首次运行)
        if config_manager.get_api_key().is_empty() {
            #[cfg(target_os = "windows")]
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_YESNO, MB_ICONINFORMATION, IDYES};
                use windows::core::PCWSTR;
                let title = "温馨提示\0".encode_utf16().collect::<Vec<u16>>();
                let msg = "没检测到 API Key。\n是否前往注册页面获取？\0".encode_utf16().collect::<Vec<u16>>();
                let ret = MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNO | MB_ICONINFORMATION);
                if ret == IDYES {
                    let _ = open::that("https://cloud.siliconflow.cn/account/ak");
                }
            }
            app.config_window.set_visible(true);
        }

        // 启动自动检测
        app.do_check_update();
        
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

    fn set_output_inject(&self) {
        self.output_inject_item.set_checked(true);
        self.output_clipboard_item.set_checked(false);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_output_mode("inject".to_string());
            let _ = mgr.save();
        }
    }

    fn set_output_clipboard(&self) {
        self.output_inject_item.set_checked(false);
        self.output_clipboard_item.set_checked(true);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_output_mode("clipboard".to_string());
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
        
        #[cfg(target_os = "windows")]
        {
            // 使用let绑定创建一个生命周期更长的值
            let config_ref = self.config_manager.borrow();
            let config = config_ref.as_ref().map(|v| &**v);
            if new_state {
                crate::log_set_enabled(false, config);
            }
            crate::log_set_enabled(new_state, config);
        }
    }

    fn toggle_autostart(&self) {
        let new_state = !self.autostart_item.checked();
        self.autostart_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_autostart_enabled(new_state);
            let _ = mgr.save();
        }
        #[cfg(target_os = "windows")]
        unsafe {
            if new_state {
                let ok = crate::win_utils::set_autostart(true);
                if ok.is_err() {
                    self.autostart_item.set_checked(false);
                    if let Some(mgr) = &*self.config_manager.borrow() {
                        mgr.set_autostart_enabled(false);
                        let _ = mgr.save();
                    }
                    nwg::simple_message("失败", "设置自启动失败，请以管理员或检查权限");
                } else {
                    nwg::simple_message("已启用", "已设置为开机自启动");
                }
            } else {
                let ok = crate::win_utils::set_autostart(false);
                if ok.is_err() {
                    self.autostart_item.set_checked(true);
                    if let Some(mgr) = &*self.config_manager.borrow() {
                        mgr.set_autostart_enabled(true);
                        let _ = mgr.save();
                    }
                    nwg::simple_message("失败", "取消自启动失败，请检查权限");
                } else {
                    nwg::simple_message("已关闭", "已取消开机自启动");
                }
            }
        }
    }

    fn toggle_indicator(&self) {
        let new_state = !self.indicator_item.checked();
        self.indicator_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_enable_indicator(new_state);
            let _ = mgr.save();
        }
        
        #[cfg(target_os = "windows")]
        {
            if new_state {
                 if crate::INDICATOR.get().is_none() {
                     let _ = crate::INDICATOR.set(crate::indicator::StatusIndicator::new());
                 }
            } else {
                 if let Some(ind) = crate::INDICATOR.get() {
                     ind.set_state(crate::indicator::IndicatorState::Hidden);
                 }
            }
        }
    }

    fn toggle_streaming(&self) {
        let new_state = !self.streaming_item.checked();
        
        // 如果用户要启用流式输出，显示警告提示
        if new_state {
            #[cfg(target_os = "windows")]
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_YESNO, MB_ICONWARNING, IDYES};
                use windows::core::PCWSTR;
                let title = "警告\0".encode_utf16().collect::<Vec<u16>>();
                let msg = "流式输出精准度极低，且若使用付费API会造成更多的费用\n\n是否仍然开启？\0".encode_utf16().collect::<Vec<u16>>();
                let result = MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNO | MB_ICONWARNING);
                
                // 如果用户选择取消，不启用流式输出
                if result != IDYES {
                    return;
                }
            }
        }
        
        self.streaming_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_enable_streaming(new_state);
            let _ = mgr.save();
        }
    }

    fn set_trigger_hold(&self) {
        self.trigger_hold_item.set_checked(true);
        self.trigger_toggle_item.set_checked(false);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_trigger_mode("hold".to_string());
            let _ = mgr.save();
        }
    }

    fn set_trigger_toggle(&self) {
        self.trigger_hold_item.set_checked(false);
        self.trigger_toggle_item.set_checked(true);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_trigger_mode("toggle".to_string());
            let _ = mgr.save();
        }
    }





    fn show_streaming_window(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            self.streaming_input.set_text(&mgr.streaming_interval().to_string());
        }
        self.streaming_window.set_visible(true);
        self.streaming_window.set_focus();
    }

    fn hide_streaming_window(&self) {
        self.streaming_window.set_visible(false);
    }

    fn save_streaming_config(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            if let Ok(interval) = self.streaming_input.text().parse::<u64>() {
                if interval >= 500 && interval <= 10000 {
                    mgr.set_streaming_interval(interval);
                    let _ = mgr.save();
                    nwg::simple_message("已保存", "流式输出设置已保存");
                } else {
                    nwg::simple_message("错误", "间隔值必须在 500-10000 毫秒之间");
                }
            } else {
                nwg::simple_message("错误", "请输入有效的数字");
            }
        }
        self.streaming_window.set_visible(false);
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
                let _ = open::that(".");
            }
        }
    }

    fn open_log_dir(&self) {
         if let Some(mgr) = &*self.config_manager.borrow() {
             let log_dir = mgr.log_dir();
             let _ = open::that(log_dir);
         }
    }

    fn hide_config_window(&self) {
        self.config_window.set_visible(false);
    }

    fn save_config(&self) {
        let key = self.api_input.text();
        let url = self.url_input.text();
        let model = self.model_input.text();
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_api_key(key);
            mgr.set_api_url(url);
            mgr.set_model_name(model);
            let _ = mgr.save();
        }
        nwg::simple_message("已保存", "配置已成功保存！");
        self.config_window.set_visible(false);
    }

    fn show_hotkey_window(&self) {
        self.hotkey_window.set_visible(true);
        self.hotkey_window.set_focus();
    }

    fn hide_hotkey_window(&self) {
        self.hotkey_window.set_visible(false);
    }

    fn save_hotkey_config(&self) {
         if let Some(mgr) = &*self.config_manager.borrow() {
            let hotkeys_vks = vec![
                0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 
                0x78, 0x79, 0x7A, 0x7B, 0x14, 0xA2, 0xA3, 0xA4, 0xA5, 0x56
            ];
            if let Some(idx) = self.hotkey_win_combo.selection() {
                if idx < hotkeys_vks.len() {
                    mgr.set_hotkey(hotkeys_vks[idx]);
                    let _ = mgr.save();
                }
            }
        }
        nwg::simple_message("已保存", "热键设置已保存 (部分更改可能需要重启生效)");
        self.hotkey_window.set_visible(false);
    }

    fn set_out_lang_auto(&self) {
        if !self.output_lang_auto_item.checked() {
            self.output_lang_auto_item.set_checked(true);
            self.output_lang_zh_item.set_checked(false);
            self.output_lang_en_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_output_language("auto".to_string());
                let _ = mgr.save();
            }
        }
    }

    fn set_out_lang_zh(&self) {
         if !self.output_lang_zh_item.checked() {
            self.output_lang_zh_item.set_checked(true);
            self.output_lang_auto_item.set_checked(false);
            self.output_lang_en_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_output_language("zh".to_string());
                let _ = mgr.save();
            }
        }
    }

    fn set_out_lang_en(&self) {
         if !self.output_lang_en_item.checked() {
            self.output_lang_en_item.set_checked(true);
            self.output_lang_auto_item.set_checked(false);
            self.output_lang_zh_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_output_language("en".to_string());
                let _ = mgr.save();
            }
        }
    }

    fn reset_ai_config(&self) {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_YESNO, MB_ICONWARNING, IDYES};
            use windows::core::PCWSTR;
            let title = "确认重置\0".encode_utf16().collect::<Vec<u16>>();
            let msg = "将重置 URL 与 模型名称 为默认值\nApiKey将被清空\n确定继续？\0".encode_utf16().collect::<Vec<u16>>();
            let ret = MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNO | MB_ICONWARNING);
            if ret != IDYES {
                return;
            }
        }
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.reset_ai_config();
            self.api_input.set_text(&mgr.get_api_key());
            self.url_input.set_text(&mgr.get_api_url());
            self.model_input.set_text(&mgr.get_model_name());
            let _ = mgr.save();
        }
        nwg::simple_message("已重置", "已重置 API Key、API URL 与模型为默认值");
    }

    fn show_update_window(&self) {
        self.update_window.set_visible(true);
        self.update_window.set_focus();
        self.do_check_update();
    }

    fn hide_update_window(&self) {
        self.update_window.set_visible(false);
    }

    fn do_check_update(&self) {
        self.latest_ver_val.set_text("正在检测...");
        self.start_update_btn.set_enabled(false);
        self.changelog_text.set_text("");
        self.update_progress.set_visible(false);

        let sender = self.check_notice.sender();
        let result_store = self.check_result.borrow().as_ref().unwrap().clone();
        
        std::thread::spawn(move || {
            let res = update::check_update();
            *result_store.lock().unwrap() = Some(res);
            sender.notice();
        });
    }

    fn on_check_notice(&self) {
        let result_guard = self.check_result.borrow();
        let arc = result_guard.as_ref().unwrap();
        let mut guard = arc.lock().unwrap();
        
        if let Some(res) = guard.take() {
             match res {
                 Ok(Some(info)) => {
                     // 保存 info 到 RefCell
                     *self.update_info.borrow_mut() = Some(info.clone());
                     
                     self.latest_ver_val.set_text(&info.version);
                     self.changelog_text.set_text(&info.body);
                     self.start_update_btn.set_enabled(true);
                     self.ignore_btn.set_enabled(true);
                     
                     // 如果窗口不可见（后台检测），且未忽略，则提示
                     if !self.update_window.visible() {
                         let (ignored, last_check) = if let Some(mgr) = &*self.config_manager.borrow() {
                             (mgr.ignored_version(), mgr.last_check_time())
                         } else {
                             (String::new(), 0)
                         };
                         
                         let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

                         if info.version != ignored && (now > last_check + 86400) {
                             #[cfg(target_os = "windows")]
                             unsafe {
                                use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_YESNO, MB_ICONINFORMATION, IDYES};
                                use windows::core::PCWSTR;
                                let title = "发现新版本\0".encode_utf16().collect::<Vec<u16>>();
                                let msg = format!("新版本 {} 已发布！\n是否查看详情？\0", info.version).encode_utf16().collect::<Vec<u16>>();
                                let ret = MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNO | MB_ICONINFORMATION);
                                
                                // 记录提示时间
                                if let Some(mgr) = &*self.config_manager.borrow() {
                                    mgr.set_last_check_time(now);
                                    let _ = mgr.save();
                                }

                                if ret == IDYES {
                                    self.show_update_window();
                                }
                             }
                         }
                     }
                 },
                 Ok(None) => {
                     self.latest_ver_val.set_text("当前已是最新");
                     self.ignore_btn.set_enabled(false);
                 },
                 Err(e) => {
                     self.latest_ver_val.set_text("检测失败");
                     self.ignore_btn.set_enabled(false);
                     if self.update_window.visible() {
                         nwg::simple_message("检测失败", &format!("错误: {}", e));
                     }
                 }
             }
        }
    }

    fn ignore_update(&self) {
        if let Some(info) = self.update_info.borrow().as_ref() {
             if let Some(mgr) = &*self.config_manager.borrow() {
                 mgr.set_ignored_version(info.version.clone());
                 let _ = mgr.save();
             }
             self.hide_update_window();
             nwg::simple_message("已忽略", "将不再提示此版本的更新。");
        }
    }

    fn start_update(&self) {
        let info_opt = self.update_info.borrow().clone();
        if let Some(info) = info_opt {
            self.start_update_btn.set_enabled(false);
            self.check_update_btn.set_enabled(false);
            self.update_progress.set_visible(true);
            self.update_progress.set_pos(0);
            
            let sender = self.progress_notice.sender();
            let progress_data = self.progress_data.borrow().as_ref().unwrap().clone();
            let download_url = info.download_url.clone();
            let version = info.version.clone();
            
            std::thread::spawn(move || {
                // 修改：下载到当前程序所在目录，避免跨盘符移动导致的 os error 17
                let current_exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let parent_dir = current_exe.parent().unwrap_or_else(|| std::path::Path::new("."));
                let target_path = parent_dir.join(format!("voice2type_update_{}.exe", version));
                
                let pd = progress_data.clone();
                let sender_clone = sender.clone();
                
                let res = update::download_file(&download_url, &target_path, move |curr, total| {
                    *pd.lock().unwrap() = (curr, total);
                    sender_clone.notice();
                });
                
                if let Err(e) = res {
                    #[cfg(target_os = "windows")]
                    unsafe {
                        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONERROR};
                        use windows::core::PCWSTR;
                        let title = "更新失败\0".encode_utf16().collect::<Vec<u16>>();
                        let msg = format!("下载失败: {}\0", e).encode_utf16().collect::<Vec<u16>>();
                        MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONERROR);
                    }
                    return;
                }
                
                if let Err(e) = update::install_update(&target_path) {
                    #[cfg(target_os = "windows")]
                    unsafe {
                        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONERROR};
                        use windows::core::PCWSTR;
                        let title = "更新失败\0".encode_utf16().collect::<Vec<u16>>();
                        let msg = format!("安装失败: {}\0", e).encode_utf16().collect::<Vec<u16>>();
                        MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONERROR);
                    }
                    return;
                }
                
                #[cfg(target_os = "windows")]
                unsafe {
                    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONINFORMATION};
                    use windows::core::PCWSTR;
                    let title = "更新成功\0".encode_utf16().collect::<Vec<u16>>();
                    let msg = "更新已完成，程序将立即重启。\0".encode_utf16().collect::<Vec<u16>>();
                    MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION);
                    
                    // 显式释放资源和互斥锁
                    crate::log_set_enabled(false, None);
                    crate::release_app_mutex();
                }
                
                use std::process::Command;
                if let Ok(exe) = std::env::current_exe() {
                    let _ = Command::new(exe).arg("--restart").spawn();
                }
                std::process::exit(0);
            });
        }
    }

    fn on_progress_notice(&self) {
         let guard = self.progress_data.borrow();
         let arc = guard.as_ref().unwrap();
         let (curr, total) = *arc.lock().unwrap();
         
         if total > 0 {
             let percent = (curr as f64 / total as f64 * 100.0) as u32;
             self.update_progress.set_pos(percent);
         }
    }

    fn show_about(&self) {
        // 更新最新版本信息
        if let Some(info) = self.update_info.borrow().as_ref() {
            self.about_latest_ver_label.set_text(&format!("最新版本: {}", info.version));
        } else {
            self.about_latest_ver_label.set_text("最新版本: 未检测");
        }
        self.about_window.set_visible(true);
        self.about_window.set_focus();
    }

    fn hide_about_window(&self) {
        self.about_window.set_visible(false);
    }

    fn open_github(&self) {
        let _ = open::that("https://github.com/guchang233/VOICE2TYPE");
    }

    fn quit(&self) {
        nwg::stop_thread_dispatch();
    }



    fn show_indicator_window(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            self.indicator_fade_input.set_text(&mgr.indicator_fade_duration().to_string());
            self.indicator_error_input.set_text(&mgr.indicator_error_duration().to_string());
            self.indicator_success_input.set_text(&mgr.indicator_success_duration().to_string());
        }
        self.indicator_window.set_visible(true);
        self.indicator_window.set_focus();
    }

    fn hide_indicator_window(&self) {
        self.indicator_window.set_visible(false);
    }

    fn save_indicator_config(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            // 验证并保存淡出动画时间
            if let Ok(fade_duration) = self.indicator_fade_input.text().parse::<u64>() {
                if fade_duration >= 100 && fade_duration <= 2000 {
                    // 验证并保存错误状态持续时间
                    if let Ok(error_duration) = self.indicator_error_input.text().parse::<u64>() {
                        if error_duration >= 1000 && error_duration <= 10000 {
                            // 验证并保存成功状态持续时间
                            if let Ok(success_duration) = self.indicator_success_input.text().parse::<u64>() {
                                if success_duration >= 1000 && success_duration <= 10000 {
                                    mgr.set_indicator_fade_duration(fade_duration);
                                    mgr.set_indicator_error_duration(error_duration);
                                    mgr.set_indicator_success_duration(success_duration);
                                    let _ = mgr.save();
                                    nwg::simple_message("已保存", "指示器参数设置已保存");
                                } else {
                                    nwg::simple_message("错误", "成功状态持续时间必须在 1000-10000 毫秒之间");
                                }
                            } else {
                                nwg::simple_message("错误", "请输入有效的成功状态持续时间");
                            }
                        } else {
                            nwg::simple_message("错误", "错误状态持续时间必须在 1000-10000 毫秒之间");
                        }
                    } else {
                        nwg::simple_message("错误", "请输入有效的错误状态持续时间");
                    }
                } else {
                    nwg::simple_message("错误", "淡出动画时间必须在 100-2000 毫秒之间");
                }
            } else {
                nwg::simple_message("错误", "请输入有效的淡出动画时间");
            }
        }
        self.indicator_window.set_visible(false);
    }
}

impl Voice2TypeApp {
    fn on_tick(&self) {
        #[cfg(target_os = "windows")]
        {
            if crate::should_uncheck_log_menu_and_reset() {
                self.log_item.set_checked(false);
                if let Some(mgr) = &*self.config_manager.borrow() {
                    mgr.set_show_log(false);
                    let _ = mgr.save();
                }
            }
        }
    }
}
