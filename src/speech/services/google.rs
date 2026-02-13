use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;
use crate::speech::service::SpeechService;

/// Google Cloud Speech-to-Text请求
#[derive(Debug, Serialize)]
struct GoogleSpeechRequest {
    config: GoogleSpeechConfig,
    audio: GoogleSpeechAudio,
}

/// Google Cloud Speech-to-Text配置
#[derive(Debug, Serialize)]
struct GoogleSpeechConfig {
    encoding: String,
    sample_rate_hertz: u32,
    language_code: String,
}

/// Google Cloud Speech-to-Text音频
#[derive(Debug, Serialize)]
struct GoogleSpeechAudio {
    content: String,
}

/// Google Cloud Speech-to-Text响应
#[derive(Debug, Deserialize)]
struct GoogleSpeechResponse {
    results: Vec<GoogleSpeechResult>,
}

/// Google Cloud Speech-to-Text结果
#[derive(Debug, Deserialize)]
struct GoogleSpeechResult {
    alternatives: Vec<GoogleSpeechAlternative>,
}

/// Google Cloud Speech-to-Text替代方案
#[derive(Debug, Deserialize)]
struct GoogleSpeechAlternative {
    transcript: String,
}

/// Google Cloud Speech-to-Text服务
pub struct GoogleCloudService;

impl GoogleCloudService {
    /// 创建新的Google Cloud Speech-to-Text服务
    pub fn new() -> Self {
        Self
    }
}

impl SpeechService for GoogleCloudService {
    /// 识别音频数据
    async fn recognize(&self, audio_data: &[u8], config: &Arc<ConfigManager>) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key 未配置");
        }

        // Google Cloud Speech-to-Text的API端点
        let api_url = format!("https://speech.googleapis.com/v1/speech:recognize?key={}", api_key);

        // 将音频数据编码为base64
        let audio_content = base64::encode(audio_data);

        // 构建请求
        let request = GoogleSpeechRequest {
            config: GoogleSpeechConfig {
                encoding: "LINEAR16".to_string(),
                sample_rate_hertz: 16000,
                language_code: config.output_language(),
            },
            audio: GoogleSpeechAudio {
                content: audio_content,
            },
        };

        let resp = HTTP_CLIENT.post(&api_url)
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            anyhow::bail!("API 错误: {}", error_text);
        }

        let result = resp.json::<GoogleSpeechResponse>().await?;

        // 提取转录结果
        let transcript = result.results
            .into_iter()
            .flat_map(|r| r.alternatives)
            .map(|a| a.transcript)
            .collect::<Vec<_>>()
            .join("");

        Ok(transcript)
    }

    /// 获取服务名称
    fn name(&self) -> &str {
        "Google Cloud"
    }

    /// 检查服务是否可用
    async fn is_available(&self, config: &Arc<ConfigManager>) -> bool {
        let api_key = config.get_api_key();
        !api_key.is_empty()
    }
}
