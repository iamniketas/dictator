//! Transcription module - Speech to text using faster-whisper HTTP server
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write as IoWrite;
use std::path::PathBuf;
use tracing::{info, warn};
use uuid::Uuid;

/// Transcribe audio samples to text using faster-whisper HTTP server
///
/// # Arguments
/// * `audio_data` - 16 kHz f32 PCM audio samples
/// * `language` - Language code (e.g., "ru", "en")
pub fn transcribe_audio(audio_data: &[f32], language: &str) -> Result<String> {
    let duration_secs = audio_data.len() as f32 / 16000.0;

    info!(
        "Transcribing {} samples ({:.1} seconds) with faster-whisper HTTP...",
        audio_data.len(),
        duration_secs
    );

    if audio_data.is_empty() {
        warn!("Empty audio data, skipping transcription");
        return Ok(String::new());
    }

    // Create temporary WAV file
    let temp_dir = std::env::temp_dir();
    let audio_path = temp_dir.join(format!("dictator_audio_{}.wav", Uuid::new_v4()));

    write_wav_file(&audio_path, audio_data)?;
    info!("Saved audio to temporary file: {:?}", audio_path);

    // Send HTTP request to Whisper server
    info!("Reading WAV file for upload...");
    let file_bytes = std::fs::read(&audio_path)
        .context("Failed to read audio file")?;
    info!("Read {} bytes from WAV file", file_bytes.len());

    let file_part = reqwest::blocking::multipart::Part::bytes(file_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .context("Failed to create file part")?;

    let form = reqwest::blocking::multipart::Form::new()
        .part("file", file_part)
        .text("language", language.to_string());

    // Dynamic timeout: at least 60s, or 2x audio duration (whisper can be slow)
    let timeout_secs = (60.0 + duration_secs * 1.5) as u64;
    info!("HTTP client timeout set to {} seconds", timeout_secs);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent("curl/7.68.0")
        .no_proxy()  // CRITICAL: disable system proxy for localhost
        .build()
        .context("Failed to build HTTP client")?;

    info!("Sending transcription request to http://127.0.0.1:5000/transcribe");
    let response = match client
        .post("http://127.0.0.1:5000/transcribe")
        .header("Connection", "close")
        .multipart(form)
        .send()
    {
        Ok(resp) => resp,
        Err(e) => {
            // Keep audio file for debugging on error
            let backup_path = audio_path.with_extension("wav.failed");
            let _ = std::fs::rename(&audio_path, &backup_path);
            anyhow::bail!("Failed to send HTTP request to Whisper server: {} (audio saved to {:?})", e, backup_path);
        }
    };

    let status = response.status();
    info!("Received response with status: {}", status);

    if !status.is_success() {
        // Keep audio file on server error too
        let backup_path = audio_path.with_extension("wav.failed");
        let _ = std::fs::rename(&audio_path, &backup_path);
        warn!("Audio file preserved at: {:?}", backup_path);
        let error_text = response.text().unwrap_or_else(|_| "Unknown error".to_string());
        warn!("Response body: {}", error_text);
        anyhow::bail!("Whisper server returned {}: {}", status, error_text);
    }

    // Parse JSON response
    let json: serde_json::Value = response
        .json()
        .context("Failed to parse JSON response")?;

    let result = json["text"]
        .as_str()
        .context("Missing 'text' field in response")?
        .to_string();

    let _ = std::fs::remove_file(&audio_path);
    info!("Transcription complete: \"{}\"", result);
    Ok(result)
}

/// Write audio samples to WAV file
fn write_wav_file(path: &PathBuf, samples: &[f32]) -> Result<()> {
    let mut file = File::create(path).context("Failed to create WAV file")?;

    // WAV header for 16 kHz mono f32 PCM
    let sample_rate = 16000u32;
    let num_channels = 1u16;
    let bits_per_sample = 32u16; // f32
    let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample / 8) as u32;
    let block_align = num_channels * (bits_per_sample / 8);
    let data_size = (samples.len() * 4) as u32; // 4 bytes per f32

    // RIFF header
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_size).to_le_bytes())?;
    file.write_all(b"WAVE")?;

    // fmt chunk
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // chunk size
    file.write_all(&3u16.to_le_bytes())?; // audio format (3 = IEEE float)
    file.write_all(&num_channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&bits_per_sample.to_le_bytes())?;

    // data chunk
    file.write_all(b"data")?;
    file.write_all(&data_size.to_le_bytes())?;

    // Write samples
    for sample in samples {
        file.write_all(&sample.to_le_bytes())?;
    }

    Ok(())
}

/// Async version of transcribe_audio for streaming support
pub async fn transcribe_audio_async(audio_data: &[f32], language: &str) -> Result<String> {
    use tokio::fs;

    let duration_secs = audio_data.len() as f32 / 16000.0;

    info!(
        "Transcribing {} samples ({:.1} seconds) with faster-whisper HTTP (async)...",
        audio_data.len(),
        duration_secs
    );

    if audio_data.is_empty() {
        warn!("Empty audio data, skipping transcription");
        return Ok(String::new());
    }

    // Create temporary WAV file
    let temp_dir = std::env::temp_dir();
    let audio_path = temp_dir.join(format!("dictator_audio_{}.wav", uuid::Uuid::new_v4()));

    write_wav_file(&audio_path, audio_data)?;
    info!("Saved audio to temporary file: {:?}", audio_path);

    // Send HTTP request to Whisper server
    info!("Reading WAV file for upload...");
    let file_bytes = fs::read(&audio_path)
        .await
        .context("Failed to read audio file")?;
    info!("Read {} bytes from WAV file", file_bytes.len());

    let file_part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .context("Failed to create file part")?;

    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("language", language.to_string());

    // Dynamic timeout: at least 60s, or 1.5x audio duration
    let timeout_secs = (60.0 + duration_secs * 1.5) as u64;
    info!("HTTP client timeout set to {} seconds (async)", timeout_secs);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent("curl/7.68.0")
        .no_proxy()
        .build()
        .context("Failed to build HTTP client")?;

    info!("Sending transcription request to http://127.0.0.1:5000/transcribe");
    let response = match client
        .post("http://127.0.0.1:5000/transcribe")
        .header("Connection", "close")
        .multipart(form)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            let backup_path = audio_path.with_extension("wav.failed");
            let _ = tokio::fs::rename(&audio_path, &backup_path).await;
            anyhow::bail!("Failed to send HTTP request to Whisper server: {} (audio saved to {:?})", e, backup_path);
        }
    };

    let status = response.status();
    info!("Received response with status: {}", status);

    if !status.is_success() {
        let backup_path = audio_path.with_extension("wav.failed");
        let _ = tokio::fs::rename(&audio_path, &backup_path).await;
        warn!("Audio file preserved at: {:?}", backup_path);
        let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        warn!("Response body: {}", error_text);
        anyhow::bail!("Whisper server returned {}: {}", status, error_text);
    }

    // Parse JSON response
    let json: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse JSON response")?;

    let result = json["text"]
        .as_str()
        .context("Missing 'text' field in response")?
        .to_string();

    let _ = tokio::fs::remove_file(&audio_path).await;
    info!("Transcription complete: \"{}\"", result);
    Ok(result)
}
