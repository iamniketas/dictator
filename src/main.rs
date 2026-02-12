#![windows_subsystem = "windows"]
//! Dictator - Voice dictation service for Windows

use anyhow::Result;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use dictator::audio::AudioRecorder;
use dictator::config::Config;
use dictator::input::{self, HotkeyEvent};
use dictator::llm::OllamaClient;
use dictator::overlay_win32::{OverlayConfig, OverlayWindow};
use dictator::streaming::{StreamingEvent, StreamingTranscriber};
use dictator::transcribe;
use dictator::ui;
use dictator::whisper_server::WhisperServerManager;

fn estimate_recording_size_mb(elapsed: Duration) -> f32 {
    let samples = elapsed.as_secs_f32() * 16000.0;
    (samples * std::mem::size_of::<f32>() as f32) / (1024.0 * 1024.0)
}

fn format_recording_status(
    elapsed: Duration,
    streaming_enabled: bool,
    chunk_seconds: u64,
    whisper_status: &str,
) -> String {
    let elapsed_secs = elapsed.as_secs_f32();
    let size_mb = estimate_recording_size_mb(elapsed);
    if streaming_enabled {
        format!(
            "Recording: {:.1}s | ~{:.2} MB | {}\nMode: streaming (chunk {}s)",
            elapsed_secs, size_mb, whisper_status, chunk_seconds
        )
    } else {
        format!(
            "Recording: {:.1}s | ~{:.2} MB | {}\nMode: full transcription",
            elapsed_secs, size_mb, whisper_status
        )
    }
}

fn format_transcribing_status(
    spinner: &str,
    elapsed_secs: f32,
    expected_secs: f32,
    progress: f32,
) -> String {
    let eta = (expected_secs - elapsed_secs).max(0.0);
    format!(
        "Transcribing {} {:.0}%\nElapsed: {:.1}s | ETA: ~{:.1}s",
        spinner,
        progress * 100.0,
        elapsed_secs,
        eta
    )
}

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

    // Always start in full transcription mode (streaming disabled by default).
    ui::set_streaming_enabled(false);
    ui::set_streaming_chunk_seconds(config.streaming.poll_interval);
    info!("[MAIN] Initial streaming state: {}", ui::is_streaming_enabled());
    info!(
        "[MAIN] Initial streaming chunk: {}s",
        ui::streaming_chunk_seconds()
    );

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

    // Create streaming channel
    let (streaming_tx, streaming_rx) = std::sync::mpsc::channel::<StreamingEvent>();

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
        let mut recording_started_at: Option<Instant> = None;
        let mut last_recording_second: u64 = u64::MAX;
        let mut avg_transcribe_ratio: f32 = 0.20;
        let mut whisper_ready = false;
        let mut whisper_status_text = String::from("Whisper: idle");
        let mut whisper_manager = WhisperServerManager::new(
            config_clone
                .whisper
                .model_path
                .to_string_lossy()
                .to_string(),
        );

        info!("[MAIN] Event handler thread started, waiting for hotkey events...");

        loop {
            tracing::debug!(
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
                    if is_recording {
                        if let Some(started_at) = recording_started_at {
                            let elapsed = started_at.elapsed();
                            let elapsed_sec = elapsed.as_secs();
                            if elapsed_sec != last_recording_second {
                                last_recording_second = elapsed_sec;
                                if !whisper_ready {
                                    match whisper_manager.poll_ready() {
                                        Ok(true) => {
                                            whisper_ready = true;
                                            whisper_status_text = "Whisper: ready".to_string();
                                        }
                                        Ok(false) => {
                                            whisper_status_text = "Whisper: starting...".to_string();
                                        }
                                        Err(e) => {
                                            whisper_status_text = "Whisper: startup error".to_string();
                                            warn!("[MAIN] Whisper poll error: {}", e);
                                        }
                                    }
                                }
                                let streaming_enabled = ui::is_streaming_enabled();
                                let chunk_seconds = ui::streaming_chunk_seconds();
                                let status = format_recording_status(
                                    elapsed,
                                    streaming_enabled,
                                    chunk_seconds,
                                    &whisper_status_text,
                                );
                                overlay_clone.update_status_text(&status);
                            }
                        }
                    }
                    // No hotkey event, continue to check streaming events and recording progress
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
                        overlay_clone.update_body_text(&text);
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
                    recording_started_at = Some(Instant::now());
                    last_recording_second = u64::MAX;

                    // Save the window handle for later focus restoration
                    saved_hwnd = Some(hwnd);
                    info!("[MAIN] Saved focus window handle: {}", hwnd);

                    overlay_clone.position_near_cursor();
                    info!("[MAIN] Calling overlay.set_recording(true)...");
                    overlay_clone.set_recording(true);
                    overlay_clone.update_status_text("Preparing transcription...");
                    overlay_clone.update_body_text("");

                    info!("[MAIN] Calling recorder.start_recording()...");
                    if let Err(e) = recorder_clone.start_recording() {
                        error!("[MAIN] FAILED to start recording: {}", e);
                        is_recording = false;
                        overlay_clone.hide();
                    } else {
                        info!("[MAIN] Recording started");
                        let streaming_enabled = ui::is_streaming_enabled();
                        let chunk_seconds = ui::streaming_chunk_seconds();
                        overlay_clone.update_status_text(&format_recording_status(
                            Duration::from_secs(0),
                            streaming_enabled,
                            chunk_seconds,
                            &whisper_status_text,
                        ));

                        whisper_ready = WhisperServerManager::is_healthy();
                        if whisper_ready {
                            whisper_status_text = "Whisper: ready".to_string();
                        } else {
                            whisper_status_text = "Whisper: starting...".to_string();
                            if let Err(e) = whisper_manager.start_if_needed() {
                                warn!("[MAIN] Whisper server warmup start failed: {}", e);
                                whisper_status_text = "Whisper: startup error".to_string();
                            }
                        }
                        overlay_clone.update_status_text(&format_recording_status(
                            Duration::from_secs(0),
                            streaming_enabled,
                            chunk_seconds,
                            &whisper_status_text,
                        ));

                        // Start streaming if enabled (from tray menu)
                        if ui::is_streaming_enabled() {
                            info!("[MAIN] Starting streaming transcription...");
                            accumulated_text.clear();
                            let chunk_seconds = ui::streaming_chunk_seconds();
                            info!("[MAIN] Streaming chunk duration: {}s", chunk_seconds);
                            streaming_transcriber = Some(StreamingTranscriber::new(
                                streaming_tx_clone.clone(),
                                config_clone.whisper.language.clone(),
                                chunk_seconds,
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
                    recording_started_at = None;
                    last_recording_second = u64::MAX;
                    whisper_ready = false;
                    whisper_status_text = "Whisper: idle".to_string();
                    overlay_clone.set_recording(false);

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
                            whisper_manager.stop_if_owned();
                            continue;
                        }
                    };

                    if audio_data.is_empty() {
                        info!("No audio recorded");
                        overlay_clone.hide();
                        whisper_manager.stop_if_owned();
                        continue;
                    }

                                        // Determine raw text: use streaming results if available, otherwise transcribe full audio
                    let raw_text = if !accumulated_text.is_empty() {
                        info!(
                            "[MAIN] Using streaming text ({} chars)",
                            accumulated_text.len()
                        );
                        accumulated_text.clone()
                    } else {
                        if let Err(e) = whisper_manager.ensure_running(Duration::from_secs(30)) {
                            error!("[MAIN] Failed to start Whisper server: {}", e);
                            overlay_clone.show("Whisper server startup error");
                            std::thread::sleep(Duration::from_secs(2));
                            overlay_clone.hide();
                            whisper_manager.stop_if_owned();
                            continue;
                        }

                        overlay_clone.update_status_text("Transcribing...");
                        overlay_clone.update_body_text("");

                        let audio_duration_secs = audio_data.len() as f32 / 16000.0;
                        let expected_secs = (audio_duration_secs * avg_transcribe_ratio).max(2.0);
                        let language = config.whisper.language.clone();
                        let audio_for_transcribe = audio_data.clone();
                        let (transcribe_tx, transcribe_rx) = mpsc::channel();

                        std::thread::spawn(move || {
                            let result = transcribe::transcribe_audio(&audio_for_transcribe, &language);
                            let _ = transcribe_tx.send(result);
                        });

                        let transcribe_started = Instant::now();
                        let spinner_frames = ["|", "/", "-", "\\"];
                        let mut spinner_index = 0usize;

                        let transcribed = loop {
                            match transcribe_rx.recv_timeout(Duration::from_millis(250)) {
                                Ok(result) => break result,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    let elapsed_secs = transcribe_started.elapsed().as_secs_f32();
                                    let progress = (elapsed_secs / expected_secs).min(0.99);
                                    let status = format_transcribing_status(
                                        spinner_frames[spinner_index % spinner_frames.len()],
                                        elapsed_secs,
                                        expected_secs,
                                        progress,
                                    );
                                    spinner_index = spinner_index.wrapping_add(1);
                                    overlay_clone.update_status_text(&status);
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => {
                                    break Err(anyhow::anyhow!(
                                        "Transcription worker thread disconnected unexpectedly"
                                    ));
                                }
                            }
                        };

                        match transcribed {
                            Ok(text) => {
                                let transcribe_elapsed = transcribe_started.elapsed().as_secs_f32();
                                if audio_duration_secs > 0.1 {
                                    let observed_ratio =
                                        (transcribe_elapsed / audio_duration_secs).clamp(0.05, 2.0);
                                    avg_transcribe_ratio =
                                        (avg_transcribe_ratio * 0.7 + observed_ratio * 0.3)
                                            .clamp(0.05, 2.0);
                                }

                                let words = text.split_whitespace().count();
                                let chars = text.chars().count();
                                overlay_clone.show(&format!(
                                    "Transcribed in {:.1}s\n{} words | {} chars",
                                    transcribe_elapsed, words, chars
                                ));
                                std::thread::sleep(Duration::from_millis(1200));
                                text
                            }
                            Err(e) => {
                                error!("Transcription error: {}", e);
                                overlay_clone.hide();
                                whisper_manager.stop_if_owned();
                                continue;
                            }
                        }
                    };

                    if raw_text.is_empty() {
                        info!("No text transcribed");
                        overlay_clone.hide();
                        whisper_manager.stop_if_owned();
                        continue;
                    }

                    // Correct text with Ollama (if enabled in config)
                    let final_text = if config_clone.ollama.enabled {
                        overlay_clone.update_status_text("Correcting...");
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
                    whisper_manager.stop_if_owned();

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
                        overlay_clone.update_body_text(&text);
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


