//! Configuration module for Dictator

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub hotkey: HotkeyConfig,
    pub audio: AudioConfig,
    pub whisper: WhisperConfig,
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub streaming: StreamingConfig,
    #[serde(default)]
    pub history: HistoryConfig,
    #[serde(default)]
    pub injection: InjectionConfig,
}

/// History storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// Enable saving transcriptions and audio to disk (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Days to keep history files before auto-cleanup (default: 7)
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Max entries to show in tray menu (default: 5)
    #[serde(default = "default_max_recent")]
    pub max_recent: usize,
}

fn default_true() -> bool {
    true
}

fn default_retention_days() -> u32 {
    7
}

fn default_max_recent() -> usize {
    5
}

/// Streaming configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingConfig {
    /// Enable real-time streaming transcription (default: false)
    #[serde(default)]
    pub enabled: bool,
    /// Chunk/poll interval in seconds (default: 15)
    #[serde(default = "default_streaming_poll_interval")]
    pub poll_interval: u64,
}

fn default_streaming_poll_interval() -> u64 {
    15
}

/// Hotkey configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    pub modifiers: Vec<String>,
    pub key: String,
}

/// Audio configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub device: Option<String>,
    pub sample_rate: u32,
}

/// Whisper configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperConfig {
    pub model_path: PathBuf,
    pub language: String,
    /// Directory to scan for available models.
    /// If not set, defaults to the parent of model_path.
    #[serde(default)]
    pub models_dir: Option<PathBuf>,
}

impl WhisperConfig {
    /// Returns the directory to scan for models.
    pub fn effective_models_dir(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.models_dir {
            return Some(dir.clone());
        }
        self.model_path.parent().map(|p| p.to_path_buf())
    }
}

/// Text injection method
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionMethod {
    /// Unicode SendInput (auto-switches to clipboard for >500 chars)
    Direct,
    /// Always use clipboard + Ctrl+V (preserves original clipboard)
    Clipboard,
    /// Clipboard + Ctrl+V + Enter
    ClipboardEnter,
}

impl Default for InjectionMethod {
    fn default() -> Self {
        InjectionMethod::Direct
    }
}

/// Text injection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionConfig {
    #[serde(default)]
    pub method: InjectionMethod,
}

impl Default for InjectionConfig {
    fn default() -> Self {
        Self {
            method: InjectionMethod::Direct,
        }
    }
}

/// Ollama configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    #[serde(default)]
    pub enabled: bool,
    pub url: String,
    pub model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: HotkeyConfig {
                modifiers: vec![],
                key: "right_alt".into(),
            },
            audio: AudioConfig {
                device: None,
                sample_rate: 16000,
            },
            whisper: WhisperConfig {
                model_path: PathBuf::from("models/ggml-large-v3.bin"),
                language: "ru".into(),
                models_dir: None,
            },
            ollama: OllamaConfig {
                enabled: false, // Disabled by default for speed
                url: "http://localhost:11434".into(),
                model: "glm-4.7-flash".into(),
            },
            streaming: StreamingConfig {
                enabled: false, // Disabled by default - safer
                poll_interval: default_streaming_poll_interval(),
            },
            history: HistoryConfig::default(),
            injection: InjectionConfig::default(),
        }
    }
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            poll_interval: default_streaming_poll_interval(),
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            retention_days: default_retention_days(),
            max_recent: default_max_recent(),
        }
    }
}

impl Config {
    /// Load configuration from file or create default
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path();

        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            let config = Config::default();
            config.save()?;
            Ok(config)
        }
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path();

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        fs::write(config_path, content)?;
        Ok(())
    }

    /// Get configuration file path
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("dictator")
            .join("config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.hotkey.key, "right_alt");
        assert_eq!(config.audio.sample_rate, 16000);
    }
}
