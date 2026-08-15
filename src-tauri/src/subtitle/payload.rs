//! 拉取接口的返回负载：快照与主题。
//!
//! 字幕窗口通过 `subtitle_snapshot(windowId)` / `subtitle_theme(windowId)`
//! 主动拉取；负载为编译期定型的 Serialize 结构体（camelCase）。

use serde::Serialize;

use crate::config::{SubtitleElement, SubtitleTheme, SubtitleWindow};
use crate::subtitle::state::{SharedState, Snapshot, TranslationView};

/// A/B 源文本状态
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatePayload {
    pub full_text: String,
    pub definite: String,
    pub indefinite: String,
    pub tail: String,
    pub history: Vec<String>,
    pub speaker: String,
}

impl From<&crate::subtitle::state::SourceState> for SourceStatePayload {
    fn from(s: &crate::subtitle::state::SourceState) -> Self {
        Self {
            full_text: s.full_text.clone(),
            definite: s.definite.clone(),
            indefinite: s.indefinite.clone(),
            tail: s.tail.clone(),
            history: s.history.clone(),
            speaker: s.speaker.clone(),
        }
    }
}

/// 单窗口译文视图
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationViewPayload {
    pub history: Vec<String>,
    pub current: String,
}

impl From<&TranslationView> for TranslationViewPayload {
    fn from(v: &TranslationView) -> Self {
        Self {
            history: v.history.clone(),
            current: v.current.clone(),
        }
    }
}

/// 快照负载（subtitle_snapshot 命令返回）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPayload {
    pub window_id: String,
    pub running: bool,
    pub version: u64,
    pub status: String,
    pub dual: bool,
    pub a: SourceStatePayload,
    pub b: SourceStatePayload,
    pub translation: TranslationViewPayload,
}

impl SnapshotPayload {
    pub fn build(window_id: &str, state: &SharedState) -> Self {
        let snap = state.read();
        SnapshotPayload::from_snapshot(window_id, &snap, state.version())
    }

    fn from_snapshot(window_id: &str, snap: &Snapshot, version: u64) -> Self {
        let translation = snap
            .translation
            .get(window_id)
            .map(TranslationViewPayload::from)
            .unwrap_or(TranslationViewPayload {
                history: Vec::new(),
                current: String::new(),
            });
        Self {
            window_id: window_id.to_string(),
            running: snap.running,
            version,
            status: snap.status.clone(),
            dual: snap.dual,
            a: SourceStatePayload::from(&snap.a),
            b: SourceStatePayload::from(&snap.b),
            translation,
        }
    }
}

/// 窗口控制标志
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowFlagsPayload {
    pub always_on_top: bool,
    pub click_through: bool,
    pub obs_mode: bool,
    pub auto_fit: bool,
}

/// 主题负载（subtitle_theme 命令返回）：窗口标志 + 主题 + 元素 + 翻译配置。
///
/// `SubtitleTheme` / `SubtitleElement` 等配置结构体自身已按 camelCase 序列化，
/// 与配置文件（settings.json / save_config）中的形态一致，前端只需一份解析逻辑。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePayload {
    pub window_id: String,
    pub flags: WindowFlagsPayload,
    pub theme: SubtitleTheme,
    pub elements: Vec<SubtitleElement>,
    pub translation: crate::config::SubtitleTranslationConfig,
}

impl ThemePayload {
    pub fn build(window: &SubtitleWindow) -> Self {
        Self {
            window_id: window.id.clone(),
            flags: WindowFlagsPayload {
                always_on_top: window.always_on_top,
                click_through: window.click_through,
                obs_mode: window.obs_mode,
                auto_fit: window.auto_fit,
            },
            theme: window.theme.clone(),
            elements: window.elements.clone(),
            translation: window.translation.clone(),
        }
    }
}

/// 轻量信号负载（subtitle-signal 事件）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalPayload {
    /// "text"：文本/译文变化 → 拉快照；"theme"：配置变化 → 拉主题；"session"：会话启停 → 两者都拉
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u64,
}
