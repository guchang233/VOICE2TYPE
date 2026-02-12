use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// 配置管理器
// 处理 API Key 和设置的加载/保存
#[derive(Clone)]
pub struct ConfigManager {
    config: Arc<Mutex<AppConfig>>,
    config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub api_key: String,
    pub api_url: String,
    pub model_name: String,
    pub allow_emoji: bool,
    pub allow_punctuation: bool,
    pub show_log: bool,
    pub language: String, // "zh" 或 "en"
    pub output_mode: String,
    pub autostart: bool,
    pub hotkey: u32, // Windows 虚拟键码
    pub enable_indicator: bool,
    pub last_check_time: u64,
    pub ignored_version: String,
    pub output_language: String, // "auto", "zh", "en", etc.
    pub enable_streaming: bool, // 是否启用流式输出
    pub streaming_interval: u64, // 流式处理时间间隔（毫秒）
    pub trigger_mode: String, // 触发模式: "hold" 或 "toggle"
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            api_url: "https://api.siliconflow.cn/v1/audio/transcriptions".to_string(),
            model_name: "FunAudioLLM/SenseVoiceSmall".to_string(),
            allow_emoji: true, // 默认开启
            allow_punctuation: true, // 默认开启
            show_log: false, // 默认关闭
            language: "zh".to_string(), // 默认中文 (Interface)
            output_mode: "clipboard".to_string(),
            autostart: false,
            hotkey: 0x71, // 默认 F2
            enable_indicator: true,
            last_check_time: 0,
            ignored_version: String::new(),
            output_language: "auto".to_string(),
            enable_streaming: false, // 默认关闭流式输出
            streaming_interval: 2000, // 默认 2000 毫秒
            trigger_mode: "hold".to_string(), // 默认按住输入模式
        }
    }
}

impl ConfigManager {
    pub fn new() -> Self {
        // 尝试确定合适的配置目录
        // 1. 本地目录 (便携模式)
        // 2. Roaming AppData
        let mut path = PathBuf::from("voice2type_config.json");
        if !path.exists() {
            if let Some(proj_dirs) = directories::ProjectDirs::from("com", "guchang233", "voice2type") {
                let config_dir = proj_dirs.config_dir();
                if !config_dir.exists() {
                    let _ = fs::create_dir_all(config_dir);
                }
                path = config_dir.join("settings.json");
            }
        }

        let mut config = AppConfig::default();

        // 尝试加载现有配置
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(loaded) = serde_json::from_str::<AppConfig>(&content) {
                    config = loaded;
                }
            }
        } else {
            // 如果为空，尝试从 .env 加载 (迁移/首次运行)
            if let Ok(key) = std::env::var("SILICONFLOW_API_KEY") {
                config.api_key = key;
            }
        }

        Self {
            config: Arc::new(Mutex::new(config)),
            config_path: path,
        }
    }

    pub fn get_api_key(&self) -> String {
        self.config.lock().unwrap().api_key.clone()
    }

    pub fn set_api_key(&self, key: String) {
        self.config.lock().unwrap().api_key = key;
    }

    pub fn get_api_url(&self) -> String {
        self.config.lock().unwrap().api_url.clone()
    }

    pub fn set_api_url(&self, url: String) {
        self.config.lock().unwrap().api_url = url;
    }

    pub fn get_model_name(&self) -> String {
        self.config.lock().unwrap().model_name.clone()
    }

    pub fn set_model_name(&self, model: String) {
        self.config.lock().unwrap().model_name = model;
    }

    pub fn allow_emoji(&self) -> bool {
        self.config.lock().unwrap().allow_emoji
    }

    pub fn set_allow_emoji(&self, allow: bool) {
        self.config.lock().unwrap().allow_emoji = allow;
    }

    pub fn allow_punctuation(&self) -> bool {
        self.config.lock().unwrap().allow_punctuation
    }

    pub fn set_allow_punctuation(&self, allow: bool) {
        self.config.lock().unwrap().allow_punctuation = allow;
    }

    pub fn show_log(&self) -> bool {
        self.config.lock().unwrap().show_log
    }

    pub fn set_show_log(&self, show: bool) {
        self.config.lock().unwrap().show_log = show;
    }

    pub fn language(&self) -> String {
        self.config.lock().unwrap().language.clone()
    }

    pub fn set_language(&self, lang: String) {
        self.config.lock().unwrap().language = lang;
    }

    pub fn output_mode(&self) -> String {
        self.config.lock().unwrap().output_mode.clone()
    }

    pub fn set_output_mode(&self, mode: String) {
        self.config.lock().unwrap().output_mode = mode;
    }

    pub fn autostart_enabled(&self) -> bool {
        self.config.lock().unwrap().autostart
    }

    pub fn set_autostart_enabled(&self, enabled: bool) {
        self.config.lock().unwrap().autostart = enabled;
    }

    pub fn hotkey(&self) -> u32 {
        self.config.lock().unwrap().hotkey
    }

    pub fn set_hotkey(&self, vk: u32) {
        self.config.lock().unwrap().hotkey = vk;
    }

    pub fn enable_indicator(&self) -> bool {
        self.config.lock().unwrap().enable_indicator
    }

    pub fn set_enable_indicator(&self, enable: bool) {
        self.config.lock().unwrap().enable_indicator = enable;
    }

    pub fn last_check_time(&self) -> u64 {
        self.config.lock().unwrap().last_check_time
    }

    pub fn set_last_check_time(&self, time: u64) {
        self.config.lock().unwrap().last_check_time = time;
    }

    pub fn ignored_version(&self) -> String {
        self.config.lock().unwrap().ignored_version.clone()
    }

    pub fn set_ignored_version(&self, version: String) {
        self.config.lock().unwrap().ignored_version = version;
    }

    pub fn output_language(&self) -> String {
        self.config.lock().unwrap().output_language.clone()
    }

    pub fn set_output_language(&self, lang: String) {
        self.config.lock().unwrap().output_language = lang;
    }

    pub fn enable_streaming(&self) -> bool {
        self.config.lock().unwrap().enable_streaming
    }

    pub fn set_enable_streaming(&self, enable: bool) {
        self.config.lock().unwrap().enable_streaming = enable;
    }

    pub fn streaming_interval(&self) -> u64 {
        self.config.lock().unwrap().streaming_interval
    }

    pub fn set_streaming_interval(&self, interval: u64) {
        self.config.lock().unwrap().streaming_interval = interval;
    }

    pub fn trigger_mode(&self) -> String {
        self.config.lock().unwrap().trigger_mode.clone()
    }

    pub fn set_trigger_mode(&self, mode: String) {
        self.config.lock().unwrap().trigger_mode = mode;
    }

    pub fn reset_ai_config(&self) {
        let mut cfg = self.config.lock().unwrap();
        cfg.api_key = AppConfig::default().api_key;
        cfg.api_url = AppConfig::default().api_url;
        cfg.model_name = AppConfig::default().model_name;
        cfg.enable_streaming = AppConfig::default().enable_streaming;
        cfg.streaming_interval = AppConfig::default().streaming_interval;
        cfg.trigger_mode = AppConfig::default().trigger_mode;
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config = self.config.lock().unwrap();
        let json = serde_json::to_string_pretty(&*config)?;
        fs::write(&self.config_path, json)?;
        Ok(())
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_path.clone()
    }

    pub fn log_dir(&self) -> PathBuf {
        let mut p = self.config_path.clone();
        p.pop(); // 移除文件名
        p.join("logs")
    }

    pub fn log_file_path(&self) -> PathBuf {
        self.log_dir().join("app.log")
    }
}
