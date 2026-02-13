use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;
use crate::speech::service::SpeechService;

/// Azure Speech Services请求
#[derive(Debug, Serialize)]
struct AzureSpeechRequest {
    config: AzureSpeechConfig,
    audio: AzureSpeechAudio,
}

/// Azure Speech Services配置
#[derive(Debug, Serialize)]
struct AzureSpeechConfig {
    speech_config: AzureSpeechConfigDetails,
    audio_config: AzureAudioConfig,
}

/// Azure Speech Services配置详情
#[derive(Debug, Serialize)]
struct AzureSpeechConfigDetails {
    language: String,
}

/// Azure Speech Services音频配置
#[derive(Debug, Serialize)]
struct AzureAudioConfig {
    audio_source: AzureAudioSource,
}

/// Azure Speech Services音频源
#[derive(Debug, Serialize)]
struct AzureAudioSource {
    input_format: String,
    stream: AzureAudioStream,
}

/// Azure Speech Services音频流
#[derive(Debug, Serialize)]
struct AzureAudioStream {
    stream_url: String,
}

/// Azure Speech Services响应
#[derive(Debug, Deserialize)]
struct AzureSpeechResponse {
    recognition_status: String,
    display_text: String,
}

/// Azure Speech Services
pub struct AzureService;

impl AzureService {
    /// 创建新的Azure Speech Services
    pub fn new() -> Self {
        Self
    }
}

impl SpeechService for AzureService {
    /// 识别音频数据
    async fn recognize(&self, audio_data: &[u8], config: &Arc<ConfigManager>) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key 未配置");
        }

        // Azure Speech Services的API端点
        // 注意：这里需要用户在配置中设置正确的区域，例如eastus, westus等
        let region = "eastus"; // 默认区域
        let api_url = format!("https://{}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1", region);

        // 构建请求
        let request = AzureSpeechRequest {
            config: AzureSpeechConfig {
                speech_config: AzureSpeechConfigDetails {
                    language: config.output_language(),
                },
                audio_config: AzureAudioConfig {
                    audio_source: AzureAudioSource {
                        input_format: "audio/wav; codecs=audio/pcm; samplerate=16000".to_string(),
                        stream: AzureAudioStream {
                            stream_url: "".to_string(), // Azure使用请求体中的音频数据
                        },
                    },
                },
            },
            audio: AzureSpeechAudio {
                audio_data: base64::encode(audio_data),
            },
        };

        let resp = HTTP_CLIENT.post(&api_url)
            .header("Ocp-Apim-Subscription-Key", api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            anyhow::bail!("API 错误: {}", error_text);
        }

        let result = resp.json::<AzureSpeechResponse>().await?;

        Ok(result.display_text)
    }

    /// 获取服务名称
    fn name(&self) -> &str {
        "Azure"
    }

    /// 检查服务是否可用
    async fn is_available(&self, config: &Arc<ConfigManager>) -> bool {
        let api_key = config.get_api_key();
        !api_key.is_empty()
    }
}

/// Azure Speech Services音频
#[derive(Debug, Serialize)]
struct AzureSpeechAudio {
    audio_data: String,
}
