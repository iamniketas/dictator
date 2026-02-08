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
        if Self::is_healthy() {
            return Ok(());
        }

        if self.child.is_none() {
            let script = find_server_script()?;
            info!(
                "[WHISPER] Starting local server: {} {}",
                script.display(),
                self.model_path
            );

            let mut cmd = Command::new("python");
            cmd.arg(script)
                .arg(&self.model_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            #[cfg(target_os = "windows")]
            {
                cmd.creation_flags(CREATE_NO_WINDOW);
            }

            let child = cmd.spawn().context("Failed to spawn whisper_server.py")?;
            self.child = Some(child);
            self.owns_process = true;
        }

        let start = Instant::now();
        while start.elapsed() < startup_timeout {
            if Self::is_healthy() {
                info!("[WHISPER] Server is ready");
                return Ok(());
            }

            if let Some(child) = self.child.as_mut() {
                if let Some(status) = child.try_wait().context("Failed to poll server process")? {
                    self.child = None;
                    self.owns_process = false;
                    anyhow::bail!("Whisper server exited early with status: {}", status);
                }
            }

            std::thread::sleep(Duration::from_millis(250));
        }

        anyhow::bail!(
            "Timed out waiting for Whisper server to become healthy after {:?}",
            startup_timeout
        )
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

impl Drop for WhisperServerManager {
    fn drop(&mut self) {
        self.stop_if_owned();
    }
}

fn find_server_script() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(Path::parent).map(PathBuf::from);

    let mut candidates = vec![cwd.join("whisper_server.py")];
    if let Some(dir) = exe_dir {
        candidates.push(dir.join("whisper_server.py"));
        if let Some(parent) = dir.parent() {
            candidates.push(parent.join("whisper_server.py"));
            if let Some(grand) = parent.parent() {
                candidates.push(grand.join("whisper_server.py"));
            }
        }
    }

    for path in candidates {
        if path.exists() {
            return Ok(path);
        }
    }

    anyhow::bail!("whisper_server.py not found in expected locations")
}
