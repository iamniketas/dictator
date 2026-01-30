//! Streaming module - Real-time transcription with synchronous polling
//!
//! This is a simplified version without async/tokio to avoid threading issues.
//! Uses simple thread::sleep for polling every 3 seconds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{error, info, warn};

use crate::audio::AudioRecorder;
use crate::transcribe::transcribe_audio;

/// Events from streaming transcription
#[derive(Debug, Clone)]
pub enum StreamingEvent {
    /// New partial text available
    PartialText(String),
    /// Final text when streaming stops
    FinalText(String),
    /// Error during streaming
    Error(String),
}

/// Simple synchronous streaming transcriber
pub struct StreamingTranscriber {
    /// Channel to send events to main thread
    event_tx: mpsc::Sender<StreamingEvent>,
    /// Stop signal
    stop_signal: Arc<AtomicBool>,
    /// Thread handle
    thread_handle: Option<JoinHandle<()>>,
    /// Language for transcription
    language: String,
}

impl StreamingTranscriber {
    /// Create new streaming transcriber
    pub fn new(event_tx: mpsc::Sender<StreamingEvent>, language: String) -> Self {
        Self {
            event_tx,
            stop_signal: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
            language,
        }
    }

    /// Start streaming transcription
    pub fn start(&mut self, recorder: Arc<AudioRecorder>) {
        if self.thread_handle.is_some() {
            warn!("[STREAMING] Already started, ignoring");
            return;
        }

        info!("[STREAMING] Starting simple synchronous streaming");

        // Reset stop signal
        self.stop_signal.store(false, Ordering::SeqCst);

        let event_tx = self.event_tx.clone();
        let stop_signal = self.stop_signal.clone();
        let language = self.language.clone();

        // Spawn simple thread with sleep-based polling
        let handle = thread::spawn(move || {
            info!("[STREAMING] Thread started, will poll every 3 seconds");

            let mut accumulated_text = String::new();
            let mut last_processed_samples: usize = 0;
            let mut iteration = 0;

            loop {
                iteration += 1;

                // Check if we should stop
                if stop_signal.load(Ordering::SeqCst) {
                    info!(
                        "[STREAMING] Stop signal received, breaking loop at iteration {}",
                        iteration
                    );
                    break;
                }

                // Sleep for 3 seconds (synchronous, no async)
                thread::sleep(Duration::from_secs(3));

                // Check again after sleep
                if stop_signal.load(Ordering::SeqCst) {
                    info!("[STREAMING] Stop signal after sleep, breaking");
                    break;
                }

                // Get current buffer
                info!("[STREAMING] Iteration {}: Getting buffer...", iteration);
                let (audio_data, start_idx) = match recorder.get_unprocessed_buffer() {
                    Ok((data, idx)) => {
                        info!(
                            "[STREAMING] Got buffer: {} samples, start_idx: {}",
                            data.len(),
                            idx
                        );
                        (data, idx)
                    }
                    Err(e) => {
                        error!("[STREAMING] Failed to get buffer: {}", e);
                        continue;
                    }
                };

                // Calculate new samples since last check
                let new_samples = audio_data.len().saturating_sub(last_processed_samples);
                let new_seconds = new_samples as f32 / 16000.0;

                info!(
                    "[STREAMING] New samples: {} ({:.1}s)",
                    new_samples, new_seconds
                );

                // Skip if not enough new data (less than 1 second)
                if new_seconds < 1.0 {
                    info!(
                        "[STREAMING] Not enough new audio ({:.1}s < 1.0s), skipping",
                        new_seconds
                    );
                    continue;
                }

                // Extract new portion
                let audio_to_process = if start_idx < audio_data.len() {
                    &audio_data[start_idx..]
                } else {
                    &audio_data[last_processed_samples..]
                };

                if audio_to_process.is_empty() {
                    warn!("[STREAMING] Audio to process is empty, skipping");
                    continue;
                }

                info!(
                    "[STREAMING] Processing {} samples...",
                    audio_to_process.len()
                );

                // Send to Whisper (blocking call, but in separate thread)
                match transcribe_audio(audio_to_process, &language) {
                    Ok(partial_text) => {
                        if !partial_text.is_empty() {
                            // Append with space
                            if !accumulated_text.is_empty() {
                                accumulated_text.push(' ');
                            }
                            accumulated_text.push_str(&partial_text);

                            info!("[STREAMING] Partial: \"{}\"", partial_text);
                            info!("[STREAMING] Accumulated: \"{}\"", accumulated_text);

                            // Send only the current chunk to main thread
                            if let Err(e) =
                                event_tx.send(StreamingEvent::PartialText(partial_text.clone()))
                            {
                                error!("[STREAMING] Failed to send event: {}", e);
                            }
                        } else {
                            info!("[STREAMING] Whisper returned empty text");
                        }
                    }
                    Err(e) => {
                        warn!("[STREAMING] Transcription error (continuing): {}", e);
                        // Continue anyway, don't break
                    }
                }

                // Update last processed position
                last_processed_samples = audio_data.len();
                info!(
                    "[STREAMING] Updated last_processed_samples to {}",
                    last_processed_samples
                );
            }

            // Send final text
            info!("[STREAMING] Sending final text: \"{}\"", accumulated_text);
            let _ = event_tx.send(StreamingEvent::FinalText(accumulated_text));
            info!("[STREAMING] Thread exiting");
        });

        self.thread_handle = Some(handle);
        info!("[STREAMING] Thread spawned successfully");
    }

    /// Stop streaming transcription
    pub fn stop(&mut self) {
        info!("[STREAMING] Stopping...");

        // Signal stop
        self.stop_signal.store(true, Ordering::SeqCst);

        // Wait for thread to finish
        if let Some(handle) = self.thread_handle.take() {
            info!("[STREAMING] Waiting for thread to finish...");
            match handle.join() {
                Ok(_) => info!("[STREAMING] Thread finished successfully"),
                Err(_) => error!("[STREAMING] Thread panicked!"),
            }
        }

        info!("[STREAMING] Stopped");
    }

    /// Check if streaming is active
    pub fn is_active(&self) -> bool {
        self.thread_handle.is_some()
    }
}

impl Drop for StreamingTranscriber {
    fn drop(&mut self) {
        if self.is_active() {
            info!("[STREAMING] Dropping while active, stopping...");
            self.stop();
        }
    }
}
