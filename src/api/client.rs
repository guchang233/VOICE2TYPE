use anyhow::{Context, Result};
use reqwest::multipart::Form;
use reqwest::Client;
use std::sync::Arc;
use once_cell::sync::Lazy;

use crate::config::ConfigManager;

#[cfg(target_os = "windows")]
pub static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| Client::new());

/// API响应结构体
#[derive(serde::Deserialize, Debug)]
pub struct ApiResponse {
    pub text: String,
}

/// API客户端
#[derive(Clone)]
pub struct ApiClient {
    client: Arc<Client>,
}

impl ApiClient {
    /// 创建新的API客户端
    pub fn new() -> Self {
        Self {
            client: Arc::new(Client::new()),
        }
    }

    /// 处理音频并获取转写结果
    pub async fn process_audio(&self, audio_data: Vec<u8>, config: &ConfigManager) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key is not configured");
        }

        let form = Form::new()
            .text("model", config.get_model_name())
            .part("file", reqwest::multipart::Part::bytes(audio_data)
                .file_name("recording.wav")
                .mime_str("audio/wav")?);

        let resp = HTTP_CLIENT.post(&config.get_api_url())
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await
            .context("Failed to send API request")?;
        
        if !resp.status().is_success() {
            let error_text = resp.text().await
                .context("Failed to read error response")?;
            anyhow::bail!("API error: {}", error_text);
        }

        let result = resp.json::<ApiResponse>()
            .await
            .context("Failed to parse API response")?;

        Ok(result.text.trim().to_string())
    }

    /// 流式处理音频并获取转写结果
    pub async fn process_audio_streaming(&self, audio_data: Vec<u8>, config: &ConfigManager) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            return Ok(String::new());
        }

        let form = Form::new()
            .text("model", config.get_model_name())
            .part("file", reqwest::multipart::Part::bytes(audio_data)
                .file_name("recording.wav")
                .mime_str("audio/wav")?);

        let resp = HTTP_CLIENT.post(&config.get_api_url())
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await
            .context("Failed to send streaming API request")?;
        
        if !resp.status().is_success() {
            return Ok(String::new());
        }

        let result = resp.json::<ApiResponse>()
            .await
            .context("Failed to parse streaming API response")?;

        Ok(result.text.trim().to_string())
    }
}