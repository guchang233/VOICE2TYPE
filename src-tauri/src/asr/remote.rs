//! 云端 ASR 适配器（批量模式）。
//!
//! 将现有的 [`crate::api::client::ApiClient`]（兼容 OpenAI 接口的批量转写 HTTP 客户端）
//! 适配为 [`crate::asr::engine::AsrEngine`] trait。
//!
//! 工作方式：
//! - `start`：清空内部缓冲
//! - `push_audio`：累积 f32 样本
//! - `stop`：resample → 16kHz i16 → WAV 编码 → HTTP multipart 上传 → 返回文本
//!
//! 适用于：硅基流动、Groq、自定义兼容 OpenAI 的转写接口。

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::api::client::ApiClient;
use crate::asr::engine::AsrEngine;
use crate::audio::processor::{encode_wav_memory, resample_and_convert};
use crate::config::ConfigManager;

/// 默认输入采样率标记值，0 表示尚未设置。
const SAMPLE_RATE_UNSET: u32 = 0;

/// 云端批量 ASR 引擎适配器。
pub struct RemoteAsrEngine {
    config: Arc<ConfigManager>,
    client: ApiClient,
    /// 累积的 f32 音频样本。
    buffer: Vec<f32>,
    /// 输入音频采样率。
    input_sample_rate: u32,
}

impl RemoteAsrEngine {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        Self {
            config,
            client: ApiClient::new(),
            buffer: Vec::new(),
            input_sample_rate: SAMPLE_RATE_UNSET,
        }
    }

    /// 设置输入音频采样率。应在 `start` 之后、`push_audio` 之前调用。
    pub fn set_input_sample_rate(&mut self, rate: u32) {
        self.input_sample_rate = rate;
    }
}

#[async_trait]
impl AsrEngine for RemoteAsrEngine {
    async fn start(&mut self) -> Result<()> {
        self.buffer.clear();
        self.input_sample_rate = SAMPLE_RATE_UNSET;
        let service = self.config.get_speech_service();
        log::info!("[asr/remote] 会话开始, service={}", service);
        Ok(())
    }

    async fn push_audio(&mut self, audio: &[f32]) -> Result<()> {
        self.buffer.extend_from_slice(audio);
        Ok(())
    }

    async fn stop(&mut self) -> Result<String> {
        if self.buffer.is_empty() {
            anyhow::bail!("RemoteAsrEngine: 无音频数据");
        }

        let input_rate = if self.input_sample_rate == 0 {
            16_000
        } else {
            self.input_sample_rate
        };

        let samples_owned = std::mem::take(&mut self.buffer);
        let (samples_i16, output_rate) =
            resample_and_convert(&samples_owned, input_rate);

        if samples_i16.is_empty() {
            anyhow::bail!("RemoteAsrEngine: 重采样后样本为空");
        }

        let wav_data = encode_wav_memory(&samples_i16, output_rate)?;

        let text = self.client.process_audio(wav_data, &self.config).await?;

        log::info!("[asr/remote] 转写完成, 文本长度={}", text.len());
        Ok(text)
    }

    fn name(&self) -> &str {
        "cloud-batch"
    }
}
