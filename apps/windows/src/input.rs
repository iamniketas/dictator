//! Input module - Global hotkeys and text injection

use std::sync::mpsc;
use std::thread;
use tracing::{error, info};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetMessageW, SetForegroundWindow, ShowWindow, MSG, SW_RESTORE, WM_HOTKEY,
};

const HOTKEY_ID: i32 = 1;

/// Events from hotkey listener
/// Uses isize to store HWND value (HWND is not Send-safe due to *mut c_void)
#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    RecordStart { hwnd: isize },
    RecordStop { hwnd: isize },
}

/// Get handle of currently focused window as isize
pub fn get_foreground_window_handle() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// Convert isize back to HWND
fn hwnd_from_isize(handle: isize) -> HWND {
    HWND(handle as *mut _)
}

/// Set foreground window (restore focus)
pub fn set_foreground_window(hwnd_value: isize) -> anyhow::Result<()> {
    unsafe {
        let hwnd = hwnd_from_isize(hwnd_value);

        if hwnd.0.is_null() {
            return Err(anyhow::anyhow!("Invalid window handle"));
        }

        // Try to restore focus to the window
        let result = SetForegroundWindow(hwnd);
        if result.0 == 0 {
            // Window might be minimized, try to restore it
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }

        info!("Focus restored to window: {:?}", hwnd_value);
        Ok(())
    }
}

/// Start hotkey listener in a separate thread
pub fn start_hotkey_listener(tx: mpsc::Sender<HotkeyEvent>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        unsafe {
            // Register Ctrl+Shift+D
            let modifiers = MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT;
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
                    tracing::debug!(
                        "[HOTKEY] WM_HOTKEY received! Current toggle_state: {}",
                        toggle_state
                    );

                    // Calculate next state BEFORE toggling
                    let next_state = !toggle_state;

                    // Save the currently focused window handle
                    let hwnd = GetForegroundWindow().0 as isize;

                    // Send event based on NEXT state
                    let send_result = if next_state {
                        info!("[HOTKEY] RecordStart");
                        tx.send(HotkeyEvent::RecordStart { hwnd })
                    } else {
                        info!("[HOTKEY] RecordStop");
                        tx.send(HotkeyEvent::RecordStop { hwnd })
                    };

                    // Only toggle if send succeeded
                    match send_result {
                        Ok(_) => {
                            toggle_state = next_state;
                        }
                        Err(e) => {
                            error!("[HOTKEY] Failed to send event: {}", e);
                        }
                    }
                }
            }

            // Cleanup
            let _ = UnregisterHotKey(None, HOTKEY_ID);
            info!("Hotkey listener stopped");
        }
    })
}

/// Maximum input events per SendInput call (Windows typically handles 64-128)
const MAX_INPUT_CHUNK_SIZE: usize = 60; // Conservative limit (30 chars * 2 events each)

/// Inject text into the currently focused application
/// For small texts (< 500 chars): uses direct Unicode input
/// For large texts: uses clipboard paste (Ctrl+V)
pub fn inject_text(text: &str) -> anyhow::Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    info!("Injecting text: {} chars", text.len());

    // For large texts, use clipboard paste which is much faster and more reliable
    if text.len() > 500 {
        inject_text_via_clipboard(text)?;
    } else {
        inject_text_via_unicode_input(text)?;
    }

    info!("Text injected successfully");
    Ok(())
}

/// Inject text using clipboard (Ctrl+V) - fast for large texts
fn inject_text_via_clipboard(text: &str) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, KEYBDINPUT, KEYEVENTF_KEYUP,
    };

    // Save current clipboard content
    let previous_clipboard = clipboard_win::get_clipboard_string().ok();

    // Copy our text to clipboard
    clipboard_win::set_clipboard_string(text)
        .map_err(|e| anyhow::anyhow!("Failed to set clipboard: {:?}", e))?;

    // Small delay to ensure clipboard is updated
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Create Ctrl+V input sequence
    let ctrl_v_inputs = [
        // Ctrl down
        INPUT {
            r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11), // VK_CONTROL
                    wScan: 0,
                    dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        // V down
        INPUT {
            r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x56), // 'V'
                    wScan: 0,
                    dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        // V up
        INPUT {
            r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x56), // 'V'
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        // Ctrl up
        INPUT {
            r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11), // VK_CONTROL
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];

    unsafe {
        let sent = SendInput(&ctrl_v_inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != 4 {
            return Err(anyhow::anyhow!("SendInput failed for Ctrl+V: sent {} of 4 events", sent));
        }
    }

    // Wait for paste to complete
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Restore previous clipboard content (if any)
    if let Some(prev) = previous_clipboard {
        let _ = clipboard_win::set_clipboard_string(&prev);
    }

    Ok(())
}

/// Inject text using Unicode input - reliable for small texts
fn inject_text_via_unicode_input(text: &str) -> anyhow::Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    };

    let utf16_chars: Vec<u16> = text.encode_utf16().collect();
    let total_chars = utf16_chars.len();
    let mut chars_sent = 0;

    // Process in chunks to avoid SendInput limits
    for chunk in utf16_chars.chunks(MAX_INPUT_CHUNK_SIZE / 2) {
        let mut inputs: Vec<INPUT> = Vec::with_capacity(chunk.len() * 2);

        // Create input events for this chunk
        for &c in chunk {
            // Key down
            inputs.push(INPUT {
                r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
                        wScan: c,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });

            // Key up
            inputs.push(INPUT {
                r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
                        wScan: c,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }

        // Send this chunk
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

        chars_sent += chunk.len();

        // Small delay between chunks to avoid overwhelming the target app
        if chars_sent < total_chars {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    Ok(())
}
