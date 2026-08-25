//! 云端 ASR 带时间戳转写：将音频分块上传至 OpenAI 兼容转写接口，
//! 优先请求 `verbose_json`（含逐段时间戳），失败时回退 `srt` 自行解析。
//!
//! 复用整段识别的配置（ConfigManager）：provider/model/api_key/api_url/输出语言。
//! 音频按需从磁盘读取，避免整块驻留内存；回退重试时重新读取，峰值只持有一份。

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;
use crate::dubbing::DubSegment;

/// 转写单个音频分块文件，返回带时间戳分段（时间为分块内相对时间，毫秒）。
pub async fn transcribe_file(path: &Path, config: &ConfigManager) -> Result<Vec<DubSegment>> {
    let api_key = config.get_api_key();
    if api_key.is_empty() {
        return Err(anyhow!(
            "未配置云端识别 API Key：请在「设置 → API 密钥」中填写（本地 Whisper 暂不支持视频配音工作流）"
        ));
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("chunk.mp3")
        .to_string();
    let target = TranscribeTarget {
        url: api_url(config),
        api_key,
        model: model_name(config),
        service: config.get_speech_service(),
    };

    // 1) 优先 verbose_json
    {
        let audio = read_chunk(path).await?;
        match post_transcribe(&target, "verbose_json", audio, &file_name, config).await {
            Ok(body) => {
                if let Some(mut segs) = parse_verbose_json(&body).filter(|s| !s.is_empty()) {
                    finalize(&mut segs);
                    return Ok(segs);
                }
                // 有些服务 verbose_json 不带 segments，尝试 srt
                log::warn!("[dubbing] verbose_json 无分段数据，回退 srt 格式");
            }
            Err(e) => {
                log::warn!("[dubbing] verbose_json 请求失败（{}），回退 srt 格式", e);
            }
        }
    }

    // 2) 回退 srt（重新读文件，避免两份音频同时驻留）
    let audio = read_chunk(path).await?;
    let body = post_transcribe(&target, "srt", audio, &file_name, config)
        .await
        .map_err(|e| anyhow!("云端识别失败（verbose_json 与 srt 均不可用）: {}", e))?;

    let mut segs = parse_srt(&body);
    if segs.is_empty() {
        return Err(anyhow!(
            "无法从识别结果中解析出带时间戳的分段，当前提供商可能不支持时间戳输出，建议切换为 Groq (Whisper Large v3)"
        ));
    }
    finalize(&mut segs);
    Ok(segs)
}

async fn read_chunk(path: &Path) -> Result<Vec<u8>> {
    tokio::fs::read(path)
        .await
        .with_context(|| format!("读取音频分块失败: {}", path.display()))
}

fn api_url(config: &ConfigManager) -> String {
    if config.get_speech_service() == "groq" {
        "https://api.groq.com/openai/v1/audio/transcriptions".to_string()
    } else {
        config.get_api_url()
    }
}

/// 与 ConfigManager::get_model_name 保持一致：自定义提供商返回实际模型名，
/// 云端预置模型返回原始 ID（本地 Whisper 不适用于配音工作流，由调用方校验拦截）。
fn model_name(config: &ConfigManager) -> String {
    config.get_model_name()
}

/// 转写接口目标（provider 端点与鉴权信息）
struct TranscribeTarget {
    url: String,
    api_key: String,
    model: String,
    service: String,
}

async fn post_transcribe(
    target: &TranscribeTarget,
    response_format: &str,
    audio: Vec<u8>,
    file_name: &str,
    config: &ConfigManager,
) -> Result<String> {
    let mime = if file_name.to_lowercase().ends_with(".wav") {
        "audio/wav"
    } else {
        "audio/mpeg"
    };

    let mut form = reqwest::multipart::Form::new()
        .text("model", target.model.clone())
        .text("response_format", response_format.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(audio)
                .file_name(file_name.to_string())
                .mime_str(mime)?,
        );

    if target.service == "groq" {
        form = form.text("temperature", "0");
    }
    let lang = config.output_language();
    if lang != "auto" && !lang.is_empty() {
        form = form.text("language", lang);
    }

    let resp = HTTP_CLIENT
        .post(&target.url)
        .header("Authorization", format!("Bearer {}", target.api_key))
        .multipart(form)
        .send()
        .await
        .context("发送识别请求失败")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "API {}: {}",
            status,
            body.chars().take(300).collect::<String>()
        ));
    }
    Ok(body)
}

/// 解析 OpenAI 兼容 verbose_json：`segments[].{start,end,text}`（秒）
pub fn parse_verbose_json(body: &str) -> Option<Vec<DubSegment>> {
    let v: Value = serde_json::from_str(body).ok()?;
    let arr = v.get("segments")?.as_array()?;
    let mut out = Vec::new();
    for item in arr {
        let start = num_field(item, "start")?;
        let end = num_field(item, "end")?;
        let text = item
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(DubSegment {
            index: 0,
            start_ms: (start * 1000.0).max(0.0) as u64,
            end_ms: (end * 1000.0).max(0.0) as u64,
            text,
        });
    }
    Some(out)
}

fn num_field(item: &Value, key: &str) -> Option<f64> {
    match item.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

/// 解析 SRT 文本：`HH:MM:SS,mmm --> HH:MM:SS,mmm`
pub fn parse_srt(body: &str) -> Vec<DubSegment> {
    let mut out = Vec::new();
    for block in body.replace("\r\n", "\n").split("\n\n") {
        let mut lines = block.lines().map(|l| l.trim()).filter(|l| !l.is_empty());
        let mut time_line: Option<&str> = None;
        for line in lines.by_ref() {
            if line.contains("-->") {
                time_line = Some(line);
                break;
            }
        }
        let Some(tl) = time_line else { continue };
        let text: Vec<&str> = block
            .lines()
            .skip_while(|l| !l.contains("-->"))
            .skip(1)
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let joined = text.join(" ");
        if let Some((start, end)) = parse_srt_time_range(tl) {
            out.push(DubSegment {
                index: 0,
                start_ms: start,
                end_ms: end,
                text: joined,
            });
        }
    }
    out
}

/// 解析 `HH:MM:SS,mmm --> HH:MM:SS,mmm`（毫秒分隔符兼容 `,` 和 `.`）
fn parse_srt_time_range(line: &str) -> Option<(u64, u64)> {
    let mut parts = line.split("-->");
    let start = parse_srt_timestamp(parts.next()?.trim())?;
    let end = parse_srt_timestamp(parts.next()?.trim())?;
    Some((start, end))
}

fn parse_srt_timestamp(s: &str) -> Option<u64> {
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].trim().parse().ok()?;
    let m: f64 = parts[1].trim().parse().ok()?;
    let sec: f64 = parts[2].trim().parse().ok()?;
    let ms = (h * 3_600_000.0 + m * 60_000.0 + sec * 1000.0).max(0.0) as u64;
    Some(ms)
}

/// 收尾处理：修正异常区间、过滤噪声、合并过近的碎片段
fn finalize(segs: &mut Vec<DubSegment>) {
    segs.retain_mut(|s| {
        if s.end_ms <= s.start_ms {
            s.end_ms = s.start_ms + 200;
        }
        !crate::dubbing::is_noise_text(&s.text)
    });
    merge_fragments(segs);
    for (i, s) in segs.iter_mut().enumerate() {
        s.index = i;
    }
}

/// 合并间隔极小且总长可控的相邻碎片（提升配音流畅度）
fn merge_fragments(segs: &mut Vec<DubSegment>) {
    const MAX_GAP_MS: u64 = 150;
    const MAX_MERGED_CHARS: usize = 120;
    let mut merged: Vec<DubSegment> = Vec::with_capacity(segs.len());
    for seg in segs.drain(..) {
        match merged.last_mut() {
            Some(prev)
                if seg.start_ms.saturating_sub(prev.end_ms) <= MAX_GAP_MS
                    && prev.text.chars().count() + seg.text.chars().count() < MAX_MERGED_CHARS =>
            {
                if !prev.text.ends_with(' ') && !seg.text.starts_with(' ') {
                    prev.text.push(' ');
                }
                prev.text.push_str(&seg.text);
                prev.end_ms = seg.end_ms;
            }
            _ => merged.push(seg),
        }
    }
    *segs = merged;
}

/// 分段序列化为 SRT 文本（与字幕导出格式一致）
pub fn segments_to_srt(segs: &[DubSegment]) -> String {
    let mut out = String::new();
    for s in segs {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            s.index + 1,
            format_srt_ts(s.start_ms),
            format_srt_ts(s.end_ms),
            s.text
        ));
    }
    out
}

fn format_srt_ts(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let mm = ms % 1000;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, mm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verbose_json() {
        let body = r#"{
            "task": "transcribe",
            "text": "你好。世界。",
            "segments": [
                {"id": 0, "start": 0.0, "end": 1.25, "text": " 你好。"},
                {"id": 1, "start": 1.5, "end": "2.8", "text": " 世界。"}
            ]
        }"#;
        let segs = parse_verbose_json(body).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].start_ms, 0);
        assert_eq!(segs[0].end_ms, 1250);
        assert_eq!(segs[1].start_ms, 1500);
        assert_eq!(segs[1].end_ms, 2800); // 字符串数字也能解析
    }

    #[test]
    fn parses_srt_blocks() {
        let body = "1\r\n00:00:01,000 --> 00:00:03,500\r\n你好\r\n\r\n2\r\n00:00:04,000 --> 00:00:05,250\r\n世界\r\n";
        let segs = parse_srt(body);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].start_ms, 1000);
        assert_eq!(segs[0].end_ms, 3500);
        assert_eq!(segs[0].text, "你好");
        assert_eq!(segs[1].end_ms, 5250);
    }

    #[test]
    fn srt_roundtrip() {
        let segs = vec![DubSegment {
            index: 0,
            start_ms: 3661000,
            end_ms: 3665500,
            text: "测试一行".into(),
        }];
        let srt = segments_to_srt(&segs);
        assert!(srt.contains("01:01:01,000 --> 01:01:05,500"));
        let reparsed = parse_srt(&srt);
        assert_eq!(reparsed.len(), 1);
        assert_eq!(reparsed[0].start_ms, 3661000);
        assert_eq!(reparsed[0].end_ms, 3665500);
    }

    #[test]
    fn merges_close_fragments_and_drops_noise() {
        let mut segs = vec![
            DubSegment {
                index: 0,
                start_ms: 0,
                end_ms: 800,
                text: "你".into(),
            },
            DubSegment {
                index: 0,
                start_ms: 850,
                end_ms: 1600,
                text: "好".into(),
            },
            DubSegment {
                index: 0,
                start_ms: 1700,
                end_ms: 1900,
                text: "[Music]".into(),
            },
            DubSegment {
                index: 0,
                start_ms: 3000,
                end_ms: 4200,
                text: "世界".into(),
            },
        ];
        finalize(&mut segs);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "你 好");
        assert_eq!(segs[0].index, 0);
        assert_eq!(segs[1].text, "世界");
        assert_eq!(segs[1].index, 1);
    }
}
