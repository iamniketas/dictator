//! UI module - System tray and overlay windows

use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GetCursorPos, GetMessageW, LoadIconW, LoadImageW, MessageBoxW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, TrackPopupMenu, CS_HREDRAW, CS_VREDRAW, IDI_APPLICATION, IMAGE_ICON,
    LR_LOADFROMFILE, MB_ICONINFORMATION, MB_OK, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING,
    MF_UNCHECKED, MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_COMMAND, WM_DESTROY, WM_RBUTTONUP,
    WM_USER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const WM_TRAYICON: u32 = WM_USER + 1;
const ID_EXIT: u16 = 1001;
const ID_STREAMING: u16 = 1002;
const ID_CHUNK_3: u16 = 1003;
const ID_CHUNK_8: u16 = 1004;
const ID_CHUNK_15: u16 = 1005;
const ID_OPEN_HISTORY: u16 = 1006;
const ID_OLLAMA: u16 = 1007;
const ID_OPEN_CONFIG: u16 = 1008;
const ID_ABOUT: u16 = 1009;
const ID_HISTORY_START: u16 = 1100; // Start of dynamic history IDs (1100-1199)
const ID_HISTORY_END: u16 = 1199;
const ID_MODEL_START: u16 = 1200; // Start of dynamic model IDs (1200-1299)
const ID_MODEL_END: u16 = 1299;
const ID_DOWNLOAD_START: u16 = 1300; // Start of download model IDs (1300-1399)
const ID_DOWNLOAD_END: u16 = 1399;
const ID_SETTINGS: u16 = 1010;

static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
static STREAMING_ENABLED: AtomicBool = AtomicBool::new(false);
static STREAMING_CHUNK_SECONDS: AtomicU64 = AtomicU64::new(15);
static OLLAMA_ENABLED: AtomicBool = AtomicBool::new(false);
static IS_DOWNLOADING: AtomicBool = AtomicBool::new(false);

// History callbacks
static HISTORY_OPEN_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() + Send + 'static>>> = std::sync::Mutex::new(None);
static HISTORY_COPY_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn(usize) + Send + 'static>>> = std::sync::Mutex::new(None);
static HISTORY_GET_ENTRIES_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() -> Vec<HistoryMenuEntry> + Send + 'static>>> = std::sync::Mutex::new(None);

// Model selector callbacks
static MODEL_SELECT_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn(usize) + Send + 'static>>> = std::sync::Mutex::new(None);
static MODEL_GET_LIST_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() -> Vec<ModelMenuItem> + Send + 'static>>> = std::sync::Mutex::new(None);

// Config open callback
static OPEN_CONFIG_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() + Send + 'static>>> = std::sync::Mutex::new(None);

// Download model callbacks
static DOWNLOAD_MODEL_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn(usize) + Send + 'static>>> = std::sync::Mutex::new(None);
static DOWNLOAD_LIST_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() -> Vec<DownloadModelItem> + Send + 'static>>> = std::sync::Mutex::new(None);

// Settings window callback
static SETTINGS_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() + Send + 'static>>> = std::sync::Mutex::new(None);

/// Entry for history menu
#[derive(Debug, Clone)]
pub struct HistoryMenuEntry {
    pub id: usize,  // 0-based index
    pub label: String,
}

/// Entry for download model submenu
#[derive(Debug, Clone)]
pub struct DownloadModelItem {
    pub index: usize,
    pub name: String,
    pub size_mb: u32,
    /// Whether the file already exists in the models directory
    pub already_downloaded: bool,
}

/// Entry for model selector menu
#[derive(Debug, Clone)]
pub struct ModelMenuItem {
    pub index: usize,
    pub name: String,
    pub is_current: bool,
    /// Human-readable size string, e.g. " (3.1 GB)". Empty if unknown.
    pub size_label: String,
}

/// Set the callback for opening history folder
pub fn set_history_open_callback<F>(callback: F)
where
    F: Fn() + Send + 'static,
{
    if let Ok(mut cb) = HISTORY_OPEN_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Set the callback for copying history entry
pub fn set_history_copy_callback<F>(callback: F)
where
    F: Fn(usize) + Send + 'static,
{
    if let Ok(mut cb) = HISTORY_COPY_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Set the callback for getting history entries
pub fn set_history_entries_callback<F>(callback: F)
where
    F: Fn() -> Vec<HistoryMenuEntry> + Send + 'static,
{
    if let Ok(mut cb) = HISTORY_GET_ENTRIES_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Set the callback invoked when user selects a model (receives 0-based index)
pub fn set_model_select_callback<F>(callback: F)
where
    F: Fn(usize) + Send + 'static,
{
    if let Ok(mut cb) = MODEL_SELECT_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Set the callback that returns the list of available models
pub fn set_model_list_callback<F>(callback: F)
where
    F: Fn() -> Vec<ModelMenuItem> + Send + 'static,
{
    if let Ok(mut cb) = MODEL_GET_LIST_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Check if Ollama LLM correction is enabled (runtime toggle)
pub fn is_ollama_enabled() -> bool {
    OLLAMA_ENABLED.load(Ordering::SeqCst)
}

/// Set Ollama enabled state
pub fn set_ollama_enabled(enabled: bool) {
    OLLAMA_ENABLED.store(enabled, Ordering::SeqCst);
}

/// Set the callback for opening config file
pub fn set_open_config_callback<F>(callback: F)
where
    F: Fn() + Send + 'static,
{
    if let Ok(mut cb) = OPEN_CONFIG_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Set the callback invoked when user selects a model to download (receives 0-based KNOWN_MODELS index)
pub fn set_download_model_callback<F>(callback: F)
where
    F: Fn(usize) + Send + 'static,
{
    if let Ok(mut cb) = DOWNLOAD_MODEL_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Set the callback that returns the list of downloadable models (with availability flags)
pub fn set_download_list_callback<F>(callback: F)
where
    F: Fn() -> Vec<DownloadModelItem> + Send + 'static,
{
    if let Ok(mut cb) = DOWNLOAD_LIST_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Set the callback invoked when user clicks "Settings..." in the tray
pub fn set_settings_callback<F>(callback: F)
where
    F: Fn() + Send + 'static,
{
    if let Ok(mut cb) = SETTINGS_CALLBACK.lock() {
        *cb = Some(Box::new(callback));
    }
}

/// Returns true while a model download is in progress (blocks tray interaction)
pub fn is_downloading() -> bool {
    IS_DOWNLOADING.load(Ordering::SeqCst)
}

/// Set the downloading state (called by main thread during download)
pub fn set_is_downloading(value: bool) {
    IS_DOWNLOADING.store(value, Ordering::SeqCst);
}

/// Check if streaming is enabled
pub fn is_streaming_enabled() -> bool {
    STREAMING_ENABLED.load(Ordering::SeqCst)
}

/// Set streaming enabled state
pub fn set_streaming_enabled(enabled: bool) {
    STREAMING_ENABLED.store(enabled, Ordering::SeqCst);
}

/// Get selected streaming chunk length in seconds
pub fn streaming_chunk_seconds() -> u64 {
    STREAMING_CHUNK_SECONDS.load(Ordering::SeqCst)
}

/// Set streaming chunk length in seconds (allowed values: 3, 8, 15)
pub fn set_streaming_chunk_seconds(seconds: u64) {
    let normalized = match seconds {
        3 | 8 | 15 => seconds,
        _ => 15,
    };
    STREAMING_CHUNK_SECONDS.store(normalized, Ordering::SeqCst);
}

/// Check if application should exit
pub fn should_exit() -> bool {
    SHOULD_EXIT.load(Ordering::SeqCst)
}

/// Run the system tray (blocking)
pub fn run_tray() -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("DictatorTrayClass");

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            ..Default::default()
        };

        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("Dictator"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        )?;

        let tray_icon = load_tray_icon()?;
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            hIcon: tray_icon,
            szTip: {
                let mut tip = [0u16; 128];
                let text = "Dictator - Voice Dictation";
                for (i, c) in text.encode_utf16().enumerate() {
                    if i >= 127 {
                        break;
                    }
                    tip[i] = c;
                }
                tip
            },
            ..Default::default()
        };

        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            return Err(anyhow::anyhow!("Failed to add tray icon"));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = DispatchMessageW(&msg);
        }

        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        Ok(())
    }
}

fn load_tray_icon() -> Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("assets").join("dictator.ico"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        candidates.push(exe_dir.join("assets").join("dictator.ico"));
        if let Some(parent) = exe_dir.parent() {
            candidates.push(parent.join("assets").join("dictator.ico"));
            if let Some(grand) = parent.parent() {
                candidates.push(grand.join("assets").join("dictator.ico"));
            }
        }
    }

    for path in candidates {
        if !path.exists() {
            continue;
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            if let Ok(icon) = LoadImageW(
                None,
                windows::core::PCWSTR(wide.as_ptr()),
                IMAGE_ICON,
                0,
                0,
                LR_LOADFROMFILE,
            ) {
                return Ok(windows::Win32::UI::WindowsAndMessaging::HICON(icon.0));
            }
        }
    }

    Ok(unsafe { LoadIconW(None, IDI_APPLICATION)? })
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAYICON => {
                if lparam.0 as u32 == WM_RBUTTONUP {
                    show_context_menu(hwnd);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let cmd = (wparam.0 & 0xFFFF) as u16;
                if cmd == ID_EXIT {
                    SHOULD_EXIT.store(true, Ordering::SeqCst);
                    PostQuitMessage(0);
                } else if cmd == ID_STREAMING {
                    let new_state = !STREAMING_ENABLED.load(Ordering::SeqCst);
                    STREAMING_ENABLED.store(new_state, Ordering::SeqCst);
                    eprintln!("[TRAY] Streaming {}", if new_state { "enabled" } else { "disabled" });
                } else if cmd == ID_CHUNK_3 {
                    set_streaming_chunk_seconds(3);
                    eprintln!("[TRAY] Streaming chunk set to 3s");
                } else if cmd == ID_CHUNK_8 {
                    set_streaming_chunk_seconds(8);
                    eprintln!("[TRAY] Streaming chunk set to 8s");
                } else if cmd == ID_CHUNK_15 {
                    set_streaming_chunk_seconds(15);
                    eprintln!("[TRAY] Streaming chunk set to 15s");
                } else if cmd == ID_OPEN_HISTORY {
                    eprintln!("[TRAY] Opening history folder");
                    if let Ok(cb) = HISTORY_OPEN_CALLBACK.lock() {
                        if let Some(ref callback) = *cb {
                            callback();
                        }
                    }
                } else if cmd >= ID_HISTORY_START && cmd <= ID_HISTORY_END {
                    let index = (cmd - ID_HISTORY_START) as usize;
                    eprintln!("[TRAY] Copying history entry {}", index);
                    if let Ok(cb) = HISTORY_COPY_CALLBACK.lock() {
                        if let Some(ref callback) = *cb {
                            callback(index);
                        }
                    }
                } else if cmd >= ID_MODEL_START && cmd <= ID_MODEL_END {
                    let index = (cmd - ID_MODEL_START) as usize;
                    eprintln!("[TRAY] Selecting model {}", index);
                    if let Ok(cb) = MODEL_SELECT_CALLBACK.lock() {
                        if let Some(ref callback) = *cb {
                            callback(index);
                        }
                    }
                } else if cmd == ID_OLLAMA {
                    let new_state = !OLLAMA_ENABLED.load(Ordering::SeqCst);
                    OLLAMA_ENABLED.store(new_state, Ordering::SeqCst);
                    eprintln!("[TRAY] Ollama LLM correction {}", if new_state { "enabled" } else { "disabled" });
                } else if cmd == ID_OPEN_CONFIG {
                    eprintln!("[TRAY] Opening config file");
                    if let Ok(cb) = OPEN_CONFIG_CALLBACK.lock() {
                        if let Some(ref callback) = *cb {
                            callback();
                        }
                    }
                } else if cmd == ID_SETTINGS {
                    eprintln!("[TRAY] Opening settings");
                    if let Ok(cb) = SETTINGS_CALLBACK.lock() {
                        if let Some(ref callback) = *cb {
                            callback();
                        }
                    }
                } else if cmd >= ID_DOWNLOAD_START && cmd <= ID_DOWNLOAD_END {
                    let index = (cmd - ID_DOWNLOAD_START) as usize;
                    if !IS_DOWNLOADING.load(Ordering::SeqCst) {
                        eprintln!("[TRAY] Download model index {}", index);
                        if let Ok(cb) = DOWNLOAD_MODEL_CALLBACK.lock() {
                            if let Some(ref callback) = *cb {
                                callback(index);
                            }
                        }
                    }
                } else if cmd == ID_ABOUT {
                    let _ = MessageBoxW(
                        hwnd,
                        w!("Dictator v0.1.0-alpha\r\nVoice dictation for Windows\r\n\r\nHotkey: Right Ctrl\r\n  Hold >300ms \u{2192} Push-to-Talk\r\n  Tap          \u{2192} Toggle mode\r\n\r\nRuns 100% locally. No cloud, no telemetry.\r\n\r\nhttps://github.com/iamniketas/dictator"),
                        w!("About Dictator"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };

        // ── Model selector (quick switch) ────────────────────────────────────
        let models = if let Ok(cb) = MODEL_GET_LIST_CALLBACK.lock() {
            if let Some(ref callback) = *cb { callback() } else { Vec::new() }
        } else {
            Vec::new()
        };

        if models.is_empty() {
            let _ = AppendMenuW(menu, MF_GRAYED | MF_STRING, 0, w!("No models — open Settings"));
        } else {
            for model in &models {
                let flag = if model.is_current { MF_CHECKED } else { MF_UNCHECKED };
                let menu_id = ID_MODEL_START + model.index as u16;
                let display = format!("{}{}", model.name, model.size_label);
                let wide: Vec<u16> = display.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = AppendMenuW(
                    menu,
                    flag | MF_STRING,
                    menu_id as usize,
                    windows::core::PCWSTR(wide.as_ptr()),
                );
            }
        }

        // ── Recent recordings (last 3) ───────────────────────────────────────
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);

        let history_entries = if let Ok(cb) = HISTORY_GET_ENTRIES_CALLBACK.lock() {
            if let Some(ref callback) = *cb { callback() } else { Vec::new() }
        } else {
            Vec::new()
        };

        if history_entries.is_empty() {
            let _ = AppendMenuW(menu, MF_GRAYED | MF_STRING, 0, w!("No recent recordings"));
        } else {
            for entry in history_entries.iter().take(3) {
                let menu_id = ID_HISTORY_START + entry.id as u16;
                let wide_label: Vec<u16> =
                    entry.label.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = AppendMenuW(
                    menu,
                    MF_STRING,
                    menu_id as usize,
                    windows::core::PCWSTR(wide_label.as_ptr()),
                );
            }
        }

        // ── Actions ──────────────────────────────────────────────────────────
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            ID_OPEN_HISTORY as usize,
            w!("Open Recordings Folder"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS as usize, w!("Settings..."));
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT as usize, w!("Exit"));

        let mut pt = Default::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
    }
}
