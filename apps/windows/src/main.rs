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
use dictator::corrections::CorrectionsManager;
use dictator::history::HistoryManager;
use dictator::input::{self, HotkeyEvent};
use dictator::llm::OllamaClient;
use dictator::model_downloader;
use dictator::overlay_win32::{OverlayConfig, OverlayWindow};
use dictator::streaming::{StreamingEvent, StreamingTranscriber};
use dictator::transcribe;
use dictator::ui;
use dictator::ui::{DownloadModelItem, ModelMenuItem};
use dictator::updater;
use dictator::whisper_engine::{self, SharedEngine};
use dictator::whisper_server::WhisperServerManager;
use model_store::{
    self, InstalledModel as StoreInstalledModel, InstalledRuntime as StoreInstalledRuntime,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SYNCHRONIZATION_ACCESS_RIGHTS, SetEvent,
    WaitForMultipleObjects,
};

/// RAII guard that holds the single-instance named mutex.
/// When dropped (on exit), the mutex is released, allowing a new instance to start.
struct SingleInstanceGuard(windows::Win32::Foundation::HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Try to open and signal an existing named event (used by CLI --toggle / --stop).
/// Returns true if the event was found and signaled (another instance is running).
fn try_signal_remote(event_name: windows::core::PCWSTR) -> bool {
    // EVENT_MODIFY_STATE = 0x0002
    unsafe {
        // EVENT_MODIFY_STATE = 0x0002
        match OpenEventW(
            SYNCHRONIZATION_ACCESS_RIGHTS(0x0002),
            windows::Win32::Foundation::BOOL(0),
            event_name,
        ) {
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
    use windows::Win32::Foundation::BOOL;
    use windows::core::w;

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
    use windows::Win32::Foundation::{BOOL, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_DESKTOP, MB_ICONINFORMATION, MB_OK, MessageBoxW,
    };
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
        "Transcribing {} {:.0}% | elapsed {:.1}s | ETA ~{:.1}s",
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

fn dir_size_bytes(path: &std::path::Path) -> Option<u64> {
    if !path.exists() {
        return None;
    }
    if path.is_file() {
        return std::fs::metadata(path).ok().map(|m| m.len());
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                if let Ok(meta) = std::fs::metadata(&p) {
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }
    Some(total)
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RuntimeProfileMarker {
    #[serde(rename = "LocalModelPath")]
    local_model_path: Option<String>,
}

fn normalize_updated_by(updated_by: &str) -> String {
    match updated_by {
        "dictator" | "contora" | "manual" | "unknown" => updated_by.to_string(),
        _ => String::from("dictator"),
    }
}

fn discover_server_runtime_models(
    models_dir: &std::path::Path,
) -> Vec<(String, std::path::PathBuf)> {
    let mut result: Vec<(String, std::path::PathBuf)> = Vec::new();
    let runtime_models_root = models_dir.join("runtime-models");
    if runtime_models_root.exists() {
        let Ok(entries) = std::fs::read_dir(&runtime_models_root) else {
            return result;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(model_id) = path
                .file_name()
                .and_then(|v| v.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            let has_any_files = std::fs::read_dir(&path)
                .ok()
                .map(|rd| rd.flatten().next().is_some())
                .unwrap_or(false);
            if has_any_files {
                result.push((model_id, path));
            }
        }
    }

    let profile_root = models_dir.join("runtimes").join("profiles");
    if profile_root.exists() {
        let Ok(entries) = std::fs::read_dir(&profile_root) else {
            return result;
        };
        for entry in entries.flatten() {
            let marker_path = entry.path();
            if !marker_path
                .extension()
                .and_then(|v| v.to_str())
                .map(|s| s.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
            {
                continue;
            }
            let Some(model_id) = marker_path
                .file_stem()
                .and_then(|v| v.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            let local_path = std::fs::read_to_string(&marker_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<RuntimeProfileMarker>(&raw).ok())
                .and_then(|m| m.local_model_path)
                .map(std::path::PathBuf::from)
                .filter(|p| p.exists())
                .unwrap_or_else(|| runtime_models_root.join(&model_id));
            if local_path.exists() {
                result.push((model_id, local_path));
            }
        }
    }

    result.sort_by(|a, b| a.0.cmp(&b.0));
    result.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    result
}

fn runtime_prefers_gpu(pref: &RuntimePreference) -> bool {
    !matches!(pref, RuntimePreference::ForceCpu)
}

fn server_device_hint(pref: &RuntimePreference) -> Option<String> {
    match pref {
        RuntimePreference::ForceCpu => Some(String::from("cpu")),
        RuntimePreference::ForceGpu => Some(String::from("cuda")),
        RuntimePreference::Auto => Some(String::from("auto")),
    }
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
    audio_dir: &std::path::Path,
    transcripts_dir: &std::path::Path,
    onboarding: bool,
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
                    warn!(
                        "[SETTINGS] Failed to check existing WinUI host process: {}",
                        e
                    );
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
        .arg(history_dir)
        .arg("--audio-dir")
        .arg(audio_dir)
        .arg("--transcripts-dir")
        .arg(transcripts_dir);
    if onboarding {
        cmd.arg("--onboarding");
    }

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

fn onboarding_marker_path() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("dictator")
        .join("state")
        .join("onboarding_completed.marker")
}

fn is_first_run() -> bool {
    !onboarding_marker_path().exists()
}

fn mark_onboarding_completed() {
    let marker = onboarding_marker_path();
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(marker, b"ok");
}
fn scan_available_models(config: &Config, current_path: &std::path::Path) -> Vec<ModelMenuItem> {
    let Some(models_dir) = config.whisper.effective_models_dir() else {
        return Vec::new();
    };

    let current_normalized = current_path
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase();

    let embedded = model_store::discover_local_ggml_models(&models_dir).unwrap_or_default();
    let server = discover_server_runtime_models(&models_dir);

    let mut models: Vec<ModelMenuItem> = Vec::new();

    for path in embedded {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let path_str = path.to_string_lossy().to_string();
        let is_current = path_str.replace('/', "\\").to_lowercase() == current_normalized;
        let size_label = std::fs::metadata(&path)
            .map(|m| file_size_label_bytes(m.len()))
            .unwrap_or_default();
        models.push(ModelMenuItem {
            index: 0,
            name,
            is_current,
            model_path: path_str,
            backend_kind: String::from("embedded"),
            size_label,
        });
    }

    for (model_id, path) in server {
        let path_str = path.to_string_lossy().to_string();
        let is_current = path_str.replace('/', "\\").to_lowercase() == current_normalized;
        let size_label = model_dir_size_label(&path);
        models.push(ModelMenuItem {
            index: 0,
            name: model_id,
            is_current,
            model_path: path_str,
            backend_kind: String::from("server"),
            size_label,
        });
    }

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
            warn!(
                "[MODEL-STORE] failed to load store {}: {}",
                store_path.display(),
                e
            );
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
            disk_usage_bytes: dir_size_bytes(&models_dir),
        },
    );
    let server_runtime_id = String::from("server_python_asr");
    let server_runtime_root = models_dir.join("runtimes").join("python-asr");
    if server_runtime_root.exists() {
        model_store::upsert_runtime(
            &mut store,
            StoreInstalledRuntime {
                id: server_runtime_id.clone(),
                display_name: String::from("Server Python ASR"),
                kind: String::from("faster_whisper"),
                version: None,
                entry_path: server_runtime_root.display().to_string(),
                disk_usage_bytes: dir_size_bytes(&server_runtime_root),
            },
        );
    }

    let discovered = match model_store::discover_local_ggml_models(&models_dir) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "[MODEL-STORE] failed to scan models in {}: {}",
                models_dir.display(),
                e
            );
            Vec::new()
        }
    };
    let catalog = model_store::load_embedded_catalog().ok();
    let mut by_filename: HashMap<String, model_store::CatalogModel> = HashMap::new();
    if let Some(c) = &catalog {
        for model in &c.models {
            if let Some(filename) = &model.download_filename {
                by_filename.insert(filename.to_ascii_lowercase(), model.clone());
            }
        }
    }

    let managed_runtime_ids = [runtime_id.as_str(), server_runtime_id.as_str()];
    store.installed_models.retain(|m| {
        !managed_runtime_ids
            .iter()
            .any(|id| m.runtime_id.eq_ignore_ascii_case(id))
    });
    for path in discovered {
        let Some(file_name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        let catalog_entry = by_filename.get(&file_name.to_ascii_lowercase());
        let model_id = catalog_entry
            .map(|m| m.id.clone())
            .unwrap_or_else(|| file_name.to_string());
        let required_files = catalog_entry
            .map(|m| {
                if m.required_files.is_empty() {
                    vec![file_name.to_string()]
                } else {
                    m.required_files.clone()
                }
            })
            .unwrap_or_else(|| vec![file_name.to_string()]);
        let health = model_store::evaluate_model_health(&models_dir, &required_files);
        model_store::upsert_model(
            &mut store,
            StoreInstalledModel {
                id: model_id,
                runtime_id: runtime_id.clone(),
                directory_path: path.display().to_string(),
                size_bytes: std::fs::metadata(&path).ok().map(|m| m.len()),
                is_default: Some(path == active_model_path),
                health,
                required_files: Some(required_files),
                registered_at: None,
            },
        );
    }

    for (model_id, model_path) in discover_server_runtime_models(&models_dir) {
        let health = if model_path.exists() {
            String::from("ok")
        } else {
            String::from("missing_files")
        };
        let active_for_server = matches!(config.whisper.backend, WhisperBackend::Server)
            && active_model_path == model_path;
        model_store::upsert_model(
            &mut store,
            StoreInstalledModel {
                id: model_id,
                runtime_id: server_runtime_id.clone(),
                directory_path: model_path.display().to_string(),
                size_bytes: dir_size_bytes(&model_path),
                is_default: Some(active_for_server),
                health,
                required_files: None,
                registered_at: None,
            },
        );
    }

    store.models_root_path = models_dir.display().to_string();
    store.active_runtime_id = match config.whisper.backend {
        WhisperBackend::Embedded => Some(runtime_id),
        WhisperBackend::Server => Some(server_runtime_id),
    };
    store.active_model_id = active_model_path
        .file_name()
        .and_then(|v| v.to_str())
        .map(|s| s.to_string())
        .or_else(|| {
            active_model_path
                .to_str()
                .and_then(|s| (!s.is_empty()).then_some(s.to_string()))
        });
    store.updated_by = normalize_updated_by(updated_by);
    store.updated_at = Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));

    if let Err(e) = model_store::save_store_atomic(&store_path, &store) {
        warn!(
            "[MODEL-STORE] failed to save store {}: {}",
            store_path.display(),
            e
        );
        return;
    }

    if let Err(e) = write_cross_app_manifest(&models_dir, &store) {
        warn!("[MODEL-STORE] failed to write cross-app manifest: {}", e);
    }
}

fn write_cross_app_manifest(models_dir: &std::path::Path, store: &model_store::SharedModelStore) -> anyhow::Result<()> {
    let manifest_path = models_dir.join("shared_runtime_manifest.v1.json");
    let manifest = serde_json::json!({
        "schema_version": "shared_runtime_manifest.v1",
        "updated_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "updated_by": "dictator",
        "producer": {
            "app_id": "dictator.windows",
            "app_version": env!("CARGO_PKG_VERSION"),
            "runtime_policy_schema": "runtime_policy.v1",
            "model_store_schema": model_store::STORE_SCHEMA_VERSION,
            "hardware_profile_schema": "hardware_profile.v1",
            "corrections_schema": "dictator_corrections.v1"
        },
        "active_runtime_id": store.active_runtime_id,
        "active_model_id": store.active_model_id,
        "installed_runtimes_count": store.installed_runtimes.len(),
        "installed_models_count": store.installed_models.len(),
        "compat": {
            "contora_min_schema_support": "shared_model_store.v1",
            "dictator_min_schema_support": "shared_model_store.v1"
        }
    });
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    Ok(())
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
        updated.whisper.backend = WhisperBackend::Embedded;
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

    let installed_model_paths =
        model_store::discover_local_ggml_models(&models_dir).unwrap_or_default();
    let installed_model_filenames = installed_model_paths
        .iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
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
                .filter_map(|m| {
                    if m.download_filename.is_none() {
                        Some(m.id)
                    } else {
                        None
                    }
                })
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
        let _ = std::process::Command::new("notepad")
            .arg(&report_path)
            .spawn();
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
        use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
        use windows::core::w;
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
    info!(
        "[BUILD] CUDA support: {}",
        if cfg!(feature = "cuda") {
            "enabled"
        } else {
            "disabled"
        }
    );
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
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
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
                .filter_map(|m| {
                    if m.download_filename.is_none() {
                        Some(m.id)
                    } else {
                        None
                    }
                })
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

    let runtime_preference_state = Arc::new(RwLock::new(config.runtime.preference.clone()));
    let last_policy_stage = Arc::new(RwLock::new(String::from("startup")));
    let policy_enable_server_fallback =
        Arc::new(RwLock::new(runtime_policy.enable_server_fallback));
    let policy_enable_cloud_fallback = Arc::new(RwLock::new(runtime_policy.enable_cloud_fallback));

    let policy_fallback_models = Arc::new(RwLock::new(
        runtime_policy
            .fallback_models
            .iter()
            .map(|f| models_dir.join(f))
            .collect::<Vec<_>>(),
    ));

    // Safe auto-heal: only heal when current backend's model target is actually missing.
    let model_target_missing = match config.whisper.backend {
        WhisperBackend::Embedded => !config.whisper.model_path.is_file(),
        WhisperBackend::Server => !config.whisper.model_path.exists(),
    };
    if model_target_missing {
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
    info!(
        "[MAIN] Initial streaming state: {}",
        ui::is_streaming_enabled()
    );
    info!(
        "[MAIN] Initial streaming chunk: {}s",
        ui::streaming_chunk_seconds()
    );

    // Create Ollama client
    let ollama = Arc::new(OllamaClient::new(&config.ollama.url, &config.ollama.model));

    // Log Ollama status
    if config.ollama.enabled {
        info!(
            "Ollama correction enabled ({}) Р Р†Р вЂљРІР‚Сњ togglable from tray",
            config.ollama.url
        );
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
    let history = Arc::new(HistoryManager::new(
        config.storage.audio_history_dir.clone(),
        config.storage.transcripts_dir.clone(),
        config.history.retention_days,
    )?);
    info!(
        "[MAIN] History manager created, enabled: {}",
        config.history.enabled
    );
    let history_root = config
        .storage
        .audio_history_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| {
            dirs::document_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Dictator")
                .join("History")
        });

    let corrections = Arc::new(CorrectionsManager::new(&config.corrections)?);
    info!(
        "[MAIN] Corrections dictionary: {:?}",
        corrections.dictionary_path()
    );

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
        let config_path_for_settings = Config::config_path();
        let models_dir_for_settings = models_dir.clone();
        let history_root_for_settings = history_root.clone();
        let audio_dir_for_settings = config.storage.audio_history_dir.clone();
        let transcripts_dir_for_settings = config.storage.transcripts_dir.clone();

        ui::set_settings_callback(move || {
            if try_open_winui_settings_host(
                &config_path_for_settings,
                &models_dir_for_settings,
                &model_store::default_store_path(),
                &history_root_for_settings,
                &audio_dir_for_settings,
                &transcripts_dir_for_settings,
                false,
            ) {
                return;
            }

            unsafe {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
                use windows::core::w;
                let _ = MessageBoxW(
                    HWND(std::ptr::null_mut()),
                    w!(
                        "WinUI settings host is unavailable.\nRebuild from apps/windows and settings-host, then run the latest binary."
                    ),
                    w!("Dictator Settings"),
                    MB_OK | MB_ICONERROR,
                );
            }
            return;
        });
    }

    if is_first_run() {
        if try_open_winui_settings_host(
            &Config::config_path(),
            &models_dir,
            &model_store::default_store_path(),
            &history_root,
            &config.storage.audio_history_dir,
            &config.storage.transcripts_dir,
            true,
        ) {
            mark_onboarding_completed();
        }
    }

    // Callback to open config file in default editor
    ui::set_open_config_callback(|| {
        let config_path = Config::config_path();
        if let Err(e) = std::process::Command::new("notepad")
            .arg(&config_path)
            .spawn()
        {
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
        let Some(model) = model_downloader::get_downloadable_models()
            .into_iter()
            .nth(index)
        else {
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

            let result = model_downloader::download_model(
                &filename,
                &download_url,
                &target_dir,
                |progress| {
                    overlay.update_status_text(&format!(
                        "Downloading {} ({:.0}%)",
                        name,
                        progress.progress * 100.0
                    ));
                },
            );

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

    let initial_model_ok = active_model_path
        .read()
        .map(|p| p.is_file())
        .unwrap_or(false);
    if config.whisper.backend == WhisperBackend::Embedded && !initial_model_ok {
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
    let active_model_path_for_list = active_model_path.clone();
    ui::set_model_list_callback(move || {
        let live_cfg = match Config::load() {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let current = active_model_path_for_list
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        scan_available_models(&live_cfg, &current)
    });

    let active_model_path_for_select = active_model_path.clone();
    let engine_for_select = shared_engine.clone();
    ui::set_model_select_callback(move |index| {
        let live_cfg = match Config::load() {
            Ok(v) => v,
            Err(e) => {
                error!("[MAIN] Failed to load config for model switch: {}", e);
                return;
            }
        };
        let current = active_model_path_for_select
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let models = scan_available_models(&live_cfg, &current);
        if let Some(model) = models.get(index) {
            let new_path = std::path::PathBuf::from(&model.model_path);
            let new_backend = if model.backend_kind.eq_ignore_ascii_case("server") {
                WhisperBackend::Server
            } else {
                WhisperBackend::Embedded
            };

            // Hot-switch: update shared path + unload engine (reloads on next recording)
            if let Ok(mut guard) = active_model_path_for_select.write() {
                *guard = new_path.clone();
            }
            whisper_engine::unload_engine(&engine_for_select);

            let mut updated = live_cfg.clone();
            updated.whisper.model_path = new_path.clone();
            updated.whisper.backend = new_backend;
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
    let corrections_clone = corrections.clone();
    let engine_clone = shared_engine.clone();
    let active_model_path_clone = active_model_path.clone();
    let policy_fallback_models_clone = policy_fallback_models.clone();
    let policy_enable_server_fallback_clone = policy_enable_server_fallback.clone();
    let policy_enable_cloud_fallback_clone = policy_enable_cloud_fallback.clone();
    let last_policy_stage_clone = last_policy_stage.clone();
    let runtime_preference_state_clone = runtime_preference_state.clone();

    std::thread::spawn(move || {
        let history = history_clone;
        let corrections = corrections_clone;
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
        whisper_manager.set_preferred_device(server_device_hint(&config_clone.runtime.preference));

        info!("[MAIN] Event handler thread started, waiting for hotkey events...");

        loop {
            if let Ok(live_cfg) = Config::load() {
                live_backend = live_cfg.whisper.backend.clone();
                ui::set_streaming_enabled(live_cfg.streaming.enabled);
                ui::set_streaming_chunk_seconds(live_cfg.streaming.poll_interval);
                ui::set_ollama_enabled(live_cfg.ollama.enabled);
                if let Ok(mut pref_guard) = runtime_preference_state.write() {
                    *pref_guard = live_cfg.runtime.preference.clone();
                }
                let live_model_path = live_cfg.whisper.model_path;
                if let Ok(mut active_guard) = active_model_path_clone.write() {
                    *active_guard = live_model_path.clone();
                }
                whisper_manager.set_model_path(live_model_path.to_string_lossy().to_string());
                whisper_manager.set_preferred_device(server_device_hint(&live_cfg.runtime.preference));
            }
            let is_embedded = live_backend == WhisperBackend::Embedded;

            // Use recv_timeout to periodically check streaming events even without hotkey
            let event = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(evt) => {
                    info!("[MAIN] ===> RECEIVED event: {:?}", evt);
                    Some(evt)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if is_recording && let Some(started_at) = recording_started_at {
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
                                if let Ok(mut pref_guard) = runtime_preference_state.write() {
                                    *pref_guard = live_cfg.runtime.preference.clone();
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
                            if last.elapsed()
                                >= Duration::from_secs(idle_unload_minutes as u64 * 60)
                            {
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
                        info!(
                            "[MAIN] РЎР‚РЎСџРІР‚СљРўС’ Streaming partial text: \"{}\"",
                            text
                        );
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
                        HotkeyEvent::RecordStop {
                            hwnd: input::get_foreground_window_handle(),
                        }
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
                            if let Ok(mut pref_guard) = runtime_preference_state.write() {
                                *pref_guard = live_cfg.runtime.preference.clone();
                            }
                        }
                        let streaming_enabled = ui::is_streaming_enabled();
                        let chunk_seconds = ui::streaming_chunk_seconds();
                        let mut recording_backend_is_embedded = is_embedded;
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
                                    if let Some(new_model_path) = try_switch_to_next_fallback_model(
                                        &config_clone,
                                        &active_model_path_clone,
                                        &engine,
                                        &policy_fallback_models,
                                    ) {
                                        warn!(
                                            "[POLICY] Warmup fallback: switching to embedded model {:?}",
                                            new_model_path
                                        );
                                        recording_backend_is_embedded = true;
                                        live_backend = WhisperBackend::Embedded;
                                        whisper_ready = whisper_engine::is_engine_loaded(&engine);
                                        whisper_status_text = if whisper_ready {
                                            "Whisper: embedded fallback ready".to_string()
                                        } else {
                                            "Whisper: embedded fallback (load on first chunk)"
                                                .to_string()
                                        };
                                    } else {
                                        whisper_status_text = "Whisper: startup error".to_string();
                                    }
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
                            streaming_transcriber = Some(if recording_backend_is_embedded {
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
                    overlay_clone.update_status_text("Processing audio...");
                    overlay_clone.update_body_text("Preparing transcription task");

                    // CRITICAL: Stop streaming FIRST while recording is still active
                    // This allows streaming to read the final buffer before it's cleared
                    if let Some(mut st) = streaming_transcriber.take() {
                        info!(
                            "[MAIN] Stopping streaming transcription (while recorder still active)..."
                        );
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
                                            info!(
                                                "[MAIN] РЎР‚РЎСџРІР‚СљРўС’ Late partial text: \"{}\"",
                                                text
                                            );
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
                            info!(
                                "[MAIN] No final text received from streaming, using accumulated text"
                            );
                        }
                    }

                    // NOW stop recording (after streaming has processed final buffer)
                    info!("[MAIN] Calling recorder.stop_recording()...");
                    let mut audio_data = match recorder_clone.stop_recording() {
                        Ok(data) => {
                            info!("[MAIN] Got {} samples of audio", data.len());
                            data
                        }
                        Err(e) => {
                            error!("[MAIN] FAILED to stop recording: {}", e);
                            match recorder_clone.get_unprocessed_buffer() {
                                Ok((fallback_data, _)) if !fallback_data.is_empty() => {
                                    warn!(
                                        "[MAIN] Recovered {} samples from fallback buffer after stop failure",
                                        fallback_data.len()
                                    );
                                    fallback_data
                                }
                                Ok(_) => {
                                    error!("[MAIN] Fallback buffer is empty after stop failure");
                                    Vec::new()
                                }
                                Err(buf_err) => {
                                    error!(
                                        "[MAIN] Failed to read fallback buffer after stop failure: {}",
                                        buf_err
                                    );
                                    Vec::new()
                                }
                            }
                        }
                    };

                    if audio_data.is_empty() {
                        if let Ok((fallback_data, _)) = recorder_clone.get_unprocessed_buffer() {
                            if !fallback_data.is_empty() {
                                warn!(
                                    "[MAIN] stop_recording returned empty, recovered {} samples via fallback buffer",
                                    fallback_data.len()
                                );
                                audio_data = fallback_data;
                            }
                        }
                    }

                    if audio_data.is_empty() {
                        info!("No audio recorded (including fallback)");
                        if config_clone.history.enabled {
                            let base_mode = if ui::is_streaming_enabled() {
                                "streaming"
                            } else {
                                "full"
                            };
                            let mode = format!("{}|{}", base_mode, "empty_capture");
                            let fail_text = "[Recording failed] Empty capture after stop".to_string();
                            if let Err(err) = history.save_recording(
                                &audio_data,
                                &fail_text,
                                0.0,
                                &mode,
                                &config_clone.whisper.language,
                            ) {
                                error!("[MAIN] Failed to persist empty fallback recording: {}", err);
                            }
                        }
                        if !is_embedded {
                            whisper_manager.stop_if_owned();
                        }
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
                            let base_mode = if ui::is_streaming_enabled() {
                                "streaming"
                            } else {
                                "full"
                            };
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
                        let mut force_embedded_this_run = false;
                        if !is_embedded {
                            if let Err(e) = whisper_manager.ensure_running(Duration::from_secs(30))
                            {
                                error!("[MAIN] Failed to start Whisper server: {}", e);
                                overlay_clone.update_status_text("Server startup failed, trying local fallback...");
                                overlay_clone.update_body_text("Switching to embedded model");
                                whisper_manager.stop_if_owned();

                                if let Some(new_model_path) = try_switch_to_next_fallback_model(
                                    &config_clone,
                                    &active_model_path_clone,
                                    &engine,
                                    &policy_fallback_models,
                                ) {
                                    info!(
                                        "[POLICY] Switching to embedded recovery model after server start fail: {:?}",
                                        new_model_path
                                    );
                                    policy_stage =
                                        String::from("embedded_recovery_after_server_start_fail");
                                    policy_retry_count = policy_retry_count.saturating_add(1);
                                    force_embedded_this_run = true;
                                } else {
                                    overlay_clone.show("Whisper server startup error");
                                    std::thread::sleep(Duration::from_secs(2));
                                    overlay_clone.hide();
                                    save_failed_audio("server startup error", "server_start_failed");
                                    continue;
                                }
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
                            let result = if is_embedded || force_embedded_this_run {
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
                        let hard_timeout_secs = (expected_secs * 12.0).clamp(180.0, 1800.0);

                        let transcribed = loop {
                            match transcribe_rx.recv_timeout(Duration::from_millis(250)) {
                                Ok(result) => break result,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    let elapsed_secs = transcribe_started.elapsed().as_secs_f32();
                                    if elapsed_secs >= hard_timeout_secs {
                                        break Err(anyhow::anyhow!(
                                            "Transcription timeout after {:.1}s (limit {:.0}s)",
                                            elapsed_secs,
                                            hard_timeout_secs
                                        ));
                                    }
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
                                    avg_transcribe_ratio = (avg_transcribe_ratio * 0.7
                                        + observed_ratio * 0.3)
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
                                        overlay_clone
                                            .update_status_text("Retrying with fallback model...");

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
                                                policy_retry_count =
                                                    policy_retry_count.saturating_add(1);
                                                let retry_elapsed =
                                                    retry_started.elapsed().as_secs_f32();
                                                if audio_duration_secs > 0.1 {
                                                    let observed_ratio = (retry_elapsed
                                                        / audio_duration_secs)
                                                        .clamp(0.05, 2.0);
                                                    avg_transcribe_ratio = (avg_transcribe_ratio
                                                        * 0.7
                                                        + observed_ratio * 0.3)
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
                                                if policy_enable_server_fallback
                                                    .read()
                                                    .map(|v| *v)
                                                    .unwrap_or(false)
                                                {
                                                    if let Some(server_text) =
                                                        try_server_fallback_transcription(
                                                            &mut whisper_manager,
                                                            &audio_data,
                                                            &config_clone.whisper.language,
                                                            &overlay_clone,
                                                        )
                                                    {
                                                        policy_stage =
                                                            String::from("server_fallback");
                                                        server_fallback_used = true;
                                                        server_text
                                                    } else {
                                                        if policy_enable_cloud_fallback
                                                            .read()
                                                            .map(|v| *v)
                                                            .unwrap_or(false)
                                                        {
                                                            warn!(
                                                                "[POLICY] server fallback unavailable; cloud fallback planned but not implemented yet"
                                                            );
                                                        }
                                                        overlay_clone.hide();
                                                        save_failed_audio(
                                                            "embedded retry failed and server fallback unavailable",
                                                            "embedded_retry_failed",
                                                        );
                                                        continue;
                                                    }
                                                } else {
                                                    overlay_clone.hide();
                                                    save_failed_audio(
                                                        "embedded retry failed",
                                                        "embedded_retry_failed",
                                                    );
                                                    continue;
                                                }
                                            }
                                        }
                                    } else if policy_enable_server_fallback
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
                                            policy_stage = String::from("server_fallback");
                                            server_fallback_used = true;
                                            server_text
                                        } else {
                                            if policy_enable_cloud_fallback
                                                .read()
                                                .map(|v| *v)
                                                .unwrap_or(false)
                                            {
                                                warn!(
                                                    "[POLICY] no local/server fallback left; cloud fallback planned but not implemented yet"
                                                );
                                            }
                                            overlay_clone.hide();
                                            save_failed_audio(
                                                "server fallback unavailable",
                                                "server_fallback_unavailable",
                                            );
                                            continue;
                                        }
                                    } else {
                                        overlay_clone.hide();
                                        save_failed_audio(
                                            "embedded transcription failed",
                                            "embedded_failed",
                                        );
                                        continue;
                                    }
                                } else {
                                    whisper_manager.stop_if_owned();
                                    overlay_clone.hide();
                                    save_failed_audio(
                                        "server transcription failed",
                                        "server_failed",
                                    );
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
                            policy_enable_cloud_fallback
                                .read()
                                .map(|v| *v)
                                .unwrap_or(false),
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
                                                if let Some(server_text) =
                                                    try_server_fallback_transcription(
                                                        &mut whisper_manager,
                                                        &audio_data,
                                                        &config_clone.whisper.language,
                                                        &overlay_clone,
                                                    )
                                                {
                                                    policy_stage = String::from(
                                                        "server_fallback_quality_guard",
                                                    );
                                                    server_fallback_used = true;
                                                    raw_text = server_text
                                                        .split_whitespace()
                                                        .collect::<Vec<_>>()
                                                        .join(" ");
                                                }
                                            }
                                        } else {
                                            policy_stage =
                                                String::from("embedded_quality_guard_retry");
                                            policy_retry_count =
                                                policy_retry_count.saturating_add(1);
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
                    let llm_text = if ui::is_ollama_enabled() {
                        overlay_clone.update_status_text("Correcting...");
                        match ollama_clone.correct_text(&raw_text) {
                            Ok(corrected) => corrected,
                            Err(e) => {
                                error!("LLM correction error: {}", e);
                                raw_text.clone() // Use raw text if correction fails
                            }
                        }
                    } else {
                        info!("Ollama disabled in config, using raw transcription");
                        raw_text.clone()
                    };
                    corrections.learn_from_pair(&raw_text, &llm_text);
                    let final_text = corrections.apply(&llm_text);

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
                        policy_enable_cloud_fallback
                            .read()
                            .map(|v| *v)
                            .unwrap_or(false),
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

                    // Optional post-transcription overlay summary (configured in [ui] settings).
                    if config_clone.ui.show_post_transcription_overlay {
                        let preview = final_text
                            .split_whitespace()
                            .take(16)
                            .collect::<Vec<_>>()
                            .join(" ");
                        overlay_clone.show(&format!(
                            "Inserted: {} words | {} chars\n{}{}",
                            final_text.split_whitespace().count(),
                            final_text.chars().count(),
                            preview,
                            if final_text.split_whitespace().count() > 16 {
                                "..."
                            } else {
                                ""
                            }
                        ));
                    }

                    // Inject text into focused application
                    if let Err(e) = input::inject_text(&final_text, &config_clone.injection.method)
                    {
                        error!("Failed to inject text: {}", e);
                    }

                    // Hide overlay after configured confirmation period.
                    if config_clone.ui.show_post_transcription_overlay {
                        let secs = config_clone
                            .ui
                            .post_transcription_overlay_seconds
                            .clamp(1, 15) as u64;
                        std::thread::sleep(std::time::Duration::from_secs(secs));
                    }
                    overlay_clone.hide();

                    // Update last activity time for idle unload timer
                    last_transcription_time = Some(Instant::now());

                    // Save recording to history (if enabled)
                    if config_clone.history.enabled {
                        let duration_secs = audio_data.len() as f32 / 16000.0;
                        let base_mode = if ui::is_streaming_enabled() {
                            "streaming"
                        } else {
                            "full"
                        };
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
                        info!(
                            "[MAIN] РЎР‚РЎСџРІР‚СљРўС’ Streaming partial text: \"{}\"",
                            text
                        );
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
