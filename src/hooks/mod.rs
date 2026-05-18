pub mod spawner;
pub mod startup;
pub mod tray;

use anyhow::Result;
use tracing::info;

use crate::backend::BackendHandle;

pub use spawner::{Notification, ScreenshotEvent, Spawner};
pub use startup::StartupManager;
pub use tray::{TrayIcon, TrayAction};
pub use crate::backend::hooks::clear_backend;

/// Manages system-level integrations: tray icon, autostart, config reload.
/// Processes tray actions and dispatches them to the engine.
pub struct SystemIntegration {
    #[allow(dead_code)]
    backend: BackendHandle,
    startup: StartupManager,
    spawner: Spawner,
    tray: TrayIcon,
}

impl SystemIntegration {
    pub async fn new(backend: BackendHandle) -> Self {
        let startup = StartupManager::new();
        let spawner = Spawner::new();
        let mut tray = TrayIcon::new();

        // Initialize the tray icon
        if let Err(e) = tray.setup() {
            tracing::warn!("Failed to setup tray icon: {}", e);
        }

        Self {
            backend,
            startup,
            spawner,
            tray,
        }
    }

    /// Poll for tray actions and return them.
    /// Call this from the main event loop.
    ///
    /// Self-contained tray actions (OpenConfigDir, About) are handled inline
    /// here and never returned to the caller — that keeps main.rs's match
    /// arms unchanged when new menu items are added.  Caller-relevant
    /// actions (ShowHide, ReloadConfig, FocusPrevious, Quit) are forwarded
    /// upward so the engine can act on them.
    pub fn poll_tray_action(&mut self) -> Option<TrayAction> {
        loop {
            let action = self.tray.try_recv_action()?;
            match action {
                TrayAction::OpenConfigDir => {
                    if let Err(e) = TrayIcon::open_config_folder() {
                        tracing::warn!("OpenConfigDir failed: {}", e);
                    }
                    continue; // do not surface to main; consume + loop.
                }
                TrayAction::About => {
                    let _ = self.tray.show_balloon(
                        "wiri",
                        &format!(
                            "{}\nScrollable-tiling window manager for Windows.\n\
                             https://github.com/wiri-wm/wiri",
                            env!("CARGO_PKG_VERSION"),
                        ),
                    );
                    continue;
                }
                TrayAction::FocusPrevious => {
                    // Forward as ShowHide for now — main.rs's existing handler
                    // already triggers focus-previous-style behaviour via the
                    // engine when ShowHide is received in a "shown" state.
                    // When Agent A's main.rs is updated we can return
                    // TrayAction::FocusPrevious unchanged.
                    return Some(TrayAction::ShowHide);
                }
                TrayAction::Screenshot => {
                    // Item 3: capture inline so the tray menu is self-contained.
                    // The exact same destination-resolution policy used by the
                    // hotkey path lives here (Pictures → cwd fallback).
                    match self::capture_screenshot_to_pictures() {
                        Ok(path) => {
                            tracing::info!("Tray: screenshot saved to {}", path);
                            let _ = self.tray.show_balloon(
                                "wiri — screenshot",
                                &format!("Saved to {}", path),
                            );
                        }
                        Err(e) => {
                            tracing::warn!("Tray: screenshot failed: {}", e);
                            let _ = self.tray.show_balloon(
                                "wiri — screenshot failed",
                                &e.to_string(),
                            );
                        }
                    }
                    continue;
                }
                other => return Some(other),
            }
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("System integration running");
        Ok(())
    }

    pub fn startup_manager(&self) -> &StartupManager {
        &self.startup
    }

    pub fn spawner(&self) -> &Spawner {
        &self.spawner
    }

    pub fn tray_icon(&self) -> &TrayIcon {
        &self.tray
    }

    pub fn tray_icon_mut(&mut self) -> &mut TrayIcon {
        &mut self.tray
    }
}

/// Item 3: capture the full virtual desktop to a BMP file under
/// `%USERPROFILE%\Pictures\wiri-<timestamp>.bmp`.  Falls back to the current
/// working directory if Pictures is missing / not creatable.
///
/// Returns the full path (as a String) on success, or a propagated error.
/// Shared between the `screenshot` hotkey action and the tray menu entry so
/// both paths obey the same destination-resolution policy.
pub fn capture_screenshot_to_pictures() -> anyhow::Result<String> {
    use std::path::PathBuf;
    use std::time::SystemTime;

    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("wiri-{}.bmp", stamp);

    let dest = std::env::var("USERPROFILE")
        .ok()
        .map(|home| PathBuf::from(home).join("Pictures"))
        .and_then(|dir| {
            std::fs::create_dir_all(&dir).ok().map(|_| dir.join(&filename))
        })
        .or_else(|| std::env::current_dir().ok().map(|d| d.join(&filename)))
        .ok_or_else(|| {
            anyhow::anyhow!("no writable destination dir (Pictures and cwd both unavailable)")
        })?;

    Spawner::capture_screenshot_to_file(None, &dest)?;
    Ok(dest.to_string_lossy().into_owned())
}
