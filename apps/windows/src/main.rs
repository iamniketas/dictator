#![windows_subsystem = "windows"]
//! Dictator - Voice dictation service for Windows

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use dictator::audio::AudioRecorder;
use dictator::config::{Config, WhisperBackend};
use dictator::history::HistoryManager;
use dictator::input::{self, HotkeyEvent};
use dictator::llm::OllamaClient;
use dictator::model_downloader;
use dictator::overlay_win32::{OverlayConfig, OverlayWindow};
use dictator::streaming::{StreamingEvent, StreamingTranscriber};
use dictator::transcribe;
use dictator::ui;
use dictator::settings_window::{self, InstalledModel, SavedSettings, SettingsParams};
use dictator::updater;
use dictator::ui::{DownloadModelItem, ModelMenuItem};
use dictator::whisper_engine::{self, SharedEngine};
use dictator::whisper_server::WhisperServerManager;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForMultipleObjects,
    SYNCHRONIZATION_ACCESS_RIGHTS,
};

/// RAII guard that holds the single-instance named mutex.
/// When dropped (on exit), the mutex is released, allowing a new instance to start.
struct SingleInstanceGuard(windows::Win32::Foundation::HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}

/// Try to open and signal an existing named event (used by CLI --toggle / --stop).
/// Returns true if the event was found and signaled (another instance is running).
fn try_signal_remote(event_name: windows::core::PCWSTR) -> bool {
    // EVENT_MODIFY_STATE = 0x0002
    unsafe {
        // EVENT_MODIFY_STATE = 0x0002
        match OpenEventW(SYNCHRONIZATION_ACCESS_RIGHTS(0x0002), windows::Win32::Foundation::BOOL(0), event_name) {
            Ok(handle) => {
                let _ = SetEvent(handle);
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
}

/// Start IPC listener thread. Creates two named events:
/// - `DictatorToggleEvent` — signals RecordToggle (used by --toggle)
/// - `DictatorStopEvent`  — signals RecordStop   (used by --stop)
fn start_ipc_listener(tx: std::sync::mpsc::Sender<dictator::input::HotkeyEvent>) {
    use dictator::input::HotkeyEvent;
    use windows::core::w;
    use windows::Win32::Foundation::BOOL;

    thread::spawn(move || unsafe {
        let Ok(toggle_ev) = CreateEventW(None, BOOL(0), BOOL(0), w!("DictatorToggleEvent")) else {
            warn!("[IPC] Failed to create DictatorToggleEvent");
            return;
        };
        let Ok(stop_ev) = CreateEventW(None, BOOL(0), BOOL(0), w!("DictatorStopEvent")) else {
            warn!("[IPC] Failed to create DictatorStopEvent");
            let _ = CloseHandle(toggle_ev);
            return;
        };

        info!("[IPC] Listener ready (toggle=DictatorToggleEvent, stop=DictatorStopEvent)");

        let handles = [toggle_ev, stop_ev];
        loop {
            // INFINITE = 0xFFFF_FFFF; WaitForMultipleObjects returns WAIT_EVENT (newtype u32)
            let result = WaitForMultipleObjects(&handles, BOOL(0), 0xFFFF_FFFF_u32);
            match result.0 {
                0 => {
                    info!("[IPC] Remote toggle received");
                    if tx.send(HotkeyEvent::RemoteToggle).is_err() {
                        break; // channel closed (app shutting down)
                    }
                }
                1 => {
                    info!("[IPC] Remote stop received");
                    if tx.send(HotkeyEvent::RemoteStop).is_err() {
                        break;
                    }
                }
                _ => break, // WAIT_FAILED or unexpected
            }
        }

        let _ = CloseHandle(toggle_ev);
        let _ = CloseHandle(stop_ev);
        info!("[IPC] Listener stopped");
    });
}

/// Try to acquire the single-instance mutex.
/// Returns `None` (and shows a message box) if another instance is already running.
fn acquire_single_instance() -> Option<SingleInstanceGuard> {
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, BOOL};
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONINFORMATION, HWND_DESKTOP};
    use windows::core::w;

    unsafe {
        let handle = match CreateMutexW(None, BOOL(1), w!("Global\\DictatorSingleInstance")) {
            Ok(h) => h,
            Err(_) => return None,
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            let _ = MessageBoxW(
                HWND_DESKTOP,
                w!("Dictator is already running.\nCheck the system tray."),
                w!("Dictator"),
                MB_OK | MB_ICONINFORMATION,
            );
            return None;
        }
        Some(SingleInstanceGuard(handle))
    }
}

fn estimate_recording_size_mb(elapsed: Duration) -> f32 {
    let samples = elapsed.as_secs_f32() * 16000.0;
    (samples * std::mem::size_of::<f32>() as f32) / (1024.0 * 1024.0)
}

fn format_recording_status(
    elapsed: Duration,
    streaming_enabled: bool,
    chunk_seconds: u64,
    whisper_status: &str,
) -> String {
    let elapsed_secs = elapsed.as_secs_f32();
    let size_mb = estimate_recording_size_mb(elapsed);
    if streaming_enabled {
        format!(
            "Recording: {:.1}s | ~{:.2} MB | {}\nMode: streaming (chunk {}s)",
            elapsed_secs, size_mb, whisper_status, chunk_seconds
        )
    } else {
        format!(
            "Recording: {:.1}s | ~{:.2} MB | {}\nMode: full transcription",
            elapsed_secs, size_mb, whisper_status
        )
    }
}

fn format_transcribing_status(
    spinner: &str,
    elapsed_secs: f32,
    expected_secs: f32,
    progress: f32,
) -> String {
    let eta = (expected_secs - elapsed_secs).max(0.0);
    format!(
        "Transcribing {} {:.0}%\nElapsed: {:.1}s | ETA: ~{:.1}s",
        spinner,
        progress * 100.0,
        elapsed_secs,
        eta
    )
}

/// Compute a human-readable size label for a model directory (single-level scan).
fn model_dir_size_label(path: &std::path::Path) -> String {
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                }
            }
        }
    }
    if total == 0 {
        return String::new();
    }
    let gb = total as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 0.1 {
        format!(" ({:.1} GB)", gb)
    } else {
        format!(" ({:.0} MB)", total as f64 / (1024.0 * 1024.0))
    }
}

fn scan_available_models(config: &Config, current_path: &std::path::Path) -> Vec<ModelMenuItem> {
    let Some(models_dir) = config.whisper.effective_models_dir() else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(&models_dir) else {
        return Vec::new();
    };

    let current_name = current_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let is_embedded = config.whisper.backend == WhisperBackend::Embedded;

    let mut models: Vec<ModelMenuItem> = entries
        .flatten()
        .filter(|e| {
            let path = e.path();
            if is_embedded {
                // Embedded: GGML .bin files
                path.is_file()
                    && path.extension().map(|ext| ext == "bin").unwrap_or(false)
            } else {
                // Server (legacy): CTranslate2 model directories
                path.is_dir()
            }
        })
        .enumerate()
        .map(|(i, e)| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let is_current = name == current_name;
            let size_label = if is_embedded {
                // For .bin files, use the file size directly
                std::fs::metadata(&path)
                    .map(|m| {
                        let gb = m.len() as f64 / (1024.0 * 1024.0 * 1024.0);
                        if gb >= 0.1 {
                            format!(" ({:.1} GB)", gb)
                        } else {
                            format!(" ({:.0} MB)", m.len() as f64 / (1024.0 * 1024.0))
                        }
                    })
                    .unwrap_or_default()
            } else {
                model_dir_size_label(&path)
            };
            ModelMenuItem { index: i, name, is_current, size_label }
        })
        .collect();

    models.sort_by(|a, b| a.name.cmp(&b.name));
    for (i, m) in models.iter_mut().enumerate() {
        m.index = i;
    }

    models
}

fn main() -> Result<()> {
    // Velopack startup — must be first. May apply a pending update and exit.
    updater::startup();

    // Handle CLI remote-control args before single-instance check.
    // These signal a running instance and exit immediately.
    {
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONINFORMATION, MB_OK,
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        for arg in &args {
            match arg.as_str() {
                "--toggle" => {
                    if !try_signal_remote(w!("DictatorToggleEvent")) {
                        unsafe {
                            let _ = MessageBoxW(
                                windows::Win32::Foundation::HWND(std::ptr::null_mut()),
                                w!("Dictator is not running."),
                                w!("Dictator"),
                                MB_OK | MB_ICONINFORMATION,
                            );
                        }
                    }
                    return Ok(());
                }
                "--stop" => {
                    let _ = try_signal_remote(w!("DictatorStopEvent"));
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    // Enforce single instance — exit early if another Dictator is running
    let _single_instance = match acquire_single_instance() {
        Some(guard) => guard,
        None => return Ok(()),
    };

    // Initialize logging to file
    let log_dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("dictator")
        .join("logs");

    // Create log directory if it doesn't exist
    std::fs::create_dir_all(&log_dir)?;

    let log_file = log_dir.join("dictator.log");
    let file_appender = tracing_appender::rolling::never(&log_dir, "dictator.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_env_filter("dictator=debug")
        .init();

    info!("Dictator starting...");
    info!("Log file location: {:?}", log_file);

    // Load configuration
    let config = Config::load()?;
    info!("Config loaded, hotkey: {:?}", config.hotkey);

    // Sync runtime toggles from config
    ui::set_ollama_enabled(config.ollama.enabled);

    // Always start in full transcription mode (streaming disabled by default).
    ui::set_streaming_enabled(false);
    ui::set_streaming_chunk_seconds(config.streaming.poll_interval);
    info!("[MAIN] Initial streaming state: {}", ui::is_streaming_enabled());
    info!(
        "[MAIN] Initial streaming chunk: {}s",
        ui::streaming_chunk_seconds()
    );

    // Create Ollama client
    let ollama = Arc::new(OllamaClient::new(&config.ollama.url, &config.ollama.model));

    // Log Ollama status
    if config.ollama.enabled {
        info!("Ollama correction enabled ({}) — togglable from tray", config.ollama.url);
    } else {
        info!("Ollama correction disabled (can be enabled from tray menu)");
    }

    // Create embedded whisper engine (lazy — model loads on first transcription)
    let shared_engine: SharedEngine = whisper_engine::new_shared_engine();

    // Shared active model path — updated at runtime when user switches or downloads a model.
    // The event thread reads from this, so model changes take effect without restarting.
    let active_model_path = Arc::new(RwLock::new(config.whisper.model_path.clone()));

    // Create shared audio recorder
    let recorder = Arc::new(AudioRecorder::new()?);

    // Create overlay window
    let overlay_config = OverlayConfig::default();
    let overlay = Arc::new(OverlayWindow::new(overlay_config)?);

    // Create history manager
    let history = Arc::new(HistoryManager::new(config.history.retention_days)?);
    info!("[MAIN] History manager created, enabled: {}", config.history.enabled);

    // Set up history callbacks for tray menu
    let history_for_open = history.clone();
    ui::set_history_open_callback(move || {
        if let Err(e) = history_for_open.open_folder() {
            error!("[MAIN] Failed to open history folder: {}", e);
        }
    });

    // Callback to get recent recordings for menu
    let history_for_entries = history.clone();
    ui::set_history_entries_callback(move || {
        let recordings = history_for_entries.get_recent_recordings(10);
        recordings
            .into_iter()
            .enumerate()
            .map(|(idx, rec)| {
                let time = &rec.metadata.datetime[11..16]; // HH:MM
                let preview = if rec.metadata.text_preview.len() > 35 {
                    format!("{}...", &rec.metadata.text_preview[..35])
                } else {
                    rec.metadata.text_preview.clone()
                };
                ui::HistoryMenuEntry {
                    id: idx,
                    label: format!("[{}] {}", time, preview),
                }
            })
            .collect()
    });

    // Callback to copy recording to clipboard
    let history_for_copy = history.clone();
    ui::set_history_copy_callback(move |index| {
        let recordings = history_for_copy.get_recent_recordings(10);
        if let Some(recording) = recordings.get(index) {
            if let Err(e) = history_for_copy.copy_to_clipboard(recording) {
                error!("[MAIN] Failed to copy recording to clipboard: {}", e);
            } else {
                info!("[MAIN] Copied recording {} to clipboard", recording.id);
            }
        }
    });

    // Settings window callback — opens the native Win32 settings window
    {
        let config_for_settings = config.clone();
        let active_model_path_for_settings = active_model_path.clone();
        let engine_for_settings = shared_engine.clone();
        let log_dir_for_settings = dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("dictator")
            .join("logs");
        let config_path_for_settings = Config::config_path();

        ui::set_settings_callback(move || {
            let amp = active_model_path_for_settings.clone();
            let amp2 = active_model_path_for_settings.clone();
            let eng = engine_for_settings.clone();
            let cfg = config_for_settings.clone();

            let injection_method = match &config_for_settings.injection.method {
                dictator::config::InjectionMethod::Clipboard => "clipboard".to_string(),
                dictator::config::InjectionMethod::ClipboardEnter => "clipboard_enter".to_string(),
                dictator::config::InjectionMethod::Direct => "direct".to_string(),
            };

            let params = SettingsParams {
                injection_method,
                llm_enabled: ui::is_ollama_enabled(),
                ollama_url: config_for_settings.ollama.url.clone(),
                ollama_model: config_for_settings.ollama.model.clone(),
                idle_unload_minutes: config_for_settings.memory.idle_unload_minutes,
                log_dir: log_dir_for_settings.clone(),
                config_path: config_path_for_settings.clone(),
                get_models: {
                    let amp = amp.clone();
                    let cfg = cfg.clone();
                    Arc::new(move || {
                        let active = amp.read().map(|g| g.clone()).unwrap_or_default();
                        let models_dir = cfg.whisper.effective_models_dir()
                            .unwrap_or_else(dictator::model_downloader::default_models_dir);
                        let Ok(entries) = std::fs::read_dir(&models_dir) else { return vec![]; };
                        let mut result: Vec<InstalledModel> = entries.flatten()
                            .filter(|e| {
                                let p = e.path();
                                p.is_file() && p.extension().map(|x| x == "bin").unwrap_or(false)
                            })
                            .map(|e| {
                                let path = e.path();
                                let name = e.file_name().to_string_lossy().to_string();
                                let size_label = std::fs::metadata(&path).map(|m| {
                                    let gb = m.len() as f64 / (1024.0 * 1024.0 * 1024.0);
                                    if gb >= 0.1 { format!(" ({:.1} GB)", gb) }
                                    else { format!(" ({:.0} MB)", m.len() as f64 / (1024.0 * 1024.0)) }
                                }).unwrap_or_default();
                                let is_active = path == active;
                                InstalledModel { path, name, size_label, is_active }
                            })
                            .collect();
                        result.sort_by(|a, b| a.name.cmp(&b.name));
                        result
                    })
                },
                on_use_model: {
                    let amp = amp2.clone();
                    let eng = eng.clone();
                    let cfg = cfg.clone();
                    Arc::new(move |path: std::path::PathBuf| {
                        if let Ok(mut guard) = amp.write() { *guard = path.clone(); }
                        whisper_engine::unload_engine(&eng);
                        let mut updated = cfg.clone();
                        updated.whisper.model_path = path;
                        if let Err(e) = updated.save() {
                            error!("[SETTINGS] Failed to save config after model switch: {}", e);
                        }
                    })
                },
                on_delete_model: Arc::new(|path: std::path::PathBuf| {
                    if let Err(e) = std::fs::remove_file(&path) {
                        error!("[SETTINGS] Failed to delete model {:?}: {}", path, e);
                    } else {
                        info!("[SETTINGS] Deleted model: {:?}", path);
                    }
                }),
                on_download_model: {
                    // Reuse the existing tray download callback by triggering the same mechanism
                    let cfg2 = config_for_settings.clone();
                    let amp3 = active_model_path_for_settings.clone();
                    let eng2 = engine_for_settings.clone();
                    Arc::new(move |index: usize| {
                        let Some(model) = dictator::model_downloader::KNOWN_MODELS.get(index) else { return; };
                        let filename = model.filename.to_string();
                        let name = model.name.to_string();
                        let target_dir = cfg2.whisper.effective_models_dir()
                            .unwrap_or_else(dictator::model_downloader::default_models_dir);
                        let amp = amp3.clone();
                        let eng = eng2.clone();
                        let cfg = cfg2.clone();
                        std::thread::spawn(move || {
                            ui::set_is_downloading(true);
                            if let Ok(path) = dictator::model_downloader::download_model(&filename, &target_dir, |_| {}) {
                                if let Ok(mut guard) = amp.write() { *guard = path.clone(); }
                                whisper_engine::unload_engine(&eng);
                                let mut updated = cfg.clone();
                                updated.whisper.model_path = path;
                                let _ = updated.save();
                                info!("[SETTINGS] Downloaded model: {}", name);
                            }
                            ui::set_is_downloading(false);
                        });
                    })
                },
                on_save: {
                    let cfg = config_for_settings.clone();
                    Arc::new(move |s: SavedSettings| {
                        // Update global atomics immediately
                        ui::set_ollama_enabled(s.llm_enabled);

                        // Persist to config.toml
                        let mut updated = cfg.clone();
                        updated.injection.method = match s.injection_method.as_str() {
                            "clipboard" => dictator::config::InjectionMethod::Clipboard,
                            "clipboard_enter" => dictator::config::InjectionMethod::ClipboardEnter,
                            _ => dictator::config::InjectionMethod::Direct,
                        };
                        updated.ollama.enabled = s.llm_enabled;
                        updated.ollama.url = s.ollama_url;
                        updated.ollama.model = s.ollama_model;
                        updated.memory.idle_unload_minutes = s.idle_unload_minutes;
                        if let Err(e) = updated.save() {
                            error!("[SETTINGS] Failed to save: {}", e);
                        } else {
                            info!("[SETTINGS] Config saved successfully");
                        }
                    })
                },
            };

            settings_window::open(params);
        });
    }

    // Callback to open config file in default editor
    ui::set_open_config_callback(|| {
        let config_path = Config::config_path();
        if let Err(e) = std::process::Command::new("notepad").arg(&config_path).spawn() {
            error!("[MAIN] Failed to open config file: {}", e);
        }
    });

    // ── Model download callbacks ──────────────────────────────────────────────

    // Return list of known models, marking which are already downloaded
    let config_for_dl_list = config.clone();
    ui::set_download_list_callback(move || {
        let models_dir = config_for_dl_list
            .whisper
            .effective_models_dir()
            .unwrap_or_else(model_downloader::default_models_dir);

        model_downloader::KNOWN_MODELS
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let already_downloaded = models_dir.join(m.filename).is_file();
                DownloadModelItem {
                    index: i,
                    name: m.name.to_string(),
                    size_mb: m.size_mb,
                    already_downloaded,
                }
            })
            .collect()
    });

    // Handle a download request from the tray
    let config_for_dl = config.clone();
    let overlay_for_dl = overlay.clone();
    let active_model_path_for_dl = active_model_path.clone();
    let engine_for_dl = shared_engine.clone();
    ui::set_download_model_callback(move |index| {
        let Some(model) = model_downloader::KNOWN_MODELS.get(index) else {
            return;
        };
        let filename = model.filename.to_string();
        let name = model.name.to_string();
        let size_mb = model.size_mb;

        let target_dir = config_for_dl
            .whisper
            .effective_models_dir()
            .unwrap_or_else(model_downloader::default_models_dir);

        let overlay = overlay_for_dl.clone();
        let config_snapshot = config_for_dl.clone();
        let active_path = active_model_path_for_dl.clone();
        let engine = engine_for_dl.clone();

        thread::spawn(move || {
            ui::set_is_downloading(true);
            overlay.show(&format!("Downloading {} (~{} MB)...", name, size_mb));

            let result = model_downloader::download_model(&filename, &target_dir, |progress| {
                overlay.update_status_text(&format!(
                    "Downloading {} ({:.0}%)",
                    name,
                    progress * 100.0
                ));
            });

            ui::set_is_downloading(false);

            match result {
                Ok(path) => {
                    info!("[DOWNLOAD] Model saved to: {:?}", path);

                    // Hot-switch to the new model — no restart needed
                    if let Ok(mut guard) = active_path.write() {
                        *guard = path.clone();
                    }
                    whisper_engine::unload_engine(&engine);

                    // Persist to config.toml
                    let mut updated = config_snapshot.clone();
                    updated.whisper.model_path = path.clone();
                    if let Err(e) = updated.save() {
                        error!("[DOWNLOAD] Failed to save config after download: {}", e);
                    } else {
                        info!("[DOWNLOAD] config.toml updated: model_path = {:?}", path);
                    }

                    overlay.show(&format!("Downloaded {} \u{2713}\nReady to use!", name));
                    thread::sleep(Duration::from_secs(3));
                    overlay.hide();
                }
                Err(e) => {
                    error!("[DOWNLOAD] Failed: {}", e);
                    overlay.show(&format!("Download failed: {}", e));
                    thread::sleep(Duration::from_secs(5));
                    overlay.hide();
                }
            }
        });
    });

    // ── On first launch: show a hint if no model is found ───────────────────
    let initial_model_ok = active_model_path.read().map(|p| p.is_file()).unwrap_or(false);
    if config.whisper.backend == WhisperBackend::Embedded && !initial_model_ok
    {
        let overlay_for_hint = overlay.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(800)); // let the app fully start
            overlay_for_hint
                .show("No Whisper model found.\nRight-click tray \u{2192} Download Model");
            thread::sleep(Duration::from_secs(6));
            overlay_for_hint.hide();
        });
    }

    // Set up model selector callbacks
    let config_for_models = config.clone();
    let active_model_path_for_list = active_model_path.clone();
    ui::set_model_list_callback(move || {
        let current = active_model_path_for_list
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        scan_available_models(&config_for_models, &current)
    });

    let config_for_select = config.clone();
    let active_model_path_for_select = active_model_path.clone();
    let engine_for_select = shared_engine.clone();
    ui::set_model_select_callback(move |index| {
        let current = active_model_path_for_select
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let models = scan_available_models(&config_for_select, &current);
        if let Some(model) = models.get(index) {
            let new_path = config_for_select
                .whisper
                .effective_models_dir()
                .unwrap_or_default()
                .join(&model.name);

            // Hot-switch: update shared path + unload engine (reloads on next recording)
            if let Ok(mut guard) = active_model_path_for_select.write() {
                *guard = new_path.clone();
            }
            whisper_engine::unload_engine(&engine_for_select);

            let mut updated = config_for_select.clone();
            updated.whisper.model_path = new_path.clone();
            if let Err(e) = updated.save() {
                error!("[MAIN] Failed to save config after model switch: {}", e);
            } else {
                info!("[MAIN] Hot-switched model to: {:?}", new_path);
            }
        }
    });

    // ── Auto-updater ──────────────────────────────────────────────────────────
    // Wire up the install callback (download + apply + restart on user approval)
    ui::set_install_update_callback(|| {
        thread::spawn(|| {
            // Re-check to get the UpdateInfo then download + restart
            updater::download_and_restart("latest");
        });
    });

    // Kick off a background update check (silent, doesn't block startup)
    updater::check_for_updates_async(|version| {
        info!("[MAIN] Update available: v{}", version);
        ui::set_update_available(version);
    });

    // Start hotkey listener
    let (tx, rx) = mpsc::channel();
    let _hotkey_handle = input::start_hotkey_listener(tx.clone());

    // Start IPC listener for CLI --toggle / --stop
    start_ipc_listener(tx.clone());

    // Create streaming channel
    let (streaming_tx, streaming_rx) = std::sync::mpsc::channel::<StreamingEvent>();

    // Handle hotkey events in a separate thread
    let recorder_clone = recorder.clone();
    let ollama_clone = ollama.clone();
    let overlay_clone = overlay.clone();
    let config_clone = config.clone();
    let streaming_tx_clone = streaming_tx.clone();
    let history_clone = history.clone();
    let engine_clone = shared_engine.clone();
    let active_model_path_clone = active_model_path.clone();

    std::thread::spawn(move || {
        let history = history_clone;
        let mut saved_hwnd: Option<isize> = None;
        let mut is_recording = false;
        let mut streaming_transcriber: Option<StreamingTranscriber> = None;
        let mut accumulated_text = String::new();
        let mut recording_started_at: Option<Instant> = None;
        let mut last_recording_second: u64 = u64::MAX;
        let mut waveform_thread: Option<thread::JoinHandle<()>> = None;
        let waveform_stop = Arc::new(AtomicBool::new(false));
        let mut avg_transcribe_ratio: f32 = 0.20;
        let mut whisper_ready = false;
        let mut whisper_status_text = String::from("Whisper: idle");
        let mut last_transcription_time: Option<Instant> = None;
        let idle_unload_minutes = config_clone.memory.idle_unload_minutes;
        let is_embedded = config_clone.whisper.backend == WhisperBackend::Embedded;
        let engine = engine_clone;
        let mut whisper_manager = WhisperServerManager::new(
            config_clone
                .whisper
                .model_path
                .to_string_lossy()
                .to_string(),
        );

        info!("[MAIN] Event handler thread started, waiting for hotkey events...");

        loop {
            tracing::debug!(
                "[MAIN] Waiting for next event... (is_recording: {})",
                is_recording
            );

            // Use recv_timeout to periodically check streaming events even without hotkey
            let event = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(evt) => {
                    info!("[MAIN] ===> RECEIVED event: {:?}", evt);
                    Some(evt)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if is_recording
                        && let Some(started_at) = recording_started_at
                    {
                        let elapsed = started_at.elapsed();
                        let elapsed_sec = elapsed.as_secs();
                        if elapsed_sec != last_recording_second {
                            last_recording_second = elapsed_sec;
                            if !whisper_ready && !is_embedded {
                                match whisper_manager.poll_ready() {
                                    Ok(true) => {
                                        whisper_ready = true;
                                        whisper_status_text = "Whisper: ready".to_string();
                                    }
                                    Ok(false) => {
                                        whisper_status_text = "Whisper: starting...".to_string();
                                    }
                                    Err(e) => {
                                        whisper_status_text = "Whisper: startup error".to_string();
                                        warn!("[MAIN] Whisper poll error: {}", e);
                                    }
                                }
                            }
                            let streaming_enabled = ui::is_streaming_enabled();
                            let chunk_seconds = ui::streaming_chunk_seconds();
                            let mut status = format_recording_status(
                                elapsed,
                                streaming_enabled,
                                chunk_seconds,
                                &whisper_status_text,
                            );
                            if elapsed_sec >= 30 {
                                status.push_str("\nTip: tap hotkey again to stop");
                            }
                            overlay_clone.update_status_text(&status);
                        }
                    }
                    // Idle unload: free model memory after N minutes of inactivity
                    if idle_unload_minutes > 0 && !is_recording {
                        if let Some(last) = last_transcription_time {
                            if last.elapsed() >= Duration::from_secs(idle_unload_minutes as u64 * 60) {
                                if is_embedded {
                                    if whisper_engine::is_engine_loaded(&engine) {
                                        info!(
                                            "[MAIN] Idle timeout ({} min): unloading embedded engine",
                                            idle_unload_minutes
                                        );
                                        whisper_engine::unload_engine(&engine);
                                    }
                                } else if whisper_manager.is_server_running() {
                                    info!(
                                        "[MAIN] Idle timeout ({} min): stopping whisper server",
                                        idle_unload_minutes
                                    );
                                    whisper_manager.stop_if_owned();
                                }
                                last_transcription_time = None;
                            }
                        }
                    }
                    // No hotkey event, continue to check streaming events and recording progress
                    None
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    error!("[MAIN] Channel closed! Exiting event loop.");
                    break;
                }
            };

            // Process streaming events (non-blocking) - do this every iteration
            while let Ok(streaming_event) = streaming_rx.try_recv() {
                match streaming_event {
                    StreamingEvent::PartialText(text) => {
                        info!("[MAIN] 📥 Streaming partial text: \"{}\"", text);
                        accumulated_text = text.clone();
                        overlay_clone.update_body_text(&text);
                    }
                    StreamingEvent::FinalText(text) => {
                        info!("[MAIN] 🏁 Streaming final text: \"{}\"", text);
                        accumulated_text = text;
                    }
                    StreamingEvent::Error(e) => {
                        error!("[MAIN] Streaming error: {}", e);
                    }
                }
            }

            // If no hotkey event, continue loop
            let event = match event {
                Some(evt) => evt,
                None => continue,
            };

            // Normalize remote CLI events to concrete RecordStart/RecordStop
            let event = match event {
                HotkeyEvent::RemoteToggle => {
                    let hwnd = input::get_foreground_window_handle();
                    if is_recording {
                        HotkeyEvent::RecordStop { hwnd }
                    } else {
                        HotkeyEvent::RecordStart { hwnd }
                    }
                }
                HotkeyEvent::RemoteStop => {
                    if is_recording {
                        HotkeyEvent::RecordStop { hwnd: input::get_foreground_window_handle() }
                    } else {
                        continue; // not recording, nothing to stop
                    }
                }
                other => other,
            };

            match event {
                // RemoteToggle/RemoteStop are normalized to RecordStart/RecordStop above
                HotkeyEvent::RemoteToggle | HotkeyEvent::RemoteStop => unreachable!(),
                HotkeyEvent::RecordStart { hwnd } => {
                    if is_recording {
                        warn!("[MAIN] Received RecordStart but already recording! Ignoring.");
                        continue;
                    }

                    info!("[MAIN] ===> PROCESSING RecordStart");
                    is_recording = true;
                    recording_started_at = Some(Instant::now());
                    last_recording_second = u64::MAX;

                    // Save the window handle for later focus restoration
                    saved_hwnd = Some(hwnd);
                    info!("[MAIN] Saved focus window handle: {}", hwnd);

                    overlay_clone.position_near_cursor();
                    info!("[MAIN] Calling overlay.set_recording(true)...");
                    overlay_clone.set_recording(true);
                    overlay_clone.update_status_text("Preparing transcription...");
                    overlay_clone.update_body_text("");

                    info!("[MAIN] Calling recorder.start_recording()...");
                    if let Err(e) = recorder_clone.start_recording() {
                        error!("[MAIN] FAILED to start recording: {}", e);
                        is_recording = false;
                        overlay_clone.hide();
                    } else {
                        info!("[MAIN] Recording started");
                        let streaming_enabled = ui::is_streaming_enabled();
                        let chunk_seconds = ui::streaming_chunk_seconds();
                        overlay_clone.update_status_text(&format_recording_status(
                            Duration::from_secs(0),
                            streaming_enabled,
                            chunk_seconds,
                            &whisper_status_text,
                        ));

                        if is_embedded {
                            // Embedded: model loads lazily on first transcription
                            whisper_ready = whisper_engine::is_engine_loaded(&engine);
                            whisper_status_text = if whisper_ready {
                                "Whisper: ready (embedded)".to_string()
                            } else {
                                "Whisper: will load on stop".to_string()
                            };
                        } else {
                            whisper_ready = WhisperServerManager::is_healthy();
                            if whisper_ready {
                                whisper_status_text = "Whisper: ready".to_string();
                            } else {
                                whisper_status_text = "Whisper: starting...".to_string();
                                if let Err(e) = whisper_manager.start_if_needed() {
                                    warn!("[MAIN] Whisper server warmup start failed: {}", e);
                                    whisper_status_text = "Whisper: startup error".to_string();
                                }
                            }
                        }
                        overlay_clone.update_status_text(&format_recording_status(
                            Duration::from_secs(0),
                            streaming_enabled,
                            chunk_seconds,
                            &whisper_status_text,
                        ));

                        // Start waveform animation thread (30fps)
                        waveform_stop.store(false, Ordering::SeqCst);
                        let wf_stop = waveform_stop.clone();
                        let wf_recorder = recorder_clone.clone();
                        let wf_overlay = overlay_clone.clone();
                        waveform_thread = Some(thread::spawn(move || {
                            while !wf_stop.load(Ordering::SeqCst) {
                                let amp = wf_recorder.get_amplitude();
                                wf_overlay.update_waveform(amp);
                                thread::sleep(Duration::from_millis(33));
                            }
                        }));

                        // Start streaming if enabled (from tray menu)
                        if ui::is_streaming_enabled() {
                            info!("[MAIN] Starting streaming transcription...");
                            accumulated_text.clear();
                            let chunk_seconds = ui::streaming_chunk_seconds();
                            info!("[MAIN] Streaming chunk duration: {}s", chunk_seconds);
                            streaming_transcriber = Some(if is_embedded {
                                let model_path = active_model_path_clone
                                    .read()
                                    .map(|g| g.clone())
                                    .unwrap_or_default();
                                StreamingTranscriber::new_embedded(
                                    streaming_tx_clone.clone(),
                                    config_clone.whisper.language.clone(),
                                    chunk_seconds,
                                    engine.clone(),
                                    model_path,
                                )
                            } else {
                                StreamingTranscriber::new(
                                    streaming_tx_clone.clone(),
                                    config_clone.whisper.language.clone(),
                                    chunk_seconds,
                                )
                            });
                            if let Some(ref mut st) = streaming_transcriber {
                                st.start(recorder_clone.clone());
                            }
                        }
                    }
                }
                HotkeyEvent::RecordStop { hwnd: _ } => {
                    if !is_recording {
                        warn!("[MAIN] Received RecordStop but not recording! Ignoring.");
                        continue;
                    }

                    info!("[MAIN] ===> PROCESSING RecordStop");
                    is_recording = false;
                    recording_started_at = None;
                    last_recording_second = u64::MAX;
                    whisper_ready = false;
                    whisper_status_text = "Whisper: idle".to_string();

                    // Stop waveform thread
                    waveform_stop.store(true, Ordering::SeqCst);
                    if let Some(handle) = waveform_thread.take() {
                        let _ = handle.join();
                    }

                    overlay_clone.set_recording(false);

                    // CRITICAL: Stop streaming FIRST while recording is still active
                    // This allows streaming to read the final buffer before it's cleared
                    if let Some(mut st) = streaming_transcriber.take() {
                        info!("[MAIN] Stopping streaming transcription (while recorder still active)...");
                        st.stop();
                        // Wait for streaming to send final text (with timeout)
                        let mut final_text_received = false;
                        let timeout_duration = std::time::Duration::from_millis(3000);
                        let start_time = std::time::Instant::now();
                        while start_time.elapsed() < timeout_duration {
                            match streaming_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                                Ok(streaming_event) => {
                                    match streaming_event {
                                        StreamingEvent::FinalText(text) => {
                                            info!(
                                                "[MAIN] 🏁 Streaming final text received: \"{}\"",
                                                text
                                            );
                                            accumulated_text = text;
                                            final_text_received = true;
                                            break;
                                        }
                                        StreamingEvent::PartialText(text) => {
                                            info!("[MAIN] 📥 Late partial text: \"{}\"", text);
                                            // Update accumulated text even if we receive partial after stop
                                            accumulated_text = text;
                                        }
                                        _ => {}
                                    }
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                        if !final_text_received {
                            info!("[MAIN] No final text received from streaming, using accumulated text");
                        }
                    }

                    // NOW stop recording (after streaming has processed final buffer)
                    info!("[MAIN] Calling recorder.stop_recording()...");
                    let audio_data = match recorder_clone.stop_recording() {
                        Ok(data) => {
                            info!("[MAIN] Got {} samples of audio", data.len());
                            data
                        }
                        Err(e) => {
                            error!("[MAIN] FAILED to stop recording: {}", e);
                            if !is_embedded {
                                whisper_manager.stop_if_owned();
                            }
                            continue;
                        }
                    };

                    if audio_data.is_empty() {
                        info!("No audio recorded");
                        overlay_clone.hide();
                        continue;
                    }

                                        // Determine raw text: use streaming results if available, otherwise transcribe full audio
                    let raw_text = if !accumulated_text.is_empty() {
                        info!(
                            "[MAIN] Using streaming text ({} chars)",
                            accumulated_text.len()
                        );
                        accumulated_text.clone()
                    } else {
                        if !is_embedded {
                            if let Err(e) = whisper_manager.ensure_running(Duration::from_secs(30)) {
                                error!("[MAIN] Failed to start Whisper server: {}", e);
                                overlay_clone.show("Whisper server startup error");
                                std::thread::sleep(Duration::from_secs(2));
                                overlay_clone.hide();
                                whisper_manager.stop_if_owned();
                                continue;
                            }
                        }

                        overlay_clone.update_status_text("Transcribing...");
                        overlay_clone.update_body_text("");

                        let audio_duration_secs = audio_data.len() as f32 / 16000.0;
                        let expected_secs = (audio_duration_secs * avg_transcribe_ratio).max(2.0);
                        let language = config.whisper.language.clone();
                        let audio_for_transcribe = audio_data.clone();
                        let (transcribe_tx, transcribe_rx) = mpsc::channel();

                        let engine_for_transcribe = engine.clone();
                        let model_path_for_transcribe = active_model_path_clone
                            .read()
                            .map(|g| g.clone())
                            .unwrap_or_default();
                        std::thread::spawn(move || {
                            let result = if is_embedded {
                                whisper_engine::transcribe_with_engine(
                                    &engine_for_transcribe,
                                    &model_path_for_transcribe,
                                    &audio_for_transcribe,
                                    &language,
                                )
                            } else {
                                transcribe::transcribe_audio(&audio_for_transcribe, &language)
                            };
                            let _ = transcribe_tx.send(result);
                        });

                        let transcribe_started = Instant::now();
                        let spinner_frames = ["|", "/", "-", "\\"];
                        let mut spinner_index = 0usize;

                        let transcribed = loop {
                            match transcribe_rx.recv_timeout(Duration::from_millis(250)) {
                                Ok(result) => break result,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    let elapsed_secs = transcribe_started.elapsed().as_secs_f32();
                                    let progress = (elapsed_secs / expected_secs).min(0.99);
                                    let status = format_transcribing_status(
                                        spinner_frames[spinner_index % spinner_frames.len()],
                                        elapsed_secs,
                                        expected_secs,
                                        progress,
                                    );
                                    spinner_index = spinner_index.wrapping_add(1);
                                    overlay_clone.update_status_text(&status);
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => {
                                    break Err(anyhow::anyhow!(
                                        "Transcription worker thread disconnected unexpectedly"
                                    ));
                                }
                            }
                        };

                        match transcribed {
                            Ok(text) => {
                                let transcribe_elapsed = transcribe_started.elapsed().as_secs_f32();
                                if audio_duration_secs > 0.1 {
                                    let observed_ratio =
                                        (transcribe_elapsed / audio_duration_secs).clamp(0.05, 2.0);
                                    avg_transcribe_ratio =
                                        (avg_transcribe_ratio * 0.7 + observed_ratio * 0.3)
                                            .clamp(0.05, 2.0);
                                }

                                let words = text.split_whitespace().count();
                                let chars = text.chars().count();
                                overlay_clone.show(&format!(
                                    "Transcribed in {:.1}s\n{} words | {} chars",
                                    transcribe_elapsed, words, chars
                                ));
                                std::thread::sleep(Duration::from_millis(1200));
                                text
                            }
                            Err(e) => {
                                error!("Transcription error: {}", e);
                                overlay_clone.hide();
                                if !is_embedded {
                                    whisper_manager.stop_if_owned();
                                }
                                continue;
                            }
                        }
                    };

                    // Normalize whitespace: faster-whisper sometimes inserts double spaces
                    // between segments; split_whitespace + join gives clean single spaces.
                    let raw_text: String = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");

                    if raw_text.is_empty() {
                        info!("No text transcribed");
                        overlay_clone.hide();
                        continue;
                    }

                    // Correct text with Ollama (if enabled via config or tray toggle)
                    let final_text = if ui::is_ollama_enabled() {
                        overlay_clone.update_status_text("Correcting...");
                        match ollama_clone.correct_text(&raw_text) {
                            Ok(corrected) => corrected,
                            Err(e) => {
                                error!("LLM correction error: {}", e);
                                raw_text // Use raw text if correction fails
                            }
                        }
                    } else {
                        info!("Ollama disabled in config, using raw transcription");
                        raw_text
                    };

                    info!("========================================");
                    info!("FINAL TEXT: {}", final_text);
                    info!("========================================");

                    // Copy to clipboard (always keep a backup)
                    if let Err(e) = clipboard_win::set_clipboard_string(&final_text) {
                        error!("Failed to copy to clipboard: {}", e);
                    } else {
                        info!("Text copied to clipboard");
                    }

                    // Restore focus to the original window before injecting text
                    if let Some(hwnd) = saved_hwnd {
                        if let Err(e) = input::set_foreground_window(hwnd) {
                            error!("Failed to restore focus: {}", e);
                        }
                        // Small delay to ensure focus is restored
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }

                    // Show overlay with final text
                    overlay_clone.show(&final_text);

                    // Inject text into focused application
                    if let Err(e) = input::inject_text(&final_text, &config_clone.injection.method) {
                        error!("Failed to inject text: {}", e);
                    }

                    // Hide overlay after delay
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    overlay_clone.hide();

                    // Update last activity time for idle unload timer
                    last_transcription_time = Some(Instant::now());

                    // Save recording to history (if enabled)
                    if config_clone.history.enabled {
                        let duration_secs = audio_data.len() as f32 / 16000.0;
                        let mode = if ui::is_streaming_enabled() { "streaming" } else { "full" };
                        if let Err(e) = history.save_recording(
                            &audio_data,
                            &final_text,
                            duration_secs,
                            mode,
                            &config_clone.whisper.language,
                        ) {
                            error!("[MAIN] Failed to save recording to history: {}", e);
                        }
                    }

                    // Reset accumulated text for next recording
                    accumulated_text.clear();
                }
            }

            // Process streaming events (non-blocking)
            while let Ok(streaming_event) = streaming_rx.try_recv() {
                match streaming_event {
                    StreamingEvent::PartialText(text) => {
                        info!("[MAIN] 📥 Streaming partial text: \"{}\"", text);
                        accumulated_text = text.clone();
                        overlay_clone.update_body_text(&text);
                    }
                    StreamingEvent::FinalText(text) => {
                        info!("[MAIN] 🏁 Streaming final text: \"{}\"", text);
                        accumulated_text = text;
                    }
                    StreamingEvent::Error(e) => {
                        error!("[MAIN] Streaming error: {}", e);
                    }
                }
            }
        }
    });

    // Run system tray (blocking)
    info!("Starting system tray...");
    ui::run_tray()?;

    info!("Dictator shutting down");
    Ok(())
}


