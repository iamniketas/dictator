
//! Settings window - native Win32 settings UI.
//! Multi-page layout with immediate apply (no Save button).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use tracing::error;
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT, HBRUSH, WHITE_BRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HMENU, IDYES, KillTimer, LB_ADDSTRING,
    LB_GETCURSEL, LB_RESETCONTENT, MB_ICONERROR,
    MB_ICONINFORMATION, MB_OK, MB_YESNO, MSG, MessageBoxW, PostQuitMessage, RegisterClassW,
    SendMessageW, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, SW_HIDE, SW_SHOW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_SETFONT,
    WM_TIMER, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

const BS_PUSHBUTTON: u32 = 0x0000_0000;
const BS_AUTOCHECKBOX: u32 = 0x0000_0003;
const BS_FLAT: u32 = 0x0000_8000;
const CBS_DROPDOWNLIST: u32 = 0x0000_0003;
const TIMER_UI_REFRESH: usize = 1;

const IDC_NAV_MODELS: usize = 1001;
const IDC_NAV_RUNTIME: usize = 1002;
const IDC_NAV_DICTATION: usize = 1003;
const IDC_NAV_STORAGE: usize = 1004;
const IDC_NAV_ABOUT: usize = 1005;

const IDC_MODEL_LIST: usize = 1101;
const IDC_MODEL_DETAILS: usize = 1102;
const IDC_BTN_USE_MODEL: usize = 1103;
const IDC_BTN_DELETE_MODEL: usize = 1104;
const IDC_DOWNLOAD_LIST: usize = 1105;
const IDC_DOWNLOAD_DETAILS: usize = 1106;
const IDC_BTN_DOWNLOAD: usize = 1107;
const IDC_PROGRESS_DOWNLOAD: usize = 1108;
const IDC_STATIC_DOWNLOAD_STATUS: usize = 1109;

const IDC_CMB_RUNTIME_MODE: usize = 1201;
const IDC_RUNTIME_EXPLAIN: usize = 1202;
const IDC_RUNTIME_STATUS: usize = 1203;
const IDC_BTN_REFRESH_RUNTIME: usize = 1204;

const IDC_CMB_INJECTION: usize = 1301;
const IDC_CHK_LLM: usize = 1302;
const IDC_EDIT_OLLAMA_URL: usize = 1303;
const IDC_EDIT_OLLAMA_MODEL: usize = 1304;
const IDC_EDIT_IDLE: usize = 1305;

const IDC_STORAGE_INFO: usize = 1401;
const IDC_BTN_OPEN_SHARED_MODELS: usize = 1402;
const IDC_BTN_OPEN_SHARED_STORE: usize = 1403;
const IDC_BTN_OPEN_HISTORY: usize = 1404;
const IDC_BTN_OPEN_LOGS: usize = 1405;
const IDC_BTN_OPEN_CONFIG: usize = 1406;

const IDC_ABOUT_INFO: usize = 1501;
const IDC_BTN_CLOSE: usize = 1502;

const PBM_SETRANGE32: u32 = 0x0406;
const PBM_SETPOS: u32 = 0x0402;
const LBS_NOTIFY_U32: u32 = 0x0000_0001;
const LBS_NOINTEGRALHEIGHT_U32: u32 = 0x0000_0100;
const ES_MULTILINE_U32: u32 = 0x0000_0004;
const ES_AUTOVSCROLL_U32: u32 = 0x0000_0040;
const ES_READONLY_U32: u32 = 0x0000_0800;
const CBN_SELCHANGE_U16: u16 = 1;
const EN_KILLFOCUS_U16: u16 = 0x0200;

static SETTINGS_OPEN: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub struct InstalledModel {
    pub path: PathBuf,
    pub name: String,
    pub size_label: String,
    pub is_active: bool,
}

pub struct SavedSettings {
    pub injection_method: String,
    pub llm_enabled: bool,
    pub ollama_url: String,
    pub ollama_model: String,
    pub idle_unload_minutes: u32,
    pub runtime_mode: String,
}

#[derive(Clone, Default)]
pub struct DownloadStatus {
    pub active: bool,
    pub model_name: String,
    pub progress: f32,
    pub downloaded_mb: f32,
    pub total_mb: f32,
    pub speed_mbps: f32,
    pub eta_seconds: Option<u64>,
    pub completed: bool,
    pub error: Option<String>,
}

pub struct RuntimeStatus {
    pub backend: String,
    pub device: String,
    pub preferred_model: String,
    pub fallback_chain: String,
    pub server_fallback: bool,
    pub cloud_fallback: bool,
    pub last_stage: String,
    pub hardware_summary: String,
    pub storage_summary: String,
}

pub struct SettingsParams {
    pub injection_method: String,
    pub llm_enabled: bool,
    pub ollama_url: String,
    pub ollama_model: String,
    pub idle_unload_minutes: u32,
    pub runtime_mode: String,
    pub get_models: Arc<dyn Fn() -> Vec<InstalledModel> + Send + Sync>,
    pub on_use_model: Arc<dyn Fn(PathBuf) + Send + Sync>,
    pub on_delete_model: Arc<dyn Fn(PathBuf) + Send + Sync>,
    pub on_download_model: Arc<dyn Fn(usize) + Send + Sync>,
    pub on_save: Arc<dyn Fn(SavedSettings) + Send + Sync>,
    pub get_runtime_status: Arc<dyn Fn() -> RuntimeStatus + Send + Sync>,
    pub get_download_status: Arc<dyn Fn() -> DownloadStatus + Send + Sync>,
    pub log_dir: PathBuf,
    pub config_path: PathBuf,
    pub shared_models_dir: PathBuf,
    pub shared_store_path: PathBuf,
    pub history_enabled: bool,
    pub history_retention_days: u32,
    pub hotkey_summary: String,
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum SettingsPage {
    Models,
    Runtime,
    Dictation,
    Storage,
    About,
}

struct WindowState {
    params: SettingsParams,
    current_page: SettingsPage,
    nav_models: HWND,
    nav_runtime: HWND,
    nav_dictation: HWND,
    nav_storage: HWND,
    nav_about: HWND,
    models_controls: Vec<HWND>,
    runtime_controls: Vec<HWND>,
    dictation_controls: Vec<HWND>,
    storage_controls: Vec<HWND>,
    about_controls: Vec<HWND>,
    hwnd_model_list: HWND,
    hwnd_model_details: HWND,
    hwnd_download_list: HWND,
    hwnd_download_details: HWND,
    hwnd_download_progress: HWND,
    hwnd_download_status: HWND,
    hwnd_cmb_runtime_mode: HWND,
    hwnd_runtime_explain: HWND,
    hwnd_runtime_status: HWND,
    hwnd_cmb_injection: HWND,
    hwnd_chk_llm: HWND,
    hwnd_edit_ollama_url: HWND,
    hwnd_edit_ollama_model: HWND,
    hwnd_edit_idle: HWND,
    hwnd_storage_info: HWND,
    hwnd_about_info: HWND,
    last_download_completed: bool,
}

impl WindowState {
    fn page_controls(&self, page: SettingsPage) -> &Vec<HWND> {
        match page {
            SettingsPage::Models => &self.models_controls,
            SettingsPage::Runtime => &self.runtime_controls,
            SettingsPage::Dictation => &self.dictation_controls,
            SettingsPage::Storage => &self.storage_controls,
            SettingsPage::About => &self.about_controls,
        }
    }
}
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
            current_page: SettingsPage::Models,
            nav_models: HWND(std::ptr::null_mut()),
            nav_runtime: HWND(std::ptr::null_mut()),
            nav_dictation: HWND(std::ptr::null_mut()),
            nav_storage: HWND(std::ptr::null_mut()),
            nav_about: HWND(std::ptr::null_mut()),
            models_controls: vec![],
            runtime_controls: vec![],
            dictation_controls: vec![],
            storage_controls: vec![],
            about_controls: vec![],
            hwnd_model_list: HWND(std::ptr::null_mut()),
            hwnd_model_details: HWND(std::ptr::null_mut()),
            hwnd_download_list: HWND(std::ptr::null_mut()),
            hwnd_download_details: HWND(std::ptr::null_mut()),
            hwnd_download_progress: HWND(std::ptr::null_mut()),
            hwnd_download_status: HWND(std::ptr::null_mut()),
            hwnd_cmb_runtime_mode: HWND(std::ptr::null_mut()),
            hwnd_runtime_explain: HWND(std::ptr::null_mut()),
            hwnd_runtime_status: HWND(std::ptr::null_mut()),
            hwnd_cmb_injection: HWND(std::ptr::null_mut()),
            hwnd_chk_llm: HWND(std::ptr::null_mut()),
            hwnd_edit_ollama_url: HWND(std::ptr::null_mut()),
            hwnd_edit_ollama_model: HWND(std::ptr::null_mut()),
            hwnd_edit_idle: HWND(std::ptr::null_mut()),
            hwnd_storage_info: HWND(std::ptr::null_mut()),
            hwnd_about_info: HWND(std::ptr::null_mut()),
            last_download_completed: false,
        });

        let state_ptr = Box::into_raw(state);
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class_name,
            w!("Dictator Settings"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1180,
            790,
            None,
            None,
            hinstance,
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
                SetWindowLongPtrW(hwnd, windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA, cs.lpCreateParams as isize);
                let state = &mut *(cs.lpCreateParams as *mut WindowState);
                create_controls(hwnd, state);
                show_page(state, SettingsPage::Models);
                let _ = SetTimer(hwnd, TIMER_UI_REFRESH, 300, None);
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as usize;
                let notify = ((wparam.0 >> 16) & 0xFFFF) as u16;
                let state_ptr = GetWindowLongPtrW(hwnd, windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    handle_command(hwnd, id, notify, &mut *state_ptr);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                let state_ptr = GetWindowLongPtrW(hwnd, windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    populate_runtime_panel(state);
                    populate_storage_panel(state);
                    populate_about_panel(state);
                    populate_download_status(state);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = KillTimer(hwnd, TIMER_UI_REFRESH);
                let state_ptr = GetWindowLongPtrW(hwnd, windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    drop(Box::from_raw(state_ptr));
                    SetWindowLongPtrW(hwnd, windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA, 0);
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

fn create_controls(hwnd: HWND, state: &mut WindowState) {
    unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let font = GetStockObject(DEFAULT_GUI_FONT);

        let mk = |class: windows::core::PCWSTR,
                  text: windows::core::PCWSTR,
                  style: u32,
                  x: i32,
                  y: i32,
                  w: i32,
                  h: i32,
                  id: usize|
         -> HWND {
            let ctrl = CreateWindowExW(
                WINDOW_EX_STYLE::default(), class, text,
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                x, y, w, h, hwnd, HMENU(id as *mut _), hinstance, None,
            ).unwrap_or(HWND(std::ptr::null_mut()));
            SendMessageW(ctrl, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            ctrl
        };

        let mk_ex = |class: windows::core::PCWSTR,
                     text: windows::core::PCWSTR,
                     ex: WINDOW_EX_STYLE,
                     style: u32,
                     x: i32,
                     y: i32,
                     w: i32,
                     h: i32,
                     id: usize|
         -> HWND {
            let ctrl = CreateWindowExW(
                ex, class, text,
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                x, y, w, h, hwnd, HMENU(id as *mut _), hinstance, None,
            ).unwrap_or(HWND(std::ptr::null_mut()));
            SendMessageW(ctrl, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            ctrl
        };

        mk(w!("STATIC"), w!("Settings"), 0, 20, 20, 160, 24, 0);
        state.nav_models = mk(w!("BUTTON"), w!("Models"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 20, 64, 180, 34, IDC_NAV_MODELS);
        state.nav_runtime = mk(w!("BUTTON"), w!("Runtime"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 20, 104, 180, 34, IDC_NAV_RUNTIME);
        state.nav_dictation = mk(w!("BUTTON"), w!("Dictation"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 20, 144, 180, 34, IDC_NAV_DICTATION);
        state.nav_storage = mk(w!("BUTTON"), w!("Storage"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 20, 184, 180, 34, IDC_NAV_STORAGE);
        state.nav_about = mk(w!("BUTTON"), w!("About"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 20, 224, 180, 34, IDC_NAV_ABOUT);
        let m_title = mk(w!("STATIC"), w!("Models"), 0, 230, 20, 860, 24, 0);
        state.models_controls.push(m_title);
        state.models_controls.push(mk(w!("STATIC"), w!("Installed models"), 0, 230, 56, 280, 20, 0));

        state.hwnd_model_list = mk_ex(w!("LISTBOX"), w!(""), WS_EX_CLIENTEDGE,
            LBS_NOTIFY_U32 | LBS_NOINTEGRALHEIGHT_U32 | WS_VSCROLL.0 | WS_TABSTOP.0 | WS_BORDER.0,
            230, 80, 420, 210, IDC_MODEL_LIST);
        state.models_controls.push(state.hwnd_model_list);

        state.hwnd_model_details = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_BORDER.0 | ES_MULTILINE_U32 | ES_AUTOVSCROLL_U32 | ES_READONLY_U32,
            230, 300, 420, 130, IDC_MODEL_DETAILS);
        state.models_controls.push(state.hwnd_model_details);

        state.models_controls.push(mk(w!("BUTTON"), w!("Use Selected"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 662, 80, 150, 30, IDC_BTN_USE_MODEL));
        state.models_controls.push(mk(w!("BUTTON"), w!("Delete Selected"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 662, 118, 150, 30, IDC_BTN_DELETE_MODEL));

        state.models_controls.push(mk(w!("STATIC"), w!("Download catalog"), 0, 830, 56, 260, 20, 0));
        state.hwnd_download_list = mk_ex(w!("LISTBOX"), w!(""), WS_EX_CLIENTEDGE,
            LBS_NOTIFY_U32 | LBS_NOINTEGRALHEIGHT_U32 | WS_VSCROLL.0 | WS_TABSTOP.0 | WS_BORDER.0,
            830, 80, 260, 210, IDC_DOWNLOAD_LIST);
        state.models_controls.push(state.hwnd_download_list);

        state.hwnd_download_details = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_BORDER.0 | ES_MULTILINE_U32 | ES_AUTOVSCROLL_U32 | ES_READONLY_U32,
            830, 300, 260, 130, IDC_DOWNLOAD_DETAILS);
        state.models_controls.push(state.hwnd_download_details);

        state.models_controls.push(mk(w!("BUTTON"), w!("Download"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 830, 438, 130, 30, IDC_BTN_DOWNLOAD));

        state.hwnd_download_progress = mk(w!("msctls_progress32"), w!(""), WS_TABSTOP.0, 230, 446, 420, 18, IDC_PROGRESS_DOWNLOAD);
        SendMessageW(state.hwnd_download_progress, PBM_SETRANGE32, WPARAM(0), LPARAM(100));
        state.models_controls.push(state.hwnd_download_progress);

        state.hwnd_download_status = mk(w!("STATIC"), w!("Download: idle"), 0, 230, 470, 860, 20, IDC_STATIC_DOWNLOAD_STATUS);
        state.models_controls.push(state.hwnd_download_status);

        state.runtime_controls.push(mk(w!("STATIC"), w!("Runtime & Recommendations"), 0, 230, 20, 860, 24, 0));
        state.runtime_controls.push(mk(w!("STATIC"), w!("Runtime mode"), 0, 230, 60, 140, 20, 0));
        state.hwnd_cmb_runtime_mode = mk(w!("COMBOBOX"), w!(""), CBS_DROPDOWNLIST | WS_TABSTOP.0, 230, 84, 260, 140, IDC_CMB_RUNTIME_MODE);
        state.runtime_controls.push(state.hwnd_cmb_runtime_mode);
        for opt in ["Auto (Recommended)\0", "Force GPU\0", "Force CPU\0"] {
            let wide: Vec<u16> = opt.encode_utf16().collect();
            SendMessageW(state.hwnd_cmb_runtime_mode, CB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize));
        }
        let runtime_sel = match state.params.runtime_mode.as_str() { "force_gpu" => 1, "force_cpu" => 2, _ => 0 };
        SendMessageW(state.hwnd_cmb_runtime_mode, CB_SETCURSEL, WPARAM(runtime_sel), LPARAM(0));
        state.runtime_controls.push(mk(w!("BUTTON"), w!("Refresh diagnostics"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 500, 84, 180, 30, IDC_BTN_REFRESH_RUNTIME));
        state.hwnd_runtime_explain = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_BORDER.0 | ES_MULTILINE_U32 | ES_AUTOVSCROLL_U32 | ES_READONLY_U32,
            230, 130, 860, 110, IDC_RUNTIME_EXPLAIN);
        state.runtime_controls.push(state.hwnd_runtime_explain);
        state.hwnd_runtime_status = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_BORDER.0 | ES_MULTILINE_U32 | ES_AUTOVSCROLL_U32 | ES_READONLY_U32,
            230, 250, 860, 320, IDC_RUNTIME_STATUS);
        state.runtime_controls.push(state.hwnd_runtime_status);

        state.dictation_controls.push(mk(w!("STATIC"), w!("Dictation Behavior"), 0, 230, 20, 860, 24, 0));
        state.dictation_controls.push(mk(w!("STATIC"), w!("Text injection"), 0, 230, 60, 140, 20, 0));
        state.hwnd_cmb_injection = mk(w!("COMBOBOX"), w!(""), CBS_DROPDOWNLIST | WS_TABSTOP.0, 230, 84, 360, 120, IDC_CMB_INJECTION);
        state.dictation_controls.push(state.hwnd_cmb_injection);
        for opt in ["Direct (SendInput)\0", "Clipboard (Ctrl+V)\0", "Clipboard + Enter\0"] {
            let wide: Vec<u16> = opt.encode_utf16().collect();
            SendMessageW(state.hwnd_cmb_injection, CB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize));
        }
        let inj_sel: usize = match state.params.injection_method.as_str() { "clipboard" => 1, "clipboard_enter" => 2, _ => 0 };
        SendMessageW(state.hwnd_cmb_injection, CB_SETCURSEL, WPARAM(inj_sel), LPARAM(0));

        state.hwnd_chk_llm = mk(w!("BUTTON"), w!("Enable Ollama correction"), BS_AUTOCHECKBOX | WS_TABSTOP.0, 230, 126, 280, 24, IDC_CHK_LLM);
        SendMessageW(state.hwnd_chk_llm, BM_SETCHECK, WPARAM(if state.params.llm_enabled { 1 } else { 0 }), LPARAM(0));
        state.dictation_controls.push(state.hwnd_chk_llm);
        state.dictation_controls.push(mk(w!("STATIC"), w!("Ollama URL"), 0, 230, 166, 120, 20, 0));

        state.hwnd_edit_ollama_url = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE, WS_TABSTOP.0 | WS_BORDER.0, 230, 190, 560, 26, IDC_EDIT_OLLAMA_URL);
        state.dictation_controls.push(state.hwnd_edit_ollama_url);
        set_control_text(state.hwnd_edit_ollama_url, &state.params.ollama_url);

        state.dictation_controls.push(mk(w!("STATIC"), w!("Ollama model"), 0, 230, 228, 120, 20, 0));
        state.hwnd_edit_ollama_model = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE, WS_TABSTOP.0 | WS_BORDER.0, 230, 252, 360, 26, IDC_EDIT_OLLAMA_MODEL);
        state.dictation_controls.push(state.hwnd_edit_ollama_model);
        set_control_text(state.hwnd_edit_ollama_model, &state.params.ollama_model);

        state.dictation_controls.push(mk(w!("STATIC"), w!("Idle unload (minutes)"), 0, 230, 290, 200, 20, 0));
        state.hwnd_edit_idle = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE, WS_TABSTOP.0 | WS_BORDER.0, 230, 314, 120, 26, IDC_EDIT_IDLE);
        state.dictation_controls.push(state.hwnd_edit_idle);
        set_control_text(state.hwnd_edit_idle, &state.params.idle_unload_minutes.to_string());

        state.storage_controls.push(mk(w!("STATIC"), w!("Shared Storage"), 0, 230, 20, 860, 24, 0));
        state.hwnd_storage_info = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_BORDER.0 | ES_MULTILINE_U32 | ES_AUTOVSCROLL_U32 | ES_READONLY_U32,
            230, 60, 860, 260, IDC_STORAGE_INFO);
        state.storage_controls.push(state.hwnd_storage_info);
        state.storage_controls.push(mk(w!("BUTTON"), w!("Open models folder"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 230, 336, 180, 30, IDC_BTN_OPEN_SHARED_MODELS));
        state.storage_controls.push(mk(w!("BUTTON"), w!("Open store JSON"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 420, 336, 160, 30, IDC_BTN_OPEN_SHARED_STORE));
        state.storage_controls.push(mk(w!("BUTTON"), w!("Open history folder"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 590, 336, 170, 30, IDC_BTN_OPEN_HISTORY));
        state.storage_controls.push(mk(w!("BUTTON"), w!("Open logs"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 770, 336, 120, 30, IDC_BTN_OPEN_LOGS));
        state.storage_controls.push(mk(w!("BUTTON"), w!("Open config"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 900, 336, 130, 30, IDC_BTN_OPEN_CONFIG));

        state.about_controls.push(mk(w!("STATIC"), w!("About"), 0, 230, 20, 860, 24, 0));
        state.hwnd_about_info = mk_ex(w!("EDIT"), w!(""), WS_EX_CLIENTEDGE,
            WS_BORDER.0 | ES_MULTILINE_U32 | ES_AUTOVSCROLL_U32 | ES_READONLY_U32,
            230, 60, 860, 280, IDC_ABOUT_INFO);
        state.about_controls.push(state.hwnd_about_info);
        state.about_controls.push(mk(w!("BUTTON"), w!("Close"), BS_PUSHBUTTON | BS_FLAT | WS_TABSTOP.0, 960, 700, 130, 34, IDC_BTN_CLOSE));

        populate_model_list(state.hwnd_model_list, &state.params);
        populate_download_list(state);
        populate_runtime_panel(state);
        populate_storage_panel(state);
        populate_about_panel(state);
        populate_model_details(state);
        populate_download_details(state);
        populate_download_status(state);
    }
}

fn set_control_text(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { SetWindowTextW(hwnd, windows::core::PCWSTR(wide.as_ptr())).ok(); }
}

fn set_active_nav(state: &WindowState) {
    let (m, r, d, s, a) = match state.current_page {
        SettingsPage::Models => ("Models  [Selected]", "Runtime", "Dictation", "Storage", "About"),
        SettingsPage::Runtime => ("Models", "Runtime  [Selected]", "Dictation", "Storage", "About"),
        SettingsPage::Dictation => ("Models", "Runtime", "Dictation  [Selected]", "Storage", "About"),
        SettingsPage::Storage => ("Models", "Runtime", "Dictation", "Storage  [Selected]", "About"),
        SettingsPage::About => ("Models", "Runtime", "Dictation", "Storage", "About  [Selected]"),
    };
    set_control_text(state.nav_models, m);
    set_control_text(state.nav_runtime, r);
    set_control_text(state.nav_dictation, d);
    set_control_text(state.nav_storage, s);
    set_control_text(state.nav_about, a);
}

fn show_page(state: &mut WindowState, page: SettingsPage) {
    state.current_page = page;
    let all_pages = [SettingsPage::Models, SettingsPage::Runtime, SettingsPage::Dictation, SettingsPage::Storage, SettingsPage::About];
    for p in all_pages {
        let visible = p == page;
        let controls = state.page_controls(p).clone();
        for h in controls {
            unsafe { let _ = ShowWindow(h, if visible { SW_SHOW } else { SW_HIDE }); }
        }
    }
    set_active_nav(state);
}
fn model_profile_hints(name: &str) -> (&'static str, &'static str) {
    let lower = name.to_ascii_lowercase();
    if lower.contains("tiny") { ("Speed 10/10", "Accuracy 5/10") }
    else if lower.contains("base") { ("Speed 8/10", "Accuracy 7/10") }
    else if lower.contains("small") { ("Speed 6/10", "Accuracy 8/10") }
    else if lower.contains("medium") { ("Speed 4/10", "Accuracy 9/10") }
    else if lower.contains("large") { ("Speed 2/10", "Accuracy 10/10") }
    else { ("Speed --", "Accuracy --") }
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
                let active = if m.is_active { "  [active]" } else { "" };
                let label = format!("{}{}{}\0", m.name, m.size_label, active);
                let wide: Vec<u16> = label.encode_utf16().collect();
                SendMessageW(hwnd_list, LB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize));
            }
        }
    }
}

fn populate_download_list(state: &WindowState) {
    unsafe {
        SendMessageW(state.hwnd_download_list, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for m in crate::model_downloader::get_downloadable_models() {
            let installed = state.params.shared_models_dir.join(&m.filename).exists();
            let marker = if installed { "  [installed]" } else { "" };
            let label = format!("{}  |  {} MB{}\0", m.name, m.size_mb, marker);
            let wide: Vec<u16> = label.encode_utf16().collect();
            SendMessageW(state.hwnd_download_list, LB_ADDSTRING, WPARAM(0), LPARAM(wide.as_ptr() as isize));
        }
    }
}

fn populate_model_details(state: &WindowState) {
    let sel = unsafe { SendMessageW(state.hwnd_model_list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    if sel < 0 {
        set_control_text(state.hwnd_model_details, "Select an installed model to view details.");
        return;
    }
    let models = (state.params.get_models)();
    let Some(m) = models.get(sel as usize) else { return; };
    let (speed, acc) = model_profile_hints(&m.name);
    let details = format!(
        "Name: {}\r\nSize: {}\r\n{}\r\n{}\r\nBest for: {}",
        m.name,
        if m.size_label.is_empty() { "unknown".to_string() } else { m.size_label.clone() },
        speed,
        acc,
        if m.is_active { "current dictation model" } else { "manual switch available" }
    );
    set_control_text(state.hwnd_model_details, &details);
}

fn populate_download_details(state: &WindowState) {
    let sel = unsafe { SendMessageW(state.hwnd_download_list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    if sel < 0 {
        set_control_text(state.hwnd_download_details, "Select a catalog model to view download details.");
        return;
    }
    let Some(m) = crate::model_downloader::get_downloadable_models().into_iter().nth(sel as usize) else { return; };
    let (speed, acc) = model_profile_hints(&m.name);
    let installed = state.params.shared_models_dir.join(&m.filename).exists();
    let details = format!(
        "Name: {}\r\nFile: {}\r\nSize: {} MB\r\n{}\r\n{}\r\nStatus: {}",
        m.name, m.filename, m.size_mb, speed, acc,
        if installed { "already installed" } else { "not installed" }
    );
    set_control_text(state.hwnd_download_details, &details);
}

fn populate_download_status(state: &mut WindowState) {
    let ds = (state.params.get_download_status)();
    let progress_pos = (ds.progress.clamp(0.0, 1.0) * 100.0).round() as isize;
    unsafe {
        SendMessageW(state.hwnd_download_progress, PBM_SETPOS, WPARAM(progress_pos as usize), LPARAM(0));
        let _ = ShowWindow(state.hwnd_download_progress, if ds.active { SW_SHOW } else { SW_HIDE });
    }
    let status = if ds.active {
        let eta = ds.eta_seconds.map(|s| format!("{}s", s)).unwrap_or_else(|| "--".to_string());
        format!("Downloading {}: {:.0}% ({:.1}/{:.1} MB) | {:.2} MB/s | ETA {}", ds.model_name, ds.progress * 100.0, ds.downloaded_mb, ds.total_mb, ds.speed_mbps, eta)
    } else if let Some(err) = ds.error { format!("Download error: {}", err) }
      else if ds.completed { format!("Download complete: {}", ds.model_name) }
      else { "Download: idle".to_string() };
    set_control_text(state.hwnd_download_status, &status);
    if ds.completed && !state.last_download_completed {
        populate_model_list(state.hwnd_model_list, &state.params);
        populate_download_list(state);
    }
    state.last_download_completed = ds.completed;
}

fn runtime_recommendation_text(rs: &RuntimeStatus) -> String {
    let recommendation = if rs.device.to_ascii_lowercase().contains("gpu") {
        "Recommended: high-accuracy model (Base/Small or higher), real-time dictation should be stable."
    } else if rs.device.to_ascii_lowercase().contains("cpu") {
        "Recommended: balanced model (Tiny/Base). Prioritize stability over maximum accuracy."
    } else {
        "Recommended: Auto mode with fallback chain enabled."
    };
    format!(
        "System profile: backend={} on {}.\r\n{}\r\nIf speed is low, use a smaller model. If quality is low, use a larger model.",
        rs.backend, rs.device, recommendation
    )
}

fn populate_runtime_panel(state: &WindowState) {
    let rs = (state.params.get_runtime_status)();
    set_control_text(state.hwnd_runtime_explain, &runtime_recommendation_text(&rs));
    let details = format!(
        "Backend: {}\r\nDevice: {}\r\nPreferred model: {}\r\nFallback chain: {}\r\nServer fallback: {}\r\nCloud fallback: {}\r\nLast stage: {}\r\n\r\nHardware summary:\r\n{}",
        rs.backend, rs.device, rs.preferred_model, rs.fallback_chain,
        if rs.server_fallback { "enabled" } else { "disabled" },
        if rs.cloud_fallback { "planned" } else { "disabled" },
        rs.last_stage, rs.hardware_summary,
    );
    set_control_text(state.hwnd_runtime_status, &details);
}

fn populate_storage_panel(state: &WindowState) {
    let rs = (state.params.get_runtime_status)();
    let text = format!(
        "Shared models directory:\r\n{}\r\n\r\nShared store metadata:\r\n{}\r\n\r\n{}\r\n\r\nHistory retention: {} days",
        state.params.shared_models_dir.display(),
        state.params.shared_store_path.display(),
        rs.storage_summary,
        state.params.history_retention_days,
    );
    set_control_text(state.hwnd_storage_info, &text);
}

fn populate_about_panel(state: &WindowState) {
    let text = format!(
        "Dictator for Windows\r\nVersion: 0.3.x alpha\r\n\r\nHotkey profile:\r\n{}\r\n\r\nHistory: {}\r\n\r\nAll settings are applied instantly.",
        state.params.hotkey_summary,
        if state.params.history_enabled { "enabled" } else { "disabled" },
    );
    set_control_text(state.hwnd_about_info, &text);
}

fn apply_settings(state: &WindowState) {
    (state.params.on_save)(collect_settings(state));
}

fn handle_command(hwnd: HWND, id: usize, notify: u16, state: &mut WindowState) {
    match id {
        IDC_NAV_MODELS => show_page(state, SettingsPage::Models),
        IDC_NAV_RUNTIME => show_page(state, SettingsPage::Runtime),
        IDC_NAV_DICTATION => show_page(state, SettingsPage::Dictation),
        IDC_NAV_STORAGE => show_page(state, SettingsPage::Storage),
        IDC_NAV_ABOUT => show_page(state, SettingsPage::About),
        IDC_MODEL_LIST if notify == CBN_SELCHANGE_U16 => populate_model_details(state),
        IDC_DOWNLOAD_LIST if notify == CBN_SELCHANGE_U16 => populate_download_details(state),
        IDC_BTN_USE_MODEL => {
            let sel = unsafe { SendMessageW(state.hwnd_model_list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
            if sel >= 0 {
                let models = (state.params.get_models)();
                if let Some(m) = models.get(sel as usize) {
                    (state.params.on_use_model)(m.path.clone());
                    populate_model_list(state.hwnd_model_list, &state.params);
                    populate_model_details(state);
                }
            }
        }
        IDC_BTN_DELETE_MODEL => {
            let sel = unsafe { SendMessageW(state.hwnd_model_list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
            if sel >= 0 {
                let models = (state.params.get_models)();
                if let Some(m) = models.get(sel as usize) {
                    if m.is_active {
                        unsafe { MessageBoxW(hwnd, w!("Cannot delete active model."), w!("Dictator"), MB_OK | MB_ICONERROR); }
                        return;
                    }
                    unsafe {
                        let msg: Vec<u16> = format!("Delete {}?\0", m.name).encode_utf16().collect();
                        if MessageBoxW(hwnd, windows::core::PCWSTR(msg.as_ptr()), w!("Confirm delete"), MB_YESNO | MB_ICONINFORMATION) == IDYES {
                            (state.params.on_delete_model)(m.path.clone());
                            populate_model_list(state.hwnd_model_list, &state.params);
                            populate_download_list(state);
                            populate_model_details(state);
                        }
                    }
                }
            }
        }
        IDC_BTN_DOWNLOAD => {
            let sel = unsafe { SendMessageW(state.hwnd_download_list, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
            if sel >= 0 { (state.params.on_download_model)(sel as usize); state.last_download_completed = false; }
        }
        IDC_BTN_REFRESH_RUNTIME => { populate_runtime_panel(state); populate_storage_panel(state); populate_about_panel(state); }
        IDC_CMB_INJECTION | IDC_CMB_RUNTIME_MODE if notify == CBN_SELCHANGE_U16 => apply_settings(state),
        IDC_CHK_LLM => apply_settings(state),
        IDC_EDIT_OLLAMA_URL | IDC_EDIT_OLLAMA_MODEL | IDC_EDIT_IDLE if notify == EN_KILLFOCUS_U16 => apply_settings(state),
        IDC_BTN_OPEN_SHARED_MODELS => { let _ = std::process::Command::new("explorer").arg(&state.params.shared_models_dir).spawn(); }
        IDC_BTN_OPEN_SHARED_STORE => { let _ = std::process::Command::new("notepad").arg(&state.params.shared_store_path).spawn(); }
        IDC_BTN_OPEN_HISTORY => { let _ = std::process::Command::new("explorer").arg(dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("dictator").join("recordings")).spawn(); }
        IDC_BTN_OPEN_LOGS => { let _ = std::process::Command::new("explorer").arg(&state.params.log_dir).spawn(); }
        IDC_BTN_OPEN_CONFIG => { let _ = std::process::Command::new("notepad").arg(&state.params.config_path).spawn(); }
        IDC_BTN_CLOSE => unsafe { let _ = DestroyWindow(hwnd); },
        _ => {}
    }
}

fn collect_settings(state: &WindowState) -> SavedSettings {
    let injection_method = unsafe {
        match SendMessageW(state.hwnd_cmb_injection, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 {
            1 => "clipboard".to_string(),
            2 => "clipboard_enter".to_string(),
            _ => "direct".to_string(),
        }
    };
    let runtime_mode = unsafe {
        match SendMessageW(state.hwnd_cmb_runtime_mode, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 {
            1 => "force_gpu".to_string(),
            2 => "force_cpu".to_string(),
            _ => "auto".to_string(),
        }
    };
    let llm_enabled = unsafe { SendMessageW(state.hwnd_chk_llm, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 != 0 };
    SavedSettings {
        injection_method,
        llm_enabled,
        ollama_url: read_edit(state.hwnd_edit_ollama_url),
        ollama_model: read_edit(state.hwnd_edit_ollama_model),
        idle_unload_minutes: read_edit(state.hwnd_edit_idle).trim().parse().unwrap_or(5),
        runtime_mode,
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




