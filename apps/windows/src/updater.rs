//! Background update checker using Velopack + GitHub Releases.
//!
//! - At startup: `VelopackApp::build().run()` must be called before any other code.
//! - After startup: call `check_for_updates_async()` to kick off a background check.
//!
//! When an update is available the tray menu item becomes active.
//! The user clicks it → `download_and_restart()` is called.

use tracing::{info, warn};
use velopack::sources::HttpSource;
use velopack::{UpdateCheck, UpdateManager, VelopackApp};

/// GitHub Releases URL where velopack packages are hosted.
/// The updater will fetch `{URL}/releases.stable.json` for the feed.
const GITHUB_RELEASES_URL: &str = "https://github.com/iamniketas/dictator/releases/latest/download";

/// **Must be called at the very beginning of `main()`**, before anything else.
///
/// Velopack may apply a pending update and restart the process — the function
/// exits the process in that case, so it never returns to the caller.
pub fn startup() {
    VelopackApp::build().run();
}

/// Spawn a background thread that checks for a new release on GitHub.
///
/// If an update is found, calls `on_update_found(version_string)`.
/// Silent on errors (no internet, no releases yet, dev build, etc.).
pub fn check_for_updates_async(on_update_found: impl Fn(String) + Send + 'static) {
    std::thread::spawn(move || {
        if let Err(e) = check_inner(on_update_found) {
            // Errors are expected in dev mode (app not installed via velopack)
            info!("[UPDATER] Update check skipped or failed: {}", e);
        }
    });
}

fn check_inner(on_update_found: impl Fn(String)) -> anyhow::Result<()> {
    let source = HttpSource::new(GITHUB_RELEASES_URL);
    let mgr = UpdateManager::new(source, None, None)?;

    match mgr.check_for_updates() {
        Ok(UpdateCheck::UpdateAvailable(update)) => {
            let version = update.TargetFullRelease.Version.clone();
            info!("[UPDATER] Update available: v{}", version);
            on_update_found(version);
        }
        Ok(UpdateCheck::NoUpdateAvailable) => {
            info!("[UPDATER] Already up-to-date.");
        }
        Ok(UpdateCheck::RemoteIsEmpty) => {
            info!("[UPDATER] Remote feed is empty.");
        }
        Err(e) => {
            warn!("[UPDATER] check_for_updates error: {}", e);
        }
    }
    Ok(())
}

/// Download the pending update and restart the application.
///
/// This should be called on a background thread — it may take a while to download.
/// After the download completes the process is replaced by the new version.
pub fn download_and_restart(version: &str) {
    let source = HttpSource::new(GITHUB_RELEASES_URL);
    let Ok(mgr) = UpdateManager::new(source, None, None) else {
        warn!("[UPDATER] Could not create UpdateManager for download");
        return;
    };

    let Ok(UpdateCheck::UpdateAvailable(update)) = mgr.check_for_updates() else {
        warn!("[UPDATER] Update disappeared before download ({})", version);
        return;
    };

    info!("[UPDATER] Downloading update v{}...", version);
    if let Err(e) = mgr.download_updates(&update, None) {
        warn!("[UPDATER] Download failed: {}", e);
        return;
    }

    info!("[UPDATER] Applying update and restarting...");
    if let Err(e) = mgr.apply_updates_and_restart(&update) {
        warn!("[UPDATER] Apply failed: {}", e);
    }
}
