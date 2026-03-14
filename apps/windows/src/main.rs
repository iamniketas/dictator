#![windows_subsystem = "windows"]
//! Dictator - Voice dictation service for Windows

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use dictator::audio::AudioRecorder;
use dictator::config::{Config, RuntimePreference, WhisperBackend};
use dictator::history::HistoryManager;
use dictator::input::{self, HotkeyEvent};
use dictator::llm::OllamaClient;
use dictator::model_downloader;
use model_store::{self, InstalledModel as StoreInstalledModel, InstalledRuntime as StoreInstalledRuntime};
use dictator::overlay_win32::{OverlayConfig, OverlayWindow};
use dictator::streaming::{StreamingEvent, StreamingTranscriber};
use dictator::transcribe;
use dictator::ui;
use dictator::settings_window::{self, DownloadStatus, InstalledModel, RuntimeStatus, SavedSettings, SettingsParams};
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
/// - `DictatorToggleEvent` Р Р†Р вЂљРІР‚Сњ signals RecordToggle (used by --toggle)
/// - `DictatorStopEvent`  Р Р†Р вЂљРІР‚Сњ signals RecordStop   (used by --stop)
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
            "Rec {:.1}s | {:.2} MB | streaming {}s | {}",
            elapsed_secs, size_mb, chunk_seconds, whisper_status
        )
    } else {
        format!(
            "Rec {:.1}s | {:.2} MB | full | {}",
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

fn normalize_guard_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect::<String>()
}

/// Fast O(n) detector for decoder-loop degradation in long transcripts.
/// Returns a reason string when transcript likely collapsed into repetition.
fn detect_transcript_degradation(text: &str) -> Option<String> {
    let tokens: Vec<String> = text
        .split_whitespace()
        .map(normalize_guard_token)
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.len() < 220 {
        return None;
    }

    let window_size = tokens.len().min(420);
    let tail = &tokens[tokens.len() - window_size..];

    let unique_count = tail.iter().collect::<HashSet<_>>().len();
    let unique_ratio = unique_count as f32 / window_size as f32;

    let mut trigram_counts: HashMap<String, usize> = HashMap::new();
    if tail.len() >= 3 {
        for i in 0..=(tail.len() - 3) {
            let key = format!("{} {} {}", tail[i], tail[i + 1], tail[i + 2]);
            *trigram_counts.entry(key).or_insert(0) += 1;
        }
    }

    let max_trigram = trigram_counts.values().copied().max().unwrap_or(0);
    let trigram_total = tail.len().saturating_sub(2).max(1);
    let max_trigram_ratio = max_trigram as f32 / trigram_total as f32;

    let mut max_same_token_run = 1usize;
    let mut current_run = 1usize;
    for i in 1..tail.len() {
        if tail[i] == tail[i - 1] {
            current_run += 1;
            max_same_token_run = max_same_token_run.max(current_run);
        } else {
            current_run = 1;
        }
    }

    if unique_ratio < 0.16 && (max_trigram >= 10 || max_trigram_ratio >= 0.22) {
        return Some(format!(
            "low_tail_uniqueness={:.3}, trigram_peak={} ({:.1}%), run={}",
            unique_ratio,
            max_trigram,
            max_trigram_ratio * 100.0,
            max_same_token_run
        ));
    }

    if max_same_token_run >= 7 {
        return Some(format!("same_token_run={}", max_same_token_run));
    }

    None
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
    file_size_label_bytes(total)
}


fn runtime_prefers_gpu(pref: &RuntimePreference) -> bool {
    !matches!(pref, RuntimePreference::ForceCpu)
}

fn file_size_label_bytes(bytes: u64) -> String {
    format!(" ({:.2} MB)", bytes as f64 / (1024.0 * 1024.0))
}

static SETTINGS_HOST_CHILD: LazyLock<Mutex<Option<Child>>> = LazyLock::new(|| Mutex::new(None));

fn shutdown_winui_settings_host() {
    let Ok(mut slot) = SETTINGS_HOST_CHILD.lock() else {
        return;
    };
    let Some(child) = slot.as_mut() else {
        return;
    };

    let still_running = match child.try_wait() {
        Ok(Some(_)) => false,
        Ok(None) => true,
        Err(_) => true,
    };

    if still_running {
        if let Err(e) = child.kill() {
            warn!("[SETTINGS] Failed to stop WinUI host process: {}", e);
        }
        let _ = child.wait();
    }
    *slot = None;
}

fn try_open_winui_settings_host(
    config_path: &std::path::Path,
    models_dir: &std::path::Path,
    store_path: &std::path::Path,
    history_dir: &std::path::Path,
) -> bool {
    if let Ok(mut slot) = SETTINGS_HOST_CHILD.lock() {
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    *slot = None;
                }
                Ok(None) => {
                    info!("[SETTINGS] WinUI settings host is already running");
                    return true;
                }
                Err(e) => {
                    warn!("[SETTINGS] Failed to check existing WinUI host process: {}", e);
                    *slot = None;
                }
            }
        }
    }

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            candidates.push(parent.join("Dictator.SettingsHost.exe"));

            // Dev layout: apps/windows/target/release -> apps/windows/settings-host/...
            if let Some(target_dir) = parent.parent().and_then(|p| p.parent()) {
                candidates.push(
                    target_dir
                        .join("settings-host")
                        .join("Dictator.SettingsHost")
                        .join("bin")
                        .join("Release")
                        .join("net8.0-windows10.0.19041.0")
                        .join("Dictator.SettingsHost.exe"),
                );
            }
        }
    }

    let repo_host_base = std::path::PathBuf::from("apps")
        .join("windows")
        .join("settings-host")
        .join("Dictator.SettingsHost")
        .join("bin");
    candidates.push(
        repo_host_base
            .join("Release")
            .join("net8.0-windows10.0.19041.0")
            .join("Dictator.SettingsHost.exe"),
    );
    candidates.push(
        repo_host_base
            .join("Debug")
            .join("net8.0-windows10.0.19041.0")
            .join("Dictator.SettingsHost.exe"),
    );

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(
            cwd.join("apps")
                .join("windows")
                .join("settings-host")
                .join("Dictator.SettingsHost")
                .join("bin")
                .join("Release")
                .join("net8.0-windows10.0.19041.0")
                .join("Dictator.SettingsHost.exe"),
        );
        candidates.push(
            cwd.join("settings-host")
                .join("Dictator.SettingsHost")
                .join("bin")
                .join("Release")
                .join("net8.0-windows10.0.19041.0")
                .join("Dictator.SettingsHost.exe"),
        );
    }

    let Some(exe_path) = candidates.into_iter().find(|p| p.exists()) else {
        warn!("[SETTINGS] WinUI settings host not found in known locations");
        return false;
    };

    let mut cmd = std::process::Command::new(&exe_path);
    cmd.arg("--config")
        .arg(config_path)
        .arg("--models-dir")
        .arg(models_dir)
        .arg("--store-path")
        .arg(store_path)
        .arg("--history-dir")
        .arg(history_dir);

    match cmd.spawn() {
        Ok(child) => {
            if let Ok(mut slot) = SETTINGS_HOST_CHILD.lock() {
                *slot = Some(child);
            }
            info!("[SETTINGS] Started WinUI host: {}", exe_path.display());
            true
        }
        Err(e) => {
            warn!(
                "[SETTINGS] Failed to start WinUI settings host {}: {}",
                exe_path.display(),
                e
            );
            false
        }
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
            let filename = e.file_name().to_string_lossy().to_string();
            let name = filename.clone();
            let is_current = filename == current_name;
            let size_label = if is_embedded {
                // For .bin files, use the file size directly
                std::fs::metadata(&path)
                    .map(|m| {
                        file_size_label_bytes(m.len())
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

fn sync_shared_model_store(updated_by: &str, config: &Config, active_model_path: &std::path::Path) {
    let store_path = model_store::default_store_path();
    let mut store = match model_store::load_or_default_store(&store_path) {
        Ok(v) => v,
        Err(e) => {
            warn!("[MODEL-STORE] failed to load store {}: {}", store_path.display(), e);
            return;
        }
    };

    let models_dir = config
        .whisper
        .effective_models_dir()
        .unwrap_or_else(model_downloader::default_models_dir);

    let runtime_id = String::from("embedded_whisper_rs");
    model_store::upsert_runtime(
        &mut store,
        StoreInstalledRuntime {
            id: runtime_id.clone(),
            display_name: String::from("Embedded whisper-rs"),
            kind: String::from("whisper_rs"),
            version: None,
            entry_path: models_dir.display().to_string(),
            disk_usage_bytes: None,
        },
    );

    let discovered = match model_store::discover_local_ggml_models(&models_dir) {
        Ok(v) => v,
        Err(e) => {
            warn!("[MODEL-STORE] failed to scan models in {}: {}", models_dir.display(), e);
            Vec::new()
        }
    };

    store.installed_models.clear();
    for path in discovered {
        let Some(file_name) = path.file_name().and_then(|v| v.to_str()) else { continue; };
        let model_id = file_name.to_string();
        let health = model_store::evaluate_model_health(
            &models_dir,
            &[model_id.clone()],
        );
        model_store::upsert_model(
            &mut store,
            StoreInstalledModel {
                id: model_id,
                runtime_id: runtime_id.clone(),
                directory_path: path.display().to_string(),
                size_bytes: std::fs::metadata(&path).ok().map(|m| m.len()),
                is_default: Some(path == active_model_path),
                health,
                required_files: Some(vec![file_name.to_string()]),
                registered_at: None,
            },
        );
    }

    store.models_root_path = models_dir.display().to_string();
    store.active_runtime_id = Some(runtime_id);
    store.active_model_id = active_model_path
        .file_name()
        .and_then(|v| v.to_str())
        .map(|s| s.to_string());
    store.updated_by = updated_by.to_string();

    if let Err(e) = model_store::save_store_atomic(&store_path, &store) {
        warn!("[MODEL-STORE] failed to save store {}: {}", store_path.display(), e);
    }
}

fn try_switch_to_next_fallback_model(
    config_template: &Config,
    active_model_path: &Arc<RwLock<std::path::PathBuf>>,
    engine: &SharedEngine,
    fallback_models: &Arc<RwLock<Vec<std::path::PathBuf>>>,
) -> Option<std::path::PathBuf> {
    let current = active_model_path
        .read()
        .map(|g| g.clone())
        .unwrap_or_default();

    let mut queue = match fallback_models.write() {
        Ok(v) => v,
        Err(_) => return None,
    };

    while !queue.is_empty() {
        let candidate = queue.remove(0);
        if candidate == current || !candidate.is_file() {
            continue;
        }

        if let Ok(mut guard) = active_model_path.write() {
            *guard = candidate.clone();
        }

        whisper_engine::unload_engine(engine);

        let mut updated = config_template.clone();
        updated.whisper.model_path = candidate.clone();
        if let Err(e) = updated.save() {
            error!("[POLICY] failed to save fallback model switch: {}", e);
        } else {
            sync_shared_model_store("dictator", &updated, &candidate);
        }

        return Some(candidate);
    }

    None
}

fn try_server_fallback_transcription(
    whisper_manager: &mut WhisperServerManager,
    audio_data: &[f32],
    language: &str,
    overlay: &OverlayWindow,
) -> Option<String> {
    overlay.update_status_text("Trying server fallback...");

    if let Err(e) = whisper_manager.ensure_running(Duration::from_secs(20)) {
        warn!("[POLICY] server fallback startup failed: {}", e);
        return None;
    }

    match transcribe::transcribe_audio(audio_data, language) {
        Ok(text) => Some(text),
        Err(e) => {
            warn!("[POLICY] server fallback transcription failed: {}", e);
            whisper_manager.stop_if_owned();
            None
        }
    }
}

fn log_policy_telemetry(
    stage: &str,
    retry_count: u32,
    server_fallback_used: bool,
    cloud_fallback_candidate: bool,
    active_model_path: &std::path::Path,
    audio_duration_secs: f32,
    success: bool,
) {
    let event = serde_json::json!({
        "stage": stage,
        "retry_count": retry_count,
        "server_fallback_used": server_fallback_used,
        "cloud_fallback_candidate": cloud_fallback_candidate,
        "active_model": active_model_path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
        "audio_duration_secs": audio_duration_secs,
        "success": success,
    });
    info!("[POLICY_TELEMETRY] {}", event);
}


fn run_foundation_diagnostics(fix_store: bool, open_report: bool) -> Result<std::path::PathBuf> {
    let config = Config::load()?;
    let hardware_profile = hardware_profiler::detect_hardware_profile("dictator_doctor");
    let models_dir = config
        .whisper
        .effective_models_dir()
        .unwrap_or_else(model_downloader::default_models_dir);

    let installed_model_paths = model_store::discover_local_ggml_models(&models_dir).unwrap_or_default();
    let installed_model_filenames = installed_model_paths
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
        .collect::<Vec<_>>();

    let catalog_filenames = model_downloader::get_downloadable_models()
        .into_iter()
        .map(|m| m.filename)
        .collect::<Vec<_>>();

    let catalog_remote_refs = model_store::load_embedded_catalog()
        .ok()
        .map(|c| {
            c.models
                .into_iter()
                .filter(|m| {
                    m.supported_backends
                        .iter()
                        .any(|b| b.eq_ignore_ascii_case("server"))
                })
                .filter_map(|m| if m.download_filename.is_none() { Some(m.id) } else { None })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let configured_model_ref = config.whisper.model_path.to_str();

    let runtime_policy = dictator::runtime_adapter::plan_runtime_policy(
        &hardware_profile,
        config.whisper.backend.clone(),
        configured_model_ref,
        config.runtime.preference.clone(),
        &installed_model_filenames,
        &catalog_filenames,
        &catalog_remote_refs,
    );

    if fix_store {
        sync_shared_model_store("dictator_doctor", &config, &config.whisper.model_path);
    }

    let store_path = model_store::default_store_path();
    let store_state = match model_store::load_or_default_store(&store_path) {
        Ok(store) => serde_json::json!({
            "ok": true,
            "models_root_path": store.models_root_path,
            "active_model_id": store.active_model_id,
            "active_runtime_id": store.active_runtime_id,
            "installed_models_count": store.installed_models.len(),
            "installed_runtimes_count": store.installed_runtimes.len(),
            "updated_by": store.updated_by,
        }),
        Err(e) => serde_json::json!({
            "ok": false,
            "error": e.to_string(),
        }),
    };

    let diagnostics_dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("dictator")
        .join("diagnostics");
    std::fs::create_dir_all(&diagnostics_dir)?;
    let report_path = diagnostics_dir.join("foundation_report.json");

    let report = serde_json::json!({
        "schema": "dictator.foundation_report.v1",
        "generated_at_unix_s": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "fix_store_applied": fix_store,
        "config": {
            "backend": match config.whisper.backend {
                WhisperBackend::Embedded => "embedded",
                WhisperBackend::Server => "server",
            },
            "model_path": config.whisper.model_path,
            "models_dir": models_dir,
            "language": config.whisper.language,
            "history_enabled": config.history.enabled,
            "history_retention_days": config.history.retention_days,
            "hotkey_key": config.hotkey.key,
            "hotkey_modifiers": config.hotkey.modifiers,
        },
        "hardware_profile": hardware_profile,
        "runtime_policy": {
            "summary": runtime_policy.summary_line(),
            "backend": match runtime_policy.backend {
                WhisperBackend::Embedded => "embedded",
                WhisperBackend::Server => "server",
            },
            "device": runtime_policy.device.as_str(),
            "preferred_model": runtime_policy.preferred_model,
            "fallback_models": runtime_policy.fallback_models,
            "needs_model_download": runtime_policy.needs_model_download,
            "enable_server_fallback": runtime_policy.enable_server_fallback,
            "enable_cloud_fallback": runtime_policy.enable_cloud_fallback,
            "reasons": runtime_policy.reasons,
        },
        "models": {
            "installed_count": installed_model_filenames.len(),
            "installed": installed_model_filenames,
            "catalog_count": catalog_filenames.len(),
        },
        "shared_store": {
            "path": store_path,
            "state": store_state,
        }
    });

    std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;

    if open_report {
        let _ = std::process::Command::new("notepad").arg(&report_path).spawn();
    }

    Ok(report_path)
}
fn main() -> Result<()> {
    // Velopack startup should run for installed builds.
    // In local dev runs from `target/` we skip it to avoid updater side-effects.
    let updater_enabled = std::env::var("DICTATOR_ENABLE_UPDATER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if updater_enabled {
        updater::startup();
    }

    // Handle CLI remote-control args before single-instance check.
    // These signal a running instance and exit immediately.
    {
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONINFORMATION, MB_OK,
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        let run_foundation_check = args.iter().any(|a| a == "--foundation-check");
        let run_foundation_fix = args.iter().any(|a| a == "--foundation-fix");
        let open_report = args.iter().any(|a| a == "--open-report");

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

        if run_foundation_check || run_foundation_fix {
            let fix = run_foundation_fix;
            match run_foundation_diagnostics(fix, open_report || !run_foundation_check) {
                Ok(path) => unsafe {
                    let msg: Vec<u16> = format!(
                        "Foundation diagnostics complete.\nReport: {}\nStore resync: {}",
                        path.display(),
                        if fix { "applied" } else { "no" }
                    )
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                    let _ = MessageBoxW(
                        windows::Win32::Foundation::HWND(std::ptr::null_mut()),
                        windows::core::PCWSTR(msg.as_ptr()),
                        w!("Dictator Foundation Check"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                },
                Err(e) => unsafe {
                    let msg: Vec<u16> = format!("Foundation diagnostics failed:\n{}", e)
                        .encode_utf16()
                        .chain(std::iter::once(0))
                        .collect();
                    let _ = MessageBoxW(
                        windows::Win32::Foundation::HWND(std::ptr::null_mut()),
                        windows::core::PCWSTR(msg.as_ptr()),
                        w!("Dictator Foundation Check"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                },
            }
            return Ok(());
        }
    }

    // Enforce single instance Р Р†Р вЂљРІР‚Сњ exit early if another Dictator is running
    let _single_instance = match acquire_single_instance() {
        Some(guard) => guard,
        None => return Ok(()),
    };

    // Initialize logging with robust fallback:
    // 1) preferred: file appender in %APPDATA%\dictator\logs
    // 2) fallback: stderr-only logging (no panic, app still starts)
    let log_dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("dictator")
        .join("logs");
    let log_file = log_dir.join("dictator.log");
    let mut _log_guard: Option<tracing_appender::non_blocking::WorkerGuard> = None;
    let mut file_logging_enabled = false;

    if std::fs::create_dir_all(&log_dir).is_ok() {
        // Preflight explicit file open first: tracing-appender may panic on permission-denied.
        let can_open_log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .is_ok();

        if can_open_log_file {
            let file_appender = tracing_appender::rolling::never(&log_dir, "dictator.log");
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            let _ = tracing_subscriber::fmt()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_env_filter("dictator=info")
                .try_init();
            _log_guard = Some(guard);
            file_logging_enabled = true;
        }
    }

    if !file_logging_enabled {
        let _ = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter("dictator=info")
            .try_init();
        eprintln!(
            "[DICTATOR] file logging unavailable, fallback to stderr. Path: {:?}",
            log_file
        );
    }

    info!("Dictator starting...");
    if file_logging_enabled {
        info!("Log file location: {:?}", log_file);
    } else {
        info!("File logging disabled (fallback mode).");
    }

    // Load configuration
    let mut config = Config::load()?;
    info!("Config loaded, hotkey: {:?}", config.hotkey);

    // Stage 4: adaptive runtime policy (hardware-aware backend/device/model + fallback chain)
    let hardware_profile = hardware_profiler::detect_hardware_profile("dictator");
    let models_dir = config
        .whisper
        .effective_models_dir()
        .unwrap_or_else(model_downloader::default_models_dir);

    let installed_model_filenames = model_store::discover_local_ggml_models(&models_dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
        .collect::<Vec<_>>();

    let catalog_filenames = model_downloader::get_downloadable_models()
        .into_iter()
        .map(|m| m.filename)
        .collect::<Vec<_>>();

    let catalog_remote_refs = model_store::load_embedded_catalog()
        .ok()
        .map(|c| {
            c.models
                .into_iter()
                .filter(|m| {
                    m.supported_backends
                        .iter()
                        .any(|b| b.eq_ignore_ascii_case("server"))
                })
                .filter_map(|m| if m.download_filename.is_none() { Some(m.id) } else { None })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let configured_model_ref = config.whisper.model_path.to_str();

    let runtime_policy = dictator::runtime_adapter::plan_runtime_policy(
        &hardware_profile,
        config.whisper.backend.clone(),
        configured_model_ref,
        config.runtime.preference.clone(),
        &installed_model_filenames,
        &catalog_filenames,
        &catalog_remote_refs,
    );

    info!("[POLICY] {}", runtime_policy.summary_line());
    for reason in &runtime_policy.reasons {
        info!("[POLICY] reason: {}", reason);
    }

    let effective_backend = runtime_policy.backend.clone();
    if config.whisper.backend != effective_backend {
        warn!(
            "[POLICY] runtime backend override: {} -> {}",
            match config.whisper.backend {
                WhisperBackend::Embedded => "embedded",
                WhisperBackend::Server => "server",
            },
            match effective_backend {
                WhisperBackend::Embedded => "embedded",
                WhisperBackend::Server => "server",
            }
        );
        config.whisper.backend = effective_backend.clone();
    }

    let runtime_policy_state = Arc::new(RwLock::new(runtime_policy.clone()));
    let runtime_preference_state = Arc::new(RwLock::new(config.runtime.preference.clone()));
    let last_policy_stage = Arc::new(RwLock::new(String::from("startup")));
    let policy_enable_server_fallback = Arc::new(RwLock::new(runtime_policy.enable_server_fallback));
    let policy_enable_cloud_fallback = Arc::new(RwLock::new(runtime_policy.enable_cloud_fallback));

    let policy_fallback_models = Arc::new(RwLock::new(
        runtime_policy
            .fallback_models
            .iter()
            .map(|f| models_dir.join(f))
            .collect::<Vec<_>>(),
    ));

    // Safe auto-heal: if configured model is missing, switch to best installed policy candidate.
    if !config.whisper.model_path.is_file() {
        let policy_path = models_dir.join(&runtime_policy.preferred_model);
        if policy_path.is_file() {
            warn!(
                "[POLICY] configured model missing; switching to policy model: {:?}",
                policy_path
            );
            config.whisper.model_path = policy_path.clone();
            if let Err(e) = config.save() {
                error!("[POLICY] failed to persist policy model switch: {}", e);
            }
        }
    }

    sync_shared_model_store("dictator", &config, &config.whisper.model_path);

    // Sync runtime toggles from config
    ui::set_ollama_enabled(config.ollama.enabled);

    // Initialize streaming mode from config on startup.
    ui::set_streaming_enabled(config.streaming.enabled);
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
        info!("Ollama correction enabled ({}) Р Р†Р вЂљРІР‚Сњ togglable from tray", config.ollama.url);
    } else {
        info!("Ollama correction disabled (can be enabled from tray menu)");
    }

    // Create embedded whisper engine (lazy Р Р†Р вЂљРІР‚Сњ model loads on first transcription)
    let shared_engine: SharedEngine = whisper_engine::new_shared_engine();

    // Shared active model path Р Р†Р вЂљРІР‚Сњ updated at runtime when user switches or downloads a model.
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
                let time = rec.metadata.datetime.get(11..16).unwrap_or("--:--"); // HH:MM
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

    // Settings window callback Р Р†Р вЂљРІР‚Сњ opens the native Win32 settings window
    {
        let config_for_settings = config.clone();
        let active_model_path_for_settings = active_model_path.clone();
        let engine_for_settings = shared_engine.clone();
        let log_dir_for_settings = dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("dictator")
            .join("logs");
        let config_path_for_settings = Config::config_path();
        let download_status_for_settings = Arc::new(RwLock::new(DownloadStatus::default()));
        let runtime_policy_for_settings = runtime_policy_state.clone();
        let policy_fallback_models_for_settings = policy_fallback_models.clone();
        let policy_enable_server_for_settings = policy_enable_server_fallback.clone();
        let policy_enable_cloud_for_settings = policy_enable_cloud_fallback.clone();
        let models_dir_for_settings = models_dir.clone();
        let last_policy_stage_for_settings = last_policy_stage.clone();
        let hardware_profile_for_settings = hardware_profile.clone();
        let runtime_pref_for_settings = runtime_preference_state.clone();

        ui::set_settings_callback(move || {
            let history_dir = dirs::document_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Dictator")
                .join("History");

            if try_open_winui_settings_host(
                &config_path_for_settings,
                &models_dir_for_settings,
                &model_store::default_store_path(),
                &history_dir,
            ) {
                return;
            }

            unsafe {
                use windows::core::w;
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
                let _ = MessageBoxW(
                    HWND(std::ptr::null_mut()),
                    w!("WinUI settings host is unavailable.\nRebuild from apps/windows and settings-host, then run the latest binary."),
                    w!("Dictator Settings"),
                    MB_OK | MB_ICONERROR,
                );
            }
            return;
            let amp = active_model_path_for_settings.clone();
            let amp2 = active_model_path_for_settings.clone();
            let eng = engine_for_settings.clone();
            let cfg = config_for_settings.clone();
            let dl_state = download_status_for_settings.clone();
            let rp_state = runtime_policy_for_settings.clone();
            let fallback_models_state = policy_fallback_models_for_settings.clone();
            let server_fallback_state = policy_enable_server_for_settings.clone();
            let cloud_fallback_state = policy_enable_cloud_for_settings.clone();
            let runtime_pref_state = runtime_pref_for_settings.clone();

            let injection_method = match &config_for_settings.injection.method {
                dictator::config::InjectionMethod::Clipboard => "clipboard".to_string(),
                dictator::config::InjectionMethod::ClipboardEnter => "clipboard_enter".to_string(),
                dictator::config::InjectionMethod::Direct => "direct".to_string(),
            };

            let hotkey_summary = format!(
                "Primary hotkey: {} ({}) | hold >300ms = push-to-talk, tap = toggle",
                config_for_settings.hotkey.key,
                if config_for_settings.hotkey.modifiers.is_empty() {
                    "no modifiers".to_string()
                } else {
                    config_for_settings.hotkey.modifiers.join("+")
                }
            );

            let params = SettingsParams {
                injection_method,
                llm_enabled: ui::is_ollama_enabled(),
                ollama_url: config_for_settings.ollama.url.clone(),
                ollama_model: config_for_settings.ollama.model.clone(),
                idle_unload_minutes: config_for_settings.memory.idle_unload_minutes,
                runtime_mode: config_for_settings.runtime.preference.as_str().to_string(),
                log_dir: log_dir_for_settings.clone(),
                config_path: config_path_for_settings.clone(),
                shared_models_dir: config_for_settings
                    .whisper
                    .effective_models_dir()
                    .unwrap_or_else(dictator::model_downloader::default_models_dir),
                shared_store_path: model_store::default_store_path(),
                history_enabled: config_for_settings.history.enabled,
                history_retention_days: config_for_settings.history.retention_days,
                hotkey_summary,
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
                                let filename = e.file_name().to_string_lossy().to_string();
                                let name = filename.clone();
                                let size_label = std::fs::metadata(&path)
                                    .map(|m| file_size_label_bytes(m.len()))
                                    .unwrap_or_default();
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
                        updated.whisper.model_path = path.clone();
                        if let Err(e) = updated.save() {
                            error!("[SETTINGS] Failed to save config after model switch: {}", e);
                        } else {
                            sync_shared_model_store("dictator", &updated, &path);
                        }
                    })
                },
                on_delete_model: {
                    let cfg = config_for_settings.clone();
                    let amp = active_model_path_for_settings.clone();
                    Arc::new(move |path: std::path::PathBuf| {
                        if let Err(e) = std::fs::remove_file(&path) {
                            error!("[SETTINGS] Failed to delete model {:?}: {}", path, e);
                        } else {
                            info!("[SETTINGS] Deleted model: {:?}", path);
                            let active = amp.read().map(|g| g.clone()).unwrap_or_default();
                            sync_shared_model_store("dictator", &cfg, &active);
                        }
                    })
                },
                get_download_status: {
                    let dl_state = dl_state.clone();
                    Arc::new(move || dl_state.read().map(|s| s.clone()).unwrap_or_default())
                },
                on_download_model: {
                    let cfg2 = config_for_settings.clone();
                    let amp3 = active_model_path_for_settings.clone();
                    let eng2 = engine_for_settings.clone();
                    let dl_state = download_status_for_settings.clone();
            Arc::new(move |index: usize| {
                        let Some(model) = dictator::model_downloader::get_downloadable_models().into_iter().nth(index) else { return; };
                        let filename = model.filename.to_string();
                        let download_url = model.download_url.to_string();
                        let name = model.name.to_string();
                        let target_dir = cfg2.whisper.effective_models_dir()
                            .unwrap_or_else(dictator::model_downloader::default_models_dir);
                        let amp = amp3.clone();
                        let eng = eng2.clone();
                        let cfg = cfg2.clone();
                        let dl_state_inner = dl_state.clone();
                        std::thread::spawn(move || {
                            ui::set_is_downloading(true);
                            if let Ok(mut status) = dl_state_inner.write() {
                                *status = DownloadStatus {
                                    active: true,
                                    model_name: name.clone(),
                                    progress: 0.0,
                                    downloaded_mb: 0.0,
                                    total_mb: 0.0,
                                    speed_mbps: 0.0,
                                    eta_seconds: None,
                                    completed: false,
                                    error: None,
                                };
                            }

                            let result = dictator::model_downloader::download_model(
                                &filename,
                                &download_url,
                                &target_dir,
                                |progress| {
                                    if let Ok(mut status) = dl_state_inner.write() {
                                        status.active = true;
                                        status.model_name = name.clone();
                                        status.progress = progress.progress;
                                        status.downloaded_mb = progress.downloaded_bytes as f32 / (1024.0 * 1024.0);
                                        status.total_mb = progress.total_bytes as f32 / (1024.0 * 1024.0);
                                        status.speed_mbps = progress.bytes_per_sec as f32 / (1024.0 * 1024.0);
                                        status.eta_seconds = progress.eta_seconds;
                                        status.completed = false;
                                        status.error = None;
                                    }
                                },
                            );

                            match result {
                                Ok(path) => {
                                    if let Ok(mut guard) = amp.write() { *guard = path.clone(); }
                                    whisper_engine::unload_engine(&eng);
                                    let mut updated = cfg.clone();
                                    updated.whisper.model_path = path.clone();
                                    if let Err(e) = updated.save() {
                                        error!("[SETTINGS] Failed to save config after download: {}", e);
                                        if let Ok(mut status) = dl_state_inner.write() {
                                            status.active = false;
                                            status.completed = false;
                                            status.error = Some(format!("save config failed: {}", e));
                                        }
                                    } else {
                                        sync_shared_model_store("dictator", &updated, &path);
                                        info!("[SETTINGS] Downloaded model: {}", name);
                                        if let Ok(mut status) = dl_state_inner.write() {
                                            status.active = false;
                                            status.model_name = name.clone();
                                            status.progress = 1.0;
                                            status.completed = true;
                                            status.error = None;
                                            if status.total_mb <= 0.0 {
                                                status.total_mb = status.downloaded_mb;
                                            }
                                            status.eta_seconds = Some(0);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("[SETTINGS] Download failed for {}: {}", name, e);
                                    if let Ok(mut status) = dl_state_inner.write() {
                                        status.active = false;
                                        status.completed = false;
                                        status.error = Some(e.to_string());
                                    }
                                }
                            }
                            ui::set_is_downloading(false);
                        });
                    })
                },
                on_save: {
                    let cfg = config_for_settings.clone();
                    let rp_state = rp_state.clone();
                    let fallback_models_state = fallback_models_state.clone();
                    let server_fallback_state = server_fallback_state.clone();
                    let cloud_fallback_state = cloud_fallback_state.clone();
                    let runtime_pref_state = runtime_pref_state.clone();
                    let hp = hardware_profile_for_settings.clone();
                    let eng_for_save = eng.clone();
                    Arc::new(move |s: SavedSettings| {
                        ui::set_ollama_enabled(s.llm_enabled);

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
                        updated.runtime.preference = match s.runtime_mode.as_str() {
                            "force_gpu" => dictator::config::RuntimePreference::ForceGpu,
                            "force_cpu" => dictator::config::RuntimePreference::ForceCpu,
                            _ => dictator::config::RuntimePreference::Auto,
                        };

                        let models_dir = updated
                            .whisper
                            .effective_models_dir()
                            .unwrap_or_else(dictator::model_downloader::default_models_dir);
                        let installed_model_filenames = model_store::discover_local_ggml_models(&models_dir)
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                            .collect::<Vec<_>>();
                        let catalog_filenames = dictator::model_downloader::get_downloadable_models()
                            .into_iter()
                            .map(|m| m.filename)
                            .collect::<Vec<_>>();
                        let catalog_remote_refs = model_store::load_embedded_catalog()
                            .ok()
                            .map(|c| {
                                c.models
                                    .into_iter()
                                    .filter(|m| {
                                        m.supported_backends
                                            .iter()
                                            .any(|b| b.eq_ignore_ascii_case("server"))
                                    })
                                    .filter_map(|m| if m.download_filename.is_none() { Some(m.id) } else { None })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let configured_model_ref = updated.whisper.model_path.to_str();

                        let new_policy = dictator::runtime_adapter::plan_runtime_policy(
                            &hp,
                            updated.whisper.backend.clone(),
                            configured_model_ref,
                            updated.runtime.preference.clone(),
                            &installed_model_filenames,
                            &catalog_filenames,
                            &catalog_remote_refs,
                        );

                        if let Ok(mut guard) = rp_state.write() {
                            *guard = new_policy.clone();
                        }
                        if let Ok(mut guard) = fallback_models_state.write() {
                            *guard = new_policy
                                .fallback_models
                                .iter()
                                .map(|f| models_dir.join(f))
                                .collect::<Vec<_>>();
                        }
                        if let Ok(mut guard) = server_fallback_state.write() {
                            *guard = new_policy.enable_server_fallback;
                        }
                        if let Ok(mut guard) = cloud_fallback_state.write() {
                            *guard = new_policy.enable_cloud_fallback;
                        }
                        if let Ok(mut guard) = runtime_pref_state.write() {
                            *guard = updated.runtime.preference.clone();
                        }
                        whisper_engine::unload_engine(&eng_for_save);

                        if let Err(e) = updated.save() {
                            error!("[SETTINGS] Failed to save: {}", e);
                        } else {
                            info!("[SETTINGS] Config saved successfully");
                        }
                    })
                },
                get_runtime_status: {
                    let rp = runtime_policy_for_settings.clone();
                    let active = active_model_path_for_settings.clone();
                    let stage = last_policy_stage_for_settings.clone();
                    let models_dir = models_dir_for_settings.clone();
                    let hp = hardware_profile_for_settings.clone();
                    Arc::new(move || {
                        let active_model = active
                            .read()
                            .map(|g| g.clone())
                            .unwrap_or_else(|_| std::path::PathBuf::new());
                        let active_name = active_model
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let rp_snapshot = rp.read().map(|g| g.clone()).unwrap_or_else(|_| dictator::runtime_adapter::RuntimePolicy {
                            backend: WhisperBackend::Embedded,
                            device: dictator::runtime_adapter::DevicePreference::Cpu,
                            model_profile: dictator::runtime_adapter::ModelProfile::Fast,
                            preferred_model: "ggml-base.bin".to_string(),
                            fallback_models: vec!["ggml-small.bin".to_string(), "ggml-tiny.bin".to_string()],
                            needs_model_download: false,
                            enable_server_fallback: false,
                            enable_cloud_fallback: false,
                            reasons: vec!["runtime policy unavailable".to_string()],
                        });
                        let fallback_chain = if rp_snapshot.fallback_models.is_empty() {
                            String::from("none")
                        } else {
                            rp_snapshot.fallback_models.join(", ")
                        };
                        let last_stage = stage
                            .read()
                            .map(|s| s.clone())
                            .unwrap_or_else(|_| String::from("unknown"));
                        let gpu_summary = hp
                            .gpus
                            .first()
                            .map(|g| format!("{} ({} MB)", g.name, g.vram_mb))
                            .unwrap_or_else(|| String::from("none"));
                        let recommendation = match hp.tier {
                            hardware_profiler::Tier::High => "Your hardware is strong. Use quality models (large/medium) for best accuracy.",
                            hardware_profiler::Tier::Medium => "Balanced setup detected. Start with medium/base for stable speed and quality.",
                            hardware_profiler::Tier::Low => "This PC is resource-limited. Prefer small/base models; realtime may be slower.",
                            hardware_profiler::Tier::Unknown => "Hardware score is uncertain. Start with base model and switch if latency is high.",
                        };
                        let hardware_summary = format!(
                            "OS: {:?}/{:?}\r\nCPU: {} ({}C/{}T)\r\nRAM: {} MB\r\nGPU: {}\r\nTier: {:?} ({:.2})\r\nRecommendation: {}",
                            hp.host.os,
                            hp.host.arch,
                            hp.cpu.model,
                            hp.cpu.physical_cores,
                            hp.cpu.logical_cores,
                            hp.memory.total_mb,
                            gpu_summary,
                            hp.tier,
                            hp.confidence,
                            recommendation
                        );
                        let store_path = model_store::default_store_path();
                        let storage_summary = match model_store::load_or_default_store(&store_path) {
                            Ok(store) => format!(
                                "Model folder: {}\r\nStore file: {}\r\nInstalled models: {} | runtimes: {}\r\nLast writer: {}",
                                store.models_root_path,
                                store_path.display(),
                                store.installed_models.len(),
                                store.installed_runtimes.len(),
                                store.updated_by
                            ),
                            Err(e) => format!(
                                "Model folder: {}\r\nStore file: {}\r\nStore read error: {}",
                                models_dir.display(),
                                store_path.display(),
                                e
                            ),
                        };
                        RuntimeStatus {
                            backend: match rp_snapshot.backend {
                                WhisperBackend::Embedded => String::from("embedded"),
                                WhisperBackend::Server => String::from("server"),
                            },
                            device: rp_snapshot.device.as_str().to_string(),
                            preferred_model: if active_model.is_file() {
                                active_name
                            } else {
                                format!("{} (missing in {})", active_name, models_dir.display())
                            },
                            fallback_chain,
                            server_fallback: rp_snapshot.enable_server_fallback,
                            cloud_fallback: rp_snapshot.enable_cloud_fallback,
                            last_stage,
                            hardware_summary,
                            storage_summary,
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

    // Р Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљ Model download callbacks Р Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљ

    // Return list of known models, marking which are already downloaded
    let config_for_dl_list = config.clone();
    ui::set_download_list_callback(move || {
        let models_dir = config_for_dl_list
            .whisper
            .effective_models_dir()
            .unwrap_or_else(model_downloader::default_models_dir);

        model_downloader::get_downloadable_models()
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                let already_downloaded = model_downloader::model_health(&models_dir, &m) == "ok";
                DownloadModelItem {
                    index: i,
                    name: m.name,
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
        let Some(model) = model_downloader::get_downloadable_models().into_iter().nth(index) else {
            return;
        };
        let filename = model.filename.to_string();
        let download_url = model.download_url.to_string();
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

            let result = model_downloader::download_model(&filename, &download_url, &target_dir, |progress| {
                overlay.update_status_text(&format!(
                    "Downloading {} ({:.0}%)",
                    name,
                    progress.progress * 100.0
                ));
            });

            ui::set_is_downloading(false);

            match result {
                Ok(path) => {
                    info!("[DOWNLOAD] Model saved to: {:?}", path);

                    // Hot-switch to the new model Р Р†Р вЂљРІР‚Сњ no restart needed
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
                        sync_shared_model_store("dictator", &updated, &path);
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
                sync_shared_model_store("dictator", &updated, &new_path);
                info!("[MAIN] Hot-switched model to: {:?}", new_path);
            }
        }
    });

    // Р Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљ Auto-updater Р Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљР Р†РІР‚СњР вЂљ
    if updater_enabled {
        // Wire up the install callback (download + apply + restart on user approval)
        ui::set_install_update_callback(|| {
            thread::spawn(|| {
                updater::download_and_restart("latest");
            });
        });

        // Kick off a background update check (silent, doesn't block startup)
        updater::check_for_updates_async(|version| {
            info!("[MAIN] Update available: v{}", version);
            ui::set_update_available(version);
        });
    } else {
        ui::set_install_update_callback(|| {});
    }

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
    let policy_fallback_models_clone = policy_fallback_models.clone();
    let policy_enable_server_fallback_clone = policy_enable_server_fallback.clone();
    let policy_enable_cloud_fallback_clone = policy_enable_cloud_fallback.clone();
    let last_policy_stage_clone = last_policy_stage.clone();
    let runtime_preference_state_clone = runtime_preference_state.clone();

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
        let mut live_backend = config_clone.whisper.backend.clone();
        let engine = engine_clone;
        let policy_fallback_models = policy_fallback_models_clone;
        let policy_enable_server_fallback = policy_enable_server_fallback_clone;
        let policy_enable_cloud_fallback = policy_enable_cloud_fallback_clone;
        let last_policy_stage = last_policy_stage_clone;
        let runtime_preference_state = runtime_preference_state_clone;
        let mut whisper_manager = WhisperServerManager::new(
            config_clone
                .whisper
                .model_path
                .to_string_lossy()
                .to_string(),
        );

        info!("[MAIN] Event handler thread started, waiting for hotkey events...");

        loop {
            if let Ok(live_cfg) = Config::load() {
                live_backend = live_cfg.whisper.backend.clone();
                ui::set_streaming_enabled(live_cfg.streaming.enabled);
                ui::set_streaming_chunk_seconds(live_cfg.streaming.poll_interval);
                ui::set_ollama_enabled(live_cfg.ollama.enabled);
                let live_model_path = live_cfg.whisper.model_path;
                if let Ok(mut active_guard) = active_model_path_clone.write() {
                    *active_guard = live_model_path.clone();
                }
                whisper_manager.set_model_path(live_model_path.to_string_lossy().to_string());
            }
            let is_embedded = live_backend == WhisperBackend::Embedded;

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
                            if let Ok(live_cfg) = Config::load() {
                            ui::set_streaming_enabled(live_cfg.streaming.enabled);
                            ui::set_streaming_chunk_seconds(live_cfg.streaming.poll_interval);
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
                        info!("[MAIN] РЎР‚РЎСџРІР‚СљРўС’ Streaming partial text: \"{}\"", text);
                        accumulated_text = text.clone();
                        overlay_clone.update_body_text(&text);
                    }
                    StreamingEvent::FinalText(text) => {
                        info!("[MAIN] РЎР‚РЎСџР РЏР С“ Streaming final text: \"{}\"", text);
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
                        if let Ok(live_cfg) = Config::load() {
                            ui::set_streaming_enabled(live_cfg.streaming.enabled);
                            ui::set_streaming_chunk_seconds(live_cfg.streaming.poll_interval);
                        }
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
                            } else if streaming_enabled {
                                "Whisper: loading on first chunk".to_string()
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
                                    runtime_preference_state
                                        .read()
                                        .map(|p| runtime_prefers_gpu(&p))
                                        .unwrap_or(true),
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
                                                "[MAIN] РЎР‚РЎСџР РЏР С“ Streaming final text received: \"{}\"",
                                                text
                                            );
                                            accumulated_text = text;
                                            final_text_received = true;
                                            break;
                                        }
                                        StreamingEvent::PartialText(text) => {
                                            info!("[MAIN] РЎР‚РЎСџРІР‚СљРўС’ Late partial text: \"{}\"", text);
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

                    let mut policy_stage = if is_embedded {
                        String::from("embedded_primary")
                    } else {
                        String::from("server_primary")
                    };
                    let mut policy_retry_count: u32 = 0;
                    let mut server_fallback_used = false;
                    let save_failed_audio = |reason: &str, stage: &str| {
                        if config_clone.history.enabled {
                            let duration_secs = audio_data.len() as f32 / 16000.0;
                            let base_mode = if ui::is_streaming_enabled() { "streaming" } else { "full" };
                            let mode = format!("{}|{}", base_mode, stage);
                            let fail_text = format!("[Transcription failed] {}", reason);
                            if let Err(err) = history.save_recording(
                                &audio_data,
                                &fail_text,
                                duration_secs,
                                &mode,
                                &config_clone.whisper.language,
                            ) {
                                error!("[MAIN] Failed to persist failed recording: {}", err);
                            }
                        }
                    };

                                        // Determine raw text: use streaming results if available, otherwise transcribe full audio
                    let mut raw_text = if !accumulated_text.is_empty() {
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
                                save_failed_audio("server startup error", "server_start_failed");
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
                        let prefer_gpu_now = runtime_preference_state
                            .read()
                            .map(|p| runtime_prefers_gpu(&p))
                            .unwrap_or(true);
                        std::thread::spawn(move || {
                            let result = if is_embedded {
                                whisper_engine::transcribe_with_engine(
                                    &engine_for_transcribe,
                                    &model_path_for_transcribe,
                                    &audio_for_transcribe,
                                    &language,
                                    prefer_gpu_now,
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
                                if is_embedded {
                                    if let Some(new_model_path) = try_switch_to_next_fallback_model(
                                        &config_clone,
                                        &active_model_path_clone,
                                        &engine,
                                        &policy_fallback_models,
                                    ) {
                                        warn!(
                                            "[POLICY] transcription failed; retrying with fallback model {:?}",
                                            new_model_path
                                        );
                                        overlay_clone.update_status_text("Retrying with fallback model...");

                                        let retry_started = Instant::now();
                                        let retry_language = config_clone.whisper.language.clone();
                                        match whisper_engine::transcribe_with_engine(
                                            &engine,
                                            &new_model_path,
                                            &audio_data,
                                            &retry_language,
                                            runtime_preference_state
                                                .read()
                                                .map(|p| runtime_prefers_gpu(&p))
                                                .unwrap_or(true),
                                        ) {
                                            Ok(retry_text) => {
                                                policy_stage = String::from("embedded_model_retry");
                                                policy_retry_count = policy_retry_count.saturating_add(1);
                                                let retry_elapsed = retry_started.elapsed().as_secs_f32();
                                                if audio_duration_secs > 0.1 {
                                                    let observed_ratio =
                                                        (retry_elapsed / audio_duration_secs).clamp(0.05, 2.0);
                                                    avg_transcribe_ratio =
                                                        (avg_transcribe_ratio * 0.7 + observed_ratio * 0.3)
                                                            .clamp(0.05, 2.0);
                                                }

                                                let words = retry_text.split_whitespace().count();
                                                let chars = retry_text.chars().count();
                                                overlay_clone.show(&format!(
                                                    "Fallback transcribed in {:.1}s\n{} words | {} chars",
                                                    retry_elapsed, words, chars
                                                ));
                                                std::thread::sleep(Duration::from_millis(900));
                                                retry_text
                                            }
                                            Err(retry_err) => {
                                                error!(
                                                    "Transcription retry failed on fallback model: {}",
                                                    retry_err
                                                );
                                                if policy_enable_server_fallback.read().map(|v| *v).unwrap_or(false) {
                                                    if let Some(server_text) = try_server_fallback_transcription(
                                                        &mut whisper_manager,
                                                        &audio_data,
                                                        &config_clone.whisper.language,
                                                        &overlay_clone,
                                                    ) {
                                                        policy_stage = String::from("server_fallback");
                                                        server_fallback_used = true;
                                                        server_text
                                                    } else {
                                                        if policy_enable_cloud_fallback.read().map(|v| *v).unwrap_or(false) {
                                                            warn!("[POLICY] server fallback unavailable; cloud fallback planned but not implemented yet");
                                                        }
                                                        overlay_clone.hide();
                                                        save_failed_audio("embedded retry failed and server fallback unavailable", "embedded_retry_failed");
                                                        continue;
                                                    }
                                                } else {
                                                    overlay_clone.hide();
                                                    save_failed_audio("embedded retry failed", "embedded_retry_failed");
                                                    continue;
                                                }
                                            }
                                        }
                                    } else if policy_enable_server_fallback.read().map(|v| *v).unwrap_or(false) {
                                        if let Some(server_text) = try_server_fallback_transcription(
                                            &mut whisper_manager,
                                            &audio_data,
                                            &config_clone.whisper.language,
                                            &overlay_clone,
                                        ) {
                                            policy_stage = String::from("server_fallback");
                                            server_fallback_used = true;
                                            server_text
                                        } else {
                                            if policy_enable_cloud_fallback.read().map(|v| *v).unwrap_or(false) {
                                                warn!("[POLICY] no local/server fallback left; cloud fallback planned but not implemented yet");
                                            }
                                            overlay_clone.hide();
                                            save_failed_audio("server fallback unavailable", "server_fallback_unavailable");
                                            continue;
                                        }
                                    } else {
                                        overlay_clone.hide();
                                        save_failed_audio("embedded transcription failed", "embedded_failed");
                                        continue;
                                    }
                                } else {
                                    whisper_manager.stop_if_owned();
                                    overlay_clone.hide();
                                    save_failed_audio("server transcription failed", "server_failed");
                                    continue;
                                }
                            }
                        }
                    };

                    // Normalize whitespace: faster-whisper sometimes inserts double spaces
                    // between segments; split_whitespace + join gives clean single spaces.
                    raw_text = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");

                    if raw_text.is_empty() {
                        info!("No text transcribed");
                        if let Ok(mut stage) = last_policy_stage.write() {
                            *stage = String::from("empty_text");
                        }
                        let active_model_for_telemetry = active_model_path_clone
                            .read()
                            .map(|g| g.clone())
                            .unwrap_or_default();
                        let telemetry_duration_secs = audio_data.len() as f32 / 16000.0;
                        log_policy_telemetry(
                            "empty_text",
                            policy_retry_count,
                            server_fallback_used,
                            policy_enable_cloud_fallback.read().map(|v| *v).unwrap_or(false),
                            &active_model_for_telemetry,
                            telemetry_duration_secs,
                            false,
                        );
                        overlay_clone.hide();
                        continue;
                    }

                    // Guard against long-form decoder loops (repeated phrase collapse).
                    // Runs in O(n) and adds only a few milliseconds for typical transcripts.
                    if is_embedded && !ui::is_streaming_enabled() {
                        if let Some(reason) = detect_transcript_degradation(&raw_text) {
                            warn!(
                                "[QUALITY_GUARD] Degradation suspected, retrying safe profile: {}",
                                reason
                            );
                            overlay_clone.update_status_text("Quality guard: retrying...");

                            let safe_model_path = active_model_path_clone
                                .read()
                                .map(|g| g.clone())
                                .unwrap_or_default();
                            let safe_language = config_clone.whisper.language.clone();
                            let prefer_gpu_now = runtime_preference_state
                                .read()
                                .map(|p| runtime_prefers_gpu(&p))
                                .unwrap_or(true);

                            match whisper_engine::transcribe_with_engine_profile(
                                &engine,
                                &safe_model_path,
                                &audio_data,
                                &safe_language,
                                prefer_gpu_now,
                                whisper_engine::DecodeProfile::Safe,
                            ) {
                                Ok(safe_text_raw) => {
                                    let safe_text = safe_text_raw
                                        .split_whitespace()
                                        .collect::<Vec<_>>()
                                        .join(" ");

                                    if !safe_text.is_empty() {
                                        if let Some(safe_reason) =
                                            detect_transcript_degradation(&safe_text)
                                        {
                                            warn!(
                                                "[QUALITY_GUARD] Safe profile still degraded: {}",
                                                safe_reason
                                            );
                                            if policy_enable_server_fallback
                                                .read()
                                                .map(|v| *v)
                                                .unwrap_or(false)
                                            {
                                                if let Some(server_text) = try_server_fallback_transcription(
                                                    &mut whisper_manager,
                                                    &audio_data,
                                                    &config_clone.whisper.language,
                                                    &overlay_clone,
                                                ) {
                                                    policy_stage = String::from("server_fallback_quality_guard");
                                                    server_fallback_used = true;
                                                    raw_text = server_text
                                                        .split_whitespace()
                                                        .collect::<Vec<_>>()
                                                        .join(" ");
                                                }
                                            }
                                        } else {
                                            policy_stage = String::from("embedded_quality_guard_retry");
                                            policy_retry_count = policy_retry_count.saturating_add(1);
                                            raw_text = safe_text;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("[QUALITY_GUARD] Safe profile retry failed: {}", e);
                                }
                            }
                        }
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

                    let active_model_for_telemetry = active_model_path_clone
                        .read()
                        .map(|g| g.clone())
                        .unwrap_or_default();
                    if let Ok(mut stage) = last_policy_stage.write() {
                        *stage = policy_stage.clone();
                    }
                    let telemetry_duration_secs = audio_data.len() as f32 / 16000.0;
                    log_policy_telemetry(
                        &policy_stage,
                        policy_retry_count,
                        server_fallback_used,
                        policy_enable_cloud_fallback.read().map(|v| *v).unwrap_or(false),
                        &active_model_for_telemetry,
                        telemetry_duration_secs,
                        true,
                    );

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
                        let base_mode = if ui::is_streaming_enabled() { "streaming" } else { "full" };
                        let mode = format!("{}|{}", base_mode, policy_stage);
                        if let Err(e) = history.save_recording(
                            &audio_data,
                            &final_text,
                            duration_secs,
                            &mode,
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
                        info!("[MAIN] РЎР‚РЎСџРІР‚СљРўС’ Streaming partial text: \"{}\"", text);
                        accumulated_text = text.clone();
                        overlay_clone.update_body_text(&text);
                    }
                    StreamingEvent::FinalText(text) => {
                        info!("[MAIN] РЎР‚РЎСџР РЏР С“ Streaming final text: \"{}\"", text);
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
    shutdown_winui_settings_host();

    info!("Dictator shutting down");
    Ok(())
}































































































































