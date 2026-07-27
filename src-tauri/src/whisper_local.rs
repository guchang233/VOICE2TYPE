use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

pub struct LocalWhisperEngine {
    model_path: PathBuf,
    model_loaded: bool,
}

impl LocalWhisperEngine {
    pub fn new(model_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&model_dir).ok();
        Self {
            model_path: model_dir.join("ggml-base.bin"),
            model_loaded: false,
        }
    }

    pub fn is_model_available(&self) -> bool {
        self.model_path.exists()
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    pub fn set_model_path(&mut self, path: PathBuf) {
        self.model_path = path;
        self.model_loaded = false;
    }

    pub fn load_model(&mut self) -> Result<()> {
        if self.model_loaded {
            return Ok(());
        }
        if !self.model_path.exists() {
            bail!("Whisper model not found at {:?}. Please download a model first.", self.model_path);
        }
        self.model_loaded = true;
        Ok(())
    }

    pub fn transcribe(&mut self, _audio_data: &[f32], _language: Option<&str>) -> Result<String> {
        bail!(
            "本地 Whisper 引擎暂未启用。\n\
            whisper-rs 需要 cmake 和 C++ 编译环境（Visual Studio Build Tools）。\n\
            安装这些工具后，在 Cargo.toml 中取消注释 whisper-rs 依赖并重新编译即可启用内置引擎。\n\
            \n\
            当前可用功能：模型自动下载。模型位置：{}",
            self.model_path.display()
        )
    }

    pub fn transcribe_i16(&mut self, _samples_i16: &[i16], _language: Option<&str>) -> Result<String> {
        self.transcribe(&[], _language)
    }
}
