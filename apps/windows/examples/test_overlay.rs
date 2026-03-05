//! Test overlay example - Tests the overlay_win32 module
//!
//! Run with: cargo run --example test_overlay

use dictator::overlay_win32::{OverlayConfig, OverlayWindow};
use std::thread;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("Starting overlay test...");

    let config = OverlayConfig::default();
    let overlay = OverlayWindow::new(config)?;

    println!("Overlay created. Showing test messages...");

    // Test 1: Show simple message
    overlay.show("Hello from Dictator!");
    thread::sleep(Duration::from_secs(2));

    // Test 2: Update body text
    overlay.update_body_text("Processing audio...");
    thread::sleep(Duration::from_secs(2));

    // Test 3: Position near cursor and show recording state
    overlay.position_near_cursor();
    overlay.set_recording(true);
    thread::sleep(Duration::from_secs(3));

    // Test 4: Stop recording and show final text
    overlay.set_recording(false);
    overlay.show("Final test message");
    thread::sleep(Duration::from_secs(2));

    // Test 5: Hide overlay
    overlay.hide();
    println!("Test complete. Overlay hidden.");

    thread::sleep(Duration::from_millis(500));

    Ok(())
}
