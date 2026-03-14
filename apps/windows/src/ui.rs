//! UI module - System tray and overlay windows

use anyhow::Result;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_HREDRAW, CS_VREDRAW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, IDI_APPLICATION, IMAGE_ICON,
    LR_LOADFROMFILE, LoadIconW, LoadImageW, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, MF_UNCHECKED, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TrackPopupMenu, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_NULL, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_USER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::w;

const WM_TRAYICON: u32 = WM_USER + 1;
const ID_EXIT: u16 = 1001;
const ID_STREAMING: u16 = 1002;
const ID_CHUNK_3: u16 = 1003;
const ID_CHUNK_8: u16 = 1004;
const ID_CHUNK_15: u16 = 1005;
const ID_OLLAMA: u16 = 1007;
const ID_OPEN_CONFIG: u16 = 1008;
const ID_MODEL_START: u16 = 1200; // Start of dynamic model IDs (1200-1299)
const ID_MODEL_END: u16 = 1299;
const ID_DOWNLOAD_START: u16 = 1300; // Start of download model IDs (1300-1399)
const ID_DOWNLOAD_END: u16 = 1399;
const ID_SETTINGS: u16 = 1010;
const ID_INSTALL_UPDATE: u16 = 1011;

static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
static STREAMING_ENABLED: AtomicBool = AtomicBool::new(false);
static STREAMING_CHUNK_SECONDS: AtomicU64 = AtomicU64::new(15);
static OLLAMA_ENABLED: AtomicBool = AtomicBool::new(false);
static IS_DOWNLOADING: AtomicBool = AtomicBool::new(false);
static UPDATE_AVAILABLE_VERSION: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static INSTALL_UPDATE_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() + Send + 'static>>> =
    std::sync::Mutex::new(None);

// History callbacks
static HISTORY_OPEN_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() + Send + 'static>>> =
    std::sync::Mutex::new(None);
static HISTORY_COPY_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn(usize) + Send + 'static>>> =
    std::sync::Mutex::new(None);
static HISTORY_GET_ENTRIES_CALLBACK: std::sync::Mutex<
    Option<Box<dyn Fn() -> Vec<HistoryMenuEntry> + Send + 'static>>,
> = std::sync::Mutex::new(None);

// Model selector callbacks
static MODEL_SELECT_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn(usize) + Send + 'static>>> =
    std::sync::Mutex::new(None);
static MODEL_GET_LIST_CALLBACK: std::sync::Mutex<
    Option<Box<dyn Fn() -> Vec<ModelMenuItem> + Send + 'static>>,
> = std::sync::Mutex::new(None);

// Config open callback
static OPEN_CONFIG_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() + Send + 'static>>> =
    std::sync::Mutex::new(None);

// Download model callbacks
static DOWNLOAD_MODEL_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn(usize) + Send + 'static>>> =
    std::sync::Mutex::new(None);
static DOWNLOAD_LIST_CALLBACK: std::sync::Mutex<
    Option<Box<dyn Fn() -> Vec<DownloadModelItem> + Send + 'static>>,
> = std::sync::Mutex::new(None);

// Settings window callback
static SETTINGS_CALLBACK: std::sync::Mutex<Option<Box<dyn Fn() + Send + 'static>>> =
    std::sync::Mutex::new(None);

/// Entry for history menu
#[derive(Debug, Clone)]
pub struct HistoryMenuEntry {
    pub id: usize, // 0-based index
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

/// Set the callback invoked when user selects a model to download (receives 0-based catalog index)
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

/// Notify that an update is available (shown in tray menu).
pub fn set_update_available(version: String) {
    if let Ok(mut v) = UPDATE_AVAILABLE_VERSION.lock() {
        *v = Some(version);
    }
}

/// Set the callback invoked when user clicks the update item in the tray.
pub fn set_install_update_callback<F>(callback: F)
where
    F: Fn() + Send + 'static,
{
    if let Ok(mut cb) = INSTALL_UPDATE_CALLBACK.lock() {
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
        let mut version_nid = nid;
        version_nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &version_nid);

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
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        window_proc_impl(hwnd, msg, wparam, lparam)
    })) {
        Ok(result) => result,
        Err(_) => {
            eprintln!("[TRAY] panic in window_proc; ignoring message");
            LRESULT(0)
        }
    }
}

unsafe fn window_proc_impl(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            let tray_event = (lparam.0 as u32) & 0xFFFF;
            if tray_event == WM_RBUTTONUP
                || tray_event == WM_RBUTTONDOWN
                || tray_event == WM_CONTEXTMENU
            {
                unsafe { show_context_menu(hwnd) };
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = (wparam.0 & 0xFFFF) as u16;
            if cmd == ID_EXIT {
                SHOULD_EXIT.store(true, Ordering::SeqCst);
                unsafe { PostQuitMessage(0) };
            } else if cmd == ID_STREAMING {
                let new_state = !STREAMING_ENABLED.load(Ordering::SeqCst);
                STREAMING_ENABLED.store(new_state, Ordering::SeqCst);
                eprintln!(
                    "[TRAY] Streaming {}",
                    if new_state { "enabled" } else { "disabled" }
                );
            } else if cmd == ID_CHUNK_3 {
                set_streaming_chunk_seconds(3);
                eprintln!("[TRAY] Streaming chunk set to 3s");
            } else if cmd == ID_CHUNK_8 {
                set_streaming_chunk_seconds(8);
                eprintln!("[TRAY] Streaming chunk set to 8s");
            } else if cmd == ID_CHUNK_15 {
                set_streaming_chunk_seconds(15);
                eprintln!("[TRAY] Streaming chunk set to 15s");
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
                eprintln!(
                    "[TRAY] Ollama LLM correction {}",
                    if new_state { "enabled" } else { "disabled" }
                );
            } else if cmd == ID_OPEN_CONFIG {
                eprintln!("[TRAY] Opening config file");
                if let Ok(cb) = OPEN_CONFIG_CALLBACK.lock() {
                    if let Some(ref callback) = *cb {
                        callback();
                    }
                }
            } else if cmd == ID_INSTALL_UPDATE {
                eprintln!("[TRAY] Install update clicked");
                if let Ok(cb) = INSTALL_UPDATE_CALLBACK.lock() {
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
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };

        // Update notification (shown when a new version is available).
        if let Ok(guard) = UPDATE_AVAILABLE_VERSION.lock() {
            if let Some(ref version) = *guard {
                let label = format!("\u{2191} Update available (v{}) \u{2014} Install", version);
                let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = AppendMenuW(
                    menu,
                    MF_STRING,
                    ID_INSTALL_UPDATE as usize,
                    windows::core::PCWSTR(wide.as_ptr()),
                );
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            }
        }

        // Model selector.
        let models = if let Ok(cb) = MODEL_GET_LIST_CALLBACK.lock() {
            if let Some(ref callback) = *cb {
                callback()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        if models.is_empty() {
            let _ = AppendMenuW(menu, MF_GRAYED | MF_STRING, 0, w!("No models - open Settings"));
        } else {
            for model in &models {
                let flag = if model.is_current {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                };
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

        // Dictation mode toggles.
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_GRAYED | MF_STRING, 0, w!("Dictation Mode"));
        let streaming_enabled = STREAMING_ENABLED.load(Ordering::SeqCst);
        let streaming_flag = if streaming_enabled {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        let _ = AppendMenuW(
            menu,
            streaming_flag | MF_STRING,
            ID_STREAMING as usize,
            w!("Streaming Transcription"),
        );
        let selected_chunk = STREAMING_CHUNK_SECONDS.load(Ordering::SeqCst);
        let chunk3_flag = if selected_chunk == 3 {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        let chunk8_flag = if selected_chunk == 8 {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        let chunk15_flag = if selected_chunk == 15 {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        let disabled_if_full = if streaming_enabled {
            MENU_ITEM_FLAGS(0)
        } else {
            MF_GRAYED
        };
        let _ = AppendMenuW(
            menu,
            chunk3_flag | MF_STRING | disabled_if_full,
            ID_CHUNK_3 as usize,
            w!("Chunk: 3s"),
        );
        let _ = AppendMenuW(
            menu,
            chunk8_flag | MF_STRING | disabled_if_full,
            ID_CHUNK_8 as usize,
            w!("Chunk: 8s"),
        );
        let _ = AppendMenuW(
            menu,
            chunk15_flag | MF_STRING | disabled_if_full,
            ID_CHUNK_15 as usize,
            w!("Chunk: 15s"),
        );

        // Main actions.
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS as usize, w!("Settings"));
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT as usize, w!("Exit"));

        let mut pt = Default::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}
