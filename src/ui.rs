//! UI module - System tray and overlay windows

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GetCursorPos, GetMessageW, LoadIconW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
    TrackPopupMenu, CS_HREDRAW, CS_VREDRAW, IDI_APPLICATION, MF_STRING, MSG, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, WM_COMMAND, WM_DESTROY, WM_RBUTTONUP, WM_USER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const WM_TRAYICON: u32 = WM_USER + 1;
const ID_EXIT: u16 = 1001;

static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);

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
            hbrBackground: HBRUSH(0 as *mut _),
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

        // Add tray icon
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
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

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = DispatchMessageW(&msg);
        }

        // Cleanup
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);

        Ok(())
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Rust 2024 requires explicit unsafe blocks inside unsafe fn
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
    // Rust 2024 requires explicit unsafe blocks inside unsafe fn
    unsafe {
        if let Ok(menu) = CreatePopupMenu() {
            let _ = AppendMenuW(menu, MF_STRING, ID_EXIT as usize, w!("Exit"));

            let mut pt = Default::default();
            let _ = GetCursorPos(&mut pt);

            let _ = SetForegroundWindow(hwnd);
            let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
        }
    }
}
