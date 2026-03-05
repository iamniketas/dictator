//! In-app downloader for GGML Whisper models.
//!
//! Downloads GGML `.bin` files from https://huggingface.co/ggerganov/whisper.cpp
//! into `%LocalAppData%\whisper-models\` (shared models directory).

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::info;

/// A known GGML model available for download.
pub struct KnownModel {
    /// Display name shown in UI (e.g. "large-v3")
    pub name: &'static str,
    /// File name on HuggingFace (e.g. "ggml-large-v3.bin")
    pub filename: &'static str,
    /// Approximate size in MB (for display only)
    pub size_mb: u32,
}

/// All known models, ordered from smallest to largest.
pub const KNOWN_MODELS: &[KnownModel] = &[
    KnownModel { name: "tiny",           filename: "ggml-tiny.bin",           size_mb: 39   },
    KnownModel { name: "base",           filename: "ggml-base.bin",           size_mb: 74   },
    KnownModel { name: "small",          filename: "ggml-small.bin",          size_mb: 244  },
    KnownModel { name: "medium",         filename: "ggml-medium.bin",         size_mb: 769  },
    KnownModel { name: "large-v3-turbo", filename: "ggml-large-v3-turbo.bin", size_mb: 874  },
    KnownModel { name: "large-v3",       filename: "ggml-large-v3.bin",       size_mb: 1550 },
];

/// Returns the default models directory (`%LocalAppData%\whisper-models\`).
pub fn default_models_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("whisper-models")
}

/// Download a GGML model file into `target_dir`.
///
/// `progress_cb` is called with values 0.0..=1.0 during the download.
/// Returns the path to the downloaded file on success.
///
/// Uses a temporary `.tmp` file and atomically renames it on completion
/// to avoid leaving partial/corrupt model files.
pub fn download_model(
    filename: &str,
    target_dir: &Path,
    progress_cb: impl Fn(f32),
) -> Result<PathBuf> {
    std::fs::create_dir_all(target_dir)
        .with_context(|| format!("Cannot create models directory: {}", target_dir.display()))?;

    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        filename
    );
    let dest = target_dir.join(filename);
    let tmp = target_dir.join(format!("{}.tmp", filename));

    info!("[DOWNLOAD] Starting: {} -> {:?}", url, dest);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3600)) // 1 h for large models
        .build()
        .context("Failed to build HTTP client")?;

    let mut response = client
        .get(&url)
        .send()
        .with_context(|| format!("HTTP GET failed: {}", url))?;

    if !response.status().is_success() {
        anyhow::bail!("Server returned {} for {}", response.status(), url);
    }

    let total = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    let mut file = std::fs::File::create(&tmp)
        .with_context(|| format!("Cannot create temp file: {:?}", tmp))?;

    let mut buf = vec![0u8; 256 * 1024]; // 256 KB chunks
    loop {
        let n = response.read(&mut buf).context("Read error during download")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("Write error during download")?;
        downloaded += n as u64;
        if total > 0 {
            progress_cb(downloaded as f32 / total as f32);
        }
    }

    drop(file);
    std::fs::rename(&tmp, &dest)
        .with_context(|| format!("Failed to rename {:?} -> {:?}", tmp, dest))?;

    info!("[DOWNLOAD] Complete: {:?} ({} bytes)", dest, downloaded);
    Ok(dest)
}
