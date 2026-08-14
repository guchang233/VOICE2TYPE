//! 实时字幕子系统
//!
//! 模块划分：
//! - [`window`]：场景窗口管理（动态创建、属性、事件、配置推送）
//! - [`session`]：字幕会话（单源/双音源 → ASR → 句段/说话人 → 翻译 → 帧广播 + 转录）
//! - [`transcript`]：会话转录存储与 TXT/SRT/MD 序列化
//! - [`translate`]：可插拔翻译引擎 + 增量翻译流水线

mod session;
pub mod transcript;
pub mod translate;
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Mutex};

use crate::config::{ConfigManager, SubtitleSceneConfig};
use crate::subtitle::transcript::Transcript;

pub use window::{
    apply_scene_window_props, attach_scene_window_events, build_config_payload,
    ensure_scene_window, push_scene_config, scene_window_label, DEFAULT_SCENE_ID,
};

/// 字幕服务：会话编排 + 场景窗口管理 + 转录存储
pub struct SubtitleService {
    app_handle: Arc<Mutex<Option<AppHandle>>>,
    config: Arc<ConfigManager>,
    running: Arc<AtomicBool>,
    stop_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    /// 会话代际：每次 start 递增，旧会话收尾时不误伤新会话的窗口
    session_gen: Arc<std::sync::atomic::AtomicU64>,
    /// 最近一次会话的转录（会话结束后保留，供导出；下次会话启动时替换）
    transcript: Arc<StdMutex<Option<Arc<StdMutex<Transcript>>>>>,
    /// 启停串行化锁：防止快速连续 toggle/start/stop 竞态创建多个会话
    toggle_lock: Arc<Mutex<()>>,
}

impl SubtitleService {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        Self {
            app_handle: Arc::new(Mutex::new(None)),
            config,
            running: Arc::new(AtomicBool::new(false)),
            stop_tx: Arc::new(Mutex::new(None)),
            session_gen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            transcript: Arc::new(StdMutex::new(None)),
            toggle_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.lock().await = Some(handle);
    }

    /// 应用启动时为主场景（静态 subtitle 窗口）挂接事件
    pub fn init_default_scene_window(&self, app: &AppHandle) {
        if let Some(window) = app.get_webview_window("subtitle") {
            attach_scene_window_events(&window, DEFAULT_SCENE_ID, &self.config);
        }
    }

    /// 当前转录（无会话时返回 None）
    pub fn transcript(&self) -> Option<Arc<StdMutex<Transcript>>> {
        self.transcript.lock().unwrap().clone()
    }

    /// 清空转录
    pub fn clear_transcript(&self) {
        *self.transcript.lock().unwrap() = None;
    }

    /// 启动字幕会话（串行化，避免竞态重复创建会话）
    pub async fn start(&self, config: Arc<ConfigManager>) -> Result<(), String> {
        let _guard = self.toggle_lock.lock().await;
        if self.running.load(Ordering::SeqCst) {
            return Err("字幕已在运行".to_string());
        }
        self.start_inner(config).await
    }

    /// 停止字幕会话（串行化）
    pub async fn stop(&self) -> Result<(), String> {
        let _guard = self.toggle_lock.lock().await;
        self.stop_inner().await
    }

    /// 切换字幕会话：运行中则停止，否则启动（串行化，返回切换后的运行状态）
    pub async fn toggle(&self, config: Arc<ConfigManager>) -> Result<bool, String> {
        let _guard = self.toggle_lock.lock().await;
        if self.running.load(Ordering::SeqCst) {
            self.stop_inner().await?;
            Ok(false)
        } else {
            self.start_inner(config).await?;
            Ok(true)
        }
    }

    async fn start_inner(&self, config: Arc<ConfigManager>) -> Result<(), String> {
        let handle = self.app_handle.lock().await;
        let app = handle.as_ref().ok_or("App handle not ready")?.clone();
        drop(handle);

        let cfg = config.get_config();
        let scenes: Vec<SubtitleSceneConfig> = cfg
            .subtitle
            .subtitle_scenes
            .iter()
            .filter(|s| s.enabled)
            .cloned()
            .collect();
        if scenes.is_empty() {
            return Err("没有启用的字幕场景，请先在字幕设置中启用至少一个场景".to_string());
        }
        let scene_ids: Vec<String> = scenes.iter().map(|s| s.id.clone()).collect();

        // 1. 确保窗口存在、应用窗口属性、推送配置、显示
        for scene in &scenes {
            if let Some(window) = ensure_scene_window(&app, scene, &config).await {
                apply_scene_window_props(&window, scene);
                push_scene_config(&app, scene);
                let _ = window.show();
            }
        }

        self.running.store(true, Ordering::SeqCst);
        let session_gen = self.session_gen.fetch_add(1, Ordering::SeqCst) + 1;

        // 2. 新建本次会话的转录存储
        let transcript = Arc::new(StdMutex::new(Transcript::new()));
        *self.transcript.lock().unwrap() = Some(transcript.clone());

        // 3. 会话生命周期事件
        let _ = app.emit(
            "subtitle-session-started",
            serde_json::json!({
                "sceneIds": scene_ids,
                "dual": cfg.subtitle.subtitle_audio_source == "dual",
            }),
        );

        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        {
            let mut tx = self.stop_tx.lock().await;
            *tx = Some(stop_tx);
        }

        let running = self.running.clone();
        let app_handle = app.clone();
        let scene_ids_task = scene_ids.clone();
        let config_task = config.clone();
        let gen_flag = self.session_gen.clone();

        tokio::spawn(async move {
            if let Err(e) = session::run_session(
                app_handle.clone(),
                config_task.clone(),
                scene_ids_task.clone(),
                running.clone(),
                &mut stop_rx,
                transcript,
            )
            .await
            {
                log::error!("[字幕] 会话错误: {}", e);
                session::push_status(
                    &app_handle,
                    &scene_ids_task,
                    Some(&format!("错误: {}", e)),
                    false,
                );
            }

            running.store(false, Ordering::SeqCst);

            // 只有最新会话才执行收尾（旧会话收尾不隐藏新会话刚显示的窗口）
            if gen_flag.load(Ordering::SeqCst) == session_gen {
                for scene_id in &scene_ids_task {
                    let label = scene_window_label(scene_id);
                    if let Some(window) = app_handle.get_webview_window(&label) {
                        if scene_id == DEFAULT_SCENE_ID {
                            // 主场景窗口保留（隐藏），动态场景窗口销毁以释放 WebView 内存
                            let _ = window.hide();
                        } else {
                            let _ = window.destroy();
                        }
                    }
                }
                let _ = app_handle.emit("subtitle-session-stopped", ());
            }
        });

        Ok(())
    }

    async fn stop_inner(&self) -> Result<(), String> {
        self.running.store(false, Ordering::SeqCst);

        let mut tx = self.stop_tx.lock().await;
        if let Some(sender) = tx.take() {
            let _ = sender.send(()).await;
        }

        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
