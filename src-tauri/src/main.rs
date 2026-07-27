#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app_state;
mod audio;
mod commands;
mod config;
mod history;
mod indicator;
mod notify;
mod output;
mod recorder;
mod streaming;
mod subtitle;
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

pub static INDICATOR: OnceCell<StatusIndicator> = OnceCell::new();
pub static CONFIG_GLOBAL: OnceCell<Arc<ConfigManager>> = OnceCell::new();
pub static LOG_MENU_NEEDS_UNCHECK: AtomicBool = AtomicBool::new(false);

pub fn request_uncheck_log_menu() {}

fn main() {
    let _ = env_logger::try_init();

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
            commands::start_recording,
            commands::stop_recording,
            commands::cancel_recording,
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
            commands::download_whisper_model,
            commands::list_available_models,
        ])
        .setup(move |app| {
            let app_handle = app.handle();

            {
                let state = app_state.clone();
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    state.set_app_handle(handle).await;
                });
            }

            let config_dir = config_manager.config_dir();
            history::init(config_dir);

            let _ = config_manager.save();

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

            if let Some(window) = app.get_webview_window("subtitle") {
                let window_clone = window.clone();
                let state_for_subtitle = app_state.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window_clone.hide();
                        let state = state_for_subtitle.clone();
                        tauri::async_runtime::spawn(async move {
                            if state.is_subtitle_running() {
                                let _ = state.stop_subtitle().await;
                            }
                        });
                    }
                });
            }

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

                    match event.event_type {
                        rdev::EventType::KeyPress(key) => {
                            let vk = key_to_vk(key);

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
        _ => 0,
    }
}
