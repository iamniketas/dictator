#![windows_subsystem = "windows"]
//! Dictator - Voice dictation service for Windows

use anyhow::Result;
use std::sync::mpsc;
use std::sync::Arc;
use tracing::{error, info};

use dictator::audio::AudioRecorder;
use dictator::config::Config;
use dictator::input::{self, HotkeyEvent};
use dictator::llm::OllamaClient;
use dictator::transcribe;
use dictator::ui;

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("dictator=debug")
        .init();

    info!("Dictator starting...");

    // Load configuration
    let config = Config::load()?;
    info!("Config loaded, hotkey: {:?}", config.hotkey);

    // Create Ollama client
    let ollama = Arc::new(OllamaClient::new(&config.ollama.url, &config.ollama.model));

    // Check Ollama availability
    if ollama.health_check() {
        info!("Ollama server is available at {}", config.ollama.url);
    } else {
        info!("Warning: Ollama server not available. Text correction will be skipped.");
    }

    // Create shared audio recorder
    let recorder = Arc::new(AudioRecorder::new()?);

    // Start hotkey listener
    let (tx, rx) = mpsc::channel();
    let _hotkey_handle = input::start_hotkey_listener(tx);

    // Handle hotkey events in a separate thread
    let recorder_clone = recorder.clone();
    let ollama_clone = ollama.clone();

    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            match event {
                HotkeyEvent::RecordStart => {
                    info!("==> Starting audio recording...");
                    if let Err(e) = recorder_clone.start_recording() {
                        error!("Failed to start recording: {}", e);
                    }
                }
                HotkeyEvent::RecordStop => {
                    info!("==> Stopping audio recording...");

                    // Get audio data
                    let audio_data = match recorder_clone.stop_recording() {
                        Ok(data) => data,
                        Err(e) => {
                            error!("Error stopping recording: {}", e);
                            continue;
                        }
                    };

                    if audio_data.is_empty() {
                        info!("No audio recorded");
                        continue;
                    }

                    // Transcribe audio
                    let raw_text = match transcribe::transcribe_audio(
                        &audio_data,
                        &config.whisper.language,
                    ) {
                        Ok(text) => text,
                        Err(e) => {
                            error!("Transcription error: {}", e);
                            continue;
                        }
                    };

                    if raw_text.is_empty() {
                        info!("No text transcribed");
                        continue;
                    }

                    // Correct text with Ollama (if available)
                    let final_text = if ollama_clone.health_check() {
                        match ollama_clone.correct_text(&raw_text) {
                            Ok(corrected) => corrected,
                            Err(e) => {
                                error!("LLM correction error: {}", e);
                                raw_text // Use raw text if correction fails
                            }
                        }
                    } else {
                        info!("Ollama not available, using raw transcription");
                        raw_text
                    };

                    info!("========================================");
                    info!("FINAL TEXT: {}", final_text);
                    info!("========================================");

                    // Inject text into focused application
                    if let Err(e) = input::inject_text(&final_text) {
                        error!("Failed to inject text: {}", e);
                    }
                }
            }
        }
    });

    // Run system tray (blocking)
    info!("Starting system tray...");
    ui::run_tray()?;

    info!("Dictator shutting down");
    Ok(())
}