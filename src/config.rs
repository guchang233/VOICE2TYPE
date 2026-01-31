use anyhow::Result;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_key: String,
    pub allow_emoji: bool,
    pub allow_punctuation: bool,
    pub show_log: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            allow_emoji: false,
            allow_punctuation: true,
            show_log: false,
        }
    }
}

pub struct ConfigManager {
    config_path: PathBuf,
    pub config: Arc<Mutex<AppConfig>>,
}

impl ConfigManager {
    pub fn new() -> Self {
        let config_path = Self::get_config_path().unwrap_or_else(|| PathBuf::from("config.json"));
        let config = Self::load_from_path(&config_path).unwrap_or_default();
        
        // Try to load from .env if empty (migration/first run)
        let mut final_config = config;
        if final_config.api_key.is_empty() {
            if let Ok(key) = std::env::var("SILICONFLOW_API_KEY") {
                final_config.api_key = key;
            }
        }

        Self {
            config_path,
            config: Arc::new(Mutex::new(final_config)),
        }
    }

    fn get_config_path() -> Option<PathBuf> {
        ProjectDirs::from("com", "voice2type", "assistant")
            .map(|proj_dirs| proj_dirs.config_dir().join("settings.json"))
    }

    fn load_from_path(path: &PathBuf) -> Result<AppConfig> {
        if !path.exists() {
            return Ok(AppConfig::default());
        }
        let content = fs::read_to_string(path)?;
        let config = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let config = self.config.lock().unwrap();
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&*config)?;
        fs::write(&self.config_path, content)?;
        Ok(())
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
}
