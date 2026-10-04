//! 视频配音流水线（v2），分两阶段执行，中间留给用户编辑字幕。
//!
//! ```text
//! 阶段一 prepare ：ffmpeg 就绪 → 探测时长 → 提取分块音轨 ─▶ 云端 ASR（滑动窗口并发、按块序回填）
//!                 每块识别完立即推送 `dubbing-transcript`，结束时 done 事件携带全部分段
//! 阶段二 generate：逐段 TTS（有界并发、保序）→ 只压缩放不下的段 → 时间轴拼装
//!                 → 替换原声（或保留压低的原声做背景）导出视频 + 同名 SRT
//! ```
//!
//! 事件（前端按 phase 区分阶段）：
//! - `dubbing-progress`  `{phase, stage, status: running|done|error|cancelled, percent, message, current?, total?, result?}`
//! - `dubbing-transcript` `{added: [DubSegment], done, total}`
//! - `dubbing-segment`   `{index, status: running|done|failed, fit: natural|compressed|truncated, ms?, error?}`
//!
//! 同一时间只允许一个任务。[`JobSlot`] 是 RAII 运行锁：任务无论成功、失败、取消还是
//! panic 展开都会释放，不会再出现「已有配音任务正在进行」卡死。

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures_util::FutureExt;
use serde::Deserialize;
use tauri::Emitter;
use tokio::sync::mpsc;

use crate::config::{ConfigManager, TtsConfig};
use crate::dubbing::{asr_ali, ffmpeg, transcribe, tts_segments, DubSegment};
use crate::tts::client::FishTtsClient;

const CANCELLED: &str = "__cancelled__";

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

/// 音频分块时长（秒）：64kbps 单声道下每块约 4.8MB，远低于各云厂商上传限额
const DEFAULT_CHUNK_SECONDS: u64 = 600;
/// 提取 → 识别 通道容量（在途分块数）
const CHUNK_CHANNEL_CAP: usize = 2;
/// 云端 ASR 并发块数（结果仍按块顺序回填）
const ASR_CONCURRENCY: usize = 2;
/// Fish Audio 有速率限制，3 路并发在提速 2-3 倍的同时不易触发限流
const TTS_CONCURRENCY: usize = 3;
/// 开头连续失败多少段即判定配置有误、整体中止
const MAX_INITIAL_FAILS: usize = 3;
/// 最后一段可向后借用的时长
const TAIL_ROOM_MS: u64 = 1500;

// ===================== 选项 =====================

/// 阶段一选项：识别引擎与其参数
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PrepareOptions {
    /// `ali-dashscope`（默认）/ `global-compat`（跟随整段识别配置）
    #[serde(default)]
    pub asr_provider: Option<String>,
    /// 词级时间戳（阿里专有，开启后前端可精确重新分段）
    #[serde(default)]
    pub ali_enable_words: Option<bool>,
    /// 逆文本规范化（数字转阿拉伯数字等）
    #[serde(default)]
    pub ali_enable_itn: Option<bool>,
    /// 语种提示（空/auto = 自动检测）
    #[serde(default)]
    pub ali_language: Option<String>,
    /// 音频分块时长（秒）
    #[serde(default)]
    pub chunk_seconds: Option<u64>,
}

/// Fish Audio 合成参数覆盖（未提供的字段沿用「语音合成」页保存值）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TtsOverrides {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub reference_title: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub volume: Option<f32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub latency: Option<String>,
    #[serde(default)]
    pub chunk_length: Option<u32>,
    #[serde(default)]
    pub normalize: Option<bool>,
}

impl TtsOverrides {
    fn apply_to(&self, cfg: &mut TtsConfig) {
        if let Some(v) = &self.model {
            cfg.model = v.clone();
        }
        if let Some(v) = &self.reference_id {
            cfg.reference_id = v.clone();
        }
        if let Some(v) = self.speed {
            cfg.speed = v;
        }
        if let Some(v) = self.volume {
            cfg.volume = v;
        }
        if let Some(v) = self.temperature {
            cfg.temperature = v;
        }
        if let Some(v) = self.top_p {
            cfg.top_p = v;
        }
        if let Some(v) = &self.latency {
            cfg.latency = v.clone();
        }
        if let Some(v) = self.chunk_length {
            cfg.chunk_length = v;
        }
        if let Some(v) = self.normalize {
            cfg.normalize = v;
        }
    }
}

/// 阶段二选项
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GenerateOptions {
    #[serde(default)]
    pub output_dir: Option<String>,
    #[serde(default)]
    pub tts: Option<TtsOverrides>,
    /// 原声保留音量（0 = 完全替换；0.1~0.3 适合保留背景音乐与环境声）
    #[serde(default)]
    pub original_volume: Option<f32>,
}

// ===================== 运行锁与取消 =====================

/// RAII 运行锁：持有期间其它任务无法启动，Drop 时释放
struct JobSlot;

impl JobSlot {
    fn acquire() -> Result<Self, String> {
        if RUNNING.swap(true, Ordering::SeqCst) {
            return Err("已有配音任务正在进行".to_string());
        }
        CANCEL.store(false, Ordering::SeqCst);
        Ok(JobSlot)
    }
}

impl Drop for JobSlot {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn request_cancel() {
    if is_running() {
        CANCEL.store(true, Ordering::SeqCst);
    }
}

/// 供 ffmpeg 下载、阿里 ASR 轮询等子模块检查取消
pub fn is_cancel_requested() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

fn check_cancel() -> Result<()> {
    if is_cancel_requested() {
        Err(anyhow!(CANCELLED))
    } else {
        Ok(())
    }
}

fn is_cancel_error(e: &anyhow::Error) -> bool {
    e.to_string().contains(CANCELLED)
}

// ===================== 进度 =====================

#[derive(Clone, Copy)]
enum Phase {
    Prepare,
    Generate,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Prepare => "prepare",
            Phase::Generate => "generate",
        }
    }
}

#[derive(Clone)]
struct Progress {
    app: tauri::AppHandle,
    phase: Phase,
}

impl Progress {
    fn running(&self, stage: &str, percent: u32, message: &str, current: Option<usize>, total: Option<usize>) {
        let _ = self.app.emit(
            "dubbing-progress",
            serde_json::json!({
                "phase": self.phase.as_str(),
                "stage": stage,
                "status": "running",
                "percent": percent.min(99),
                "message": message,
                "current": current,
                "total": total,
            }),
        );
    }

    fn finish(&self, result: Result<serde_json::Value>) {
        let (status, message, payload) = match result {
            Ok(v) => ("done", "完成".to_string(), v),
            Err(e) if is_cancel_error(&e) => {
                log::info!("[dubbing] 任务已取消");
                ("cancelled", "任务已取消".to_string(), serde_json::Value::Null)
            }
            Err(e) => {
                let msg = e.to_string();
                log::error!("[dubbing] 任务失败: {}", msg);
                ("error", msg, serde_json::Value::Null)
            }
        };
        let _ = self.app.emit(
            "dubbing-progress",
            serde_json::json!({
                "phase": self.phase.as_str(),
                "stage": status,
                "status": status,
                "percent": if status == "done" { 100 } else { 0 },
                "message": message,
                "result": payload,
            }),
        );
    }

    fn segment(&self, index: usize, status: &str, fit: &str, ms: Option<u64>, error: Option<&str>) {
        let _ = self.app.emit(
            "dubbing-segment",
            serde_json::json!({ "index": index, "status": status, "fit": fit, "ms": ms, "error": error }),
        );
    }
}

/// 在后台运行一个配音任务：持锁 → 执行（捕获 panic）→ 释放锁 → 发送终态事件
fn spawn_job<F>(app: tauri::AppHandle, phase: Phase, job: F) -> Result<(), String>
where
    F: Future<Output = Result<serde_json::Value>> + Send + 'static,
{
    let slot = JobSlot::acquire()?;
    let progress = Progress { app, phase };
    // 同步命令运行在主线程事件循环中，必须用 tauri::async_runtime::spawn
    tauri::async_runtime::spawn(async move {
        let result = AssertUnwindSafe(job)
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(anyhow!("配音任务内部错误，已中止")));
        // 先释放锁再通知：前端收到 done 后可立即开始下一阶段
        drop(slot);
        progress.finish(result);
    });
    Ok(())
}

/// 任务临时目录：销毁时清理中间产物（分块音频、配音音轨等）
struct TempDir(PathBuf);

impl TempDir {
    async fn create(config: &ConfigManager) -> Result<Self> {
        let dir = config
            .config_dir()
            .join("dubbing_tmp")
            .join(uuid::Uuid::new_v4().to_string());
        tokio::fs::create_dir_all(&dir).await.context("创建配音工作目录失败")?;
        Ok(Self(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn ready_ffmpeg(app: &tauri::AppHandle, config: &ConfigManager, progress: &Progress) -> Result<PathBuf> {
    progress.running("tools", 0, "正在准备 ffmpeg…", None, None);
    let ff = ffmpeg::ensure_ffmpeg(&config.current_models_dir(), Some(app))
        .await
        .map_err(|e| anyhow!("ffmpeg 不可用且自动下载失败：{}", e))?;
    check_cancel()?;
    Ok(ff)
}

/// 运行同步 ffmpeg 封装，避免阻塞异步运行时
async fn blocking<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| anyhow!("后台任务执行失败: {}", e))?
}

// ===================== 阶段一：识别字幕 =====================

pub fn spawn_prepare(
    app: tauri::AppHandle,
    config: Arc<ConfigManager>,
    video_path: String,
    options: PrepareOptions,
) -> Result<(), String> {
    let progress = Progress { app: app.clone(), phase: Phase::Prepare };
    spawn_job(app.clone(), Phase::Prepare, async move {
        run_prepare(&app, &progress, config, video_path, options).await
    })
}

async fn run_prepare(
    app: &tauri::AppHandle,
    progress: &Progress,
    config: Arc<ConfigManager>,
    video_path: String,
    options: PrepareOptions,
) -> Result<serde_json::Value> {
    let video = PathBuf::from(&video_path);
    if !video.is_file() {
        return Err(anyhow!("视频文件不存在: {}", video_path));
    }
    let chunk_seconds = options.chunk_seconds.unwrap_or(DEFAULT_CHUNK_SECONDS).clamp(60, 3600);
    let ff = ready_ffmpeg(app, &config, progress).await?;

    // ===== 提取 =====
    progress.running("extract", 5, "正在读取视频信息…", None, None);
    let duration_secs = {
        let (ff, video) = (ff.clone(), video.clone());
        blocking(move || ffmpeg::probe_duration(&ff, &video)).await?
    };
    check_cancel()?;
    progress.running("extract", 8, &format!("视频时长 {}，正在提取音轨…", fmt_duration(duration_secs)), None, None);

    let temp = TempDir::create(&config).await?;
    let chunks = {
        let (ff, video, dir) = (ff.clone(), video.clone(), temp.0.clone());
        blocking(move || ffmpeg::extract_audio_chunks(&ff, &video, &dir, chunk_seconds)).await?
    };
    check_cancel()?;
    let total = chunks.len();
    progress.running("asr", 15, &format!("音轨已提取（{} 块），开始识别…", total), Some(0), Some(total));

    // ===== 识别 =====
    let backend = resolve_asr_backend(&config, options.asr_provider.as_deref())?;
    log::info!("[dubbing] ASR 引擎: {}", backend.name());
    let ali_opts = asr_ali::AliAsrOptions {
        enable_itn: options.ali_enable_itn.unwrap_or(true),
        enable_words: options.ali_enable_words.unwrap_or(true),
        language: options.ali_language.unwrap_or_default(),
    };

    let (tx, rx) = mpsc::channel::<(usize, PathBuf)>(CHUNK_CHANNEL_CAP);
    let feeder = tokio::spawn(async move {
        for item in chunks.into_iter().enumerate() {
            if tx.send(item).await.is_err() {
                break;
            }
        }
    });
    let segments = recognize(rx, app, progress, config.clone(), backend, ali_opts, total, chunk_seconds).await;
    feeder.abort();
    let segments = segments?;
    check_cancel()?;
    drop(temp);

    log::info!("[dubbing] 识别完成：{} 块音频 / {} 个分段", total, segments.len());
    Ok(serde_json::json!({
        "phase": "prepare",
        "segments": segments,
        "durationMs": (duration_secs * 1000.0) as u64,
    }))
}

#[derive(Debug, Clone)]
enum AsrBackend {
    /// 阿里云百炼录音文件转写（句/词级毫秒时间戳，支持超长音频）
    AliDashScope { api_key: String },
    /// OpenAI 兼容 /v1/audio/transcriptions（verbose_json → srt 回退）
    GlobalCompat,
}

impl AsrBackend {
    fn name(&self) -> &'static str {
        match self {
            AsrBackend::AliDashScope { .. } => "阿里云百炼 (qwen3-asr-flash-filetrans)",
            AsrBackend::GlobalCompat => "全局整段识别配置",
        }
    }
}

fn resolve_asr_backend(config: &ConfigManager, provider: Option<&str>) -> Result<AsrBackend> {
    let provider = provider.unwrap_or_default();
    let ali_key = config.get_dashscope_api_key();
    match provider {
        crate::config::DUBBING_ASR_GLOBAL => Ok(AsrBackend::GlobalCompat),
        "" | crate::config::DUBBING_ASR_ALI => {
            if ali_key.is_empty() {
                log::warn!("[dubbing] 未配置阿里云百炼 Key，回退为全局整段识别配置");
                Ok(AsrBackend::GlobalCompat)
            } else {
                Ok(AsrBackend::AliDashScope { api_key: ali_key })
            }
        }
        other => Err(anyhow!("未知识别引擎: {}", other)),
    }
}

/// 滑动窗口并发识别：最多 ASR_CONCURRENCY 块在途，按块序收割，
/// 分块内相对时间平移为全局时间后推送增量结果。
#[allow(clippy::too_many_arguments)]
async fn recognize(
    mut rx: mpsc::Receiver<(usize, PathBuf)>,
    app: &tauri::AppHandle,
    progress: &Progress,
    config: Arc<ConfigManager>,
    backend: AsrBackend,
    ali_opts: asr_ali::AliAsrOptions,
    total: usize,
    chunk_seconds: u64,
) -> Result<Vec<DubSegment>> {
    type Pending = (usize, tokio::task::JoinHandle<Result<Vec<DubSegment>>>);
    let mut inflight: std::collections::VecDeque<Pending> = Default::default();
    let mut all: Vec<DubSegment> = Vec::new();
    let mut done = 0usize;

    let result: Result<()> = async {
        loop {
            // 窗口未满且还有块：继续派发；否则收割最早一块
            let next = if inflight.len() < ASR_CONCURRENCY { rx.recv().await } else { None };
            match next {
                Some((index, path)) => {
                    check_cancel()?;
                    let (backend, opts, cfg) = (backend.clone(), ali_opts.clone(), config.clone());
                    let name = format!("chunk_{:04}.mp3", index);
                    inflight.push_back((
                        index,
                        tokio::spawn(async move {
                            match backend {
                                AsrBackend::AliDashScope { api_key } => {
                                    asr_ali::transcribe_chunk(&path, &api_key, &name, &opts, is_cancel_requested).await
                                }
                                AsrBackend::GlobalCompat => transcribe::transcribe_file(&path, &cfg).await,
                            }
                        }),
                    ));
                }
                None => {
                    let Some((index, handle)) = inflight.pop_front() else { break };
                    let mut part = handle.await.map_err(|e| anyhow!("识别任务执行失败: {}", e))??;
                    check_cancel()?;
                    let offset = index as u64 * chunk_seconds * 1000;
                    for seg in part.iter_mut() {
                        seg.start_ms += offset;
                        seg.end_ms += offset;
                        if let Some(words) = seg.words.as_mut() {
                            for w in words.iter_mut() {
                                w.begin_ms += offset;
                                w.end_ms += offset;
                            }
                        }
                    }
                    part.retain(|s| !crate::dubbing::is_noise_text(&s.text));
                    for (i, seg) in part.iter_mut().enumerate() {
                        seg.index = all.len() + i;
                    }
                    done += 1;
                    let _ = app.emit(
                        "dubbing-transcript",
                        serde_json::json!({ "added": &part, "done": done, "total": total }),
                    );
                    all.extend(part);
                    let pct = 15 + (done * 84 / total.max(1)) as u32;
                    progress.running("asr", pct, &format!("已识别 {}/{} 块，共 {} 句", done, total, all.len()), Some(done), Some(total));
                }
            }
        }
        Ok(())
    }
    .await;

    if let Err(e) = result {
        for (_, h) in inflight {
            h.abort();
        }
        return Err(e);
    }
    if all.is_empty() {
        return Err(anyhow!("没有识别到任何语音，请确认视频里有人声"));
    }
    Ok(all)
}

// ===================== 阶段二：生成配音 =====================

pub fn spawn_generate(
    app: tauri::AppHandle,
    config: Arc<ConfigManager>,
    video_path: String,
    segments: Vec<DubSegment>,
    options: GenerateOptions,
) -> Result<(), String> {
    let segments = clean_segments(segments);
    if segments.is_empty() {
        return Err("字幕为空，请先识别字幕或手动添加分段".to_string());
    }
    let progress = Progress { app: app.clone(), phase: Phase::Generate };
    spawn_job(app.clone(), Phase::Generate, async move {
        run_generate(&app, &progress, config, video_path, segments, options).await
    })
}

/// 规整前端提交的分段：去空文本、按起点排序、修正倒置时间、重排索引
fn clean_segments(mut segments: Vec<DubSegment>) -> Vec<DubSegment> {
    segments.retain(|s| !s.text.trim().is_empty());
    segments.sort_by_key(|s| s.start_ms);
    for (i, s) in segments.iter_mut().enumerate() {
        s.index = i;
        if s.end_ms <= s.start_ms {
            s.end_ms = s.start_ms + 500;
        }
    }
    segments
}

/// 每段配音可用的时长：到下一段开始为止（借用句间停顿），最后一段可多借一点。
/// 只有超出这段空间才压缩，空间内保持自然语速。
fn room_for(segments: &[DubSegment]) -> Vec<u64> {
    segments
        .iter()
        .enumerate()
        .map(|(i, s)| match segments.get(i + 1) {
            Some(next) => next.start_ms.saturating_sub(s.start_ms).max(s.duration_ms()),
            None => s.duration_ms() + TAIL_ROOM_MS,
        })
        .collect()
}

async fn run_generate(
    app: &tauri::AppHandle,
    progress: &Progress,
    config: Arc<ConfigManager>,
    video_path: String,
    segments: Vec<DubSegment>,
    options: GenerateOptions,
) -> Result<serde_json::Value> {
    let video = PathBuf::from(&video_path);
    if !video.is_file() {
        return Err(anyhow!("视频文件不存在: {}", video_path));
    }
    let ff = ready_ffmpeg(app, &config, progress).await?;
    let temp = TempDir::create(&config).await?;

    let mut tts_cfg = config.tts_config();
    if let Some(ov) = &options.tts {
        ov.apply_to(&mut tts_cfg);
        if let Some(title) = ov.reference_title.as_deref().filter(|t| !t.is_empty()) {
            log::info!("[dubbing] 配音音色: {}", title);
        }
    }
    let track = temp.0.join("dub_track.wav");
    let stats = synthesize_track(progress, &ff, &temp.0, &segments, &track, &tts_cfg).await?;
    check_cancel()?;
    log::info!(
        "[dubbing] 合成完成：成功 {} 段，失败 {} 段，压缩 {} 段，截断 {} 段",
        stats.ok, stats.failed, stats.compressed, stats.truncated
    );

    // ===== 混流 =====
    let output = resolve_output_path(&video, options.output_dir.as_deref())?;
    let bg = options.original_volume.unwrap_or(0.0).clamp(0.0, 1.0);
    progress.running(
        "mux",
        90,
        if bg > 0.0 { "正在混合原声背景并导出视频…" } else { "正在替换音轨并导出视频…" },
        None,
        None,
    );
    {
        let (ff, video, track, out) = (ff.clone(), video.clone(), track.clone(), output.clone());
        blocking(move || ffmpeg::mux_dub_audio(&ff, &video, &track, &out, bg)).await?;
    }

    // 同目录同名 SRT
    let srt = output.with_extension("srt");
    if let Err(e) = std::fs::write(&srt, transcribe::segments_to_srt(&segments)) {
        log::warn!("[dubbing] SRT 导出失败: {}", e);
    }
    drop(temp);

    Ok(serde_json::json!({
        "phase": "generate",
        "output": output.to_string_lossy(),
        "subtitle": srt.to_string_lossy(),
        "segments": segments.len(),
        "failedSegments": stats.failed,
        "compressedSegments": stats.compressed,
        "truncatedSegments": stats.truncated,
    }))
}

#[derive(Default)]
struct TrackStats {
    ok: usize,
    failed: usize,
    compressed: usize,
    truncated: usize,
}

/// 逐段 TTS → 贴合可用空间 → 按原起点写入时间轴。
/// 有界并发 + 保序（`buffered`），每段状态实时推送给前端。
async fn synthesize_track(
    progress: &Progress,
    ff: &Path,
    temp_dir: &Path,
    segments: &[DubSegment],
    track_path: &Path,
    tts_cfg: &TtsConfig,
) -> Result<TrackStats> {
    use futures_util::stream::{self, StreamExt};

    let total = segments.len();
    let base_speed = tts_cfg.speed.clamp(0.5, 2.0);
    // 整个闭包链只捕获 Arc / 拥有所有权的值：以引用为参的 async 闭包会让 HRTB 推断失败
    let client = Arc::new(FishTtsClient::new());
    let cfg = Arc::new(tts_segments::wav_tts_config(tts_cfg));
    let rooms = room_for(segments);
    progress.running("tts", 3, &format!("共 {} 句待配音", total), Some(0), Some(total));

    let jobs = segments.iter().cloned().zip(rooms).map(|(seg, room)| {
        let client = Arc::clone(&client);
        let cfg = Arc::clone(&cfg);
        let ff = ff.to_path_buf();
        let td = temp_dir.to_path_buf();
        let progress = progress.clone();
        async move {
            check_cancel()?;
            progress.segment(seg.index, "running", "natural", None, None);
            let speed = tts_segments::estimate_fit_speed(&seg.text, seg.duration_ms(), base_speed);
            let raw = tts_segments::synthesize_segment(&client, &cfg, &seg.text, speed).await?;
            let idx = seg.index;
            let (audio, fit) = blocking(move || Ok(tts_segments::fit_to_slot(&ff, &td, idx, raw, room))).await?;
            Ok::<_, anyhow::Error>((seg, audio, fit))
        }
    });
    let mut stream = stream::iter(jobs).buffered(TTS_CONCURRENCY);

    let mut writer = tts_segments::TimelineWriter::create(track_path)?;
    let mut stats = TrackStats::default();
    let mut index = 0usize;
    while let Some(item) = stream.next().await {
        check_cancel()?;
        let seg = &segments[index];
        index += 1;
        match item {
            Ok((seg, audio, fit)) => {
                writer.write_at(seg.start_ms, &audio)?;
                stats.ok += 1;
                let kind = if fit.truncated {
                    stats.truncated += 1;
                    "truncated"
                } else if fit.stretched {
                    stats.compressed += 1;
                    "compressed"
                } else {
                    "natural"
                };
                progress.segment(seg.index, "done", kind, Some(audio.duration_ms), None);
            }
            Err(e) if is_cancel_error(&e) => return Err(e),
            Err(e) => {
                let msg = brief(&e.to_string());
                log::warn!("[dubbing] 第 {} 句合成失败，保留静音: {}", seg.index + 1, msg);
                stats.failed += 1;
                progress.segment(seg.index, "failed", "natural", None, Some(&msg));
                if stats.ok == 0 && stats.failed >= MAX_INITIAL_FAILS {
                    return Err(anyhow!(
                        "前 {} 句语音合成全部失败（{}）。请检查音色与 Fish Audio API Key 后重试",
                        stats.failed,
                        msg
                    ));
                }
            }
        }
        let finished = stats.ok + stats.failed;
        let pct = 3 + (finished * 85 / total.max(1)) as u32;
        progress.running(
            "tts",
            pct,
            &format!("已配音 {}/{} 句{}", finished, total, if stats.failed > 0 { format!("（{} 句失败）", stats.failed) } else { String::new() }),
            Some(finished),
            Some(total),
        );
    }

    let last_end = segments.last().map(|s| s.end_ms).unwrap_or(0);
    let total_ms = writer.cursor_ms().max(last_end);
    writer.finish(total_ms)?;
    Ok(stats)
}

// ===================== 单句试听 =====================

/// 合成单句试听（不贴合时长），返回 WAV 路径。与正式任务互不阻塞。
pub async fn preview_segment(
    config: &ConfigManager,
    text: &str,
    slot_ms: u64,
    overrides: Option<TtsOverrides>,
) -> Result<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return Err(anyhow!("这一句没有文字"));
    }
    let mut cfg = config.tts_config();
    if let Some(ov) = &overrides {
        ov.apply_to(&mut cfg);
    }
    let speed = tts_segments::estimate_fit_speed(text, slot_ms, cfg.speed.clamp(0.5, 2.0));
    let wav_cfg = tts_segments::wav_tts_config(&cfg);
    let audio = tts_segments::synthesize_segment(&FishTtsClient::new(), &wav_cfg, text, speed).await?;

    let dir = config.config_dir().join("dubbing_preview");
    tokio::fs::create_dir_all(&dir).await.context("创建试听目录失败")?;
    // 只保留最近一次试听
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    let path = dir.join(format!("line_{}.wav", uuid::Uuid::new_v4().simple()));
    let p2 = path.clone();
    blocking(move || tts_segments::write_track_wav(&p2, &audio)).await?;
    Ok(path)
}

// ===================== 辅助 =====================

fn brief(msg: &str) -> String {
    msg.chars().take(160).collect()
}

/// 计算输出路径：<output_dir|视频目录>/<stem>_dubbed.mp4（重名追加序号）
fn resolve_output_path(video: &Path, output_dir: Option<&str>) -> Result<PathBuf> {
    let parent = match output_dir.filter(|s| !s.trim().is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => video
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("无法确定输出目录"))?,
    };
    std::fs::create_dir_all(&parent).context("创建输出目录失败")?;
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
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
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(i: usize, a: u64, b: u64, t: &str) -> DubSegment {
        DubSegment::new(i, a, b, t.to_string())
    }

    #[test]
    fn room_extends_into_following_gap() {
        let segs = vec![seg(0, 0, 1000, "a"), seg(1, 1800, 2500, "b"), seg(2, 2400, 3000, "c")];
        let room = room_for(&segs);
        assert_eq!(room[0], 1800, "可借用到下一句开始");
        assert_eq!(room[1], 700, "下一句提前开始时至少保留自身时长");
        assert_eq!(room[2], 600 + TAIL_ROOM_MS);
    }

    #[test]
    fn clean_sorts_and_fixes_segments() {
        let segs = vec![seg(5, 3000, 2000, "后"), seg(9, 100, 900, "前"), seg(1, 50, 80, "  ")];
        let out = clean_segments(segs);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].index, out[0].text.as_str()), (0, "前"));
        assert_eq!((out[1].start_ms, out[1].end_ms), (3000, 3500));
    }

    /// 运行锁与取消标志是进程级静态量：放在同一个测试里顺序验证，避免并行测试互相干扰
    #[test]
    fn job_slot_and_cancel_lifecycle() {
        request_cancel();
        assert!(!is_cancel_requested(), "空闲时取消不应残留到下一个任务");
        let slot = JobSlot::acquire().expect("首次获取");
        assert!(is_running());
        assert!(JobSlot::acquire().is_err(), "持有期间不可重入");
        request_cancel();
        assert!(is_cancel_requested());
        drop(slot);
        assert!(!is_running());
        let again = JobSlot::acquire().expect("释放后可再次获取");
        assert!(!is_cancel_requested(), "新任务清除旧的取消标志");
        drop(again);
    }
}
