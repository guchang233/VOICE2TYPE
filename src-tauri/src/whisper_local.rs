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

    /// 用最新的模型目录刷新引擎内部所有路径（模型目录、二进制路径、模型路径）。
    /// 必须在每次转写前调用，因为用户可能在启动后修改了自定义模型目录，
    /// 导致启动时捕获的 model_dir 已过期。
    pub fn refresh_paths(&mut self, model_dir: PathBuf, model_name: &str) {
        self.binary_path = model_dir
            .join("whisper-bin")
            .join(if cfg!(target_os = "windows") {
                "whisper-cli.exe"
            } else {
                "whisper-cli"
            });
        self.model_dir = model_dir.clone();
        // model_name 为空时回退到默认 base 模型，避免 join 出目录本身
        let name = if model_name.is_empty() {
            "ggml-base.bin"
        } else {
            model_name
        };
        self.model_path = model_dir.join(name);
    }

    /// 关联函数：不持有引擎锁即可调用转写。
    /// 将音频写入临时 WAV 文件，通过文件路径调用 whisper.cpp 二进制，解析 stdout 获取文本。
    /// 不使用 stdin 模式，因为 whisper-cli 对 stdin WAV 的支持在某些版本下不稳定，
    /// 文件路径方式更可靠且便于诊断。
    pub async fn transcribe_at(
        binary_path: &Path,
        model_path: &Path,
        samples: &[i16],
        language: Option<&str>,
    ) -> Result<String> {
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
        let temp_dir = std::env::temp_dir();
        let temp_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let wav_path = temp_dir.join(format!("v2t_whisper_{}.wav", temp_id));

        // 在阻塞线程中写入 WAV 文件
        let wav_path_clone = wav_path.clone();
        let samples_vec = samples.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&wav_path_clone, spec)?;
            for &sample in &samples_vec {
                writer.write_sample(sample)?;
            }
            writer.finalize()?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("WAV 写入任务失败: {}", e))??;

        // 构建线程数（上限 8）
        let thread_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8);

        // 构建 whisper-cli 命令（用文件路径而非 stdin）
        let mut cmd = tokio::process::Command::new(binary_path);
        cmd.arg("-m")
            .arg(model_path)
            .arg("-f")
            .arg(&wav_path)
            .arg("-nt")
            .arg("-np")
            .arg("-t")
            .arg(thread_count.to_string())
            .arg("-l")
            .arg(language.unwrap_or("auto"))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Windows 隐藏控制台窗口
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.as_std_mut().creation_flags(CREATE_NO_WINDOW);
        }

        log::info!(
            "[whisper] 启动转写: 引擎={}, 模型={}, WAV={}, 采样数={}, 语言={}",
            binary_path.display(),
            model_path.display(),
            wav_path.display(),
            samples.len(),
            language.unwrap_or("auto")
        );

        let output = cmd.output().await?;

        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let stderr_str = String::from_utf8_lossy(&output.stderr);

        log::info!(
            "[whisper] 转写结束: 退出码={:?}, stdout={}字节, stderr={}字节",
            output.status.code(),
            stdout_str.len(),
            stderr_str.len()
        );

        // 清理临时文件（失败也忽略）
        let _ = std::fs::remove_file(&wav_path);

        if !output.status.success() {
            bail!(
                "whisper.cpp 执行失败（退出码 {:?}）\nstdout: {}\nstderr: {}\n引擎路径: {}",
                output.status.code(),
                stdout_str.trim(),
                stderr_str.trim(),
                binary_path.display()
            );
        }

        // 解析 stdout：每行格式为 [HH:MM:SS.mmm --> HH:MM:SS.mmm]  <text>
        // 使用 -nt 后仍可能有时间戳前缀（取决于版本），统一剥离
        let text = parse_whisper_output(&stdout_str);

        // 转写结果为空时记录 stderr 辅助排查（如模型加载失败、无语音段）
        if text.is_empty() {
            let stderr_tail = stderr_str.trim();
            if stderr_tail.is_empty() {
                bail!("转写结果为空：whisper.cpp 未输出任何文本（可能未检测到语音，或音频太短/静音）");
            } else {
                bail!("转写结果为空。whisper.cpp stderr:\n{}", stderr_tail);
            }
        }

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
