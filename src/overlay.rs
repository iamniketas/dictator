// AGENT: glm | TASK: task_fcb673d2a4fa | TIMESTAMP: 2026-01-29T18:51:37.646792
// AUTO-GENERATED: Do not edit manually. Delegate changes via orchestrator.
// SOURCE: http://localhost:8000/task/task_fcb673d2a4fa/report

use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::thread;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowAttributes, WindowId, WindowLevel},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::ffi::c_void;
use windows::{
    Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM, POINT},
    Win32::Graphics::Gdi::*,
    Win32::UI::WindowsAndMessaging::*,
};

pub struct OverlayConfig {
    pub width: u32,
    pub height: u32,
    pub font_size: f32,
    pub text_color: (u8, u8, u8),
    pub bg_color: (u8, u8, u8, u8),
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            width: 400,
            height: 100,
            font_size: 24.0,
            text_color: (255, 255, 255),
            bg_color: (0, 0, 0, 180),
        }
    }
}

struct OverlayState {
    text: String,
    visible: bool,
    config: OverlayConfig,
    position: (i32, i32),
}

pub struct OverlayWindow {
    state: Arc<Mutex<OverlayState>>,
    event_loop_proxy: Option<winit::event_loop::EventLoopProxy<OverlayCommand>>,
    window_thread: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
enum OverlayCommand {
    Show(String),
    Hide,
    SetPosition(i32, i32),
    SetText(String),
    Shutdown,
}

struct OverlayApp {
    state: Arc<Mutex<OverlayState>>,
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    context: Option<Context<Arc<Window>>>,
}

impl OverlayApp {
    fn new(state: Arc<Mutex<OverlayState>>) -> Self {
        Self {
            state,
            window: None,
            surface: None,
            context: None,
        }
    }

    fn render(&mut self) {
        let (width, height, bg_color, text_color, text, visible) = {
            let state = self.state.lock().unwrap();
            (
                state.config.width,
                state.config.height,
                state.config.bg_color,
                state.config.text_color,
                state.text.clone(),
                state.visible,
            )
        };

        let Some(surface) = self.surface.as_mut() else { return };
        
        if width == 0 || height == 0 {
            return;
        }

        let Ok(mut buffer) = surface.buffer_mut() else { return };
        
        let (r, g, b, a) = bg_color;
        let bg = ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        
        for pixel in buffer.iter_mut() {
            *pixel = bg;
        }

        if visible && !text.is_empty() {
            render_text_to_buffer(&mut buffer, width, height, &text, text_color);
        }

        let _ = buffer.present();
    }
}

fn render_text_to_buffer(buffer: &mut [u32], width: u32, height: u32, text: &str, color: (u8, u8, u8)) {
    // Text rendering disabled - simplified overlay
    // Future: implement proper text rendering with compatible font library
    let _ = (buffer, width, height, text, color);
}

impl ApplicationHandler<OverlayCommand> for OverlayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let state = self.state.lock().unwrap();
            let attrs = WindowAttributes::default()
                .with_title("Dictator Overlay")
                .with_inner_size(winit::dpi::LogicalSize::new(state.config.width, state.config.height))
                .with_position(winit::dpi::LogicalPosition::new(state.position.0, state.position.1))
                .with_decorations(false)
                .with_transparent(true)
                .with_visible(false);
            
            drop(state);
            
            let window = event_loop.create_window(attrs).unwrap();
            window.set_window_level(WindowLevel::AlwaysOnTop);
            
            let window = Arc::new(window);
            
            #[cfg(target_os = "windows")]
            unsafe {
                let hwnd = match window.window_handle() {
                    Ok(handle) => {
                        match handle.as_raw() {
                            RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut c_void),
                            _ => panic!("Not Windows platform"),
                        }
                    },
                    Err(e) => panic!("Failed to get window handle: {}", e),
                };
                let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                SetWindowLongW(hwnd, GWL_EXSTYLE, (ex_style | WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as i32);
                SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_COLORKEY);
            }
            
            let context = Context::new(window.clone()).unwrap();
            let mut surface = Surface::new(&context, window.clone()).unwrap();
            
            let state = self.state.lock().unwrap();
            surface.resize(NonZeroU32::new(state.config.width).unwrap(), NonZeroU32::new(state.config.height).unwrap()).unwrap();
            drop(state);
            
            self.window = Some(window);
            self.surface = Some(surface);
            self.context = Some(context);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                self.render();
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: OverlayCommand) {
        match event {
            OverlayCommand::Show(text) => {
                let mut state = self.state.lock().unwrap();
                state.text = text;
                state.visible = true;
                drop(state);
                
                if let Some(window) = self.window.as_ref() {
                    window.set_visible(true);
                    window.request_redraw();
                }
            }
            OverlayCommand::Hide => {
                let mut state = self.state.lock().unwrap();
                state.visible = false;
                drop(state);
                
                if let Some(window) = self.window.as_ref() {
                    window.set_visible(false);
                }
            }
            OverlayCommand::SetPosition(x, y) => {
                let mut state = self.state.lock().unwrap();
                state.position = (x, y);
                drop(state);
                
                if let Some(window) = self.window.as_ref() {
                    window.set_outer_position(winit::dpi::LogicalPosition::new(x, y));
                }
            }
            OverlayCommand::SetText(text) => {
                let mut state = self.state.lock().unwrap();
                state.text = text;
                drop(state);
                
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            OverlayCommand::Shutdown => {
                event_loop.exit();
            }
        }
    }
}

impl OverlayWindow {
    pub fn new(config: OverlayConfig) -> Result<Self> {
        let state = Arc::new(Mutex::new(OverlayState {
            text: String::new(),
            visible: false,
            config,
            position: (100, 100),
        }));

        let state_clone = state.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        
        let window_thread = thread::spawn(move || {
            let event_loop = EventLoop::<OverlayCommand>::with_user_event().build().unwrap();
            let proxy = event_loop.create_proxy();
            let _ = tx.send(proxy);
            
            event_loop.set_control_flow(ControlFlow::Wait);
            
            let mut app = OverlayApp::new(state_clone);
            let _ = event_loop.run_app(&mut app);
        });

        let event_loop_proxy = rx.recv().map_err(|e| anyhow::anyhow!("Failed to get event loop proxy: {}", e))?;

        Ok(Self {
            state,
            event_loop_proxy: Some(event_loop_proxy),
            window_thread: Some(window_thread),
        })
    }

    pub fn show(&self, text: &str) {
        if let Some(proxy) = self.event_loop_proxy.as_ref() {
            let _ = proxy.send_event(OverlayCommand::Show(text.to_string()));
        }
    }

    pub fn hide(&self) {
        if let Some(proxy) = self.event_loop_proxy.as_ref() {
            let _ = proxy.send_event(OverlayCommand::Hide);
        }
    }

    pub fn set_text(&self, text: &str) {
        if let Some(proxy) = self.event_loop_proxy.as_ref() {
            let _ = proxy.send_event(OverlayCommand::SetText(text.to_string()));
        }
    }

    pub fn position_near_cursor(&self) {
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            if GetCursorPos(&mut point).is_ok() {
                if let Some(proxy) = self.event_loop_proxy.as_ref() {
                    let _ = proxy.send_event(OverlayCommand::SetPosition(point.x, point.y + 20));
                }
            }
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        if let Some(proxy) = self.event_loop_proxy.take() {
            let _ = proxy.send_event(OverlayCommand::Shutdown);
        }
        if let Some(handle) = self.window_thread.take() {
            let _ = handle.join();
        }
    }
}