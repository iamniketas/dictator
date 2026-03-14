//! In-app downloader for GGML Whisper models.
//!
//! Downloads GGML `.bin` files into the shared `AudioModels` directory.

use anyhow::{Context, Result};
use hardware_profiler::default_audio_models_dir;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::info;

/// A downloadable GGML model resolved from the shared catalog.
#[derive(Debug, Clone)]
pub struct DownloadableModel {
    pub id: String,
    /// Display name shown in UI (e.g. "base")
    pub name: String,
    /// File name on source (e.g. "ggml-base.bin")
    pub filename: String,
    /// Direct source URL for download
    pub download_url: String,
    /// Approximate size in MB (for display only)
    pub size_mb: u32,
    /// Required files to consider model healthy in local store
    pub required_files: Vec<String>,
}


#[derive(Debug, Clone, Copy)]
pub struct DownloadProgress {
    pub progress: f32,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_sec: u64,
    pub eta_seconds: Option<u64>,
}
const FALLBACK_MODELS: &[(&str, &str, u32)] = &[
    ("whisper-ggml-tiny", "ggml-tiny.bin", 39),
    ("whisper-ggml-base", "ggml-base.bin", 74),
    ("whisper-ggml-small", "ggml-small.bin", 244),
    ("whisper-ggml-medium", "ggml-medium.bin", 769),
    ("whisper-ggml-large-v3-turbo", "ggml-large-v3-turbo.bin", 874),
    ("whisper-ggml-large-v3", "ggml-large-v3.bin", 1550),
];

fn default_download_url(filename: &str) -> String {
    format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        filename
    )
}

fn filename_to_short_name(filename: &str) -> String {
    filename
        .strip_prefix("ggml-")
        .unwrap_or(filename)
        .strip_suffix(".bin")
        .unwrap_or(filename)
        .to_string()
}

/// Return downloadable models from shared catalog (with safe fallback).
pub fn get_downloadable_models() -> Vec<DownloadableModel> {
    let mut models: Vec<DownloadableModel> = model_store::load_embedded_catalog()
        .ok()
        .map(|catalog| {
            catalog
                .models
                .into_iter()
                .filter(|m| {
                    m.family == "whisper_ggml"
                        && m.supported_backends.iter().any(|b| b == "embedded_whisper_rs")
                })
                .filter_map(|m| {
                    let filename = m.download_filename?;
                    let download_url = m
                        .download_url
                        .unwrap_or_else(|| default_download_url(&filename));
                    let required_files = if m.required_files.is_empty() {
                        vec![filename.clone()]
                    } else {
                        m.required_files
                    };
                    let size_mb = ((m.size_bytes as f64) / (1024.0 * 1024.0)).ceil() as u32;
                    Some(DownloadableModel {
                        id: m.id,
                        name: filename_to_short_name(&filename),
                        filename,
                        download_url,
                        size_mb,
                        required_files,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if models.is_empty() {
        models = FALLBACK_MODELS
            .iter()
            .map(|(id, filename, size_mb)| DownloadableModel {
                id: (*id).to_string(),
                name: filename_to_short_name(filename),
                filename: (*filename).to_string(),
                download_url: default_download_url(filename),
                size_mb: *size_mb,
                required_files: vec![(*filename).to_string()],
            })
            .collect();
    }

    models.sort_by_key(|m| m.size_mb);
    models
}

/// Evaluate local model health in `models_root` using catalog required files.
pub fn model_health(models_root: &Path, model: &DownloadableModel) -> String {
    let health = model_store::evaluate_model_health(models_root, &model.required_files);
    if health != "ok" {
        return health;
    }

    for file in &model.required_files {
        let Ok(meta) = std::fs::metadata(models_root.join(file)) else {
            return String::from("missing_files");
        };
        if meta.is_file() && meta.len() == 0 {
            return String::from("corrupt_file");
        }
    }

    String::from("ok")
}

/// Returns the canonical shared models directory (`AudioModels`).
pub fn default_models_dir() -> PathBuf {
    default_audio_models_dir()
}

/// Download a model file into `target_dir`.
///
/// `progress_cb` is called with values 0.0..=1.0 during the download.
/// Returns the path to the downloaded file on success.
///
/// Uses a temporary `.tmp` file and atomically renames it on completion
/// to avoid leaving partial/corrupt model files.
pub fn download_model(
    filename: &str,
    source_url: &str,
    target_dir: &Path,
    progress_cb: impl Fn(DownloadProgress),
) -> Result<PathBuf> {
    std::fs::create_dir_all(target_dir)
        .with_context(|| format!("Cannot create models directory: {}", target_dir.display()))?;

    let dest = target_dir.join(filename);
    let tmp = target_dir.join(format!("{}.tmp", filename));

    info!("[DOWNLOAD] Starting: {} -> {:?}", source_url, dest);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3600)) // 1 h for large models
        .build()
        .context("Failed to build HTTP client")?;

    let mut response = client
        .get(source_url)
        .send()
        .with_context(|| format!("HTTP GET failed: {}", source_url))?;

    if !response.status().is_success() {
        anyhow::bail!("Server returned {} for {}", response.status(), source_url);
    }

    let total = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let started = Instant::now();

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
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let bytes_per_sec = (downloaded as f64 / elapsed) as u64;
        let progress = if total > 0 {
            (downloaded as f32 / total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let eta_seconds = if total > 0 && bytes_per_sec > 0 {
            Some((total.saturating_sub(downloaded)) / bytes_per_sec)
        } else {
            None
        };
        progress_cb(DownloadProgress {
            progress,
            downloaded_bytes: downloaded,
            total_bytes: total,
            bytes_per_sec,
            eta_seconds,
        });
    }

    drop(file);
    std::fs::rename(&tmp, &dest)
        .with_context(|| format!("Failed to rename {:?} -> {:?}", tmp, dest))?;

    info!("[DOWNLOAD] Complete: {:?} ({} bytes)", dest, downloaded);
    Ok(dest)
}




