//! 权威状态板：字幕子系统的唯一真相源。
//!
//! - 定稿字幕行（[`Caption`]）带全局唯一 ID；
//! - 译文按「语言 → 字幕 ID」存放，多个字幕窗口选同一目标语言时共享同一份译文；
//! - 任何变更后 [`SharedState::bump`] 版本号并触发通知器，字幕窗口按信号拉取快照。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::subtitle::captions::{LiveLine, Piece};

/// 渲染用的最近字幕行保留数（每个音源各自计数前的总上限）
pub const KEEP_CAPTIONS: usize = 32;

/// 音源：A = 主音源（麦克风或系统声音）；B = 同传模式下的麦克风副音源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    A,
    B,
}

impl Source {
    pub fn tag(self) -> &'static str {
        match self {
            Source::A => "A",
            Source::B => "B",
        }
    }

    pub fn idx(self) -> usize {
        match self {
            Source::A => 0,
            Source::B => 1,
        }
    }
}

/// 一行定稿字幕
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub id: u64,
    pub source: Source,
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// 某个目标语言的译文
#[derive(Debug, Clone, Default)]
pub struct LangView {
    /// 字幕 ID → 译文
    pub finals: HashMap<u64, String>,
    /// 当前行（同声预览）译文
    pub live: String,
}

/// 状态板
pub struct Board {
    pub running: bool,
    pub status: String,
    pub dual: bool,
    /// 各音源的说话人标签（A, B）
    pub labels: [String; 2],
    /// 最近的定稿字幕（两个音源混排，按时间顺序）
    pub captions: VecDeque<Caption>,
    /// 各音源当前行
    pub live: [LiveLine; 2],
    /// A 源当前行代际：每次有字幕定稿 +1，迟到的同声预览译文据此丢弃
    pub live_gen: u64,
    /// 目标语言 → 译文
    pub translations: HashMap<String, LangView>,
    next_id: u64,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    pub fn new() -> Self {
        Self {
            running: false,
            status: String::new(),
            dual: false,
            labels: [String::new(), String::new()],
            captions: VecDeque::new(),
            live: [LiveLine::default(), LiveLine::default()],
            live_gen: 0,
            translations: HashMap::new(),
            next_id: 1,
        }
    }

    /// 新会话：清空文字与译文，保留 ID 递增（跨会话 ID 不复用）
    pub fn reset(&mut self, dual: bool, labels: [String; 2], status: &str) {
        self.running = true;
        self.dual = dual;
        self.labels = labels;
        self.status = status.to_string();
        self.captions.clear();
        self.live = [LiveLine::default(), LiveLine::default()];
        self.live_gen += 1;
        self.translations.clear();
    }

    /// 定稿一段文字：分配 ID、入列并裁剪旧行（同时裁剪对应译文）
    pub fn push_caption(&mut self, source: Source, piece: Piece) -> Caption {
        let caption = Caption {
            id: self.next_id,
            source,
            text: piece.text,
            start_ms: piece.start_ms,
            end_ms: piece.end_ms,
        };
        self.next_id += 1;
        self.captions.push_back(caption.clone());
        while self.captions.len() > KEEP_CAPTIONS {
            if let Some(old) = self.captions.pop_front() {
                for view in self.translations.values_mut() {
                    view.finals.remove(&old.id);
                }
            }
        }
        caption
    }

    /// A 源有新字幕定稿：当前行同声译文暂挂到最后一行上作为过渡（正式译文到达后覆盖），
    /// 并推进代际让迟到的同声预览作废
    pub fn settle_live_translation(&mut self, last_id: u64) {
        self.live_gen += 1;
        for view in self.translations.values_mut() {
            let live = std::mem::take(&mut view.live);
            if !live.is_empty() {
                view.finals.entry(last_id).or_insert(live);
            }
        }
    }

    pub fn ensure_lang(&mut self, lang: &str) {
        self.translations.entry(lang.to_string()).or_default();
    }

    /// 某音源最近 n 行（时间顺序）
    pub fn recent(&self, source: Source, n: usize) -> Vec<&Caption> {
        let mut out: Vec<&Caption> = self.captions.iter().rev().filter(|c| c.source == source).take(n).collect();
        out.reverse();
        out
    }

    /// 某音源最近 n 行原文（翻译上下文用）
    pub fn context_before(&self, id: u64, n: usize) -> Vec<String> {
        let mut out: Vec<String> = self
            .captions
            .iter()
            .rev()
            .filter(|c| c.source == Source::A && c.id < id)
            .take(n)
            .map(|c| c.text.clone())
            .collect();
        out.reverse();
        out
    }
}

// ==================== 共享状态（版本 + 通知器） ====================

type Notifier = dyn Fn() + Send + Sync;

/// 状态板句柄：读写、变更后 bump 版本并通知字幕窗口拉取
pub struct SharedState {
    inner: RwLock<Board>,
    version: AtomicU64,
    notifier: RwLock<Option<Arc<Notifier>>>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Board::new()),
            version: AtomicU64::new(0),
            notifier: RwLock::new(None),
        }
    }

    pub fn set_notifier(&self, notifier: Arc<Notifier>) {
        *self.notifier.write().unwrap_or_else(|e| e.into_inner()) = Some(notifier);
    }

    pub fn read(&self) -> RwLockReadGuard<'_, Board> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, Board> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    /// 版本号 +1 并触发通知器（调用方不得持有锁）
    pub fn bump(&self) -> u64 {
        let v = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        let notifier = self.notifier.read().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(n) = notifier {
            n();
        }
        v
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(t: &str) -> Piece {
        Piece { text: t.into(), start_ms: 0, end_ms: 0 }
    }

    #[test]
    fn ids_are_unique_and_translations_follow_pruning() {
        let mut b = Board::new();
        b.ensure_lang("英文");
        let first = b.push_caption(Source::A, piece("第一句。"));
        b.translations.get_mut("英文").unwrap().finals.insert(first.id, "First.".into());
        for i in 0..KEEP_CAPTIONS {
            b.push_caption(Source::A, piece(&format!("句{}。", i)));
        }
        assert_eq!(b.captions.len(), KEEP_CAPTIONS);
        assert!(!b.translations["英文"].finals.contains_key(&first.id), "裁掉的行译文同步移除");
        let ids: std::collections::HashSet<u64> = b.captions.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), KEEP_CAPTIONS);
    }

    #[test]
    fn live_translation_becomes_provisional_final() {
        let mut b = Board::new();
        b.ensure_lang("英文");
        b.translations.get_mut("英文").unwrap().live = "Hello every".into();
        let gen = b.live_gen;
        let c = b.push_caption(Source::A, piece("大家好。"));
        b.settle_live_translation(c.id);
        assert_eq!(b.translations["英文"].finals[&c.id], "Hello every");
        assert!(b.translations["英文"].live.is_empty());
        assert!(b.live_gen > gen);
    }

    #[test]
    fn recent_and_context_filter_by_source() {
        let mut b = Board::new();
        b.push_caption(Source::A, piece("a1"));
        b.push_caption(Source::B, piece("b1"));
        let a2 = b.push_caption(Source::A, piece("a2"));
        b.push_caption(Source::A, piece("a3"));
        assert_eq!(b.recent(Source::A, 2).iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["a2", "a3"]);
        assert_eq!(b.context_before(a2.id + 1, 5), vec!["a1".to_string(), "a2".to_string()]);
    }
}
