//! Input module - Global hotkeys and text injection

use std::sync::mpsc;
use std::thread;
use tracing::{error, info};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

const HOTKEY_ID: i32 = 1;

/// Events from hotkey listener
#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    RecordStart,
    RecordStop,
}

/// Start hotkey listener in a separate thread
pub fn start_hotkey_listener(tx: mpsc::Sender<HotkeyEvent>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        unsafe {
            // Register Ctrl+Shift+D
            let modifiers = MOD_CONTROL | MOD_SHIFT;
            let vk_d = 0x44u32; // Virtual key code for 'D'

            if RegisterHotKey(None, HOTKEY_ID, modifiers, vk_d).is_err() {
                error!("Failed to register hotkey Ctrl+Shift+D");
                return;
            }

            info!("Hotkey Ctrl+Shift+D registered successfully");

            // Message loop for hotkey events
            let mut msg = MSG::default();
            let mut toggle_state = false; // Track recording state

            while GetMessageW(&mut msg, None, 0, 0).into() {
                if msg.message == WM_HOTKEY && msg.wParam.0 as i32 == HOTKEY_ID {
                    toggle_state = !toggle_state;

                    if toggle_state {
                        info!("Hotkey pressed! Starting recording...");
                        let _ = tx.send(HotkeyEvent::RecordStart);
                    } else {
                        info!("Hotkey pressed! Stopping recording...");
                        let _ = tx.send(HotkeyEvent::RecordStop);
                    }
                }
            }

            // Cleanup
            let _ = UnregisterHotKey(None, HOTKEY_ID);
            info!("Hotkey listener stopped");
        }
    })
}

/// Inject text into the currently focused application
pub fn inject_text(text: &str) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_UNICODE, KEYEVENTF_KEYUP,
    };

    if text.is_empty() {
        return Ok(());
    }

    info!("Injecting text: {} chars", text.len());

    let mut inputs: Vec<INPUT> = Vec::new();

    // Convert text to UTF-16 and create key events
    for c in text.encode_utf16() {
        // Key down
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: Default::default(),
                    wScan: c,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });

        // Key up
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: Default::default(),
                    wScan: c,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    // Send all inputs
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            return Err(anyhow::anyhow!(
                "SendInput failed: sent {} of {} events",
                sent,
                inputs.len()
            ));
        }
    }

    info!("Text injected successfully");
    Ok(())
}