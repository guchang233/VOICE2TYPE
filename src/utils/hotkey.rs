use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::ConfigManager;

#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VIRTUAL_KEY};

#[cfg(not(target_os = "windows"))]
use rdev::{listen, Event, EventType, Key};

/// 输入消息类型
#[derive(Debug)]
pub enum InputMessage {
    StartRecording,
    StopRecording,
    CancelRecording,
}

/// 启动热键监听线程
#[cfg(target_os = "windows")]
pub fn start_hotkey_listener(tx: mpsc::Sender<InputMessage>, config: Arc<ConfigManager>) {
    thread::spawn(move || {
        let vk_esc = VIRTUAL_KEY(0x1B); // ESC
        let mut is_pressed = false;
        let mut is_recording = false;

        loop {
            let current_vk = config.hotkey();
            let vk_main = VIRTUAL_KEY(current_vk as u16);
            let trigger_mode = config.trigger_mode();

            // GetAsyncKeyState 的最高位表示按键当前是否按下
            let main_down = unsafe { (GetAsyncKeyState(vk_main.0 as i32) as u16 & 0x8000) != 0 };
            let esc_down = unsafe { (GetAsyncKeyState(vk_esc.0 as i32) as u16 & 0x8000) != 0 };

            if trigger_mode == "hold" {
                // 按住输入模式
                if main_down {
                    if !is_pressed {
                        is_pressed = true;
                        let _ = tx.blocking_send(InputMessage::StartRecording);
                    }

                    if esc_down {
                        let _ = tx.blocking_send(InputMessage::CancelRecording);
                        // 等待 ESC 松开，防止连续触发
                        while unsafe { (GetAsyncKeyState(vk_esc.0 as i32) as u16 & 0x8000) != 0 } {
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                } else {
                    if is_pressed {
                        is_pressed = false;
                        let _ = tx.blocking_send(InputMessage::StopRecording);
                    }
                }
            } else if trigger_mode == "toggle" {
                // 按下输入模式
                static mut LAST_STOP_TIME: Option<Instant> = None;
                const COOLDOWN_DURATION: Duration = Duration::from_millis(500); // 500ms冷却时间

                if main_down && !is_pressed {
                    is_pressed = true;

                    // 检查是否在冷却期内
                    let in_cooldown = unsafe {
                        if let Some(last_stop) = LAST_STOP_TIME {
                            last_stop.elapsed() < COOLDOWN_DURATION
                        } else {
                            false
                        }
                    };

                    if !in_cooldown {
                        if is_recording {
                            // 停止录音
                            let _ = tx.blocking_send(InputMessage::StopRecording);
                            is_recording = false;
                            // 记录停止时间
                            unsafe {
                                LAST_STOP_TIME = Some(Instant::now());
                            }
                        } else {
                            // 开始录音
                            let _ = tx.blocking_send(InputMessage::StartRecording);
                            is_recording = true;
                        }
                    }
                } else if !main_down {
                    is_pressed = false;
                }

                if esc_down && is_recording {
                    let _ = tx.blocking_send(InputMessage::CancelRecording);
                    is_recording = false;
                    // 等待 ESC 松开，防止连续触发
                    while unsafe { (GetAsyncKeyState(vk_esc.0 as i32) as u16 & 0x8000) != 0 } {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }

            thread::sleep(Duration::from_millis(15));
        }
    });
}

/// 启动热键监听线程（非Windows平台）
#[cfg(not(target_os = "windows"))]
pub fn start_hotkey_listener(tx: mpsc::Sender<InputMessage>, config: Arc<ConfigManager>) {
    thread::spawn(move || {
        let mut is_f2_pressed = false;
        let mut is_recording = false;

        if let Err(error) = listen(move |event| {
            let trigger_mode = config.trigger_mode();
            match event.event_type {
                EventType::KeyPress(Key::F2) => {
                    if !is_f2_pressed {
                        is_f2_pressed = true;

                        if trigger_mode == "hold" {
                            // 按住输入模式
                            let _ = tx.blocking_send(InputMessage::StartRecording);
                        } else if trigger_mode == "toggle" {
                            // 按下输入模式
                            if is_recording {
                                // 停止录音
                                let _ = tx.blocking_send(InputMessage::StopRecording);
                                is_recording = false;
                            } else {
                                // 开始录音
                                let _ = tx.blocking_send(InputMessage::StartRecording);
                                is_recording = true;
                            }
                        }
                    }
                }
                EventType::KeyRelease(Key::F2) => {
                    if is_f2_pressed {
                        is_f2_pressed = false;

                        if trigger_mode == "hold" {
                            // 只有在按住输入模式下才发送停止录音消息
                            let _ = tx.blocking_send(InputMessage::StopRecording);
                        } else if trigger_mode == "toggle" {
                            // 按下输入模式不需要在松开时停止录音
                        }
                    }
                }
                EventType::KeyPress(Key::Escape) => {
                    if (trigger_mode == "hold" && is_f2_pressed)
                        || (trigger_mode == "toggle" && is_recording)
                    {
                        // 如果正在录音，此时按下了 ESC -> 取消
                        let _ = tx.blocking_send(InputMessage::CancelRecording);
                        is_recording = false;
                    }
                }
                _ => {}
            }
        }) {
            eprintln!("全局按键监听错误: {:?}", error);
        }
    });
}
