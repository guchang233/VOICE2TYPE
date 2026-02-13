use anyhow::Result;
use reqwest::multipart;
use std::sync::Arc;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;
use crate::speech::service::SpeechService;

/// OpenAI语音识别服务
pub struct OpenAIService;

impl OpenAIService {
    /// 创建新的OpenAI语音识别服务
    pub fn new() -> Self {
        Self
    }
}

impl SpeechService for OpenAIService {
    /// 识别音频数据
    async fn recognize(&self, audio_data: &[u8], config: &Arc<ConfigManager>) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key 未配置");
        }

        // OpenAI的API端点
        let api_url = "https://api.openai.com/v1/audio/transcriptions";

        let form = multipart::Form::new()
            .text("model", "whisper-1") // OpenAI使用whisper模型
            .text("language", config.output_language())
            .text("response_format", "text")
            .part("file", multipart::Part::bytes(audio_data.to_vec())
                .file_name("recording.wav")
                .mime_str("audio/wav")?);

        let resp = HTTP_CLIENT.post(api_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            anyhow::bail!("API 错误: {}", error_text);
        }

        let result = resp.text().await?;
        Ok(result)
    }

    /// 获取服务名称
    fn name(&self) -> &str {
        "OpenAI"
    }

    /// 检查服务是否可用
    async fn is_available(&self, config: &Arc<ConfigManager>) -> bool {
        let api_key = config.get_api_key();
        !api_key.is_empty()
    }
}
