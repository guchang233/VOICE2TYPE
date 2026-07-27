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
    config.models_dir().to_string_lossy().into_owned()
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
