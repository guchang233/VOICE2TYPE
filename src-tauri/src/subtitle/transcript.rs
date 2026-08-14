//! 会话实时转录：句段记录、译文同步、TXT/SRT/MD 序列化

use std::time::Instant;

use serde::Serialize;

/// 转录句段上限：防止超长会话导致内存无限增长
const MAX_TRANSCRIPT_SEGMENTS: usize = 2000;

/// 一条转录句段（已定稿的完整句段）
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptSegment {
    /// 序号（从 1 开始）
    pub index: u64,
    /// 会话内起始时间（毫秒）
    pub start_ms: u64,
    /// 音源："A"（主音源）| "B"（双源模式的麦克风副音源）
    pub source: String,
    /// 说话人标签
    pub speaker: String,
    /// 原文
    pub text: String,
    /// 译文（由主场景翻译流水线同步，可能为空）
    pub translation: String,
}

/// 会话转录存储：内存中累积本次会话的定稿句段
pub struct Transcript {
    started_at: Instant,
    segments: Vec<TranscriptSegment>,
    next_index: u64,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            segments: Vec::new(),
            next_index: 1,
        }
    }

    /// 追加一条句段
    pub fn push(&mut self, source: &str, speaker: &str, text: &str) -> TranscriptSegment {
        let seg = TranscriptSegment {
            index: self.next_index,
            start_ms: self.started_at.elapsed().as_millis() as u64,
            source: source.to_string(),
            speaker: speaker.to_string(),
            text: text.to_string(),
            translation: String::new(),
        };
        self.next_index += 1;
        self.segments.push(seg.clone());
        // 超上限时丢弃最旧句段，保持内存有界
        if self.segments.len() > MAX_TRANSCRIPT_SEGMENTS {
            let excess = self.segments.len() - MAX_TRANSCRIPT_SEGMENTS;
            self.segments.drain(..excess);
        }
        seg
    }

    /// 用主场景翻译流水线的历史译文按位置同步 A 源句段的译文。
    /// 返回本次发生变化的 (句段序号, 新译文) 列表（供增量推送）。
    pub fn sync_translations(&mut self, translation_history: &[String]) -> Vec<(u64, String)> {
        let mut changed = Vec::new();
        let mut a_idx = 0usize;
        for seg in self.segments.iter_mut() {
            if seg.source != "A" {
                continue;
            }
            if a_idx < translation_history.len() {
                let t = &translation_history[a_idx];
                if seg.translation != *t {
                    seg.translation = t.clone();
                    changed.push((seg.index, t.clone()));
                }
            }
            a_idx += 1;
        }
        changed
    }

    pub fn segments(&self) -> &[TranscriptSegment] {
        &self.segments
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// 每条句段估算时长（下一句段起点 - 本句段起点；末句默认 3000ms）
    fn segment_duration_ms(&self, idx: usize) -> u64 {
        let cur = self.segments[idx].start_ms;
        let next = self
            .segments
            .get(idx + 1)
            .map(|s| s.start_ms)
            .unwrap_or(cur + 3000);
        next.saturating_sub(cur).max(1000)
    }

    /// 序列化为 SRT 字幕格式
    pub fn to_srt(&self) -> String {
        let mut out = String::new();
        for (i, seg) in self.segments.iter().enumerate() {
            let start = format_srt_time(seg.start_ms);
            let end = format_srt_time(seg.start_ms + self.segment_duration_ms(i));
            let line = if seg.translation.is_empty() {
                format_srt_line(&seg.speaker, &seg.text)
            } else {
                format!("{}\n{}", format_srt_line(&seg.speaker, &seg.text), seg.translation)
            };
            out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, start, end, line));
        }
        out
    }

    /// 序列化为纯文本格式
    pub fn to_txt(&self) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            let time = format_mmss(seg.start_ms);
            let speaker = if seg.speaker.is_empty() {
                String::new()
            } else {
                format!("{}: ", seg.speaker)
            };
            out.push_str(&format!("[{}] {}{}\n", time, speaker, seg.text));
            if !seg.translation.is_empty() {
                out.push_str(&format!("[{}] 译: {}\n", time, seg.translation));
            }
        }
        out
    }

    /// 序列化为 Markdown 格式
    pub fn to_md(&self) -> String {
        let mut out = String::from("# 字幕转录记录\n\n");
        for seg in &self.segments {
            let time = format_mmss(seg.start_ms);
            let speaker = if seg.speaker.is_empty() {
                String::new()
            } else {
                format!("**{}** ", seg.speaker)
            };
            out.push_str(&format!("- `[{}]` {}{}\n", time, speaker, seg.text));
            if !seg.translation.is_empty() {
                out.push_str(&format!("  - 译: {}\n", seg.translation));
            }
        }
        out
    }
}

fn format_mmss(ms: u64) -> String {
    let total_secs = ms / 1000;
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

fn format_srt_time(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms / 60_000) % 60;
    let s = (ms / 1000) % 60;
    let millis = ms % 1000;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, millis)
}

fn format_srt_line(speaker: &str, text: &str) -> String {
    if speaker.is_empty() {
        text.to_string()
    } else {
        format!("[{}] {}", speaker, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_serialization_and_translation_sync() {
        let mut tr = Transcript::new();
        tr.push("A", "说话人1", "大家好。");
        tr.push("B", "麦克风", "收到。");
        // 译文按位置只同步 A 源句段
        tr.sync_translations(&["Hello everyone.".to_string()]);

        assert_eq!(tr.segments()[0].translation, "Hello everyone.");
        assert_eq!(tr.segments()[1].translation, "");

        let srt = tr.to_srt();
        assert!(srt.contains("-->"));
        assert!(srt.contains("[说话人1] 大家好。"));
        assert!(srt.contains("Hello everyone."));
        assert!(srt.contains("00:00:0"));

        let txt = tr.to_txt();
        assert!(txt.contains("译: Hello everyone."));
        assert!(txt.contains("麦克风: 收到。"));

        let md = tr.to_md();
        assert!(md.starts_with("# 字幕转录记录"));
        assert!(md.contains("`[00:00]`"));
    }
}
