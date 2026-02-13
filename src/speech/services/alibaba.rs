use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;
use crate::speech::service::SpeechService;

/// 阿里云语音识别请求
#[derive(Debug, Serialize)]
struct AlibabaSpeechRequest {
    format: String,
    sample_rate: u32,
    channel: u32,
    speech: String,
    enable_punctuation_prediction: bool,
    enable_inverse_text_normalization: bool,
    language: String,
}

/// 阿里云语音识别响应
#[derive(Debug, Deserialize)]
struct AlibabaSpeechResponse {
    request_id: String,
    status: String,
    result: Option<AlibabaSpeechResult>,
    error: Option<AlibabaSpeechError>,
}

/// 阿里云语音识别结果
#[derive(Debug, Deserialize)]
struct AlibabaSpeechResult {
    transcript: String,
    sentences: Option<Vec<AlibabaSpeechSentence>>,
}

/// 阿里云语音识别句子
#[derive(Debug, Deserialize)]
struct AlibabaSpeechSentence {
    transcript: String,
    start_time: i64,
    end_time: i64,
}

/// 阿里云语音识别错误
#[derive(Debug, Deserialize)]
struct AlibabaSpeechError {
    code: String,
    message: String,
}

/// 阿里云语音识别服务
pub struct AlibabaService;

impl AlibabaService {
    /// 创建新的阿里云语音识别服务
    pub fn new() -> Self {
        Self
    }
}

impl SpeechService for AlibabaService {
    /// 识别音频数据
    async fn recognize(&self, audio_data: &[u8], config: &Arc<ConfigManager>) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key 未配置");
        }

        // 阿里云语音识别的API端点
        let api_url = "https://nlsapi.aliyun.com/stream/v1/asr";

        // 构建请求
        let request = AlibabaSpeechRequest {
            format: "wav".to_string(),
            sample_rate: 16000,
            channel: 1,
            speech: base64::encode(audio_data),
            enable_punctuation_prediction: true,
            enable_inverse_text_normalization: true,
            language: self.get_language_code(config.output_language()),
        };

        let resp = HTTP_CLIENT.post(api_url)
            .header("Content-Type", "application/json")
            .header("X-NLS-Token", api_key)
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            anyhow::bail!("API 错误: {}", error_text);
        }

        let result = resp.json::<AlibabaSpeechResponse>().await?;

        if result.status != "OK" {
            if let Some(error) = result.error {
                anyhow::bail!("API 错误: {} - {}", error.code, error.message);
            } else {
                anyhow::bail!("API 错误: 未知错误");
            }
        }

        let transcript = result.result
            .map(|r| r.transcript)
            .unwrap_or_default();

        Ok(transcript)
    }

    /// 获取服务名称
    fn name(&self) -> &str {
        "阿里云"
    }

    /// 检查服务是否可用
    async fn is_available(&self, config: &Arc<ConfigManager>) -> bool {
        let api_key = config.get_api_key();
        !api_key.is_empty()
    }
}

impl AlibabaService {
    /// 获取语言代码
    fn get_language_code(&self, language: &str) -> String {
        match language.to_lowercase().as_str() {
            "zh" => "zh-CN".to_string(),
            "en" => "en-US".to_string(),
            "ja" => "ja-JP".to_string(),
            "ko" => "ko-KR".to_string(),
            _ => "zh-CN".to_string(),
        }
    }
}
