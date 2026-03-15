use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tracing::{info, warn};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const SERVER_URL: &str = "http://127.0.0.1:5000/health";
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct WhisperServerManager {
    model_path: String,
    python_executable: Option<PathBuf>,
    runtime_hint: Option<String>,
    preferred_device: Option<String>,
    child: Option<Child>,
    owns_process: bool,
}

impl WhisperServerManager {
    pub fn new(model_path: String) -> Self {
        let (python_executable, runtime_hint) = resolve_runtime_profile_metadata(&model_path);
        Self {
            model_path,
            python_executable,
            runtime_hint,
            preferred_device: None,
            child: None,
            owns_process: false,
        }
    }

    pub fn ensure_running(&mut self, startup_timeout: Duration) -> Result<()> {
        self.start_if_needed()?;

        let start = Instant::now();
        while start.elapsed() < startup_timeout {
            if self.poll_ready()? {
                info!("[WHISPER] Server is ready");
                return Ok(());
            }

            std::thread::sleep(Duration::from_millis(250));
        }

        anyhow::bail!(
            "Timed out waiting for Whisper server to become healthy after {:?}",
            startup_timeout
        )
    }

    pub fn start_if_needed(&mut self) -> Result<()> {
        if Self::is_healthy() {
            return Ok(());
        }

        if self.child.is_none() {
            let script = find_server_script()?;
            let model_arg = if self.model_path.trim().is_empty() {
                warn!("[WHISPER] Model path is empty. Using server default path.");
                None
            } else {
                Some(self.model_path.as_str())
            };
            info!(
                "[WHISPER] Starting local server: {} {}",
                script.display(),
                model_arg.unwrap_or("<server-default>")
            );

            let child = spawn_server_process(
                &script,
                model_arg,
                self.python_executable.as_deref(),
                self.preferred_device.as_deref(),
                self.runtime_hint.as_deref(),
            )
                .context("Failed to spawn whisper_server.py")?;
            self.child = Some(child);
            self.owns_process = true;
        }

        Ok(())
    }

    pub fn poll_ready(&mut self) -> Result<bool> {
        if Self::is_healthy() {
            return Ok(true);
        }

        if let Some(child) = self.child.as_mut()
            && let Some(status) = child.try_wait().context("Failed to poll server process")?
        {
            self.child = None;
            self.owns_process = false;
            anyhow::bail!("Whisper server exited early with status: {}", status);
        }

        Ok(false)
    }

    pub fn stop_if_owned(&mut self) {
        if !self.owns_process {
            return;
        }

        if let Some(mut child) = self.child.take() {
            match child.kill() {
                Ok(_) => {
                    let _ = child.wait();
                    info!("[WHISPER] Server stopped");
                }
                Err(e) => {
                    warn!("[WHISPER] Failed to stop server process: {}", e);
                }
            }
        }

        self.owns_process = false;
    }

    pub fn set_model_path(&mut self, model_path: String) {
        self.model_path = model_path;
        let (python_executable, runtime_hint) = resolve_runtime_profile_metadata(&self.model_path);
        self.python_executable = python_executable;
        self.runtime_hint = runtime_hint;
    }

    pub fn set_preferred_device(&mut self, preferred_device: Option<String>) {
        self.preferred_device = preferred_device;
    }

    pub fn is_server_running(&self) -> bool {
        self.child.is_some()
    }

    pub fn is_healthy() -> bool {
        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(800))
            .no_proxy()
            .build()
        {
            Ok(c) => c,
            Err(_) => return false,
        };

        match client.get(SERVER_URL).send() {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }
}

fn spawn_server_process(
    script: &Path,
    model_path: Option<&str>,
    python_executable: Option<&Path>,
    preferred_device: Option<&str>,
    runtime_hint: Option<&str>,
) -> Result<Child> {
    // On Windows we only use GUI Python launchers to avoid any console flash.
    #[cfg(target_os = "windows")]
    let mut candidates: Vec<PathBuf> = vec![PathBuf::from("pythonw"), PathBuf::from("pyw")];
    #[cfg(not(target_os = "windows"))]
    let mut candidates: Vec<PathBuf> = vec![PathBuf::from("python3"), PathBuf::from("python")];
    if let Some(py) = python_executable {
        candidates.insert(0, py.to_path_buf());
    }
    let mut last_error: Option<anyhow::Error> = None;
    let whisper_log = resolve_whisper_server_log_file();
    let stderr_writer = whisper_log
        .as_ref()
        .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
    let stdout_writer = whisper_log
        .as_ref()
        .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());

    for exe in candidates {
        let mut cmd = Command::new(&exe);
        cmd.arg(script)
            .stdin(Stdio::null())
            .stdout(
                stdout_writer
                    .as_ref()
                    .and_then(|f| f.try_clone().ok())
                    .map(Stdio::from)
                    .unwrap_or(Stdio::null()),
            )
            .stderr(
                stderr_writer
                    .as_ref()
                    .and_then(|f| f.try_clone().ok())
                    .map(Stdio::from)
                    .unwrap_or(Stdio::null()),
            );

        if let Some(path) = model_path {
            cmd.arg(path);
        }
        if let Some(device) = preferred_device {
            cmd.env("WHISPER_DEVICE", device);
        }
        if let Some(runtime) = runtime_hint {
            cmd.env("WHISPER_RUNTIME", runtime);
        }

        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) => {
                last_error = Some(anyhow::anyhow!("{}: {}", exe.display(), e));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No python executable found")))
}

impl Drop for WhisperServerManager {
    fn drop(&mut self) {
        self.stop_if_owned();
    }
}

fn find_server_script() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(Path::parent).map(PathBuf::from);

    let mut candidates: Vec<PathBuf> = Vec::new();

    // Walk up from exe dir and cwd, checking both old location and new shared/ location
    for start in [exe_dir.as_deref(), Some(cwd.as_path())]
        .into_iter()
        .flatten()
    {
        let mut dir = start.to_path_buf();
        for _ in 0..6 {
            candidates.push(dir.join("whisper_server.py"));
            candidates.push(
                dir.join("shared")
                    .join("whisper-server")
                    .join("whisper_server.py"),
            );
            match dir.parent().map(PathBuf::from) {
                Some(p) if p != dir => dir = p,
                _ => break,
            }
        }
    }

    for path in candidates {
        if path.exists() {
            info!("[WHISPER] Found server script at: {}", path.display());
            return Ok(path);
        }
    }

    anyhow::bail!("whisper_server.py not found in expected locations")
}

#[derive(Debug, Deserialize)]
struct RuntimeProfileMarker {
    #[serde(alias = "ModelId", alias = "model_id")]
    model_id: Option<String>,
    #[serde(alias = "ExecutionModelRef", alias = "execution_model_ref")]
    execution_model_ref: Option<String>,
    #[serde(alias = "LocalModelPath", alias = "local_model_path")]
    local_model_path: Option<String>,
    #[serde(alias = "PythonPath", alias = "python_path")]
    python_path: Option<String>,
}

fn resolve_runtime_profile_metadata(model_path: &str) -> (Option<PathBuf>, Option<String>) {
    let path = Path::new(model_path);
    let model_id = path
        .file_name()
        .and_then(|v| v.to_str())
        .map(|s| s.to_string());
    let Some(model_id) = model_id else {
        return (None, None);
    };

    let profiles_dir = hardware_profiler::default_audio_models_dir()
        .join("runtimes")
        .join("profiles");
    let marker_path = profiles_dir.join(format!("{}.json", model_id));
    let raw = match fs::read_to_string(marker_path) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let marker = match serde_json::from_str::<RuntimeProfileMarker>(&raw) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };

    let mut python = None;
    if let Some(py) = marker.python_path {
        let py_path = PathBuf::from(py);
        if py_path.exists() {
            info!(
                "[WHISPER] Using runtime profile python for model {}: {}",
                marker.model_id.as_deref().unwrap_or(&model_id),
                py_path.display()
            );
            python = Some(py_path);
        }
    }

    // If marker matches different local path, do not force wrong python/runtime hint.
    if let Some(local_path) = marker.local_model_path
        && Path::new(&local_path) != path
    {
        return (python, None);
    }

    let runtime_hint = marker
        .execution_model_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|model_ref| {
            let lower = model_ref.to_ascii_lowercase();
            if lower.starts_with("nvidia/parakeet")
                || lower.starts_with("nvidia/canary")
                || lower.starts_with("ibm-granite/")
            {
                Some(String::from("transformers"))
            } else {
                None
            }
        });

    (python, runtime_hint)
}

fn resolve_whisper_server_log_file() -> Option<PathBuf> {
    let log_dir = dirs::data_dir()?.join("dictator").join("logs");
    let _ = fs::create_dir_all(&log_dir);
    Some(log_dir.join("whisper-server.log"))
}
