use anyhow::{Context, Result};

use crate::api::client::ApiClient;
use crate::audio::processor::encode_wav_memory;
use crate::config::ConfigManager;
use crate::whisper_local::LocalWhisper;

pub struct PipelineConfig {
    pub use_translation: bool,
    pub source_language: String,
    pub target_language: String,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            use_translation: true,
            source_language: "auto".to_string(),
            target_language: "zh".to_string(),
        }
    }
}

pub struct Pipeline {
    config: PipelineConfig,
    api_client: ApiClient,
}

impl Pipeline {
    pub fn new(config: PipelineConfig) -> Self {
        Self {
            config,
            api_client: ApiClient::new(),
        }
    }

    pub async fn process_audio(&self, audio_data: Vec<i16>, config_manager: &ConfigManager) -> Result<String> {
        let wav_data = encode_wav_memory(&audio_data, 16000)?;

        let raw_text = if config_manager.is_local_whisper() {
            LocalWhisper::transcribe_sync(&wav_data, config_manager)?
        } else {
            self.api_client.process_audio(wav_data, config_manager).await?
        };

        if self.config.use_translation && !raw_text.is_empty() {
            self.translate_text(&raw_text, config_manager).await
        } else {
            Ok(raw_text)
        }
    }

    async fn translate_text(&self, text: &str, config_manager: &ConfigManager) -> Result<String> {
        let groq_key = config_manager.get_groq_api_key();
        if groq_key.is_empty() {
            log::warn!("Groq API key not configured, skipping translation");
            return Ok(text.to_string());
        }

        let prompt = format!(
            "Translate the following text from {} to {}:\n\n{}",
            self.config.source_language,
            self.config.target_language,
            text
        );

        let resp = reqwest::Client::new()
            .post("https://api.groq.com/openai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", groq_key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "model": "llama3-8b-8192",
                "messages": [
                    {
                        "role": "user",
                        "content": prompt
                    }
                ],
                "temperature": 0.3,
                "max_tokens": 512
            }))
            .send()
            .await
            .context("Translation API request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error_text = resp.text().await.context("Failed to read error response")?;
            log::warn!("Translation API error {}: {}", status, error_text);
            return Ok(text.to_string());
        }

        let result: serde_json::Value = resp.json().await.context("Failed to parse translation response")?;

        let translated_text = result["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or(text);

        Ok(translated_text.trim().to_string())
    }
}
