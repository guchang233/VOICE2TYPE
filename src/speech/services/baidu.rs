use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;
use crate::speech::service::SpeechService;

/// 百度语音识别请求
#[derive(Debug, Serialize)]
struct BaiduSpeechRequest {
    format: String,
    rate: u32,
    channel: u32,
    cuid: String,
    dev_pid: u32,
    speech: String,
    len: usize,
}

/// 百度语音识别响应
#[derive(Debug, Deserialize)]
struct BaiduSpeechResponse {
    err_no: i32,
    err_msg: String,
    result: Option<Vec<String>>,
}

/// 百度语音识别服务
pub struct BaiduService;

impl BaiduService {
    /// 创建新的百度语音识别服务
    pub fn new() -> Self {
        Self
    }
}

impl SpeechService for BaiduService {
    /// 识别音频数据
    async fn recognize(&self, audio_data: &[u8], config: &Arc<ConfigManager>) -> Result<String> {
        let api_key = config.get_api_key();
        if api_key.is_empty() {
            anyhow::bail!("API Key 未配置");
        }

        // 百度语音识别的API端点
        let api_url = "https://vop.baidu.com/server_api";

        // 获取access token
        let access_token = self.get_access_token(api_key).await?;

        // 构建请求
        let request = BaiduSpeechRequest {
            format: "wav".to_string(),
            rate: 16000,
            channel: 1,
            cuid: "Voice2Type".to_string(),
            dev_pid: self.get_dev_pid(config.output_language()),
            speech: base64::encode(audio_data),
            len: audio_data.len(),
        };

        let resp = HTTP_CLIENT.post(api_url)
            .header("Content-Type", "application/json")
            .query(&["access_token", &access_token])
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            anyhow::bail!("API 错误: {}", error_text);
        }

        let result = resp.json::<BaiduSpeechResponse>().await?;

        if result.err_no != 0 {
            anyhow::bail!("API 错误: {}", result.err_msg);
        }

        let transcript = result.result
            .unwrap_or_default()
            .join("");

        Ok(transcript)
    }

    /// 获取服务名称
    fn name(&self) -> &str {
        "百度"
    }

    /// 检查服务是否可用
    async fn is_available(&self, config: &Arc<ConfigManager>) -> bool {
        let api_key = config.get_api_key();
        !api_key.is_empty()
    }
}

impl BaiduService {
    /// 获取access token
    async fn get_access_token(&self, api_key: &str) -> Result<String> {
        // 百度获取access token的API端点
        let url = "https://aip.baidubce.com/oauth/2.0/token";

        // 注意：这里需要用户提供Secret Key，实际使用中应该从配置中获取
        let secret_key = "";
        if secret_key.is_empty() {
            anyhow::bail!("Secret Key 未配置");
        }

        let resp = HTTP_CLIENT.get(url)
            .query(&[
                ("grant_type", "client_credentials"),
                ("client_id", api_key),
                ("client_secret", secret_key),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            anyhow::bail!("获取access token失败: {}", error_text);
        }

        #[derive(Debug, Deserialize)]
        struct TokenResponse {
            access_token: String,
        }

        let result = resp.json::<TokenResponse>().await?;
        Ok(result.access_token)
    }

    /// 获取开发语言ID
    fn get_dev_pid(&self, language: &str) -> u32 {
        match language.to_lowercase().as_str() {
            "zh" => 1537, // 中文普通话
            "en" => 1737, // 英语
            "ja" => 1637, // 日语
            "ko" => 1837, // 韩语
            _ => 1537, // 默认中文普通话
        }
    }
}
