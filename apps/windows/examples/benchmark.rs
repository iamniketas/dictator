//! Benchmark: Streaming vs Full transcription
//!
//! Run with: cargo run --example benchmark
//!
//! This test measures the time from "hotkey press" to "text ready"
//! for both streaming and full transcription modes.

use dictator::audio::AudioRecorder;
use dictator::config::Config;
use std::thread;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("=== Dictator Benchmark: Streaming vs Full ===\n");

    // Load config
    let config = Config::load()?;
    println!("Config loaded. Language: {}\n", config.whisper.language);

    // Create recorder
    let recorder = AudioRecorder::new()?;

    // === TEST 1: Streaming mode ===
    println!("TEST 1: Streaming mode (3-sec chunks)");
    println!("----------------------------------------");
    println!("Speak for ~15-20 seconds, then press Enter to stop.");
    println!("This simulates hotkey press during recording.\n");

    println!("Starting recording in 3 seconds...");
    thread::sleep(Duration::from_secs(3));

    // Start recording
    recorder.start_recording()?;
    let streaming_start = Instant::now();

    println!("🎤 RECORDING... Speak now!");
    println!("Press Enter when done speaking...");

    // Wait for user input
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    // Measure time from "hotkey press" to "ready"
    let hotkey_time = Instant::now();

    // Stop streaming first (simulating main.rs logic)
    println!("Stopping streaming (simulating hotkey)...");
    thread::sleep(Duration::from_millis(100)); // Small delay for streaming to process

    // Now stop recording
    println!("Stopping recording...");
    let _audio_data = recorder.stop_recording()?;

    let streaming_total = hotkey_time.elapsed();

    println!("\n📊 STREAMING RESULTS:");
    println!(
        "  Total recording time: {:.1}s",
        streaming_start.elapsed().as_secs_f32()
    );
    println!(
        "  Time from hotkey to ready: {:.1}s",
        streaming_total.as_secs_f32()
    );
    println!(
        "  Audio samples: {} ({:.1}s at 16kHz)",
        _audio_data.len(),
        _audio_data.len() as f32 / 16000.0
    );

    // === TEST 2: Full transcription mode ===
    println!("\n\nTEST 2: Full transcription mode");
    println!("----------------------------------------");
    println!("Speak for ~15-20 seconds (same as before), then press Enter.");
    println!("This measures full audio transcription without streaming.\n");

    println!("Starting recording in 3 seconds...");
    thread::sleep(Duration::from_secs(3));

    // Start recording
    recorder.start_recording()?;
    let full_start = Instant::now();

    println!("🎤 RECORDING... Speak now!");
    println!("Press Enter when done speaking...");

    let mut input2 = String::new();
    std::io::stdin().read_line(&mut input2)?;

    // Measure time from "hotkey" to "ready"
    let hotkey_time2 = Instant::now();

    println!("Stopping recording...");
    let audio_data_full = recorder.stop_recording()?;

    println!("Transcribing full audio...");
    match dictator::transcribe::transcribe_audio(&audio_data_full, &config.whisper.language) {
        Ok(text) => {
            let full_total = hotkey_time2.elapsed();

            println!("\n📊 FULL TRANSCRIPTION RESULTS:");
            println!(
                "  Total recording time: {:.1}s",
                full_start.elapsed().as_secs_f32()
            );
            println!(
                "  Time from hotkey to ready: {:.1}s",
                full_total.as_secs_f32()
            );
            println!(
                "  Audio samples: {} ({:.1}s at 16kHz)",
                audio_data_full.len(),
                audio_data_full.len() as f32 / 16000.0
            );
            println!("  Transcribed text length: {} chars", text.len());

            // Summary
            println!("\n\n=== SUMMARY ===");
            println!(
                "Streaming time:       {:.1}s",
                streaming_total.as_secs_f32()
            );
            println!("Full transcribe time: {:.1}s", full_total.as_secs_f32());

            if streaming_total < full_total {
                let speedup = full_total.as_secs_f32() / streaming_total.as_secs_f32();
                println!("Streaming is {:.1}x FASTER", speedup);
            } else {
                let slowdown = streaming_total.as_secs_f32() / full_total.as_secs_f32();
                println!("Streaming is {:.1}x SLOWER", slowdown);
            }
        }
        Err(e) => {
            eprintln!("Transcription error: {}", e);
        }
    }

    println!("\nPress Enter to exit...");
    let mut _exit = String::new();
    std::io::stdin().read_line(&mut _exit)?;

    Ok(())
}
