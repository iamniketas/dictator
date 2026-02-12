//! Streaming module - Real-time transcription with synchronous polling
//!
//! This is a simplified version without async/tokio to avoid threading issues.
//! Uses simple thread::sleep with configurable chunk length.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
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
    /// Chunk duration for polling/transcription
    chunk_duration_secs: u64,
}

impl StreamingTranscriber {
    /// Create new streaming transcriber
    pub fn new(
        event_tx: mpsc::Sender<StreamingEvent>,
        language: String,
        chunk_duration_secs: u64,
    ) -> Self {
        Self {
            event_tx,
            stop_signal: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
            language,
            chunk_duration_secs: chunk_duration_secs.max(1),
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
        let chunk_duration_secs = self.chunk_duration_secs;
        let chunk_duration = Duration::from_secs(chunk_duration_secs);

        // Spawn simple thread with sleep-based polling
        let handle = thread::spawn(move || {
            info!(
                "[STREAMING] Thread started, will poll every {} seconds",
                chunk_duration_secs
            );

            let mut accumulated_text = String::new();
            let mut last_processed_samples: usize = 0;
            let mut iteration = 0;

            let mut is_final_iteration = false;

            loop {
                iteration += 1;

                // Sleep for selected chunk duration (synchronous, no async)
                // But skip sleep on final iteration
                if !is_final_iteration {
                    thread::sleep(chunk_duration);
                }

                // Check if we should stop (but still process remaining audio)
                if stop_signal.load(Ordering::SeqCst) {
                    if is_final_iteration {
                        info!("[STREAMING] Final iteration complete, exiting");
                        break;
                    }
                    info!(
                        "[STREAMING] Stop signal received at iteration {}, processing final chunk...",
                        iteration
                    );
                    is_final_iteration = true;
                    // Continue to process final chunk without sleeping
                }

                // Get current buffer
                tracing::debug!("[STREAMING] Iteration {}: Getting buffer...", iteration);
                let (audio_data, start_idx) = match recorder.get_unprocessed_buffer() {
                    Ok((data, idx)) => {
                        tracing::debug!(
                            "[STREAMING] Got buffer: {} samples, start_idx: {}",
                            data.len(),
                            idx
                        );
                        (data, idx)
                    }
                    Err(e) => {
                        error!("[STREAMING] Failed to get buffer: {}", e);
                        if is_final_iteration {
                            break;
                        }
                        continue;
                    }
                };

                // Calculate new samples since last check
                let new_samples = audio_data.len().saturating_sub(last_processed_samples);
                let new_seconds = new_samples as f32 / 16000.0;

                tracing::debug!(
                    "[STREAMING] New samples: {} ({:.1}s)",
                    new_samples, new_seconds
                );

                // Skip if not enough new data (less than 1 second)
                // But don't skip on final iteration - process whatever is left
                if !is_final_iteration && new_seconds < 1.0 {
                    tracing::debug!(
                        "[STREAMING] Not enough new audio ({:.1}s < 1.0s), skipping",
                        new_seconds
                    );
                    continue;
                }

                // On final iteration, process all remaining audio regardless of length
                let audio_to_process = if is_final_iteration {
                    // Process everything from last_processed_samples to end
                    &audio_data[last_processed_samples..]
                } else if start_idx < audio_data.len() {
                    &audio_data[start_idx..]
                } else {
                    &audio_data[last_processed_samples..]
                };

                if audio_to_process.is_empty() {
                    warn!("[STREAMING] Audio to process is empty, skipping");
                    if is_final_iteration {
                        break;
                    }
                    continue;
                }

                // On final iteration, log how much we're processing
                if is_final_iteration {
                    info!(
                        "[STREAMING] FINAL iteration: Processing {} samples ({:.1}s)...",
                        audio_to_process.len(),
                        audio_to_process.len() as f32 / 16000.0
                    );
                } else {
                    tracing::debug!(
                        "[STREAMING] Processing {} samples...",
                        audio_to_process.len()
                    );
                }

                // Send to Whisper (blocking call, but in separate thread)
                match transcribe_audio(audio_to_process, &language) {
                    Ok(partial_text) => {
                        if !partial_text.is_empty() {
                            // Append with space
                            if !accumulated_text.is_empty() {
                                accumulated_text.push(' ');
                            }
                            accumulated_text.push_str(&partial_text);

                            tracing::debug!("[STREAMING] Partial ({} chars)", partial_text.len());
                            info!("[STREAMING] Accumulated: {} chars", accumulated_text.len());

                            // Send full accumulated text to main thread (not just current chunk)
                            if let Err(e) =
                                event_tx.send(StreamingEvent::PartialText(accumulated_text.clone()))
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
                tracing::debug!(
                    "[STREAMING] Updated last_processed_samples to {}",
                    last_processed_samples
                );

                // If this was final iteration, we're done
                if is_final_iteration {
                    break;
                }
            }

            // Send final text
            info!("[STREAMING] Sending final text ({} chars)", accumulated_text.len());
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
