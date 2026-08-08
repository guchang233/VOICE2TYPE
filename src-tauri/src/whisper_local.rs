use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Whisper 模型官方 SHA256 哈希表（HuggingFace LFS）
pub const MODEL_SHA256: &[(&str, &str)] = &[
    ("ggml-tiny.bin", "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21"),
    ("ggml-base.bin", "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"),
    ("ggml-small.bin", "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"),
    ("ggml-medium.bin", "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208"),
];

/// 根据模型文件名获取预期的 SHA256 哈希值
pub fn expected_sha256(filename: &str) -> Option<&'static str> {
    MODEL_SHA256
        .iter()
        .find(|(name, _)| *name == filename)
        .map(|(_, hash)| *hash)
}

/// 二进制健康状态
#[derive(Debug, Clone, serde::Serialize)]
pub enum BinaryHealth {
    Ok,
    Missing,
    Corrupt(String),
}

pub struct LocalWhisperEngine {
    model_dir: PathBuf,
    model_path: PathBuf,
    binary_path: PathBuf,
}

impl LocalWhisperEngine {
    pub fn new(model_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&model_dir).ok();
        // 二进制目录是模型目录的子目录 whisper-bin：
        // model_dir (即 models_dir) 下直接放 ggml-*.bin
        // model_dir/whisper-bin/ 下放 whisper-cli.exe（v1.9.2 起 main.exe 已弃用）
        let binary_path = model_dir
            .join("whisper-bin")
            .join(if cfg!(target_os = "windows") {
                "whisper-cli.exe"
            } else {
                "whisper-cli"
            });
        Self {
            model_dir: model_dir.clone(),
            model_path: model_dir.join("ggml-base.bin"),
            binary_path,
        }
    }

    pub fn is_model_available(&self) -> bool {
        self.model_path.exists()
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    /// 返回二进制路径的克隆（用于在不持有锁的情况下调用转写）
    pub fn binary_path_clone(&self) -> PathBuf {
        self.binary_path.clone()
    }

    /// 二进制是否已下载且可用
    pub fn is_binary_available(&self) -> bool {
        self.binary_path.exists()
    }

    /// 确保二进制可用，不存在则返回错误提示用户在设置中下载
    pub async fn ensure_binary(&self, _app: &tauri::AppHandle) -> Result<()> {
        if self.is_binary_available() {
            Ok(())
        } else {
            bail!("whisper.cpp 二进制未下载，请在设置中下载引擎")
        }
    }

    /// 检查二进制健康状态：存在、配套 DLL 是否齐全
    /// 注意：whisper-cli.exe 本身约 479KB，且依赖 whisper.dll/ggml.dll，
    /// 因此不能用 >500KB 的大小阈值判断完整性。
    pub fn check_binary_health(&self) -> BinaryHealth {
        if !self.binary_path.exists() {
            return BinaryHealth::Missing;
        }
        // Windows 下检查关键配套 DLL 是否存在（whisper.dll 是核心依赖）
        #[cfg(target_os = "windows")]
        {
            if let Some(dir) = self.binary_path.parent() {
                let whisper_dll = dir.join("whisper.dll");
                if !whisper_dll.exists() {
                    return BinaryHealth::Corrupt(
                        "缺少 whisper.dll，请重新下载引擎".to_string(),
                    );
                }
                let ggml_dll = dir.join("ggml.dll");
                if !ggml_dll.exists() {
                    return BinaryHealth::Corrupt(
                        "缺少 ggml.dll，请重新下载引擎".to_string(),
                    );
                }
            }
        }
        BinaryHealth::Ok
    }

    /// 根据配置中的模型名（如 "ggml-small.bin"）更新当前模型路径
    /// 若文件不存在则回退到目录中第一个可用的 ggml-*.bin
    pub fn sync_model_path(&mut self, model_name: &str) {
        let target = self.model_dir.join(model_name);
        if target.exists() {
            self.model_path = target;
            return;
        }
        // 回退：扫描目录中第一个 ggml-*.bin
        if let Ok(entries) = std::fs::read_dir(&self.model_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "bin").unwrap_or(false) {
                    self.model_path = path;
                    return;
                }
            }
        }
        // 仍保留默认值（可能不存在，后续转写会报错）
        self.model_path = target;
    }

    /// 实例方法：使用引擎当前的模型路径和二进制路径进行转写
    pub async fn transcribe_i16(
        &self,
        samples_i16: &[i16],
        language: Option<&str>,
    ) -> Result<String> {
        Self::transcribe_at(&self.binary_path, &self.model_path, samples_i16, language).await
    }

    /// 关联函数：不持有引擎锁即可调用转写。
    /// 将 i16 采样写入临时 WAV 文件，调用 whisper.cpp 二进制，解析 stdout 获取文本。
    pub async fn transcribe_at(
        binary_path: &Path,
        model_path: &Path,
        samples: &[i16],
        language: Option<&str>,
    ) -> Result<String> {
        use std::process::Stdio;

        if !binary_path.exists() {
            bail!(
                "whisper.cpp 二进制不存在: {}，请在设置中下载引擎",
                binary_path.display()
            );
        }
        if !model_path.exists() {
            bail!(
                "Whisper 模型不存在: {}，请先下载模型",
                model_path.display()
            );
        }

        // 写入临时 WAV 文件（16kHz mono 16-bit）
        let wav_path =
            std::env::temp_dir().join(format!("voice2type-{}.wav", uuid::Uuid::new_v4()));
        {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&wav_path, spec)?;
            for &sample in samples {
                writer.write_sample(sample)?;
            }
            writer.finalize()?;
        }

        // 构建 whisper.cpp 命令
        let mut cmd = tokio::process::Command::new(binary_path);
        cmd.arg("-m")
            .arg(model_path)
            .arg("-f")
            .arg(&wav_path)
            .arg("-nt")
            .arg("-np")
            .arg("-l")
            .arg(language.unwrap_or("auto"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Windows 隐藏控制台窗口
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.as_std_mut().creation_flags(CREATE_NO_WINDOW);
        }

        let output = cmd.output().await?;

        // 清理临时文件（尽力而为）
        let _ = std::fs::remove_file(&wav_path);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!(
                "whisper.cpp 执行失败（退出码 {:?}）: {}\nstdout: {}\n引擎路径: {}",
                output.status.code(),
                stderr.trim(),
                stdout.trim(),
                binary_path.display()
            );
        }

        // 解析 stdout：每行格式为 [HH:MM:SS.mmm --> HH:MM:SS.mmm]  <text>
        // 使用 -nt 后仍可能有时间戳前缀（取决于版本），统一剥离
        let stdout = String::from_utf8_lossy(&output.stdout);
        let text = parse_whisper_output(&stdout);

        Ok(text)
    }
}

/// 解析 whisper.cpp stdout 输出，提取转写文本
fn parse_whisper_output(stdout: &str) -> String {
    let mut lines = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // 剥离 [HH:MM:SS.mmm --> HH:MM:SS.mmm]  前缀
        let text = if let Some(close) = trimmed.find(']') {
            let after = &trimmed[close + 1..];
            after.trim_start().to_string()
        } else {
            // 无时间戳前缀，直接取整行
            trimmed.to_string()
        };
        if !text.is_empty() {
            lines.push(text);
        }
    }
    lines.join(" ")
}
