//! ffmpeg 定位、按需下载与子进程封装。
//!
//! 查找顺序：系统 PATH → `<models_dir>/ffmpeg-bin/` → 触发下载。
//! 下载沿用项目内 whisper 引擎的模式：固定版本 zip → 多源回退 → 大小校验 →
//! 解压提取 exe → 运行 `-version` 验证。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use tauri::Emitter;

/// 固定版本的 ffmpeg essentials 构建（含 ffmpeg.exe / ffprobe.exe）
const FFMPEG_ZIP_PATH: &str = "7.1.1/ffmpeg-7.1.1-essentials_build.zip";
/// zip 预期大小（字节），用于 ±10% 校验，防止代理返回错误页面
const FFMPEG_ZIP_EXPECTED_SIZE: u64 = 92_234_348;

/// 下载源列表（依次回退）：GitHub 直连 + 加速镜像
const FFMPEG_ZIP_URLS: &[&str] = &[
    "https://github.com/GyanD/codexffmpeg/releases/download/",
    "https://gh-proxy.com/https://github.com/GyanD/codexffmpeg/releases/download/",
    "https://ghproxy.net/https://github.com/GyanD/codexffmpeg/releases/download/",
];

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hidden_command(program: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 在 PATH 中查找可执行文件（Windows 使用 where，类 Unix 使用 which）
fn find_in_path(name: &str) -> Option<PathBuf> {
    let tool = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    let mut cmd = std::process::Command::new(tool);
    cmd.arg(name);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = stdout.lines().map(|l| l.trim()).find(|l| !l.is_empty())?;
    let p = PathBuf::from(first);
    p.is_file().then_some(p)
}

pub fn ffmpeg_dir(models_dir: &str) -> PathBuf {
    Path::new(models_dir).join("ffmpeg-bin")
}

pub fn ffmpeg_in_dir(dir: &Path) -> PathBuf {
    dir.join(if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    })
}

/// 定位 ffmpeg：PATH → models 目录。找不到返回 None（由调用方决定是否下载）。
pub fn locate_ffmpeg(models_dir: &str) -> Option<PathBuf> {
    if let Some(p) = find_in_path("ffmpeg") {
        log::info!("[dubbing] 使用系统 PATH 中的 ffmpeg: {}", p.display());
        return Some(p);
    }
    let local = ffmpeg_in_dir(&ffmpeg_dir(models_dir));
    if local.is_file() {
        log::info!("[dubbing] 使用本地 ffmpeg: {}", local.display());
        return Some(local);
    }
    None
}

/// 确保 ffmpeg 可用；不存在时自动下载并解压到 models 目录。
/// `app` 用于发送 `dubbing-progress` 下载进度事件。
pub async fn ensure_ffmpeg(models_dir: &str, app: Option<&tauri::AppHandle>) -> Result<PathBuf> {
    if let Some(p) = locate_ffmpeg(models_dir) {
        return Ok(p);
    }

    let dir = ffmpeg_dir(models_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .context("创建 ffmpeg 目录失败")?;

    let target = ffmpeg_in_dir(&dir);
    log::info!("[dubbing] 未找到 ffmpeg，开始下载到 {}", target.display());

    let tmp_zip = dir.join(format!("ffmpeg-{}.zip", uuid::Uuid::new_v4()));
    let mut errors = Vec::new();
    let mut downloaded = false;

    for base in FFMPEG_ZIP_URLS {
        // 用户取消则停止尝试下一个下载源
        if super::pipeline::is_cancel_requested() {
            return Err(anyhow!("__cancelled__"));
        }
        let url = format!("{}{}", base, FFMPEG_ZIP_PATH);
        log::info!("[dubbing] 尝试下载 ffmpeg: {}", url);
        let tmp = tmp_zip.clone();
        let app2 = app.cloned();
        let res = tokio::task::spawn_blocking(move || {
            crate::update::download_file_any(&url, &tmp, |cur, total| {
                if let (Some(app), true) = (&app2, total > 0) {
                    let pct = (cur as f64 / total as f64 * 100.0).min(100.0);
                    let _ = app.emit(
                        "dubbing-progress",
                        serde_json::json!({
                            "stage": "prepare",
                            "label": "准备工具",
                            "status": "running",
                            "percent": 0,
                            "message": format!("正在下载 ffmpeg 引擎 {:.1}%（约 {} MB）", pct, total / 1024 / 1024),
                        }),
                    );
                }
            })
        })
        .await
        .map_err(|e| anyhow!("下载任务执行失败: {}", e))
        .and_then(|r| r);

        match res {
            Ok(()) => {
                // 大小校验（-50% ~ +10%，防止代理返回错误内容）
                let meta = tokio::fs::metadata(&tmp_zip).await?;
                let size = meta.len();
                let size_valid = (FFMPEG_ZIP_EXPECTED_SIZE / 2
                    ..=FFMPEG_ZIP_EXPECTED_SIZE * 11 / 10)
                    .contains(&size);
                if !size_valid {
                    let msg = format!(
                        "文件大小异常（{} bytes，预期约 {}）",
                        size, FFMPEG_ZIP_EXPECTED_SIZE
                    );
                    log::warn!("[dubbing] {}", msg);
                    errors.push(msg);
                    let _ = tokio::fs::remove_file(&tmp_zip).await;
                    continue;
                }
                downloaded = true;
                break;
            }
            Err(e) => {
                log::warn!("[dubbing] 下载源失败: {}", e);
                errors.push(format!(
                    "{}: {}",
                    base.trim_start_matches("https://")
                        .split('/')
                        .next()
                        .unwrap_or(base),
                    e
                ));
                let _ = tokio::fs::remove_file(&tmp_zip).await;
            }
        }
    }

    if !downloaded {
        return Err(anyhow!(
            "ffmpeg 下载失败，所有源均不可用：\n{}",
            errors.join("\n")
        ));
    }

    // 解压：仅提取 zip 内的 .exe（bin/ffmpeg.exe、bin/ffprobe.exe 等），拍平存放
    let tmp = tmp_zip.clone();
    let dir_clone = dir.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::open(&tmp)?;
        let mut archive = zip::ZipArchive::new(file)?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            if name.ends_with('/') || !name.to_lowercase().ends_with(".exe") {
                continue;
            }
            let Some(fname) = Path::new(&name).file_name() else {
                continue;
            };
            let out_path = dir_clone.join(fname);
            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
            log::info!("[dubbing] 解压 {} -> {}", name, out_path.display());
        }
        Ok(())
    })
    .await
    .map_err(|e| anyhow!("解压任务执行失败: {}", e))??;

    let _ = tokio::fs::remove_file(&tmp_zip).await;

    if !target.is_file() {
        return Err(anyhow!("解压完成但未找到 ffmpeg.exe"));
    }

    // 运行 -version 验证可用性
    verify_ffmpeg(&target)?;

    if let Some(app) = app {
        let _ = app.emit(
            "dubbing-progress",
            serde_json::json!({
                "stage": "prepare",
                "label": "准备工具",
                "status": "running",
                "percent": 0,
                "message": "ffmpeg 就绪",
            }),
        );
    }
    Ok(target)
}

/// 运行 `ffmpeg -version` 验证二进制可用
pub fn verify_ffmpeg(path: &Path) -> Result<String> {
    let out = hidden_command(path)
        .arg("-version")
        .output()
        .context("无法启动 ffmpeg")?;
    if !out.status.success() {
        return Err(anyhow!("ffmpeg -version 执行失败: {:?}", out.status.code()));
    }
    let first_line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    Ok(first_line)
}

/// 解析 ffmpeg stderr 中 `Duration: HH:MM:SS.xx`
pub fn parse_duration_line(stderr: &str) -> Option<f64> {
    let line = stderr.lines().find(|l| l.contains("Duration:"))?;
    let idx = line.find("Duration:")?;
    let rest = line[idx + "Duration:".len()..].trim();
    let end = rest.find(',').unwrap_or(rest.len());
    let dur = rest[..end].trim();
    let parts: Vec<&str> = dur.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].trim().parse().ok()?;
    let m: f64 = parts[1].trim().parse().ok()?;
    let s: f64 = parts[2].trim().parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// 探测视频总时长（秒）
pub fn probe_duration(ffmpeg: &Path, video: &Path) -> Result<f64> {
    let out = hidden_command(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(video)
        .output()
        .context("探测视频时长失败")?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    parse_duration_line(&stderr).ok_or_else(|| anyhow!("无法解析视频时长，可能不是有效的视频文件"))
}

/// 提取音轨并分块压缩为 mp3（16kHz 单声道 64kbps），返回生成的分块路径列表。
pub fn extract_audio_chunks(
    ffmpeg: &Path,
    video: &Path,
    out_dir: &Path,
    chunk_seconds: u64,
) -> Result<Vec<PathBuf>> {
    let pattern = out_dir.join("chunk_%04d.mp3");
    let out = hidden_command(ffmpeg)
        .args(["-y", "-hide_banner", "-i"])
        .arg(video)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "64k",
            "-f",
            "segment",
            "-segment_time",
            &chunk_seconds.to_string(),
            "-reset_timestamps",
            "1",
        ])
        .arg(&pattern)
        .output()
        .context("启动 ffmpeg 提取音轨失败")?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("does not contain any stream")
            || stderr.contains("Output file #0 does not contain any stream")
            || stderr.contains("Stream map 'a' matches no streams")
        {
            return Err(anyhow!("该视频没有音频轨道，无需配音"));
        }
        return Err(anyhow!("提取音轨失败: {}", last_stderr_lines(&stderr)));
    }

    let mut chunks: Vec<PathBuf> = std::fs::read_dir(out_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mp3"))
                .unwrap_or(false)
        })
        .collect();
    chunks.sort();
    if chunks.is_empty() {
        return Err(anyhow!("音轨提取结果为空，该视频可能没有音频轨道"));
    }
    Ok(chunks)
}

/// 将配音音轨替换进视频（先尝试视频流直拷贝，失败时降级为全量重编码）
pub fn mux_replace_audio(
    ffmpeg: &Path,
    video: &Path,
    audio_wav: &Path,
    output: &Path,
) -> Result<()> {
    // 第一次尝试：-c:v copy（最快，绝大多数 mp4/mkv(h264) 可行）
    let out = hidden_command(ffmpeg)
        .args(["-y", "-hide_banner", "-i"])
        .arg(video)
        .args(["-i"])
        .arg(audio_wav)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-movflags",
            "+faststart",
        ])
        .arg(output)
        .output()
        .context("启动 ffmpeg 混流失败")?;

    if out.status.success() && output.is_file() {
        return Ok(());
    }
    let err1 = String::from_utf8_lossy(&out.stderr).to_string();
    log::warn!(
        "[dubbing] 视频流直拷贝混流失败，改用重编码: {}",
        last_stderr_lines(&err1)
    );

    // 兜底：视频重编码（兼容 vp9/avi 等无法放入 mp4 容器的编码）
    let out = hidden_command(ffmpeg)
        .args(["-y", "-hide_banner", "-i"])
        .arg(video)
        .args(["-i"])
        .arg(audio_wav)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-movflags",
            "+faststart",
        ])
        .arg(output)
        .output()
        .context("启动 ffmpeg 重编码失败")?;

    if out.status.success() && output.is_file() {
        return Ok(());
    }
    Err(anyhow!(
        "混流失败: {}",
        last_stderr_lines(&String::from_utf8_lossy(&out.stderr))
    ))
}

/// 构建 atempo 滤镜链：单级取值限 [0.5, 2.0]，超出范围自动级联多级。
/// `tempo` = 原时长 / 目标时长（>1 压缩加速，<1 拉伸减速，音调不变）。
pub fn atempo_filter_chain(tempo: f64) -> String {
    let mut t = tempo.clamp(0.05, 20.0);
    let mut parts = Vec::new();
    while t > 2.0 + 1e-9 {
        parts.push("atempo=2.0".to_string());
        t /= 2.0;
    }
    while t < 0.5 - 1e-9 {
        parts.push("atempo=0.5".to_string());
        t /= 0.5;
    }
    parts.push(format!("atempo={:.4}", t));
    parts.join(",")
}

/// 用 atempo 变时基（不变调）把 WAV 精确拉伸/压缩，用于配音段贴合原时长。
/// 输出统一为 24kHz 单声道 s16，与配音音轨规格一致。
pub fn time_stretch_wav(ffmpeg: &Path, src: &Path, dst: &Path, tempo: f64) -> Result<()> {
    let filter = atempo_filter_chain(tempo);
    let ar = super::tts_segments::TRACK_SAMPLE_RATE.to_string();
    let out = hidden_command(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-filter:a", &filter, "-ac", "1", "-ar", &ar])
        .arg(dst)
        .output()
        .context("启动 ffmpeg 变速失败")?;
    if !out.status.success() || !dst.is_file() {
        return Err(anyhow!(
            "atempo 变速失败（tempo={:.4}）: {}",
            tempo,
            last_stderr_lines(&String::from_utf8_lossy(&out.stderr))
        ));
    }
    Ok(())
}

fn last_stderr_lines(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().rev().take(6).collect();
    lines
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ")
        .chars()
        .take(500)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parse() {
        assert_eq!(
            parse_duration_line("  Duration: 00:01:30.50, start: 0.000000, bitrate: 123 kb/s"),
            Some(90.5)
        );
        assert_eq!(parse_duration_line("Duration: 01:00:00.00"), Some(3600.0));
        assert_eq!(parse_duration_line("no duration here"), None);
    }

    #[test]
    fn atempo_chain_build() {
        assert_eq!(atempo_filter_chain(1.0), "atempo=1.0000");
        assert_eq!(atempo_filter_chain(1.5), "atempo=1.5000");
        // 超 2.0 级联：3.0 = 2.0 * 1.5
        assert_eq!(atempo_filter_chain(3.0), "atempo=2.0,atempo=1.5000");
        // 低于 0.5 级联：0.3 = 0.5 * 0.6
        assert_eq!(atempo_filter_chain(0.3), "atempo=0.5,atempo=0.6000");
        // 极端值被限幅：8.0 = 2*2*2
        assert_eq!(atempo_filter_chain(8.0), "atempo=2.0,atempo=2.0,atempo=2.0000");
    }
}
