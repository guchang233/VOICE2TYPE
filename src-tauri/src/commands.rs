use std::sync::Arc;
use tauri::{Emitter, Manager};
use crate::app_state::AppState;
use crate::config::{AppConfig, ConfigManager};
use crate::history;

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
/// 选择后立即保存到配置，并返回新目录路径
#[tauri::command]
pub async fn pick_models_directory(
    app: tauri::AppHandle,
    config: tauri::State<'_, Arc<ConfigManager>>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let folder = app
        .dialog()
        .file()
        .set_title("选择模型下载目录")
        .blocking_pick_folder();

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

#[tauri::command]
pub async fn download_whisper_model(
    config: tauri::State<'_, Arc<ConfigManager>>,
    app: tauri::AppHandle,
    model_name: String,
) -> Result<String, String> {
    let model_dir = config.whisper_models_dir();
    std::fs::create_dir_all(&model_dir).map_err(|e| e.to_string())?;

    let model_url = match model_name.as_str() {
        "tiny" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        "base" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        "small" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        "medium" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        _ => return Err(format!("Unknown model: {}", model_name)),
    };

    let output_path = model_dir.join(format!("ggml-{}.bin", model_name));

    if output_path.exists() {
        return Ok(format!("Model already exists at {:?}", output_path));
    }

    let client = reqwest::Client::new();
    let response = client.get(model_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    use futures_util::StreamExt;
    let mut file = tokio::fs::File::create(&output_path).await.map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        downloaded += chunk.len() as u64;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await.map_err(|e| e.to_string())?;

        if total_size > 0 {
            let progress = (downloaded as f64 / total_size as f64 * 100.0) as u32;
            let _ = app.emit("model-download-progress", serde_json::json!({
                "model": model_name,
                "progress": progress,
                "downloaded": downloaded,
                "total": total_size
            }));
        }
    }

    Ok(format!("Model downloaded to {:?}", output_path))
}

#[tauri::command]
pub async fn list_available_models(config: tauri::State<'_, Arc<ConfigManager>>) -> Result<Vec<serde_json::Value>, String> {
    let model_dir = config.whisper_models_dir();
    let mut models = Vec::new();

    if model_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&model_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "bin").unwrap_or(false) {
                    let size = path.metadata().map(|m| m.len()).unwrap_or(0);
                    models.push(serde_json::json!({
                        "name": path.file_name().unwrap().to_string_lossy(),
                        "path": path.to_string_lossy(),
                        "size": size
                    }));
                }
            }
        }
    }

    Ok(models)
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
