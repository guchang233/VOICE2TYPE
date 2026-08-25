//! 视频配音工作流：视频 → 提取音轨 → 云端 ASR 带时间戳转写 → 逐段 TTS →
//! 按原时间轴拼接配音音轨 → 替换原声混流出新视频。
//!
//! 模块结构：
//! - [`ffmpeg`]：ffmpeg 定位（PATH → models 目录）与按需下载，及子进程封装
//! - [`transcribe`]：云端 ASR（verbose_json / srt 双格式回退），输出带时间戳分段
//! - [`tts_segments`]：逐段 TTS 合成、时长贴合重试、时间轴 PCM 流式拼装
//! - [`pipeline`]：整体状态机编排 + `dubbing-progress` 进度事件 + 取消

pub mod ffmpeg;
pub mod pipeline;
pub mod transcribe;
pub mod tts_segments;

use serde::{Deserialize, Serialize};

/// 带时间戳的字幕/配音分段（毫秒）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DubSegment {
    pub index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl DubSegment {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// 过滤 ASR 输出中的噪声片段：纯标点、省略号、[Music]/（音乐）等占位符。
/// 返回 true 表示应丢弃该分段。
pub fn is_noise_text(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    // 去掉包裹符号后判断是否只剩标点/空白
    let stripped: String = t
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '[' | ']' | '(' | ')' | '{' | '}' | '【' | '】' | '（' | '）' | '「' | '」'
            )
        })
        .collect();
    let lower = stripped.trim().to_lowercase();
    matches!(
        lower.as_str(),
        "music" | "applause" | "laughter" | "silence" | "音乐" | "掌声" | "笑声" | "..."
    ) || stripped
        .trim()
        .chars()
        .all(|c| c.is_whitespace() || c.is_ascii_punctuation() || is_cjk_punctuation(c))
}

fn is_cjk_punctuation(c: char) -> bool {
    matches!(
        c,
        '。' | '，'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '…'
            | '—'
            | '·'
            | '～'
            | '．'
            | '﹒'
            | '　'
            | '“'
            | '”'
            | '‘'
            | '’'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_detection() {
        assert!(is_noise_text(""));
        assert!(is_noise_text("   "));
        assert!(is_noise_text("..."));
        assert!(is_noise_text("。。。"));
        assert!(is_noise_text("[Music]"));
        assert!(is_noise_text("（音乐）"));
        assert!(is_noise_text("[Applause]"));
        assert!(!is_noise_text("你好世界"));
        assert!(!is_noise_text("Hello, world!"));
        assert!(!is_noise_text("嗯。")); // 有实际语义的语气词保留
    }
}
