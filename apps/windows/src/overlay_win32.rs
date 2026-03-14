use anyhow::Result;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use windows::{
    Win32::Foundation::{COLORREF, HWND, POINT, RECT},
    Win32::Graphics::Gdi::*,
    Win32::UI::WindowsAndMessaging::*,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes, WindowId, WindowLevel},
};

#[derive(Clone)]
pub struct OverlayConfig {
    pub width: u32,
    pub height: u32,
    pub font_size: i32,
    pub text_color: (u8, u8, u8),
    pub bg_color: (u8, u8, u8, u8),
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            width: 900,
            height: 120,
            font_size: 24,
            text_color: (245, 245, 245), // Higher contrast text
            bg_color: (28, 28, 30, 245), // Nearly opaque dark background
        }
    }
}

struct OverlayState {
    status_text: String,
    body_text: String,
    visible: bool,
    config: OverlayConfig,
    position: (i32, i32),
    is_recording: bool,
    recording_frame: u32,
    /// Rolling amplitude history for waveform display (newest at back)
    waveform_bars: Vec<f32>,
}

pub struct OverlayWindow {
    state: Arc<Mutex<OverlayState>>,
    event_loop_proxy: Option<winit::event_loop::EventLoopProxy<OverlayCommand>>,
    window_thread: Option<thread::JoinHandle<()>>,
    stop_animation: Arc<Mutex<bool>>,
    animation_thread: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

#[derive(Clone)]
enum OverlayCommand {
    Show(String),
    Hide,
    SetRecording(bool),
    UpdateStatusText(String),
    UpdateBodyText(String),
    AnimationTick,
    WaveformUpdate(f32),
    SetPosition(i32, i32),
    Shutdown,
}

struct OverlayApp {
    state: Arc<Mutex<OverlayState>>,
    window: Option<Arc<Window>>,
    hwnd: Option<HWND>,
    font: Option<HFONT>,
    rec_font: Option<HFONT>,
    is_dragging: bool,
    drag_start: (i32, i32),
}

impl OverlayApp {
    fn new(state: Arc<Mutex<OverlayState>>) -> Self {
        Self {
            state,
            window: None,
            hwnd: None,
            font: None,
            rec_font: None,
            is_dragging: false,
            drag_start: (0, 0),
        }
    }

    fn create_fonts(&mut self, base_height: i32) {
        unsafe {
            // Create Cascadia Code fonts
            // Main text: 4 sizes smaller than base (e.g., 28 -> 20)
            // REC text: 2 sizes smaller than main (e.g., 20 -> 16)
            let main_height = base_height - 4;
            let rec_height = main_height - 2;

            let font_name: Vec<u16> = "Cascadia Code\0".encode_utf16().collect();

            let main_font = CreateFontW(
                main_height,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                DEFAULT_QUALITY.0 as u32,
                (VARIABLE_PITCH.0 | FF_SWISS.0) as u32,
                windows::core::PCWSTR(font_name.as_ptr()),
            );

            let rec_font = CreateFontW(
                rec_height,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                DEFAULT_QUALITY.0 as u32,
                (VARIABLE_PITCH.0 | FF_SWISS.0) as u32,
                windows::core::PCWSTR(font_name.as_ptr()),
            );

            if !main_font.0.is_null() {
                self.font = Some(main_font);
            }
            if !rec_font.0.is_null() {
                self.rec_font = Some(rec_font);
            }
        }
    }

    fn render(&mut self) {
        let (status_text, body_text, visible, config, is_recording, waveform_bars) = {
            let state = self.state.lock().unwrap();
            (
                state.status_text.clone(),
                state.body_text.clone(),
                state.visible,
                state.config.clone(),
                state.is_recording,
                state.waveform_bars.clone(),
            )
        };

        if !visible {
            return;
        }

        if let Some(hwnd) = self.hwnd {
            unsafe {
                let hdc = GetDC(hwnd);
                if hdc.0.is_null() {
                    return;
                }

                let mem_dc = CreateCompatibleDC(hdc);
                let rect = RECT {
                    left: 0,
                    top: 0,
                    right: config.width as i32,
                    bottom: config.height as i32,
                };

                let bmp = CreateCompatibleBitmap(hdc, config.width as i32, config.height as i32);
                let old_bmp = SelectObject(mem_dc, bmp);

                let (r, g, b, _a) = config.bg_color;
                let brush =
                    CreateSolidBrush(COLORREF((r as u32) << 16 | (g as u32) << 8 | b as u32));
                FillRect(mem_dc, &rect, brush);
                let _ = DeleteObject(brush);

                // Border improves readability on bright backgrounds.
                let border_pen = CreatePen(PS_SOLID, 1, COLORREF(0x005A5A5A));
                let old_pen = SelectObject(mem_dc, border_pen);
                let old_brush = SelectObject(mem_dc, GetStockObject(HOLLOW_BRUSH));
                let _ = Rectangle(mem_dc, 0, 0, config.width as i32, config.height as i32);
                SelectObject(mem_dc, old_pen);
                SelectObject(mem_dc, old_brush);
                let _ = DeleteObject(border_pen);

                if self.font.is_none() || self.rec_font.is_none() {
                    self.create_fonts(config.font_size);
                }

                let (tr, tg, tb) = config.text_color;
                SetTextColor(
                    mem_dc,
                    COLORREF((tr as u32) << 16 | (tg as u32) << 8 | tb as u32),
                );
                SetBkMode(mem_dc, TRANSPARENT);

                if is_recording {
                    // Draw waveform bars (20 bars, 2px wide, 1px gap)
                    const BAR_WIDTH: i32 = 2;
                    const BAR_GAP: i32 = 1;
                    const BAR_STEP: i32 = BAR_WIDTH + BAR_GAP;
                    const X_START: i32 = 8;

                    let center_y = config.height as i32 / 2;
                    let max_half = (center_y - 8).max(4);

                    let bar_brush = CreateSolidBrush(COLORREF(0x0000FF)); // red
                    let null_pen = GetStockObject(NULL_PEN);
                    let old_bar_brush = SelectObject(mem_dc, bar_brush);
                    let old_bar_pen = SelectObject(mem_dc, null_pen);

                    for (i, &amp) in waveform_bars.iter().enumerate() {
                        let x = X_START + i as i32 * BAR_STEP;
                        let scaled = (amp * 6.0).min(1.0_f32);
                        let bar_half = ((scaled * max_half as f32) as i32).max(2);
                        let _ = Rectangle(
                            mem_dc,
                            x,
                            center_y - bar_half,
                            x + BAR_WIDTH,
                            center_y + bar_half,
                        );
                    }

                    SelectObject(mem_dc, old_bar_pen);
                    SelectObject(mem_dc, old_bar_brush);
                    let _ = DeleteObject(bar_brush);

                    if let Some(main_font) = self.font {
                        SelectObject(mem_dc, main_font);
                    }
                } else if let Some(main_font) = self.font {
                    SelectObject(mem_dc, main_font);
                }

                let left_margin = if is_recording { 75 } else { 20 };

                if !status_text.is_empty() {
                    if let Some(rec_font) = self.rec_font {
                        SelectObject(mem_dc, rec_font);
                    }

                    let mut status_utf16: Vec<u16> = status_text.encode_utf16().collect();
                    let mut status_rect = RECT {
                        left: left_margin,
                        top: 8,
                        right: config.width as i32 - 10,
                        bottom: 38,
                    };
                    DrawTextW(
                        mem_dc,
                        &mut status_utf16,
                        &mut status_rect,
                        DT_LEFT | DT_TOP | DT_WORDBREAK,
                    );

                    if let Some(main_font) = self.font {
                        SelectObject(mem_dc, main_font);
                    }
                }

                if !body_text.is_empty() {
                    const MAX_LINES: usize = 3;
                    const MAX_CHARS_PER_LINE: usize = 75;
                    let visible_text = Self::get_last_lines(&body_text, MAX_LINES, MAX_CHARS_PER_LINE);

                    let mut body_utf16: Vec<u16> = visible_text.encode_utf16().collect();
                    let mut body_rect = RECT {
                        left: left_margin,
                        top: if status_text.is_empty() { 10 } else { 42 },
                        right: config.width as i32 - 10,
                        bottom: config.height as i32 - 10,
                    };
                    DrawTextW(mem_dc, &mut body_utf16, &mut body_rect, DT_LEFT | DT_TOP | DT_WORDBREAK);
                }

                let _ = BitBlt(
                    hdc,
                    0,
                    0,
                    config.width as i32,
                    config.height as i32,
                    mem_dc,
                    0,
                    0,
                    SRCCOPY,
                );

                SelectObject(mem_dc, old_bmp);
                let _ = DeleteObject(bmp);
                let _ = DeleteDC(mem_dc);
                ReleaseDC(hwnd, hdc);
            }
        }
    }

    fn update_position(&mut self, x: i32, y: i32) {
        if let Some(window) = self.window.as_ref() {
            window.set_outer_position(winit::dpi::LogicalPosition::new(x, y));
        }
    }

    /// Get last N lines of text, wrapping long lines to fit width
    fn get_last_lines(text: &str, max_lines: usize, max_chars: usize) -> String {
        if text.is_empty() || max_lines == 0 {
            return text.to_string();
        }
        
        // Split text into words and wrap to lines
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut lines: Vec<String> = Vec::new();
        let mut current_line = String::new();
        
        for word in words {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.len() + 1 + word.len() <= max_chars {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                // Line full, start new one
                lines.push(current_line);
                current_line = word.to_string();
            }
        }
        
        // Don't forget last line
        if !current_line.is_empty() {
            lines.push(current_line);
        }
        
        // Keep only last max_lines
        if lines.len() <= max_lines {
            lines.join("\n")
        } else {
            lines[lines.len() - max_lines..].join("\n")
        }
    }
}

impl ApplicationHandler<OverlayCommand> for OverlayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let state = self.state.lock().unwrap();
            let attrs = WindowAttributes::default()
                .with_title("Dictator Overlay")
                .with_inner_size(winit::dpi::LogicalSize::new(
                    state.config.width,
                    state.config.height,
                ))
                .with_position(winit::dpi::LogicalPosition::new(
                    state.position.0,
                    state.position.1,
                ))
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
                    Ok(handle) => match handle.as_raw() {
                        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut c_void),
                        _ => panic!("Not Windows platform"),
                    },
                    Err(e) => panic!("Failed to get window handle: {}", e),
                };

                self.hwnd = Some(hwnd);

                // Set layered window with full transparency support
                let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                SetWindowLongW(
                    hwnd,
                    GWL_EXSTYLE,
                    (ex_style
                        | WS_EX_LAYERED.0
                        | WS_EX_TRANSPARENT.0
                        | WS_EX_TOOLWINDOW.0
                        | WS_EX_NOACTIVATE.0) as i32,
                );

                // Use alpha blending and get dimensions for rounded corners
                let state = self.state.lock().unwrap();
                let (_, _, _, a) = state.config.bg_color;
                let width = state.config.width;
                let height = state.config.height;
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), a, LWA_ALPHA);
                drop(state);

                // Keep hidden on startup to avoid any first-frame flicker.
                let _ = ShowWindow(hwnd, SW_HIDE);
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_HIDEWINDOW,
                );

                // Set rounded corners (12px radius)
                let rgn = CreateRoundRectRgn(0, 0, width as i32, height as i32, 12, 12);
                SetWindowRgn(hwnd, rgn, true);
            }

            self.window = Some(window);
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                if button == winit::event::MouseButton::Left {
                    self.is_dragging = true;
                    unsafe {
                        let mut point = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut point).ok();
                        self.drag_start = (point.x, point.y);
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button,
                ..
            } => {
                if button == winit::event::MouseButton::Left {
                    self.is_dragging = false;
                }
            }
            WindowEvent::CursorMoved { position: _, .. } => {
                if self.is_dragging {
                    unsafe {
                        let mut point = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut point).ok();
                        let dx = point.x - self.drag_start.0;
                        let dy = point.y - self.drag_start.1;

                        if let Some(window) = self.window.as_ref() {
                            let pos = window.outer_position();
                            if let Ok(pos) = pos {
                                let new_x = pos.x + dx;
                                let new_y = pos.y + dy;
                                window.set_outer_position(winit::dpi::LogicalPosition::new(
                                    new_x, new_y,
                                ));

                                let mut state = self.state.lock().unwrap();
                                state.position = (new_x, new_y);
                            }
                        }

                        self.drag_start = (point.x, point.y);
                    }
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: OverlayCommand) {
        match event {
            OverlayCommand::Show(text) => {
                let mut state = self.state.lock().unwrap();
                state.status_text.clear();
                state.body_text = text;
                state.visible = true;
                state.is_recording = false;
                drop(state);

                if let Some(window) = self.window.as_ref() {
                    window.set_visible(true);
                    window.request_redraw();
                }
                if let Some(hwnd) = self.hwnd {
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        let _ = SetWindowPos(
                            hwnd,
                            HWND_TOPMOST,
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    }
                }
            }
            OverlayCommand::Hide => {
                let mut state = self.state.lock().unwrap();
                state.visible = false;
                state.is_recording = false;
                drop(state);

                if let Some(window) = self.window.as_ref() {
                    window.set_visible(false);
                }
                if let Some(hwnd) = self.hwnd {
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    }
                }
            }
            OverlayCommand::SetRecording(recording) => {
                let mut state = self.state.lock().unwrap();
                state.is_recording = recording;
                state.recording_frame = 0;
                if recording {
                    state.visible = true;
                    state.status_text.clear();
                    state.body_text.clear();
                    // Pre-fill with silence so bars appear immediately
                    state.waveform_bars = vec![0.0f32; 20];
                } else {
                    state.waveform_bars.clear();
                }
                drop(state);

                if let Some(window) = self.window.as_ref() {
                    if recording {
                        window.set_visible(true);
                    }
                    window.request_redraw();
                }
                if let Some(hwnd) = self.hwnd {
                    unsafe {
                        if recording {
                            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                            let _ = SetWindowPos(
                                hwnd,
                                HWND_TOPMOST,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                            );
                        } else {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                    }
                }
            }
            OverlayCommand::AnimationTick => {
                let mut state = self.state.lock().unwrap();
                if state.is_recording {
                    state.recording_frame = state.recording_frame.wrapping_add(1);
                }
                let needs_redraw = state.is_recording;
                drop(state);

                if needs_redraw
                    && let Some(window) = self.window.as_ref()
                {
                    window.request_redraw();
                }
            }
            OverlayCommand::SetPosition(x, y) => {
                self.update_position(x, y);
                let mut state = self.state.lock().unwrap();
                state.position = (x, y);
            }
            OverlayCommand::UpdateStatusText(text) => {
                let mut state = self.state.lock().unwrap();
                state.status_text = text;
                state.visible = true;
                drop(state);

                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            OverlayCommand::UpdateBodyText(text) => {
                let mut state = self.state.lock().unwrap();
                state.body_text = text;
                state.visible = true;
                drop(state);

                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            OverlayCommand::WaveformUpdate(amplitude) => {
                let mut state = self.state.lock().unwrap();
                if state.is_recording {
                    state.waveform_bars.push(amplitude);
                    if state.waveform_bars.len() > 20 {
                        state.waveform_bars.remove(0);
                    }
                }
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
            status_text: String::new(),
            body_text: String::new(),
            visible: false,
            config,
            position: (100, 100),
            is_recording: false,
            recording_frame: 0,
            waveform_bars: Vec::new(),
        }));

        let state_clone = state.clone();
        let (tx, rx) = std::sync::mpsc::channel();

        let window_thread = thread::spawn(move || {
            let event_loop = EventLoop::<OverlayCommand>::with_user_event()
                .with_any_thread(true)
                .build()
                .unwrap();
            let proxy = event_loop.create_proxy();
            let _ = tx.send(proxy);

            event_loop.set_control_flow(ControlFlow::Wait);

            let mut app = OverlayApp::new(state_clone);
            let _ = event_loop.run_app(&mut app);
        });

        let event_loop_proxy = rx
            .recv()
            .map_err(|e| anyhow::anyhow!("Failed to get event loop proxy: {}", e))?;

        Ok(Self {
            state,
            event_loop_proxy: Some(event_loop_proxy),
            window_thread: Some(window_thread),
            stop_animation: Arc::new(Mutex::new(false)),
            animation_thread: Arc::new(Mutex::new(None)),
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

    pub fn set_recording(&self, recording: bool) {
        if let Some(proxy) = self.event_loop_proxy.as_ref() {
            let _ = proxy.send_event(OverlayCommand::SetRecording(recording));

            // Start/stop animation thread
            if recording {
                // Stop any existing animation
                if let Ok(mut stop) = self.stop_animation.lock() {
                    *stop = true;
                }

                // Wait for previous animation thread to finish
                if let Ok(mut anim_thread) = self.animation_thread.lock()
                    && let Some(handle) = anim_thread.take()
                {
                    let _ = handle.join();
                }

                // Start new animation thread
                let proxy = proxy.clone();
                let stop_flag = Arc::clone(&self.stop_animation);
                let anim_thread = Arc::clone(&self.animation_thread);

                // Reset stop flag
                if let Ok(mut stop) = stop_flag.lock() {
                    *stop = false;
                }

                let handle = thread::spawn(move || {
                    loop {
                        // Check if we should stop
                        if let Ok(stop) = stop_flag.lock()
                            && *stop
                        {
                            break;
                        }

                        // Send animation tick every 1000ms
                        let _ = proxy.send_event(OverlayCommand::AnimationTick);
                        thread::sleep(Duration::from_millis(1000));
                    }
                });

                // Save handle
                if let Ok(mut anim) = anim_thread.lock() {
                    anim.replace(handle);
                }
            } else {
                // Signal animation thread to stop
                if let Ok(mut stop) = self.stop_animation.lock() {
                    *stop = true;
                }

                // Wait for animation thread to finish
                if let Ok(mut anim_thread) = self.animation_thread.lock()
                    && let Some(handle) = anim_thread.take()
                {
                    let _ = handle.join();
                }
            }
        }
    }

    pub fn set_position(&self, x: i32, y: i32) {
        if let Some(proxy) = self.event_loop_proxy.as_ref() {
            let _ = proxy.send_event(OverlayCommand::SetPosition(x, y));
        }
    }

    /// Position overlay above the cursor (centered horizontally)
    pub fn position_near_cursor(&self) {
        unsafe {
            let mut cursor_pos = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut cursor_pos).is_ok() {
                // Get window dimensions from config
                let (width, height) = {
                    let state = self.state.lock().unwrap();
                    (state.config.width as i32, state.config.height as i32)
                };

                // Position window ABOVE the cursor, centered horizontally
                let offset = 10;
                let mut x = cursor_pos.x - width / 2; // Center horizontally
                let mut y = cursor_pos.y - height - offset; // Above cursor

                // Get screen dimensions to ensure window stays on screen
                let screen_width = GetSystemMetrics(SM_CXSCREEN);
                let screen_height = GetSystemMetrics(SM_CYSCREEN);

                // Clamp X to screen bounds
                if x < 0 {
                    x = 10; // Small margin from left edge
                } else if x + width > screen_width {
                    x = screen_width - width - 10; // Small margin from right edge
                }

                // If window would go above the screen, show it BELOW cursor instead
                if y < 0 {
                    y = cursor_pos.y + offset;
                }

                // Ensure Y doesn't go below screen
                if y + height > screen_height {
                    y = screen_height - height - 10;
                }

                self.set_position(x, y);
            }
        }
    }

    /// Update overlay status line (top)
    pub fn update_status_text(&self, text: &str) {
        if let Some(proxy) = self.event_loop_proxy.as_ref() {
            let _ = proxy.send_event(OverlayCommand::UpdateStatusText(text.to_string()));
        }
    }

    /// Update overlay body text area (bottom)
    pub fn update_body_text(&self, text: &str) {
        if let Some(proxy) = self.event_loop_proxy.as_ref() {
            let _ = proxy.send_event(OverlayCommand::UpdateBodyText(text.to_string()));
        }
    }

    /// Push a new amplitude sample to the waveform (call ~10fps while recording)
    pub fn update_waveform(&self, amplitude: f32) {
        if let Some(proxy) = self.event_loop_proxy.as_ref() {
            let _ = proxy.send_event(OverlayCommand::WaveformUpdate(amplitude));
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        // Stop animation thread
        if let Ok(mut stop) = self.stop_animation.lock() {
            *stop = true;
        }

        if let Ok(mut anim_thread) = self.animation_thread.lock()
            && let Some(handle) = anim_thread.take()
        {
            let _ = handle.join();
        }

        if let Some(proxy) = self.event_loop_proxy.take() {
            let _ = proxy.send_event(OverlayCommand::Shutdown);
        }
        if let Some(handle) = self.window_thread.take() {
            let _ = handle.join();
        }
    }
}






