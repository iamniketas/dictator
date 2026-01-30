//! Input module - Global hotkeys and text injection

use std::sync::mpsc;
use std::thread;
use tracing::{error, info};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_SHIFT,
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

                    // Save the currently focused window handle
                    let hwnd = GetForegroundWindow().0 as isize;

                    if toggle_state {
                        info!("Hotkey pressed! Starting recording...");
                        let _ = tx.send(HotkeyEvent::RecordStart { hwnd });
                    } else {
                        info!("Hotkey pressed! Stopping recording...");
                        let _ = tx.send(HotkeyEvent::RecordStop { hwnd });
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
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
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
