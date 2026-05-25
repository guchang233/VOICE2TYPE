use anyhow::Result;
use self_update::backends::github::Update;
use self_update::cargo_crate_version;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

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
    body: Option<String>,
    published_at: Option<String>,
    assets: Vec<GithubAsset>,
}

fn fetch_latest_release() -> Result<GithubRelease> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Voice2Type-App")
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let resp = client
        .get("https://api.github.com/repos/guchang233/VOICE2TYPE/releases/latest")
        .send()?;

    if !resp.status().is_success() {
        anyhow::bail!("GitHub API returned error status: {}", resp.status());
    }

    let release: GithubRelease = resp.json()?;
    Ok(release)
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

pub fn check_update() -> Result<Option<UpdateInfo>> {
    let release = fetch_latest_release()?;

    // semver parsing
    // Clean up version string (remove 'v' prefix if present)
    let clean_current = cargo_crate_version!().trim_start_matches('v');
    let clean_latest = release.tag_name.trim_start_matches('v');

    let current =
        semver::Version::parse(clean_current).unwrap_or_else(|_| semver::Version::new(0, 0, 0));
    let target =
        semver::Version::parse(clean_latest).unwrap_or_else(|_| semver::Version::new(0, 0, 0));

    if target > current {
        // Find the asset for Windows
        // Strictly require .exe extension to avoid downloading source code zips
        let asset = release
            .assets
            .iter()
            .find(|a| a.name.to_lowercase().ends_with(".exe"))
            .cloned();

        if let Some(asset) = asset {
            Ok(Some(UpdateInfo {
                version: release.tag_name,
                body: release.body.unwrap_or_default(),
                download_url: asset.browser_download_url,
                filename: asset.name,
                date: release.published_at.unwrap_or_default(),
            }))
        } else {
            // No exe asset found, probably a source-only release
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

// Progress callback: (current_bytes, total_bytes)
pub fn download_file<F>(url: &str, path: &PathBuf, on_progress: F) -> Result<()>
where
    F: Fn(u64, u64),
{
    let client = reqwest::blocking::Client::builder()
        .user_agent("Voice2Type-App")
        .build()?;

    let mut resp = client
        .get(url)
        .header("Accept", "application/octet-stream")
        .send()?;

    if !resp.status().is_success() {
        anyhow::bail!("Download failed with status: {}", resp.status());
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
