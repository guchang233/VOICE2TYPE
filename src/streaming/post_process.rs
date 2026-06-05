//! 流式模式后处理：按配置分三种模式

use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::{ConfigManager, STREAMING_POST_AI, STREAMING_POST_LOCAL, STREAMING_POST_NONE};

static EMOJI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[\p{Emoji_Presentation}\p{Extended_Pictographic}]")
        .expect("emoji regex")
});
static WHITESPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t]+").expect("whitespace regex"));
static PUNCT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\p{P}]").expect("punctuation regex"));
/// 仅合并 3 个及以上连续相同标点
static TRIPLE_PUNCT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([，,])\1{2,}|([。．.])\2{2,}").expect("triple punct")
});

/// 极高置信、整词替换
static LIGHT_PHRASE_FIXES: &[(&str, &str)] = &[
    ("因该", "应该"),
    ("在见", "再见"),
];

pub fn process_streaming_text(text: &str, config: &ConfigManager) -> String {
    match config.streaming_post_process_mode().as_str() {
        STREAMING_POST_NONE => process_none(text, config),
        STREAMING_POST_LOCAL => process_local(text, config),
        STREAMING_POST_AI => process_live_raw(text, config),
        _ => process_local(text, config),
    }
}

pub fn process_none(text: &str, config: &ConfigManager) -> String {
    let mut result = text.trim().to_string();
    if result.is_empty() {
        return result;
    }
    apply_emoji_punct_flags(&mut result, config);
    if !looks_like_sequence_or_list(&result) {
        result = collapse_spaces_outside_numbers(&result);
    }
    result.trim().to_string()
}

pub fn process_live_raw(text: &str, config: &ConfigManager) -> String {
    process_none(text, config)
}

/// 本地极轻修正：不碰数字序列，不概括
pub fn process_local(text: &str, config: &ConfigManager) -> String {
    let mut result = text.trim().to_string();
    if result.is_empty() {
        return result;
    }

    let is_list = looks_like_sequence_or_list(&result);

    if !is_list {
        result = dedupe_exact_half_repeat_safe(&result);
        result = apply_light_typos(&result);
    }

    apply_emoji_punct_flags(&mut result, config);

    if config.streaming_allow_punctuation() && !is_list {
        result = light_normalize_punctuation(&result);
    }

    if !is_list {
        result = collapse_spaces_outside_numbers(&result);
    }

    result.trim().to_string()
}

fn apply_emoji_punct_flags(result: &mut String, config: &ConfigManager) {
    if !config.streaming_allow_emoji() {
        *result = EMOJI_RE.replace_all(result, "").to_string();
    }
    if !config.streaming_allow_punctuation() {
        *result = strip_punctuation(result);
    }
}

/// 含大量数字/中文数词时视为序列，跳过去重与标点整理
fn looks_like_sequence_or_list(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return false;
    }
    let num_like = chars
        .iter()
        .filter(|c| is_number_like(**c))
        .count();
    num_like * 100 / chars.len() >= 25
}

fn is_number_like(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            '零' | '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十' | '百'
                | '千' | '万' | '两' | '〇' | '第'
        )
}

/// 仅当整段前后两半完全相同，且不像数数/列表
fn dedupe_exact_half_repeat_safe(text: &str) -> String {
    if looks_like_sequence_or_list(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n < 8 || n % 2 != 0 {
        return text.to_string();
    }
    let half = n / 2;
    if chars[..half] == chars[half..] {
        return chars[..half].iter().collect();
    }
    text.to_string()
}

fn apply_light_typos(text: &str) -> String {
    if looks_like_sequence_or_list(text) {
        return text.to_string();
    }
    let mut s = text.to_string();
    for (from, to) in LIGHT_PHRASE_FIXES {
        if s.contains(from) {
            s = s.replace(from, to);
        }
    }
    s
}

fn light_normalize_punctuation(text: &str) -> String {
    TRIPLE_PUNCT_RE
        .replace_all(text, |caps: &regex::Captures| {
            if caps.get(1).is_some() {
                "，".to_string()
            } else {
                "。".to_string()
            }
        })
        .to_string()
}

/// 只合并空格/制表符，不把中文逐字拆开
fn collapse_spaces_outside_numbers(text: &str) -> String {
    WHITESPACE_RE.replace_all(text, " ").to_string()
}

fn is_punctuation(c: char) -> bool {
    let mut buf = [0u8; 4];
    PUNCT_RE.is_match(c.encode_utf8(&mut buf))
}

fn strip_punctuation(text: &str) -> String {
    let is_numeric_separator = |c: char| matches!(c, '.' | ':' | ',' | '-');
    let chars: Vec<char> = text.chars().collect();
    let mut cleaned = String::with_capacity(text.len());

    for (i, &c) in chars.iter().enumerate() {
        if !is_punctuation(c) {
            cleaned.push(c);
            continue;
        }
        let keep = is_numeric_separator(c)
            && i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_ascii_digit()
            && chars[i + 1].is_ascii_digit();
        cleaned.push(if keep { c } else { ' ' });
    }
    cleaned
}
