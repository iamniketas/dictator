//! Settings window — native Win32 window for configuring Dictator.
//!
//! Opens on its own thread (modeless). Communicates with the main thread via
//! Arc callbacks passed in through [`SettingsParams`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use tracing::{error, info};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetStockObject,
    SelectObject, SetBkMode, TextOutW, DEFAULT_GUI_FONT, FW_BOLD, HBRUSH, PAINTSTRUCT, TRANSPARENT,
    WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, GWLP_USERDATA, HMENU, IDYES,
    LB_ADDSTRING, LB_GETCURSEL, LB_RESETCONTENT, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
    MB_YESNO, MSG, PostQuitMessage, RegisterClassW, SendMessageW, SetWindowLongPtrW,
    SetWindowTextW, ShowWindow, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_DESTROY, WM_PAINT, WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL, MessageBoxW, CS_HREDRAW, CS_VREDRAW,
};

// ── Win32 style constants not exposed as named consts in this windows-rs version ──
const BS_PUSHBUTTON: u32 = 0x0000_0000;
const BS_AUTOCHECKBOX: u32 = 0x0000_0003;
const LBS_NOTIFY: u32 = 0x0000_0001;
const LBS_NOINTEGRALHEIGHT: u32 = 0x0000_0100;
const CBS_DROPDOWNLIST: u32 = 0x0000_0003;

// ── Settings already open guard ───────────────────────────────────────────────
static SETTINGS_OPEN: AtomicBool = AtomicBool::new(false);

// ── Control IDs ───────────────────────────────────────────────────────────────
const IDC_MODEL_LIST: usize = 101;
const IDC_BTN_USE_MODEL: usize = 102;
const IDC_BTN_DELETE_MODEL: usize = 103;
const IDC_CMB_DOWNLOAD: usize = 104;
const IDC_BTN_DOWNLOAD: usize = 105;
const IDC_CMB_INJECTION: usize = 106;
const IDC_CHK_LLM: usize = 107;
const IDC_EDIT_OLLAMA_URL: usize = 108;
const IDC_EDIT_OLLAMA_MODEL: usize = 109;
const IDC_EDIT_IDLE: usize = 110;
const IDC_BTN_OPEN_LOGS: usize = 111;
const IDC_BTN_OPEN_CONFIG: usize = 112;
const IDC_BTN_SAVE: usize = 113;
const IDC_BTN_CLOSE: usize = 114;

// ── Public types ──────────────────────────────────────────────────────────────

/// An installed GGML model file.
#[derive(Clone)]
pub struct InstalledModel {
    pub path: PathBuf,
    pub name: String,
    pub size_label: String,
    pub is_active: bool,
}

/// Settings values collected when the user clicks Save.
pub struct SavedSettings {
    pub injection_method: String, // "direct" | "clipboard" | "clipboard_enter"
    pub llm_enabled: bool,
    pub ollama_url: String,
    pub ollama_model: String,
    pub idle_unload_minutes: u32,
}

/// Parameters passed to the settings window when it is opened.
pub struct SettingsParams {
    pub injection_method: String,
    pub llm_enabled: bool,
    pub ollama_url: String,
    pub ollama_model: String,
    pub idle_unload_minutes: u32,
    pub get_models: Arc<dyn Fn() -> Vec<InstalledModel> + Send + Sync>,
    pub on_use_model: Arc<dyn Fn(PathBuf) + Send + Sync>,
    pub on_delete_model: Arc<dyn Fn(PathBuf) + Send + Sync>,
    pub on_download_model: Arc<dyn Fn(usize) + Send + Sync>,
    pub on_save: Arc<dyn Fn(SavedSettings) + Send + Sync>,
    pub log_dir: PathBuf,
    pub config_path: PathBuf,
}

// ── Per-window state (stored in GWLP_USERDATA) ────────────────────────────────

struct WindowState {
    params: SettingsParams,
    hwnd_model_list: HWND,
    hwnd_cmb_download: HWND,
    hwnd_cmb_injection: HWND,
    hwnd_chk_llm: HWND,
    hwnd_edit_ollama_url: HWND,
    hwnd_edit_ollama_model: HWND,
    hwnd_edit_idle: HWND,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Open the settings window. Returns `false` if it is already open.
pub fn open(params: SettingsParams) -> bool {
    if SETTINGS_OPEN.swap(true, Ordering::SeqCst) {
        return false;
    }
    thread::spawn(move || {
        if let Err(e) = run_settings_window(params) {
            error!("[SETTINGS] Window error: {}", e);
        }
        SETTINGS_OPEN.store(false, Ordering::SeqCst);
    });
    true
}

// ── Window implementation ─────────────────────────────────────────────────────

fn run_settings_window(params: SettingsParams) -> anyhow::Result<()> {
    unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("DictatorSettings");

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: hinstance,
            lpszClassName: class_name,
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let state = Box::new(WindowState {
            params,
            hwnd_model_list: HWND(std::ptr::null_mut()),
            hwnd_cmb_download: HWND(std::ptr::null_mut()),
            hwnd_cmb_injection: HWND(std::ptr::null_mut()),
            hwnd_chk_llm: HWND(std::ptr::null_mut()),
            hwnd_edit_ollama_url: HWND(std::ptr::null_mut()),
            hwnd_edit_ollama_model: HWND(std::ptr::null_mut()),
            hwnd_edit_idle: HWND(std::ptr::null_mut()),
        });
        let state_ptr = Box::into_raw(state);

        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class_name,
            w!("Dictator — Settings"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT, CW_USEDEFAULT,
            482, 602,
            None, None, hinstance,
            Some(state_ptr as *mut _),
        )?;

        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            DispatchMessageW(&msg);
        }
        Ok(())
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                let cs = &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
                let state = &mut *(cs.lpCreateParams as *mut WindowState);
                create_controls(hwnd, state);
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as usize;
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    handle_command(hwnd, id, &mut *state_ptr);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                draw_section_labels(hwnd);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_CLOSE => {
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    drop(Box::from_raw(state_ptr));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                DestroyWindow(hwnd).ok();
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

// ── Control creation ──────────────────────────────────────────────────────────

fn create_controls(hwnd: HWND, state: &mut WindowState) {
    unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let font = GetStockObject(DEFAULT_GUI_FONT);

        // Helper macros → inline closures
        let mk = |class: windows::core::PCWSTR,
                  text: windows::core::PCWSTR,
                  style: u32,
                  x: i32, y: i32, w: i32, h: i32,
                  id: usize|
         -> HWND {
            let ctrl = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class, text,
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                x, y, w, h,
                hwnd,
                HMENU(id as *mut _),
                hinstance, None,
            )
            .unwrap_or(HWND(std::ptr::null_mut()));
            SendMessageW(ctrl, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            ctrl
        };

        let mk_ex = |class: windows::core::PCWSTR,
                     text: windows::core::PCWSTR,
                     ex: WINDOW_EX_STYLE,
                     style: u32,
                     x: i32, y: i32, w: i32, h: i32,
                     id: usize|
         -> HWND {
            let ctrl = CreateWindowExW(
                ex, class, text,
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                x, y, w, h,
                hwnd,
                HMENU(id as *mut _),
                hinstance, None,
            )
            .unwrap_or(HWND(std::ptr::null_mut()));
            SendMessageW(ctrl, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            ctrl
        };

        // ── MODELS section (y=30..180) ─────────────────────────────────────
        state.hwnd_model_list = mk_ex(
            w!("LISTBOX"), w!(""),
            WS_EX_CLIENTEDGE,
            LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | WS_VSCROLL.0 | WS_TABSTOP.0 | WS_BORDER.0,
            14, 46, 318, 110,
            IDC_MODEL_LIST,
        );
        populate_model_list(state.hwnd_model_list, &state.params);

        mk(w!("BUTTON"), w!("Use"), BS_PUSHBUTTON | WS_TABSTOP.0, 342, 46, 110, 26, IDC_BTN_USE_MODEL);
        mk(w!("BUTTON"), w!("Delete"), BS_PUSHBUTTON | WS_TABSTOP.0, 342, 78, 110, 26, IDC_BTN_DELETE_MODEL);

        // ── DOWNLOAD row (y=170..205) ──────────────────────────────────────
        mk(w!("STATIC"), w!("Model:"), 0, 14, 178, 50, 20, 0);
        state.hwnd_cmb_download = mk(
            w!("COMBOBOX"), w!(""),
            CBS_DROPDOWNLIST | WS_TABSTOP.0 | WS_VSCROLL.0,
            68, 174, 268, 140,
            IDC_CMB_DOWNLOAD,
        );
        for m in crate::model_downloader::KNOWN_MODELS {
            let label = format!("{} (~{} MB)\0", m.name, m.size_mb);
            let wide: Vec<u16> = label.encode_utf16().collect();
            SendMessageW(state.hwnd_cmb_download, CB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize));
        }
        SendMessageW(state.hwnd_cmb_download, CB_SETCURSEL, WPARAM(0), LPARAM(0));
        mk(w!("BUTTON"), w!("Download"), BS_PUSHBUTTON | WS_TABSTOP.0, 342, 174, 110, 26, IDC_BTN_DOWNLOAD);

        // ── GENERAL section (y=215..370) ──────────────────────────────────
        mk(w!("STATIC"), w!("Injection:"), 0, 14, 228, 68, 20, 0);
        state.hwnd_cmb_injection = mk(
            w!("COMBOBOX"), w!(""),
            CBS_DROPDOWNLIST | WS_TABSTOP.0,
            86, 224, 366, 80,
            IDC_CMB_INJECTION,
        );
        for opt in ["Direct (SendInput)\0", "Clipboard (Ctrl+V)\0", "Clipboard + Enter\0"] {
            let wide: Vec<u16> = opt.encode_utf16().collect();
            SendMessageW(state.hwnd_cmb_injection, CB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize));
        }
        let inj_sel: usize = match state.params.injection_method.as_str() {
            "clipboard" => 1,
            "clipboard_enter" => 2,
            _ => 0,
        };
        SendMessageW(state.hwnd_cmb_injection, CB_SETCURSEL, WPARAM(inj_sel), LPARAM(0));

        state.hwnd_chk_llm = mk(
            w!("BUTTON"), w!("LLM correction via Ollama"),
            BS_AUTOCHECKBOX | WS_TABSTOP.0,
            14, 258, 300, 22, IDC_CHK_LLM,
        );
        SendMessageW(
            state.hwnd_chk_llm, BM_SETCHECK,
            WPARAM(if state.params.llm_enabled { 1 } else { 0 }), LPARAM(0),
        );

        mk(w!("STATIC"), w!("Ollama URL:"), 0, 14, 288, 72, 20, 0);
        state.hwnd_edit_ollama_url = mk_ex(
            w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_TABSTOP.0 | WS_BORDER.0,
            90, 284, 362, 22, IDC_EDIT_OLLAMA_URL,
        );
        let url_wide: Vec<u16> = format!("{}\0", state.params.ollama_url).encode_utf16().collect();
        SetWindowTextW(state.hwnd_edit_ollama_url, windows::core::PCWSTR(url_wide.as_ptr())).ok();

        mk(w!("STATIC"), w!("Ollama model:"), 0, 14, 316, 72, 20, 0);
        state.hwnd_edit_ollama_model = mk_ex(
            w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_TABSTOP.0 | WS_BORDER.0,
            90, 312, 362, 22, IDC_EDIT_OLLAMA_MODEL,
        );
        let model_wide: Vec<u16> = format!("{}\0", state.params.ollama_model).encode_utf16().collect();
        SetWindowTextW(state.hwnd_edit_ollama_model, windows::core::PCWSTR(model_wide.as_ptr())).ok();

        mk(w!("STATIC"), w!("Idle unload:"), 0, 14, 344, 72, 20, 0);
        state.hwnd_edit_idle = mk_ex(
            w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_TABSTOP.0 | WS_BORDER.0,
            90, 340, 60, 22, IDC_EDIT_IDLE,
        );
        let idle_wide: Vec<u16> = format!("{}\0", state.params.idle_unload_minutes).encode_utf16().collect();
        SetWindowTextW(state.hwnd_edit_idle, windows::core::PCWSTR(idle_wide.as_ptr())).ok();
        mk(w!("STATIC"), w!("min  (0 = never unload)"), 0, 158, 344, 180, 20, 0);

        // ── ABOUT section (y=390..470) ────────────────────────────────────
        mk(w!("STATIC"), w!("Version: 0.1.0-alpha  \u{2022}  Runs 100% locally, no telemetry"), 0, 14, 412, 440, 20, 0);
        mk(w!("STATIC"), w!("Hotkey: Right Ctrl   hold >300ms = Push-to-Talk   tap = Toggle"), 0, 14, 434, 440, 20, 0);
        mk(w!("BUTTON"), w!("Open Logs Folder"), BS_PUSHBUTTON | WS_TABSTOP.0, 14, 462, 148, 26, IDC_BTN_OPEN_LOGS);
        mk(w!("BUTTON"), w!("Open Config File"), BS_PUSHBUTTON | WS_TABSTOP.0, 170, 462, 148, 26, IDC_BTN_OPEN_CONFIG);

        // ── Bottom bar ────────────────────────────────────────────────────
        mk(w!("BUTTON"), w!("Close"), BS_PUSHBUTTON | WS_TABSTOP.0, 248, 536, 100, 28, IDC_BTN_CLOSE);
        mk(w!("BUTTON"), w!("Save"), BS_PUSHBUTTON | WS_TABSTOP.0, 358, 536, 100, 28, IDC_BTN_SAVE);

        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
    }
}

fn populate_model_list(hwnd_list: HWND, params: &SettingsParams) {
    unsafe {
        SendMessageW(hwnd_list, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        let models = (params.get_models)();
        if models.is_empty() {
            let s: Vec<u16> = "No models installed\0".encode_utf16().collect();
            SendMessageW(hwnd_list, LB_ADDSTRING, WPARAM(0), LPARAM(s.as_ptr() as isize));
        } else {
            for m in &models {
                let active = if m.is_active { "  \u{2190} active" } else { "" };
                let label = format!("{}{}{}\0", m.name, m.size_label, active);
                let wide: Vec<u16> = label.encode_utf16().collect();
                SendMessageW(hwnd_list, LB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize));
            }
        }
    }
}

// ── Section label drawing ─────────────────────────────────────────────────────

fn draw_section_labels(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        SetBkMode(hdc, TRANSPARENT);

        let bold = CreateFontW(
            15, 0, 0, 0, FW_BOLD.0 as i32, 0, 0, 0,
            windows::Win32::Graphics::Gdi::DEFAULT_CHARSET.0 as u32,
            0, 0, 0, 0,
            w!("Segoe UI"),
        );

        let sections: &[(&str, i32)] = &[
            ("Models", 14),
            ("Download", 162),
            ("General", 210),
            ("About", 396),
        ];

        let old = SelectObject(hdc, bold);
        for (label, y) in sections {
            let wide: Vec<u16> = label.encode_utf16().collect();
            let _ = TextOutW(hdc, 14, *y, &wide);

            let sep_brush = CreateSolidBrush(COLORREF(0x00CCCCCC));
            let sep_rect = RECT { left: 14, top: y + 18, right: 454, bottom: y + 19 };
            let _ = FillRect(hdc, &sep_rect, sep_brush);
            let _ = DeleteObject(sep_brush);
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(bold);

        let _ = EndPaint(hwnd, &ps);
    }
}

// ── Command handling ──────────────────────────────────────────────────────────

fn handle_command(hwnd: HWND, id: usize, state: &mut WindowState) {
    match id {
        IDC_BTN_USE_MODEL => {
            let sel = unsafe {
                SendMessageW(state.hwnd_model_list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0
            };
            if sel >= 0 {
                let models = (state.params.get_models)();
                if let Some(m) = models.get(sel as usize) {
                    info!("[SETTINGS] Switching to model: {:?}", m.path);
                    (state.params.on_use_model)(m.path.clone());
                    populate_model_list(state.hwnd_model_list, &state.params);
                }
            }
        }
        IDC_BTN_DELETE_MODEL => {
            let sel = unsafe {
                SendMessageW(state.hwnd_model_list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0
            };
            if sel >= 0 {
                let models = (state.params.get_models)();
                if let Some(m) = models.get(sel as usize) {
                    if m.is_active {
                        unsafe {
                            MessageBoxW(hwnd,
                                w!("Cannot delete the active model.\nSwitch to another model first."),
                                w!("Dictator"), MB_OK | MB_ICONERROR);
                        }
                        return;
                    }
                    unsafe {
                        let msg: Vec<u16> = format!("Delete {}?\nThis cannot be undone.\0", m.name)
                            .encode_utf16().collect();
                        let r = MessageBoxW(hwnd,
                            windows::core::PCWSTR(msg.as_ptr()),
                            w!("Confirm Delete"), MB_YESNO | MB_ICONINFORMATION);
                        if r == IDYES {
                            (state.params.on_delete_model)(m.path.clone());
                            populate_model_list(state.hwnd_model_list, &state.params);
                        }
                    }
                }
            }
        }
        IDC_BTN_DOWNLOAD => {
            let sel = unsafe {
                SendMessageW(state.hwnd_cmb_download, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0
            };
            if sel >= 0 {
                info!("[SETTINGS] Download requested for model index {}", sel);
                (state.params.on_download_model)(sel as usize);
            }
        }
        IDC_BTN_OPEN_LOGS => {
            if let Err(e) = std::process::Command::new("explorer")
                .arg(&state.params.log_dir).spawn()
            {
                error!("[SETTINGS] Failed to open logs: {}", e);
            }
        }
        IDC_BTN_OPEN_CONFIG => {
            if let Err(e) = std::process::Command::new("notepad")
                .arg(&state.params.config_path).spawn()
            {
                error!("[SETTINGS] Failed to open config: {}", e);
            }
        }
        IDC_BTN_SAVE => {
            let settings = collect_settings(state);
            info!("[SETTINGS] Saving: injection={} llm={}", settings.injection_method, settings.llm_enabled);
            (state.params.on_save)(settings);
            unsafe {
                MessageBoxW(hwnd, w!("Settings saved."), w!("Dictator"), MB_OK | MB_ICONINFORMATION);
            }
        }
        IDC_BTN_CLOSE => {
            unsafe { let _ = DestroyWindow(hwnd); }
        }
        _ => {}
    }
}

fn collect_settings(state: &WindowState) -> SavedSettings {
    let injection_method = unsafe {
        let sel = SendMessageW(state.hwnd_cmb_injection, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
        match sel {
            1 => "clipboard".to_string(),
            2 => "clipboard_enter".to_string(),
            _ => "direct".to_string(),
        }
    };
    let llm_enabled = unsafe {
        SendMessageW(state.hwnd_chk_llm, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 != 0
    };
    SavedSettings {
        injection_method,
        llm_enabled,
        ollama_url: read_edit(state.hwnd_edit_ollama_url),
        ollama_model: read_edit(state.hwnd_edit_ollama_model),
        idle_unload_minutes: read_edit(state.hwnd_edit_idle).trim().parse().unwrap_or(5),
    }
}

fn read_edit(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 { return String::new(); }
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
    }
}
