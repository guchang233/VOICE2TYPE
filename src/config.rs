use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const MODEL_TELEAI: &str = "TeleAI/TeleSpeechASR";
pub const MODEL_SENSEVOICE: &str = "FunAudioLLM/SenseVoiceSmall";
pub const MODEL_WHISPER: &str = "whisper-large-v3";
pub const MODEL_LOCAL_WHISPER: &str = "local-whisper";
pub const MODEL_CUSTOM: &str = "custom";

/// 豆包大模型流式语音识别 WebSocket 路径（双向流式）
pub const STREAMING_ASR_URI: &str = "/api/v3/sauc/bigmodel";
pub const STREAMING_RESOURCE_BIGASR_DURATION: &str = "volc.bigasr.sauc.duration";
pub const STREAMING_RESOURCE_SEEDASR_DURATION: &str = "volc.seedasr.sauc.duration";

/// 流式后处理：AI 润色（硅基流动 / Groq）
pub const STREAMING_POST_AI: &str = "ai";
/// 流式后处理：本地轻量规则
pub const STREAMING_POST_LOCAL: &str = "local";
/// 流式后处理：关闭
pub const STREAMING_POST_NONE: &str = "none";

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
    /// 空字符串表示系统默认麦克风
    pub input_device: String,
}

impl Default for BasicConfig {
    fn default() -> Self {
        Self {
            model_name: MODEL_SENSEVOICE.to_string(),
            output_language: "auto".to_string(),
            output_mode: "clipboard".to_string(),
            autostart: false,
            hotkey: 0x71, // F2
            show_log: false,
            input_device: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureConfig {
    pub allow_emoji: bool,
    pub allow_punctuation: bool,
    pub enable_indicator: bool,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            allow_emoji: true,
            allow_punctuation: true,
            enable_indicator: true,
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
    /// `models/` 下的 ggml 模型文件名，例如 ggml-base.bin
    pub local_whisper_model: String,
    /// 用户自选的 Whisper 根目录（其下应有 bin/、models/）
    pub local_whisper_dir: String,
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
            local_whisper_model: "ggml-base.bin".to_string(),
            local_whisper_dir: String::new(),
        }
    }
}

/// 流式语音识别（豆包）独立配置，与录音文件识别模式隔离。
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
    /// ai | local | none
    pub post_process_mode: String,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            hotkey: 0x75, // F6
            trigger_mode: "hold".to_string(),
            resource_id: STREAMING_RESOURCE_BIGASR_DURATION.to_string(),
            model_name: "bigmodel".to_string(),
            output_language: String::new(),
            allow_emoji: true,
            allow_punctuation: true,
            enable_indicator: true,
            post_process_mode: STREAMING_POST_LOCAL.to_string(),
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
pub struct AppConfig {
    pub basic: BasicConfig,
    pub features: FeatureConfig,
    pub advanced: AdvancedConfig,
    pub update: UpdateConfig,
    pub model: ModelConfig,
    pub streaming: StreamingConfig,
    pub indicator: IndicatorConfig,

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
    }

    fn normalize_streaming_post_process_mode(mode: &str) -> String {
        match mode {
            STREAMING_POST_AI | STREAMING_POST_LOCAL | STREAMING_POST_NONE => mode.to_string(),
            _ => STREAMING_POST_LOCAL.to_string(),
        }
    }
}

impl ConfigManager {
    pub fn new() -> Self {
        let mut path = PathBuf::from("voice2type_config.json");
        if !path.exists() {
            if let Some(proj_dirs) =
                directories::ProjectDirs::from("com", "guchang233", "voice2type")
            {
                let config_dir = proj_dirs.config_dir();
                let _ = fs::create_dir_all(config_dir);
                path = config_dir.join("settings.json");
            }
        }

        let mut config = Self::load_config(&path).unwrap_or_default();

        if !path.exists() {
            if let Ok(key) = std::env::var("SILICONFLOW_API_KEY") {
                config.model.siliconflow_api_key = key;
            }
        }

        let manager = Self {
            config: Arc::new(Mutex::new(config)),
            config_path: path,
        };
        manager.migrate_legacy_whisper_dir();
        manager
    }

    /// 若用户尚未配置目录，但旧版默认路径 `{config_dir}/whisper` 已存在，则自动迁移。
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

    pub fn is_local_whisper(&self) -> bool {
        self.config.lock().unwrap().basic.model_name == MODEL_LOCAL_WHISPER
    }

    pub fn needs_api_key(&self) -> bool {
        self.config.lock().unwrap().basic.model_name != MODEL_LOCAL_WHISPER
    }

    pub fn get_api_key(&self) -> String {
        let cfg = self.config.lock().unwrap();
        match cfg.basic.model_name.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => cfg.model.siliconflow_api_key.clone(),
            MODEL_WHISPER => cfg.model.groq_api_key.clone(),
            MODEL_LOCAL_WHISPER => String::new(),
            _ => cfg.model.custom_api_key.clone(),
        }
    }

    pub fn set_api_key(&self, key: String) {
        let mut cfg = self.config.lock().unwrap();
        match cfg.basic.model_name.as_str() {
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
        match cfg.basic.model_name.as_str() {
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
        self.config.lock().unwrap().basic.model_name.clone()
    }

    pub fn get_model_name(&self) -> String {
        let cfg = self.config.lock().unwrap();
        if cfg.basic.model_name == MODEL_CUSTOM {
            cfg.model.custom_model_name.clone()
        } else if cfg.basic.model_name == MODEL_LOCAL_WHISPER {
            format!(
                "本地 Whisper ({})",
                if cfg.model.local_whisper_model.is_empty() {
                    "ggml-base.bin"
                } else {
                    &cfg.model.local_whisper_model
                }
            )
        } else {
            cfg.basic.model_name.clone()
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

    pub fn get_speech_service(&self) -> String {
        let cfg = self.config.lock().unwrap();
        match cfg.basic.model_name.as_str() {
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

    /// 保存配置；失败时通过托盘通知用户。
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
