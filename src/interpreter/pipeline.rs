use crate::config::ConfigManager;
use crate::utils::logger::{write_log, LogLevel};
use std::sync::Mutex;

const GROQ_STT_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const GROQ_CHAT_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const SILICONFLOW_STT_URL: &str = "https://api.siliconflow.cn/v1/audio/transcriptions";
const SILICONFLOW_CHAT_URL: &str = "https://api.siliconflow.cn/v1/chat/completions";

static TRANSLATION_CONTEXT: once_cell::sync::Lazy<
    Mutex<Vec<(String, String, std::time::Instant)>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));

pub fn reset_translation_context() {
    TRANSLATION_CONTEXT.lock().unwrap().clear();
}

pub fn reset_last_transcription() {
    reset_translation_context();
}

/// 对单句做翻译并返回展示结果（句子缓冲在 mod 层处理）。
pub async fn finalize_sentence(
    text: String,
    config: &ConfigManager,
) -> Option<SubtitleResult> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    let filtered = filter_hallucination(&text)?;

    if !config.interpreter_use_translation() {
        return Some(SubtitleResult {
            original: filtered,
            translated: None,
        });
    }

    let source = config.interpreter_source_language();
    let target = config.interpreter_target_language();
    if source != "auto" && source == target {
        return Some(SubtitleResult {
            original: filtered,
            translated: None,
        });
    }

    let ctx: Vec<(String, String)> = {
        let now = std::time::Instant::now();
        TRANSLATION_CONTEXT
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, t)| now.duration_since(*t).as_secs() < 120)
            .map(|(o, t, _)| (o.clone(), t.clone()))
            .collect()
    };

    match translate(&filtered, &source, &target, config, &ctx).await {
        Ok(translated) => {
            write_log(
                LogLevel::INFO,
                &format!("[字幕] 翻译: {}", translated.trim()),
                None,
            );
            {
                let mut lock = TRANSLATION_CONTEXT.lock().unwrap();
                let now = std::time::Instant::now();
                lock.retain(|(_, _, t)| now.duration_since(*t).as_secs() < 120);
                lock.push((filtered.clone(), translated.clone(), now));
                if lock.len() > 5 {
                    lock.remove(0);
                }
            }
            Some(SubtitleResult {
                original: filtered,
                translated: Some(translated),
            })
        }
        Err(e) => {
            write_log(LogLevel::WARN, &format!("[字幕] 翻译失败: {}", e), None);
            Some(SubtitleResult {
                original: filtered,
                translated: None,
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubtitleResult {
    pub original: String,
    pub translated: Option<String>,
}

/// 转写音频块，返回原始文本（由上层句子缓冲决定何时展示）。
pub async fn transcribe_chunk(wav_data: Vec<u8>, config: &ConfigManager) -> Option<String> {
    let raw = transcribe(&wav_data, config).await?;
    let raw_trimmed = raw.trim();
    if raw_trimmed.is_empty() {
        return None;
    }
    write_log(
        LogLevel::INFO,
        &format!("[字幕] 转写: {}", raw_trimmed),
        None,
    );
    filter_hallucination(raw_trimmed)
}

fn filter_hallucination(text: &str) -> Option<String> {
    let cleaned = text.trim().trim_matches(|c| c == '.' || c == ',' || c == ' ' || c == '\u{3000}');
    if cleaned.is_empty() { return None; }
    let meaningful = cleaned.chars().filter(|c| c.is_alphanumeric() || is_cjk(*c)).count();
    if meaningful < 2 { return None; }
    let lower = cleaned.to_lowercase();
    const HALLUCINATIONS: &[&str] = &[
        "you", "thank you", "thanks", "thank you.", "thanks.",
        "thank you so much", "thanks for watching", "please subscribe",
        "bye", "bye bye", "okay", "ok", "hmm", "hm", "uh", "um", "ah", "oh",
        "...", "..", ".", "!", "?", "-", "--",
        "subtitles by", "subtitled by", "transcript", "caption",
        "♪", "♫", "[music]", "[applause]", "[laughter]",
        "谢谢", "谢谢你", "谢谢大家", "再见", "好的", "嗯", "哦", "啊",
        "the", "a", "an", "is", "it", "in", "on", "at", "to", "of",
        "and", "or", "but", "for", "with", "this", "that",
        "i", "me", "my", "we", "he", "she", "they",
        "so", "no", "yes", "not", "all", "just",
        "do", "be", "am", "are", "was", "were", "been",
        "have", "has", "had", "will", "would", "can", "could",
        "what", "who", "how", "when", "where", "why",
        "1", "2", "3", "4", "5", "6", "7", "8", "9", "0",
    ];
    if HALLUCINATIONS.iter().any(|&h| lower == h) {
        write_log(LogLevel::DEBUG, &format!("[字幕] 过滤幻觉: {:?}", cleaned), None);
        return None;
    }
    const HALLUCINATION_PATTERNS: &[&str] = &[
        "subscribe", "like and subscribe", "follow", "click",
        "comment", "share", "ring the bell", "notification",
        "channel", "video", "watch", "stream",
        "字幕", "关注", "订阅", "点赞", "投币", "收藏", "转发",
    ];
    if HALLUCINATION_PATTERNS.iter().any(|&p| lower.contains(p) && cleaned.len() < 30) {
        write_log(LogLevel::DEBUG, &format!("[字幕] 过滤幻觉(模式): {:?}", cleaned), None);
        return None;
    }
    Some(cleaned.to_string())
}

#[inline]
fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3040..=0x309F | 0x30A0..=0x30FF | 0xAC00..=0xD7AF)
}

async fn transcribe(wav_data: &[u8], config: &ConfigManager) -> Option<String> {
    if config.interpreter_use_local_whisper() {
        let wav = wav_data.to_vec();
        let cfg = config.clone();
        return match tokio::task::spawn_blocking(move || {
            crate::whisper_local::LocalWhisper::transcribe_sync(&wav, &cfg)
        }).await {
            Ok(Ok(t)) => Some(t),
            Ok(Err(e)) => { write_log(LogLevel::ERROR, &format!("[字幕] 本地 Whisper 失败: {}", e), None); None }
            Err(e) => { write_log(LogLevel::ERROR, &format!("[字幕] 本地 Whisper 异常: {}", e), None); None }
        };
    }

    let model = config.interpreter_model_name();
    let is_siliconflow_model = model.contains('/');
    let (api_url, api_key) = if is_siliconflow_model {
        (
            SILICONFLOW_STT_URL.to_string(),
            { let k = config.interpreter_api_key(); if k.is_empty() { config.get_siliconflow_api_key() } else { k } },
        )
    } else {
        (
            { let u = config.interpreter_api_url(); if u.is_empty() { GROQ_STT_URL.to_string() } else { u } },
            { let k = config.interpreter_api_key(); if k.is_empty() { config.get_groq_api_key() } else { k } },
        )
    };

    if api_key.is_empty() {
        write_log(LogLevel::ERROR, &format!("[字幕] 转写 Key 未配置 (model={})", model), None);
        return None;
    }

    let source = config.interpreter_source_language();

    let mut form = reqwest::multipart::Form::new()
        .text("model", model)
        .text("response_format", "json")
        .part("file", reqwest::multipart::Part::bytes(wav_data.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav").unwrap());
    if source != "auto" {
        form = form.text("language", source);
    }

    match crate::api::client::HTTP_CLIENT
        .post(&api_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send().await
    {
        Ok(r) if r.status().is_success() => {
            match r.json::<crate::api::client::ApiResponse>().await {
                Ok(res) => Some(res.text),
                Err(e) => { write_log(LogLevel::ERROR, &format!("[字幕] 解析转写响应失败: {}", e), None); None }
            }
        }
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            write_log(LogLevel::ERROR, &format!("[字幕] 转写 API 错误 {}: {}", status, body), None);
            None
        }
        Err(e) => { write_log(LogLevel::ERROR, &format!("[字幕] 转写请求失败: {}", e), None); None }
    }
}

fn lang_display(code: &str) -> &str {
    match code {
        "zh" => "简体中文", "en" => "English", "ja" => "日本語",
        "ko" => "한국어",   "fr" => "Français",  "de" => "Deutsch",
        "es" => "Español",  "ru" => "Русский",   "ar" => "العربية",
        _ => "the source language",
    }
}

async fn translate(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    config: &ConfigManager,
    context: &[(String, String)],
) -> Result<String, String> {
    let model = config.interpreter_translation_model();
    let is_siliconflow_model = model.contains('/');
    let (api_url, api_key) = if is_siliconflow_model {
        (
            SILICONFLOW_CHAT_URL.to_string(),
            { let k = config.interpreter_translation_api_key(); if k.is_empty() { config.get_siliconflow_api_key() } else { k } },
        )
    } else {
        (
            { let u = config.interpreter_translation_api_url(); if u.is_empty() { GROQ_CHAT_URL.to_string() } else { u } },
            { let k = config.interpreter_translation_api_key(); if k.is_empty() { config.get_groq_api_key() } else { k } },
        )
    };

    if api_key.is_empty() {
        return Err(format!("翻译 Key 未配置 (model={})", model));
    }
    let system_prompt = format!(
        "You are a professional real-time interpreter from {} to {}.\n\
         Output ONLY the translated text — no explanations, no notes, no parentheses.\n\
         Keep proper nouns, names, and brands untranslated.\n\
         Match register: casual stays casual, formal stays formal.\n\
         If input is already in {}, output it unchanged.",
        lang_display(source_lang), lang_display(target_lang), lang_display(target_lang)
    );

    let mut messages = vec![serde_json::json!({"role": "system", "content": system_prompt})];
    for (orig, trans) in context.iter().rev().take(3).rev() {
        messages.push(serde_json::json!({"role": "user", "content": orig}));
        messages.push(serde_json::json!({"role": "assistant", "content": trans}));
    }
    messages.push(serde_json::json!({"role": "user", "content": text}));

    let resp = crate::api::client::HTTP_CLIENT
        .post(&api_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": 0.2,
            "max_tokens": 512,
        }))
        .send().await
        .map_err(|e| format!("翻译请求失败: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("翻译 API 错误 {}: {}", status, body));
    }

    let body: serde_json::Value = resp.json().await
        .map_err(|e| format!("解析翻译响应失败: {}", e))?;

    body["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "翻译响应格式异常".to_string())
}