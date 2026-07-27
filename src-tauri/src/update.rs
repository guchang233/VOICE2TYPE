use anyhow::Result;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

/// 当前应用版本号（来自 Cargo.toml 的 version 字段）
pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub body: String,
    pub download_url: String,
    pub filename: String,
    pub date: String,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

const GITHUB_API_BASE: &str = "https://api.github.com/repos/guchang233/VOICE2TYPE";

fn build_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("Voice2Type-App")
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(Into::into)
}

/// 获取最新正式 release（跳过 prerelease 和 draft）
/// 优先使用 /releases/latest，失败时回退到 /releases 列表
fn fetch_latest_release() -> Result<GithubRelease> {
    let client = build_client()?;

    // 1. 首选 /releases/latest（仅返回最新正式发布，自动排除 prerelease 和 draft）
    let resp = client
        .get(format!("{}/releases/latest", GITHUB_API_BASE))
        .header("Accept", "application/vnd.github+json")
        .send()?;

    if resp.status().is_success() {
        return Ok(resp.json()?);
    }

    // 如果 latest 接口返回 404（仓库还没有任何正式 release），回退到 /releases 列表
    if resp.status().as_u16() == 404 {
        log::warn!("No latest release found (404), falling back to /releases list");
        let resp2 = client
            .get(format!("{}/releases?per_page=10", GITHUB_API_BASE))
            .header("Accept", "application/vnd.github+json")
            .send()?;

        if !resp2.status().is_success() {
            anyhow::bail!(
                "GitHub API returned error status: {} (fallback)",
                resp2.status()
            );
        }

        let releases: Vec<GithubRelease> = resp2.json()?;
        // 过滤掉 prerelease 和 draft，取第一个（即最新）
        return releases
            .into_iter()
            .find(|r| !r.prerelease && !r.draft)
            .ok_or_else(|| anyhow::anyhow!("No stable releases found in repository"));
    }

    // 其他错误状态码
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    anyhow::bail!("GitHub API returned error status: {} - {}", status, body);
}

pub fn get_latest_release_info() -> Result<UpdateInfo> {
    let release = fetch_latest_release()?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.to_lowercase().ends_with(".exe"))
        .cloned();

    Ok(UpdateInfo {
        version: release.tag_name,
        body: release.body.unwrap_or_default(),
        download_url: asset
            .as_ref()
            .map(|a| a.browser_download_url.clone())
            .unwrap_or_default(),
        filename: asset.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
        date: release.published_at.unwrap_or_default(),
    })
}

#[derive(Clone, Debug)]
pub struct UpdateCheckResult {
    pub has_update: bool,
    pub info: UpdateInfo,
}

pub fn check_update() -> Result<UpdateCheckResult> {
    let release = fetch_latest_release()?;

    // semver parsing
    // 清理版本字符串（去除 'v' 前缀，例如 "v0.1.1" -> "0.1.1"）
    let clean_current = env!("CARGO_PKG_VERSION").trim_start_matches('v');
    let clean_latest = release.tag_name.trim_start_matches('v');

    let current =
        semver::Version::parse(clean_current).unwrap_or_else(|_| semver::Version::new(0, 0, 0));
    let target =
        semver::Version::parse(clean_latest).unwrap_or_else(|_| semver::Version::new(0, 0, 0));

    let has_update = target > current;

    // 查找 Windows 安装包
    // 严格匹配 .exe 扩展名，避免下载源代码压缩包
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.to_lowercase().ends_with(".exe"))
        .cloned();

    let info = UpdateInfo {
        version: release.tag_name,
        body: release.body.unwrap_or_default(),
        download_url: asset
            .as_ref()
            .map(|a| a.browser_download_url.clone())
            .unwrap_or_default(),
        filename: asset.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
        date: release.published_at.unwrap_or_default(),
    };

    Ok(UpdateCheckResult { has_update, info })
}

// Progress callback: (current_bytes, total_bytes)
pub fn download_file<F>(url: &str, path: &PathBuf, on_progress: F) -> Result<()>
where
    F: Fn(u64, u64),
{
    let client = build_client()?;

    let mut resp = client
        .get(url)
        .header("Accept", "application/octet-stream")
        .send()?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("Download failed with status: {} - {}", status, body);
    }

    let total_size = resp.content_length().unwrap_or(0);

    let mut file = fs::File::create(path)?;
    let mut downloaded: u64 = 0;
    let mut buffer = [0; 8192];

    // Magic number check for EXE (MZ)
    let mut first_chunk = true;

    loop {
        let n = resp.read(&mut buffer)?;
        if n == 0 {
            break;
        }

        if first_chunk {
            if n >= 2 {
                if buffer[0] != 0x4D || buffer[1] != 0x5A {
                    // 'M' 'Z'
                    // Close and delete the file
                    drop(file);
                    let _ = fs::remove_file(path);
                    anyhow::bail!(
                        "Downloaded file is not a valid Windows Executable (Header mismatch)."
                    );
                }
            }
            first_chunk = false;
        }

        file.write_all(&buffer[..n])?;
        downloaded += n as u64;
        on_progress(downloaded, total_size);
    }

    file.sync_all()?;
    Ok(())
}

pub fn install_update(new_bin: &PathBuf) -> Result<()> {
    let current_exe = std::env::current_exe()?;
    let old_exe = current_exe.with_extension("exe.old");

    // Remove old backup if exists
    if old_exe.exists() {
        let _ = fs::remove_file(&old_exe);
    }

    // Rename current to old
    // On Windows, you can rename a running executable.
    fs::rename(&current_exe, &old_exe)?;

    // Set .old file as hidden so user doesn't see it
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};

        let path_str = old_exe.to_string_lossy();
        // Ensure null-terminated wide string
        let path_wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = SetFileAttributesW(PCWSTR(path_wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN);
    }

    // Move new to current
    fs::rename(new_bin, &current_exe)?;

    Ok(())
}
