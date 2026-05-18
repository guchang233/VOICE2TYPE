use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use reqwest::{multipart::Form, Client};

use crate::config::ConfigManager;

pub static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("failed to build HTTP client")
});

/// 多数兼容 OpenAI 的转写接口都会返回 text 字段。
#[derive(serde::Deserialize, Debug)]
pub struct ApiResponse {
    pub text: String,
}

#[derive(Clone, Default)]
pub struct ApiClient;

impl ApiClient {
    pub fn new() -> Self {
        Self
    }

    pub async fn process_audio(
        &self,
        audio_data: Vec<u8>,
        config: &ConfigManager,
    ) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key is not configured");
        }

        let resp = HTTP_CLIENT
            .post(self.api_url(config))
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(self.build_form(audio_data, config, "recording.wav")?)
            .send()
            .await
            .context("Failed to send API request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error_text = resp.text().await.context("Failed to read error response")?;
            anyhow::bail!("API error {}: {}", status, error_text);
        }

        let result = resp
            .json::<ApiResponse>()
            .await
            .context("Failed to parse API response")?;

        Ok(result.text.trim().to_string())
    }

    /// 保留给后续真正的分段上传。现在整段上传，避免把 WAV 字节硬切成非法音频。
    pub async fn process_audio_streaming(
        &self,
        audio_data: Vec<u8>,
        config: &ConfigManager,
    ) -> Result<Vec<String>> {
        let text = self.process_audio(audio_data, config).await?;
        Ok(vec![text])
    }

    fn api_url(&self, config: &ConfigManager) -> String {
        if config.get_speech_service() == "groq" {
            "https://api.groq.com/openai/v1/audio/transcriptions".to_string()
        } else {
            config.get_api_url()
        }
    }

    fn build_form(
        &self,
        audio_data: Vec<u8>,
        config: &ConfigManager,
        file_name: &str,
    ) -> Result<Form> {
        let mut form = Form::new().text("model", config.get_model_name()).part(
            "file",
            reqwest::multipart::Part::bytes(audio_data)
                .file_name(file_name.to_string())
                .mime_str("audio/wav")?,
        );

        if config.get_speech_service() == "groq" {
            form = form
                .text("temperature", "0")
                .text("response_format", "verbose_json");
        } else if config.output_language() != "auto" {
            form = form.text("language", config.output_language());
        }

        Ok(form)
    }
}
