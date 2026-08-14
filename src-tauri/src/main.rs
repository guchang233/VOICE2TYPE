#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![recursion_limit = "256"]

mod api;
mod app_state;
mod asr;
mod audio;
mod commands;
mod config;
mod history;
mod indicator;
mod notify;
mod output;
mod pipeline;
mod recorder;
mod session;
mod streaming;
mod subtitle;
mod tts;
mod update;
mod utils;
mod whisper_local;
mod win_utils;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use once_cell::sync::OnceCell;
use tauri::{Emitter, Manager};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};

use app_state::AppState;
use config::ConfigManager;
use indicator::StatusIndicator;
use utils::logger;

pub static INDICATOR: OnceCell<StatusIndicator> = OnceCell::new();
pub static CONFIG_GLOBAL: OnceCell<Arc<ConfigManager>> = OnceCell::new();
pub static LOG_MENU_NEEDS_UNCHECK: AtomicBool = AtomicBool::new(false);

pub fn request_uncheck_log_menu() {
    LOG_MENU_NEEDS_UNCHECK.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn main() {
    // 必须在任何 WebView2 初始化之前设置环境变量
    // 禁用 DirectComposition 后，WebView2 回退到 GDI 合成，OBS 等录屏软件的
    // Window Capture (BitBlt) 才能正常捕捉窗口内容（否则会捕捉到纯黑画面）。
    // 副作用：Tauri 透明窗口效果失效，因此 tauri.conf.json 中窗口 transparent 必须为 false。
    if std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_err() {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--disable-direct-composition",
        );
    }

    logger::init_logger();

    let config_manager = Arc::new(ConfigManager::new());
    let _ = CONFIG_GLOBAL.set(config_manager.clone());
    let app_state = Arc::new(AppState::new(config_manager.clone()));

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_os::init())
        .manage(config_manager.clone())
        .manage(app_state.clone())
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::get_models_dir,
            commands::pick_models_directory,
            commands::reset_models_directory,
            commands::open_directory,
            commands::start_recording,
            commands::stop_recording,
            commands::cancel_recording,
            commands::force_cancel,
            commands::get_history,
            commands::remove_history,
            commands::clear_history,
            commands::get_app_status,
            commands::copy_to_clipboard,
            commands::open_subtitle_window,
            commands::start_subtitle,
            commands::stop_subtitle,
            commands::is_subtitle_running,
            commands::toggle_subtitle,
            commands::list_input_devices,
            commands::download_whisper_model,
            commands::cancel_download,
            commands::delete_whisper_model,
            commands::list_available_models,
            commands::download_whisper_binary,
            commands::check_whisper_binary_health,
            commands::check_update,
            commands::download_and_install_update,
            commands::restart_app,
            commands::get_app_version,
            commands::set_subtitle_always_on_top,
            commands::set_subtitle_click_through,
            commands::show_subtitle_window,
            commands::push_subtitle_config,
            commands::get_subtitle_window_status,
            commands::set_subtitle_obs_mode,
            commands::list_subtitle_scenes,
            commands::add_subtitle_scene,
            commands::duplicate_subtitle_scene,
            commands::remove_subtitle_scene,
            commands::get_subtitle_transcript,
            commands::clear_subtitle_transcript,
            commands::export_subtitle_transcript,
            commands::tts_synthesize,
            commands::tts_export,
            commands::tts_list_voices,
            commands::tts_get_voice,
        ])
        .setup(move |app| {
            let app_handle = app.handle();

            // 设置 AppHandle 到日志记录器，启用向前端推送日志事件
            logger::set_app_handle(app_handle.clone());

            {
                let state = app_state.clone();
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    state.set_app_handle(handle).await;
                });
            }

            // 预热模型文件到 OS 文件缓存，加速后续 whisper-cli 模型加载
            {
                let state = app_state.clone();
                tauri::async_runtime::spawn(async move {
                    state.prewarm_model_cache().await;
                });
            }

            // 空闲周期性 OS 缓存保活：全天托盘工具期间，别的程序可能把模型挤出 OS page cache，
            // 导致下次转写又走磁盘 IO。每 10 分钟廉价重读一次（不占应用 RAM，只刷 OS 缓存）。
            {
                let state = app_state.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    // 跳过首次立即触发（启动预热已在上一个任务做过）
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        // prewarm 内部会在模型不存在时安全跳过
                        state.prewarm_model_cache().await;
                    }
                });
            }

            let config_dir = config_manager.config_dir();
            history::init(config_dir);

            if let Err(e) = config_manager.save() {
                log::error!("启动时保存配置失败: {}", e);
            }

            let indicator = StatusIndicator::new(
                config_manager.indicator_fade_duration(),
                config_manager.indicator_error_duration(),
                config_manager.indicator_success_duration(),
            );
            let _ = INDICATOR.set(indicator);

            if let Some(window) = app.get_webview_window("main") {
                let window_clone = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window_clone.hide();
                    }
                });
            }

            // 主场景字幕窗口事件：关闭 → 停用场景；移动/缩放 → 保存位置尺寸
            // （动态场景窗口在创建时由 subtitle 模块自行挂接同样的事件）
            app_state.subtitle.init_default_scene_window(app_handle);

            let show_item = MenuItem::with_id(app.handle(), "show", "显示主窗口", true, None::<&str>)?;
            let quit_item = PredefinedMenuItem::quit(app.handle(), Some("退出"))?;
            let separator = PredefinedMenuItem::separator(app.handle())?;

            let menu = Menu::with_id(app.handle(), "tray-menu")?;
            menu.append(&show_item)?;
            menu.append(&separator)?;
            menu.append(&quit_item)?;

            let tray_app = app.handle();
            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("Voice2Type - 语音输入")
                .on_menu_event(move |app, event| {
                    match event.id().as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick { .. } = event {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(tray_app)?;

            let state_for_hotkey = app_state.clone();
            let config_for_hotkey = config_manager.clone();

            std::thread::spawn(move || {
                let mut is_holding = false;
                let mut toggle_recording = false;

                let mut callback = move |event: rdev::Event| {
                    let batch_hotkey = config_for_hotkey.hotkey();
                    let subtitle_hotkey = config_for_hotkey.subtitle_hotkey();

                    match event.event_type {
                        rdev::EventType::KeyPress(key) => {
                            let vk = key_to_vk(key);

                            if vk == subtitle_hotkey && vk != batch_hotkey && vk != 0 {
                                // 字幕开关热键（默认 F7）：按下切换字幕会话
                                let state = state_for_hotkey.clone();
                                tauri::async_runtime::spawn(async move {
                                    let _ = state.toggle_subtitle().await;
                                });
                            }

                            if vk == batch_hotkey {
                                let trigger_mode = config_for_hotkey.trigger_mode();
                                if trigger_mode == "hold" {
                                    if !is_holding {
                                        is_holding = true;
                                        let state = state_for_hotkey.clone();
                                        tauri::async_runtime::spawn(async move {
                                            let _ = state.start_recording().await;
                                        });
                                    }
                                } else {
                                    toggle_recording = !toggle_recording;
                                    let state = state_for_hotkey.clone();
                                    tauri::async_runtime::spawn(async move {
                                        if toggle_recording {
                                            let _ = state.start_recording().await;
                                        } else {
                                            let _ = state.stop_recording_and_recognize().await;
                                        }
                                    });
                                }
                            }
                        }
                        rdev::EventType::KeyRelease(key) => {
                            let vk = key_to_vk(key);

                            if vk == batch_hotkey {
                                let trigger_mode = config_for_hotkey.trigger_mode();
                                if trigger_mode == "hold" && is_holding {
                                    is_holding = false;
                                    let state = state_for_hotkey.clone();
                                    tauri::async_runtime::spawn(async move {
                                        let _ = state.stop_recording_and_recognize().await;
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                };

                if let Err(e) = rdev::listen(callback) {
                    log::error!("Failed to start rdev listener: {:?}", e);
                }
            });

            let app_handle_notify = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let _ = app_handle_notify.emit("app-ready", ());
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Voice2Type application");
}

fn key_to_vk(key: rdev::Key) -> u32 {
    match key {
        rdev::Key::F1 => 0x70,
        rdev::Key::F2 => 0x71,
        rdev::Key::F3 => 0x72,
        rdev::Key::F4 => 0x73,
        rdev::Key::F5 => 0x74,
        rdev::Key::F6 => 0x75,
        rdev::Key::F7 => 0x76,
        rdev::Key::F8 => 0x77,
        rdev::Key::F9 => 0x78,
        rdev::Key::F10 => 0x79,
        rdev::Key::F11 => 0x7a,
        rdev::Key::F12 => 0x7b,
        // 数字键
        rdev::Key::Num0 => 0x30,
        rdev::Key::Num1 => 0x31,
        rdev::Key::Num2 => 0x32,
        rdev::Key::Num3 => 0x33,
        rdev::Key::Num4 => 0x34,
        rdev::Key::Num5 => 0x35,
        rdev::Key::Num6 => 0x36,
        rdev::Key::Num7 => 0x37,
        rdev::Key::Num8 => 0x38,
        rdev::Key::Num9 => 0x39,
        // 字母键
        rdev::Key::KeyA => 0x41,
        rdev::Key::KeyB => 0x42,
        rdev::Key::KeyC => 0x43,
        rdev::Key::KeyD => 0x44,
        rdev::Key::KeyE => 0x45,
        rdev::Key::KeyF => 0x46,
        rdev::Key::KeyG => 0x47,
        rdev::Key::KeyH => 0x48,
        rdev::Key::KeyI => 0x49,
        rdev::Key::KeyJ => 0x4a,
        rdev::Key::KeyK => 0x4b,
        rdev::Key::KeyL => 0x4c,
        rdev::Key::KeyM => 0x4d,
        rdev::Key::KeyN => 0x4e,
        rdev::Key::KeyO => 0x4f,
        rdev::Key::KeyP => 0x50,
        rdev::Key::KeyQ => 0x51,
        rdev::Key::KeyR => 0x52,
        rdev::Key::KeyS => 0x53,
        rdev::Key::KeyT => 0x54,
        rdev::Key::KeyU => 0x55,
        rdev::Key::KeyV => 0x56,
        rdev::Key::KeyW => 0x57,
        rdev::Key::KeyX => 0x58,
        rdev::Key::KeyY => 0x59,
        rdev::Key::KeyZ => 0x5a,
        _ => 0,
    }
}
