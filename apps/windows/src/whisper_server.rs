use anyhow::{Context, Result};
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
    child: Option<Child>,
    owns_process: bool,
}

impl WhisperServerManager {
    pub fn new(model_path: String) -> Self {
        Self {
            model_path,
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
            let model_arg = if std::path::Path::new(&self.model_path).exists() {
                Some(self.model_path.as_str())
            } else {
                warn!(
                    "[WHISPER] Configured model path does not exist: {}. Using server default path.",
                    self.model_path
                );
                None
            };
            info!(
                "[WHISPER] Starting local server: {} {}",
                script.display(),
                model_arg.unwrap_or("<server-default>")
            );

            let child = spawn_server_process(&script, model_arg)
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

fn spawn_server_process(script: &Path, model_path: Option<&str>) -> Result<Child> {
    // pythonw prevents a visible console window; fallback to python if unavailable.
    let candidates = ["pythonw", "python"];
    let mut last_error: Option<anyhow::Error> = None;

    for exe in candidates {
        let mut cmd = Command::new(exe);
        cmd.arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if let Some(path) = model_path {
            cmd.arg(path);
        }

        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) => {
                last_error = Some(anyhow::anyhow!("{}: {}", exe, e));
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
    for start in [exe_dir.as_deref(), Some(cwd.as_path())].into_iter().flatten() {
        let mut dir = start.to_path_buf();
        for _ in 0..6 {
            candidates.push(dir.join("whisper_server.py"));
            candidates.push(dir.join("shared").join("whisper-server").join("whisper_server.py"));
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
