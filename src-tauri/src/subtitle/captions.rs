//! 字幕切分：把豆包流式 ASR 的 definite / indefinite 文本流切成稳定的「字幕行」。
//!
//! 这是整个字幕子系统里**唯一**做断句的地方：每一行定稿后由 [`Board`](super::state::Board)
//! 分配全局唯一 ID，原文、译文、转录、SRT 时间都挂在同一个 ID 上，不再出现
//! 「原文与译文各自断句、按位置对齐」造成的错位。
//!
//! 定稿规则（任一满足即定稿）：
//! 1. 句末标点（。！？；!?; 换行）；
//! 2. 行宽超过 [`LINE_WIDTH_CHARS`]（英文优先在空格处断开）；
//! 3. 后面已经出现下一句的临时文本 —— 豆包只在一句话说完（VAD 断句）后才把它标为
//!    definite，下一句开始说时，上一句的 definite 尾部必然已经完整；
//! 4. definite 尾部静止超过 [`TAIL_SETTLE_MS`]（兜底：无标点、无后续语音）。

/// 行宽（字符数）：超过即强制切行
pub const LINE_WIDTH_CHARS: usize = 40;
/// definite 尾部静止多久视为一句结束
pub const TAIL_SETTLE_MS: u64 = 1200;

/// 一段刚定稿的文字（尚未分配 ID）
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// 当前行（未定稿）：definite 尾部 + 临时文本
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveLine {
    pub definite: String,
    pub indefinite: String,
}

impl LiveLine {
    /// 当前行完整文本（用于同声预览翻译）
    pub fn text(&self) -> String {
        match (self.definite.is_empty(), self.indefinite.is_empty()) {
            (true, true) => String::new(),
            (false, true) => self.definite.clone(),
            (true, false) => self.indefinite.clone(),
            (false, false) => format!("{}{}{}", self.definite, joiner(&self.definite, &self.indefinite), self.indefinite),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.definite.is_empty() && self.indefinite.is_empty()
    }
}

/// 拼接两段文本：拉丁字母之间补空格，中日韩直接相连
fn joiner(a: &str, b: &str) -> &'static str {
    let last = a.chars().last();
    let first = b.chars().next();
    match (last, first) {
        (Some(x), Some(y)) if x.is_ascii_alphanumeric() && y.is_ascii_alphanumeric() => " ",
        (Some(x), _) if x.is_ascii_punctuation() && first.map_or(false, |c| c.is_ascii_alphanumeric()) => " ",
        _ => "",
    }
}

/// 单个音源的切分器
#[derive(Debug, Default)]
pub struct CaptionStream {
    /// definite 流中已定稿的字符数
    consumed: usize,
    /// definite[consumed..]（未定稿尾部，原样保留空白）
    tail: String,
    indefinite: String,
    /// 尾部最后一次变化的时间
    tail_changed_ms: u64,
    /// 当前行开始出现文字的时间（用于转录 / SRT 起点）
    line_start_ms: Option<u64>,
}

impl CaptionStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一帧识别结果，返回本帧定稿的文字段
    pub fn feed(&mut self, definite: &str, indefinite: &str, now_ms: u64) -> Vec<Piece> {
        let def: Vec<char> = definite.chars().collect();
        if self.consumed > def.len() {
            // ASR 会话重启导致文本回退：从头开始
            self.consumed = 0;
        }
        let tail: String = def[self.consumed..].iter().collect();
        if tail != self.tail {
            self.tail = tail;
            self.tail_changed_ms = now_ms;
        }
        self.indefinite = indefinite.trim().to_string();
        if self.line_start_ms.is_none() && (has_content(&self.tail) || has_content(&self.indefinite)) {
            self.line_start_ms = Some(now_ms);
        }
        let tail_complete = !self.indefinite.is_empty();
        self.cut(now_ms, tail_complete)
    }

    /// 周期调用：definite 尾部静止过久则定稿
    pub fn tick(&mut self, now_ms: u64) -> Vec<Piece> {
        if !has_content(&self.tail) || !self.indefinite.is_empty() {
            return Vec::new();
        }
        if now_ms.saturating_sub(self.tail_changed_ms) >= TAIL_SETTLE_MS {
            return self.cut(now_ms, true);
        }
        Vec::new()
    }

    /// 会话结束：尾部与临时文本全部定稿，不丢最后一句
    pub fn flush(&mut self, now_ms: u64) -> Vec<Piece> {
        if !self.indefinite.is_empty() {
            let extra = format!("{}{}", joiner(self.tail.trim_end(), &self.indefinite), self.indefinite);
            self.tail.push_str(&extra);
            self.indefinite.clear();
        }
        self.cut(now_ms, true)
    }

    /// 当前行（未定稿部分）
    pub fn live(&self) -> LiveLine {
        LiveLine {
            definite: self.tail.trim().to_string(),
            indefinite: self.indefinite.clone(),
        }
    }

    fn cut(&mut self, now_ms: u64, tail_complete: bool) -> Vec<Piece> {
        let tail_len = self.tail.chars().count();
        if tail_len == 0 {
            return Vec::new();
        }
        let (mut texts, mut rest) = split_complete_sentences(&self.tail);
        if rest.chars().count() >= LINE_WIDTH_CHARS {
            let (lines, r) = split_by_width(&rest, LINE_WIDTH_CHARS);
            texts.extend(lines);
            rest = r;
        }
        if tail_complete {
            if is_meaningful(rest.trim()) {
                texts.push(rest.trim().to_string());
            }
            rest.clear();
        }
        let consumed_now = tail_len - rest.chars().count();
        if consumed_now == 0 {
            return Vec::new();
        }
        self.consumed += consumed_now;
        self.tail = rest;
        self.tail_changed_ms = now_ms;

        let start = self.line_start_ms.unwrap_or(now_ms).min(now_ms);
        // 剩余尾部或下一句已在说：新一行从此刻开始；否则等下一次出现文字
        self.line_start_ms = if has_content(&self.tail) || has_content(&self.indefinite) {
            Some(now_ms)
        } else {
            None
        };

        // 多段同时定稿时按字数比例分配时间
        let total: usize = texts.iter().map(|t| t.chars().count()).sum::<usize>().max(1);
        let span = now_ms - start;
        let mut acc = 0usize;
        texts
            .into_iter()
            .map(|text| {
                let n = text.chars().count();
                let s = start + span * acc as u64 / total as u64;
                acc += n;
                let e = start + span * acc as u64 / total as u64;
                Piece { text, start_ms: s, end_ms: e.max(s) }
            })
            .collect()
    }
}

// ==================== 纯函数 ====================

/// 切分完整句段：返回 (完整句段列表, 未完结尾部（原样）)
pub fn split_complete_sentences(text: &str) -> (Vec<String>, String) {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut last_end = 0usize;
    for (idx, &c) in chars.iter().enumerate() {
        if matches!(c, '。' | '！' | '？' | '!' | '?' | ';' | '；' | '\n') {
            // 连续的收尾引号/括号归入本句
            let mut end = idx + 1;
            while end < chars.len() && matches!(chars[end], '”' | '’' | '」' | '）' | ')' | '"') {
                end += 1;
            }
            let sentence: String = chars[last_end..end].iter().collect();
            let trimmed = sentence.trim();
            if is_meaningful(trimmed) {
                sentences.push(trimmed.to_string());
            }
            last_end = end;
        }
    }
    (sentences, chars[last_end..].iter().collect())
}

/// 长行按行宽强制切行；英文在行尾附近的空格处断开，避免切断单词
pub fn split_by_width(text: &str, limit: usize) -> (Vec<String>, String) {
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut idx = 0usize;
    while idx < chars.len() && chars[idx] == ' ' {
        idx += 1;
    }
    while idx + limit <= chars.len() {
        let mut end = idx + limit;
        if chars[end - 1].is_alphanumeric() && end < chars.len() && chars[end].is_alphanumeric() {
            let floor = idx + limit * 3 / 4;
            if let Some(space) = (floor..end).rev().find(|&i| chars[i] == ' ') {
                end = space;
            }
        }
        let line: String = chars[idx..end].iter().collect();
        if is_meaningful(line.trim()) {
            lines.push(line.trim().to_string());
        }
        idx = end;
        while idx < chars.len() && chars[idx] == ' ' {
            idx += 1;
        }
    }
    (lines, chars[idx..].iter().collect())
}

/// 是否含有实质内容（字母/数字/中日韩字符）
pub fn is_meaningful(text: &str) -> bool {
    text.chars().any(|c| c.is_alphanumeric() || (c as u32) > 0x2E80 && !is_cjk_punct(c))
}

fn has_content(text: &str) -> bool {
    is_meaningful(text.trim())
}

fn is_cjk_punct(c: char) -> bool {
    matches!(c, '。' | '，' | '、' | '；' | '：' | '？' | '！' | '…' | '—' | '·' | '～' | '“' | '”' | '‘' | '’' | '「' | '」' | '（' | '）' | '　')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_split_keeps_tail_and_closing_quotes() {
        let (s, tail) = split_complete_sentences("他说：“好。”然后走了！还有");
        assert_eq!(s, vec!["他说：“好。”".to_string(), "然后走了！".to_string()]);
        assert_eq!(tail, "还有");
    }

    #[test]
    fn pure_punctuation_is_dropped() {
        let (s, tail) = split_complete_sentences("。。！");
        assert!(s.is_empty());
        assert!(tail.is_empty());
    }

    #[test]
    fn width_split_cjk_and_english() {
        let (lines, tail) = split_by_width(&"一二三四五六七八九十".repeat(10), 40);
        assert_eq!(lines.len(), 2);
        assert_eq!(tail.chars().count(), 20);

        let text = "hello world this is a fairly long english sentence that keeps going";
        let (lines, tail) = split_by_width(text, 40);
        assert!(lines.iter().all(|l| l.chars().count() <= 40));
        assert!(!lines[0].ends_with("sente"), "不应切断单词: {:?}", lines);
        let joined = format!("{} {}", lines.join(" "), tail);
        assert_eq!(joined.replace(' ', ""), text.replace(' ', ""));
    }

    #[test]
    fn punctuation_finalizes_immediately_with_timing() {
        let mut s = CaptionStream::new();
        assert!(s.feed("", "大家", 1000).is_empty());
        let out = s.feed("大家好。", "", 2000);
        assert_eq!(out, vec![Piece { text: "大家好。".into(), start_ms: 1000, end_ms: 2000 }]);
        assert!(s.live().is_empty());
    }

    #[test]
    fn next_utterance_finalizes_unpunctuated_tail() {
        let mut s = CaptionStream::new();
        s.feed("", "今天天气", 0);
        assert!(s.feed("今天天气不错", "", 800).is_empty(), "无标点、无后续时先保留为当前行");
        assert_eq!(s.live().definite, "今天天气不错");
        let out = s.feed("今天天气不错", "我们出去", 1000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "今天天气不错");
        assert_eq!(s.live(), LiveLine { definite: String::new(), indefinite: "我们出去".into() });
    }

    #[test]
    fn settled_tail_finalizes_on_tick() {
        let mut s = CaptionStream::new();
        s.feed("没有标点的一句话", "", 100);
        assert!(s.tick(100 + TAIL_SETTLE_MS - 1).is_empty());
        let out = s.tick(100 + TAIL_SETTLE_MS);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "没有标点的一句话");
    }

    #[test]
    fn multiple_sentences_share_time_by_length() {
        let mut s = CaptionStream::new();
        s.feed("", "一", 0);
        let out = s.feed("一二。三四五六。", "", 1200);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].start_ms, out[0].end_ms), (0, 450)); // 3/8 的字数（含标点）
        assert_eq!((out[1].start_ms, out[1].end_ms), (450, 1200));
    }

    #[test]
    fn utterance_join_space_and_rollback() {
        let mut s = CaptionStream::new();
        let a = s.feed("第一句。", "", 10);
        assert_eq!(a[0].text, "第一句。");
        let b = s.feed("第一句。 第二句。", "", 20);
        assert_eq!(b[0].text, "第二句。");
        // 会话重启：definite 变短，从头重新计数
        let c = s.feed("新的。", "", 30);
        assert_eq!(c[0].text, "新的。");
    }

    #[test]
    fn flush_keeps_last_words() {
        let mut s = CaptionStream::new();
        s.feed("前半句", "后半句", 0);
        // 有后续临时文本 → 前半句已定稿
        let mut s2 = CaptionStream::new();
        s2.feed("", "only interim words", 0);
        let out = s2.flush(500);
        assert_eq!(out[0].text, "only interim words");
        assert!(s.flush(10).iter().any(|p| p.text == "后半句"));
    }

    #[test]
    fn live_text_joins_latin_with_space() {
        let l = LiveLine { definite: "hello".into(), indefinite: "world".into() };
        assert_eq!(l.text(), "hello world");
        let c = LiveLine { definite: "你好".into(), indefinite: "世界".into() };
        assert_eq!(c.text(), "你好世界");
    }
}
