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
            model_path: model_dir.join("ggml-tiny.bin"),
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
        // model_name 为空时回退到默认 tiny 模型，避免 join 出目录本身
        let name = if model_name.is_empty() {
            "ggml-tiny.bin"
        } else {
            model_name
        };
        self.model_path = model_dir.join(name);
    }

    /// 同步转写（在 spawn_blocking 中调用）。
    /// 使用 std::process::Command（同步），避免 tokio runtime 争用。
    /// 使用 -otxt 文件输出，匹配原始快速实现。
    /// 返回 (文本, 检测到的语言代码) —— 语言代码仅在 auto 检测时有值，用于缓存。
    ///
    /// 性能参数：
    /// - `threads`：whisper-cli `-t` 线程数；0 = 自动取物理核数（上限 8）
    /// - `greedy`：true 追加 `-bs 1 -bo 1` 贪婪解码（跳过 beam search 与候选重采样）
    /// - `no_fallback`：true 追加 `-nf` 关闭温度回退重试
    pub fn transcribe_sync(
        binary_path: &Path,
        model_path: &Path,
        samples: &[i16],
        language: Option<&str>,
        threads: u32,
        greedy: bool,
        no_fallback: bool,
    ) -> Result<(String, Option<String>)> {
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

        let total_start = std::time::Instant::now();

        // 写临时 WAV（16kHz mono 16-bit），用时间戳命名（匹配原始实现）
        let tmp_dir = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let wav_path = tmp_dir.join(format!("v2t_recording_{}.wav", stamp));
        let out_prefix = tmp_dir.join(format!("v2t_recording_{}", stamp));
        let txt_path = std::path::PathBuf::from(format!("{}.txt", out_prefix.display()));

        // 首尾静音裁剪：缩短推理音频长度，线性提速，构建无关（不依赖 --vad）
        let trim_start = std::time::Instant::now();
        let samples = trim_silence(samples, 0.01f32, 50);
        log::info!(
            "[whisper] 静音裁剪: {}ms, 采样数={}",
            trim_start.elapsed().as_millis(),
            samples.len()
        );

        let wav_start = std::time::Instant::now();
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
        log::info!(
            "[whisper] WAV 写入: {}ms, 采样数={}",
            wav_start.elapsed().as_millis(),
            samples.len()
        );

        // 构建命令 — 匹配原始快速实现
        let mut cmd = std::process::Command::new(binary_path);
        cmd.stdin(std::process::Stdio::null())
            .arg("-m")
            .arg(model_path)
            .arg("-f")
            .arg(&wav_path)
            .arg("-otxt")
            .arg("-of")
            .arg(&out_prefix)
            .arg("-nt")
            .arg("-np");

        // 消除 OpenBLAS 线程池启动开销：blas 二进制默认 fork 逻辑核数个线程，
        // 每次 spawn 都要付线程创建税（8核16线程机器 ~50-150ms）。
        // tiny 模型矩阵小，BLAS 多线程无收益；whisper 自己用 -t 控制 decode 线程。
        // 设为 1 让 BLAS 单线程，省掉线程池初始化。
        cmd.env("OPENBLAS_NUM_THREADS", "1");
        cmd.env("OMP_NUM_THREADS", "1");

        // 线程数：0=自动取物理核数（上限 8，whisper.cpp 超过 8 线程收益递减）
        let effective_threads = if threads == 0 {
            auto_thread_count()
        } else {
            threads.min(8)
        };
        cmd.arg("-t").arg(effective_threads.to_string());

        // 可选贪婪解码：跳过 beam search 与候选重采样，对短音频可省 30-50%
        if greedy {
            cmd.arg("-bs").arg("1").arg("-bo").arg("1");
        }
        // 可选关闭温度回退：跳过低置信重试
        if no_fallback {
            cmd.arg("-nf");
        }

        // 语言处理：
        // - 具体语言（zh/en）：传 -l <lang>，无检测开销
        // - "auto"：传 -l auto，触发语言检测（首次有 ~100-300ms 开销，
        //   由调用方缓存检测结果，后续用具体语言跳过检测）
        // - None/空：不传 -l，whisper-cli 默认 "en"
        let is_auto = language
            .map(|l| l == "auto")
            .unwrap_or(false);
        if let Some(lang) = language {
            if !lang.is_empty() {
                cmd.arg("-l").arg(lang);
            }
        }

        // Windows 隐藏控制台窗口
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let cmd_start = std::time::Instant::now();
        log::info!(
            "[whisper] 启动转写: 引擎={}, 模型={}, 语言={}, 线程={}",
            binary_path.display(),
            model_path.display(),
            language.unwrap_or("(default)"),
            effective_threads
        );

        // 用 spawn + 流式读 stderr，精确区分"启动税"（DLL加载+BLAS初始化+模型加载）
        // 与"推理"阶段。stdout 设 null（用 -otxt 文件输出），只 pipe stderr。
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());

        let spawn_t = std::time::Instant::now();
        let mut child = cmd.spawn()?;
        log::info!(
            "[whisper] 进程创建(CreateProcess): {}ms",
            spawn_t.elapsed().as_millis()
        );

        // 读 stderr：首行出现 = 进程已加载 DLL + 初始化完成并开始输出
        // 这个时间 ≈ 启动税（PE加载 + BLAS线程池初始化 + 模型加载到首条日志）
        let stderr = child.stderr.take().unwrap();
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stderr);
        let mut lines = reader.lines();

        let first_resp_t = std::time::Instant::now();
        let first_line = lines.next();
        log::info!(
            "[whisper] 启动税(到stderr首行): {}ms",
            first_resp_t.elapsed().as_millis()
        );

        // 收集剩余 stderr
        let mut stderr_str = String::new();
        if let Some(Ok(l)) = first_line {
            stderr_str.push_str(&l);
            stderr_str.push('\n');
        }
        for line in lines {
            if let Ok(l) = line {
                stderr_str.push_str(&l);
                stderr_str.push('\n');
            }
        }

        let status = child.wait()?;
        log::info!(
            "[whisper] 进程总耗时: {}ms (其中启动税见上, 退出码={:?})",
            cmd_start.elapsed().as_millis(),
            status.code()
        );

        // 清理临时 WAV
        let _ = std::fs::remove_file(&wav_path);

        if !status.success() {
            bail!(
                "whisper.cpp 执行失败（退出码 {:?}）\nstderr: {}\n引擎路径: {}",
                status.code(),
                stderr_str.trim(),
                binary_path.display()
            );
        }

        // 从 .txt 文件读取结果（stdout 已设 null，依赖 -otxt 文件输出）
        let text = if txt_path.is_file() {
            std::fs::read_to_string(&txt_path).unwrap_or_default()
        } else {
            String::new()
        };
        let _ = std::fs::remove_file(&txt_path);

        let text = text.trim().to_string();

        if text.is_empty() {
            let stderr_tail = stderr_str.trim();
            if stderr_tail.is_empty() {
                bail!("转写结果为空：whisper.cpp 未输出任何文本（可能未检测到语音，或音频太短/静音）");
            } else {
                bail!("转写结果为空。whisper.cpp stderr:\n{}", stderr_tail);
            }
        }

        // 解析 auto 检测到的语言（whisper-cli stderr 含 "language = xx" 或 "whisper_ctx_init..." ）
        let detected_lang = if is_auto {
            parse_detected_language(&stderr_str)
        } else {
            None
        };

        log::info!(
            "[whisper] 总耗时: {}ms, 检测语言={:?}",
            total_start.elapsed().as_millis(),
            detected_lang
        );

        Ok((text, detected_lang))
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

/// 从 whisper-cli stderr 解析自动检测到的语言代码
/// stderr 中含类似 "language: zh" 或 "whisper_auto_detect_language: language = zh"
fn parse_detected_language(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        let lower = line.to_lowercase();
        if lower.contains("language") {
            // 尝试提取 "language = xx" 或 "language: xx" 中的语言代码
            for sep in &["= ", ": "] {
                if let Some(pos) = lower.find(sep) {
                    let rest = &line[pos + sep.len()..];
                    let code: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric())
                        .collect();
                    let code = code.to_lowercase();
                    // whisper 语言代码为 2 字母（en/zh/ja/fr...）
                    if code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic()) {
                        return Some(code);
                    }
                }
            }
        }
    }
    None
}

/// 自动确定 whisper-cli 线程数。
/// 取 `std::thread::available_parallelism`（逻辑核数），保守折半近似物理核数
/// （超线程对 whisper.cpp 矩阵运算收益有限甚至负优化），上限钳制 8。
/// 取不到时回退 4（whisper-cli 默认）。
fn auto_thread_count() -> u32 {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let physical = (logical / 2).max(1);
    physical.min(8) as u32
}

/// 首尾静音裁剪：返回原切片的子切片，零拷贝。
/// - `threshold`：归一化振幅阈值（0.0-1.0），样本绝对值 > i16_MAX*threshold 视为有语音
/// - `min_pad_ms`：首尾各保留的最小缓冲毫秒数，避免吞字（按 16kHz 换算样本数）
///
/// 行为：
/// - 全静音（无样本超阈值）→ 返回原 samples（交给 whisper 判 no-speech）
/// - 裁剪后从起点向前、终点向后各扩展 min_pad_ms 缓冲，钳制到原范围
fn trim_silence(samples: &[i16], threshold: f32, min_pad_ms: u32) -> &[i16] {
    if samples.is_empty() {
        return samples;
    }
    let thr = (i16::MAX as f32 * threshold.clamp(0.0, 1.0)) as i32;
    // 找首个超阈值样本
    let first = samples.iter().position(|&s| (s as i32).abs() > thr);
    let first = match first {
        Some(i) => i,
        None => return samples, // 全静音
    };
    // 找末个超阈值样本
    let last = samples.iter().rposition(|&s| (s as i32).abs() > thr);
    let last = match last {
        Some(i) => i,
        None => return samples, // 理论不可达（first 已 Some）
    };
    // 首尾缓冲（16kHz：1ms = 16 样本）
    let pad = (min_pad_ms as usize * 16).min(samples.len());
    let start = first.saturating_sub(pad);
    let end = (last + 1 + pad).min(samples.len());
    &samples[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_silence_empty_input_returns_empty() {
        assert!(trim_silence(&[], 0.01, 50).is_empty());
    }

    #[test]
    fn trim_silence_all_silence_returns_original() {
        let samples = vec![0i16; 16000];
        let out = trim_silence(&samples, 0.01, 50);
        assert_eq!(out.len(), samples.len());
    }

    #[test]
    fn trim_silence_pure_speech_not_trimmed_much() {
        // 1 秒满幅正弦语音，首尾无静音
        let samples: Vec<i16> = (0..16000).map(|i| ((i as f32 * 0.1).sin() * 16384.0) as i16).collect();
        let out = trim_silence(&samples, 0.01, 50);
        // 起点应为 0（首样本已超阈值），长度接近原长
        assert_eq!(out.len(), samples.len());
    }

    #[test]
    fn trim_silence_leading_trailing_silence_trimmed() {
        // 500ms 静音 + 1 秒语音 + 500ms 静音
        let mut samples = vec![0i16; 8000];
        samples.extend((0..16000).map(|i| ((i as f32 * 0.1).sin() * 16384.0) as i16));
        samples.extend(vec![0i16; 8000]);
        let out = trim_silence(&samples, 0.01, 50);
        // 应裁掉大部分静音：原始 32000 → 应小于原长
        assert!(out.len() < 32000, "out.len()={} 应小于原长 32000", out.len());
        // 应保留完整语音段（~16000）+ 首尾各 50ms 缓冲（~1600），故 > 16000
        assert!(out.len() > 16000, "out.len()={} 应保留完整语音段+缓冲", out.len());
        // 精确上界：语音段 16000 + 双侧 pad 1600 = 17600，允许 ±2 容差
        assert!(out.len() <= 17602, "out.len()={} 不应超过语音段+双侧缓冲", out.len());
    }

    #[test]
    fn trim_silence_min_pad_respected() {
        // 单个语音样本在中间，前后静音
        let mut samples = vec![0i16; 16000];
        samples[8000] = 16384;
        let out = trim_silence(&samples, 0.01, 50);
        // 首尾各保留 ~50ms (800 样本) 缓冲
        // start = 8000 - 800 = 7200, end = 8001 + 800 = 8801
        assert_eq!(out.len(), 8801 - 7200);
    }

    #[test]
    fn auto_thread_count_returns_at_least_1_and_at_most_8() {
        let t = auto_thread_count();
        assert!(t >= 1);
        assert!(t <= 8);
    }
}
