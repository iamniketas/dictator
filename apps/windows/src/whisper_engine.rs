//! Embedded Whisper inference engine via whisper-rs (whisper.cpp bindings).
//!
//! Eliminates the Python dependency by running whisper.cpp directly in-process.
//! Uses GGML .bin model files (download from https://huggingface.co/ggerganov/whisper.cpp).
//!
//! GPU (CUDA) support: build with `cargo build --features cuda`.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// A loaded Whisper model. Drop to free VRAM/RAM.
pub struct WhisperEngine {
    ctx: WhisperContext,
}

impl WhisperEngine {
    /// Load a GGML model file (e.g. `ggml-large-v3.bin`).
    pub fn load(model_path: &Path) -> Result<Self> {
        let path_str = model_path
            .to_str()
            .context("Model path contains non-UTF-8 characters")?;

        info!("[ENGINE] Loading model: {}", path_str);

        let params = {
            #[allow(unused_mut)]
            let mut p = WhisperContextParameters::default();
            // Enable GPU if the crate was compiled with the `cuda` feature
            #[cfg(feature = "cuda")]
            {
                p.use_gpu(true);
                info!("[ENGINE] CUDA GPU acceleration enabled");
            }
            p
        };

        let ctx = WhisperContext::new_with_params(path_str, params)
            .map_err(|e| anyhow::anyhow!("Failed to load whisper model '{}': {:?}", path_str, e))?;

        info!("[ENGINE] Model loaded successfully");
        Ok(Self { ctx })
    }

    /// Transcribe raw 16 kHz mono f32 PCM samples.
    pub fn transcribe(&self, audio: &[f32], language: &str) -> Result<String> {
        let duration_secs = audio.len() as f32 / 16000.0;
        info!(
            "[ENGINE] Transcribing {} samples ({:.1}s), language={}",
            audio.len(),
            duration_secs,
            language
        );

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("Failed to create whisper state: {:?}", e))?;

        let n_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8) as i32;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language));
        params.set_n_threads(n_threads);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, audio)
            .map_err(|e| anyhow::anyhow!("Transcription failed: {:?}", e))?;

        let n_segments = state
            .full_n_segments()
            .map_err(|e| anyhow::anyhow!("Failed to get segment count: {:?}", e))?;

        let mut text = String::new();
        for i in 0..n_segments {
            let seg = state
                .full_get_segment_text(i)
                .map_err(|e| anyhow::anyhow!("Failed to get segment {}: {:?}", i, e))?;
            let seg = seg.trim();
            if !seg.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(seg);
            }
        }

        info!("[ENGINE] Transcription complete: {} chars", text.len());
        Ok(text)
    }
}

// ── Shared engine ─────────────────────────────────────────────────────────────

/// Thread-safe shared handle to an optionally-loaded engine.
/// `None` = model not in memory (after idle unload or before first use).
pub type SharedEngine = Arc<Mutex<Option<WhisperEngine>>>;

/// Create a new shared engine (model not loaded yet).
pub fn new_shared_engine() -> SharedEngine {
    Arc::new(Mutex::new(None))
}

/// Transcribe audio, loading the model first if necessary.
///
/// The mutex is held for the entire transcription. Since only one transcription
/// runs at a time in the current architecture, this is acceptable.
pub fn transcribe_with_engine(
    engine: &SharedEngine,
    model_path: &Path,
    audio: &[f32],
    language: &str,
) -> Result<String> {
    let mut guard = engine
        .lock()
        .map_err(|_| anyhow::anyhow!("Whisper engine mutex was poisoned"))?;

    if guard.is_none() {
        *guard = Some(WhisperEngine::load(model_path)?);
    }

    guard.as_ref().unwrap().transcribe(audio, language)
}

/// Drop the loaded model to free VRAM/RAM. No-op if already unloaded.
pub fn unload_engine(engine: &SharedEngine) {
    match engine.lock() {
        Ok(mut guard) => {
            if guard.is_some() {
                *guard = None;
                info!("[ENGINE] Model unloaded (idle timeout or manual unload)");
            }
        }
        Err(_) => warn!("[ENGINE] Could not lock engine for unload (mutex poisoned)"),
    }
}

/// Returns true if the model is currently loaded in memory.
pub fn is_engine_loaded(engine: &SharedEngine) -> bool {
    engine.lock().map(|g| g.is_some()).unwrap_or(false)
}
