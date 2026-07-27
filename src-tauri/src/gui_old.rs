use crate::config::ConfigManager;
use crate::history;
use crate::notify::PENDING_TRAY_MESSAGES;
use crate::output::handler::OutputHandler;
use crate::update;
use cpal::traits::{DeviceTrait, HostTrait};
use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use native_windows_gui::NativeUi;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

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
    #[nwg_control(icon: Some(&data.icon), tip: Some("Voice2Type 语音输入"))]
    #[nwg_events(MousePressLeftUp: [Voice2TypeApp::show_menu], OnContextMenu: [Voice2TypeApp::show_menu])]
    pub tray: nwg::TrayNotification,

    #[nwg_resource(source_bin: Some(include_bytes!("../icon.ico")))]
    pub icon: nwg::Icon,

    // 托�    // 设置 (Top Level)
    // 托盘菜单
    #[nwg_control(popup: true)]
    pub tray_menu: nwg::Menu,

    #[nwg_control(parent: tray_menu, text: "语音转文字（录音识别）")]
    pub voice_menu: nwg::Menu,

    #[nwg_control(parent: tray_menu, text: "流式语音识别")]
    pub streaming_menu: nwg::Menu,

    #[nwg_control(parent: streaming_menu, text: "触发模式")]
    pub streaming_trigger_menu: nwg::Menu,

    #[nwg_control(parent: streaming_trigger_menu, text: "按住说话", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_streaming_trigger_hold])]
    pub streaming_trigger_hold_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_trigger_menu, text: "按一下开始/结束", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_streaming_trigger_toggle])]
    pub streaming_trigger_toggle_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_menu, text: "通用设置")]
    pub streaming_general_menu: nwg::Menu,

    #[nwg_control(parent: streaming_general_menu, text: "状态浮窗", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_streaming_indicator])]
    pub streaming_indicator_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_general_menu, text: "保留标点", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_streaming_punctuation])]
    pub streaming_allow_punct_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_general_menu, text: "保留表情符号", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_streaming_emoji])]
    pub streaming_allow_emoji_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_menu, text: "后处理模式")]
    pub streaming_post_menu: nwg::Menu,

    #[nwg_control(parent: streaming_post_menu, text: "AI 润色（硅基流动/Groq）", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_streaming_post_ai])]
    pub streaming_post_ai_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_post_menu, text: "本地轻量修正", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_streaming_post_local])]
    pub streaming_post_local_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_post_menu, text: "关闭后处理", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_streaming_post_none])]
    pub streaming_post_none_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_menu, text: "识别语言")]
    pub streaming_lang_menu: nwg::Menu,

    #[nwg_control(parent: streaming_lang_menu, text: "自动", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_streaming_lang_auto])]
    pub streaming_lang_auto_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_lang_menu, text: "中文", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_streaming_lang_zh])]
    pub streaming_lang_zh_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_lang_menu, text: "English", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_streaming_lang_en])]
    pub streaming_lang_en_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_menu)]
    pub sep_streaming: nwg::MenuSeparator,

    #[nwg_control(parent: streaming_menu, text: "流式热键绑定...")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_streaming_hotkey_window])]
    pub streaming_hotkey_item: nwg::MenuItem,

    #[nwg_control(parent: streaming_menu, text: "资源 ID...")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_streaming_resource_window])]
    pub streaming_resource_item: nwg::MenuItem,

    #[nwg_control(parent: voice_menu, text: "触发模式")]
    pub trigger_menu: nwg::Menu,

    #[nwg_control(parent: trigger_menu, text: "按住说话", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_trigger_hold])]
    pub trigger_hold_item: nwg::MenuItem,

    #[nwg_control(parent: trigger_menu, text: "按一下开始/结束", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_trigger_toggle])]
    pub trigger_toggle_item: nwg::MenuItem,

    #[nwg_control(parent: voice_menu, text: "通用设置")]
    pub general_menu: nwg::Menu,

    #[nwg_control(parent: general_menu, text: "开机自启动", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_autostart])]
    pub autostart_item: nwg::MenuItem,

    #[nwg_control(parent: general_menu, text: "状态浮窗", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_indicator])]
    pub indicator_item: nwg::MenuItem,

    #[nwg_control(parent: general_menu)]
    pub sep_general: nwg::MenuSeparator,

    #[nwg_control(parent: general_menu, text: "保留标点", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_punctuation])]
    pub allow_punct_item: nwg::MenuItem,

    #[nwg_control(parent: general_menu, text: "保留表情符号", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_emoji])]
    pub allow_emoji_item: nwg::MenuItem,

    #[nwg_control(parent: general_menu, text: "选择麦克风")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_mic_window])]
    pub mic_settings_item: nwg::MenuItem,

    #[nwg_control(parent: voice_menu, text: "输出方式")]
    pub output_menu: nwg::Menu,

    #[nwg_control(parent: output_menu, text: "键盘注入", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_output_inject])]
    pub output_inject_item: nwg::MenuItem,

    #[nwg_control(parent: output_menu, text: "剪贴板粘贴（推荐）", check: false)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_output_clipboard])]
    pub output_clipboard_item: nwg::MenuItem,

    #[nwg_control(parent: voice_menu, text: "识别语言")]
    pub output_lang_menu: nwg::Menu,

    #[nwg_control(parent: output_lang_menu, text: "自动", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_out_lang_auto])]
    pub output_lang_auto_item: nwg::MenuItem,

    #[nwg_control(parent: output_lang_menu, text: "中文", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_out_lang_zh])]
    pub output_lang_zh_item: nwg::MenuItem,

    #[nwg_control(parent: output_lang_menu, text: "English", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::set_out_lang_en])]
    pub output_lang_en_item: nwg::MenuItem,

    #[nwg_control(parent: voice_menu)]
    pub sep_voice_inner: nwg::MenuSeparator,

    #[nwg_control(parent: voice_menu, text: "重新粘贴上一条")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::repaste_last])]
    pub repaste_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    pub sep_voice_model: nwg::MenuSeparator,

    #[nwg_control(parent: tray_menu, text: "模型与密钥")]
    pub model_and_key_menu: nwg::Menu,

    #[nwg_control(parent: model_and_key_menu, text: "模型设置...")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_model_settings_window])]
    pub model_settings_item: nwg::MenuItem,

    #[nwg_control(parent: model_and_key_menu, text: "API Key")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_key_config_window])]
    pub key_config_item: nwg::MenuItem,

    #[nwg_control(parent: model_and_key_menu, text: "热键绑定")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_hotkey_window])]
    pub hotkey_settings_item: nwg::MenuItem,

    #[nwg_control(parent: model_and_key_menu, text: "设置本地 Whisper 目录")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_whisper_window])]
    pub whisper_settings_item: nwg::MenuItem,

    #[nwg_control(parent: model_and_key_menu, text: "打开配置目录")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::open_config_dir])]
    pub config_file_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    pub sep_model_debug: nwg::MenuSeparator,

    #[nwg_control(parent: tray_menu, text: "调试")]
    pub debug_menu: nwg::Menu,

    #[nwg_control(parent: debug_menu, text: "显示日志窗口", check: true)]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::toggle_log])]
    pub log_item: nwg::MenuItem,

    #[nwg_control(parent: debug_menu, text: "打开日志目录")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::open_log_dir])]
    pub log_dir_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    pub sep_voice_about: nwg::MenuSeparator,

    // --- 关于父菜单 (Top Level) ---
    #[nwg_control(parent: tray_menu, text: "关于")]
    pub about_parent_menu: nwg::Menu,

    #[nwg_control(parent: about_parent_menu, text: "项目信息")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_about])]
    pub about_item: nwg::MenuItem,

    #[nwg_control(parent: about_parent_menu, text: "检查更新")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::show_update_window])]
    pub update_item: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    pub sep: nwg::MenuSeparator,

    #[nwg_control(parent: tray_menu, text: "退出")]
    #[nwg_events(OnMenuItemSelected: [Voice2TypeApp::quit])]
    pub quit_item: nwg::MenuItem,

    #[nwg_control(size: (580, 260), position: (300, 300), title: "API 密钥配置", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_key_config_window])]
    pub key_config_window: nwg::Window,

    #[nwg_layout(parent: key_config_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub key_config_layout: nwg::GridLayout,

    #[nwg_control(parent: key_config_window, text: "硅基流动 API Key:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: key_config_layout, row: 0, col: 0)]
    pub key_siliconflow_label: nwg::Label,

    #[nwg_control(parent: key_config_window, text: "", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: key_config_layout, row: 0, col: 1)]
    pub key_siliconflow_input: nwg::TextInput,

    #[nwg_control(parent: key_config_window, text: "Groq API Key:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: key_config_layout, row: 1, col: 0)]
    pub key_groq_label: nwg::Label,

    #[nwg_control(parent: key_config_window, text: "", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: key_config_layout, row: 1, col: 1)]
    pub key_groq_input: nwg::TextInput,

    #[nwg_control(parent: key_config_window, text: "豆包流式 API Key:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: key_config_layout, row: 2, col: 0)]
    pub key_doubao_label: nwg::Label,

    #[nwg_control(parent: key_config_window, text: "", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: key_config_layout, row: 2, col: 1)]
    pub key_doubao_input: nwg::TextInput,

    #[nwg_control(parent: key_config_window, text: "确认保存", font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: key_config_layout, row: 3, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_key_config])]
    pub key_save_btn: nwg::Button,

    // --- 热键设置窗口 ---
    #[nwg_control(size: (420, 160), position: (350, 350), title: "全局录音热键绑定", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_hotkey_window])]
    pub hotkey_window: nwg::Window,

    #[nwg_layout(parent: hotkey_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub hotkey_layout: nwg::GridLayout,

    #[nwg_control(parent: hotkey_window, text: "录音热键 (按住说话):", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: hotkey_layout, row: 0, col: 0)]
    pub hotkey_win_label: nwg::Label,

    #[nwg_control(parent: hotkey_window, font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: hotkey_layout, row: 0, col: 1, col_span: 2)]
    pub hotkey_win_combo: nwg::ComboBox<String>,

    #[nwg_control(parent: hotkey_window, text: "保存", font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: hotkey_layout, row: 1, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_hotkey_config])]
    pub hotkey_save_btn: nwg::Button,

    #[nwg_control(size: (420, 160), position: (360, 360), title: "流式语音识别热键", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_streaming_hotkey_window])]
    pub streaming_hotkey_window: nwg::Window,

    #[nwg_layout(parent: streaming_hotkey_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub streaming_hotkey_layout: nwg::GridLayout,

    #[nwg_control(parent: streaming_hotkey_window, text: "流式识别热键:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: streaming_hotkey_layout, row: 0, col: 0)]
    pub streaming_hotkey_label: nwg::Label,

    #[nwg_control(parent: streaming_hotkey_window, font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: streaming_hotkey_layout, row: 0, col: 1, col_span: 2)]
    pub streaming_hotkey_combo: nwg::ComboBox<String>,

    #[nwg_control(parent: streaming_hotkey_window, text: "保存", font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: streaming_hotkey_layout, row: 1, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_streaming_hotkey_config])]
    pub streaming_hotkey_save_btn: nwg::Button,

    #[nwg_control(size: (480, 160), position: (340, 340), title: "流式识别资源 ID", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_streaming_resource_window])]
    pub streaming_resource_window: nwg::Window,

    #[nwg_layout(parent: streaming_resource_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub streaming_resource_layout: nwg::GridLayout,

    #[nwg_control(parent: streaming_resource_window, text: "X-Api-Resource-Id:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: streaming_resource_layout, row: 0, col: 0)]
    pub streaming_resource_label: nwg::Label,

    #[nwg_control(parent: streaming_resource_window, font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: streaming_resource_layout, row: 0, col: 1)]
    pub streaming_resource_combo: nwg::ComboBox<String>,

    #[nwg_control(parent: streaming_resource_window, text: "保存", font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: streaming_resource_layout, row: 1, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_streaming_resource_config])]
    pub streaming_resource_save_btn: nwg::Button,

    // --- 麦克风选择窗口 ---
    #[nwg_control(size: (480, 150), position: (350, 350), title: "选择麦克风", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_mic_window])]
    pub mic_window: nwg::Window,

    #[nwg_layout(parent: mic_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub mic_layout: nwg::GridLayout,

    #[nwg_control(parent: mic_window, text: "输入设备:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: mic_layout, row: 0, col: 0)]
    pub mic_label: nwg::Label,

    #[nwg_control(parent: mic_window, font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: mic_layout, row: 0, col: 1, col_span: 2)]
    pub mic_combo: nwg::ComboBox<String>,

    #[nwg_control(parent: mic_window, text: "保存（重启后生效）", font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: mic_layout, row: 1, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_mic_config])]
    pub mic_save_btn: nwg::Button,

    // --- 本地 Whisper 目录设置 ---
    #[nwg_control(size: (580, 210), position: (320, 320), title: "本地 Whisper 目录", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_whisper_window])]
    pub whisper_window: nwg::Window,

    #[nwg_layout(parent: whisper_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub whisper_layout: nwg::GridLayout,

    #[nwg_control(parent: whisper_window, text: "Whisper 根目录（其下需有 bin\\、models\\）:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: whisper_layout, row: 0, col: 0, col_span: 3)]
    pub whisper_hint_label: nwg::Label,

    #[nwg_control(parent: whisper_window, text: "", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: whisper_layout, row: 1, col: 0, col_span: 2)]
    pub whisper_path_input: nwg::TextInput,

    #[nwg_control(parent: whisper_window, text: "浏览...", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: whisper_layout, row: 1, col: 2)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::browse_whisper_dir])]
    pub whisper_browse_btn: nwg::Button,

    #[nwg_control(parent: whisper_window, text: "打开目录", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: whisper_layout, row: 2, col: 0)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::open_whisper_dir])]
    pub whisper_open_btn: nwg::Button,

    #[nwg_control(parent: whisper_window, text: "保存", font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: whisper_layout, row: 2, col: 2)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_whisper_dir_config])]
    pub whisper_save_btn: nwg::Button,

    // --- 关于与更新窗口 ---
    #[nwg_control(size: (560, 500), position: (400, 400), title: "关于 & 检查更新", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_about_window])]
    pub about_window: nwg::Window,

    #[nwg_layout(parent: about_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub about_layout: nwg::GridLayout,

    #[nwg_control(parent: about_window, text: "Voice2Type", font: Some(&data.font_bold))]
    #[nwg_layout_item(layout: about_layout, row: 0, col: 0, col_span: 2)]
    pub app_name_label: nwg::Label,

    #[nwg_control(parent: about_window, text: "作者：guchang233", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 0, col: 2, col_span: 1)]
    pub author_label: nwg::Label,

    #[nwg_control(parent: about_window, text: "当前版本:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 1, col: 0, col_span: 1)]
    pub current_ver_label: nwg::Label,

    #[nwg_control(parent: about_window, text: "", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 1, col: 1, col_span: 1)]
    pub current_ver_val: nwg::Label,

    #[nwg_control(parent: about_window, text: "访问 GitHub 仓库", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 1, col: 2, col_span: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::open_github])]
    pub github_btn: nwg::Button,

    #[nwg_control(parent: about_window, text: "最新版本:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 2, col: 0, col_span: 1)]
    pub latest_ver_label: nwg::Label,

    #[nwg_control(parent: about_window, text: "未检测", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 2, col: 1, col_span: 2)]
    pub latest_ver_val: nwg::Label,

    #[nwg_control(parent: about_window, text: "更新日志与项目说明:", font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: about_layout, row: 3, col: 0, col_span: 3)]
    pub changelog_label: nwg::Label,

    #[nwg_control(parent: about_window, text: "", flags: "VISIBLE", readonly: true, font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 4, col: 0, col_span: 3, row_span: 3)]
    pub changelog_text: nwg::TextBox,

    #[nwg_control(parent: about_window, text: "检测新版本", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 7, col: 0, col_span: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::do_check_update])]
    pub check_update_btn: nwg::Button,

    #[nwg_control(parent: about_window, text: "忽略此版本", enabled: false, font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: about_layout, row: 7, col: 1, col_span: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::ignore_update])]
    pub ignore_btn: nwg::Button,

    #[nwg_control(parent: about_window, text: "立即更新", enabled: false, font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: about_layout, row: 7, col: 2, col_span: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::start_update])]
    pub start_update_btn: nwg::Button,

    #[nwg_control(parent: about_window, range: 0..100, pos: 0)]
    #[nwg_layout_item(layout: about_layout, row: 8, col: 0, col_span: 3)]
    pub update_progress: nwg::ProgressBar,

    #[nwg_resource(family: "DengXian", size: 16, weight: 400)]
    pub font_normal: nwg::Font,

    #[nwg_resource(family: "DengXian", size: 16, weight: 600)]
    pub font_medium: nwg::Font,

    #[nwg_resource(family: "DengXian", size: 26, weight: 700)]
    pub font_bold: nwg::Font,

    #[nwg_control(size: (520, 160), position: (350, 300), title: "模型设置", flags: "WINDOW", icon: Some(&data.icon))]
    #[nwg_events(OnWindowClose: [Voice2TypeApp::hide_model_settings_window])]
    pub model_settings_window: nwg::Window,

    #[nwg_layout(parent: model_settings_window, spacing: 10, margin: [20, 20, 20, 20])]
    pub model_settings_layout: nwg::GridLayout,

    #[nwg_control(parent: model_settings_window, text: "语音转文字模型:", font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: model_settings_layout, row: 0, col: 0)]
    pub voice_model_label: nwg::Label,

    #[nwg_control(parent: model_settings_window, font: Some(&data.font_normal))]
    #[nwg_layout_item(layout: model_settings_layout, row: 0, col: 1, col_span: 2)]
    pub voice_model_combo: nwg::ComboBox<String>,

    #[nwg_control(parent: model_settings_window, text: "保存", font: Some(&data.font_medium))]
    #[nwg_layout_item(layout: model_settings_layout, row: 1, col: 1)]
    #[nwg_events(OnButtonClick: [Voice2TypeApp::save_model_settings])]
    pub model_settings_save_btn: nwg::Button,

    pub update_info: RefCell<Option<update::UpdateInfo>>,

    #[nwg_control]
    #[nwg_events( OnNotice: [Voice2TypeApp::on_check_notice] )]
    pub check_notice: nwg::Notice,

    #[nwg_control]
    #[nwg_events( OnNotice: [Voice2TypeApp::on_progress_notice] )]
    pub progress_notice: nwg::Notice,

    // Thread communication state
    pub check_result:
        RefCell<Option<Arc<Mutex<Option<anyhow::Result<update::UpdateCheckResult>>>>>>,
    pub progress_data: RefCell<Option<Arc<Mutex<(u64, u64)>>>>,

    // 状态
    pub config_manager: RefCell<Option<Arc<ConfigManager>>>,

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
        app.allow_emoji_item
            .set_checked(config_manager.allow_emoji());
        app.allow_punct_item
            .set_checked(config_manager.allow_punctuation());

        let show_log = config_manager.show_log();
        app.log_item.set_checked(show_log);

        // Output Language settings
        let out_lang = config_manager.output_language();
        match out_lang.as_str() {
            "zh" => {
                app.output_lang_zh_item.set_checked(true);
                app.output_lang_auto_item.set_checked(false);
                app.output_lang_en_item.set_checked(false);
            }
            "en" => {
                app.output_lang_en_item.set_checked(true);
                app.output_lang_auto_item.set_checked(false);
                app.output_lang_zh_item.set_checked(false);
            }
            _ => {
                // "auto" or others
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
            let config_autostart = config_manager.autostart_enabled();
            let new_autostart = enabled || config_autostart;
            app.autostart_item.set_checked(new_autostart);
            if new_autostart != config_autostart {
                if let Some(mgr) = &*app.config_manager.borrow() {
                    mgr.set_autostart_enabled(new_autostart);
                    mgr.save_or_notify();
                }
            }
        }

        app.indicator_item
            .set_checked(config_manager.enable_indicator());

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

        let st_mode = config_manager.streaming_trigger_mode();
        if st_mode == "toggle" {
            app.streaming_trigger_hold_item.set_checked(false);
            app.streaming_trigger_toggle_item.set_checked(true);
        } else {
            app.streaming_trigger_hold_item.set_checked(true);
            app.streaming_trigger_toggle_item.set_checked(false);
        }
        app.streaming_indicator_item
            .set_checked(config_manager.streaming_enable_indicator());
        app.streaming_allow_punct_item
            .set_checked(config_manager.streaming_allow_punctuation());
        app.streaming_allow_emoji_item
            .set_checked(config_manager.streaming_allow_emoji());
        let st_lang = config_manager.streaming_output_language();
        app.streaming_lang_auto_item.set_checked(st_lang.is_empty() || st_lang == "auto");
        app.streaming_lang_zh_item.set_checked(st_lang == "zh");
        app.streaming_lang_en_item.set_checked(st_lang == "en");

        match config_manager.streaming_post_process_mode().as_str() {
            crate::config::STREAMING_POST_AI => {
                app.streaming_post_ai_item.set_checked(true);
                app.streaming_post_local_item.set_checked(false);
                app.streaming_post_none_item.set_checked(false);
            }
            crate::config::STREAMING_POST_NONE => {
                app.streaming_post_ai_item.set_checked(false);
                app.streaming_post_local_item.set_checked(false);
                app.streaming_post_none_item.set_checked(true);
            }
            _ => {
                app.streaming_post_ai_item.set_checked(false);
                app.streaming_post_local_item.set_checked(true);
                app.streaming_post_none_item.set_checked(false);
            }
        }

        Voice2TypeApp::populate_hotkey_combo(
            &app.streaming_hotkey_combo,
            config_manager.streaming_hotkey(),
            5,
        );

        let resources = [
            ("豆包 ASR 1.0 小时版", crate::config::STREAMING_RESOURCE_BIGASR_DURATION),
            ("豆包 ASR 1.0 并发版", "volc.bigasr.sauc.concurrent"),
            ("豆包 ASR 2.0 小时版", crate::config::STREAMING_RESOURCE_SEEDASR_DURATION),
            ("豆包 ASR 2.0 并发版", "volc.seedasr.sauc.concurrent"),
        ];
        let current_rid = config_manager.streaming_resource_id();
        let mut rid_idx = 0usize;
        for (i, (label, id)) in resources.iter().enumerate() {
            app.streaming_resource_combo.push(label.to_string());
            if *id == current_rid.as_str() {
                rid_idx = i;
            }
        }
        app.streaming_resource_combo.set_selection(Some(rid_idx));

        // 初始隐藏窗口
        app.hotkey_window.set_visible(false);
        app.streaming_hotkey_window.set_visible(false);
        app.streaming_resource_window.set_visible(false);
        app.whisper_window.set_visible(false);
        app.about_window.set_visible(false);
        app.current_ver_val.set_text(CURRENT_VERSION);

        // 确保主锚点窗口也不可见
        app.window.set_visible(false);

        // 初始隐藏窗口
        app.key_config_window.set_visible(false);
        app.model_settings_window.set_visible(false);

        // 初始化热键下拉框
        let hotkeys = vec![
            ("F1", 0x70),
            ("F2", 0x71),
            ("F3", 0x72),
            ("F4", 0x73),
            ("F5", 0x74),
            ("F6", 0x75),
            ("F7", 0x76),
            ("F8", 0x77),
            ("F9", 0x78),
            ("F10", 0x79),
            ("F11", 0x7A),
            ("F12", 0x7B),
            ("CAPS LOCK", 0x14),
            ("LEFT CTRL", 0xA2),
            ("RIGHT CTRL", 0xA3),
            ("LEFT ALT", 0xA4),
            ("RIGHT ALT", 0xA5),
            ("V", 0x56),
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

        // 检查 API Key 是否为空 (首次运行，仅当不是本地离线模型时进行检查)
        if config_manager.needs_api_key() && config_manager.get_api_key().is_empty() {
            #[cfg(target_os = "windows")]
            unsafe {
                use windows::core::PCWSTR;
                use windows::Win32::UI::WindowsAndMessaging::{
                    MessageBoxW, IDYES, MB_ICONINFORMATION, MB_YESNO,
                };
                let title = "需要配置 API Key\0".encode_utf16().collect::<Vec<u16>>();
                let msg = "还没有检测到 API Key。\n是否打开 SiliconFlow 控制台获取？\0"
                    .encode_utf16()
                    .collect::<Vec<u16>>();
                let ret = MessageBoxW(
                    None,
                    PCWSTR(msg.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    MB_YESNO | MB_ICONINFORMATION,
                );
                if ret == IDYES {
                    let _ = open::that("https://cloud.siliconflow.cn/account/ak");
                }
            }
            app.key_config_window.set_visible(true);
        }

        // 启动自动检测
        app.do_check_update();

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
            mgr.save_or_notify();
        }
    }

    fn toggle_punctuation(&self) {
        let new_state = !self.allow_punct_item.checked();
        self.allow_punct_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_allow_punctuation(new_state);
            mgr.save_or_notify();
        }
    }

    fn set_output_inject(&self) {
        self.output_inject_item.set_checked(true);
        self.output_clipboard_item.set_checked(false);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_output_mode("inject".to_string());
            mgr.save_or_notify();
        }
    }

    fn set_output_clipboard(&self) {
        self.output_inject_item.set_checked(false);
        self.output_clipboard_item.set_checked(true);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_output_mode("clipboard".to_string());
            mgr.save_or_notify();
        }
    }

    fn toggle_log(&self) {
        let new_state = !self.log_item.checked();
        self.log_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_show_log(new_state);
            mgr.save_or_notify();
        }

        #[cfg(target_os = "windows")]
        {
            // 使用let绑定创建一个生命周期更长的值
            let config_ref = self.config_manager.borrow();
            let config = config_ref.as_ref().map(|v| &**v);
            crate::log_set_enabled(new_state, config);
        }
    }

    fn toggle_autostart(&self) {
        let new_state = !self.autostart_item.checked();
        self.autostart_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_autostart_enabled(new_state);
            mgr.save_or_notify();
        }
        #[cfg(target_os = "windows")]
        unsafe {
            if new_state {
                let ok = crate::win_utils::set_autostart(true);
                if ok.is_err() {
                    self.autostart_item.set_checked(false);
                    if let Some(mgr) = &*self.config_manager.borrow() {
                        mgr.set_autostart_enabled(false);
                        mgr.save_or_notify();
                    }
                    nwg::simple_message("设置失败", "开机自动启动设置失败，请检查权限。");
                } else {
                    nwg::simple_message("已启用", "已设置为开机自动启动。");
                }
            } else {
                let ok = crate::win_utils::set_autostart(false);
                if ok.is_err() {
                    self.autostart_item.set_checked(true);
                    if let Some(mgr) = &*self.config_manager.borrow() {
                        mgr.set_autostart_enabled(true);
                        mgr.save_or_notify();
                    }
                    nwg::simple_message("设置失败", "取消开机自动启动失败，请检查权限。");
                } else {
                    nwg::simple_message("已关闭", "已取消开机自动启动。");
                }
            }
        }
    }

    fn toggle_indicator(&self) {
        let new_state = !self.indicator_item.checked();
        self.indicator_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_enable_indicator(new_state);
            mgr.save_or_notify();
        }

        #[cfg(target_os = "windows")]
        {
            if new_state {
                if crate::INDICATOR.get().is_none() {
                    if let Some(mgr) = &*self.config_manager.borrow() {
                        let _ = crate::INDICATOR.set(crate::indicator::StatusIndicator::new(
                            mgr.indicator_fade_duration(),
                            mgr.indicator_error_duration(),
                            mgr.indicator_success_duration(),
                        ));
                    }
                }
            } else {
                if let Some(ind) = crate::INDICATOR.get() {
                    ind.set_state(crate::indicator::IndicatorState::Hidden);
                }
            }
        }
    }

    fn set_trigger_hold(&self) {
        self.trigger_hold_item.set_checked(true);
        self.trigger_toggle_item.set_checked(false);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_trigger_mode("hold".to_string());
            mgr.save_or_notify();
        }
    }

    fn set_trigger_toggle(&self) {
        self.trigger_hold_item.set_checked(false);
        self.trigger_toggle_item.set_checked(true);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_trigger_mode("toggle".to_string());
            mgr.save_or_notify();
        }
    }

    fn populate_hotkey_combo(combo: &nwg::ComboBox<String>, current_vk: u32, default_index: usize) {
        let hotkeys = [
            ("F1", 0x70_u32),
            ("F2", 0x71),
            ("F3", 0x72),
            ("F4", 0x73),
            ("F5", 0x74),
            ("F6", 0x75),
            ("F7", 0x76),
            ("F8", 0x77),
            ("F9", 0x78),
            ("F10", 0x79),
            ("F11", 0x7A),
            ("F12", 0x7B),
            ("CAPS LOCK", 0x14),
            ("LEFT CTRL", 0xA2),
            ("RIGHT CTRL", 0xA3),
            ("LEFT ALT", 0xA4),
            ("RIGHT ALT", 0xA5),
            ("V", 0x56),
        ];
        let mut selected = default_index;
        for (i, (name, vk)) in hotkeys.iter().enumerate() {
            combo.push(name.to_string());
            if *vk == current_vk {
                selected = i;
            }
        }
        combo.set_selection(Some(selected));
    }

    fn hotkey_vk_list() -> Vec<u32> {
        vec![
            0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B, 0x14, 0xA2,
            0xA3, 0xA4, 0xA5, 0x56,
        ]
    }

    fn set_streaming_trigger_hold(&self) {
        self.streaming_trigger_hold_item.set_checked(true);
        self.streaming_trigger_toggle_item.set_checked(false);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_streaming_trigger_mode("hold".to_string());
            mgr.save_or_notify();
        }
    }

    fn set_streaming_trigger_toggle(&self) {
        self.streaming_trigger_hold_item.set_checked(false);
        self.streaming_trigger_toggle_item.set_checked(true);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_streaming_trigger_mode("toggle".to_string());
            mgr.save_or_notify();
        }
    }

    fn toggle_streaming_indicator(&self) {
        let new_state = !self.streaming_indicator_item.checked();
        self.streaming_indicator_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_streaming_enable_indicator(new_state);
            mgr.save_or_notify();
        }
    }

    fn toggle_streaming_punctuation(&self) {
        let new_state = !self.streaming_allow_punct_item.checked();
        self.streaming_allow_punct_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_streaming_allow_punctuation(new_state);
            mgr.save_or_notify();
        }
    }

    fn toggle_streaming_emoji(&self) {
        let new_state = !self.streaming_allow_emoji_item.checked();
        self.streaming_allow_emoji_item.set_checked(new_state);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_streaming_allow_emoji(new_state);
            mgr.save_or_notify();
        }
    }

    fn set_streaming_post_ai(&self) {
        self.streaming_post_ai_item.set_checked(true);
        self.streaming_post_local_item.set_checked(false);
        self.streaming_post_none_item.set_checked(false);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_streaming_post_process_mode(crate::config::STREAMING_POST_AI.to_string());
            mgr.save_or_notify();
        }
    }

    fn set_streaming_post_local(&self) {
        self.streaming_post_ai_item.set_checked(false);
        self.streaming_post_local_item.set_checked(true);
        self.streaming_post_none_item.set_checked(false);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_streaming_post_process_mode(crate::config::STREAMING_POST_LOCAL.to_string());
            mgr.save_or_notify();
        }
    }

    fn set_streaming_post_none(&self) {
        self.streaming_post_ai_item.set_checked(false);
        self.streaming_post_local_item.set_checked(false);
        self.streaming_post_none_item.set_checked(true);
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_streaming_post_process_mode(crate::config::STREAMING_POST_NONE.to_string());
            mgr.save_or_notify();
        }
    }

    fn set_streaming_lang_auto(&self) {
        if !self.streaming_lang_auto_item.checked() {
            self.streaming_lang_auto_item.set_checked(true);
            self.streaming_lang_zh_item.set_checked(false);
            self.streaming_lang_en_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_streaming_output_language("auto".to_string());
                mgr.save_or_notify();
            }
        }
    }

    fn set_streaming_lang_zh(&self) {
        if !self.streaming_lang_zh_item.checked() {
            self.streaming_lang_zh_item.set_checked(true);
            self.streaming_lang_auto_item.set_checked(false);
            self.streaming_lang_en_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_streaming_output_language("zh".to_string());
                mgr.save_or_notify();
            }
        }
    }

    fn set_streaming_lang_en(&self) {
        if !self.streaming_lang_en_item.checked() {
            self.streaming_lang_en_item.set_checked(true);
            self.streaming_lang_auto_item.set_checked(false);
            self.streaming_lang_zh_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_streaming_output_language("en".to_string());
                mgr.save_or_notify();
            }
        }
    }

    fn show_streaming_hotkey_window(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            Self::populate_hotkey_combo(
                &self.streaming_hotkey_combo,
                mgr.streaming_hotkey(),
                5,
            );
        }
        self.streaming_hotkey_window.set_visible(true);
        self.streaming_hotkey_window.set_focus();
    }

    fn hide_streaming_hotkey_window(&self) {
        self.streaming_hotkey_window.set_visible(false);
    }

    fn save_streaming_hotkey_config(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            let vks = Self::hotkey_vk_list();
            if let Some(idx) = self.streaming_hotkey_combo.selection() {
                if idx < vks.len() {
                    mgr.set_streaming_hotkey(vks[idx]);
                    mgr.save_or_notify();
                }
            }
        }
        nwg::simple_message("已保存", "流式识别热键已保存，请重启程序后生效。");
    }

    fn show_streaming_resource_window(&self) {
        self.streaming_resource_window.set_visible(true);
        self.streaming_resource_window.set_focus();
    }

    fn hide_streaming_resource_window(&self) {
        self.streaming_resource_window.set_visible(false);
    }

    fn save_streaming_resource_config(&self) {
        let ids = [
            crate::config::STREAMING_RESOURCE_BIGASR_DURATION,
            "volc.bigasr.sauc.concurrent",
            crate::config::STREAMING_RESOURCE_SEEDASR_DURATION,
            "volc.seedasr.sauc.concurrent",
        ];
        if let Some(mgr) = &*self.config_manager.borrow() {
            if let Some(idx) = self.streaming_resource_combo.selection() {
                if idx < ids.len() {
                    mgr.set_streaming_resource_id(ids[idx].to_string());
                    mgr.save_or_notify();
                }
            }
        }
        nwg::simple_message("已保存", "流式识别资源 ID 已保存。");
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

    fn show_hotkey_window(&self) {
        self.hotkey_window.set_visible(true);
        self.hotkey_window.set_focus();
    }

    fn hide_hotkey_window(&self) {
        self.hotkey_window.set_visible(false);
    }

    fn show_whisper_window(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            self.whisper_path_input.set_text(&mgr.local_whisper_dir());
        }
        self.whisper_window.set_visible(true);
        self.whisper_window.set_focus();
    }

    fn hide_whisper_window(&self) {
        self.whisper_window.set_visible(false);
    }

    fn browse_whisper_dir(&self) {
        let mut dialog = nwg::FileDialog::default();
        if let Err(e) = nwg::FileDialog::builder()
            .action(nwg::FileDialogAction::OpenDirectory)
            .title("选择本地 Whisper 根目录")
            .build(&mut dialog)
        {
            nwg::simple_message("错误", &format!("无法创建文件夹选择对话框: {}", e));
            return;
        }

        let current = self.whisper_path_input.text();
        if !current.is_empty() && std::path::Path::new(&current).is_dir() {
            let _ = dialog.set_default_folder(&current);
        }

        if dialog.run(Some(&self.window)) {
            if let Ok(path) = dialog.get_selected_item() {
                self.whisper_path_input
                    .set_text(&path.to_string_lossy());
            }
        }
    }

    fn save_whisper_dir_config(&self) {
        let Some(mgr) = &*self.config_manager.borrow() else {
            return;
        };
        let path = self.whisper_path_input.text().trim().to_string();
        if path.is_empty() {
            nwg::simple_message("提示", "请先选择或输入 Whisper 根目录。");
            return;
        }
        mgr.set_local_whisper_dir(path);
        match crate::whisper_local::LocalWhisper::ensure_layout(mgr) {
            Ok(()) => {
                mgr.save_or_notify();
                let status = crate::whisper_local::LocalWhisper::status_message(mgr);
                nwg::simple_message("已保存", &status);
            }
            Err(e) => {
                mgr.save_or_notify();
                nwg::simple_message("目录已保存", &format!("{}\n\n{}", e, mgr.local_whisper_dir()));
            }
        }
    }

    fn open_whisper_dir(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            let dir = mgr.local_whisper_dir();
            if dir.is_empty() {
                nwg::simple_message("提示", "尚未设置目录，请先保存路径。");
                return;
            }
            let path = std::path::PathBuf::from(&dir);
            if path.is_dir() {
                let _ = open::that(path);
            } else {
                nwg::simple_message("提示", &format!("目录不存在:\n{}", dir));
            }
        }
    }

    fn repaste_last(&self) {
        let Some(text) = history::last() else {
            nwg::simple_message("提示", "还没有识别记录。");
            return;
        };
        let Some(mgr) = self.config_manager.borrow().clone() else {
            return;
        };
        let _ = match OutputHandler::repaste(text, &mgr) {
            Ok(()) => nwg::simple_message("已粘贴", "上一条识别结果已重新输入。"),
            Err(e) => nwg::simple_message("失败", &format!("粘贴失败: {}", e)),
        };
    }

    fn show_mic_window(&self) {
        let mut items = vec!["系统默认".to_string()];
        let mut selected = 0usize;
        let current = self
            .config_manager
            .borrow()
            .as_ref()
            .map(|m| m.input_device())
            .unwrap_or_default();

        let host = cpal::default_host();
        if let Ok(devices) = host.input_devices() {
            for device in devices {
                if let Ok(name) = device.name() {
                    if name == current {
                        selected = items.len();
                    }
                    items.push(name);
                }
            }
        }

        self.mic_combo.set_collection(items);
        self.mic_combo.set_selection(Some(selected));
        self.mic_window.set_visible(true);
        self.mic_window.set_focus();
    }

    fn hide_mic_window(&self) {
        self.mic_window.set_visible(false);
    }

    fn save_mic_config(&self) {
        let Some(mgr) = &*self.config_manager.borrow() else {
            return;
        };
        let idx = self.mic_combo.selection().unwrap_or(0);
        let name = if idx == 0 {
            String::new()
        } else {
            let host = cpal::default_host();
            host.input_devices()
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|d| d.name().ok())
                .nth(idx - 1)
                .unwrap_or_default()
        };
        mgr.set_input_device(name);
        mgr.save_or_notify();
        nwg::simple_message("已保存", "麦克风设置已保存，请重启程序后生效。");
    }

    fn show_key_config_window(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            self.key_siliconflow_input.set_text(&mgr.get_siliconflow_api_key());
            self.key_groq_input.set_text(&mgr.get_groq_api_key());
            self.key_doubao_input.set_text(&mgr.get_doubao_api_key());
        }
        self.key_config_window.set_visible(true);
        self.key_config_window.set_focus();
    }

    fn hide_key_config_window(&self) {
        self.key_config_window.set_visible(false);
    }

    fn save_key_config(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            mgr.set_siliconflow_api_key(self.key_siliconflow_input.text());
            mgr.set_groq_api_key(self.key_groq_input.text());
            mgr.set_doubao_api_key(self.key_doubao_input.text());
            mgr.save_or_notify();
            nwg::simple_message("已保存", "API 密钥已保存。");
        }
    }

    fn show_model_settings_window(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            let voice_models = vec![
                "SiliconFlow - FunAudioLLM/SenseVoiceSmall".to_string(),
                "SiliconFlow - TeleAI/TeleSpeechASR".to_string(),
                "Groq - whisper-large-v3".to_string(),
                "本地 Whisper (离线)".to_string(),
            ];
            let voice_model_ids = vec![
                "FunAudioLLM/SenseVoiceSmall",
                "TeleAI/TeleSpeechASR",
                "whisper-large-v3",
                "local-whisper",
            ];
            let current_voice = mgr.get_model_id();
            let voice_idx = voice_model_ids.iter().position(|m| *m == current_voice).unwrap_or(0);
            self.voice_model_combo.set_collection(voice_models);
            self.voice_model_combo.set_selection(Some(voice_idx));
        }
        self.model_settings_window.set_visible(true);
        self.model_settings_window.set_focus();
    }

    fn hide_model_settings_window(&self) {
        self.model_settings_window.set_visible(false);
    }

    fn save_model_settings(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            let voice_model_ids = vec![
                "FunAudioLLM/SenseVoiceSmall",
                "TeleAI/TeleSpeechASR",
                "whisper-large-v3",
                "local-whisper",
            ];
            if let Some(idx) = self.voice_model_combo.selection() {
                if idx < voice_model_ids.len() {
                    let model_id = voice_model_ids[idx];
                    if model_id == "local-whisper" {
                        if !mgr.has_local_whisper_dir() {
                            nwg::simple_message("本地 Whisper", "请先设置 Whisper 根目录。");
                            self.show_whisper_window();
                            return;
                        }
                    }
                    mgr.set_model_name(model_id.to_string());
                }
            }

            mgr.save_or_notify();
            nwg::simple_message("已保存", "模型设置已保存。");
        }
    }

    fn save_hotkey_config(&self) {
        if let Some(mgr) = &*self.config_manager.borrow() {
            let hotkeys_vks = vec![
                0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B, 0x14, 0xA2,
                0xA3, 0xA4, 0xA5, 0x56,
            ];
            if let Some(idx) = self.hotkey_win_combo.selection() {
                if idx < hotkeys_vks.len() {
                    mgr.set_hotkey(hotkeys_vks[idx]);
                    mgr.save_or_notify();
                }
            }
        }
        nwg::simple_message("已保存", "热键已保存，部分场景可能需要重启后生效。");
    }

    fn set_out_lang_auto(&self) {
        if !self.output_lang_auto_item.checked() {
            self.output_lang_auto_item.set_checked(true);
            self.output_lang_zh_item.set_checked(false);
            self.output_lang_en_item.set_checked(false);
            if let Some(mgr) = &*self.config_manager.borrow() {
                mgr.set_output_language("auto".to_string());
                mgr.save_or_notify();
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
                mgr.save_or_notify();
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
                mgr.save_or_notify();
            }
        }
    }

    fn show_update_window(&self) {
        self.about_window.set_visible(true);
        self.about_window.set_focus();
        self.do_check_update();
    }

    fn hide_update_window(&self) {
        self.about_window.set_visible(false);
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
                Ok(result) => {
                    let info = result.info;
                    // 保存 info 到 RefCell
                    *self.update_info.borrow_mut() = Some(info.clone());

                    self.changelog_text.set_text(&info.body);

                    if result.has_update {
                         self.latest_ver_val.set_text(&info.version);
                         self.start_update_btn.set_enabled(true);
                         self.ignore_btn.set_enabled(true);

                         // 如果窗口不可见（后台检测），且未忽略，则提示
                         if !self.about_window.visible() {
                             let (ignored, last_check) =
                                 if let Some(mgr) = &*self.config_manager.borrow() {
                                     (mgr.ignored_version(), mgr.last_check_time())
                                 } else {
                                     (String::new(), 0)
                                 };

                             let now = std::time::SystemTime::now()
                                 .duration_since(std::time::UNIX_EPOCH)
                                 .unwrap()
                                 .as_secs();

                             if info.version != ignored && (now > last_check + 86400) {
                                 #[cfg(target_os = "windows")]
                                 unsafe {
                                     use windows::core::PCWSTR;
                                     use windows::Win32::UI::WindowsAndMessaging::{
                                         MessageBoxW, IDYES, MB_ICONINFORMATION, MB_YESNO,
                                     };
                                     let title = "发现新版本\0".encode_utf16().collect::<Vec<u16>>();
                                     let msg =
                                         format!("新版本 {} 已发布！\n是否查看详情？\0", info.version)
                                             .encode_utf16()
                                             .collect::<Vec<u16>>();
                                     let ret = MessageBoxW(
                                         None,
                                         PCWSTR(msg.as_ptr()),
                                         PCWSTR(title.as_ptr()),
                                         MB_YESNO | MB_ICONINFORMATION,
                                     );

                                     // 记录提示时间
                                     if let Some(mgr) = &*self.config_manager.borrow() {
                                         mgr.set_last_check_time(now);
                                         mgr.save_or_notify();
                                     }

                                     if ret == IDYES {
                                         self.show_update_window();
                                     }
                                 }
                             }
                         }
                    } else {
                        self.latest_ver_val.set_text(&format!("{} (当前已是最新)", info.version));
                        self.start_update_btn.set_enabled(false);
                        self.ignore_btn.set_enabled(false);
                    }
                }
                Err(e) => {
                    self.latest_ver_val.set_text("检测失败");
                    self.ignore_btn.set_enabled(false);
                    if self.about_window.visible() {
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
                mgr.save_or_notify();
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
                let current_exe =
                    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let parent_dir = current_exe
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
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
                        use windows::core::PCWSTR;
                        use windows::Win32::UI::WindowsAndMessaging::{
                            MessageBoxW, MB_ICONERROR, MB_OK,
                        };
                        let title = "更新失败\0".encode_utf16().collect::<Vec<u16>>();
                        let msg = format!("下载失败: {}\0", e)
                            .encode_utf16()
                            .collect::<Vec<u16>>();
                        MessageBoxW(
                            None,
                            PCWSTR(msg.as_ptr()),
                            PCWSTR(title.as_ptr()),
                            MB_OK | MB_ICONERROR,
                        );
                    }
                    return;
                }

                if let Err(e) = update::install_update(&target_path) {
                    #[cfg(target_os = "windows")]
                    unsafe {
                        use windows::core::PCWSTR;
                        use windows::Win32::UI::WindowsAndMessaging::{
                            MessageBoxW, MB_ICONERROR, MB_OK,
                        };
                        let title = "更新失败\0".encode_utf16().collect::<Vec<u16>>();
                        let msg = format!("安装失败: {}\0", e)
                            .encode_utf16()
                            .collect::<Vec<u16>>();
                        MessageBoxW(
                            None,
                            PCWSTR(msg.as_ptr()),
                            PCWSTR(title.as_ptr()),
                            MB_OK | MB_ICONERROR,
                        );
                    }
                    return;
                }

                #[cfg(target_os = "windows")]
                unsafe {
                    use windows::core::PCWSTR;
                    use windows::Win32::UI::WindowsAndMessaging::{
                        MessageBoxW, MB_ICONINFORMATION, MB_OK,
                    };
                    let title = "更新成功\0".encode_utf16().collect::<Vec<u16>>();
                    let msg = "更新已完成，程序将立即重启。\0"
                        .encode_utf16()
                        .collect::<Vec<u16>>();
                    MessageBoxW(
                        None,
                        PCWSTR(msg.as_ptr()),
                        PCWSTR(title.as_ptr()),
                        MB_OK | MB_ICONINFORMATION,
                    );

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
        self.about_window.set_visible(true);
        self.about_window.set_focus();
        self.do_check_update();
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

}

impl Voice2TypeApp {
    fn on_tick(&self) {
        #[cfg(target_os = "windows")]
        {
            if crate::should_uncheck_log_menu_and_reset() {
                self.log_item.set_checked(false);
                if let Some(mgr) = &*self.config_manager.borrow() {
                    mgr.set_show_log(false);
                    mgr.save_or_notify();
                }
            }

            let messages: Vec<(String, String)> = PENDING_TRAY_MESSAGES
                .lock()
                .map(|mut q| std::mem::take(&mut *q))
                .unwrap_or_default();
            if let Some(hwnd) = self.window.handle.hwnd() {
                let hwnd = windows::Win32::Foundation::HWND(hwnd as _);
                for (title, body) in messages {
                    unsafe {
                        crate::win_utils::show_tray_balloon(hwnd, &title, &body);
                    }
                }
            }
        }
    }
}
