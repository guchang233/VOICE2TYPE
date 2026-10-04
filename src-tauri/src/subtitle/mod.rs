//! 实时字幕子系统（v4）
//!
//! 架构总览：
//! - [`audio`]：单路音源（采集线程 + 豆包 WS + 音频泵）
//! - [`captions`]：唯一的断句处 —— definite/indefinite 流 → 定稿字幕行（带真实起止时间）
//! - [`state`]：状态板 —— 字幕行（全局 ID）、当前行、按「语言 → ID」存放的译文
//! - [`translate`]：按目标语言去重的同传调度（定稿按 ID 翻译 + 同声预览节流）
//! - [`transcript`]：会话转录（译文按 ID 回填）与 TXT/SRT/MD 导出
//! - [`session`]：会话编排
//! - [`payload`]：拉取接口负载（快照里原文与译文按行成对）
//! - [`windows`]：字幕窗口生命周期与几何持久化
//! - [`migration`]：旧配置迁移
//!
//! 数据流：ASR 帧 → 切分 → 状态板（bump 版本）→ 轻量信号 → 字幕窗口拉取快照 → 渲染。

pub mod audio;
pub mod captions;
pub mod migration;
pub mod payload;
mod session;
pub mod state;
pub mod transcript;
pub mod translate;
pub mod windows;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use once_cell::sync::OnceCell;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Mutex};

use crate::config::{ConfigManager, SubtitleWindow, PRIMARY_WINDOW_ID};
use crate::subtitle::payload::{SnapshotPayload, ThemePayload};
use crate::subtitle::session::SessionCtx;
use crate::subtitle::state::SharedState;
use crate::subtitle::transcript::Transcript;

/// 引擎弱引用（供窗口关闭回调使用，避免循环强引用）
static SUBTITLE_ENGINE: OnceCell<Weak<SubtitleEngine>> = OnceCell::new();

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
            state: Arc::new(SharedState::new()),
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

    /// 注册引擎弱引用（AppState 创建引擎后调用）
    pub fn register(engine: &Arc<Self>) {
        let _ = SUBTITLE_ENGINE.set(Arc::downgrade(engine));
    }

    /// 窗口关闭回调：停止字幕会话（若在运行）。
    /// 由窗口的 CloseRequested 事件触发，保证「窗口关闭 ⇔ 会话停止 ⇔ 主界面按钮更新」。
    fn close_callback() -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(|| {
            if let Some(engine) = SUBTITLE_ENGINE.get().and_then(|w| w.upgrade()) {
                tauri::async_runtime::spawn(async move {
                    let _ = engine.stop().await;
                });
            }
        })
    }

    /// 应用启动时为主窗口（静态 subtitle 窗口）挂接事件并应用窗口属性。
    pub fn init_primary_window(&self, app: &AppHandle) {
        if let Some(window) = app.get_webview_window("subtitle") {
            windows::attach_window_events(
                &window,
                PRIMARY_WINDOW_ID,
                &self.config,
                Some(Self::close_callback()),
            );
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

    /// 拉取快照（字幕窗口渲染用）：按窗口的目标语言取译文
    pub fn snapshot(&self, window_id: &str) -> SnapshotPayload {
        let cfg = self.config.get_config();
        let tr = cfg
            .subtitle
            .window(window_id)
            .filter(|w| translate::resolve_engine(&w.translation.engine).is_some())
            .map(|w| (w.translation.target_lang.clone(), w.translation.interim));
        let mut snap = SnapshotPayload::build(window_id, tr.as_ref().map(|(l, _)| l.as_str()), &self.state);
        // 同语言的另一个窗口开了同声预览时，状态板里也会有当前句译文；本窗口没开就不显示
        if matches!(tr, Some((_, false))) {
            snap.a.live_translation.clear();
        }
        snap
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

    /// 显示/隐藏指定字幕窗口，与会话状态强关联：
    /// - 隐藏（含标题栏关闭）→ 同时停止字幕会话，主界面按钮回到「开启实时字幕」；
    /// - 显示 → 若会话未运行则自动启动会话（连带显示所有启用的字幕窗口）。
    pub async fn show_window(&self, app: &AppHandle, window_id: &str, show: bool) -> Result<(), String> {
        let label = windows::window_label(window_id);
        if !show {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.hide();
            }
            emit_window_state(app, window_id, false);
            // 强关联：隐藏窗口 = 停止会话
            if self.running.load(Ordering::SeqCst) {
                self.stop().await?;
            }
            return Ok(());
        }
        // 显示 = 重新启用该窗口
        let win = self
            .config
            .get_subtitle_windows()
            .into_iter()
            .find(|w| w.id == window_id)
            .ok_or("字幕窗口不存在")?;
        self.config.set_subtitle_window_enabled(window_id, true);
        let _ = self.config.save();
        // 强关联：会话未运行 → 先启动会话（start 会显示所有启用的窗口）
        if !self.running.load(Ordering::SeqCst) {
            let config = self.config.clone();
            self.start(config).await?;
            return Ok(());
        }
        if let Some(window) =
            windows::ensure_window(app, &win, &self.config, Some(Self::close_callback()))
        {
            windows::apply_window_props(&window, &win);
            let _ = window.show();
            let _ = window.set_focus();
            emit_window_state(app, window_id, true);
            // 重新显示后补发信号：窗口拉取最新主题与快照
            let version = self.state.version();
            windows::signal(&window, window.label(), "theme", version);
            windows::signal(&window, window.label(), "session", version);
        }
        Ok(())
    }

    /// 设置窗口控制开关（置顶/穿透/自适应）
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
            // "obs_mode"：仅保存配置标记，不触碰窗口任何属性。
            // 两种模式的显示完全一致（页面 CSS 近黑背景），
            // 运行时改背景色已被证实会导致部分机器 WebView2 内容不渲染。
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
            windows::signal(&window, window.label(), "theme", self.state.version());
        }
    }

    /// 当前转录（无会话时返回 None）
    pub fn transcript(&self) -> Option<Arc<StdMutex<Transcript>>> {
        self.transcript.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 清空转录
    pub fn clear_transcript(&self) {
        *self.transcript.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
            if let Some(window) = windows::ensure_window(&app, win, &config, Some(Self::close_callback())) {
                windows::apply_window_props(&window, win);
                let _ = window.show();
                emit_window_state(&app, &win.id, true);
                windows::signal(&window, window.label(), "session", self.state.version());
            }
        }

        self.running.store(true, Ordering::SeqCst);
        let session_gen = self.session_gen.fetch_add(1, Ordering::SeqCst) + 1;

        // 2. 新建本次会话的转录存储
        let transcript = Arc::new(StdMutex::new(Transcript::new()));
        *self.transcript.lock().unwrap_or_else(|e| e.into_inner()) = Some(transcript.clone());

        // 3. 会话生命周期事件
        let _ = app.emit("subtitle-session-started", serde_json::json!({ "running": true }));

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

        let ctx = SessionCtx {
            app: app_handle.clone(),
            config: config_task.clone(),
            state: state_task.clone(),
            transcript,
            running: running.clone(),
            windows: enabled.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = session::run_session(ctx, &mut stop_rx).await {
                log::error!("[字幕] 会话错误: {}", e);
                {
                    let mut board = state_task.write();
                    board.status = format!("错误: {}", e);
                    board.running = false;
                }
                state_task.bump();
                // 留出时间让窗口显示错误原因，再进入收尾
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
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
                        // 强关联：同步「窗口已关闭」状态给主界面设置面板
                        emit_window_state(&app_handle, &win.id, false);
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

/// 广播字幕窗口显示状态（主窗口设置面板据此同步「窗口已关闭/显示中」）
fn emit_window_state(app: &AppHandle, window_id: &str, visible: bool) {
    let _ = app.emit(
        "subtitle-window-state",
        serde_json::json!({ "windowId": window_id, "visible": visible }),
    );
}

const MAIN_WINDOW_LABEL: &str = "main";

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
        windows::signal(&window, window.label(), kind, version);
    }
    // 主界面的实时预览也跟随变化拉取（页面只在字幕页可见时才真正拉取）
    windows::signal(app, MAIN_WINDOW_LABEL, kind, version);
}
