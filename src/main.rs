#![windows_subsystem = "windows"]
//! Dictator - Voice dictation service for Windows

use anyhow::Result;
use std::sync::mpsc;
use std::sync::Arc;
use tracing::{error, info, warn};

use dictator::audio::AudioRecorder;
use dictator::config::Config;
use dictator::input::{self, HotkeyEvent};
use dictator::llm::OllamaClient;
use dictator::overlay_win32::{OverlayConfig, OverlayWindow};
use dictator::streaming::{StreamingEvent, StreamingTranscriber};
use dictator::transcribe;
use dictator::ui;

fn main() -> Result<()> {
    // Initialize logging to file
    let log_dir = dirs::data_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("dictator")
        .join("logs");

    // Create log directory if it doesn't exist
    std::fs::create_dir_all(&log_dir)?;

    let log_file = log_dir.join("dictator.log");
    let file_appender = tracing_appender::rolling::never(&log_dir, "dictator.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_env_filter("dictator=debug")
        .init();

    info!("Dictator starting...");
    info!("Log file location: {:?}", log_file);

    // Load configuration
    let config = Config::load()?;
    info!("Config loaded, hotkey: {:?}", config.hotkey);

    // Create Ollama client
    let ollama = Arc::new(OllamaClient::new(&config.ollama.url, &config.ollama.model));

    // Log Ollama status
    if config.ollama.enabled {
        info!("Ollama correction enabled ({})", config.ollama.url);
    } else {
        info!("Ollama correction disabled for speed");
    }

    // Create shared audio recorder
    let recorder = Arc::new(AudioRecorder::new()?);

    // Create overlay window
    let overlay_config = OverlayConfig::default();
    let overlay = Arc::new(OverlayWindow::new(overlay_config)?);

    // Start hotkey listener
    let (tx, rx) = mpsc::channel();
    let _hotkey_handle = input::start_hotkey_listener(tx);

    // Create streaming channel (if streaming is enabled)
    let (streaming_tx, streaming_rx) = std::sync::mpsc::channel::<StreamingEvent>();
    let streaming_enabled = config.streaming.enabled;
    info!("[MAIN] Streaming enabled: {}", streaming_enabled);

    // Handle hotkey events in a separate thread
    let recorder_clone = recorder.clone();
    let ollama_clone = ollama.clone();
    let overlay_clone = overlay.clone();
    let config_clone = config.clone();
    let streaming_tx_clone = streaming_tx.clone();

    std::thread::spawn(move || {
        let mut saved_hwnd: Option<isize> = None;
        let mut is_recording = false;
        let mut streaming_transcriber: Option<StreamingTranscriber> = None;
        let mut accumulated_text = String::new();

        info!("[MAIN] Event handler thread started, waiting for hotkey events...");

        loop {
            info!(
                "[MAIN] Waiting for next event... (is_recording: {})",
                is_recording
            );

            // Use recv_timeout to periodically check streaming events even without hotkey
            let event = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(evt) => {
                    info!("[MAIN] ===> RECEIVED event: {:?}", evt);
                    Some(evt)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // No hotkey event, continue to check streaming events
                    None
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    error!("[MAIN] Channel closed! Exiting event loop.");
                    break;
                }
            };

            // Process streaming events (non-blocking) - do this every iteration
            while let Ok(streaming_event) = streaming_rx.try_recv() {
                match streaming_event {
                    StreamingEvent::PartialText(text) => {
                        info!("[MAIN] 📥 Streaming partial text: \"{}\"", text);
                        accumulated_text = text.clone();
                        overlay_clone.update_partial_text(&text);
                    }
                    StreamingEvent::FinalText(text) => {
                        info!("[MAIN] 🏁 Streaming final text: \"{}\"", text);
                        accumulated_text = text;
                    }
                    StreamingEvent::Error(e) => {
                        error!("[MAIN] Streaming error: {}", e);
                    }
                }
            }

            // If no hotkey event, continue loop
            let event = match event {
                Some(evt) => evt,
                None => continue,
            };

            match event {
                HotkeyEvent::RecordStart { hwnd } => {
                    if is_recording {
                        warn!("[MAIN] Received RecordStart but already recording! Ignoring.");
                        continue;
                    }

                    info!("[MAIN] ===> PROCESSING RecordStart");
                    is_recording = true;

                    // Save the window handle for later focus restoration
                    saved_hwnd = Some(hwnd);
                    info!("[MAIN] Saved focus window handle: {}", hwnd);

                    info!("[MAIN] Calling overlay.set_recording(true)...");
                    overlay_clone.set_recording(true);

                    info!("[MAIN] Calling recorder.start_recording()...");
                    if let Err(e) = recorder_clone.start_recording() {
                        error!("[MAIN] FAILED to start recording: {}", e);
                        is_recording = false;
                        overlay_clone.hide();
                    } else {
                        info!("[MAIN] Recording started successfully!");

                        // Start streaming if enabled
                        if config_clone.streaming.enabled {
                            info!("[MAIN] Starting streaming transcription...");
                            accumulated_text.clear();
                            streaming_transcriber = Some(StreamingTranscriber::new(
                                streaming_tx_clone.clone(),
                                config_clone.whisper.language.clone(),
                            ));
                            if let Some(ref mut st) = streaming_transcriber {
                                st.start(recorder_clone.clone());
                            }
                        }
                    }
                }
                HotkeyEvent::RecordStop { hwnd: _ } => {
                    if !is_recording {
                        warn!("[MAIN] Received RecordStop but not recording! Ignoring.");
                        continue;
                    }

                    info!("[MAIN] ===> PROCESSING RecordStop");
                    is_recording = false;

                    // CRITICAL: Stop streaming FIRST while recording is still active
                    // This allows streaming to read the final buffer before it's cleared
                    if let Some(mut st) = streaming_transcriber.take() {
                        info!("[MAIN] Stopping streaming transcription (while recorder still active)...");
                        st.stop();
                        // Wait for streaming to send final text (with timeout)
                        let mut final_text_received = false;
                        let timeout_duration = std::time::Duration::from_millis(3000);
                        let start_time = std::time::Instant::now();
                        while start_time.elapsed() < timeout_duration {
                            match streaming_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                                Ok(streaming_event) => {
                                    match streaming_event {
                                        StreamingEvent::FinalText(text) => {
                                            info!(
                                                "[MAIN] 🏁 Streaming final text received: \"{}\"",
                                                text
                                            );
                                            accumulated_text = text;
                                            final_text_received = true;
                                            break;
                                        }
                                        StreamingEvent::PartialText(text) => {
                                            info!("[MAIN] 📥 Late partial text: \"{}\"", text);
                                            // Update accumulated text even if we receive partial after stop
                                            accumulated_text = text;
                                        }
                                        _ => {}
                                    }
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                        if !final_text_received {
                            info!("[MAIN] No final text received from streaming, using accumulated text");
                        }
                    }

                    // NOW stop recording (after streaming has processed final buffer)
                    info!("[MAIN] Calling recorder.stop_recording()...");
                    let audio_data = match recorder_clone.stop_recording() {
                        Ok(data) => {
                            info!("[MAIN] Got {} samples of audio", data.len());
                            data
                        }
                        Err(e) => {
                            error!("[MAIN] FAILED to stop recording: {}", e);
                            continue;
                        }
                    };

                    if audio_data.is_empty() {
                        info!("No audio recorded");
                        overlay_clone.hide();
                        continue;
                    }

                    // Determine raw text: use streaming results if available, otherwise transcribe full audio
                    let raw_text = if !accumulated_text.is_empty() {
                        info!(
                            "[MAIN] Using streaming accumulated text: \"{}\"",
                            accumulated_text
                        );
                        accumulated_text.clone()
                    } else {
                        // Show transcription status
                        overlay_clone.show("Расшифровка...");

                        // Transcribe audio
                        match transcribe::transcribe_audio(&audio_data, &config.whisper.language) {
                            Ok(text) => text,
                            Err(e) => {
                                error!("Transcription error: {}", e);
                                overlay_clone.hide();
                                continue;
                            }
                        }
                    };

                    if raw_text.is_empty() {
                        info!("No text transcribed");
                        overlay_clone.hide();
                        continue;
                    }

                    // Correct text with Ollama (if enabled in config)
                    let final_text = if config_clone.ollama.enabled {
                        overlay_clone.show("Исправление...");
                        match ollama_clone.correct_text(&raw_text) {
                            Ok(corrected) => corrected,
                            Err(e) => {
                                error!("LLM correction error: {}", e);
                                raw_text // Use raw text if correction fails
                            }
                        }
                    } else {
                        info!("Ollama disabled in config, using raw transcription");
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

                    // Reset accumulated text for next recording
                    accumulated_text.clear();
                }
            }

            // Process streaming events (non-blocking)
            while let Ok(streaming_event) = streaming_rx.try_recv() {
                match streaming_event {
                    StreamingEvent::PartialText(text) => {
                        info!("[MAIN] 📥 Streaming partial text: \"{}\"", text);
                        accumulated_text = text.clone();
                        overlay_clone.update_partial_text(&text);
                    }
                    StreamingEvent::FinalText(text) => {
                        info!("[MAIN] 🏁 Streaming final text: \"{}\"", text);
                        accumulated_text = text;
                    }
                    StreamingEvent::Error(e) => {
                        error!("[MAIN] Streaming error: {}", e);
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
