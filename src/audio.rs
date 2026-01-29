//! Audio module - Microphone capture using cpal

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use tracing::{error, info};

/// Commands for audio thread
enum AudioCommand {
    Start,
    Stop,
    Shutdown,
    GetBuffer,
}

/// Audio recorder that captures from microphone
pub struct AudioRecorder {
    cmd_tx: Sender<AudioCommand>,
    data_rx: Mutex<Receiver<Vec<f32>>>,
    buffer_rx: Mutex<Receiver<(Vec<f32>, usize)>>,
    is_recording: Arc<AtomicBool>,
    _thread_handle: JoinHandle<()>,
    sample_rate: u32,
}

impl AudioRecorder {
    /// Create new audio recorder with dedicated audio thread
    pub fn new() -> Result<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (data_tx, data_rx) = mpsc::channel();
        let (buffer_tx, buffer_rx) = mpsc::channel();
        let is_recording = Arc::new(AtomicBool::new(false));
        let is_recording_clone = is_recording.clone();

        let thread_handle = thread::spawn(move || {
            audio_thread(cmd_rx, data_tx, buffer_tx, is_recording_clone);
        });

        Ok(Self {
            cmd_tx,
            data_rx: Mutex::new(data_rx),
            buffer_rx: Mutex::new(buffer_rx),
            is_recording,
            _thread_handle: thread_handle,
            sample_rate: 16000,
        })
    }

    /// Start recording audio from default input device
    pub fn start_recording(&self) -> Result<()> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.cmd_tx.send(AudioCommand::Start)?;
        Ok(())
    }

    /// Stop recording and get audio data
    pub fn stop_recording(&self) -> Result<Vec<f32>> {
        if !self.is_recording.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        self.cmd_tx.send(AudioCommand::Stop)?;

        // Wait for audio data
        let data = self.data_rx.lock().unwrap().recv().unwrap_or_default();
        Ok(data)
    }

    /// Get unprocessed buffer without stopping recording
    /// Returns (audio_data, start_index) where start_index is the index of first new sample
    pub fn get_unprocessed_buffer(&self) -> Result<(Vec<f32>, usize)> {
        if !self.is_recording.load(Ordering::SeqCst) {
            return Ok((Vec::new(), 0));
        }
        self.cmd_tx.send(AudioCommand::GetBuffer)?;
        
        // Wait for buffer data
        let (data, start_idx) = self.buffer_rx.lock().unwrap().recv().unwrap_or_default();
        Ok((data, start_idx))
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// Get sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(AudioCommand::Shutdown);
    }
}

/// Dedicated audio thread that owns the cpal Stream
fn audio_thread(
    cmd_rx: Receiver<AudioCommand>,
    data_tx: Sender<Vec<f32>>,
    buffer_tx: Sender<(Vec<f32>, usize)>,
    is_recording: Arc<AtomicBool>,
) {
    let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
    let device_config = Arc::new(Mutex::new((48000u32, 1u16))); // (sample_rate, channels)
    let last_processed_index = Arc::new(Mutex::new(0usize));

    loop {
        match cmd_rx.recv() {
            Ok(AudioCommand::Start) => {
                buffer.lock().unwrap().clear();
                *last_processed_index.lock().unwrap() = 0;

                // Get default input device
                let host = cpal::default_host();
                let device = match host.default_input_device() {
                    Some(d) => d,
                    None => {
                        error!("No input device available");
                        continue;
                    }
                };

                let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
                info!("Using input device: {}", device_name);

                // Get supported config
                let config = match device.default_input_config() {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to get input config: {}", e);
                        continue;
                    }
                };

                info!("Input config: {:?}", config);
                let sample_format = config.sample_format();
                let stream_config: cpal::StreamConfig = config.into();

                // Store device config for resampling
                *device_config.lock().unwrap() = (stream_config.sample_rate.0, stream_config.channels);

                // Clone for callback
                let buffer_clone = buffer.clone();
                let is_rec = is_recording.clone();

                // Build stream
                let stream = match sample_format {
                    SampleFormat::F32 => device.build_input_stream(
                        &stream_config,
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            if is_rec.load(Ordering::SeqCst) {
                                buffer_clone.lock().unwrap().extend_from_slice(data);
                            }
                        },
                        |err| error!("Audio stream error: {}", err),
                        None,
                    ),
                    SampleFormat::I16 => device.build_input_stream(
                        &stream_config,
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            if is_rec.load(Ordering::SeqCst) {
                                let samples: Vec<f32> =
                                    data.iter().map(|&s| s as f32 / 32768.0).collect();
                                buffer_clone.lock().unwrap().extend(samples);
                            }
                        },
                        |err| error!("Audio stream error: {}", err),
                        None,
                    ),
                    SampleFormat::U16 => device.build_input_stream(
                        &stream_config,
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            if is_rec.load(Ordering::SeqCst) {
                                let samples: Vec<f32> = data
                                    .iter()
                                    .map(|&s| (s as f32 - 32768.0) / 32768.0)
                                    .collect();
                                buffer_clone.lock().unwrap().extend(samples);
                            }
                        },
                        |err| error!("Audio stream error: {}", err),
                        None,
                    ),
                    _ => {
                        error!("Unsupported sample format: {:?}", sample_format);
                        continue;
                    }
                };

                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        error!("Failed to build stream: {}", e);
                        continue;
                    }
                };

                if let Err(e) = stream.play() {
                    error!("Failed to start stream: {}", e);
                    continue;
                }

                is_recording.store(true, Ordering::SeqCst);
                info!("🎤 Recording started...");

                // Wait for stop command while stream is active
                loop {
                    match cmd_rx.recv() {
                        Ok(AudioCommand::Stop) => {
                            is_recording.store(false, Ordering::SeqCst);
                            drop(stream);

                            let raw_data = buffer.lock().unwrap().clone();
                            let (sample_rate, channels) = *device_config.lock().unwrap();

                            // Convert to 16 kHz mono
                            let mono_16k = convert_to_16khz_mono(&raw_data, sample_rate, channels);

                            let duration = mono_16k.len() as f32 / 16000.0;
                            info!(
                                "⏹️  Recording stopped. Captured {} samples ({:.1} seconds at 16kHz mono)",
                                mono_16k.len(),
                                duration
                            );

                            let _ = data_tx.send(mono_16k);
                            break;
                        }
                        Ok(AudioCommand::GetBuffer) => {
                            let raw_data = buffer.lock().unwrap().clone();
                            let last_idx = *last_processed_index.lock().unwrap();
                            let (sample_rate, channels) = *device_config.lock().unwrap();

                            // Convert to 16 kHz mono
                            let mono_16k = convert_to_16khz_mono(&raw_data, sample_rate, channels);

                            // Calculate corresponding index in converted data
                            let converted_last_idx = if sample_rate == 16000 && channels == 1 {
                                last_idx
                            } else {
                                // Approximate index after conversion
                                let ratio = sample_rate as f32 / 16000.0;
                                let mono_len = raw_data.len() / channels as usize;
                                let converted_len = (mono_len as f32 / ratio) as usize;
                                if raw_data.len() > 0 {
                                    (last_idx * converted_len) / raw_data.len()
                                } else {
                                    0
                                }
                            };

                            // Update last processed index to current length
                            *last_processed_index.lock().unwrap() = raw_data.len();

                            let _ = buffer_tx.send((mono_16k, converted_last_idx));
                        }
                        Ok(AudioCommand::Shutdown) => {
                            is_recording.store(false, Ordering::SeqCst);
                            return;
                        }
                        Ok(AudioCommand::Start) => {
                            // Already recording, ignore
                        }
                        Err(_) => return,
                    }
                }
            }
            Ok(AudioCommand::Stop) => {
                // Not recording, send empty data
                let _ = data_tx.send(Vec::new());
            }
            Ok(AudioCommand::GetBuffer) => {
                // Not recording, send empty buffer
                let _ = buffer_tx.send((Vec::new(), 0));
            }
            Ok(AudioCommand::Shutdown) | Err(_) => {
                return;
            }
        }
    }
}

/// Convert audio to 16 kHz mono
/// - Stereo → mono: average channels
/// - Resample to 16 kHz using linear interpolation
fn convert_to_16khz_mono(data: &[f32], sample_rate: u32, channels: u16) -> Vec<f32> {
    // Step 1: Convert stereo to mono
    let mono: Vec<f32> = if channels == 1 {
        data.to_vec()
    } else {
        data.chunks(channels as usize)
            .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    // Step 2: Resample to 16 kHz
    if sample_rate == 16000 {
        return mono;
    }

    let ratio = sample_rate as f32 / 16000.0;
    let output_len = (mono.len() as f32 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f32 * ratio;
        let src_index = src_pos as usize;

        if src_index + 1 < mono.len() {
            // Linear interpolation
            let frac = src_pos - src_index as f32;
            let sample = mono[src_index] * (1.0 - frac) + mono[src_index + 1] * frac;
            output.push(sample);
        } else if src_index < mono.len() {
            output.push(mono[src_index]);
        }
    }

    output
}

