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

    /// 根据配置中的模型名（如 "ggml-small.bin"）更新当前模型路径
    pub fn sync_model_path(&mut self, model_name: &str) {
        self.model_path = self.model_dir.join(model_name);
    }

    /// 关联函数：不持有引擎锁即可调用转写。
    /// 在内存中编码 WAV 并通过 stdin 传给 whisper.cpp 二进制，解析 stdout 获取文本。
    pub async fn transcribe_at(
        binary_path: &Path,
        model_path: &Path,
        samples: &[i16],
        language: Option<&str>,
    ) -> Result<String> {
        use std::process::Stdio;
        use tokio::io::AsyncWriteExt;

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

        // 在内存中编码 WAV（16kHz mono 16-bit）
        let wav_bytes = {
            let mut cursor = std::io::Cursor::new(Vec::new());
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
            for &sample in samples {
                writer.write_sample(sample)?;
            }
            writer.finalize()?;
            cursor.into_inner()
        };

        // 构建线程数（上限 8）
        let thread_count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8);

        // 构建 whisper-cli 命令
        let mut cmd = tokio::process::Command::new(binary_path);
        cmd.arg("-m")
            .arg(model_path)
            .arg("-f")
            .arg("-") // 从 stdin 读取
            .arg("-nt")
            .arg("-np")
            .arg("-t")
            .arg(thread_count.to_string()) // 线程数
            .arg("-l")
            .arg(language.unwrap_or("auto"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Windows 隐藏控制台窗口
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.as_std_mut().creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn()?;

        // 写入 WAV 数据到 stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&wav_bytes).await?;
            // stdin drop 时发送 EOF
        }

        let output = child.wait_with_output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!(
                "whisper.cpp 执行失败（退出码 {:?}）\nstdout: {}\nstderr: {}\n引擎路径: {}",
                output.status.code(),
                stdout.trim(),
                stderr.trim(),
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
