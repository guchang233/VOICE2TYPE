//! Fish Audio 文本转语音（TTS）客户端。
//!
//! 封装 Fish Audio REST API 的三类调用：
//! - [`FishTtsClient::synthesize`]：`POST /v1/tts`，文本 → 音频字节（model 通过请求头传递）
//! - [`FishTtsClient::list_voices`]：`GET /model`，分页/搜索官方音色库
//! - [`FishTtsClient::get_voice`]：`GET /model/{id}`，获取单个音色详情
//!
//! API 文档：https://docs.fish.audio
//! Base URL：https://api.fish.audio
//! 鉴权：`Authorization: Bearer <api_key>`

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

use crate::config::TtsConfig;

/// Fish Audio API 基础地址（Bearer token 鉴权）
const BASE_URL: &str = "https://api.fish.audio";

/// 读取 Windows 系统代理设置（从注册表）。
/// reqwest 默认只读环境变量，不读 Windows 注册表，需手动配置。
fn get_system_proxy() -> Option<String> {
    // 优先检查环境变量
    for var in &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    // Windows 注册表回退
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
            REG_DWORD, REG_SZ, REG_VALUE_TYPE,
        };
        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\0"
            .encode_utf16()
            .collect();
        unsafe {
            let mut hkey: HKEY = HKEY::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                0,
                KEY_READ,
                &mut hkey,
            )
            .is_err()
            {
                return None;
            }
            // 读取 ProxyEnable (DWORD)
            let enable_name: Vec<u16> = "ProxyEnable\0".encode_utf16().collect();
            let mut enable_val: u32 = 0;
            let mut enable_len: u32 = 4;
            let mut ty = REG_VALUE_TYPE(0);
            let _ = RegQueryValueExW(
                hkey,
                PCWSTR(enable_name.as_ptr()),
                None,
                Some(&mut ty),
                Some(&mut enable_val as *mut u32 as *mut u8),
                Some(&mut enable_len),
            );
            if ty != REG_DWORD || enable_val == 0 {
                let _ = RegCloseKey(hkey);
                return None;
            }
            // 读取 ProxyServer (REG_SZ)
            let server_name: Vec<u16> = "ProxyServer\0".encode_utf16().collect();
            let mut buf = [0u8; 1024];
            let mut buf_len: u32 = buf.len() as u32;
            let status = RegQueryValueExW(
                hkey,
                PCWSTR(server_name.as_ptr()),
                None,
                None,
                Some(buf.as_mut_ptr()),
                Some(&mut buf_len),
            );
            let _ = RegCloseKey(hkey);
            if status.is_err() {
                return None;
            }
            // 安全限制：API 可能将 buf_len 改写为大于 buf 的值（ERROR_MORE_DATA）
            let safe_len = (buf_len as usize).min(buf.len()) / 2;
            let server = String::from_utf16_lossy(std::slice::from_raw_parts(
                buf.as_ptr() as *const u16,
                safe_len,
            ))
            .trim_end_matches('\0')
            .to_string();
            if server.is_empty() {
                return None;
            }
            // ProxyServer 可能是 "http=127.0.0.1:8080;https=127.0.0.1:8080;socks=127.0.0.1:1080" 格式
            if server.contains('=') {
                for part in server.split(';') {
                    let (scheme, addr) = if let Some(a) = part.strip_prefix("https=") {
                        ("https", a)
                    } else if let Some(a) = part.strip_prefix("http=") {
                        ("http", a)
                    } else if let Some(a) = part.strip_prefix("socks=") {
                        ("socks5", a)
                    } else {
                        continue;
                    };
                    if !addr.is_empty() {
                        return Some(if addr.contains("://") {
                            addr.to_string()
                        } else {
                            format!("{}://{}", scheme, addr)
                        });
                    }
                }
                // 所有部分均未匹配已知协议，不使用代理
                return None;
            }
            return Some(if server.contains("://") {
                server
            } else {
                format!("http://{}", server)
            });
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 移除代理 URL 中的凭据（user:pass@）部分，用于日志记录。
fn sanitize_proxy_url(url: &str) -> String {
    if let Some(idx) = url.find("://") {
        let rest = &url[idx + 3..];
        if let Some(at_idx) = rest.find('@') {
            return format!("{}{}", &url[..idx + 3], &rest[at_idx + 1..]);
        }
    }
    url.to_string()
}

/// 获取 TTS HTTP 客户端：动态检测系统代理。
/// - 总超时 5 分钟（音频合成较慢）；连接超时 15 秒（快速失败，不挂死）
/// - 代理走 HTTP CONNECT，域名解析在代理远端进行，不受本地 DNS 污染影响
fn get_tts_client() -> Client {
    let proxy_url = get_system_proxy();

    let mut builder = Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(15));
    match &proxy_url {
        Some(p) => {
            log::info!("[tts] 使用系统代理: {}", sanitize_proxy_url(p));
            match reqwest::Proxy::all(p) {
                Ok(proxy) => builder = builder.proxy(proxy),
                Err(e) => log::warn!("[tts] 代理配置无效，回退直连: {}", e),
            }
        }
        None => log::info!("[tts] 未检测到系统代理，使用直连"),
    }
    builder.build().expect("failed to build TTS HTTP client")
}

/// 音色库搜索/列表参数。
#[derive(Debug, Clone, Default)]
pub struct VoiceListParams {
    pub page_size: u32,
    pub page_number: u32,
    pub title: String,
    pub language: String,
    pub sort_by: String,
    /// true = 仅查询当前工作区自建音色
    pub self_only: bool,
}

#[derive(Clone, Default)]
pub struct FishTtsClient {
    base_url: String,
}

impl FishTtsClient {
    pub fn new() -> Self {
        Self {
            base_url: BASE_URL.to_string(),
        }
    }

    /// 文本转语音：调用 `POST /v1/tts`，返回音频二进制字节。
    ///
    /// `model` 通过请求头 `model` 传递（不是 body 字段），这是 Fish Audio 的约定。
    /// 未选择音色时**省略** `reference_id` 字段（由模型侧默认音色兜底）；
    /// 不再填充硬编码的系统音色 ID——该 ID 已在 Fish Audio 服务端失效，
    /// 强制填充会导致所有未选音色的请求报 `Reference not found`。
    pub async fn synthesize(&self, text: &str, cfg: &TtsConfig) -> Result<Vec<u8>> {
        if cfg.fish_api_key.is_empty() {
            anyhow::bail!("Fish Audio API Key 未配置");
        }
        if text.trim().is_empty() {
            anyhow::bail!("合成文本为空");
        }

        let url = format!("{}/v1/tts", self.base_url);

        // 构建请求体
        let mut body = json!({
            "text": text,
            "format": cfg.format,
            "normalize": cfg.normalize,
            "temperature": cfg.temperature,
            "top_p": cfg.top_p,
            "prosody": {
                "speed": cfg.speed,
                "volume": cfg.volume,
            },
            "latency": cfg.latency,
            "chunk_length": cfg.chunk_length,
        });

        // 仅在显式选择音色时携带 reference_id
        if !cfg.reference_id.is_empty() {
            body["reference_id"] = json!(cfg.reference_id);
        }

        if cfg.format == "mp3" {
            body["mp3_bitrate"] = json!(cfg.mp3_bitrate);
        }
        if cfg.sample_rate > 0 {
            body["sample_rate"] = json!(cfg.sample_rate);
        }

        log::info!(
            "[tts] 请求合成: model={}, format={}, voice={}, text_len={}",
            cfg.model,
            cfg.format,
            if cfg.reference_id.is_empty() {
                "<模型默认>"
            } else {
                &cfg.reference_id
            },
            text.len()
        );

        let resp = get_tts_client()
            .post(&url)
            .header("Authorization", format!("Bearer {}", cfg.fish_api_key))
            .header("Content-Type", "application/json")
            .header("model", &cfg.model)
            .json(&body)
            .send()
            .await
            .context("Fish Audio TTS 请求失败")?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "<无法读取错误响应>".to_string());
            log::error!("[tts] 合成失败 {}: {}", status, err_text);
            anyhow::bail!("Fish Audio TTS 错误 {}: {}", status, err_text);
        }

        let bytes = resp.bytes().await.context("读取音频响应失败")?.to_vec();

        log::info!("[tts] 合成成功，音频大小 {} 字节", bytes.len());
        Ok(bytes)
    }

    /// 查询官方音色库：`GET /model`，返回原始 JSON（含 total / items）。
    pub async fn list_voices(&self, api_key: &str, params: &VoiceListParams) -> Result<Value> {
        if api_key.is_empty() {
            anyhow::bail!("Fish Audio API Key 未配置");
        }

        let url = format!("{}/model", self.base_url);
        let mut req = get_tts_client()
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key));

        if params.page_size > 0 {
            req = req.query(&[("page_size", params.page_size.to_string())]);
        }
        if params.page_number > 0 {
            req = req.query(&[("page_number", params.page_number.to_string())]);
        }
        if !params.title.is_empty() {
            req = req.query(&[("title", params.title.as_str())]);
        }
        if !params.language.is_empty() {
            req = req.query(&[("language", params.language.as_str())]);
        }
        if !params.sort_by.is_empty() {
            req = req.query(&[("sort_by", params.sort_by.as_str())]);
        }
        if params.self_only {
            req = req.query(&[("self", "true")]);
        }

        let resp = req.send().await.context("Fish Audio 音色库请求失败")?;
        let status = resp.status();
        if !status.is_success() {
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "<无法读取错误响应>".to_string());
            anyhow::bail!("Fish Audio 音色库错误 {}: {}", status, err_text);
        }

        let json: Value = resp.json().await.context("解析音色库响应失败")?;
        Ok(json)
    }

    /// 获取单个音色详情：`GET /model/{id}`。
    pub async fn get_voice(&self, api_key: &str, voice_id: &str) -> Result<Value> {
        if api_key.is_empty() {
            anyhow::bail!("Fish Audio API Key 未配置");
        }
        let url = format!("{}/model/{}", self.base_url, voice_id);

        let resp = get_tts_client()
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
            .context("Fish Audio 音色详情请求失败")?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "<无法读取错误响应>".to_string());
            anyhow::bail!("Fish Audio 音色详情错误 {}: {}", status, err_text);
        }

        let json: Value = resp.json().await.context("解析音色详情失败")?;
        Ok(json)
    }
}
