//! 将流式转写片段合并为完整句子，避免在词/短语中间切断。

use std::time::{Duration, Instant};

const FLUSH_IDLE: Duration = Duration::from_secs(1);

/// 句子结束标点（中英文）
fn is_sentence_terminal(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '…' | '．')
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3040..=0x309F | 0x30A0..=0x30FF | 0xAC00..=0xD7AF)
}

fn needs_space_between(prev: char, next: char) -> bool {
    if is_cjk(prev) || is_cjk(next) {
        return false;
    }
    !prev.is_whitespace() && !next.is_whitespace()
}

/// 合并两段转写，去除重叠并处理中英文空格。
pub fn merge_transcript_fragments(existing: &str, incoming: &str) -> String {
    let a = existing.trim();
    let b = incoming.trim();
    if b.is_empty() {
        return a.to_string();
    }
    if a.is_empty() {
        return b.to_string();
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let max_overlap = a_chars.len().min(b_chars.len()).min(24);

    let mut overlap = 0usize;
    for len in (1..=max_overlap).rev() {
        let a_tail: String = a_chars[a_chars.len().saturating_sub(len)..].iter().collect();
        let b_head: String = b_chars[..len].iter().collect();
        if a_tail.eq_ignore_ascii_case(&b_head) {
            overlap = len;
            break;
        }
    }

    let append: String = if overlap > 0 {
        b_chars[overlap..].iter().collect()
    } else {
        b.to_string()
    };

    if append.is_empty() {
        return a.to_string();
    }

    let last_a = a.chars().last().unwrap();
    let first_b = append.chars().next().unwrap();
    if needs_space_between(last_a, first_b) {
        format!("{} {}", a, append)
    } else {
        format!("{}{}", a, append)
    }
}

/// 从缓冲区切出所有已完成的句子，返回 (完整句子列表, 剩余未结束片段)。
pub fn extract_complete_sentences(buffer: &str) -> (Vec<String>, String) {
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        return (Vec::new(), String::new());
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0usize;

    for (i, &c) in chars.iter().enumerate() {
        if !is_sentence_terminal(c) {
            continue;
        }
        // 英文句号可能是小数点：前后都是数字则不算句末
        if c == '.' && i > 0 && i + 1 < chars.len() {
            let prev = chars[i - 1];
            let next = chars[i + 1];
            if prev.is_ascii_digit() && next.is_ascii_digit() {
                continue;
            }
        }

        let slice: String = chars[start..=i].iter().collect();
        let sentence = slice.trim().to_string();
        if !sentence.is_empty() {
            sentences.push(sentence);
        }
        start = i + 1;
        while start < chars.len() && chars[start].is_whitespace() {
            start += 1;
        }
    }

    let remainder: String = chars.get(start..).unwrap_or(&[]).iter().collect();
    (sentences, remainder.trim().to_string())
}

pub struct SentenceBuffer {
    pending: String,
    last_append: Instant,
}

impl SentenceBuffer {
    pub fn new() -> Self {
        Self {
            pending: String::new(),
            last_append: Instant::now(),
        }
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn pending_text(&self) -> &str {
        &self.pending
    }

    /// 追加新的转写片段，返回可以展示的完整句子。
    pub fn push_fragment(&mut self, fragment: &str) -> Vec<String> {
        let fragment = fragment.trim();
        if fragment.is_empty() {
            return Vec::new();
        }

        self.pending = merge_transcript_fragments(&self.pending, fragment);
        self.last_append = Instant::now();

        let (sentences, rest) = extract_complete_sentences(&self.pending);
        self.pending = rest;
        sentences
    }

    /// 长时间无新片段时，将剩余内容作为一句输出（若有实质内容）。
    pub fn flush_if_idle(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        if self.last_append.elapsed() < FLUSH_IDLE {
            return None;
        }
        let rest = std::mem::take(&mut self.pending);
        let trimmed = rest.trim().to_string();
        if trimmed.len() < 2 {
            return None;
        }
        Some(trimmed)
    }

    pub fn force_flush(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let rest = std::mem::take(&mut self.pending);
        let trimmed = rest.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_without_duplicate_overlap() {
        let merged = merge_transcript_fragments("今天天气", "天气很好");
        assert_eq!(merged, "今天天气很好");
    }

    #[test]
    fn splits_on_cjk_period() {
        let (s, r) = extract_complete_sentences("你好。世界");
        assert_eq!(s, vec!["你好。"]);
        assert_eq!(r, "世界");
    }

    #[test]
    fn splits_multiple_sentences() {
        let (s, r) = extract_complete_sentences("Hi. How are you? Fine");
        assert_eq!(s.len(), 2);
        assert_eq!(r, "Fine");
    }
}
