//! 实时字幕子系统（v3：中央引擎 + 信号拉取）
//!
//! 架构总览：
//! - [`state`]：权威快照（RwLock）+ 版本号 + 变更通知器 —— 唯一真相源
//! - [`session`]：会话编排（音源 → 豆包 ASR → 快照更新 → bump 触发信号）
//! - [`translate`]：TranslationHub，逐窗口翻译槽，译文直接写入快照
//! - [`audio`]：单路音源（采集线程 + 豆包 WS + 音频泵）
//! - [`windows`]：字幕窗口生命周期与几何持久化
//! - [`payload`]：拉取接口负载（快照/主题/信号）
//! - [`transcript`]：会话转录与 TXT/SRT/MD 序列化
//! - [`migration`]：旧配置 JSON → v3 模型迁移
//!
//! 数据流：ASR 帧 → 快照（bump 版本）→ 轻量信号 → 字幕窗口拉取快照 → 局部渲染。
//! 不存在逐帧广播与合并发射器。

pub mod audio;
pub mod migration;
pub mod payload;
mod session;
pub mod state;
pub mod transcript;
pub mod translate;
pub mod windows;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Mutex};

use crate::config::{ConfigManager, SubtitleWindow, PRIMARY_WINDOW_ID};
use crate::subtitle::payload::{SignalPayload, SnapshotPayload, ThemePayload};
use crate::subtitle::state::SharedState;
use crate::subtitle::transcript::Transcript;
use crate::subtitle::translate::TranslationHub;

/// 字幕服务：中央引擎 —— 权威状态、会话生命周期、窗口管理、信号发射。
pub struct SubtitleEngine {
    app: Mutex<Option<AppHandle>>,
    config: Arc<ConfigManager>,
    state: Arc<SharedState>,
    running: Arc<AtomicBool>,
    stop_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    /// 会话代际：每次 start 递增，旧会话收尾时不误伤新会话的窗口
    session_gen: Arc<AtomicU64>,
    /// 最近一次会话的转录（会话结束后保留，供导出；下次会话启动时替换）
    transcript: Arc<StdMutex<Option<Arc<StdMutex<Transcript>>>>>,
    /// 启停串行化锁：防止快速连续 toggle/start/stop 竞态创建多个会话
    toggle_lock: Arc<Mutex<()>>,
}

impl SubtitleEngine {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        Self {
            app: Mutex::new(None),
            config,
            state: Arc::new(SharedState::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            stop_tx: Arc::new(Mutex::new(None)),
            session_gen: Arc::new(AtomicU64::new(0)),
            transcript: Arc::new(StdMutex::new(None)),
            toggle_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 注入 AppHandle 并安装快照变更通知器：
    /// 任何版本变化 → 向可见字幕窗口发 `subtitle-signal`（type=text）。
    pub async fn set_app_handle(&self, handle: AppHandle) {
        *self.app.lock().await = Some(handle.clone());

        let app = handle.clone();
        let config = self.config.clone();
        self.state.set_notifier(Arc::new(move || {
            emit_signal(&app, &config, "text", 0);
        }));
    }

    /// 应用启动时为主窗口（静态 subtitle 窗口）挂接事件。
    /// 注意：**不要销毁重建窗口**——OBS 采集源绑定窗口 HWND，
    /// 重建会让 OBS 采集源指向已销毁的 HWND 而黑屏。
    /// OBS 兼容靠「启动时 GDI 回退环境变量（main.rs）+ 运行时不透明黑底背景」实现。
    pub fn init_primary_window(&self, app: &AppHandle) {
        if let Some(window) = app.get_webview_window("subtitle") {
            windows::attach_window_events(&window, PRIMARY_WINDOW_ID, &self.config);
            // 若主窗口开启了 OBS 模式，立即应用不透明黑底
            if let Some(primary) = self
                .config
                .get_subtitle_windows()
                .into_iter()
                .find(|w| w.id == PRIMARY_WINDOW_ID)
            {
                windows::apply_window_props(&window, &primary);
            }
        }
    }

    /// 拉取快照（字幕窗口渲染用）
    pub fn snapshot(&self, window_id: &str) -> SnapshotPayload {
        SnapshotPayload::build(window_id, &self.state)
    }

    /// 拉取窗口主题（字幕窗口配置用）
    pub fn theme(&self, window_id: &str) -> Result<ThemePayload, String> {
        let cfg = self.config.get_config();
        let win = cfg
            .subtitle
            .window(window_id)
            .ok_or_else(|| "字幕窗口不存在".to_string())?;
        Ok(ThemePayload::build(win))
    }

    /// 显示/隐藏指定字幕窗口（显示即视为重新启用该窗口）
    pub async fn show_window(&self, app: &AppHandle, window_id: &str, show: bool) -> Result<(), String> {
        let label = windows::window_label(window_id);
        if !show {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.hide();
            }
            return Ok(());
        }
        let win = self
            .config
            .get_subtitle_windows()
            .into_iter()
            .find(|w| w.id == window_id)
            .ok_or("字幕窗口不存在")?;
        self.config.set_subtitle_window_enabled(window_id, true);
        let _ = self.config.save();
        if let Some(window) = windows::ensure_window(app, &win, &self.config) {
            windows::apply_window_props(&window, &win);
            let _ = window.show();
            let _ = window.set_focus();
            // 重新显示后补发信号：窗口拉取最新主题与快照
            let version = self.state.version();
            let _ = window.emit(
                "subtitle-signal",
                SignalPayload {
                    kind: "theme".to_string(),
                    version,
                },
            );
            let _ = window.emit(
                "subtitle-signal",
                SignalPayload {
                    kind: "session".to_string(),
                    version,
                },
            );
        }
        Ok(())
    }

    /// 设置窗口控制开关（置顶/穿透/OBS/自适应）
    pub fn set_window_flag(&self, app: &AppHandle, window_id: &str, flag: &str, value: bool) -> Result<(), String> {
        self.config.set_subtitle_window_flag(window_id, flag, value);
        let _ = self.config.save();
        let label = windows::window_label(window_id);
        match flag {
            "always_on_top" => {
                if let Some(window) = app.get_webview_window(&label) {
                    let _ = window.set_always_on_top(value);
                }
            }
            "click_through" => {
                if let Some(window) = app.get_webview_window(&label) {
                    let _ = window.set_ignore_cursor_events(value);
                }
            }
            "obs_mode" => {
                // OBS 模式切换 → 立即切换窗口背景（不透明黑底/恢复透明）
                // 并让窗口重拉主题（黑底/关闭模糊策略）。
                // 注意：**不重建窗口**——重建会改变 HWND，导致 OBS 采集源失效变黑。
                // BitBlt 最佳兼容仍需重启应用（GDI 回退环境变量在启动时应用）。
                if let Some(win_cfg) = self
                    .config
                    .get_subtitle_windows()
                    .iter()
                    .find(|w| w.id == window_id)
                    .cloned()
                {
                    if let Some(window) = app.get_webview_window(&label) {
                        windows::apply_background(&window, win_cfg.obs_mode);
                        let _ = window.set_always_on_top(true);
                        let _ = window.emit(
                            "subtitle-signal",
                            SignalPayload {
                                kind: "theme".to_string(),
                                version: self.state.version(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 把最新配置应用到所有已存在的字幕窗口（设置保存后调用）
    pub fn push_theme(&self, app: &AppHandle) {
        let windows_cfg = self.config.get_subtitle_windows();
        for win in &windows_cfg {
            let label = windows::window_label(&win.id);
            let Some(window) = app.get_webview_window(&label) else {
                continue;
            };
            windows::apply_window_props(&window, win);
            let _ = window.emit(
                "subtitle-signal",
                SignalPayload {
                    kind: "theme".to_string(),
                    version: self.state.version(),
                },
            );
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

    // ==================== 会话生命周期 ====================

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
        let handle = self.app.lock().await;
        let app = handle.as_ref().ok_or("App handle not ready")?.clone();
        drop(handle);

        let cfg = config.get_config();
        let enabled: Vec<SubtitleWindow> = cfg.subtitle.enabled_windows();
        if enabled.is_empty() {
            return Err("没有启用的字幕窗口，请先在字幕设置中启用至少一个窗口".to_string());
        }

        // 1. 确保窗口存在、应用窗口属性、显示；窗口显示后自行拉取主题与快照
        for win in &enabled {
            if let Some(window) = windows::ensure_window(&app, win, &config) {
                windows::apply_window_props(&window, win);
                let _ = window.show();
                let _ = window.emit(
                    "subtitle-signal",
                    SignalPayload {
                        kind: "session".to_string(),
                        version: self.state.version(),
                    },
                );
            }
        }

        self.running.store(true, Ordering::SeqCst);
        let session_gen = self.session_gen.fetch_add(1, Ordering::SeqCst) + 1;

        // 2. 新建本次会话的转录存储
        let transcript = Arc::new(StdMutex::new(Transcript::new()));
        *self.transcript.lock().unwrap() = Some(transcript.clone());

        // 3. 会话生命周期事件
        let _ = app.emit("subtitle-session-started", serde_json::json!({ "running": true }));

        // 4. 构建翻译枢纽（只对配置了引擎的窗口建槽）
        let hub = Arc::new(TranslationHub::build(
            config.clone(),
            self.state.clone(),
            &enabled,
        ));

        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        {
            let mut tx = self.stop_tx.lock().await;
            *tx = Some(stop_tx);
        }

        let running = self.running.clone();
        let app_handle = app.clone();
        let config_task = config.clone();
        let gen_flag = self.session_gen.clone();
        let state_task = self.state.clone();
        let app_for_signal = app.clone();
        let config_for_signal = config.clone();

        tokio::spawn(async move {
            if let Err(e) = session::run_session(
                app_handle.clone(),
                config_task.clone(),
                state_task.clone(),
                hub,
                running.clone(),
                &mut stop_rx,
                transcript,
            )
            .await
            {
                log::error!("[字幕] 会话错误: {}", e);
                {
                    let mut snap = state_task.write();
                    snap.status = format!("错误: {}", e);
                    snap.running = false;
                }
                state_task.bump();
            }

            running.store(false, Ordering::SeqCst);

            // 只有最新会话才执行收尾（旧会话收尾不隐藏新会话刚显示的窗口）
            if gen_flag.load(Ordering::SeqCst) == session_gen {
                for win in config_task.get_subtitle_windows() {
                    if !win.enabled {
                        continue;
                    }
                    let label = windows::window_label(&win.id);
                    if let Some(window) = app_handle.get_webview_window(&label) {
                        if win.id == PRIMARY_WINDOW_ID {
                            // 主窗口保留（隐藏），动态窗口销毁以释放 WebView 内存
                            let _ = window.hide();
                        } else {
                            let _ = window.destroy();
                        }
                    }
                }
                let _ = app_handle.emit("subtitle-session-stopped", serde_json::json!({ "running": false }));
                // 让仍显示中的窗口刷新最终状态
                emit_signal(&app_for_signal, &config_for_signal, "session", 0);
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

/// 向所有可见字幕窗口发射轻量信号（kind: text/theme/session）
fn emit_signal(app: &AppHandle, config: &Arc<ConfigManager>, kind: &str, version: u64) {
    for win in config.get_subtitle_windows() {
        if !win.enabled {
            continue;
        }
        let label = windows::window_label(&win.id);
        let Some(window) = app.get_webview_window(&label) else {
            continue;
        };
        // 隐藏窗口跳过：WebView 被节流，事件只会堆积
        if !window.is_visible().unwrap_or(false) {
            continue;
        }
        let _ = window.emit(
            "subtitle-signal",
            SignalPayload {
                kind: kind.to_string(),
                version,
            },
        );
    }
}
