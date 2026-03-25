//! Configuration module for Dictator

use anyhow::Result;
use hardware_profiler::default_audio_models_dir;
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
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub corrections: CorrectionsConfig,
}

/// Memory management configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Minutes of inactivity before auto-unloading the whisper server (0 = never)
    #[serde(default = "default_idle_unload_minutes")]
    pub idle_unload_minutes: u32,
}

fn default_idle_unload_minutes() -> u32 {
    5
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            idle_unload_minutes: default_idle_unload_minutes(),
        }
    }
}
/// Runtime recommendation preference for adaptive policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePreference {
    #[default]
    #[serde(alias = "embedded_whisper_rs")]
    Auto,
    ForceGpu,
    ForceCpu,
}

impl RuntimePreference {
    pub fn as_str(&self) -> &'static str {
        match self {
            RuntimePreference::Auto => "auto",
            RuntimePreference::ForceGpu => "force_gpu",
            RuntimePreference::ForceCpu => "force_cpu",
        }
    }
}

/// Runtime policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub preference: RuntimePreference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_true")]
    pub show_post_transcription_overlay: bool,
    #[serde(default = "default_overlay_result_seconds")]
    pub post_transcription_overlay_seconds: u32,
}

fn default_overlay_result_seconds() -> u32 {
    3
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_post_transcription_overlay: default_true(),
            post_transcription_overlay_seconds: default_overlay_result_seconds(),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            preference: RuntimePreference::Auto,
        }
    }
}

/// Storage layout configuration used by history/recordings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_audio_history_dir")]
    pub audio_history_dir: PathBuf,
    #[serde(default = "default_transcripts_dir")]
    pub transcripts_dir: PathBuf,
}

fn default_history_root_dir() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Dictator")
        .join("History")
}

fn default_audio_history_dir() -> PathBuf {
    default_history_root_dir().join("audio")
}

fn default_transcripts_dir() -> PathBuf {
    default_history_root_dir().join("transcripts")
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            audio_history_dir: default_audio_history_dir(),
            transcripts_dir: default_transcripts_dir(),
        }
    }
}

/// Deterministic post-ASR corrections dictionary configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub auto_learn: bool,
    #[serde(default = "default_auto_learn_min_repeats")]
    pub min_auto_learn_repeats: u32,
    #[serde(default)]
    pub dictionary_path: Option<PathBuf>,
}

fn default_auto_learn_min_repeats() -> u32 {
    3
}

impl Default for CorrectionsConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            auto_learn: default_true(),
            min_auto_learn_repeats: default_auto_learn_min_repeats(),
            dictionary_path: None,
        }
    }
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

/// Whisper inference backend
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WhisperBackend {
    /// Embedded whisper.cpp via whisper-rs (default).
    /// Uses GGML .bin model files. No Python or external server required.
    /// GPU acceleration available via `--features cuda` at build time.
    #[default]
    #[serde(alias = "embedded_whisper_rs")]
    Embedded,
    /// Legacy Python HTTP server (faster-whisper / CTranslate2 format).
    /// Requires `whisper_server.py` and Python environment.
    Server,
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
    /// Inference backend: "embedded" (default) or "server" (legacy Python).
    #[serde(default)]
    pub backend: WhisperBackend,
}

impl WhisperConfig {
    /// Returns the directory to scan for models.
    ///
    /// Resolution order:
    /// 1. Explicit `[whisper] models_dir` in config
    /// 2. `AUDIO_MODELS_DIR` environment variable (new canonical override)
    /// 3. `WHISPER_MODELS_DIR` environment variable (legacy override)
    /// 4. Canonical shared path from runtime contract (`AudioModels`)
    /// 5. Legacy `%LocalAppData%\whisper-models\` (migration fallback)
    /// 6. Parent directory of `model_path`
    pub fn effective_models_dir(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.models_dir {
            return Some(dir.clone());
        }
        if let Ok(env_dir) = std::env::var("AUDIO_MODELS_DIR") {
            return Some(PathBuf::from(env_dir));
        }
        if let Ok(env_dir) = std::env::var("WHISPER_MODELS_DIR") {
            return Some(PathBuf::from(env_dir));
        }

        let canonical = default_audio_models_dir();
        if canonical.exists() {
            return Some(canonical);
        }

        if let Some(local_app_data) = dirs::data_local_dir() {
            let legacy_dir = local_app_data.join("whisper-models");
            if legacy_dir.exists() {
                return Some(legacy_dir);
            }
        }

        Some(canonical).or_else(|| self.model_path.parent().map(|p| p.to_path_buf()))
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
                key: "right_ctrl".into(),
            },
            audio: AudioConfig {
                device: None,
                sample_rate: 16000,
            },
            whisper: WhisperConfig {
                model_path: PathBuf::from("models/ggml-large-v3.bin"),
                language: "ru".into(),
                models_dir: None,
                backend: WhisperBackend::Embedded,
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
            memory: MemoryConfig::default(),
            runtime: RuntimeConfig::default(),
            ui: UiConfig::default(),
            storage: StorageConfig::default(),
            corrections: CorrectionsConfig::default(),
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
    /// Load configuration from file or create default.
    ///
    /// Robustness rules:
    /// - tolerate legacy backend values from previous builds;
    /// - if parsing still fails, fall back to default config so app can boot.
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path();

        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            match toml::from_str::<Config>(&content) {
                Ok(config) => {
                    if content.contains("embedded_whisper_rs") {
                        let migrated = content.replace("embedded_whisper_rs", "embedded");
                        let _ = fs::write(&config_path, migrated);
                    }
                    Ok(config)
                }
                Err(primary_err) => {
                    // Legacy compatibility: old builds persisted this backend name.
                    let repaired = content.replace("embedded_whisper_rs", "embedded");
                    if repaired != content {
                        if let Ok(config) = toml::from_str::<Config>(&repaired) {
                            let _ = fs::write(&config_path, &repaired);
                            return Ok(config);
                        }
                    }

                    eprintln!(
                        "[CONFIG] parse failed for {:?}: {}. Falling back to default config.",
                        config_path, primary_err
                    );
                    let config = Config::default();
                    let _ = config.save();
                    Ok(config)
                }
            }
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
        assert_eq!(config.hotkey.key, "right_ctrl");
        assert_eq!(config.audio.sample_rate, 16000);
        assert_eq!(config.memory.idle_unload_minutes, 5);
    }

    #[test]
    fn test_backward_compat_no_memory_section() {
        let toml = r#"
[hotkey]
modifiers = []
key = "right_alt"

[audio]
sample_rate = 16000

[whisper]
model_path = "models/ggml-large-v3.bin"
language = "ru"

[ollama]
enabled = false
url = "http://localhost:11434"
model = "glm-4.7-flash"
"#;
        let config: Config = toml::from_str(toml).expect("should parse without [memory]");
        assert_eq!(config.memory.idle_unload_minutes, 5);
    }

    #[test]
    fn test_whisper_models_dir_env_var() {
        // SAFETY: single-threaded test, no concurrent env access
        unsafe { std::env::set_var("WHISPER_MODELS_DIR", "C:\\models\\shared") };
        let whisper = WhisperConfig {
            model_path: PathBuf::from("models/ggml-large-v3.bin"),
            language: "ru".into(),
            models_dir: None,
            backend: WhisperBackend::Embedded,
        };
        let dir = whisper.effective_models_dir();
        unsafe { std::env::remove_var("WHISPER_MODELS_DIR") };
        assert_eq!(dir, Some(PathBuf::from("C:\\models\\shared")));
    }
}
