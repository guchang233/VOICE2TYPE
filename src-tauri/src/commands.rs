use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use crate::app_state::AppState;
use crate::config::{AppConfig, ConfigManager};
use crate::history;
use crate::tts::client::{FishTtsClient, VoiceListParams};

/// 全局下载取消标志：用户点击「取消」后置为 true，下载循环检测到后中断
static CANCEL_DOWNLOAD: AtomicBool = AtomicBool::new(false);

/// 用户自定义模型下载直链 base URL（不包含文件名）
/// 为空时选择"直链"源会返回错误提示
/// 用户提供直链后填入，格式如："https://example.com/whisper"
/// 下载时会自动拼接文件名：{CUSTOM_MODEL_BASE_URL}/{file_name}
const CUSTOM_MODEL_BASE_URL: &str = "";

/// 取消正在进行的下载（模型/引擎）
#[tauri::command]
pub fn cancel_download() {
    CANCEL_DOWNLOAD.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn get_config(config: tauri::State<'_, Arc<ConfigManager>>) -> AppConfig {
    config.get_config()
}

#[tauri::command]
pub fn save_config(
    config: tauri::State<'_, Arc<ConfigManager>>,
    new_config: AppConfig,
) -> Result<(), String> {
    config.set_config(new_config);
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_models_dir(config: tauri::State<'_, Arc<ConfigManager>>) -> String {
    config.current_models_dir()
}

/// 打开文件夹选择器，让用户选择模型下载目录
/// 使用非阻塞回调 API + oneshot channel，避免阻塞 async 运行时
/// 选择后立即保存到配置，并返回新目录路径
#[tauri::command]
pub async fn pick_models_directory(
    app: tauri::AppHandle,
    config: tauri::State<'_, Arc<ConfigManager>>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择模型下载目录")
        .pick_folder(move |folder| {
            let _ = tx.send(folder);
        });

    // 等待用户选择（非阻塞，不占用 async 运行时线程）
    let folder = rx.await.map_err(|e| format!("对话框通道错误: {}", e))?;

    match folder {
        Some(path) => {
            let path_str = path.to_string();
            config.set_custom_models_dir(path_str.clone());
            config.save().map_err(|e| e.to_string())?;
            Ok(Some(path_str))
        }
        None => Ok(None),
    }
}

/// 重置模型目录为默认值
#[tauri::command]
pub fn reset_models_directory(
    config: tauri::State<'_, Arc<ConfigManager>>,
) -> Result<String, String> {
    config.clear_custom_models_dir();
    config.save().map_err(|e| e.to_string())?;
    Ok(config.current_models_dir())
}

/// 在系统文件管理器中打开指定目录
/// 使用 OS 原生命令，避免 shell 插件 scope 限制
#[tauri::command]
pub async fn open_directory(path: String) -> Result<(), String> {
    let dir = std::path::PathBuf::from(&path);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .arg(&path)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| format!("打开目录失败: {}", e))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("打开目录失败: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("打开目录失败: {}", e))?;
    }

    Ok(())
}

#[tauri::command]
pub async fn start_recording(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.start_recording().await
}

#[tauri::command]
pub async fn stop_recording(state: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    state.stop_recording_and_recognize().await
}

#[tauri::command]
pub async fn cancel_recording(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.cancel_recording().await
}

#[tauri::command]
pub async fn get_history() -> Result<Vec<String>, String> {
    Ok(history::get_all())
}

#[tauri::command]
pub fn remove_history(index: usize) -> Result<bool, String> {
    Ok(history::remove(index))
}

#[tauri::command]
pub fn clear_history() -> Result<(), String> {
    history::clear();
    Ok(())
}

#[tauri::command]
pub async fn get_app_status(state: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    Ok(state.get_status().await)
}

#[tauri::command]
pub async fn copy_to_clipboard(text: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    unsafe {
        crate::win_utils::set_clipboard_text(&text, false);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = text;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_subtitle_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("subtitle") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub async fn start_subtitle(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.start_subtitle().await
}

#[tauri::command]
pub async fn stop_subtitle(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.stop_subtitle().await
}

#[tauri::command]
pub async fn is_subtitle_running(state: tauri::State<'_, Arc<AppState>>) -> Result<bool, String> {
    Ok(state.is_subtitle_running())
}

#[tauri::command]
pub async fn toggle_subtitle(state: tauri::State<'_, Arc<AppState>>) -> Result<bool, String> {
    state.toggle_subtitle().await
}

/// 枚举系统音频输入设备列表
/// 返回 (设备名列表, 默认设备名)
/// 前端用于填充语音输入和字幕识别音源的下拉选择器
#[tauri::command]
pub fn list_input_devices() -> Result<(Vec<String>, Option<String>), String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok());
    let mut devices = Vec::new();
    if let Ok(iter) = host.input_devices() {
        for d in iter {
            if let Ok(name) = d.name() {
                if !name.is_empty() {
                    devices.push(name);
                }
            }
        }
    }
    Ok((devices, default_name))
}

/// 计算文件的 SHA256 哈希（同步，分块读取）
fn compute_sha256(path: &std::path::Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    // 64KB buffer：大模型（1.5GB）校验从 ~3 分钟降到 ~10 秒级
    let mut buffer = [0u8; 65536];
    loop {
        let n = std::io::Read::read(&mut file, &mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 校验模型文件 SHA256 是否匹配官方哈希
/// 无官方哈希记录时返回 Ok(true)（跳过校验）
fn verify_model_hash(file_name: &str, path: &std::path::Path) -> Result<bool, String> {
    match crate::whisper_local::expected_sha256(file_name) {
        Some(expected) => {
            let computed = compute_sha256(path)?;
            Ok(computed == expected)
        }
        None => Ok(true),
    }
}

#[tauri::command]
pub async fn download_whisper_model(
    config: tauri::State<'_, Arc<ConfigManager>>,
    app: tauri::AppHandle,
    model_name: String,
    source: Option<String>,
) -> Result<String, String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    // 重置取消标志（每次新下载都从 false 开始）
    CANCEL_DOWNLOAD.store(false, Ordering::SeqCst);

    let model_dir = config.whisper_models_dir();
    std::fs::create_dir_all(&model_dir).map_err(|e| e.to_string())?;

    let model_paths = [
        ("tiny", "ggml-tiny.bin"),
        ("base", "ggml-base.bin"),
        ("small", "ggml-small.bin"),
        ("medium", "ggml-medium.bin"),
    ];
    let file_name = model_paths
        .iter()
        .find(|(k, _)| *k == model_name.as_str())
        .map(|(_, v)| *v)
        .ok_or_else(|| format!("未知模型: {}", model_name))?;

    let final_path = model_dir.join(file_name);
    let part_path = model_dir.join(format!("{}.part", file_name));

    // 如果最终文件已存在且哈希匹配，直接返回
    if final_path.exists() {
        let final_path_clone = final_path.clone();
        let hash_ok = tokio::task::spawn_blocking(move || verify_model_hash(file_name, &final_path_clone))
            .await
            .map_err(|e| e.to_string())??;

        if hash_ok {
            return Ok("模型已存在".to_string());
        }
        // 哈希不匹配：删除旧文件，重新下载
        let _ = std::fs::remove_file(&final_path);
    }

    // 根据用户选择的源构造下载地址列表
    // source="custom"：使用 CUSTOM_MODEL_BASE_URL 直链（待用户提供后填入）
    // source="hf" 或未指定：使用三级镜像源（HF-Mirror → ModelScope → HuggingFace）
    let mirror_urls: Vec<String> = match source.as_deref() {
        Some("custom") => {
            if CUSTOM_MODEL_BASE_URL.is_empty() {
                return Err("直链源尚未配置，请选择 HuggingFace 源或联系开发者提供直链".to_string());
            }
            vec![format!(
                "{}/{}",
                CUSTOM_MODEL_BASE_URL.trim_end_matches('/'),
                file_name
            )]
        }
        _ => {
            // 三级镜像源（HF-Mirror → ModelScope → HuggingFace）
            // ModelScope 使用社区镜像 cjc1887415157/whisper.cpp（SHA256 与官方一致，已验证）
            vec![
                format!("https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/{}", file_name),
                format!(
                    "https://modelscope.cn/api/v1/models/cjc1887415157/whisper.cpp/repo?Revision=master&FilePath={}",
                    file_name
                ),
                format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}", file_name),
            ]
        }
    };

    let mut errors: Vec<String> = Vec::new();

    for url in &mirror_urls {
        // 切换到下一镜像前检查是否已取消
        if CANCEL_DOWNLOAD.load(Ordering::SeqCst) {
            return Err("下载已取消".to_string());
        }
        log::info!("尝试下载模型: {}", url);

        // 检查已有 .part 文件大小（用于断点续传）
        let downloaded_so_far = std::fs::metadata(&part_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

        let mut request = client.get(url.to_string());
        if downloaded_so_far > 0 {
            request = request.header("Range", format!("bytes={}-", downloaded_so_far));
        }

        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("{}: {}", url, e);
                log::warn!("下载失败: {}", msg);
                errors.push(msg);
                continue;
            }
        };

        let status = response.status();
        let is_partial = status == reqwest::StatusCode::PARTIAL_CONTENT; // 206

        // 非 206 且非成功状态
        if !is_partial && !status.is_success() {
            let msg = format!("{}: HTTP {}", url, status);
            log::warn!("下载失败: {}", msg);
            errors.push(msg);
            continue;
        }

        // 确定总大小和起始位置
        let (total_size, start_pos): (u64, u64) = if is_partial {
            // 206：从 Content-Range 解析总大小
            let total = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split('/').nth(1))
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            (total, downloaded_so_far)
        } else {
            // 200：完整下载（服务器不支持 Range 或首次下载）
            let total = response.content_length().unwrap_or(0);
            (total, 0)
        };

        if !is_partial && downloaded_so_far > 0 {
            log::info!("服务器不支持断点续传，从头下载");
        }

        // 打开文件：206 追加，200 覆盖
        let mut file = if is_partial {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&part_path)
                .await
                .map_err(|e| e.to_string())?
        } else {
            tokio::fs::File::create(&part_path)
                .await
                .map_err(|e| e.to_string())?
        };

        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = start_pos;
        let bytes_at_start = start_pos;
        let start_time = std::time::Instant::now();
        let mut last_emit_time = std::time::Instant::now();

        let mut stream_ok = true;
        let mut stream_err_msg = String::new();
        let mut cancelled = false;

        while let Some(chunk_result) = stream.next().await {
            // 用户点击取消：中断下载（保留 .part 文件以便续传）
            if CANCEL_DOWNLOAD.load(Ordering::SeqCst) {
                cancelled = true;
                stream_ok = false;
                break;
            }
            match chunk_result {
                Ok(chunk) => {
                    if let Err(e) = file.write_all(&chunk).await {
                        stream_err_msg = format!("写入文件失败: {}", e);
                        stream_ok = false;
                        break;
                    }
                    downloaded += chunk.len() as u64;

                    // 每 ~500ms 发送一次进度
                    let now = std::time::Instant::now();
                    if now.duration_since(last_emit_time).as_millis() >= 500 {
                        let elapsed = start_time.elapsed().as_secs_f64();
                        let bytes_this_session = downloaded.saturating_sub(bytes_at_start);
                        let speed = if elapsed > 0.0 {
                            bytes_this_session as f64 / elapsed / 1_048_576.0
                        } else {
                            0.0
                        };
                        let eta = if speed > 0.0 && total_size > downloaded {
                            ((total_size - downloaded) as f64 / (speed * 1_048_576.0)) as u64
                        } else {
                            0
                        };
                        // total_size 已知时按比例；未知时发 0（前端改显已下载 MB）
                        let progress = if total_size > 0 {
                            (downloaded as f64 / total_size as f64 * 100.0) as u32
                        } else {
                            0
                        };

                        let _ = app.emit(
                            "model-download-progress",
                            serde_json::json!({
                                "model": model_name,
                                "progress": progress,
                                "downloaded": downloaded,
                                "total": total_size,
                                "speed": speed,
                                "eta": eta
                            }),
                        );
                        last_emit_time = now;
                    }
                }
                Err(e) => {
                    stream_err_msg = format!("流读取错误: {}", e);
                    stream_ok = false;
                    break;
                }
            }
        }

        // 刷新并关闭文件
        let _ = file.flush().await;
        drop(file);

        if !stream_ok {
            // 用户主动取消：不污染 errors，直接返回
            if cancelled {
                return Err("下载已取消".to_string());
            }
            log::warn!("下载中断 {}: {}", url, stream_err_msg);
            errors.push(format!("{}: {}", url, stream_err_msg));
            // 保留 .part 文件以便下次续传
            continue;
        }

        // 下载完成，校验 SHA256（在 spawn_blocking 中执行避免阻塞）
        let part_path_clone = part_path.clone();
        let hash_ok = tokio::task::spawn_blocking(move || verify_model_hash(file_name, &part_path_clone))
            .await
            .map_err(|e| e.to_string())??;

        if !hash_ok {
            // 哈希不匹配，删除损坏的 .part 文件
            let _ = std::fs::remove_file(&part_path);
            let msg = format!("{}: SHA256 校验失败，文件已损坏", url);
            log::warn!("{}", msg);
            errors.push(msg);
            continue;
        }

        // 校验通过，重命名 .part 为最终文件名
        std::fs::rename(&part_path, &final_path).map_err(|e| e.to_string())?;
        log::info!("模型下载完成: {}", final_path.display());

        // 发送 100% 进度
        let _ = app.emit(
            "model-download-progress",
            serde_json::json!({
                "model": model_name,
                "progress": 100,
                "downloaded": total_size,
                "total": total_size,
                "speed": 0.0,
                "eta": 0
            }),
        );

        return Ok(format!("模型下载完成: {}", final_path.display()));
    }

    // 用户取消时优先返回取消提示
    if CANCEL_DOWNLOAD.load(Ordering::SeqCst) {
        return Err("下载已取消".to_string());
    }
    // 所有镜像源均失败（不删除 .part 文件，允许未来续传）
    Err(format!("所有下载源均失败:\n{}", errors.join("\n")))
}

#[tauri::command]
pub async fn list_available_models(
    config: tauri::State<'_, Arc<ConfigManager>>,
) -> Result<Vec<serde_json::Value>, String> {
    let model_dir = config.whisper_models_dir();
    log::info!("[list_available_models] 扫描目录: {}", model_dir.display());

    let models = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let mut models = Vec::new();
        if !model_dir.exists() {
            log::warn!("[list_available_models] 目录不存在: {}", model_dir.display());
            return Ok(models);
        }
        let entries = std::fs::read_dir(&model_dir).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "bin").unwrap_or(false) {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                let size = path.metadata().map(|m| m.len()).unwrap_or(0);
                let available = verify_model_hash(&name, &path).unwrap_or(false);
                models.push(serde_json::json!({
                    "name": name,
                    "size": size,
                    "available": available
                }));
            }
        }
        Ok(models)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(models)
}

/// 删除已下载的 Whisper 模型文件
/// model_name 为完整文件名（如 ggml-tiny.bin）
#[tauri::command]
pub async fn delete_whisper_model(
    config: tauri::State<'_, Arc<ConfigManager>>,
    model_name: String,
) -> Result<String, String> {
    // 安全校验：只允许删除 ggml-*.bin 文件名
    if !model_name.starts_with("ggml-") || !model_name.ends_with(".bin") {
        return Err(format!("非法模型文件名: {}（仅允许 ggml-*.bin）", model_name));
    }
    // 防路径穿越
    if model_name.contains('/') || model_name.contains('\\') || model_name.contains("..") {
        return Err("模型文件名包含非法字符".to_string());
    }

    let model_dir = config.whisper_models_dir();
    let model_path = model_dir.join(&model_name);

    if !model_path.exists() {
        return Err(format!("模型文件不存在: {}", model_path.display()));
    }

    // 同步删除（小操作，直接 spawn_blocking）
    let path_clone = model_path.clone();
    let name_clone = model_name.clone();
    tokio::task::spawn_blocking(move || -> Result<String, String> {
        std::fs::remove_file(&path_clone)
            .map(|_| format!("已删除模型: {}", name_clone))
            .map_err(|e| format!("删除失败: {}", e))
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// whisper-bin-x64.zip 预期大小（v1.9.2，来自 GitHub API，纯 CPU 无 BLAS 版）
const WHISPER_BIN_ZIP_SIZE: u64 = 8_194_445;
/// whisper-bin-x64.zip 预期 SHA256（v1.9.2，纯 CPU 无 BLAS 版）
const WHISPER_BIN_ZIP_SHA256: &str = "49dcc16de826f20bd53d44f947a1ae49dfa81f86cad67a64d80820cb192d674a";

/// 下载 whisper.cpp 预编译二进制（Windows: whisper-bin-x64.zip，纯 CPU 无 BLAS）
/// force=true 时先删除现有二进制目录内容再重新下载
#[tauri::command]
pub async fn download_whisper_binary(
    config: tauri::State<'_, Arc<ConfigManager>>,
    app: tauri::AppHandle,
    force: Option<bool>,
) -> Result<String, String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let binary_dir = config.whisper_binary_dir();
    let binary_path = config.whisper_binary_path();

    // force=true 时清理现有二进制目录内容
    if force.unwrap_or(false) && binary_dir.exists() {
        log::info!("强制重新下载，清理二进制目录: {}", binary_dir.display());
        if let Ok(entries) = std::fs::read_dir(&binary_dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    std::fs::create_dir_all(&binary_dir).map_err(|e| e.to_string())?;

    // 已存在且未强制下载，直接返回
    if !force.unwrap_or(false) && binary_path.exists() {
        return Ok("引擎已存在".to_string());
    }

    // 多镜像源：GitHub 代理优先（国内可达），直连兜底
    // 使用纯 CPU 版本（无 BLAS）：BLAS 版启动时要初始化线程池 + 加载 libopenblas.dll，
    // 对 tiny 模型小矩阵无加速收益却增加 50-150ms 启动税。纯 CPU 版启动快、依赖少，
    // 契合轻量化全天使用场景。
    let mirror_urls = [
        "https://ghgo.xyz/https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.2/whisper-bin-x64.zip",
        "https://gh-proxy.com/https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.2/whisper-bin-x64.zip",
        "https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.2/whisper-bin-x64.zip",
    ];

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let temp_zip =
        std::env::temp_dir().join(format!("whisper-bin-{}.zip", uuid::Uuid::new_v4()));

    let mut errors: Vec<String> = Vec::new();
    let mut download_ok = false;

    for url in &mirror_urls {
        log::info!("尝试下载引擎: {}", url);

        let response = match client.get(*url).send().await {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("{}: {}", url, e);
                log::warn!("下载失败: {}", msg);
                errors.push(msg);
                continue;
            }
        };

        if !response.status().is_success() {
            let msg = format!("{}: HTTP {}", url, response.status());
            log::warn!("下载失败: {}", msg);
            errors.push(msg);
            continue;
        }

        let total_size = response.content_length().unwrap_or(0);

        // 预检：Content-Length 明显不对（<1MB，预期 ~8MB）直接跳过此源
        if total_size > 0 && total_size < 1_000_000 {
            let msg = format!("{}: Content-Length 异常 ({} bytes，预期 ~8MB)，可能为代理错误页", url, total_size);
            log::warn!("{}", msg);
            errors.push(msg);
            continue;
        }

        // 下载到临时文件
        let mut file = match tokio::fs::File::create(&temp_zip).await {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("{}: 创建临时文件失败: {}", url, e);
                errors.push(msg);
                continue;
            }
        };

        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = 0;
        let mut last_emit_time = std::time::Instant::now();
        let mut stream_ok = true;

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    if let Err(e) = file.write_all(&chunk).await {
                        errors.push(format!("{}: 写入文件失败: {}", url, e));
                        stream_ok = false;
                        break;
                    }
                    downloaded += chunk.len() as u64;

                    let now = std::time::Instant::now();
                    if now.duration_since(last_emit_time).as_millis() >= 500 {
                        let progress = if total_size > 0 {
                            (downloaded as f64 / total_size as f64 * 100.0) as u32
                        } else {
                            0
                        };
                        let _ = app.emit(
                            "binary-download-progress",
                            serde_json::json!({
                                "progress": progress,
                                "downloaded": downloaded,
                                "total": total_size
                            }),
                        );
                        last_emit_time = now;
                    }
                }
                Err(e) => {
                    errors.push(format!("{}: 流读取错误: {}", url, e));
                    stream_ok = false;
                    break;
                }
            }
        }

        if let Err(e) = file.flush().await {
            errors.push(format!("{}: 刷新文件失败: {}", url, e));
            continue;
        }
        drop(file);

        if !stream_ok {
            continue;
        }

        log::info!("引擎下载完成 ({} bytes)，验证完整性: {}", downloaded, url);

        // 验证 1：文件大小（允许 ±5% 误差，防止 CDN 压缩差异）
        let size_min = WHISPER_BIN_ZIP_SIZE * 95 / 100;
        let size_max = WHISPER_BIN_ZIP_SIZE * 110 / 100;
        if downloaded < size_min || downloaded > size_max {
            let msg = format!(
                "{}: 文件大小不匹配 ({} bytes，预期 {} bytes)，可能为代理返回的错误内容",
                url, downloaded, WHISPER_BIN_ZIP_SIZE
            );
            log::warn!("{}", msg);
            errors.push(msg);
            let _ = std::fs::remove_file(&temp_zip);
            continue;
        }

        // 验证 2：SHA256 校验（确保内容完整且未被篡改）
        let temp_zip_hash = temp_zip.clone();
        let expected_hash = WHISPER_BIN_ZIP_SHA256.to_string();
        let hash_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let hash = compute_sha256(&temp_zip_hash).map_err(|e| e.to_string())?;
            if hash != expected_hash {
                return Err(format!("SHA256 校验失败: {} != {}", hash, expected_hash));
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?;

        if let Err(e) = hash_result {
            let msg = format!("{}: {}", url, e);
            log::warn!("{}", msg);
            errors.push(msg);
            let _ = std::fs::remove_file(&temp_zip);
            continue;
        }

        // 验证 3：zip 文件有效性
        let temp_zip_verify = temp_zip.clone();
        let zip_valid = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let zip_file =
                std::fs::File::open(&temp_zip_verify).map_err(|e| e.to_string())?;
            let _archive =
                zip::ZipArchive::new(zip_file).map_err(|e| format!("无效的 zip 文件: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?;

        if let Err(e) = zip_valid {
            let msg = format!("{}: {}", url, e);
            log::warn!("{}", msg);
            errors.push(msg);
            let _ = std::fs::remove_file(&temp_zip);
            continue;
        }

        log::info!("引擎下载验证通过: {}", url);
        download_ok = true;
        break;
    }

    if !download_ok {
        return Err(format!("所有镜像源下载均失败:\n{}", errors.join("\n")));
    }

    // 解压 zip 文件，提取所有文件到 binary_dir
    let temp_zip_clone = temp_zip.clone();
    let binary_dir_clone = binary_dir.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let zip_file = std::fs::File::open(&temp_zip_clone).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(zip_file).map_err(|e| e.to_string())?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().to_string();
            // 跳过目录
            if name.ends_with('/') {
                continue;
            }
            // 提取文件名（去除 zip 内可能的目录层级）
            let file_name = std::path::Path::new(&name)
                .file_name()
                .map(|f| f.to_owned())
                .ok_or_else(|| format!("无效的文件名: {}", name))?;
            let out_path = binary_dir_clone.join(&file_name);
            let mut out_file = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;

    // 清理临时文件
    let _ = std::fs::remove_file(&temp_zip);

    // 验证 whisper-cli.exe 存在
    // 注意：whisper-cli.exe 本身约 479KB，且依赖 whisper.dll/ggml.dll，
    // 因此不做大小阈值判断，只验证可执行文件和关键 DLL 是否齐全
    if !binary_path.exists() {
        return Err("解压完成但未找到 whisper-cli.exe".to_string());
    }
    let binary_dir_check = config.whisper_binary_dir();
    #[cfg(target_os = "windows")]
    {
        let whisper_dll = binary_dir_check.join("whisper.dll");
        let ggml_dll = binary_dir_check.join("ggml.dll");
        if !whisper_dll.exists() {
            return Err("解压完成但未找到 whisper.dll".to_string());
        }
        if !ggml_dll.exists() {
            return Err("解压完成但未找到 ggml.dll".to_string());
        }
    }

    log::info!("whisper.cpp 二进制下载完成: {}", binary_path.display());

    // 发送 100% 进度
    let _ = app.emit(
        "binary-download-progress",
        serde_json::json!({
            "progress": 100,
            "downloaded": WHISPER_BIN_ZIP_SIZE,
            "total": WHISPER_BIN_ZIP_SIZE
        }),
    );

    Ok(format!("引擎下载完成: {}", binary_path.display()))
}

/// 检查 whisper.cpp 二进制健康状态（存在性、配套 DLL 是否齐全）
/// 注意：whisper-cli.exe 约 479KB，且依赖 whisper.dll/ggml.dll，
/// 因此不能用 >500KB 的大小阈值判断完整性，改为检查关键 DLL 是否存在。
#[tauri::command]
pub fn check_whisper_binary_health(
    config: tauri::State<'_, Arc<ConfigManager>>,
) -> serde_json::Value {
    let binary_path = config.whisper_binary_path();
    let binary_dir = config.whisper_binary_dir();

    if !binary_path.exists() {
        return serde_json::json!({
            "status": "missing",
            "message": "引擎未下载",
            "size": 0
        });
    }

    let size = binary_path.metadata().map(|m| m.len()).unwrap_or(0);

    // 列出二进制目录下所有文件
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&binary_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files.push(serde_json::json!({
                "name": name,
                "size": file_size
            }));
        }
    }

    // Windows 下检查关键配套 DLL 是否存在
    #[cfg(target_os = "windows")]
    {
        let whisper_dll = binary_dir.join("whisper.dll");
        let ggml_dll = binary_dir.join("ggml.dll");
        if !whisper_dll.exists() || !ggml_dll.exists() {
            let mut missing = Vec::new();
            if !whisper_dll.exists() {
                missing.push("whisper.dll");
            }
            if !ggml_dll.exists() {
                missing.push("ggml.dll");
            }
            return serde_json::json!({
                "status": "corrupt",
                "message": format!("缺少关键依赖: {}，请重新下载引擎", missing.join(", ")),
                "size": size,
                "files": files
            });
        }
    }

    serde_json::json!({
        "status": "ok",
        "message": "引擎就绪",
        "size": size,
        "files": files
    })
}

#[tauri::command]
pub async fn check_update() -> Result<serde_json::Value, String> {
    let result = tokio::task::spawn_blocking(|| crate::update::check_update())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "has_update": result.has_update,
        "current_version": crate::update::current_version(),
        "version": result.info.version,
        "body": result.info.body,
        "download_url": result.info.download_url,
        "filename": result.info.filename,
        "date": result.info.date
    }))
}

#[tauri::command]
pub async fn download_and_install_update(
    app_handle: tauri::AppHandle,
    url: String,
    filename: String,
) -> Result<(), String> {
    use std::sync::Arc;
    use tauri::Emitter;

    // 临时目录存放下载文件
    let temp_dir = std::env::temp_dir();
    let dest = temp_dir.join(if filename.is_empty() { "voice2type-update.exe".to_string() } else { filename });

    let app_clone = Arc::new(app_handle);
    let dest_clone = dest.clone();
    let url_clone = url.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let app_for_progress = app_clone.clone();
        crate::update::download_file(&url_clone, &dest_clone, move |done, total| {
            let _ = app_for_progress.emit("update-download-progress", serde_json::json!({
                "downloaded": done,
                "total": total
            }));
        }).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;

    // 安装新二进制
    crate::update::install_update(&dest).map_err(|e| e.to_string())?;

    // 删除临时文件（如果还存在）
    let _ = std::fs::remove_file(&dest);

    Ok(())
}

#[tauri::command]
pub fn restart_app(app_handle: tauri::AppHandle) {
    // 重启应用：先启动新进程，再退出当前
    let current = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("voice2type.exe"));
    let _ = std::process::Command::new(current).spawn();
    app_handle.exit(0);
}

#[tauri::command]
pub fn get_app_version() -> String {
    crate::update::current_version()
}

/// 设置字幕窗口置顶
#[tauri::command]
pub fn set_subtitle_always_on_top(app_handle: tauri::AppHandle, on_top: bool) -> Result<(), String> {
    if let Some(window) = app_handle.get_webview_window("subtitle") {
        window
            .set_always_on_top(on_top)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 设置字幕窗口点击穿透
#[tauri::command]
pub fn set_subtitle_click_through(app_handle: tauri::AppHandle, click_through: bool) -> Result<(), String> {
    if let Some(window) = app_handle.get_webview_window("subtitle") {
        window
            .set_ignore_cursor_events(click_through)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 显示/隐藏字幕窗口
#[tauri::command]
pub fn show_subtitle_window(app_handle: tauri::AppHandle, show: bool) -> Result<(), String> {
    if let Some(window) = app_handle.get_webview_window("subtitle") {
        if show {
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
        } else {
            window.hide().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 主动推送当前字幕样式配置到字幕窗口（用于实时字幕设置变更后即时应用）
#[tauri::command]
pub fn push_subtitle_config(
    app_handle: tauri::AppHandle,
    config: tauri::State<'_, Arc<ConfigManager>>,
) -> Result<(), String> {
    use tauri::Emitter;
    let cfg = config.get_config();
    let sub_cfg = &cfg.subtitle;

    let _ = app_handle.emit("subtitle-config", serde_json::json!({
        "fontFamily": sub_cfg.subtitle_font_family,
        "fontSize": sub_cfg.subtitle_font_size,
        "fontWeight": sub_cfg.subtitle_font_weight,
        "bold": sub_cfg.subtitle_font_weight >= 700,
        "italic": sub_cfg.subtitle_italic,
        "fontColor": sub_cfg.subtitle_font_color,
        "textAlign": sub_cfg.subtitle_text_align,
        "letterSpacing": sub_cfg.subtitle_letter_spacing,
        "lineHeight": sub_cfg.subtitle_line_height,
        "textShadowColor": sub_cfg.subtitle_text_shadow_color,
        "textShadowStrength": sub_cfg.subtitle_text_shadow_strength,
        "bgColor": sub_cfg.subtitle_bg_color,
        "bgOpacity": sub_cfg.subtitle_bg_opacity,
        "blur": sub_cfg.subtitle_blur,
        "borderRadius": sub_cfg.subtitle_border_radius,
        "borderColor": sub_cfg.subtitle_border_color,
        "borderWidth": sub_cfg.subtitle_border_width,
        "paddingX": sub_cfg.subtitle_padding_x,
        "paddingY": sub_cfg.subtitle_padding_y,
        "maxLines": sub_cfg.subtitle_max_lines,
        "interimColor": sub_cfg.subtitle_interim_color,
        "interimOpacity": sub_cfg.subtitle_interim_opacity,
    }));
    Ok(())
}

/// 查询字幕窗口当前状态
#[tauri::command]
pub fn get_subtitle_window_status(app_handle: tauri::AppHandle) -> serde_json::Value {
    use tauri::Manager;
    let visible = app_handle
        .get_webview_window("subtitle")
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    serde_json::json!({ "visible": visible })
}

/// 设置字幕窗口 OBS 捕捉兼容模式
/// 由于已通过 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--disable-direct-composition 全局禁用 DComp，
/// OBS 默认即可捕捉窗口内容。此命令现在仅用于：
/// 1. 切换字幕窗口 body 背景渲染模式（CSS）
/// 2. 在 OBS 模式下确保窗口置顶以便录屏
#[tauri::command]
pub async fn set_subtitle_obs_mode(
    app_handle: tauri::AppHandle,
    obs_mode: bool,
) -> Result<(), String> {
    use tauri::{Emitter, Manager};
    if let Some(window) = app_handle.get_webview_window("subtitle") {
        let _ = app_handle.emit("subtitle-obs-mode", serde_json::json!({ "enabled": obs_mode }));
        if obs_mode {
            let _ = window.set_always_on_top(true);
        }
    }
    Ok(())
}

// ===================== Fish Audio TTS（文本转语音）=====================

/// 根据输出格式返回文件扩展名
fn tts_format_ext(format: &str) -> &str {
    match format {
        "wav" => "wav",
        "pcm" => "pcm",
        "opus" => "opus",
        _ => "mp3",
    }
}

/// 文本转语音（生成）：合成音频并写入临时文件，返回文件路径。
/// 前端可用 `convertFileSrc` 将该路径转为可在 <audio> 中播放的 URL。
///
/// 每次合成都写入唯一文件名（UUID），避免浏览器/asset 协议因 URL
/// 未变而缓存旧音频，导致再次生成时播放器仍播放上一条。
#[tauri::command]
pub async fn tts_synthesize(
    config: tauri::State<'_, Arc<ConfigManager>>,
    text: String,
) -> Result<String, String> {
    let tts_cfg = config.tts_config();
    let client = FishTtsClient::new();

    let bytes = client.synthesize(&text, &tts_cfg).await.map_err(|e| e.to_string())?;

    // 写入配置目录下的 tts/ 目录
    let mut dir = config.config_dir();
    dir.push("tts");
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("创建 TTS 目录失败: {}", e))?;

    let ext = tts_format_ext(&tts_cfg.format);

    // 清理旧的生成文件（所有 preview_* 前缀，不论扩展名），避免磁盘堆积
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with("preview_") {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
    }

    // 使用 UUID 生成唯一文件名，确保 URL 改变，强制 <audio> 重新加载
    let id = uuid::Uuid::new_v4();
    let path = dir.join(format!("preview_{}.{}", id, ext));
    tokio::fs::write(&path, &bytes).await.map_err(|e| format!("写入生成文件失败: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}

/// 文本转语音（导出/下载）：将已合成的生成文件复制到用户选择的路径。
/// 返回保存路径；用户取消则返回 None。
/// 采用「复制生成文件」而非重新合成，保证下载内容与生成完全一致。
#[tauri::command]
pub async fn tts_export(
    app: tauri::AppHandle,
    config: tauri::State<'_, Arc<ConfigManager>>,
    src_path: String,
    file_name: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    // 安全校验：src_path 必须位于 TTS 目录内，防止路径遍历
    let mut tts_dir = config.config_dir();
    tts_dir.push("tts");
    let canonical_src = std::path::Path::new(&src_path)
        .canonicalize()
        .map_err(|e| format!("源文件不存在或无法访问: {}", e))?;
    let canonical_dir = tts_dir
        .canonicalize()
        .map_err(|e| format!("TTS 目录无法访问: {}", e))?;
    if !canonical_src.starts_with(&canonical_dir) {
        return Err("源文件不在 TTS 目录内".to_string());
    }

    // 从源文件扩展名推断保存格式
    let ext = std::path::Path::new(&src_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3")
        .to_string();

    let default_name = if file_name.is_empty() {
        format!("tts_output.{}", ext)
    } else if std::path::Path::new(&file_name).extension().is_some() {
        file_name
    } else {
        format!("{}.{}", file_name, ext)
    };

    // 弹出保存对话框（非阻塞回调 + oneshot channel）
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("保存语音文件")
        .add_filter("音频", &[ext.as_str()])
        .set_file_name(&default_name)
        .save_file(move |path| {
            let _ = tx.send(path);
        });

    let chosen = rx.await.map_err(|e| format!("对话框通道错误: {}", e))?;
    let dest = match chosen {
        Some(p) => p.to_string(),
        None => return Ok(None),
    };

    // 复制生成文件到目标路径
    tokio::fs::copy(&src_path, &dest).await.map_err(|e| format!("保存文件失败: {}", e))?;

    Ok(Some(dest))
}

/// 查询 Fish Audio 官方音色库（GET /model），返回原始 JSON。
#[tauri::command]
pub async fn tts_list_voices(
    config: tauri::State<'_, Arc<ConfigManager>>,
    page_size: Option<u32>,
    page_number: Option<u32>,
    title: Option<String>,
    language: Option<String>,
    sort_by: Option<String>,
    self_only: Option<bool>,
) -> Result<serde_json::Value, String> {
    let tts_cfg = config.tts_config();
    let client = FishTtsClient::new();
    let params = VoiceListParams {
        page_size: page_size.unwrap_or(20),
        page_number: page_number.unwrap_or(1),
        title: title.unwrap_or_default(),
        language: language.unwrap_or_default(),
        sort_by: sort_by.unwrap_or_else(|| "score".to_string()),
        self_only: self_only.unwrap_or(false),
    };
    client
        .list_voices(&tts_cfg.fish_api_key, &params)
        .await
        .map_err(|e| e.to_string())
}

/// 获取单个音色详情（GET /model/{id}）。
#[tauri::command]
pub async fn tts_get_voice(
    config: tauri::State<'_, Arc<ConfigManager>>,
    voice_id: String,
) -> Result<serde_json::Value, String> {
    let tts_cfg = config.tts_config();
    let client = FishTtsClient::new();
    client
        .get_voice(&tts_cfg.fish_api_key, &voice_id)
        .await
        .map_err(|e| e.to_string())
}
