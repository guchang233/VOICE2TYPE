use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::api::client::ApiClient;
use crate::audio::processor;
use crate::config::ConfigManager;
use crate::output::handler::{self, OutputHandler};
use crate::recorder::Recorder;
use crate::streaming::audio as stream_audio;
use crate::streaming::session::StreamingSession;
use crate::subtitle::SubtitleService;
use crate::whisper_local::LocalWhisperEngine;

/// 持有流式 ASR 会话与音频采集流。`cpal::Stream` 不是 `Send`/`Sync`，
/// 这里沿用 `Recorder` 的做法用 `unsafe impl` 包装，仅在主线程之外持有引用。
struct StreamingRuntime {
    session: Option<StreamingSession>,
    stream: Option<SendStream>,
}

/// 把 `cpal::Stream` 包裹成 `Send`/`Sync`，使其可以安全地跨越 await 点。
struct SendStream(cpal::Stream);
unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}

unsafe impl Send for StreamingRuntime {}
unsafe impl Sync for StreamingRuntime {}

impl StreamingRuntime {
    fn new() -> Self {
        Self {
            session: None,
            stream: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppStatus {
    Idle,
    Recording,
    Processing,
    Error(String),
}

impl ToString for AppStatus {
    fn to_string(&self) -> String {
        match self {
            AppStatus::Idle => "idle".to_string(),
            AppStatus::Recording => "recording".to_string(),
            AppStatus::Processing => "processing".to_string(),
            AppStatus::Error(e) => format!("error:{}", e),
        }
    }
}

pub struct AppState {
    pub config: Arc<ConfigManager>,
    pub recorder: Arc<Mutex<Recorder>>,
    pub api_client: ApiClient,
    pub output_handler: OutputHandler,
    pub status: Arc<Mutex<AppStatus>>,
    pub app_handle: Arc<Mutex<Option<AppHandle>>>,
    pub whisper_engine: Arc<std::sync::Mutex<LocalWhisperEngine>>,
    pub subtitle: Arc<SubtitleService>,
    streaming_runtime: Arc<Mutex<StreamingRuntime>>,
}

impl AppState {
    pub fn new(config: Arc<ConfigManager>) -> Self {
        let whisper_model_dir = config.whisper_models_dir();
        let whisper_engine = Arc::new(std::sync::Mutex::new(LocalWhisperEngine::new(whisper_model_dir)));
        let subtitle = Arc::new(SubtitleService::new());

        Self {
            config,
            recorder: Arc::new(Mutex::new(Recorder::new())),
            api_client: ApiClient::new(),
            output_handler: OutputHandler::new(),
            status: Arc::new(Mutex::new(AppStatus::Idle)),
            app_handle: Arc::new(Mutex::new(None)),
            whisper_engine,
            subtitle,
            streaming_runtime: Arc::new(Mutex::new(StreamingRuntime::new())),
        }
    }

    pub async fn set_app_handle(&self, handle: AppHandle) {
        self.subtitle.set_app_handle(handle.clone()).await;
        *self.app_handle.lock().await = Some(handle);
    }

    pub async fn start_subtitle(&self) -> Result<(), String> {
        self.subtitle.start(self.config.clone()).await
    }

    pub async fn stop_subtitle(&self) -> Result<(), String> {
        self.subtitle.stop().await
    }

    pub fn is_subtitle_running(&self) -> bool {
        self.subtitle.is_running()
    }

    pub async fn toggle_subtitle(&self) -> Result<bool, String> {
        if self.subtitle.is_running() {
            self.subtitle.stop().await?;
            Ok(false)
        } else {
            self.subtitle.start(self.config.clone()).await?;
            Ok(true)
        }
    }

    async fn emit_status(&self, status: AppStatus) {
        *self.status.lock().await = status.clone();
        if let Some(handle) = self.app_handle.lock().await.as_ref() {
            let _ = handle.emit("app-status", status.to_string());
        }
    }

    pub async fn start_recording(&self) -> Result<(), String> {
        if self.config.is_stream_mode() {
            return self.start_streaming_recording().await;
        }
        let mut recorder = self.recorder.lock().await;
        if recorder.is_recording() {
            return Err("Already recording".to_string());
        }

        let device_name = self.config.input_device();
        let device = if device_name.is_empty() { None } else { Some(device_name.as_str()) };

        recorder.start(device).map_err(|e| e.to_string())?;
        self.emit_status(AppStatus::Recording).await;
        Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Recording).await;

        if let Some(handle) = self.app_handle.lock().await.as_ref() {
            let _ = handle.emit("recording-started", ());
        }

        log::info!("Recording started");
        Ok(())
    }

    /// 启动流式识别会话：先创建会话拿 pcm_buffer，再启动音频采集得到真实采样率，
    /// 最后调用 session.start 建立 WebSocket 并开始 pump。
    pub async fn start_streaming_recording(&self) -> Result<(), String> {
        // 1. 快速检查是否已在运行（不持有锁跨 await）
        {
            let runtime = self.streaming_runtime.lock().await;
            if runtime.session.as_ref().map(|s| s.is_running()).unwrap_or(false) {
                return Err("Already streaming".to_string());
            }
        }

        // 2. 创建会话（占位采样率，稍后由采集返回值覆盖）
        let mut session = StreamingSession::new(0);
        let pcm_buffer = session.pcm_buffer();

        // 3. 启动音频采集（同步）——立即用 SendStream 包裹，避免裸 cpal::Stream 跨 await
        let (sample_rate, _channels, stream) =
            stream_audio::start_capture(pcm_buffer).map_err(|e| e.to_string())?;
        let stream = SendStream(stream);
        session.set_sample_rate(sample_rate);

        // 4. 启动 WebSocket 会话（不持有 runtime 锁）
        if let Err(e) = session.start(self.config.clone()).await {
            let msg = e.to_string();
            self.emit_status(AppStatus::Error(msg.clone())).await;
            Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Error).await;
            return Err(msg);
        }

        // 5. 存入 runtime
        {
            let mut runtime = self.streaming_runtime.lock().await;
            runtime.session = Some(session);
            runtime.stream = Some(stream);
        }

        self.emit_status(AppStatus::Recording).await;
        Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Recording).await;

        if let Some(handle) = self.app_handle.lock().await.as_ref() {
            let _ = handle.emit("recording-started", ());
        }

        log::info!("Streaming recording started");
        Ok(())
    }

    /// 停止流式识别会话，发送最后一帧并等待结果。
    pub async fn stop_streaming_recording(&self) -> Result<String, String> {
        // 1. 取出 session（不持有锁跨 await）
        let mut session_opt = {
            let mut runtime = self.streaming_runtime.lock().await;
            // 先检查是否真的在运行
            let running = runtime
                .session
                .as_ref()
                .map(|s| s.is_running())
                .unwrap_or(false);
            if !running {
                return Err("Not streaming".to_string());
            }
            // take 出 session 与 stream，避免持有锁跨 await
            let _stream = runtime.stream.take();
            runtime.session.take()
        };

        self.emit_status(AppStatus::Processing).await;
        Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Processing).await;

        if let Some(session) = session_opt.as_mut() {
            session.stop(&self.config).await;
        }
        // session_opt drop here -> session dropped, stream already dropped above

        if let Some(handle) = self.app_handle.lock().await.as_ref() {
            let _ = handle.emit("recording-result", String::new());
        }

        self.emit_status(AppStatus::Idle).await;
        Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Success).await;

        log::info!("Streaming recording stopped");
        Ok(String::new())
    }

    pub async fn stop_recording_and_recognize(&self) -> Result<String, String> {
        if self.config.is_stream_mode() {
            return self.stop_streaming_recording().await;
        }
        let samples = {
            let mut recorder = self.recorder.lock().await;
            if !recorder.is_recording() {
                return Err("Not recording".to_string());
            }
            recorder.stop().map_err(|e| e.to_string())?
        };

        self.emit_status(AppStatus::Processing).await;
        Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Processing).await;

        let input_rate = {
            let recorder = self.recorder.lock().await;
            recorder.sample_rate()
        };

        let result = tokio::task::spawn_blocking(move || -> Result<(Vec<i16>, u32), String> {
            Ok(processor::resample_and_convert(&samples, input_rate))
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?;

        let (samples_i16, output_rate) = match result {
            Ok(v) => v,
            Err(e) => {
                self.emit_status(AppStatus::Error(e.clone())).await;
                Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Error).await;
                return Err(e);
            }
        };

        if samples_i16.is_empty() {
            self.emit_status(AppStatus::Idle).await;
            Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Cancelled).await;
            return Err("No audio data captured".to_string());
        }

        let service = self.config.get_speech_service();
        let lang = self.config.output_language();
        let lang_opt: Option<String> = if lang == "auto" { None } else { Some(lang.clone()) };

        let recognition_result: Result<String, String> = if service == "local" {
            // 本地 Whisper：通过 whisper.cpp 预编译二进制执行转写。
            // 先在锁内同步模型路径并检查可用性，再在不持有锁的情况下异步调用转写。
            async {
                let (model_path, binary_path) = {
                    let mut engine = self
                        .whisper_engine
                        .lock()
                        .map_err(|e| format!("Lock error: {}", e))?;
                    let model_name = self.config.local_whisper_model();
                    let model_dir = self.config.whisper_models_dir();
                    // 用实时配置刷新引擎路径，避免启动后用户修改模型目录导致路径过期
                    engine.refresh_paths(model_dir.clone(), &model_name);

                    // 配置的模型不存在时，回退到目录中任意可用的 ggml-*.bin
                    // （用户可能下载了 small 但配置仍是默认 base，不应直接报错）
                    if !engine.is_model_available() {
                        if let Some(fallback) = find_available_model(&model_dir) {
                            log::warn!(
                                "配置的本地模型 {} 不存在，回退到 {}",
                                model_name,
                                fallback
                            );
                            engine.refresh_paths(model_dir.clone(), &fallback);
                            // 持久化回退结果，避免下次重复回退
                            self.config.set_local_whisper_model(fallback);
                        }
                    }

                    if !engine.is_model_available() {
                        return Err(format!(
                            "Local Whisper model not found: {}。请在设置中确认模型目录与已下载模型",
                            engine.model_path().display()
                        ));
                    }
                    if !engine.is_binary_available() {
                        return Err(
                            "whisper.cpp 引擎未下载，请在设置中下载引擎二进制".to_string(),
                        );
                    }
                    Ok::<(std::path::PathBuf, std::path::PathBuf), String>((
                        engine.model_path().to_path_buf(),
                        engine.binary_path_clone(),
                    ))
                }?;
                // 异步转写（不持有引擎锁，避免阻塞其他操作）
                let lang_ref = lang_opt.as_deref();
                LocalWhisperEngine::transcribe_at(&binary_path, &model_path, &samples_i16, lang_ref)
                    .await
                    .map_err(|e| format!("Whisper error: {}", e))
            }
            .await
        } else {
            let wav_result = processor::encode_wav_memory(&samples_i16, output_rate)
                .map_err(|e| format!("Failed to encode WAV: {}", e));

            let wav_data = match wav_result {
                Ok(v) => v,
                Err(e) => {
                    self.emit_status(AppStatus::Error(e.clone())).await;
                    Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Error).await;
                    return Err(e);
                }
            };

            self.api_client
                .process_audio(wav_data, &self.config)
                .await
                .map_err(|e| format!("API error: {}", e))
        };

        let recognition_result = match recognition_result {
            Ok(v) => v,
            Err(e) => {
                self.emit_status(AppStatus::Error(e.clone())).await;
                Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Error).await;
                return Err(e);
            }
        };

        let processed_text = handler::post_process(&recognition_result, &self.config);

        if !processed_text.is_empty() {
            crate::history::push(processed_text.clone());
        }

        if let Err(e) = self
            .output_handler
            .handle_output(processed_text.clone(), &self.config)
            .await
        {
            log::error!("Output handling error: {}", e);
        }

        if let Some(handle) = self.app_handle.lock().await.as_ref() {
            let _ = handle.emit("recording-result", processed_text.clone());
        }

        self.emit_status(AppStatus::Idle).await;
        Self::set_indicator_state(&self.app_handle, crate::indicator::IndicatorState::Success).await;

        log::info!("Recognition complete: {}", processed_text);
        Ok(processed_text)
    }

    async fn set_indicator_state(app_handle: &Arc<Mutex<Option<AppHandle>>>, state: crate::indicator::IndicatorState) {
        if let Some(ind) = crate::INDICATOR.get() {
            ind.set_state(state);
        }
        let _ = app_handle;
    }

    pub async fn cancel_recording(&self) -> Result<(), String> {
        if self.config.is_stream_mode() {
            let mut session_opt = {
                let mut runtime = self.streaming_runtime.lock().await;
                let _stream = runtime.stream.take();
                runtime.session.take()
            };
            if let Some(session) = session_opt.as_mut() {
                if session.is_running() {
                    session.cancel(&self.config).await;
                }
            }
        } else {
            let mut recorder = self.recorder.lock().await;
            if recorder.is_recording() {
                recorder.cancel().map_err(|e| e.to_string())?;
            }
        }
        self.emit_status(AppStatus::Idle).await;
        log::info!("Recording cancelled");
        Ok(())
    }

    pub async fn get_status(&self) -> String {
        self.status.lock().await.to_string()
    }
}

/// 扫描模型目录，返回第一个非空的 ggml-*.bin 文件名。
/// 优先级：tiny > base > small > medium > 其他 .bin
fn find_available_model(model_dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(model_dir).ok()?;
    let mut found: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "bin").unwrap_or(false) {
            let name = path.file_name()?.to_string_lossy().to_string();
            // 跳过空文件（< 1MB 视为不完整）
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            if size > 1_000_000 {
                found.push(name);
            }
        }
    }
    // 按优先级排序
    let priority = ["ggml-tiny.bin", "ggml-base.bin", "ggml-small.bin", "ggml-medium.bin"];
    found.sort_by_key(|n| {
        priority.iter().position(|p| p == n).unwrap_or(usize::MAX)
    });
    found.into_iter().next()
}
