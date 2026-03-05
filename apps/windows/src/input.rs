//! Input module - Global hotkeys and text injection

use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, SetForegroundWindow,
    SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT,
    LLKHF_UP, MSG, SW_RESTORE, WH_KEYBOARD_LL,
};

/// Hold duration threshold: above = Push-to-Talk, below = Toggle
const PTT_THRESHOLD_MS: u64 = 300;

/// VK_RMENU — Right Alt key (0xA5)
const TARGET_VK: u32 = 0xA5;

struct HotkeyListenerState {
    /// true = toggle recording is active, next key down will stop
    is_toggle_recording: bool,
    /// set on key down, cleared on key up
    press_started: Option<Instant>,
    sender: Option<mpsc::Sender<HotkeyEvent>>,
}

static HOTKEY_LISTENER: Mutex<HotkeyListenerState> = Mutex::new(HotkeyListenerState {
    is_toggle_recording: false,
    press_started: None,
    sender: None,
});

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

/// Start hotkey listener in a separate thread.
/// Right Alt behavior:
///   - Hold >300ms → Push-to-Talk: recording stops on key release
///   - Tap <300ms  → Toggle: first tap starts, second tap stops
pub fn start_hotkey_listener(tx: mpsc::Sender<HotkeyEvent>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        {
            let mut state = HOTKEY_LISTENER.lock().unwrap();
            state.sender = Some(tx);
        }

        unsafe {
            let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard_hook), None, 0) {
                Ok(h) => h,
                Err(e) => {
                    error!("[HOTKEY] Failed to install LL keyboard hook: {}", e);
                    return;
                }
            };

            info!("[HOTKEY] LL keyboard hook installed (Right Alt, PTT/Toggle)");

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }

            let _ = UnhookWindowsHookEx(hook);
            info!("[HOTKEY] Keyboard hook removed");
        }
    })
}

unsafe extern "system" fn ll_keyboard_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if process_hotkey_event(kb.vkCode, (kb.flags.0 & LLKHF_UP.0) != 0) {
            // Key was our hotkey — suppress it from reaching the target app
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Returns true if the event was consumed (hotkey matched).
fn process_hotkey_event(vk: u32, is_key_up: bool) -> bool {
    if vk != TARGET_VK {
        return false;
    }

    let mut state = match HOTKEY_LISTENER.try_lock() {
        Ok(s) => s,
        Err(_) => return false,
    };

    if !is_key_up {
        // ── Key DOWN ──────────────────────────────────────────────────────────
        if state.is_toggle_recording {
            // Toggle mode: second press stops recording
            state.is_toggle_recording = false;
            state.press_started = None;
            let hwnd = unsafe { GetForegroundWindow().0 as isize };
            if let Some(ref tx) = state.sender {
                let _ = tx.send(HotkeyEvent::RecordStop { hwnd });
                info!("[HOTKEY] RecordStop (toggle off)");
            }
        } else if state.press_started.is_none() {
            // Fresh press: start recording, begin timing
            state.press_started = Some(Instant::now());
            let hwnd = unsafe { GetForegroundWindow().0 as isize };
            if let Some(ref tx) = state.sender {
                let _ = tx.send(HotkeyEvent::RecordStart { hwnd });
                info!("[HOTKEY] RecordStart");
            }
        }
        // Else: key repeat while held — ignore
    } else {
        // ── Key UP ────────────────────────────────────────────────────────────
        if let Some(press_time) = state.press_started.take() {
            let elapsed = press_time.elapsed();
            if elapsed > Duration::from_millis(PTT_THRESHOLD_MS) {
                // Push-to-Talk: release stops recording
                state.is_toggle_recording = false;
                let hwnd = unsafe { GetForegroundWindow().0 as isize };
                if let Some(ref tx) = state.sender {
                    let _ = tx.send(HotkeyEvent::RecordStop { hwnd });
                    info!("[HOTKEY] RecordStop (PTT, held {}ms)", elapsed.as_millis());
                }
            } else {
                // Short tap: switch to toggle mode, keep recording
                state.is_toggle_recording = true;
                info!("[HOTKEY] Toggle mode active (tapped {}ms)", elapsed.as_millis());
            }
        }
    }

    true
}

/// Maximum input events per SendInput call (Windows typically handles 64-128)
const MAX_INPUT_CHUNK_SIZE: usize = 60; // Conservative limit (30 chars * 2 events each)

/// Inject text into the currently focused application
pub fn inject_text(text: &str, method: &crate::config::InjectionMethod) -> anyhow::Result<()> {
    use crate::config::InjectionMethod;

    if text.is_empty() {
        return Ok(());
    }

    info!("Injecting text: {} chars, method: {:?}", text.len(), method);

    match method {
        InjectionMethod::Direct => {
            // Auto-select: clipboard for large texts, Unicode input for small
            if text.len() > 500 {
                inject_text_via_clipboard(text, false)?;
            } else {
                inject_text_via_unicode_input(text)?;
            }
        }
        InjectionMethod::Clipboard => {
            inject_text_via_clipboard(text, false)?;
        }
        InjectionMethod::ClipboardEnter => {
            inject_text_via_clipboard(text, true)?;
        }
    }

    info!("Text injected successfully");
    Ok(())
}

/// Inject text using clipboard (Ctrl+V) - fast for large texts
/// If `send_enter` is true, sends Enter key after paste
fn inject_text_via_clipboard(text: &str, send_enter: bool) -> anyhow::Result<()> {
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

    // Send Enter if requested
    if send_enter {
        let enter_inputs = [
            INPUT {
                r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x0D), // VK_RETURN
                        wScan: 0,
                        dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x0D), // VK_RETURN
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];
        unsafe {
            SendInput(&enter_inputs, std::mem::size_of::<INPUT>() as i32);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

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
