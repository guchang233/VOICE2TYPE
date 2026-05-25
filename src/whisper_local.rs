//! 本地 whisper.cpp（whisper-cli）集成，根目录由用户在设置中指定。
//!
//! 用户选择的根目录下应有：
//! ```text
//! {用户目录}/
//!   bin/whisper-cli.exe   # 或 main.exe
//!   models/ggml-base.bin
//!   tmp/                  # 临时 WAV（运行时自动创建）
//!   README.txt
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use crate::config::ConfigManager;

const BIN_CANDIDATES: &[&str] = &["whisper-cli.exe", "main.exe", "whisper.exe"];
const DEFAULT_MODEL: &str = "ggml-base.bin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalWhisperStatus {
    NotConfigured,
    Ready,
    MissingExecutable,
    MissingModel,
    InvalidDirectory,
}

pub struct LocalWhisper;

impl LocalWhisper {
    pub fn root_dir(config: &ConfigManager) -> PathBuf {
        PathBuf::from(config.local_whisper_dir())
    }

    pub fn bin_dir(root: &Path) -> PathBuf {
        root.join("bin")
    }

    pub fn models_dir(root: &Path) -> PathBuf {
        root.join("models")
    }

    pub fn tmp_dir(root: &Path) -> PathBuf {
        root.join("tmp")
    }

    pub fn ensure_layout(config: &ConfigManager) -> Result<()> {
        let root = Self::root_dir(config);
        if root.as_os_str().is_empty() {
            bail!("尚未设置本地 Whisper 目录");
        }
        if !root.is_dir() {
            bail!("Whisper 目录不存在: {}", root.display());
        }
        for dir in [
            Self::bin_dir(&root),
            Self::models_dir(&root),
            Self::tmp_dir(&root),
        ] {
            fs::create_dir_all(&dir).with_context(|| format!("创建目录失败: {}", dir.display()))?;
        }
        let readme = root.join("README.txt");
        if !readme.exists() {
            fs::write(&readme, README_CONTENT).context("写入 Whisper README 失败")?;
        }
        Ok(())
    }

    pub fn find_executable(root: &Path) -> Option<PathBuf> {
        let bin = Self::bin_dir(root);
        for name in BIN_CANDIDATES {
            let path = bin.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    pub fn model_file_name(config: &ConfigManager) -> String {
        let name = config.local_whisper_model();
        if name.is_empty() {
            DEFAULT_MODEL.to_string()
        } else {
            name
        }
    }

    pub fn find_model(root: &Path, model_file: &str) -> Option<PathBuf> {
        let path = root.join("models").join(model_file);
        if path.is_file() {
            Some(path)
        } else {
            None
        }
    }

    pub fn status(config: &ConfigManager) -> LocalWhisperStatus {
        let root = Self::root_dir(config);
        if root.as_os_str().is_empty() {
            return LocalWhisperStatus::NotConfigured;
        }
        if !root.is_dir() {
            return LocalWhisperStatus::InvalidDirectory;
        }
        if Self::find_executable(&root).is_none() {
            return LocalWhisperStatus::MissingExecutable;
        }
        let model = Self::model_file_name(config);
        if Self::find_model(&root, &model).is_none() {
            return LocalWhisperStatus::MissingModel;
        }
        LocalWhisperStatus::Ready
    }

    pub fn status_message(config: &ConfigManager) -> String {
        let root = Self::root_dir(config);
        match Self::status(config) {
            LocalWhisperStatus::NotConfigured => {
                "尚未设置本地 Whisper 目录。\n请在托盘菜单「设置 → 配置 → 设置本地 Whisper 目录」中选择安装路径。".to_string()
            }
            LocalWhisperStatus::Ready => format!(
                "本地 Whisper 已就绪。\n目录: {}",
                root.display()
            ),
            LocalWhisperStatus::InvalidDirectory => format!(
                "Whisper 目录无效或不存在:\n{}\n请重新选择目录。",
                root.display()
            ),
            LocalWhisperStatus::MissingExecutable => format!(
                "未找到 whisper 可执行文件。\n请将 whisper-cli.exe（或 main.exe）放入:\n{}\n\n详见该目录下 README.txt",
                Self::bin_dir(&root).display()
            ),
            LocalWhisperStatus::MissingModel => format!(
                "未找到模型文件「{}」。\n请下载 ggml 模型并放入:\n{}\n\n详见 README.txt",
                Self::model_file_name(config),
                Self::models_dir(&root).display()
            ),
        }
    }

    /// 同步转写（在 `spawn_blocking` 中调用）。
    pub fn transcribe_sync(wav_data: &[u8], config: &ConfigManager) -> Result<String> {
        Self::ensure_layout(config)?;

        let root = Self::root_dir(config);
        let exe = Self::find_executable(&root)
            .ok_or_else(|| anyhow::anyhow!("未找到 whisper 可执行文件"))?;
        let model_name = Self::model_file_name(config);
        let model_path = Self::find_model(&root, &model_name)
            .ok_or_else(|| anyhow::anyhow!("未找到模型文件: {}", model_name))?;

        let tmp = Self::tmp_dir(&root);
        fs::create_dir_all(&tmp)?;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let wav_path = tmp.join(format!("recording_{}.wav", stamp));
        let out_prefix = tmp.join(format!("recording_{}", stamp));
        let txt_path = PathBuf::from(format!("{}.txt", out_prefix.display()));

        fs::write(&wav_path, wav_data).context("写入临时 WAV 失败")?;

        let mut cmd = Command::new(&exe);
        cmd.stdin(std::process::Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        cmd.arg("-m").arg(&model_path);
        cmd.arg("-f").arg(&wav_path);
        cmd.arg("-otxt");
        cmd.arg("-of").arg(&out_prefix);
        cmd.arg("-nt");

        let lang = whisper_language_arg(config.output_language().as_str());
        if let Some(code) = lang {
            cmd.arg("-l").arg(code);
        }

        cmd.arg("-np");

        let output = cmd
            .output()
            .with_context(|| format!("启动本地 Whisper 失败: {}", exe.display()))?;

        let _ = fs::remove_file(&wav_path);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!(
                "本地 Whisper 退出码 {:?}\nstdout: {}\nstderr: {}",
                output.status.code(),
                stdout.trim(),
                stderr.trim()
            );
        }

        let text = if txt_path.is_file() {
            fs::read_to_string(&txt_path).context("读取 Whisper 输出文本失败")?
        } else {
            String::from_utf8_lossy(&output.stdout).into_owned()
        };

        let _ = fs::remove_file(&txt_path);

        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            bail!("本地 Whisper 未返回识别文本");
        }
        Ok(trimmed)
    }
}

fn whisper_language_arg(output_language: &str) -> Option<&'static str> {
    match output_language {
        "zh" => Some("zh"),
        "en" => Some("en"),
        "auto" | _ => None,
    }
}

const README_CONTENT: &str = r#"Voice2Type 本地 Whisper 目录
================================

将 whisper.cpp 的 Windows 构建产物放入本目录后即可离线转写。

目录结构：
  bin\whisper-cli.exe   （或 main.exe）
  models\ggml-base.bin  （或其它 ggml-*.bin 模型）

获取 whisper-cli：
  1. 打开 https://github.com/ggml-org/whisper.cpp/releases
  2. 下载 Windows 预编译包，或自行编译 examples/cli
  3. 将 whisper-cli.exe 复制到 bin\ 目录

获取模型：
  1. 打开 https://huggingface.co/ggerganov/whisper.cpp/tree/main
  2. 下载 ggml-base.bin（或 tiny/small/medium 等）
  3. 放入 models\ 目录

在托盘菜单选择「本地 Whisper（离线）」作为识别模型。
目录路径在「设置本地 Whisper 目录」中配置。
模型文件名可在 settings.json 的 model.local_whisper_model 修改。
"#;
