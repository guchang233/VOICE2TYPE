//! 拉取接口负载。
//!
//! 字幕窗口收到 `subtitle-signal`（{type: text|theme|session}）后，
//! 通过 `subtitle_snapshot(windowId)` / `subtitle_theme(windowId)` 主动拉取。
//! 快照里原文与译文以「行」为单位成对出现（同一个字幕 ID），窗口按自己的目标语言取译文。

use serde::Serialize;

use crate::config::{SubtitleElement, SubtitleTheme, SubtitleWindow};
use crate::subtitle::state::{Board, SharedState, Source};

/// 每个音源下发的最近行数（窗口按 maxLines 再裁剪）
const LINES_PER_SOURCE: usize = 8;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinePayload {
    pub id: u64,
    pub text: String,
    /// 该窗口目标语言的译文（未开启翻译或尚未到达时为空）
    pub translation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePayload {
    pub definite: String,
    pub indefinite: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePayload {
    pub speaker: String,
    pub lines: Vec<LinePayload>,
    pub live: LivePayload,
    pub live_translation: String,
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
    /// 该窗口是否开启了翻译
    pub translating: bool,
    pub a: SourcePayload,
    pub b: SourcePayload,
}

impl SnapshotPayload {
    /// `lang`：窗口的目标语言（未开启翻译为 None）
    pub fn build(window_id: &str, lang: Option<&str>, state: &SharedState) -> Self {
        let version = state.version();
        let board = state.read();
        Self {
            window_id: window_id.to_string(),
            running: board.running,
            version,
            status: board.status.clone(),
            dual: board.dual,
            translating: lang.is_some(),
            a: source_payload(&board, Source::A, lang),
            b: source_payload(&board, Source::B, None),
        }
    }
}

fn source_payload(board: &Board, source: Source, lang: Option<&str>) -> SourcePayload {
    let view = lang.and_then(|l| board.translations.get(l));
    let lines = board
        .recent(source, LINES_PER_SOURCE)
        .into_iter()
        .map(|c| LinePayload {
            id: c.id,
            text: c.text.clone(),
            translation: view.and_then(|v| v.finals.get(&c.id)).cloned().unwrap_or_default(),
        })
        .collect();
    let live = &board.live[source.idx()];
    SourcePayload {
        speaker: board.labels[source.idx()].clone(),
        lines,
        live: LivePayload { definite: live.definite.clone(), indefinite: live.indefinite.clone() },
        live_translation: view.map(|v| v.live.clone()).unwrap_or_default(),
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

/// 主题负载（subtitle_theme 命令返回）：窗口标志 + 主题 + 元素 + 翻译配置
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle::captions::Piece;

    #[test]
    fn snapshot_pairs_lines_with_window_language() {
        let state = SharedState::new();
        {
            let mut b = state.write();
            b.reset(true, ["对方".into(), "我".into()], "就绪");
            b.ensure_lang("英文");
            b.ensure_lang("日文");
            let c = b.push_caption(Source::A, Piece { text: "你好。".into(), start_ms: 0, end_ms: 1 });
            b.push_caption(Source::B, Piece { text: "收到。".into(), start_ms: 2, end_ms: 3 });
            b.translations.get_mut("英文").unwrap().finals.insert(c.id, "Hello.".into());
            b.translations.get_mut("日文").unwrap().finals.insert(c.id, "こんにちは。".into());
        }
        let en = SnapshotPayload::build("w1", Some("英文"), &state);
        assert_eq!(en.a.lines[0].translation, "Hello.");
        assert_eq!(en.b.lines[0].text, "收到。");
        assert!(en.b.lines[0].translation.is_empty(), "副音源不翻译");
        let ja = SnapshotPayload::build("w2", Some("日文"), &state);
        assert_eq!(ja.a.lines[0].translation, "こんにちは。");
        let none = SnapshotPayload::build("w3", None, &state);
        assert!(!none.translating && none.a.lines[0].translation.is_empty());
        assert_eq!(none.a.speaker, "对方");
    }
}
