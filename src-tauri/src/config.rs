use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const MODEL_TELEAI: &str = "TeleAI/TeleSpeechASR";
pub const MODEL_SENSEVOICE: &str = "FunAudioLLM/SenseVoiceSmall";
pub const MODEL_WHISPER: &str = "whisper-large-v3";
pub const MODEL_LOCAL_WHISPER: &str = "local-whisper";
pub const MODEL_CUSTOM: &str = "custom";

pub const STREAM_MODEL_DOUBAO: &str = "doubao";

pub const STREAMING_ASR_URI: &str = "/api/v3/sauc/bigmodel";
pub const STREAMING_RESOURCE_BIGASR_DURATION: &str = "volc.bigasr.sauc.duration";
pub const STREAMING_RESOURCE_SEEDASR_DURATION: &str = "volc.seedasr.sauc.duration";

pub const STREAMING_POST_AI: &str = "ai";
pub const STREAMING_POST_LOCAL: &str = "local";
pub const STREAMING_POST_NONE: &str = "none";

pub const DICTATION_MODE_BATCH: &str = "batch";
pub const DICTATION_MODE_STREAM: &str = "stream";

const SILICONFLOW_TRANSCRIPTIONS_URL: &str = "https://api.siliconflow.cn/v1/audio/transcriptions";
const GROQ_TRANSCRIPTIONS_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

#[derive(Clone)]
pub struct ConfigManager {
    config: Arc<Mutex<AppConfig>>,
    config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BasicConfig {
    pub model_name: String,
    pub output_language: String,
    pub output_mode: String,
    pub autostart: bool,
    pub hotkey: u32,
    pub show_log: bool,
    pub input_device: String,
    /// 语音输入页面的识别模式：`batch`（整段）或 `stream`（流式）
    pub dictation_mode: String,
}

impl Default for BasicConfig {
    fn default() -> Self {
        Self {
            model_name: MODEL_SENSEVOICE.to_string(),
            output_language: "auto".to_string(),
            output_mode: "clipboard".to_string(),
            autostart: false,
            hotkey: 0x71,
            show_log: false,
            input_device: String::new(),
            dictation_mode: "batch".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureConfig {
    pub allow_emoji: bool,
    pub allow_punctuation: bool,
    pub enable_indicator: bool,
    /// 是否启用增强后处理链（PostProcessorChain + TextFormatter）。
    /// false（默认）：使用现有 output::handler::post_process，行为不变。
    /// true：启用新的 LocalCorrector（错别字修正等）+ TextFormatter（中文标点/代码模式）。
    pub enable_post_processor: bool,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            allow_emoji: true,
            allow_punctuation: true,
            enable_indicator: true,
            enable_post_processor: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedConfig {
    pub trigger_mode: String,
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            trigger_mode: "hold".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    pub last_check_time: u64,
    pub ignored_version: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            last_check_time: 0,
            ignored_version: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub siliconflow_api_key: String,
    pub groq_api_key: String,
    pub doubao_api_key: String,
    pub custom_api_key: String,
    pub custom_api_url: String,
    pub custom_model_name: String,
    pub local_whisper_model: String,
    pub local_whisper_dir: String,
    /// 本地 whisper 线程数，0 = 自动取物理核数（上限 8）
    #[serde(default)]
    pub local_whisper_threads: u32,
    /// 贪婪解码（-bs 1 -bo 1），牺牲 CJK 准确率换速度，默认关
    #[serde(default)]
    pub local_whisper_greedy: bool,
    /// 关闭温度回退（-nf），跳过低置信重试，默认关
    #[serde(default)]
    pub local_whisper_no_fallback: bool,
    /// 持久化的 auto 语言检测结果（跨重启复用，避免首调检测开销）
    #[serde(default)]
    pub local_whisper_detected_language: String,
    /// 用户自定义的模型下载目录（为空时使用默认目录）
    pub custom_models_dir: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            siliconflow_api_key: String::new(),
            groq_api_key: String::new(),
            doubao_api_key: String::new(),
            custom_api_key: String::new(),
            custom_api_url: SILICONFLOW_TRANSCRIPTIONS_URL.to_string(),
            custom_model_name: "自定义模型".to_string(),
            local_whisper_model: "ggml-tiny.bin".to_string(),
            local_whisper_dir: String::new(),
            local_whisper_threads: 0,
            local_whisper_greedy: false,
            local_whisper_no_fallback: false,
            local_whisper_detected_language: String::new(),
            custom_models_dir: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamingConfig {
    pub hotkey: u32,
    pub trigger_mode: String,
    pub resource_id: String,
    pub model_name: String,
    pub output_language: String,
    pub allow_emoji: bool,
    pub allow_punctuation: bool,
    pub enable_indicator: bool,
    pub post_process_mode: String,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            hotkey: 0x75,
            trigger_mode: "hold".to_string(),
            resource_id: STREAMING_RESOURCE_BIGASR_DURATION.to_string(),
            model_name: "bigmodel".to_string(),
            output_language: String::new(),
            allow_emoji: true,
            allow_punctuation: true,
            enable_indicator: true,
            post_process_mode: STREAMING_POST_NONE.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorConfig {
    pub fade_duration: u64,
    pub error_duration: u64,
    pub success_duration: u64,
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        Self {
            fade_duration: 300,
            error_duration: 5000,
            success_duration: 5000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleConfig {
    pub subtitle_hotkey: u32,
    pub subtitle_font_size: u32,
    pub subtitle_font_color: String,
    pub subtitle_bg_opacity: f32,
    pub subtitle_blur: u32,
    pub subtitle_max_lines: u32,
    pub subtitle_window_x: i32,
    pub subtitle_window_y: i32,
    pub subtitle_window_width: u32,
    pub subtitle_window_height: u32,
    /// 字体族，如 "Microsoft YaHei"、"SimHei"、"Arial"
    pub subtitle_font_family: String,
    /// 字重 100-900
    pub subtitle_font_weight: u32,
    pub subtitle_italic: bool,
    /// 文字对齐：left | center | right
    pub subtitle_text_align: String,
    /// 字间距 px
    pub subtitle_letter_spacing: f32,
    /// 行高倍数，1.4 = 1.4em
    pub subtitle_line_height: f32,
    /// 文字阴影颜色
    pub subtitle_text_shadow_color: String,
    /// 文字阴影强度 0-10
    pub subtitle_text_shadow_strength: u32,
    /// 背景颜色 (RGB)
    pub subtitle_bg_color: String,
    /// 圆角 px
    pub subtitle_border_radius: u32,
    /// 边框颜色
    pub subtitle_border_color: String,
    /// 边框宽度 px
    pub subtitle_border_width: u32,
    /// 内边距 px
    pub subtitle_padding_x: u32,
    pub subtitle_padding_y: u32,
    /// 临时（中间结果）文字颜色
    pub subtitle_interim_color: String,
    /// 临时文字透明度 0-1
    pub subtitle_interim_opacity: f32,
}

impl Default for SubtitleConfig {
    fn default() -> Self {
        Self {
            subtitle_hotkey: 0x76,
            subtitle_font_size: 32,
            subtitle_font_color: "#ffffff".to_string(),
            subtitle_bg_opacity: 0.6,
            subtitle_blur: 20,
            subtitle_max_lines: 3,
            subtitle_window_x: -1,
            subtitle_window_y: -1,
            subtitle_window_width: 1200,
            subtitle_window_height: 120,
            subtitle_font_family: "Microsoft YaHei".to_string(),
            subtitle_font_weight: 400,
            subtitle_italic: false,
            subtitle_text_align: "center".to_string(),
            subtitle_letter_spacing: 0.0,
            subtitle_line_height: 1.4,
            subtitle_text_shadow_color: "#000000".to_string(),
            subtitle_text_shadow_strength: 4,
            subtitle_bg_color: "#000000".to_string(),
            subtitle_border_radius: 12,
            subtitle_border_color: "#ffffff".to_string(),
            subtitle_border_width: 0,
            subtitle_padding_x: 24,
            subtitle_padding_y: 12,
            subtitle_interim_color: "#ffffff".to_string(),
            subtitle_interim_opacity: 0.7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    pub vad_sensitivity: f32,
    pub vad_silence_duration_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            vad_sensitivity: 0.5,
            vad_silence_duration_ms: 800,
        }
    }
}

/// LLM 智能后处理配置。
///
/// 允许用户自行配置 OpenAI 兼容的 Chat 接口，对 ASR 识别文本做智能校对：
/// - 中文同音错字修正
/// - 语序与标点优化
/// - 上下文相关纠错
///
/// 默认禁用。启用后需配置 `api_url` / `api_key` / `model`。
/// 默认指向硅基流动（SiliconFlow）免费的 Qwen2.5-7B-Instruct，用户也可改为 OpenAI / DeepSeek / KIMI 等。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmPostProcessConfig {
    /// 是否启用 LLM 智能校对。
    pub enable: bool,
    /// OpenAI 兼容的 chat/completions 端点。
    pub api_url: String,
    /// API Key（Bearer 鉴权）。
    pub api_key: String,
    /// 模型名称，如 `Qwen/Qwen2.5-7B-Instruct`、`gpt-4o-mini`、`moonshot-v1-8k`。
    pub model: String,
}

impl Default for LlmPostProcessConfig {
    fn default() -> Self {
        Self {
            enable: false,
            // 硅基流动（SiliconFlow）免费模型
            api_url: "https://api.siliconflow.cn/v1/chat/completions".to_string(),
            api_key: String::new(),
            model: "Qwen/Qwen2.5-7B-Instruct".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSelectionConfig {
    pub batch_model: String,
    pub stream_model: String,
    pub subtitle_model: String,
}

impl Default for ModelSelectionConfig {
    fn default() -> Self {
        Self {
            batch_model: MODEL_SENSEVOICE.to_string(),
            stream_model: STREAM_MODEL_DOUBAO.to_string(),
            subtitle_model: STREAM_MODEL_DOUBAO.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub basic: BasicConfig,
    pub features: FeatureConfig,
    pub advanced: AdvancedConfig,
    pub update: UpdateConfig,
    pub model: ModelConfig,
    pub streaming: StreamingConfig,
    pub indicator: IndicatorConfig,
    pub subtitle: SubtitleConfig,
    pub vad: VadConfig,
    pub model_selection: ModelSelectionConfig,
    pub llm_post: LlmPostProcessConfig,
    pub theme: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_log: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_emoji: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_punctuation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_indicator: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignored_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub siliconflow_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groq_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_api_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator_fade_duration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator_error_duration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator_success_duration: Option<u64>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            basic: BasicConfig::default(),
            features: FeatureConfig::default(),
            advanced: AdvancedConfig::default(),
            update: UpdateConfig::default(),
            model: ModelConfig::default(),
            streaming: StreamingConfig::default(),
            indicator: IndicatorConfig::default(),
            subtitle: SubtitleConfig::default(),
            vad: VadConfig::default(),
            model_selection: ModelSelectionConfig::default(),
            llm_post: LlmPostProcessConfig::default(),
            theme: "auto".to_string(),
            model_name: None,
            output_language: None,
            output_mode: None,
            autostart: None,
            hotkey: None,
            show_log: None,
            allow_emoji: None,
            allow_punctuation: None,
            enable_indicator: None,
            trigger_mode: None,
            last_check_time: None,
            ignored_version: None,
            siliconflow_api_key: None,
            groq_api_key: None,
            custom_api_key: None,
            custom_api_url: None,
            custom_model_name: None,
            indicator_fade_duration: None,
            indicator_error_duration: None,
            indicator_success_duration: None,
        }
    }
}

impl AppConfig {
    pub fn initialize(&mut self) {
        if let Some(value) = self.model_name.take() {
            self.basic.model_name = value;
        }
        if let Some(value) = self.output_language.take() {
            self.basic.output_language = value;
        }
        if let Some(value) = self.output_mode.take() {
            self.basic.output_mode = value;
        }
        if let Some(value) = self.autostart.take() {
            self.basic.autostart = value;
        }
        if let Some(value) = self.hotkey.take() {
            self.basic.hotkey = value;
        }
        if let Some(value) = self.show_log.take() {
            self.basic.show_log = value;
        }
        if let Some(value) = self.allow_emoji.take() {
            self.features.allow_emoji = value;
        }
        if let Some(value) = self.allow_punctuation.take() {
            self.features.allow_punctuation = value;
        }
        if let Some(value) = self.enable_indicator.take() {
            self.features.enable_indicator = value;
        }
        if let Some(value) = self.trigger_mode.take() {
            self.advanced.trigger_mode = value;
        }
        if let Some(value) = self.last_check_time.take() {
            self.update.last_check_time = value;
        }
        if let Some(value) = self.ignored_version.take() {
            self.update.ignored_version = value;
        }
        if let Some(value) = self.siliconflow_api_key.take() {
            self.model.siliconflow_api_key = value;
        }
        if let Some(value) = self.groq_api_key.take() {
            self.model.groq_api_key = value;
        }
        if let Some(value) = self.custom_api_key.take() {
            self.model.custom_api_key = value;
        }
        if let Some(value) = self.custom_api_url.take() {
            self.model.custom_api_url = value;
        }
        if let Some(value) = self.custom_model_name.take() {
            self.model.custom_model_name = value;
        }
        if let Some(value) = self.indicator_fade_duration.take() {
            self.indicator.fade_duration = value;
        }
        if let Some(value) = self.indicator_error_duration.take() {
            self.indicator.error_duration = value;
        }
        if let Some(value) = self.indicator_success_duration.take() {
            self.indicator.success_duration = value;
        }
        self.streaming.post_process_mode =
            Self::normalize_streaming_post_process_mode(&self.streaming.post_process_mode);
        self.basic.dictation_mode = Self::normalize_dictation_mode(&self.basic.dictation_mode);
        self.subtitle.subtitle_bg_opacity = self.subtitle.subtitle_bg_opacity.clamp(0.0, 1.0);
        self.subtitle.subtitle_interim_opacity = self.subtitle.subtitle_interim_opacity.clamp(0.0, 1.0);
        self.subtitle.subtitle_font_weight = self.subtitle.subtitle_font_weight.clamp(100, 900);
        // 取整到 100 的倍数
        self.subtitle.subtitle_font_weight = (self.subtitle.subtitle_font_weight / 100) * 100;
        self.subtitle.subtitle_text_shadow_strength = self.subtitle.subtitle_text_shadow_strength.min(10);
        self.subtitle.subtitle_letter_spacing = self.subtitle.subtitle_letter_spacing.clamp(-5.0, 20.0);
        self.subtitle.subtitle_line_height = self.subtitle.subtitle_line_height.clamp(0.8, 3.0);
        // 对齐值归一
        self.subtitle.subtitle_text_align = match self.subtitle.subtitle_text_align.as_str() {
            "left" | "center" | "right" => self.subtitle.subtitle_text_align.clone(),
            _ => "center".to_string(),
        };
        self.vad.vad_sensitivity = self.vad.vad_sensitivity.clamp(0.0, 1.0);
    }

    fn normalize_streaming_post_process_mode(mode: &str) -> String {
        match mode {
            STREAMING_POST_AI | STREAMING_POST_LOCAL | STREAMING_POST_NONE => mode.to_string(),
            _ => STREAMING_POST_NONE.to_string(),
        }
    }

    fn normalize_dictation_mode(mode: &str) -> String {
        match mode {
            DICTATION_MODE_BATCH | DICTATION_MODE_STREAM => mode.to_string(),
            _ => DICTATION_MODE_BATCH.to_string(),
        }
    }
}

impl ConfigManager {
    pub fn new() -> Self {
        Self::new_with_base_dir(None)
    }

    pub fn new_with_base_dir(base_dir: Option<PathBuf>) -> Self {
        let config_dir = if let Some(dir) = base_dir {
            dir
        } else if let Some(proj_dirs) = directories::ProjectDirs::from("com", "guchang233", "voice2type")
        {
            proj_dirs.config_dir().to_path_buf()
        } else {
            PathBuf::from(".")
        };

        let _ = fs::create_dir_all(&config_dir);
        let path = config_dir.join("settings.json");

        let legacy_path = PathBuf::from("voice2type_config.json");
        let actual_path = if !path.exists() && legacy_path.exists() {
            legacy_path
        } else {
            path
        };

        let mut config = Self::load_config(&actual_path).unwrap_or_default();

        if !config.model_selection.batch_model.is_empty() {
            config.basic.model_name = config.model_selection.batch_model.clone();
        }

        if !actual_path.exists() {
            if let Ok(key) = std::env::var("SILICONFLOW_API_KEY") {
                config.model.siliconflow_api_key = key;
            }
        }

        let manager = Self {
            config: Arc::new(Mutex::new(config)),
            config_path: actual_path,
        };
        manager.migrate_legacy_whisper_dir();
        manager
    }

    pub fn models_dir(&self) -> PathBuf {
        // 优先使用用户自定义目录
        let custom = self.config.lock().unwrap().model.custom_models_dir.clone();
        if !custom.is_empty() && PathBuf::from(&custom).is_dir() {
            return PathBuf::from(custom);
        }
        // 默认目录：Windows 优先 D:\V2T\models（避免占用 C 盘）
        // 若 D 盘不存在，则回退到用户数据目录下的 models
        #[cfg(target_os = "windows")]
        {
            if std::path::Path::new("D:\\").exists() {
                return PathBuf::from("D:\\V2T\\models");
            }
        }
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Voice2Type")
            .join("models");
        data_dir
    }

    /// 获取当前生效的模型目录（只读，用于前端展示）
    pub fn current_models_dir(&self) -> String {
        self.models_dir().to_string_lossy().into_owned()
    }

    /// 设置自定义模型目录
    pub fn set_custom_models_dir(&self, dir: String) {
        self.config.lock().unwrap().model.custom_models_dir = dir;
    }

    /// 清除自定义模型目录（恢复默认）
    pub fn clear_custom_models_dir(&self) {
        self.config.lock().unwrap().model.custom_models_dir = String::new();
    }

    pub fn ensure_models_dir(&self) -> anyhow::Result<PathBuf> {
        let dir = self.models_dir();
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn whisper_models_dir(&self) -> PathBuf {
        // 模型 .bin 文件直接放在模型目录根目录（与前端展示和帮助教程一致）
        self.models_dir()
    }

    /// whisper.cpp 预编译二进制存放目录
    pub fn whisper_binary_dir(&self) -> PathBuf {
        self.models_dir().join("whisper-bin")
    }

    /// whisper.cpp 可执行文件完整路径（Windows: whisper-cli.exe）
    /// v1.9.2 起 main.exe 已弃用（仅输出警告后退出），改用 whisper-cli
    pub fn whisper_binary_path(&self) -> PathBuf {
        if cfg!(target_os = "windows") {
            self.whisper_binary_dir().join("whisper-cli.exe")
        } else {
            self.whisper_binary_dir().join("whisper-cli")
        }
    }



    fn migrate_legacy_whisper_dir(&self) {
        let legacy = self.config_dir().join("whisper");
        let mut cfg = self.config.lock().unwrap();
        if cfg.model.local_whisper_dir.is_empty() && legacy.is_dir() {
            cfg.model.local_whisper_dir = legacy.to_string_lossy().into_owned();
        }
    }

    fn load_config(path: &PathBuf) -> Option<AppConfig> {
        let content = fs::read_to_string(path).ok()?;

        if let Ok(mut loaded) = serde_json::from_str::<AppConfig>(&content) {
            loaded.initialize();
            return Some(loaded);
        }

        #[derive(Debug, Deserialize)]
        struct OldAppConfig {
            api_key: String,
            api_url: String,
            model_name: String,
        }

        let old = serde_json::from_str::<OldAppConfig>(&content).ok()?;
        let mut config = AppConfig::default();
        config.basic.model_name = old.model_name.clone();

        match old.model_name.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => config.model.siliconflow_api_key = old.api_key,
            MODEL_WHISPER => config.model.groq_api_key = old.api_key,
            _ => {
                config.basic.model_name = MODEL_CUSTOM.to_string();
                config.model.custom_model_name = old.model_name;
                config.model.custom_api_key = old.api_key;
                config.model.custom_api_url = old.api_url;
            }
        }

        Some(config)
    }

    pub fn get_config(&self) -> AppConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn set_config(&self, new_config: AppConfig) {
        let mut cfg = self.config.lock().unwrap();
        *cfg = new_config;
        cfg.basic.model_name = cfg.model_selection.batch_model.clone();
    }

    fn effective_model_name(&self) -> String {
        let cfg = self.config.lock().unwrap();
        let batch = &cfg.model_selection.batch_model;
        if !batch.is_empty() {
            batch.clone()
        } else {
            cfg.basic.model_name.clone()
        }
    }

    pub fn is_local_whisper(&self) -> bool {
        self.effective_model_name() == MODEL_LOCAL_WHISPER
    }

    pub fn needs_api_key(&self) -> bool {
        self.effective_model_name() != MODEL_LOCAL_WHISPER
    }

    pub fn get_api_key(&self) -> String {
        let cfg = self.config.lock().unwrap();
        let model = if cfg.model_selection.batch_model.is_empty() {
            cfg.basic.model_name.clone()
        } else {
            cfg.model_selection.batch_model.clone()
        };
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => cfg.model.siliconflow_api_key.clone(),
            MODEL_WHISPER => cfg.model.groq_api_key.clone(),
            MODEL_LOCAL_WHISPER => String::new(),
            _ => cfg.model.custom_api_key.clone(),
        }
    }

    pub fn set_api_key(&self, key: String) {
        let mut cfg = self.config.lock().unwrap();
        let model = if cfg.model_selection.batch_model.is_empty() {
            cfg.basic.model_name.clone()
        } else {
            cfg.model_selection.batch_model.clone()
        };
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => cfg.model.siliconflow_api_key = key,
            MODEL_WHISPER => cfg.model.groq_api_key = key,
            _ => cfg.model.custom_api_key = key,
        }
    }

    pub fn get_siliconflow_api_key(&self) -> String {
        self.config.lock().unwrap().model.siliconflow_api_key.clone()
    }

    pub fn set_siliconflow_api_key(&self, key: String) {
        self.config.lock().unwrap().model.siliconflow_api_key = key;
    }

    pub fn get_groq_api_key(&self) -> String {
        self.config.lock().unwrap().model.groq_api_key.clone()
    }

    pub fn set_groq_api_key(&self, key: String) {
        self.config.lock().unwrap().model.groq_api_key = key;
    }

    pub fn get_doubao_api_key(&self) -> String {
        self.config.lock().unwrap().model.doubao_api_key.clone()
    }

    pub fn set_doubao_api_key(&self, key: String) {
        self.config.lock().unwrap().model.doubao_api_key = key;
    }

    pub fn streaming_hotkey(&self) -> u32 {
        self.config.lock().unwrap().streaming.hotkey
    }

    pub fn set_streaming_hotkey(&self, vk: u32) {
        self.config.lock().unwrap().streaming.hotkey = vk;
    }

    pub fn streaming_trigger_mode(&self) -> String {
        self.config.lock().unwrap().streaming.trigger_mode.clone()
    }

    pub fn set_streaming_trigger_mode(&self, mode: String) {
        self.config.lock().unwrap().streaming.trigger_mode = mode;
    }

    pub fn streaming_resource_id(&self) -> String {
        self.config.lock().unwrap().streaming.resource_id.clone()
    }

    pub fn set_streaming_resource_id(&self, id: String) {
        self.config.lock().unwrap().streaming.resource_id = id;
    }

    pub fn streaming_model_name(&self) -> String {
        self.config.lock().unwrap().streaming.model_name.clone()
    }

    pub fn streaming_output_language(&self) -> String {
        self.config.lock().unwrap().streaming.output_language.clone()
    }

    pub fn set_streaming_output_language(&self, lang: String) {
        self.config.lock().unwrap().streaming.output_language = lang;
    }

    pub fn streaming_allow_emoji(&self) -> bool {
        self.config.lock().unwrap().streaming.allow_emoji
    }

    pub fn set_streaming_allow_emoji(&self, allow: bool) {
        self.config.lock().unwrap().streaming.allow_emoji = allow;
    }

    pub fn streaming_allow_punctuation(&self) -> bool {
        self.config.lock().unwrap().streaming.allow_punctuation
    }

    pub fn set_streaming_allow_punctuation(&self, allow: bool) {
        self.config.lock().unwrap().streaming.allow_punctuation = allow;
    }

    pub fn streaming_enable_indicator(&self) -> bool {
        self.config.lock().unwrap().streaming.enable_indicator
    }

    pub fn set_streaming_enable_indicator(&self, enable: bool) {
        self.config.lock().unwrap().streaming.enable_indicator = enable;
    }

    pub fn streaming_post_process_mode(&self) -> String {
        self.config.lock().unwrap().streaming.post_process_mode.clone()
    }

    pub fn set_streaming_post_process_mode(&self, mode: String) {
        self.config.lock().unwrap().streaming.post_process_mode = mode;
    }

    pub fn get_api_url(&self) -> String {
        let cfg = self.config.lock().unwrap();
        let model = if cfg.model_selection.batch_model.is_empty() {
            cfg.basic.model_name.clone()
        } else {
            cfg.model_selection.batch_model.clone()
        };
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => SILICONFLOW_TRANSCRIPTIONS_URL.to_string(),
            MODEL_WHISPER => GROQ_TRANSCRIPTIONS_URL.to_string(),
            _ => cfg.model.custom_api_url.clone(),
        }
    }

    pub fn set_api_url(&self, url: String) {
        let mut cfg = self.config.lock().unwrap();
        if cfg.basic.model_name == MODEL_CUSTOM {
            cfg.model.custom_api_url = url;
        }
    }

    pub fn get_model_id(&self) -> String {
        self.effective_model_name()
    }

    pub fn get_model_name(&self) -> String {
        let cfg = self.config.lock().unwrap();
        let model = if cfg.model_selection.batch_model.is_empty() {
            cfg.basic.model_name.clone()
        } else {
            cfg.model_selection.batch_model.clone()
        };
        if model == MODEL_CUSTOM {
            cfg.model.custom_model_name.clone()
        } else if model == MODEL_LOCAL_WHISPER {
            format!(
                "本地 Whisper ({})",
                if cfg.model.local_whisper_model.is_empty() {
                    "ggml-base.bin"
                } else {
                    &cfg.model.local_whisper_model
                }
            )
        } else {
            model
        }
    }

    pub fn set_model_name(&self, model: String) {
        let mut cfg = self.config.lock().unwrap();
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE | MODEL_WHISPER | MODEL_LOCAL_WHISPER => {
                cfg.basic.model_name = model
            }
            _ => {
                cfg.basic.model_name = MODEL_CUSTOM.to_string();
                cfg.model.custom_model_name = model;
            }
        }
    }

    pub fn local_whisper_model(&self) -> String {
        self.config.lock().unwrap().model.local_whisper_model.clone()
    }

    pub fn set_local_whisper_model(&self, name: String) {
        self.config.lock().unwrap().model.local_whisper_model = name;
    }

    pub fn local_whisper_threads(&self) -> u32 {
        self.config.lock().unwrap().model.local_whisper_threads
    }

    pub fn set_local_whisper_threads(&self, threads: u32) {
        self.config.lock().unwrap().model.local_whisper_threads = threads;
    }

    pub fn local_whisper_greedy(&self) -> bool {
        self.config.lock().unwrap().model.local_whisper_greedy
    }

    pub fn set_local_whisper_greedy(&self, greedy: bool) {
        self.config.lock().unwrap().model.local_whisper_greedy = greedy;
    }

    pub fn local_whisper_no_fallback(&self) -> bool {
        self.config.lock().unwrap().model.local_whisper_no_fallback
    }

    pub fn set_local_whisper_no_fallback(&self, no_fallback: bool) {
        self.config.lock().unwrap().model.local_whisper_no_fallback = no_fallback;
    }

    pub fn local_whisper_detected_language(&self) -> String {
        self.config.lock()
            .unwrap()
            .model
            .local_whisper_detected_language
            .clone()
    }

    pub fn set_local_whisper_detected_language(&self, lang: String) {
        self.config.lock().unwrap().model.local_whisper_detected_language = lang;
    }

    pub fn local_whisper_dir(&self) -> String {
        self.config.lock().unwrap().model.local_whisper_dir.clone()
    }

    pub fn set_local_whisper_dir(&self, path: String) {
        self.config.lock().unwrap().model.local_whisper_dir = path;
    }

    pub fn has_local_whisper_dir(&self) -> bool {
        !self.local_whisper_dir().is_empty()
    }

    pub fn allow_emoji(&self) -> bool {
        self.config.lock().unwrap().features.allow_emoji
    }

    pub fn set_allow_emoji(&self, allow: bool) {
        self.config.lock().unwrap().features.allow_emoji = allow;
    }

    pub fn allow_punctuation(&self) -> bool {
        self.config.lock().unwrap().features.allow_punctuation
    }

    pub fn set_allow_punctuation(&self, allow: bool) {
        self.config.lock().unwrap().features.allow_punctuation = allow;
    }

    pub fn show_log(&self) -> bool {
        self.config.lock().unwrap().basic.show_log
    }

    pub fn set_show_log(&self, show: bool) {
        self.config.lock().unwrap().basic.show_log = show;
    }

    pub fn input_device(&self) -> String {
        self.config.lock().unwrap().basic.input_device.clone()
    }

    pub fn set_input_device(&self, name: String) {
        self.config.lock().unwrap().basic.input_device = name;
    }

    pub fn output_mode(&self) -> String {
        self.config.lock().unwrap().basic.output_mode.clone()
    }

    pub fn set_output_mode(&self, mode: String) {
        self.config.lock().unwrap().basic.output_mode = mode;
    }

    pub fn autostart_enabled(&self) -> bool {
        self.config.lock().unwrap().basic.autostart
    }

    pub fn set_autostart_enabled(&self, enabled: bool) {
        self.config.lock().unwrap().basic.autostart = enabled;
    }

    pub fn hotkey(&self) -> u32 {
        self.config.lock().unwrap().basic.hotkey
    }

    pub fn set_hotkey(&self, vk: u32) {
        self.config.lock().unwrap().basic.hotkey = vk;
    }

    pub fn enable_indicator(&self) -> bool {
        self.config.lock().unwrap().features.enable_indicator
    }

    pub fn set_enable_indicator(&self, enable: bool) {
        self.config.lock().unwrap().features.enable_indicator = enable;
    }

    /// 是否启用增强后处理链（PostProcessorChain + TextFormatter）。默认 false。
    pub fn enable_post_processor(&self) -> bool {
        self.config.lock().unwrap().features.enable_post_processor
    }

    pub fn set_enable_post_processor(&self, enable: bool) {
        self.config.lock().unwrap().features.enable_post_processor = enable;
    }

    /// 是否启用 LLM 智能后处理校对。默认 false。
    pub fn llm_post_enable(&self) -> bool {
        self.config.lock().unwrap().llm_post.enable
    }

    pub fn set_llm_post_enable(&self, enable: bool) {
        self.config.lock().unwrap().llm_post.enable = enable;
    }

    pub fn llm_post_api_url(&self) -> String {
        self.config.lock().unwrap().llm_post.api_url.clone()
    }

    pub fn set_llm_post_api_url(&self, url: String) {
        self.config.lock().unwrap().llm_post.api_url = url;
    }

    pub fn llm_post_api_key(&self) -> String {
        self.config.lock().unwrap().llm_post.api_key.clone()
    }

    pub fn set_llm_post_api_key(&self, key: String) {
        self.config.lock().unwrap().llm_post.api_key = key;
    }

    pub fn llm_post_model(&self) -> String {
        self.config.lock().unwrap().llm_post.model.clone()
    }

    pub fn set_llm_post_model(&self, model: String) {
        self.config.lock().unwrap().llm_post.model = model;
    }

    pub fn last_check_time(&self) -> u64 {
        self.config.lock().unwrap().update.last_check_time
    }

    pub fn set_last_check_time(&self, time: u64) {
        self.config.lock().unwrap().update.last_check_time = time;
    }

    pub fn ignored_version(&self) -> String {
        self.config.lock().unwrap().update.ignored_version.clone()
    }

    pub fn set_ignored_version(&self, version: String) {
        self.config.lock().unwrap().update.ignored_version = version;
    }

    pub fn output_language(&self) -> String {
        self.config.lock().unwrap().basic.output_language.clone()
    }

    pub fn set_output_language(&self, lang: String) {
        self.config.lock().unwrap().basic.output_language = lang;
    }

    pub fn trigger_mode(&self) -> String {
        self.config.lock().unwrap().advanced.trigger_mode.clone()
    }

    pub fn set_trigger_mode(&self, mode: String) {
        self.config.lock().unwrap().advanced.trigger_mode = mode;
    }

    pub fn dictation_mode(&self) -> String {
        self.config.lock().unwrap().basic.dictation_mode.clone()
    }

    pub fn set_dictation_mode(&self, mode: String) {
        self.config.lock().unwrap().basic.dictation_mode = mode;
    }

    pub fn is_stream_mode(&self) -> bool {
        self.dictation_mode() == DICTATION_MODE_STREAM
    }

    pub fn get_speech_service(&self) -> String {
        let model = self.effective_model_name();
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => "siliconflow".to_string(),
            MODEL_WHISPER => "groq".to_string(),
            MODEL_LOCAL_WHISPER => "local".to_string(),
            _ => "custom".to_string(),
        }
    }

    pub fn indicator_fade_duration(&self) -> u64 {
        self.config.lock().unwrap().indicator.fade_duration
    }

    pub fn set_indicator_fade_duration(&self, duration: u64) {
        self.config.lock().unwrap().indicator.fade_duration = duration;
    }

    pub fn indicator_error_duration(&self) -> u64 {
        self.config.lock().unwrap().indicator.error_duration
    }

    pub fn set_indicator_error_duration(&self, duration: u64) {
        self.config.lock().unwrap().indicator.error_duration = duration;
    }

    pub fn indicator_success_duration(&self) -> u64 {
        self.config.lock().unwrap().indicator.success_duration
    }

    pub fn set_indicator_success_duration(&self, duration: u64) {
        self.config.lock().unwrap().indicator.success_duration = duration;
    }

    pub fn subtitle_hotkey(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_hotkey
    }

    pub fn set_subtitle_hotkey(&self, vk: u32) {
        self.config.lock().unwrap().subtitle.subtitle_hotkey = vk;
    }

    pub fn subtitle_font_size(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_font_size
    }

    pub fn set_subtitle_font_size(&self, size: u32) {
        self.config.lock().unwrap().subtitle.subtitle_font_size = size;
    }

    pub fn subtitle_font_color(&self) -> String {
        self.config.lock().unwrap().subtitle.subtitle_font_color.clone()
    }

    pub fn set_subtitle_font_color(&self, color: String) {
        self.config.lock().unwrap().subtitle.subtitle_font_color = color;
    }

    pub fn subtitle_bg_opacity(&self) -> f32 {
        self.config.lock().unwrap().subtitle.subtitle_bg_opacity
    }

    pub fn set_subtitle_bg_opacity(&self, opacity: f32) {
        self.config.lock().unwrap().subtitle.subtitle_bg_opacity = opacity.clamp(0.0, 1.0);
    }

    pub fn subtitle_blur(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_blur
    }

    pub fn set_subtitle_blur(&self, blur: u32) {
        self.config.lock().unwrap().subtitle.subtitle_blur = blur;
    }

    pub fn subtitle_max_lines(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_max_lines
    }

    pub fn set_subtitle_max_lines(&self, lines: u32) {
        self.config.lock().unwrap().subtitle.subtitle_max_lines = lines;
    }

    pub fn subtitle_window_x(&self) -> i32 {
        self.config.lock().unwrap().subtitle.subtitle_window_x
    }

    pub fn set_subtitle_window_x(&self, x: i32) {
        self.config.lock().unwrap().subtitle.subtitle_window_x = x;
    }

    pub fn subtitle_window_y(&self) -> i32 {
        self.config.lock().unwrap().subtitle.subtitle_window_y
    }

    pub fn set_subtitle_window_y(&self, y: i32) {
        self.config.lock().unwrap().subtitle.subtitle_window_y = y;
    }

    pub fn subtitle_window_width(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_window_width
    }

    pub fn set_subtitle_window_width(&self, width: u32) {
        self.config.lock().unwrap().subtitle.subtitle_window_width = width;
    }

    pub fn subtitle_window_height(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_window_height
    }

    pub fn set_subtitle_window_height(&self, height: u32) {
        self.config.lock().unwrap().subtitle.subtitle_window_height = height;
    }

    pub fn vad_sensitivity(&self) -> f32 {
        self.config.lock().unwrap().vad.vad_sensitivity
    }

    pub fn set_vad_sensitivity(&self, sensitivity: f32) {
        self.config.lock().unwrap().vad.vad_sensitivity = sensitivity.clamp(0.0, 1.0);
    }

    pub fn vad_silence_duration_ms(&self) -> u32 {
        self.config.lock().unwrap().vad.vad_silence_duration_ms
    }

    pub fn set_vad_silence_duration_ms(&self, duration: u32) {
        self.config.lock().unwrap().vad.vad_silence_duration_ms = duration;
    }

    pub fn batch_model(&self) -> String {
        self.config.lock().unwrap().model_selection.batch_model.clone()
    }

    pub fn set_batch_model(&self, model: String) {
        self.config.lock().unwrap().model_selection.batch_model = model;
    }

    pub fn stream_model(&self) -> String {
        self.config.lock().unwrap().model_selection.stream_model.clone()
    }

    pub fn set_stream_model(&self, model: String) {
        self.config.lock().unwrap().model_selection.stream_model = model;
    }

    pub fn subtitle_model(&self) -> String {
        self.config.lock().unwrap().model_selection.subtitle_model.clone()
    }

    pub fn set_subtitle_model(&self, model: String) {
        self.config.lock().unwrap().model_selection.subtitle_model = model;
    }

    pub fn theme(&self) -> String {
        self.config.lock().unwrap().theme.clone()
    }

    pub fn set_theme(&self, theme: String) {
        self.config.lock().unwrap().theme = theme;
    }

    pub fn reset_ai_config(&self) {
        let mut cfg = self.config.lock().unwrap();
        cfg.basic.model_name = MODEL_SENSEVOICE.to_string();
        cfg.model = ModelConfig::default();
        cfg.advanced.trigger_mode = AdvancedConfig::default().trigger_mode;
        cfg.indicator = IndicatorConfig::default();
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config = self.config.lock().unwrap();
        let json = serde_json::to_string_pretty(&*config)?;
        fs::write(&self.config_path, json)?;
        Ok(())
    }

    pub fn save_or_notify(&self) -> bool {
        match self.save() {
            Ok(()) => true,
            Err(e) => {
                crate::notify::queue_tray_message("配置保存失败", &e.to_string());
                false
            }
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_path.clone()
    }

    pub fn config_dir(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn get_custom_model_name(&self) -> String {
        self.config.lock().unwrap().model.custom_model_name.clone()
    }

    pub fn set_custom_model_name(&self, name: String) {
        let mut cfg = self.config.lock().unwrap();
        cfg.model.custom_model_name = name;
    }

    pub fn log_dir(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(|path| path.join("logs"))
            .unwrap_or_else(|| PathBuf::from("logs"))
    }

    pub fn log_file_path(&self) -> PathBuf {
        self.log_dir().join("app.log")
    }
}
