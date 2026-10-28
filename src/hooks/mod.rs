pub mod environment;
pub mod spawner;
pub mod startup;
pub mod switch_events;
pub mod tray;

use anyhow::Result;
use tracing::info;

use crate::backend::BackendHandle;

pub use crate::backend::hooks::clear_backend;
pub use spawner::{Notification, ScreenshotEvent, Spawner};
pub use startup::StartupManager;
pub use tray::{TrayAction, TrayIcon};

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
    /// Self-contained tray actions (About) are handled inline
    /// here and never returned to the caller — that keeps main.rs's match
    /// arms unchanged when new menu items are added.  Caller-relevant
    /// actions (ShowHide, ReloadConfig, FocusPrevious, Quit) are forwarded
    /// upward so the engine can act on them.
    pub fn poll_tray_action(&mut self) -> Option<TrayAction> {
        loop {
            let action = self.tray.try_recv_action()?;
            match action {
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
                        }
                        Err(e) => {
                            tracing::warn!("Tray: screenshot failed: {}", e);
                            let _ = self
                                .tray
                                .show_balloon("wiri — screenshot failed", &e.to_string());
                        }
                    }
                    continue;
                }
                TrayAction::CaptureWindow => {
                    // Resolve the focused HWND via `GetForegroundWindow()` so
                    // we don't need the engine here.  Matches what the user
                    // sees focused on screen at the moment they click the
                    // menu entry.
                    let hwnd_isize: isize = unsafe {
                        windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize
                    };
                    if hwnd_isize == 0 {
                        tracing::warn!("Tray: CaptureWindow — no foreground window");
                        let _ = self
                            .tray
                            .show_balloon("wiri — capture failed", "No focused window to capture.");
                        continue;
                    }
                    match capture_focused_window_to_pictures(hwnd_isize) {
                        Ok(path) => {
                            tracing::info!("Tray: window capture saved to {}", path);
                        }
                        Err(e) => {
                            tracing::warn!("Tray: window capture failed: {}", e);
                            let _ = self
                                .tray
                                .show_balloon("wiri — capture failed", &e.to_string());
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

/// Shared PNG/clipboard pipeline for the hotkey and tray paths.
/// An empty result means screenshot-path null (clipboard only).
pub fn capture_screenshot_to_pictures() -> anyhow::Result<String> {
    Ok(crate::screenshot::capture(None, None)?.unwrap_or_default())
}

pub fn capture_focused_window_to_pictures(hwnd: isize) -> anyhow::Result<String> {
    Ok(crate::screenshot::capture(Some(hwnd), None)?.unwrap_or_default())
}
