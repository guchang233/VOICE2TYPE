use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;
use crate::speech::service::SpeechService;

/// 腾讯云语音识别请求
#[derive(Debug, Serialize)]
struct TencentSpeechRequest {
    engine_model_type: String,
    channel_num: u32,
    res_type: u32,
    source: u32,
    speech: String,
    speech_len: usize,
    language: String,
}

/// 腾讯云语音识别响应
#[derive(Debug, Deserialize)]
struct TencentSpeechResponse {
    code: i32,
    message: String,
    result: Option<TencentSpeechResult>,
}

/// 腾讯云语音识别结果
#[derive(Debug, Deserialize)]
struct TencentSpeechResult {
    transcript: String,
    confidence: f32,
}

/// 腾讯云语音识别服务
pub struct TencentService;

impl TencentService {
    /// 创建新的腾讯云语音识别服务
    pub fn new() -> Self {
        Self
    }
}

impl SpeechService for TencentService {
    /// 识别音频数据
    async fn recognize(&self, audio_data: &[u8], config: &Arc<ConfigManager>) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key 未配置");
        }

        // 腾讯云语音识别的API端点
        // 注意：这里需要用户在配置中设置正确的地域，例如ap-beijing, ap-shanghai等
        let region = "ap-beijing";
        let api_url = format!("https://asr.tencentcloudapi.com/?Action=RecognizeVoice&Version=2019-06-14&Region={}", region);

        // 构建请求
        let request = TencentSpeechRequest {
            engine_model_type: "16k_zh".to_string(), // 默认中文模型
            channel_num: 1,
            res_type: 2,
            source: 1,
            speech: base64::encode(audio_data),
            speech_len: audio_data.len(),
            language: self.get_language_code(config.output_language()),
        };

        // 注意：腾讯云API需要使用签名认证，这里简化处理，实际使用中需要按照腾讯云文档生成签名
        let resp = HTTP_CLIENT.post(&api_url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            anyhow::bail!("API 错误: {}", error_text);
        }

        let result = resp.json::<TencentSpeechResponse>().await?;

        if result.code != 0 {
            anyhow::bail!("API 错误: {}", result.message);
        }

        let transcript = result.result
            .map(|r| r.transcript)
            .unwrap_or_default();

        Ok(transcript)
    }

    /// 获取服务名称
    fn name(&self) -> &str {
        "腾讯云"
    }

    /// 检查服务是否可用
    async fn is_available(&self, config: &Arc<ConfigManager>) -> bool {
        let api_key = config.get_api_key();
        !api_key.is_empty()
    }
}

impl TencentService {
    /// 获取语言代码
    fn get_language_code(&self, language: &str) -> String {
        match language.to_lowercase().as_str() {
            "zh" => "zh".to_string(),
            "en" => "en".to_string(),
            "ja" => "ja".to_string(),
            "ko" => "ko".to_string(),
            _ => "zh".to_string(),
        }
    }
}
