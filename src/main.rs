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
use dictator::overlay_win32::{OverlayConfig, OverlayWindow};
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

    // Create overlay window
    let overlay_config = OverlayConfig::default();
    let overlay = Arc::new(OverlayWindow::new(overlay_config)?);

    // Start hotkey listener
    let (tx, rx) = mpsc::channel();
    let _hotkey_handle = input::start_hotkey_listener(tx);

    // Handle hotkey events in a separate thread
    let recorder_clone = recorder.clone();
    let ollama_clone = ollama.clone();
    let overlay_clone = overlay.clone();

    std::thread::spawn(move || {
        let mut saved_hwnd: Option<isize> = None;

        while let Ok(event) = rx.recv() {
            match event {
                HotkeyEvent::RecordStart { hwnd } => {
                    info!("==> Starting audio recording...");
                    // Save the window handle for later focus restoration
                    saved_hwnd = Some(hwnd);
                    info!("Saved focus window handle: {}", hwnd);

                    overlay_clone.set_recording(true);
                    if let Err(e) = recorder_clone.start_recording() {
                        error!("Failed to start recording: {}", e);
                        overlay_clone.hide();
                    }
                }
                HotkeyEvent::RecordStop { hwnd: _ } => {
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
                        overlay_clone.hide();
                        continue;
                    }

                    // Show transcription status
                    overlay_clone.show("Расшифровка...");

                    // Transcribe audio
                    let raw_text =
                        match transcribe::transcribe_audio(&audio_data, &config.whisper.language) {
                            Ok(text) => text,
                            Err(e) => {
                                error!("Transcription error: {}", e);
                                overlay_clone.hide();
                                continue;
                            }
                        };

                    if raw_text.is_empty() {
                        info!("No text transcribed");
                        overlay_clone.hide();
                        continue;
                    }

                    // Show correction status
                    overlay_clone.show("Исправление...");

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

                    // Copy to clipboard (always keep a backup)
                    if let Err(e) = clipboard_win::set_clipboard_string(&final_text) {
                        error!("Failed to copy to clipboard: {}", e);
                    } else {
                        info!("Text copied to clipboard");
                    }

                    // Restore focus to the original window before injecting text
                    if let Some(hwnd) = saved_hwnd {
                        if let Err(e) = input::set_foreground_window(hwnd) {
                            error!("Failed to restore focus: {}", e);
                        }
                        // Small delay to ensure focus is restored
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }

                    // Show overlay with final text
                    overlay_clone.show(&final_text);

                    // Inject text into focused application
                    if let Err(e) = input::inject_text(&final_text) {
                        error!("Failed to inject text: {}", e);
                    }

                    // Hide overlay after delay
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    overlay_clone.hide();
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
