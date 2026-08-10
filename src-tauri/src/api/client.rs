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

        let url = self.api_url(config);
        let model = config.get_model_name();
        let service = config.get_speech_service();

        log::info!(
            "API request: service={}, model={}, url={}, audio_size={} bytes, key={}...",
            service,
            model,
            url,
            audio_data.len(),
            &api_key[..api_key.len().min(8)]
        );

        let form = self.build_form(audio_data, config, "recording.wav")?;
        let resp = match HTTP_CLIENT
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await
            .context("Failed to send API request")
        {
            Ok(r) => r,
            Err(e) => {
                return Err(e);
            }
        };

        let status = resp.status();
        log::info!("API response status: {}", status);

        if !status.is_success() {
            let error_text = resp
                .text()
                .await
                .context("Failed to read error response")?;
            log::error!("API error {}: {}", status, error_text);
            anyhow::bail!("API error {}: {}", status, error_text);
        }

        let result = match resp
            .json::<ApiResponse>()
            .await
            .context("Failed to parse API response")
        {
            Ok(r) => r,
            Err(e) => {
                return Err(e);
            }
        };

        Ok(result.text.trim().to_string())
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
        let service = config.get_speech_service();
        let mut form = Form::new()
            .text("model", config.get_model_name())
            .text("response_format", "json")
            .part(
                "file",
                reqwest::multipart::Part::bytes(audio_data)
                    .file_name(file_name.to_string())
                    .mime_str("audio/wav")?,
            );

        if service == "groq" {
            form = form.text("temperature", "0");
        }

        if config.output_language() != "auto" {
            form = form.text("language", config.output_language());
        }

        Ok(form)
    }
}
