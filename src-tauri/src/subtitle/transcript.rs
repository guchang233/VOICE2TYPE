//! 会话转录：定稿字幕行按 ID 记录，译文按「ID + 语言」回填，导出 TXT / SRT / Markdown。
//!
//! 时间来自切分器：起点 = 这一行开始出现文字的时刻，终点 = 定稿时刻，
//! SRT 时间轴与实际说话时间一致（不再用「下一句起点」估算）。

use std::collections::BTreeMap;

use serde::Serialize;

use crate::subtitle::state::Caption;

/// 转录条数上限：超长会话保持内存有界
const MAX_ENTRIES: usize = 5000;
/// SRT 单条最短显示时长
const MIN_SRT_MS: u64 = 800;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptEntry {
    /// 字幕 ID（与状态板一致，译文据此回填）
    pub id: u64,
    /// 会话内序号（从 1 开始）
    pub index: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    /// "A" 主音源 | "B" 同传麦克风
    pub source: String,
    pub speaker: String,
    pub text: String,
    /// 目标语言 → 译文
    pub translations: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
    next_index: u64,
}

impl Transcript {
    pub fn new() -> Self {
        Self { entries: Vec::new(), next_index: 1 }
    }

    pub fn push(&mut self, caption: &Caption, speaker: &str) -> TranscriptEntry {
        let entry = TranscriptEntry {
            id: caption.id,
            index: self.next_index,
            start_ms: caption.start_ms,
            end_ms: caption.end_ms,
            source: caption.source.tag().to_string(),
            speaker: speaker.to_string(),
            text: caption.text.clone(),
            translations: BTreeMap::new(),
        };
        self.next_index += 1;
        self.entries.push(entry.clone());
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..excess);
        }
        entry
    }

    /// 回填译文；返回是否找到该行
    pub fn set_translation(&mut self, id: u64, lang: &str, text: &str) -> bool {
        // 新译文几乎总是落在末尾附近，倒序查找
        match self.entries.iter_mut().rev().find(|e| e.id == id) {
            Some(e) => {
                e.translations.insert(lang.to_string(), text.to_string());
                true
            }
            None => false,
        }
    }

    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// SRT：起止时间取实际说话时间，过短的条目延长到下一条起点前
    pub fn to_srt(&self) -> String {
        let mut out = String::new();
        for (i, e) in self.entries.iter().enumerate() {
            let next_start = self.entries.get(i + 1).map(|n| n.start_ms).unwrap_or(u64::MAX);
            let mut end = e.end_ms.max(e.start_ms + MIN_SRT_MS);
            if end > next_start && next_start > e.start_ms {
                end = next_start.max(e.start_ms + 1);
            }
            let mut body = with_speaker(&e.speaker, &e.text);
            for t in e.translations.values().filter(|t| !t.is_empty()) {
                body.push('\n');
                body.push_str(t);
            }
            out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(e.start_ms), srt_time(end), body));
        }
        out
    }

    pub fn to_txt(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            out.push_str(&format!("[{}] {}\n", mmss(e.start_ms), with_speaker_colon(&e.speaker, &e.text)));
            for (lang, t) in e.translations.iter().filter(|(_, t)| !t.is_empty()) {
                out.push_str(&format!("[{}] {}: {}\n", mmss(e.start_ms), lang, t));
            }
        }
        out
    }

    pub fn to_md(&self) -> String {
        let mut out = String::from("# 字幕转录记录\n\n");
        for e in &self.entries {
            let speaker = if e.speaker.is_empty() { String::new() } else { format!("**{}** ", e.speaker) };
            out.push_str(&format!("- `[{}]` {}{}\n", mmss(e.start_ms), speaker, e.text));
            for (lang, t) in e.translations.iter().filter(|(_, t)| !t.is_empty()) {
                out.push_str(&format!("  - {}: {}\n", lang, t));
            }
        }
        out
    }
}

fn with_speaker(speaker: &str, text: &str) -> String {
    if speaker.is_empty() { text.to_string() } else { format!("[{}] {}", speaker, text) }
}

fn with_speaker_colon(speaker: &str, text: &str) -> String {
    if speaker.is_empty() { text.to_string() } else { format!("{}: {}", speaker, text) }
}

fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn srt_time(ms: u64) -> String {
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle::state::Source;

    fn cap(id: u64, source: Source, text: &str, s: u64, e: u64) -> Caption {
        Caption { id, source, text: text.into(), start_ms: s, end_ms: e }
    }

    #[test]
    fn translations_attach_by_id_not_position() {
        let mut tr = Transcript::new();
        for i in 1..=12u64 {
            tr.push(&cap(i, Source::A, &format!("第{}句。", i), i * 1000, i * 1000 + 500), "我");
        }
        // 乱序、隔很多行之后到达的译文仍挂到正确的行上
        assert!(tr.set_translation(11, "英文", "Sentence 11."));
        assert!(tr.set_translation(2, "英文", "Sentence 2."));
        assert!(!tr.set_translation(99, "英文", "x"));
        assert_eq!(tr.entries()[10].translations["英文"], "Sentence 11.");
        assert_eq!(tr.entries()[1].translations["英文"], "Sentence 2.");
        assert!(tr.entries()[0].translations.is_empty());
    }

    #[test]
    fn srt_uses_real_times_and_avoids_overlap() {
        let mut tr = Transcript::new();
        tr.push(&cap(1, Source::A, "大家好。", 1000, 1200), "对方");
        tr.push(&cap(2, Source::B, "收到。", 1500, 2600), "我");
        tr.set_translation(1, "英文", "Hello everyone.");
        let srt = tr.to_srt();
        // 第一条过短（200ms）延长，但不越过下一条起点 1500
        assert!(srt.contains("00:00:01,000 --> 00:00:01,500"), "{}", srt);
        assert!(srt.contains("[对方] 大家好。\nHello everyone."));
        assert!(srt.contains("00:00:01,500 --> 00:00:02,600"));
        assert!(tr.to_txt().contains("[00:01] 英文: Hello everyone."));
        assert!(tr.to_md().starts_with("# 字幕转录记录"));
    }
}
