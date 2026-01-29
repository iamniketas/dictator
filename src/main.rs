// AGENT: kimi | TASK: task_73a9e22d75f1 | TIMESTAMP: 2026-01-29T14:06:32.849719
// AUTO-GENERATED: Do not edit manually. Delegate changes via orchestrator.
// SOURCE: http://localhost:8000/task/task_73a9e22d75f1/report

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
use dictator::streaming::{StreamingController, StreamingState};
use dictator::ui;
use dictator::overlay::{OverlayConfig, OverlayWindow};

use std::sync::Mutex;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging to console
    tracing_subscriber::fmt()
        .with_env_filter("dictator=debug")
        .init();

    info!("Dictator starting... (DEBUG CONSOLE MODE)");

    // Load configuration
    let config = Config::load()?;
    info!("Config loaded, hotkey: {:?}", config.hotkey);

    // Create Ollama client (blocking client)
    let ollama = Arc::new(OllamaClient::new(&config.ollama.url, &config.ollama.model));

    // Check Ollama availability
    if ollama.health_check() {
        info!("Ollama server is available at {}", config.ollama.url);
    } else {
        info!("Warning: Ollama server not available. Text correction will be skipped.");
    }

    // Create shared audio recorder
    let recorder = Arc::new(AudioRecorder::new()?);

    // Create streaming controller with ChunkDetector for parallel chunk processing
    let streaming_controller = Arc::new(StreamingController::new(
        config.whisper.language.clone(),
    )?);

    // Create overlay window for real-time text display
    let overlay_config = OverlayConfig::default();
    let overlay = match OverlayWindow::new(overlay_config) {
        Ok(ov) => {
            info!("Overlay window created successfully");
            Some(Arc::new(Mutex::new(ov)))
        }
        Err(e) => {
            info!("Warning: Failed to create overlay window: {}", e);
            None
        }
    };

    // Start hotkey listener
    let (tx, rx) = mpsc::channel();
    let _hotkey_handle = input::start_hotkey_listener(tx);

    // Clone for async task
    let recorder_clone = recorder.clone();
    let ollama_clone = ollama.clone();
    let streaming_controller_clone = streaming_controller.clone();
    let overlay_clone = overlay.clone();
    let config_clone = config.clone();

    // Spawn blocking task for hotkey handling
    let hotkey_task = tokio::task::spawn_blocking(move || {
        while let Ok(event) = rx.recv() {
            match event {
                HotkeyEvent::RecordStart => {
                    info!("==> Starting audio recording...");

                    // Show overlay with initial state
                    if let Some(overlay) = overlay_clone.as_ref() {
                        let ov = overlay.lock().unwrap();
                        ov.position_near_cursor();
                        ov.show("Recording...");
                    }

                    // Start recording
                    if let Err(e) = recorder_clone.start_recording() {
                        error!("Failed to start recording: {}", e);
                        continue;
                    }

                    // Start streaming transcription pipeline
                    if let Err(e) = streaming_controller_clone.start_recording() {
                        error!("Failed to start streaming: {}", e);
                        recorder_clone.stop_recording().ok();
                        continue;
                    }

                    // Start audio capture and streaming thread
                    let streaming_controller = streaming_controller_clone.clone();
                    let recorder = recorder_clone.clone();
                    let overlay = overlay_clone.clone();

                    std::thread::spawn(move || {
                        // Audio capture loop with ChunkDetector integration
                        loop {
                            // Check if still recording
                            if !streaming_controller.get_state().is_recording() {
                                break;
                            }

                            match recorder.get_unprocessed_buffer() {
                                Ok((data, _start_idx)) => {
                                    if data.is_empty() {
                                        std::thread::sleep(std::time::Duration::from_millis(10));
                                        continue;
                                    }

                                    // Process audio chunk through streaming pipeline
                                    // ChunkDetector inside StreamingController handles parallel chunk processing
                                    if let Err(e) = streaming_controller.process_audio(&data) {
                                        error!("Failed to process audio chunk: {}", e);
                                    }

                                    // Update overlay with real-time accumulated text
                                    let accumulated_text = streaming_controller.get_accumulated_text();
                                    if !accumulated_text.is_empty() {
                                        if let Some(overlay) = overlay.as_ref() {
                                            let ov = overlay.lock().unwrap();
                                            ov.set_text(&accumulated_text);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Error getting audio data: {}", e);
                                    break;
                                }
                            }

                            // Small delay to prevent busy waiting
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }

                        info!("Audio capture thread stopped");
                    });
                }
                HotkeyEvent::RecordStop => {
                    info!("==> Stopping audio recording...");

                    // Stop streaming transcription pipeline first
                    let _ = streaming_controller_clone.stop_recording();

                    // Stop recording
                    if let Err(e) = recorder.stop_recording() {
                        error!("Error stopping recording: {}", e);
                        continue;
                    }

                    // Wait for audio capture thread to finish
                    std::thread::sleep(std::time::Duration::from_millis(200));

                    // Hide overlay after capture thread stopped
                    if let Some(overlay) = overlay_clone.as_ref() {
                        let ov = overlay.lock().unwrap();
                        ov.hide();
                    }

                    // Get final audio for complete transcription
                    let final_audio = streaming_controller_clone.get_final_audio();

                    if final_audio.is_empty() {
                        info!("No audio recorded");
                        continue;
                    }

                    // Get accumulated text from streaming transcription (real-time results)
                    let streaming_text = streaming_controller_clone.get_final_text();
                    info!("Streaming transcription accumulated: \"{}\"", streaming_text);

                    // Final transcription pass on complete audio for higher quality
                    let final_transcribed_text = match dictator::transcribe::transcribe_audio(
                        &final_audio,
                        &config_clone.whisper.language,
                    ) {
                        Ok(text) => {
                            info!("Final transcription result: \"{}\"", text);
                            text
                        }
                        Err(e) => {
                            error!("Final transcription error: {}", e);
                            // Fall back to streaming text if available
                            if !streaming_text.is_empty() {
                                info!("Using streaming transcription as fallback");
                                streaming_text.clone()
                            } else {
                                continue;
                            }
                        }
                    };

                    // Use the better result: prefer final transcription, but use streaming if empty
                    let text_to_process = if final_transcribed_text.trim().is_empty() && !streaming_text.is_empty() {
                        info!("Using streaming transcription as fallback");
                        streaming_text
                    } else {
                        final_transcribed_text
                    };

                    if text_to_process.is_empty() {
                        info!("No text transcribed");
                        continue;
                    }

                    // Preserve final LLM correction after recording stops
                    let final_text = if ollama_clone.health_check() {
                        info!("Applying LLM correction...");
                        match ollama_clone.correct_text(&text_to_process) {
                            Ok(corrected) => {
                                info!("LLM correction applied");
                                corrected
                            }
                            Err(e) => {
                                error!("LLM correction error: {}", e);
                                text_to_process // Use raw text if correction fails
                            }
                        }
                    } else {
                        info!("Ollama not available, using raw transcription");
                        text_to_process
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

    // Run system tray in blocking task
    let tray_task = tokio::task::spawn_blocking(|| {
        info!("Starting system tray...");
        if let Err(e) = ui::run_tray() {
            error!("System tray error: {}", e);
        }
    });

    // Wait for tray to complete (blocking)
    let _ = tray_task.await;

    info!("Dictator shutting down");

    // Force process exit to terminate all threads including hotkey listener
    std::process::exit(0);
}