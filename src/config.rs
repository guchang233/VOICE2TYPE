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
pub struct AppConfig {
    pub api_key: String,
    pub allow_emoji: bool,
    pub allow_punctuation: bool,
    pub show_log: bool,
    pub language: String, // "zh" 或 "en"
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            allow_emoji: true, // 默认开启
            allow_punctuation: true, // 默认开启
            show_log: false, // 默认关闭
            language: "zh".to_string(), // 默认中文
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

    pub fn save(&self) -> anyhow::Result<()> {
        let config = self.config.lock().unwrap();
        let json = serde_json::to_string_pretty(&*config)?;
        fs::write(&self.config_path, json)?;
        Ok(())
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_path.clone()
    }
}
