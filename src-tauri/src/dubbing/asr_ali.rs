//! 阿里云百炼（DashScope）ASR：录音文件异步转写。
//!
//! 流程：本地分块上传至百炼临时存储（48h 有效）获取 `oss://` URL →
//! 提交 `qwen3-asr-flash-filetrans` 异步转写任务 → 轮询任务状态 →
//! 下载结果 JSON → 解析句级毫秒时间戳为配音分段。
//!
//! 接口参考：
//! - 上传凭证：GET /api/v1/uploads?action=getPolicy&model={model}
//! - 提交任务：POST /api/v1/services/audio/asr/transcription（X-DashScope-Async: enable）
//! - 查询任务：GET /api/v1/tasks/{task_id}

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::api::client::HTTP_CLIENT;
use crate::dubbing::DubSegment;

/// 异步录音文件转写模型（支持最长 12 小时音频，输出句级时间戳）
pub const DASHSCOPE_ASR_MODEL: &str = "qwen3-asr-flash-filetrans";
const DASHSCOPE_BASE: &str = "https://dashscope.aliyuncs.com";

/// 单块转写总超时（含排队与识别）
const TASK_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// 轮询间隔下限/上限
const POLL_MIN: Duration = Duration::from_secs(2);
const POLL_MAX: Duration = Duration::from_secs(6);

/// 阿里 ASR 识别参数（节点设置面板可调）
#[derive(Debug, Clone)]
pub struct AliAsrOptions {
    /// 逆文本规范化（中文/英文数字转阿拉伯数字）
    pub enable_itn: bool,
    /// 词级时间戳（同时影响断句：VAD+标点），供前端精确重新分段
    pub enable_words: bool,
    /// 语种提示（空/auto = 自动检测）
    pub language: String,
}

impl Default for AliAsrOptions {
    fn default() -> Self {
        Self {
            enable_itn: true,
            enable_words: true,
            language: String::new(),
        }
    }
}

/// 转写单个音频分块文件，返回分块内相对时间戳分段（毫秒）。
pub async fn transcribe_chunk(
    path: &Path,
    api_key: &str,
    chunk_name: &str,
    opts: &AliAsrOptions,
    is_cancelled: impl Fn() -> bool,
) -> Result<Vec<DubSegment>> {
    if api_key.is_empty() {
        return Err(anyhow!(
            "未配置阿里云百炼 API Key：请在「设置 → API 密钥」中填写，或将配音引擎切换为全局整段识别"
        ));
    }

    let oss_url = upload_to_temp_storage(path, api_key, chunk_name).await?;
    let task_id = submit_task(api_key, &oss_url, opts).await?;
    let result_json = poll_task(api_key, &task_id, &is_cancelled).await?;
    let mut segs = parse_transcription(&result_json)?;
    crate::dubbing::transcribe::finalize_segments(&mut segs);
    Ok(segs)
}

// ===================== 步骤 1：上传到临时存储 =====================

async fn upload_to_temp_storage(path: &Path, api_key: &str, chunk_name: &str) -> Result<String> {
    // 1a. 获取上传凭证（model 必须与后续转写模型完全一致）
    let policy_url = format!(
        "{}/api/v1/uploads?action=getPolicy&model={}",
        DASHSCOPE_BASE, DASHSCOPE_ASR_MODEL
    );
    let resp = HTTP_CLIENT
        .get(&policy_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .context("请求阿里云上传凭证失败")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("获取上传凭证失败 {}: {}", status, brief(&body)));
    }
    let data = parse_field(&body, "data").ok_or_else(|| anyhow!("上传凭证响应缺少 data 字段"))?;

    let upload_host = data
        .get("upload_host")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("上传凭证缺少 upload_host"))?
        .to_string();
    let upload_dir = data
        .get("upload_dir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("上传凭证缺少 upload_dir"))?
        .to_string();
    let max_mb = data
        .get("max_file_size_mb")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or(v.as_u64())
        })
        .unwrap_or(100);

    let get_str = |field: &str| -> String {
        data.get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let oss_access_key_id = get_str("oss_access_key_id");
    let signature = get_str("signature");
    let policy = get_str("policy");
    let acl = {
        let v = get_str("x_oss_object_acl");
        if v.is_empty() {
            "private".to_string()
        } else {
            v
        }
    };
    let forbid_overwrite = {
        let v = get_str("x_oss_forbid_overwrite");
        if v.is_empty() {
            "true".to_string()
        } else {
            v
        }
    };

    let audio = read_chunk(path)?;
    if max_mb > 0 && audio.len() as u64 > max_mb * 1024 * 1024 {
        return Err(anyhow!("音频分块超过百炼单文件上限 {} MB", max_mb));
    }

    // 1b. multipart POST 到 OSS（file 必须是最后一个表单域）
    let key = format!("{}/{}", upload_dir, chunk_name);
    let form = reqwest::multipart::Form::new()
        .text("OSSAccessKeyId", oss_access_key_id)
        .text("Signature", signature)
        .text("policy", policy)
        .text("key", key.clone())
        .text("x-oss-object-acl", acl)
        .text("x-oss-forbid-overwrite", forbid_overwrite)
        .text("success_action_status", "200")
        .part(
            "file",
            reqwest::multipart::Part::bytes(audio)
                .file_name(chunk_name.to_string())
                .mime_str("audio/mpeg")?,
        );

    let resp = HTTP_CLIENT
        .post(&upload_host)
        .multipart(form)
        .send()
        .await
        .context("上传音频到阿里云临时存储失败")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("上传音频失败 {}: {}", status, brief(&body)));
    }

    Ok(format!("oss://{}", key))
}

// ===================== 步骤 2：提交异步转写任务 =====================

async fn submit_task(api_key: &str, oss_url: &str, opts: &AliAsrOptions) -> Result<String> {
    let url = format!("{}/api/v1/services/audio/asr/transcription", DASHSCOPE_BASE);
    let mut parameters = json!({
        "channel_id": [0],
        // 句级时间戳 + VAD 断句；开启词级后为 VAD+标点断句，便于前端精确分段
        "enable_words": opts.enable_words,
        "enable_itn": opts.enable_itn,
    });
    if !opts.language.is_empty() && opts.language != "auto" {
        parameters["language"] = json!(opts.language);
    }
    let body = json!({
        "model": DASHSCOPE_ASR_MODEL,
        "input": { "file_url": oss_url },
        "parameters": parameters,
    });

    let resp = HTTP_CLIENT
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("X-DashScope-Async", "enable")
        // oss:// 临时链接需要服务端解析内部协议
        .header("X-DashScope-OssResourceResolve", "enable")
        .json(&body)
        .send()
        .await
        .context("提交阿里云转写任务失败")?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("提交转写任务失败 {}: {}", status, brief(&text)));
    }
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("解析任务响应失败: {}", brief(&text)))?;
    v.pointer("/output/task_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("任务响应缺少 task_id: {}", brief(&text)))
}

// ===================== 步骤 3：轮询任务状态 =====================

async fn poll_task(
    api_key: &str,
    task_id: &str,
    is_cancelled: impl Fn() -> bool,
) -> Result<String> {
    let url = format!("{}/api/v1/tasks/{}", DASHSCOPE_BASE, task_id);
    let started = Instant::now();
    let mut interval = POLL_MIN;

    loop {
        if is_cancelled() {
            return Err(anyhow!("__cancelled__"));
        }
        if started.elapsed() > TASK_TIMEOUT {
            return Err(anyhow!(
                "转写任务超时（{} 分钟）",
                TASK_TIMEOUT.as_secs() / 60
            ));
        }

        let resp = HTTP_CLIENT
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
            .context("查询阿里云转写任务失败")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("查询任务失败 {}: {}", status, brief(&text)));
        }
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("解析任务状态失败: {}", brief(&text)))?;
        let task_status = v
            .pointer("/output/task_status")
            .and_then(Value::as_str)
            .unwrap_or("");

        match task_status {
            "SUCCEEDED" => {
                let url = v
                    .pointer("/output/result/transcription_url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("任务成功但缺少 transcription_url"))?
                    .to_string();
                return download_result(&url).await;
            }
            "FAILED" | "UNKNOWN" => {
                let code = v
                    .pointer("/output/code")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let message = v
                    .pointer("/output/message")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                return Err(anyhow!("阿里云转写任务失败 {} {}", code, message));
            }
            _ => {} // PENDING / RUNNING
        }

        tokio::time::sleep(interval).await;
        interval = (interval * 3 / 2).min(POLL_MAX);
    }
}

async fn download_result(url: &str) -> Result<String> {
    let resp = HTTP_CLIENT
        .get(url)
        .send()
        .await
        .context("下载阿里云转写结果失败")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("下载转写结果失败 {}: {}", status, brief(&text)));
    }
    Ok(text)
}

// ===================== 步骤 4：解析结果 =====================

/// 解析转录结果 JSON：`transcripts[].sentences[].{begin_time,end_time,text,words[]}`（毫秒）
pub fn parse_transcription(body: &str) -> Result<Vec<DubSegment>> {
    let v: Value = serde_json::from_str(body).context("解析转写结果 JSON 失败")?;
    let transcripts = v
        .get("transcripts")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("转写结果缺少 transcripts 字段"))?;

    let mut out = Vec::new();
    for track in transcripts {
        let Some(sentences) = track.get("sentences").and_then(Value::as_array) else {
            continue;
        };
        for s in sentences {
            let start_ms = s.get("begin_time").and_then(Value::as_i64).unwrap_or(0);
            let end_ms = s.get("end_time").and_then(Value::as_i64).unwrap_or(0);
            let text = s
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();

            // 词级时间戳（enable_words=true 时返回），供前端精确分段
            let words = s.get("words").and_then(Value::as_array).map(|ws| {
                ws.iter()
                    .filter_map(|w| {
                        Some(crate::dubbing::DubWord {
                            begin_ms: w.get("begin_time").and_then(Value::as_i64)?.max(0) as u64,
                            end_ms: w.get("end_time").and_then(Value::as_i64)?.max(0) as u64,
                            text: w.get("text").and_then(Value::as_str)?.trim().to_string(),
                        })
                    })
                    .collect::<Vec<_>>()
            });
            let words = words.filter(|w| !w.is_empty());

            // 句级边界常带前后衬垫，用首词起点/末词终点向内收紧（不外扩，避免与相邻段重叠）
            let (start_ms, end_ms) = if let Some(ws) = &words {
                let s_start = start_ms.max(0) as u64;
                let s_end = end_ms.max(0) as u64;
                let w_start = ws.first().map(|w| w.begin_ms).unwrap_or(s_start);
                let w_end = ws.last().map(|w| w.end_ms).unwrap_or(s_end);
                let ns = w_start.max(s_start).min(s_end);
                let ne = w_end.min(s_end).max(ns);
                if ne > ns { (ns, ne) } else { (s_start, s_end) }
            } else {
                (start_ms.max(0) as u64, end_ms.max(0) as u64)
            };

            out.push(DubSegment {
                index: 0,
                start_ms,
                end_ms,
                text,
                words,
            });
        }
    }
    Ok(out)
}

fn read_chunk(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("读取音频分块失败: {}", path.display()))
}

fn parse_field(body: &str, field: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get(field)
        .cloned()
}

fn brief(s: &str) -> String {
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sentences_with_ms_timestamps() {
        let body = r#"{
            "file_url": "oss://x/y.mp3",
            "audio_info": {"format": "mp3", "sample_rate": 16000},
            "transcripts": [
                {
                    "channel_id": 0,
                    "text": "你好。世界。",
                    "sentences": [
                        {"sentence_id": 0, "begin_time": 240, "end_time": 1440, "text": " 你好。"},
                        {"sentence_id": 1, "begin_time": 1500, "end_time": 2800, "text": "世界。"}
                    ]
                }
            ]
        }"#;
        let segs = parse_transcription(body).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].start_ms, 240);
        assert_eq!(segs[0].end_ms, 1440);
        assert_eq!(segs[1].text, "世界。");
    }

    #[test]
    fn rejects_missing_transcripts() {
        assert!(parse_transcription("{\"error\":{}}").is_err());
    }
}
