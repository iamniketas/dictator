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
}

/// Streaming configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingConfig {
    /// Enable real-time streaming transcription (default: false)
    pub enabled: bool,
    /// Poll interval in seconds (default: 3)
    pub poll_interval: u64,
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
}

/// Ollama configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    pub url: String,
    pub model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: HotkeyConfig {
                modifiers: vec!["ctrl".into(), "shift".into()],
                key: "D".into(),
            },
            audio: AudioConfig {
                device: None,
                sample_rate: 16000,
            },
            whisper: WhisperConfig {
                model_path: PathBuf::from("models/ggml-large-v3.bin"),
                language: "ru".into(),
            },
            ollama: OllamaConfig {
                url: "http://localhost:11434".into(),
                model: "glm-4.7-flash".into(),
            },
            streaming: StreamingConfig {
                enabled: false, // Disabled by default - safer
                poll_interval: 3,
            },
        }
    }
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            poll_interval: 3,
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
        assert_eq!(config.hotkey.key, "D");
        assert_eq!(config.audio.sample_rate, 16000);
    }
}
