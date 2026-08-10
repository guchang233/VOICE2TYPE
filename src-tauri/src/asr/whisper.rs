//! 本地 Whisper ASR 适配器。
//!
//! 将现有的 [`crate::whisper_local::LocalWhisperEngine`]（通过 whisper-cli 子进程）
//! 适配为 [`crate::asr::engine::AsrEngine`] trait。
//!
//! 工作方式：
//! - `start`：清空内部缓冲，刷新模型路径
//! - `push_audio`：累积 f32 样本
//! - `stop`：resample → 16kHz i16 → spawn_blocking 调用 whisper-cli 转写 → 返回文本
//!
//! 该适配器与现有 `app_state.rs` 中的本地 whisper 调用路径并存，
//! 当前调用方未切换到 trait 路径，后续阶段将逐步迁移。

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::asr::engine::AsrEngine;
use crate::audio::processor::resample_and_convert;
use crate::config::ConfigManager;
use crate::whisper_local::LocalWhisperEngine;

/// 默认输入采样率标记值，0 表示尚未设置。
const SAMPLE_RATE_UNSET: u32 = 0;

/// 本地 Whisper ASR 引擎适配器。
pub struct WhisperAsrEngine {
    config: Arc<ConfigManager>,
    engine: LocalWhisperEngine,
    /// 累积的 f32 音频样本。
    buffer: Vec<f32>,
    /// 输入音频采样率（由调用方通过 `set_input_sample_rate` 设置）。
    input_sample_rate: u32,
}

impl WhisperAsrEngine {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        let model_dir = config.whisper_models_dir();
        Self {
            config,
            engine: LocalWhisperEngine::new(model_dir),
            buffer: Vec::new(),
            input_sample_rate: SAMPLE_RATE_UNSET,
        }
    }

    /// 设置输入音频采样率。应在 `start` 之后、`push_audio` 之前调用。
    /// 若未调用，默认按 16kHz 处理（跳过重采样）。
    pub fn set_input_sample_rate(&mut self, rate: u32) {
        self.input_sample_rate = rate;
    }
}

#[async_trait]
impl AsrEngine for WhisperAsrEngine {
    async fn start(&mut self) -> Result<()> {
        self.buffer.clear();
        self.input_sample_rate = SAMPLE_RATE_UNSET;
        // 刷新模型路径（用户可能在启动后修改了模型目录或模型名）
        let model_name = self.config.local_whisper_model();
        let model_dir = self.config.whisper_models_dir();
        self.engine.refresh_paths(model_dir, &model_name);
        log::info!("[asr/whisper] 会话开始, 模型={}", self.config.local_whisper_model());
        Ok(())
    }

    async fn push_audio(&mut self, audio: &[f32]) -> Result<()> {
        self.buffer.extend_from_slice(audio);
        Ok(())
    }

    async fn stop(&mut self) -> Result<String> {
        if self.buffer.is_empty() {
            anyhow::bail!("WhisperAsrEngine: 无音频数据");
        }

        let input_rate = if self.input_sample_rate == 0 {
            16_000
        } else {
            self.input_sample_rate
        };

        // 重采样到 16kHz i16（与现有 app_state 路径一致）
        let samples_owned = std::mem::take(&mut self.buffer);
        let (samples_i16, _output_rate) =
            resample_and_convert(&samples_owned, input_rate);

        if samples_i16.is_empty() {
            anyhow::bail!("WhisperAsrEngine: 重采样后样本为空");
        }

        // 准备转写参数
        let model_path = self.engine.model_path().to_path_buf();
        let binary_path = self.engine.binary_path_clone();

        let lang = self.config.output_language();
        let effective_lang = if lang.is_empty() || lang == "auto" {
            // 优先用缓存的检测语言，避免每次都做语言检测
            let cached = self.config.local_whisper_detected_language();
            if cached.is_empty() { "auto".to_string() } else { cached }
        } else {
            lang
        };

        let threads = self.config.local_whisper_threads();
        let greedy = self.config.local_whisper_greedy();
        let no_fallback = self.config.local_whisper_no_fallback();

        // spawn_blocking 调用同步 whisper-cli
        // clone effective_lang 供闭包后使用（判断是否需要缓存检测结果）
        let lang_for_blocking = effective_lang.clone();
        let result = tokio::task::spawn_blocking(move || {
            LocalWhisperEngine::transcribe_sync(
                &binary_path,
                &model_path,
                &samples_i16,
                Some(&lang_for_blocking),
                threads,
                greedy,
                no_fallback,
            )
        })
        .await??;

        // 若本次用了 auto 检测，缓存检测结果
        if effective_lang == "auto" {
            if let Some(ref detected) = result.1 {
                self.config.set_local_whisper_detected_language(detected.clone());
                log::info!("[asr/whisper] 检测到语言: {}", detected);
            }
        }

        log::info!("[asr/whisper] 转写完成, 文本长度={}", result.0.len());
        Ok(result.0)
    }

    fn name(&self) -> &str {
        "whisper-local"
    }
}
