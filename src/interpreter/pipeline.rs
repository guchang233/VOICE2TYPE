use crate::config::ConfigManager;
use crate::utils::logger::{write_log, LogLevel};

pub async fn process_chunk(wav_data: Vec<u8>, config: &ConfigManager) -> Option<String> {
    let raw_text = transcribe(&wav_data, config).await?;
    write_log(LogLevel::INFO, &format!("[字幕] 转写结果: {}", raw_text.trim()), None);
    if raw_text.trim().is_empty() {
        return None;
    }

    if !config.interpreter_use_translation() {
        return Some(raw_text);
    }

    let source = config.interpreter_source_language();
    let target = config.interpreter_target_language();

    if source == target {
        return Some(raw_text);
    }

    match translate(&raw_text, &source, &target, config).await {
        Ok(translated) => {
            if translated.trim().is_empty() {
                Some(raw_text)
            } else {
                write_log(LogLevel::INFO, &format!("[字幕] 翻译结果: {}", translated.trim()), None);
                Some(translated)
            }
        }
        Err(e) => {
            write_log(LogLevel::WARN, &format!("[字幕] 翻译失败，回退显示原文: {}", e), None);
            Some(raw_text)
        }
    }
}

async fn transcribe(wav_data: &[u8], config: &ConfigManager) -> Option<String> {
    if config.is_local_whisper() {
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
        let api_client = crate::api::client::ApiClient::new();
        match api_client.process_audio(wav_data.to_vec(), config).await {
            Ok(text) => Some(text),
            Err(e) => {
                write_log(LogLevel::ERROR, &format!("[字幕] API 转写失败: {}", e), None);
                None
            }
        }
    }
}

async fn translate(
    text: &str,
    source_lang: &str,
    target_lang: &str,
    config: &ConfigManager,
) -> Result<String, String> {
    let api_key = config.get_groq_api_key();
    if api_key.is_empty() {
        return Err("Groq API Key 未配置".to_string());
    }

    let source_desc = if source_lang == "auto" {
        "自动检测的语言".to_string()
    } else {
        source_lang.to_string()
    };

    let prompt = format!(
        "将以下文本从{}翻译为{}，只输出翻译结果，不要添加任何解释：\n\n{}",
        source_desc, target_lang, text
    );

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": "llama-3.1-8b-instant",
            "messages": [{"role": "user", "content": prompt}],
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
