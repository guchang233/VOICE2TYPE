use anyhow::Result;
use reqwest::multipart;
use std::sync::Arc;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;
use crate::speech::service::SpeechService;

/// SiliconFlow语音识别服务
pub struct SiliconFlowService;

impl SiliconFlowService {
    /// 创建新的SiliconFlow语音识别服务
    pub fn new() -> Self {
        Self
    }
}

impl SpeechService for SiliconFlowService {
    /// 识别音频数据
    async fn recognize(&self, audio_data: &[u8], config: &Arc<ConfigManager>) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key 未配置");
        }

        let form = multipart::Form::new()
            .text("model", config.get_model_name())
            .text("language", config.output_language())
            .part("file", multipart::Part::bytes(audio_data.to_vec())
                .file_name("recording.wav")
                .mime_str("audio/wav")?);

        let resp = HTTP_CLIENT.post(&config.get_api_url())
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            anyhow::bail!("API 错误: {}", error_text);
        }

        #[derive(serde::Deserialize)]
        struct SiliconFlowResponse {
            text: String,
        }

        let result = resp.json::<SiliconFlowResponse>().await?;
        Ok(result.text)
    }

    /// 获取服务名称
    fn name(&self) -> &str {
        "SiliconFlow"
    }

    /// 检查服务是否可用
    async fn is_available(&self, config: &Arc<ConfigManager>) -> bool {
        let api_key = config.get_api_key();
        !api_key.is_empty()
    }
}
