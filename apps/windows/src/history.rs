//! History module - Save and manage audio recordings and transcriptions
//!
//! Stores files in the project directory under `recordings/YYYY-MM-DD/`
//! - Audio: `{timestamp}.wav`
//! - Text: `{timestamp}.txt`
//! - Metadata: `{timestamp}.json`

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
    base_dir: PathBuf,
    retention_days: u32,
}

impl HistoryManager {
    /// Create new history manager with default recordings directory in project
    pub fn new(retention_days: u32) -> Result<Self> {
        let base_dir = Self::get_recordings_dir()?;
        fs::create_dir_all(&base_dir)?;
        
        info!("[HISTORY] Initialized at {:?}, retention: {} days", base_dir, retention_days);
        
        Ok(Self {
            base_dir,
            retention_days,
        })
    }

    /// Get the recordings directory (project_dir/recordings)
    fn get_recordings_dir() -> Result<PathBuf> {
        // Try to use project directory first
        if let Ok(cwd) = std::env::current_dir() {
            let recordings_dir = cwd.join("recordings");
            // Check if we can write here
            if recordings_dir.exists() || fs::create_dir_all(&recordings_dir).is_ok() {
                return Ok(recordings_dir);
            }
        }
        
        // Fallback to data_dir
        let data_dir = dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("dictator")
            .join("recordings");
        fs::create_dir_all(&data_dir)?;
        Ok(data_dir)
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
    fn today_dir(&self) -> PathBuf {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.base_dir.join(&today)
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
        let today_dir = self.today_dir();
        fs::create_dir_all(&today_dir)?;

        let audio_path = today_dir.join(format!("{}.wav", id));
        let text_path = today_dir.join(format!("{}.txt", id));
        let meta_path = today_dir.join(format!("{}.json", id));

        // Save audio as WAV
        self.save_wav(audio_data, &audio_path)?;
        info!("[HISTORY] Saved audio: {:?} ({:.1}s)", audio_path, duration_secs);

        // Save text
        let mut text_file = fs::File::create(&text_path)?;
        text_file.write_all(text.as_bytes())?;
        info!("[HISTORY] Saved text: {:?} ({} chars)", text_path, text.len());

        // Save metadata
        let text_preview = text.chars().take(100).collect::<String>().replace('\n', " ");
        let metadata = RecordingMetadata {
            timestamp: id.parse().unwrap_or(0),
            datetime: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            duration_secs,
            mode: mode.to_string(),
            language: language.to_string(),
            text_preview,
        };
        let meta_json = serde_json::to_string_pretty(&metadata)?;
        let mut meta_file = fs::File::create(&meta_path)?;
        meta_file.write_all(meta_json.as_bytes())?;

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

    /// Get list of recent recordings (newest first)
    pub fn get_recent_recordings(&self, limit: usize) -> Vec<Recording> {
        let mut recordings = Vec::new();

        // Read all date directories
        let Ok(entries) = fs::read_dir(&self.base_dir) else {
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
                .filter(|e| e.path().extension().map(|ext| ext == "json").unwrap_or(false))
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
        let dir = meta_path.parent()?;

        let audio_path = dir.join(format!("{}.wav", id));
        let text_path = dir.join(format!("{}.txt", id));

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
        info!("[HISTORY] Copied recording {} to clipboard ({} chars)", recording.id, text.len());
        Ok(())
    }

    /// Open recordings folder in explorer
    pub fn open_folder(&self) -> Result<()> {
        info!("[HISTORY] Opening folder: {:?}", self.base_dir);
        std::process::Command::new("explorer")
            .arg(&self.base_dir)
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

        let entries = fs::read_dir(&self.base_dir)?;
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
        &self.base_dir
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
pub fn build_history_menu_entries(recordings: &[Recording], start_id: u16) -> Vec<HistoryMenuEntry> {
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
