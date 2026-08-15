//! 权威快照：中央引擎的唯一真相源。
//!
//! 渲染窗口不接收逐帧负载，而是通过 `subtitle_snapshot` 命令按需拉取；
//! 引擎在任何变更后 `bump()` 版本号并通过通知器发出轻量信号（信号-拉取协议）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

/// 历史行最大保留数
pub const HISTORY_LIMIT: usize = 8;
/// 说话人分轮：两次发声间隔超过该阈值视为新轮次
const SPEAKER_TURN_GAP_MS: u64 = 2500;
/// 行宽（字符数）：长行超过该宽度自动切行滚动
pub const LINE_WIDTH_CHARS: usize = 40;
/// B 源（双源模式麦克风）固定说话人标签
pub const SOURCE_B_SPEAKER: &str = "麦克风";

// ==================== 文本切分（纯函数，可测试） ====================

/// 切分完整句段：返回 (完整句段列表, 未完结尾部)
pub fn split_complete_sentences(text: &str) -> (Vec<String>, String) {
    let mut sentences = Vec::new();
    let mut last_end = 0usize;
    let mut tail_start = 0usize;
    let chars: Vec<char> = text.chars().collect();
    for (idx, &c) in chars.iter().enumerate() {
        if matches!(c, '。' | '！' | '？' | '!' | '?' | ';' | '；' | '\n') {
            let end = idx + 1;
            let sentence: String = chars[last_end..end].iter().collect();
            let trimmed = sentence.trim().to_string();
            if is_meaningful(&trimmed) {
                sentences.push(trimmed);
            }
            last_end = end;
            tail_start = end;
        }
    }
    let tail: String = chars[tail_start..].iter().collect();
    (sentences, tail)
}

/// 长行按行宽强制切行：大量文本自动换行滚动（顶行滚出历史）。
pub fn split_by_width(text: &str, limit: usize) -> (Vec<String>, String) {
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut idx = 0usize;
    while idx + limit <= chars.len() {
        let mut end = idx + limit;
        // 英文/带空格文本：在行尾区间内找最近的空格断行，避免切断单词
        if chars[end - 1].is_alphanumeric() {
            let start = idx + limit * 3 / 4;
            for i in (start..end).rev() {
                if chars[i] == ' ' {
                    end = i;
                    break;
                }
            }
        }
        if end == idx {
            end = idx + limit;
        }
        let line: String = chars[idx..end].iter().collect();
        let trimmed = line.trim().to_string();
        if !trimmed.is_empty() {
            lines.push(trimmed);
        }
        idx = end;
        while idx < chars.len() && chars[idx] == ' ' {
            idx += 1;
        }
    }
    let tail: String = chars[idx..].iter().collect();
    (lines, tail)
}

/// 判断文本是否含有实质内容（字母/数字/CJK 等）
fn is_meaningful(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_alphanumeric() || (c as u32) > 0x2E80)
}

// ==================== 跟踪器 ====================

/// 增量句段跟踪器：从 definite 流中切出完整句段并维护历史。
struct LineTracker {
    history: VecDeque<String>,
    consumed_chars: usize,
}

impl LineTracker {
    fn new() -> Self {
        Self {
            history: VecDeque::new(),
            consumed_chars: 0,
        }
    }

    /// 喂入 definite 文本，返回本帧新定稿的行（标点断句 + 行宽强制切行）
    fn feed(&mut self, definite: &str) -> Vec<String> {
        let mut finalized = Vec::new();
        let chars: Vec<char> = definite.chars().collect();
        if self.consumed_chars > chars.len() {
            // ASR 文本回退（异常）：重置
            self.history.clear();
            self.consumed_chars = 0;
        }
        let new_part: String = chars[self.consumed_chars..].iter().collect();
        if new_part.is_empty() {
            return finalized;
        }
        let (complete, tail) = split_complete_sentences(&new_part);
        for s in complete {
            self.push_history(s.clone());
            finalized.push(s);
        }
        self.consumed_chars += new_part.chars().count() - tail.chars().count();

        if tail.chars().count() >= LINE_WIDTH_CHARS {
            let (lines, new_tail) = split_by_width(&tail, LINE_WIDTH_CHARS);
            for line in lines {
                self.push_history(line.clone());
                finalized.push(line);
            }
            self.consumed_chars += tail.chars().count() - new_tail.chars().count();
        }
        finalized
    }

    fn push_history(&mut self, line: String) {
        if self.history.len() >= HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.history.push_back(line);
    }

    fn history(&self) -> Vec<String> {
        self.history.iter().cloned().collect()
    }

    /// 未成句的定稿尾部（当前行定稿部分）
    fn tail(&self, definite: &str) -> String {
        let chars: Vec<char> = definite.chars().collect();
        let start = self.consumed_chars.min(chars.len());
        chars[start..].iter().collect()
    }
}

/// 说话人分轮跟踪器：按停顿间隔自动递增说话人编号
struct SpeakerTracker {
    index: u32,
    label: String,
    last_text_at: Option<Instant>,
}

impl SpeakerTracker {
    fn new() -> Self {
        Self {
            index: 0,
            label: String::new(),
            last_text_at: None,
        }
    }

    fn feed(&mut self, has_text: bool) {
        if !has_text {
            return;
        }
        let new_turn = self
            .last_text_at
            .map_or(true, |t| t.elapsed() > Duration::from_millis(SPEAKER_TURN_GAP_MS));
        if new_turn {
            self.index = (self.index + 1).min(8);
            self.label = format!("说话人{}", self.index);
        }
        self.last_text_at = Some(Instant::now());
    }

    fn label(&self) -> &str {
        &self.label
    }
}

// ==================== 快照 ====================

/// 单个音源的文本状态（快照中的可序列化视图）
#[derive(Debug, Clone, Default)]
pub struct SourceState {
    pub full_text: String,
    pub definite: String,
    pub indefinite: String,
    pub tail: String,
    pub history: Vec<String>,
    pub speaker: String,
}

/// 单窗口译文视图（写入快照，窗口拉取时直接读取）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranslationView {
    pub history: Vec<String>,
    pub current: String,
}

/// 音源标识
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    A,
    B,
}

/// 权威快照：运行状态 + A/B 源文本 + 逐窗口译文 + 内部跟踪器
pub struct Snapshot {
    pub running: bool,
    pub status: String,
    pub dual: bool,
    pub a: SourceState,
    pub b: SourceState,
    /// windowId → 译文视图（只有配置了翻译引擎的窗口有值）
    pub translation: std::collections::HashMap<String, TranslationView>,
    // 内部跟踪器（不对外暴露）
    tracker_a: LineTracker,
    tracker_b: LineTracker,
    speaker_a: SpeakerTracker,
}

impl Snapshot {
    pub fn new(dual: bool) -> Self {
        Self {
            running: false,
            status: String::new(),
            dual,
            a: SourceState::default(),
            b: SourceState::default(),
            translation: std::collections::HashMap::new(),
            tracker_a: LineTracker::new(),
            tracker_b: LineTracker::new(),
            speaker_a: SpeakerTracker::new(),
        }
    }

    /// 重置文本状态（新会话开始）
    pub fn reset(&mut self, dual: bool, status: &str) {
        self.running = true;
        self.dual = dual;
        self.status = status.to_string();
        self.a = SourceState::default();
        self.b = SourceState::default();
        self.translation.clear();
        self.tracker_a = LineTracker::new();
        self.tracker_b = LineTracker::new();
        self.speaker_a = SpeakerTracker::new();
    }

    /// 应用一帧 ASR 结果，返回本帧新定稿的行列表（供转录记录）
    pub fn apply_frame(&mut self, source: Source, definite: &str, indefinite: &str, full_text: &str) -> Vec<String> {
        let target = if source == Source::A { &mut self.a } else { &mut self.b };
        target.full_text = full_text.to_string();
        target.definite = definite.to_string();
        target.indefinite = indefinite.to_string();
        let tracker = if source == Source::A {
            &mut self.tracker_a
        } else {
            &mut self.tracker_b
        };
        let finalized = tracker.feed(definite);
        target.history = tracker.history();
        target.tail = tracker.tail(definite);
        if source == Source::B {
            target.speaker = SOURCE_B_SPEAKER.to_string();
        } else {
            self.speaker_a.feed(!(definite.is_empty() && indefinite.is_empty()));
            target.speaker = self.speaker_a.label().to_string();
        }
        finalized
    }
}

// ==================== 共享状态（版本 + 通知器） ====================

type Notifier = dyn Fn() + Send + Sync;

/// 引擎与翻译任务共享的状态句柄：读快照、变更后 bump 版本并触发通知器。
pub struct SharedState {
    inner: RwLock<Snapshot>,
    version: AtomicU64,
    notifier: RwLock<Option<Arc<Notifier>>>,
}

impl SharedState {
    pub fn new(dual: bool) -> Self {
        Self {
            inner: RwLock::new(Snapshot::new(dual)),
            version: AtomicU64::new(0),
            notifier: RwLock::new(None),
        }
    }

    /// 安装变更通知器（引擎安装「向可见字幕窗口发信号」的闭包）
    pub fn set_notifier(&self, notifier: Arc<Notifier>) {
        *self.notifier.write().unwrap() = Some(notifier);
    }

    pub fn read(&self) -> RwLockReadGuard<'_, Snapshot> {
        self.inner.read().unwrap()
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, Snapshot> {
        self.inner.write().unwrap()
    }

    /// 版本号 +1 并触发通知器，返回新版本
    pub fn bump(&self) -> u64 {
        let v = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(n) = self.notifier.read().unwrap().as_ref() {
            n();
        }
        v
    }

    /// 不触发通知的静默版本+1（批量初始化时用）
    pub fn bump_silent(&self) -> u64 {
        self.version.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// 更新某窗口的译文视图（外部已持有写锁或自行加锁）
    pub fn set_translation(&self, window_id: &str, view: TranslationView) {
        if let Ok(mut snap) = self.inner.write() {
            snap.translation.insert(window_id.to_string(), view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_by_width_chunks_long_cjk_text() {
        let text = "一二三四五六七八九十".repeat(10); // 100 字
        let (lines, tail) = split_by_width(&text, 40);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].chars().count(), 40);
        assert_eq!(lines[1].chars().count(), 40);
        assert_eq!(tail.chars().count(), 20);
    }

    #[test]
    fn split_by_width_breaks_at_space_for_english() {
        let text = "hello world this is a fairly long english sentence that keeps going";
        let (lines, tail) = split_by_width(text, 40);
        assert!(!lines.is_empty(), "长文本应切出至少一行");
        for line in &lines {
            assert!(line.chars().count() <= 40, "每行不超过宽度限制");
        }
        let all: String = lines.join(" ") + " " + &tail;
        assert_eq!(all.replace(' ', ""), text.replace(' ', ""), "内容不丢失");
    }

    #[test]
    fn split_by_width_short_text_stays_tail() {
        let (lines, tail) = split_by_width("短文本", 40);
        assert!(lines.is_empty());
        assert_eq!(tail, "短文本");
    }

    #[test]
    fn snapshot_apply_frame_tracks_history_and_tail() {
        let state = SharedState::new(false);
        {
            let mut snap = state.write();
            let finalized = snap.apply_frame(Source::A, "大家好。今天天气", "不错", "大家好。今天天气不错");
            assert_eq!(finalized, vec!["大家好。".to_string()]);
            assert_eq!(snap.a.history, vec!["大家好。".to_string()]);
            assert_eq!(snap.a.tail, "今天天气");
            assert_eq!(snap.a.indefinite, "不错");
            assert_eq!(snap.a.speaker, "说话人1");
        }
        let v = state.bump_silent();
        assert_eq!(v, 1);
    }

    #[test]
    fn snapshot_translation_roundtrip() {
        let state = SharedState::new(false);
        state.set_translation(
            "primary",
            TranslationView {
                history: vec!["Hello".to_string()],
                current: "wor".to_string(),
            },
        );
        let snap = state.read();
        assert_eq!(snap.translation.get("primary").unwrap().current, "wor");
    }
}
