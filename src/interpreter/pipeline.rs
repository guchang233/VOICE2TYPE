use crate::config::ConfigManager;
use crate::utils::logger::{write_log, LogLevel};

use std::sync::Mutex;

static LAST_TRANSCRIPTION: once_cell::sync::Lazy<Mutex<String>> =
    once_cell::sync::Lazy::new(|| Mutex::new(String::new()));

static TRANSLATION_CONTEXT: once_cell::sync::Lazy<Mutex<Vec<(String, String, std::time::Instant)>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));

pub fn reset_translation_context() {
    TRANSLATION_CONTEXT.lock().unwrap().clear();
}

pub fn reset_last_transcription() {
    *LAST_TRANSCRIPTION.lock().unwrap() = String::new();
    reset_translation_context();
}

#[derive(Debug, Clone)]
pub struct SubtitleResult {
    pub original: String,
    pub translated: Option<String>,
}

pub async fn process_chunk(wav_data: Vec<u8>, config: &ConfigManager) -> Option<SubtitleResult> {
    let raw_text = transcribe(&wav_data, config).await?;
    write_log(LogLevel::INFO, &format!("[字幕] 转写结果: {}", raw_text.trim()), None);
    if raw_text.trim().is_empty() {
        return None;
    }

    let deduped = deduplicate(&raw_text);

    if !config.interpreter_use_translation() {
        *LAST_TRANSCRIPTION.lock().unwrap() = raw_text.trim().to_string();
        return Some(SubtitleResult {
            original: deduped,
            translated: None,
        });
    }

    let source = config.interpreter_source_language();
    let target = config.interpreter_target_language();

    if source == target {
        *LAST_TRANSCRIPTION.lock().unwrap() = raw_text.trim().to_string();
        return Some(SubtitleResult {
            original: deduped,
            translated: None,
        });
    }

    let ctx = if config.interpreter_translation_context() {
        TRANSLATION_CONTEXT.lock().unwrap().iter().map(|(o, t, _)| (o.clone(), t.clone())).collect()
    } else {
        Vec::new()
    };
    match translate(&deduped, &source, &target, config, &ctx).await {
        Ok(translated) => {
            *LAST_TRANSCRIPTION.lock().unwrap() = raw_text.trim().to_string();
            if translated.trim().is_empty() {
                Some(SubtitleResult {
                    original: deduped,
                    translated: None,
                })
            } else {
                write_log(LogLevel::INFO, &format!("[字幕] 翻译结果: {}", translated.trim()), None);
                {
                    let mut ctx = TRANSLATION_CONTEXT.lock().unwrap();
                    let now = std::time::Instant::now();
                    ctx.retain(|(_, _, t)| now.duration_since(*t).as_secs() < 60);
                    ctx.push((deduped.clone(), translated.clone(), now));
                    if ctx.len() > 3 {
                        ctx.remove(0);
                    }
                }
                Some(SubtitleResult {
                    original: deduped,
                    translated: Some(translated),
                })
            }
        }
        Err(e) => {
            *LAST_TRANSCRIPTION.lock().unwrap() = raw_text.trim().to_string();
            write_log(LogLevel::WARN, &format!("[字幕] 翻译失败，回退显示原文: {}", e), None);
            Some(SubtitleResult {
                original: deduped,
                translated: None,
            })
        }
    }
}

fn deduplicate(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }

    let last = LAST_TRANSCRIPTION.lock().unwrap();
    if last.is_empty() {
        return trimmed.to_string();
    }

    let overlap = find_overlap(&last, trimmed);
    if overlap > 0 {
        let chars: Vec<char> = trimmed.chars().collect();
        let new_part: String = chars[overlap..].iter().collect();
        if new_part.trim().is_empty() {
            return String::new();
        }
        return new_part.trim().to_string();
    }

    trimmed.to_string()
}

fn find_overlap(previous: &str, current: &str) -> usize {
    let prev_chars: Vec<char> = previous.chars().collect();
    let curr_chars: Vec<char> = current.chars().collect();

    let max_check = prev_chars.len().min(curr_chars.len());
    if max_check == 0 {
        return 0;
    }

    let min_overlap = ((curr_chars.len() as f64 * 0.3) as usize).max(4);
    let mut best_overlap = 0;

    for overlap_len in min_overlap..=max_check {
        let prev_slice = &prev_chars[prev_chars.len() - overlap_len..];
        let curr_slice = &curr_chars[..overlap_len];

        let matching = prev_slice
            .iter()
            .zip(curr_slice.iter())
            .filter(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
            .count();

        let similarity = matching as f64 / overlap_len as f64;
        if similarity > 0.7 {
            best_overlap = overlap_len;
        }
    }

    best_overlap
}

async fn transcribe(wav_data: &[u8], config: &ConfigManager) -> Option<String> {
    if config.interpreter_use_local_whisper() {
        let wav = wav_data.to_vec();
        let cfg = config.clone();
        match tokio::task::spawn_blocking(move || {
            crate::whisper_local::LocalWhisper::transcribe_sync(&wav, &cfg)
        })
        .await
        {
            Ok(Ok(text)) => Some(text),
            Ok(Err(e)) => {
                write_log(LogLevel::ERROR, &format!("[字幕] 本地 Whisper 转写失败: {}", e), None);
                None
            }
            Err(e) => {
                write_log(LogLevel::ERROR, &format!("[字幕] 本地 Whisper 任务异常: {}", e), None);
                None
            }
        }
    } else {
        let api_key = config.interpreter_api_key();
        let api_key = if api_key.is_empty() { config.get_groq_api_key() } else { api_key };
        if api_key.is_empty() {
            write_log(LogLevel::ERROR, "[字幕] 转写 API Key 未配置", None);
            return None;
        }
        let api_url = config.interpreter_api_url();
        let model = config.interpreter_model_name();

        let mut form = reqwest::multipart::Form::new()
            .text("model", model)
            .text("response_format", "json")
            .part("file", reqwest::multipart::Part::bytes(wav_data.to_vec())
                .file_name("recording.wav".to_string())
                .mime_str("audio/wav").unwrap());

        let source = config.interpreter_source_language();
        if source != "auto" {
            form = form.text("language", source);
        }

        let response = crate::api::client::HTTP_CLIENT
            .post(&api_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await;

        match response {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<crate::api::client::ApiResponse>().await {
                        Ok(result) => Some(result.text),
                        Err(e) => {
                            write_log(LogLevel::ERROR, &format!("[字幕] 解析转写响应失败: {}", e), None);
                            None
                        }
                    }
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    write_log(LogLevel::ERROR, &format!("[字幕] 转写 API 错误 {}: {}", status, body), None);
                    None
                }
            }
            Err(e) => {
                write_log(LogLevel::ERROR, &format!("[字幕] 转写请求失败: {}", e), None);
                None
            }
        }
    }
}

fn lang_display(code: &str) -> &str {
    match code {
        "zh" => "简体中文",
        "en" => "English",
        "ja" => "日本語",
        "ko" => "한국어",
        "fr" => "Français",
        "de" => "Deutsch",
        "es" => "Español",
        "auto" => "the source language (auto-detect)",
        _ => code,
    }
}

async fn translate(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    config: &ConfigManager,
    context: &[(String, String)],
) -> Result<String, String> {
    let api_key = config.interpreter_translation_api_key();
    let api_key = if api_key.is_empty() { config.get_groq_api_key() } else { api_key };
    if api_key.is_empty() {
        return Err("翻译 API Key 未配置".to_string());
    }

    let api_url = config.interpreter_translation_api_url();

    let source_desc = lang_display(source_lang);
    let target_desc = lang_display(target_lang);

    let model = config.interpreter_translation_model();

    let system_prompt = format!(
        "你是一位精通{}和{}的专业翻译官。你的翻译遵循「信达雅」原则：\n\
         - 信：忠实原文含义，不增删篡改\n\
         - 达：译文通顺流畅，符合目标语言表达习惯\n\
         - 雅：用词优美得体，避免生硬机翻\n\n\
         规则：\n\
         1. 只输出翻译结果，不要添加解释、注释或括号\n\
         2. 专有名词、人名、品牌名保持原文不翻译\n\
         3. 口语化内容保持口语风格，书面内容保持书面风格\n\
         4. 如果原文已经是目标语言，直接输出原文",
        source_desc, target_desc
    );

    let user_prompt = format!("{}", text);

    let mut messages = vec![
        serde_json::json!({"role": "system", "content": system_prompt}),
    ];
    for (orig, trans) in context {
        messages.push(serde_json::json!({"role": "user", "content": orig}));
        messages.push(serde_json::json!({"role": "assistant", "content": trans}));
    }
    messages.push(serde_json::json!({"role": "user", "content": user_prompt}));

    let response = crate::api::client::HTTP_CLIENT
        .post(&api_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": 0.3,
            "max_tokens": 1024,
        }))
        .send()
        .await
        .map_err(|e| format!("翻译请求失败: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("翻译 API 返回错误 {}: {}", status, body));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("解析翻译响应失败: {}", e))?;

    body["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "翻译响应格式异常".to_string())
}
