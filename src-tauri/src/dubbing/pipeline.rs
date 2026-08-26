//! 视频配音流水线：以「节点 + 有界通道」形式组织，各阶段独立并发、按需背压。
//!
//! ```text
//! [提取节点] ──ChunkMsg──▶ (cap 2) ──▶ [识别节点] ──BatchMsg──▶ (cap 4) ──▶ [合成节点]
//!  ffmpeg 本地分块                云端 ASR 带时间戳              逐段 TTS + 时间轴流式写盘
//!                                                                      │
//!                                              [混流节点] ◀── 音轨完成后替换原声导出
//! ```
//!
//! - 识别与合成重叠执行：识别第 N+1 块的同时合成第 N 批分段，缩短总耗时
//! - 有界通道限制在途数据量，峰值内存只与单个分块相关
//! - 总进度权重：prepare 0-5% → extract 5-15% → asr 15-40% → tts 40-88% → mux 88-100%

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use crate::config::{ConfigManager, TtsConfig};
use crate::dubbing::{ffmpeg, transcribe, tts_segments, DubSegment};
use crate::tts::client::FishTtsClient;

const CANCELLED_SENTINEL: &str = "__cancelled__";

static DUBBING_RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL_DUBBING: AtomicBool = AtomicBool::new(false);

/// 音频分块时长（秒）：64kbps 单声道下每块约 4.8MB，远低于各云厂商上传限额
const CHUNK_SECONDS: u64 = 600;
/// 提取 → 识别 通道容量（在途分块数）
const CHUNK_CHANNEL_CAP: usize = 2;
/// 识别 → 合成 通道容量（在途分段批次数）
const BATCH_CHANNEL_CAP: usize = 4;

/// 配音任务选项（一期保持精简：输出目录可选，其余复用全局 ASR/TTS 配置）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DubOptions {
    #[serde(default)]
    pub output_dir: Option<String>,
}

/// 提取 → 识别 节点间的分块消息
struct ChunkMsg {
    index: usize,
    path: PathBuf,
}

/// 识别 → 合成 节点间的分段批次消息（`cumulative` 为截至目前累计分段数，
/// 用于合成节点在未知总数时估算进度）
struct BatchMsg {
    segments: Vec<DubSegment>,
    cumulative: usize,
}

pub fn is_running() -> bool {
    DUBBING_RUNNING.load(Ordering::SeqCst)
}

pub fn request_cancel() {
    CANCEL_DUBBING.store(true, Ordering::SeqCst);
}

/// 供 ffmpeg 下载等子模块轮询取消标志
pub fn is_cancel_requested() -> bool {
    CANCEL_DUBBING.load(Ordering::SeqCst)
}

fn is_cancelled() -> bool {
    CANCEL_DUBBING.load(Ordering::SeqCst)
}

fn check_cancel() -> Result<()> {
    if is_cancelled() {
        return Err(anyhow!("{}", CANCELLED_SENTINEL));
    }
    Ok(())
}

// ===================== 进度事件 =====================

fn emit_progress(
    app: &tauri::AppHandle,
    stage: &str,
    label: &str,
    percent: u32,
    message: &str,
    current: Option<usize>,
    total: Option<usize>,
) {
    let _ = app.emit(
        "dubbing-progress",
        serde_json::json!({
            "stage": stage,
            "label": label,
            "status": "running",
            "percent": percent.clamp(0, 99),
            "message": message,
            "current": current,
            "total": total,
        }),
    );
}

fn emit_done(app: &tauri::AppHandle, payload: serde_json::Value) {
    let _ = app.emit(
        "dubbing-progress",
        serde_json::json!({
            "stage": "done",
            "label": "完成",
            "status": "done",
            "percent": 100,
            "message": "配音视频已生成",
            "result": payload,
        }),
    );
}

fn emit_error(app: &tauri::AppHandle, message: &str) {
    let _ = app.emit(
        "dubbing-progress",
        serde_json::json!({
            "stage": "error",
            "label": "失败",
            "status": "error",
            "percent": 100,
            "message": message,
        }),
    );
}

fn emit_cancelled(app: &tauri::AppHandle) {
    let _ = app.emit(
        "dubbing-progress",
        serde_json::json!({
            "stage": "cancelled",
            "label": "已取消",
            "status": "cancelled",
            "percent": 0,
            "message": "任务已取消",
        }),
    );
}

// ===================== 任务入口 =====================

/// 启动配音任务（同一时间仅允许一个任务）。返回 Err 表示无法启动。
pub fn spawn_job(
    app: tauri::AppHandle,
    config: Arc<ConfigManager>,
    video_path: String,
    options: DubOptions,
) -> Result<(), String> {
    if DUBBING_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("已有配音任务正在进行".to_string());
    }
    CANCEL_DUBBING.store(false, Ordering::SeqCst);

    let app2 = app.clone();
    // 注意：dubbing_start 是同步命令，运行在主线程事件循环中，
    // 必须用 tauri::async_runtime::spawn（自带全局 runtime 句柄），
    // 直接 tokio::spawn 会因缺少 runtime 上下文而 panic
    tauri::async_runtime::spawn(async move {
        let result = run_job(&app2, config, video_path, options).await;
        DUBBING_RUNNING.store(false, Ordering::SeqCst);
        match result {
            Ok(payload) => emit_done(&app2, payload),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains(CANCELLED_SENTINEL) {
                    log::info!("[dubbing] 任务已取消");
                    emit_cancelled(&app2);
                } else {
                    log::error!("[dubbing] 任务失败: {}", msg);
                    emit_error(&app2, &msg);
                }
            }
        }
    });
    Ok(())
}

/// 任务临时目录守卫：销毁时清理中间产物（分块音频、配音音轨等）
struct TempGuard(PathBuf);

impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ===================== 编排器 =====================

async fn run_job(
    app: &tauri::AppHandle,
    config: Arc<ConfigManager>,
    video_path: String,
    options: DubOptions,
) -> Result<serde_json::Value> {
    let video = PathBuf::from(&video_path);
    if !video.is_file() {
        return Err(anyhow!("视频文件不存在: {}", video_path));
    }

    // ===== 准备节点：ffmpeg 就绪 =====
    emit_progress(
        app,
        "prepare",
        "准备工具",
        0,
        "正在定位 ffmpeg...",
        None,
        None,
    );
    let models_dir = config.current_models_dir();
    let ff = ffmpeg::ensure_ffmpeg(&models_dir, Some(app))
        .await
        .map_err(|e| anyhow!("ffmpeg 不可用且自动下载失败：{}", e))?;
    check_cancel()?;

    let temp_dir = config
        .config_dir()
        .join("dubbing_tmp")
        .join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .context("创建配音工作目录失败")?;
    let _temp_guard = TempGuard(temp_dir.clone());

    // ===== 提取节点：探测时长 + 分块提音轨 =====
    emit_progress(
        app,
        "extract",
        "提取音轨",
        5,
        "正在探测视频信息...",
        None,
        None,
    );
    let duration_secs = spawn_ffmpeg(ff.clone(), video.clone(), |ff, video| {
        ffmpeg::probe_duration(ff, video)
    })
    .await?;
    let est_chunks = ((duration_secs / CHUNK_SECONDS as f64).ceil() as usize).max(1);
    check_cancel()?;
    emit_progress(
        app,
        "extract",
        "提取音轨",
        7,
        &format!("视频时长 {}，正在提取音轨...", fmt_duration(duration_secs)),
        None,
        None,
    );

    // 先启动识别/合成节点（阻塞等待上游消息），再由提取节点喂数据，
    // 使「识别第 N+1 块」与「合成第 N 批」重叠执行
    let (chunk_tx, chunk_rx) = mpsc::channel::<ChunkMsg>(CHUNK_CHANNEL_CAP);
    let (batch_tx, batch_rx) = mpsc::channel::<BatchMsg>(BATCH_CHANNEL_CAP);

    let asr_app = app.clone();
    let asr_config = config.clone();
    let asr_node = tauri::async_runtime::spawn(asr_node(
        chunk_rx, batch_tx, asr_app, asr_config, est_chunks,
    ));

    let tts_app = app.clone();
    let tts_cfg = config.tts_config();
    let track_wav = temp_dir.join("dub_track.wav");
    let tts_node = tauri::async_runtime::spawn(tts_node(batch_rx, track_wav.clone(), tts_cfg, tts_app));

    // 提取节点（本地 ffmpeg，速度快）：产出分块后按序喂给识别节点
    let extract_result = {
        let ff2 = ff.clone();
        let v2 = video.clone();
        let td = temp_dir.clone();
        tokio::task::spawn_blocking(move || {
            ffmpeg::extract_audio_chunks(&ff2, &v2, &td, CHUNK_SECONDS)
        })
        .await
        .map_err(|e| anyhow!("提取音轨任务执行失败: {}", e))?
        .and_then(|_| collect_chunk_paths(&temp_dir))
    };

    let chunks = match extract_result {
        Ok(paths) => paths,
        Err(e) => {
            // 上游失败：关闭通道让下游节点自然收尾，再传播错误
            drop(chunk_tx);
            let _ = asr_node.await;
            let _ = tts_node.await;
            return Err(e);
        }
    };

    let chunk_total = chunks.len();
    log::info!(
        "[dubbing] 音轨分为 {} 块（每块 {} 秒）",
        chunk_total,
        CHUNK_SECONDS
    );

    for (i, path) in chunks.into_iter().enumerate() {
        if chunk_tx.send(ChunkMsg { index: i, path }).await.is_err() {
            break; // 下游已退出（取消/出错）
        }
    }
    drop(chunk_tx); // 关闭通道：识别节点消费完毕后自然收尾

    emit_progress(app, "extract", "提取音轨", 15, "音轨提取完成", None, None);

    let processed_chunks = asr_node
        .await
        .map_err(|e| anyhow!("识别节点异常退出: {}", e))??;
    let (ok_n, fail_n, segments) = tts_node
        .await
        .map_err(|e| anyhow!("合成节点异常退出: {}", e))??;

    check_cancel()?;
    if segments.is_empty() {
        return Err(anyhow!("未识别到任何语音内容"));
    }
    let seg_count = segments.len();
    log::info!(
        "[dubbing] 流水线完成：{} 块音频 / {} 个分段（TTS 成功 {} / 失败 {}）",
        processed_chunks,
        seg_count,
        ok_n,
        fail_n
    );

    // ===== 混流节点：替换音轨导出 =====
    let output = resolve_output_path(&video, options.output_dir.as_deref())?;
    emit_progress(
        app,
        "mux",
        "合成视频",
        90,
        "正在替换音轨并导出...",
        None,
        None,
    );
    {
        let ff2 = ff.clone();
        let v2 = video.clone();
        let tw = track_wav.clone();
        let out2 = output.clone();
        tokio::task::spawn_blocking(move || ffmpeg::mux_replace_audio(&ff2, &v2, &tw, &out2))
            .await
            .map_err(|e| anyhow!("混流任务执行失败: {}", e))??;
    }

    // 导出 SRT 字幕文件（与输出视频同目录同名）
    let srt_path = output.with_extension("srt");
    if let Err(e) = std::fs::write(&srt_path, transcribe::segments_to_srt(&segments)) {
        log::warn!("[dubbing] SRT 导出失败: {}", e);
    }

    Ok(serde_json::json!({
        "output": output.to_string_lossy(),
        "subtitle": srt_path.to_string_lossy(),
        "segments": seg_count,
        "failed_segments": fail_n,
    }))
}

/// 运行一个同步 ffmpeg 子命令封装（避免阻塞异步运行时）
async fn spawn_ffmpeg<T, F>(ff: PathBuf, video: PathBuf, f: F) -> Result<T>
where
    F: FnOnce(&PathBuf, &PathBuf) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || f(&ff, &video))
        .await
        .map_err(|e| anyhow!("ffmpeg 任务执行失败: {}", e))?
}

// ===================== 识别节点 =====================

async fn asr_node(
    mut rx: mpsc::Receiver<ChunkMsg>,
    tx: mpsc::Sender<BatchMsg>,
    app: tauri::AppHandle,
    config: Arc<ConfigManager>,
    est_chunks: usize,
) -> Result<usize> {
    let mut processed: usize = 0;
    let mut cumulative: usize = 0;

    while let Some(msg) = rx.recv().await {
        if is_cancelled() {
            return Err(anyhow!("{}", CANCELLED_SENTINEL));
        }
        processed += 1;
        let pct = 15 + ((processed * 25 / est_chunks.max(1)).min(25) as u32);
        emit_progress(
            &app,
            "asr",
            "识别字幕",
            pct,
            &format!("识别第 {} 段音频...", processed),
            Some(processed),
            None,
        );

        let mut part = transcribe::transcribe_file(&msg.path, &config).await?;

        // 分块内相对时间 → 全局绝对时间
        let offset_ms = (msg.index as u64) * CHUNK_SECONDS * 1000;
        for seg in part.iter_mut() {
            seg.start_ms += offset_ms;
            seg.end_ms += offset_ms;
        }
        cumulative += part.len();

        // 实时推送增量转写结果给前端预览（只发新增部分，避免全量重复序列化）
        let _ = app.emit(
            "dubbing-transcript",
            serde_json::json!({ "added": &part, "completed": processed, "cumulative": cumulative }),
        );

        if tx
            .send(BatchMsg {
                segments: part,
                cumulative,
            })
            .await
            .is_err()
        {
            // 合成节点已退出（出错或取消）：优雅停止，由编排器传播真实原因
            log::info!("[dubbing] 合成节点已退出，识别节点提前收尾");
            break;
        }
    }

    Ok(processed)
}

// ===================== 合成节点 =====================

async fn tts_node(
    mut rx: mpsc::Receiver<BatchMsg>,
    track_path: PathBuf,
    tts_cfg: TtsConfig,
    app: tauri::AppHandle,
) -> Result<(usize, usize, Vec<DubSegment>)> {
    let client = FishTtsClient::new();
    let base_speed = tts_cfg.speed.clamp(0.5, 2.0);
    let cfg = tts_segments::wav_tts_config(&tts_cfg);

    let mut writer = tts_segments::TimelineWriter::create(&track_path)?;
    let mut all_segments: Vec<DubSegment> = Vec::new();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut est_total = 0usize;

    while let Some(batch) = rx.recv().await {
        est_total = est_total.max(batch.cumulative);
        for seg in &batch.segments {
            if is_cancelled() {
                return Err(anyhow!("{}", CANCELLED_SENTINEL));
            }
            let pct = 40 + ((ok * 48 / est_total.max(1)).min(47) as u32);
            let preview: String = seg.text.chars().take(24).collect();
            emit_progress(
                &app,
                "tts",
                "语音合成",
                pct,
                &format!("合成 {}: {}...", ok + failed + 1, preview),
                Some(ok + failed),
                Some(est_total),
            );

            match tts_segments::synthesize_segment(
                &client,
                &cfg,
                &seg.text,
                seg.duration_ms(),
                base_speed,
            )
            .await
            {
                Ok(audio) => {
                    writer.write_at(seg.start_ms, &audio)?;
                    ok += 1;
                }
                Err(e) => {
                    log::warn!("[dubbing] 第 {} 段合成失败，保留静音: {}", seg.index + 1, e);
                    failed += 1;
                }
            }
        }
        all_segments.extend(batch.segments);
    }

    // 总时长取最后一段结束时间与游标的较大者（超时段已顺延）
    let last_end = all_segments.last().map(|s| s.end_ms).unwrap_or(0);
    let total_ms = writer.cursor_ms().max(last_end);
    writer.finish(total_ms)?;

    Ok((ok, failed, all_segments))
}

// ===================== 辅助 =====================

/// 收集提取节点产出的分块路径（按序）
fn collect_chunk_paths(dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut chunks: Vec<PathBuf> = std::fs::read_dir(dir)?
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

/// 计算输出路径：<output_dir|视频目录>/<stem>_dubbed.mp4（重名追加序号）
fn resolve_output_path(video: &std::path::Path, output_dir: Option<&str>) -> Result<PathBuf> {
    let parent = match output_dir.filter(|s| !s.trim().is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => video
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("无法确定输出目录"))?,
    };
    std::fs::create_dir_all(&parent).context("创建输出目录失败")?;
    let stem = video
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video");
    let mut out = parent.join(format!("{}_dubbed.mp4", stem));
    let mut n = 1;
    while out.exists() {
        n += 1;
        out = parent.join(format!("{}_dubbed_{}.mp4", stem, n));
    }
    Ok(out)
}

fn fmt_duration(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}
