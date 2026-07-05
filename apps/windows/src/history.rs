//! History module - Save and manage audio recordings and transcriptions
//!
//! Stores files in configured storage directories:
//! - Audio: `<audio_dir>/YYYY-MM-DD/{timestamp}.wav`
//! - Text: `<transcripts_dir>/YYYY-MM-DD/{timestamp}.txt`
//! - Metadata: `<transcripts_dir>/YYYY-MM-DD/{timestamp}.json`

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Recording metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingMetadata {
    pub timestamp: u64,
    pub datetime: String,
    pub duration_secs: f32,
    pub mode: String, // "streaming" or "full"
    pub language: String,
    pub text_preview: String,
}

/// Recording entry (audio + text + metadata)
#[derive(Debug, Clone)]
pub struct Recording {
    pub id: String,
    pub metadata: RecordingMetadata,
    pub audio_path: PathBuf,
    pub text_path: PathBuf,
    pub meta_path: PathBuf,
}

/// History manager for saving and retrieving recordings
pub struct HistoryManager {
    audio_dir: PathBuf,
    transcripts_dir: PathBuf,
    retention_days: u32,
}

impl HistoryManager {
    /// Create history manager from explicit storage directories.
    pub fn new(audio_dir: PathBuf, transcripts_dir: PathBuf, retention_days: u32) -> Result<Self> {
        fs::create_dir_all(&audio_dir)?;
        fs::create_dir_all(&transcripts_dir)?;

        info!(
            "[HISTORY] Initialized audio={:?}, transcripts={:?}, retention: {} days",
            audio_dir, transcripts_dir, retention_days
        );

        Ok(Self {
            audio_dir,
            transcripts_dir,
            retention_days,
        })
    }

    /// Generate timestamp-based ID
    fn generate_id() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("{}", now)
    }

    /// Get today's subdirectory path
    fn today_subdir(&self) -> String {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        today
    }

    /// Save a new recording
    pub fn save_recording(
        &self,
        audio_data: &[f32],
        text: &str,
        duration_secs: f32,
        mode: &str,
        language: &str,
    ) -> Result<Recording> {
        let id = Self::generate_id();
        let today = self.today_subdir();
        let today_audio_dir = self.audio_dir.join(&today);
        let today_transcripts_dir = self.transcripts_dir.join(&today);
        fs::create_dir_all(&today_audio_dir)?;
        fs::create_dir_all(&today_transcripts_dir)?;

        let audio_path = today_audio_dir.join(format!("{}.wav", id));
        let text_path = today_transcripts_dir.join(format!("{}.txt", id));
        let meta_path = today_transcripts_dir.join(format!("{}.json", id));

        // Save audio as WAV
        self.save_wav(audio_data, &audio_path)?;
        info!(
            "[HISTORY] Saved audio: {:?} ({:.1}s)",
            audio_path, duration_secs
        );

        // Save text
        Self::write_atomic_text(&text_path, text)?;
        info!(
            "[HISTORY] Saved text: {:?} ({} chars)",
            text_path,
            text.len()
        );

        // Save metadata
        let text_preview = text
            .chars()
            .take(100)
            .collect::<String>()
            .replace('\n', " ");
        let metadata = RecordingMetadata {
            timestamp: id.parse().unwrap_or(0),
            datetime: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            duration_secs,
            mode: mode.to_string(),
            language: language.to_string(),
            text_preview,
        };
        let meta_json = serde_json::to_string_pretty(&metadata)?;
        Self::write_atomic_text(&meta_path, &meta_json)?;

        // Clean up old recordings
        if let Err(e) = self.cleanup_old_recordings() {
            warn!("[HISTORY] Cleanup failed: {}", e);
        }

        Ok(Recording {
            id,
            metadata,
            audio_path,
            text_path,
            meta_path,
        })
    }

    /// Prepare a pending recording entry without writing the audio yet.
    ///
    /// Creates the dated directories and writes a placeholder transcript +
    /// metadata, returning the handle (id + paths). The WAV itself is written
    /// separately via [`write_audio`], typically on a background thread, so the
    /// audio lands on disk in parallel with transcription instead of after it.
    /// This guarantees the captured audio is recoverable even if transcription
    /// hangs or the process is killed before producing any text.
    pub fn prepare_pending_recording(
        &self,
        duration_secs: f32,
        mode: &str,
        language: &str,
    ) -> Result<Recording> {
        let id = Self::generate_id();
        let today = self.today_subdir();
        let today_audio_dir = self.audio_dir.join(&today);
        let today_transcripts_dir = self.transcripts_dir.join(&today);
        fs::create_dir_all(&today_audio_dir)?;
        fs::create_dir_all(&today_transcripts_dir)?;

        let audio_path = today_audio_dir.join(format!("{}.wav", id));
        let text_path = today_transcripts_dir.join(format!("{}.txt", id));
        let meta_path = today_transcripts_dir.join(format!("{}.json", id));

        let placeholder = "[Transcribing…]";
        Self::write_atomic_text(&text_path, placeholder)?;

        let metadata = RecordingMetadata {
            timestamp: id.parse().unwrap_or(0),
            datetime: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            duration_secs,
            mode: mode.to_string(),
            language: language.to_string(),
            text_preview: placeholder.to_string(),
        };
        let meta_json = serde_json::to_string_pretty(&metadata)?;
        Self::write_atomic_text(&meta_path, &meta_json)?;

        info!(
            "[HISTORY] Prepared pending recording {} ({:.1}s), audio pending at {:?}",
            id, duration_secs, audio_path
        );

        Ok(Recording {
            id,
            metadata,
            audio_path,
            text_path,
            meta_path,
        })
    }

    /// Write the WAV audio for a previously prepared recording. Safe to call
    /// from a background thread (only touches the recording's own audio file).
    pub fn write_audio(&self, recording: &Recording, audio_data: &[f32]) -> Result<()> {
        self.save_wav(audio_data, &recording.audio_path)?;
        info!(
            "[HISTORY] Saved audio: {:?} ({:.1}s)",
            recording.audio_path, recording.metadata.duration_secs
        );
        Ok(())
    }

    /// Update the transcript text + metadata of an existing recording in place,
    /// keeping the same id and audio file. Used to fill in the final text once
    /// transcription completes (or an error string if it failed).
    pub fn update_recording_text(
        &self,
        recording: &Recording,
        text: &str,
        mode: &str,
    ) -> Result<()> {
        Self::write_atomic_text(&recording.text_path, text)?;

        let text_preview = text
            .chars()
            .take(100)
            .collect::<String>()
            .replace('\n', " ");
        let mut metadata = recording.metadata.clone();
        metadata.mode = mode.to_string();
        metadata.text_preview = text_preview;
        let meta_json = serde_json::to_string_pretty(&metadata)?;
        Self::write_atomic_text(&recording.meta_path, &meta_json)?;

        info!(
            "[HISTORY] Updated recording {} text ({} chars)",
            recording.id,
            text.len()
        );

        if let Err(e) = self.cleanup_old_recordings() {
            warn!("[HISTORY] Cleanup failed: {}", e);
        }

        Ok(())
    }

    /// Save audio data as WAV file (16-bit PCM, 16kHz, mono)
    fn save_wav(&self, audio_data: &[f32], path: &Path) -> Result<()> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = hound::WavWriter::create(path, spec)?;

        for &sample in audio_data {
            // Convert f32 [-1.0, 1.0] to i16
            let sample_i16 = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            writer.write_sample(sample_i16)?;
        }

        writer.finalize()?;
        Ok(())
    }

    fn write_atomic_text(path: &Path, text: &str) -> Result<()> {
        let mut tmp = path.to_path_buf();
        tmp.set_extension("tmp");
        let mut text_file = fs::File::create(&tmp)?;
        text_file.write_all(text.as_bytes())?;
        text_file.flush()?;
        drop(text_file);
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        fs::rename(tmp, path)?;
        Ok(())
    }

    /// Get list of recent recordings (newest first)
    pub fn get_recent_recordings(&self, limit: usize) -> Vec<Recording> {
        let mut recordings = Vec::new();

        // Read all date directories
        let Ok(entries) = fs::read_dir(&self.transcripts_dir) else {
            return recordings;
        };

        let mut date_dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect();

        // Sort by name (YYYY-MM-DD) in reverse order (newest first)
        date_dirs.sort_by(|a, b| b.cmp(a));

        // Collect recordings from each date directory
        for date_dir in date_dirs {
            let Ok(entries) = fs::read_dir(&date_dir) else {
                continue;
            };

            let mut day_recordings: Vec<Recording> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "json")
                        .unwrap_or(false)
                })
                .filter_map(|e| self.load_recording_from_meta(&e.path()))
                .collect();

            // Sort by timestamp (newest first)
            day_recordings.sort_by(|a, b| b.metadata.timestamp.cmp(&a.metadata.timestamp));
            recordings.extend(day_recordings);

            if recordings.len() >= limit {
                break;
            }
        }

        recordings.truncate(limit);
        recordings
    }

    /// Load recording from metadata file
    fn load_recording_from_meta(&self, meta_path: &Path) -> Option<Recording> {
        let id = meta_path.file_stem()?.to_str()?.to_string();
        let transcripts_day_dir = meta_path.parent()?;
        let day = transcripts_day_dir
            .file_name()?
            .to_string_lossy()
            .to_string();
        let audio_day_dir = self.audio_dir.join(day);

        let audio_path = audio_day_dir.join(format!("{}.wav", id));
        let text_path = transcripts_day_dir.join(format!("{}.txt", id));

        let meta_content = fs::read_to_string(meta_path).ok()?;
        let metadata: RecordingMetadata = serde_json::from_str(&meta_content).ok()?;

        Some(Recording {
            id,
            metadata,
            audio_path,
            text_path,
            meta_path: meta_path.to_path_buf(),
        })
    }

    /// Read text content of a recording
    pub fn read_text(&self, recording: &Recording) -> Result<String> {
        Ok(fs::read_to_string(&recording.text_path)?)
    }

    /// Copy recording text to clipboard
    pub fn copy_to_clipboard(&self, recording: &Recording) -> Result<()> {
        let text = self.read_text(recording)?;
        clipboard_win::set_clipboard_string(&text)
            .map_err(|e| anyhow::anyhow!("Failed to copy to clipboard: {:?}", e))?;
        info!(
            "[HISTORY] Copied recording {} to clipboard ({} chars)",
            recording.id,
            text.len()
        );
        Ok(())
    }

    /// Open recordings folder in explorer
    pub fn open_folder(&self) -> Result<()> {
        info!("[HISTORY] Opening folder: {:?}", self.transcripts_dir);
        std::process::Command::new("explorer")
            .arg(&self.transcripts_dir)
            .spawn()?;
        Ok(())
    }

    /// Clean up recordings older than retention_days
    fn cleanup_old_recordings(&self) -> Result<()> {
        if self.retention_days == 0 {
            return Ok(());
        }

        let cutoff = chrono::Local::now() - chrono::Duration::days(self.retention_days as i64);
        let cutoff_str = cutoff.format("%Y-%m-%d").to_string();

        let entries = fs::read_dir(&self.transcripts_dir)?;
        let mut cleaned = 0;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let dir_name = entry.file_name();
            let dir_name_str = dir_name.to_string_lossy();

            // Compare date directory names (YYYY-MM-DD)
            if dir_name_str.as_ref() <= cutoff_str.as_str() {
                info!("[HISTORY] Removing old directory: {:?}", path);
                fs::remove_dir_all(&path)?;
                let day = dir_name_str.to_string();
                let audio_day = self.audio_dir.join(&day);
                if audio_day.exists() {
                    fs::remove_dir_all(audio_day)?;
                }
                cleaned += 1;
            }
        }

        if cleaned > 0 {
            info!("[HISTORY] Cleaned up {} old directories", cleaned);
        }

        Ok(())
    }

    /// Get base directory path
    pub fn base_dir(&self) -> &Path {
        &self.transcripts_dir
    }
}

/// Menu entry for tray
#[derive(Debug, Clone)]
pub struct HistoryMenuEntry {
    pub id: u16,
    pub label: String,
    pub recording_id: String,
}

/// Build menu entries for recent recordings
pub fn build_history_menu_entries(
    recordings: &[Recording],
    start_id: u16,
) -> Vec<HistoryMenuEntry> {
    recordings
        .iter()
        .enumerate()
        .map(|(i, rec)| {
            let time = rec.metadata.datetime.get(11..16).unwrap_or("--:--"); // HH:MM
            let preview = if rec.metadata.text_preview.len() > 30 {
                format!("{}...", &rec.metadata.text_preview[..30])
            } else {
                rec.metadata.text_preview.clone()
            };
            let label = format!("[{}] {}", time, preview);
            HistoryMenuEntry {
                id: start_id + i as u16,
                label,
                recording_id: rec.id.clone(),
            }
        })
        .collect()
}

/// Format recording for display
pub fn format_recording_info(rec: &Recording) -> String {
    format!(
        "{} | {:.1}s | {} | {}",
        rec.metadata.datetime,
        rec.metadata.duration_secs,
        rec.metadata.mode,
        if rec.metadata.text_preview.len() > 50 {
            format!("{}...", &rec.metadata.text_preview[..50])
        } else {
            rec.metadata.text_preview.clone()
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_id() {
        let id = HistoryManager::generate_id();
        assert!(!id.is_empty());
        assert!(id.parse::<u64>().is_ok());
    }
}
