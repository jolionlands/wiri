pub mod accent;
pub mod aumid;
pub mod cbt_hook;
pub mod cloak;
pub mod hidden;
pub mod hooks;
pub mod hotkey_conflicts;
pub mod message_loop;
// Lane D: on-demand output identity/config and event-driven monitor refresh.
pub mod output_runtime;
pub mod outputs;
pub mod taskbar;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// How long a self-applied entry is considered "recent" (suppresses the hook).
const SELF_APPLIED_RECENT_MS: u128 = 250;
/// Stale entries are pruned when older than this (prevents unbounded growth).
const SELF_APPLIED_PRUNE_MS: u128 = 1000;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Graphics::Gdi::{RedrawWindow, RDW_ALLCHILDREN, RDW_INVALIDATE};
#[cfg_attr(test, allow(unused_imports))]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, MoveWindow, PostMessageW,
    SetForegroundWindow, SetWindowPos, ShowWindow, ShowWindowAsync, SwitchToThisWindow, HWND_TOP,
    SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOWNA, WM_CLOSE,
};

/// Inject a harmless key press (F24, which no keyboard has and nothing binds).
///
/// Windows grants the right to call `SetForegroundWindow` to the process that
/// injected the most recent input, so a synthetic key event lifts the
/// foreground lock for the activation request that follows.  F24 is used
/// instead of the classic Alt tap because a lone Alt press activates the menu
/// bar of the window that receives it (and would trip the double-Alt detector).
fn nudge_foreground_lock() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    };
    const VK_F24: u8 = 0x87;
    unsafe {
        keybd_event(VK_F24, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_F24, 0, KEYEVENTF_KEYUP, 0);
    }
}

/// Ask a window to repaint after a tile move. Fixes apps (Cascadia/Windows
/// Terminal, Edge, some Electron) that show a black client area after
/// SetWindowPos because they don't process WM_SIZE eagerly.
///
/// Only *invalidates*: `RDW_UPDATENOW` would send WM_PAINT synchronously to the
/// other process, and a slow or hung application then froze wiri while it held
/// the engine lock - which is also what froze the IPC server for minutes.
fn request_redraw(hwnd: HWND) {
    unsafe {
        let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
}

use crate::utils::{OutputId, Rect};
pub use hooks::WinEventHook;
pub use message_loop::MessageLoop;

/// Spec-aligned alias for the concrete Win32 backend.
/// New code should prefer this name; existing call-sites continue to use
/// `Backend` for now.
pub type WindowsBackend = Backend;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}

#[derive(Debug, Clone)]
pub enum BackendEvent {
    WindowCreated {
        hwnd: isize,
    },
    WindowDestroyed {
        hwnd: isize,
    },
    WindowMoved {
        hwnd: isize,
        rect: Rect,
    },
    WindowResized {
        hwnd: isize,
        rect: Rect,
    },
    WindowShown {
        hwnd: isize,
    },
    WindowHidden {
        hwnd: isize,
    },
    MonitorConnected {
        id: u64,
    },
    MonitorDisconnected {
        id: u64,
    },
    ForegroundChanged {
        hwnd: isize,
    },
    WindowTitleChanged {
        hwnd: isize,
    },
    /// A window the engine tracks became (or stopped being) natively maximized, or as big
    /// as its monitor, on its own (not through wiri): an application going fullscreen (a
    /// video player, F11 in a browser) or its title bar's maximize button.
    WindowSizeClassChanged {
        hwnd: isize,
        zoomed: bool,
        covers_monitor: bool,
    },
    // CBT Hook events (fire BEFORE window operations)
    CbtCreateWindow {
        hwnd: isize,
    },
    CbtDestroyWindow {
        hwnd: isize,
    },
    CbtActivate {
        hwnd: isize,
    },
    CbtMoveSize {
        hwnd: isize,
    },
    CbtSetFocus {
        hwnd: isize,
    },
    /// Fires when the user starts a manual move/resize via window chrome.
    WindowMoveResizeStart {
        hwnd: isize,
    },
    /// Fires when the user finishes a manual move/resize via window chrome.
    WindowMoveResizeEnd {
        hwnd: isize,
    },
    /// Fires when a window enters the urgent / attention-demanding state.
    /// Sourced from EVENT_SYSTEM_ALERT (FlashWindow / FlashWindowEx) and
    /// EVENT_OBJECT_STATECHANGE (accessibility alert state).  `urgent: true`
    /// means "now flashing"; callers should highlight the window in the
    /// taskbar / border.  `urgent: false` is emitted when the engine focuses
    /// the window (i.e. the user has acknowledged it).
    WindowUrgent {
        hwnd: isize,
        urgent: bool,
    },
    /// Fired on EVENT_SYSTEM_FOREGROUND for a window that was previously
    /// in the urgent set — signals that the urgent state was implicitly
    /// cleared by the user switching to that window.
    WindowUrgentCleared {
        hwnd: isize,
    },
    /// EVENT_SYSTEM_MINIMIZESTART: the window is about to be minimized.
    WindowMinimized {
        hwnd: isize,
    },
    /// EVENT_SYSTEM_MINIMIZEEND: the window is about to be restored.
    WindowRestored {
        hwnd: isize,
    },
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub class_name: String,
    pub process_id: u32,
    pub bounds: Rect,
    pub state: WindowState,
    pub is_visible: bool,
}

impl WindowInfo {
    pub fn new(hwnd: HWND) -> Self {
        let mut title = [0u16; 512];
        let mut class_name = [0u16; 256];
        let title_len = unsafe { GetWindowTextW(hwnd, &mut title) };
        let class_len = unsafe { GetClassNameW(hwnd, &mut class_name) };
        let mut rect = RECT::default();
        let mut process_id = 0u32;
        unsafe {
            let _ = GetWindowRect(hwnd, &mut rect);
            GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        }
        let bounds = Rect::new(
            rect.left,
            rect.top,
            (rect.right - rect.left) as u32,
            (rect.bottom - rect.top) as u32,
        );
        Self {
            hwnd: hwnd.0 as isize,
            title: String::from_utf16_lossy(&title[..title_len as usize]),
            class_name: String::from_utf16_lossy(&class_name[..class_len as usize]),
            process_id,
            bounds,
            state: WindowState::Normal,
            is_visible: unsafe { IsWindowVisible(hwnd).as_bool() },
        }
    }

    pub fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut std::ffi::c_void)
    }

    pub fn refresh_bounds(&mut self) {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd(), &mut rect);
        }
        if let Some(true_bounds) = self.get_dwm_bounds() {
            self.bounds = true_bounds;
        } else {
            self.bounds = Rect::new(
                rect.left,
                rect.top,
                (rect.right - rect.left) as u32,
                (rect.bottom - rect.top) as u32,
            );
        }
    }

    pub fn get_dwm_bounds(&self) -> Option<Rect> {
        let mut rect = RECT::default();
        let hr = unsafe {
            DwmGetWindowAttribute(
                self.hwnd(),
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut _ as *mut _,
                std::mem::size_of::<RECT>() as u32,
            )
        };
        if hr.is_ok() {
            Some(Rect::new(
                rect.left,
                rect.top,
                (rect.right - rect.left) as u32,
                (rect.bottom - rect.top) as u32,
            ))
        } else {
            None
        }
    }
}

fn should_cache_window_info(info: &WindowInfo) -> bool {
    !info.class_name.is_empty()
        && !info.class_name.starts_with("Shell_")
        && info.class_name != "Progman"
        && info.class_name != "WorkerW"
}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub id: OutputId,
    pub name: String,
    pub bounds: Rect,
    pub work_area: Rect,
    pub is_primary: bool,
    /// DPI scaling factor (1.0 = 96 DPI, 1.5 = 144 DPI, 2.0 = 192 DPI)
    pub scale_factor: f64,
}

impl MonitorInfo {
    pub fn from_hmonitor(hmonitor: windows::Win32::Graphics::Gdi::HMONITOR) -> Option<Self> {
        unsafe {
            let mut info: MONITORINFOEXW = std::mem::zeroed();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

            // GetMonitorInfoW takes MONITORINFO but MONITORINFOEXW is binary-compatible
            let info_ptr = &mut info as *mut MONITORINFOEXW as *mut MONITORINFO;
            if GetMonitorInfoW(hmonitor, info_ptr).as_bool() {
                let device_name = String::from_utf16_lossy(
                    &info.szDevice[..info
                        .szDevice
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(info.szDevice.len())],
                );
                let id = OutputId::from_name(&device_name);
                let bounds = Rect::new(
                    info.monitorInfo.rcMonitor.left,
                    info.monitorInfo.rcMonitor.top,
                    (info.monitorInfo.rcMonitor.right - info.monitorInfo.rcMonitor.left) as u32,
                    (info.monitorInfo.rcMonitor.bottom - info.monitorInfo.rcMonitor.top) as u32,
                );
                let work_area = Rect::new(
                    info.monitorInfo.rcWork.left,
                    info.monitorInfo.rcWork.top,
                    (info.monitorInfo.rcWork.right - info.monitorInfo.rcWork.left) as u32,
                    (info.monitorInfo.rcWork.bottom - info.monitorInfo.rcWork.top) as u32,
                );
                // MONITORINFOF_PRIMARY = 1
                let is_primary = (info.monitorInfo.dwFlags & 1) != 0;

                // Get DPI scaling for this monitor
                let scale_factor = {
                    let mut dpi_x: u32 = 96;
                    let mut dpi_y: u32 = 96;
                    let hr = windows::Win32::UI::HiDpi::GetDpiForMonitor(
                        hmonitor,
                        windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
                        &mut dpi_x,
                        &mut dpi_y,
                    );
                    if hr.is_ok() {
                        dpi_x as f64 / 96.0
                    } else {
                        1.0
                    }
                };

                Some(Self {
                    id,
                    name: device_name,
                    bounds,
                    work_area,
                    is_primary,
                    scale_factor,
                })
            } else {
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderResult {
    Submitted,
    NoDamage,
    Skipped,
}

#[derive(Debug)]
pub struct Backend {
    windows: Arc<RwLock<HashMap<isize, WindowInfo>>>,
    monitors: Arc<RwLock<HashMap<OutputId, MonitorInfo>>>,
    event_tx: mpsc::UnboundedSender<BackendEvent>,
    event_rx: RwLock<Option<mpsc::UnboundedReceiver<BackendEvent>>>,
    running: Arc<parking_lot::Mutex<bool>>,
    #[allow(dead_code)]
    winevent_hook: Option<WinEventHook>,
    #[allow(dead_code)]
    cbt_hook: Option<cbt_hook::CbtHook>,
    /// Tracks HWNDs whose position we just set ourselves so that the
    /// EVENT_OBJECT_LOCATIONCHANGE hook can skip the self-triggered event.
    self_applied: Arc<RwLock<HashMap<isize, std::time::Instant>>>,
}

impl Backend {
    pub fn new() -> Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let backend = Self {
            windows: Arc::new(RwLock::new(HashMap::new())),
            monitors: Arc::new(RwLock::new(HashMap::new())),
            event_tx: event_tx.clone(),
            event_rx: RwLock::new(Some(event_rx)),
            running: Arc::new(parking_lot::Mutex::new(false)),
            winevent_hook: None,
            cbt_hook: None,
            self_applied: Arc::new(RwLock::new(HashMap::new())),
        };

        // Enumerate existing monitors
        let monitors_clone = backend.monitors.clone();
        backend.enumerate_monitors(monitors_clone)?;

        // Enumerate existing windows
        let windows_clone = backend.windows.clone();
        backend.enumerate_windows(windows_clone)?;

        let handle = backend.handle();
        hooks::set_backend(handle.clone());

        // Setup WinEventHook for AFTER events
        let hook = WinEventHook::new(handle.clone())?;
        hook.run();

        // Setup CBT hook for BEFORE events (optional - don't fail if it doesn't work)
        let cbt = match cbt_hook::CbtHook::new(handle.clone()) {
            Ok(cbt_hook) => {
                if cbt_hook.is_active() {
                    info!("CBT hook active - can intercept window creation before display");
                } else {
                    info!("CBT hook not available - using WinEventHook only");
                }
                Some(cbt_hook)
            }
            Err(e) => {
                info!("CBT hook not available: {} - using WinEventHook only", e);
                None
            }
        };

        let mut result = backend;
        result.winevent_hook = Some(hook);
        result.cbt_hook = cbt;

        Ok(result)
    }

    fn enumerate_monitors(
        &self,
        monitors: Arc<RwLock<HashMap<OutputId, MonitorInfo>>>,
    ) -> Result<()> {
        unsafe {
            let ok = EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(Self::monitor_enum_callback),
                LPARAM(&*monitors as *const _ as isize),
            );
            if !ok.as_bool() {
                warn!("EnumDisplayMonitors failed");
            }
        }
        info!("Enumerated {} monitors", monitors.read().len());
        Ok(())
    }

    unsafe extern "system" fn monitor_enum_callback(
        hmonitor: windows::Win32::Graphics::Gdi::HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let monitors = &*(lparam.0 as *const RwLock<HashMap<OutputId, MonitorInfo>>);

        if let Some(info) = MonitorInfo::from_hmonitor(hmonitor) {
            monitors.write().insert(info.id, info);
        }

        BOOL(1) // Continue enumeration
    }

    fn enumerate_windows(&self, windows: Arc<RwLock<HashMap<isize, WindowInfo>>>) -> Result<()> {
        unsafe {
            EnumWindows(
                Some(Self::window_enum_callback),
                LPARAM(&*windows as *const _ as isize),
            )
            .context("EnumWindows failed")?;
        }
        info!("Enumerated {} windows", windows.read().len());
        Ok(())
    }

    unsafe extern "system" fn window_enum_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = &*(lparam.0 as *const RwLock<HashMap<isize, WindowInfo>>);

        // Only include visible windows
        if IsWindowVisible(hwnd).as_bool() {
            let info = WindowInfo::new(hwnd);
            // Skip tool windows and other special windows
            if should_cache_window_info(&info) {
                windows.write().insert(info.hwnd, info);
            }
        }

        BOOL(1) // Continue enumeration
    }

    pub fn handle(&self) -> BackendHandle {
        BackendHandle {
            event_tx: self.event_tx.clone(),
            windows: self.windows.clone(),
            monitors: self.monitors.clone(),
            running: self.running.clone(),
            self_applied: self.self_applied.clone(),
        }
    }

    pub fn take_event_rx(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<BackendEvent> {
        self.event_rx
            .write()
            .take()
            .expect("event_rx already taken")
    }

    /// Run the backend message loop. This blocks until the message loop exits.
    /// Event processing is done externally via take_event_rx().
    pub async fn run(&mut self) -> Result<()> {
        {
            let mut running = self.running.lock();
            *running = true;
        }
        info!("Backend message loop started");
        MessageLoop::new()?.run().await
    }
}

#[derive(Debug, Clone)]
pub struct BackendHandle {
    event_tx: mpsc::UnboundedSender<BackendEvent>,
    windows: Arc<RwLock<HashMap<isize, WindowInfo>>>,
    monitors: Arc<RwLock<HashMap<OutputId, MonitorInfo>>>,
    running: Arc<parking_lot::Mutex<bool>>,
    /// Self-applied position tracker: hwnd → time of last wiri-issued SetWindowPos.
    /// Shared with the WinEvent hook thread to suppress spurious LOCATIONCHANGE events.
    self_applied: Arc<RwLock<HashMap<isize, std::time::Instant>>>,
}

impl BackendHandle {
    pub fn is_running(&self) -> bool {
        *self.running.lock()
    }

    /// Create a no-op BackendHandle for testing purposes
    #[cfg(test)]
    pub fn default_for_test() -> Self {
        let (event_tx, _) = tokio::sync::mpsc::unbounded_channel();
        Self {
            event_tx,
            windows: Arc::new(RwLock::new(HashMap::new())),
            monitors: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(parking_lot::Mutex::new(false)),
            self_applied: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get_windows(&self) -> Vec<WindowInfo> {
        self.windows.read().values().cloned().collect()
    }

    pub fn get_window(&self, hwnd: isize) -> Option<WindowInfo> {
        self.windows.read().get(&hwnd).cloned()
    }

    /// Refresh the backend cache from currently visible top-level windows and
    /// return the visible snapshot. Hidden valid HWNDs remain cached so wiri can
    /// keep tracking windows it hid on inactive workspaces.
    pub fn refresh_visible_windows(&self) -> Vec<WindowInfo> {
        let visible = Arc::new(RwLock::new(Vec::<WindowInfo>::new()));
        let result = unsafe {
            EnumWindows(
                Some(Self::visible_window_enum_callback),
                LPARAM(&*visible as *const _ as isize),
            )
        };
        if let Err(e) = result {
            warn!("EnumWindows refresh failed: {:?}", e);
        }

        let visible_windows = visible.read().clone();
        let mut cached = self.windows.write();
        cached.retain(|hwnd, _| {
            let hwnd_win = HWND(*hwnd as *mut std::ffi::c_void);
            unsafe { IsWindow(hwnd_win).as_bool() }
        });
        for info in &visible_windows {
            cached.insert(info.hwnd, info.clone());
        }

        visible_windows
    }

    unsafe extern "system" fn visible_window_enum_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let visible = &*(lparam.0 as *const RwLock<Vec<WindowInfo>>);

        if IsWindowVisible(hwnd).as_bool() {
            let info = WindowInfo::new(hwnd);
            if should_cache_window_info(&info) {
                visible.write().push(info);
            }
        }

        BOOL(1)
    }

    pub fn get_monitors(&self) -> Vec<MonitorInfo> {
        self.monitors.read().values().cloned().collect()
    }

    pub fn get_monitor(&self, id: OutputId) -> Option<MonitorInfo> {
        self.monitors.read().get(&id).cloned()
    }

    pub fn get_primary_monitor(&self) -> Option<MonitorInfo> {
        self.monitors
            .read()
            .values()
            .find(|m| m.is_primary)
            .cloned()
    }

    pub fn set_window_position(
        &self,
        hwnd: isize,
        rect: Rect,
        flags: SET_WINDOW_POS_FLAGS,
    ) -> Result<()> {
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        // Compensate for the invisible DWM shadow margin so the *visible*
        // window edges land at `rect`, not the frame edges (which include the
        // ~11 px shadow on Windows 10/11).  Without this, configured inner
        // gaps between tiles get eaten by overlapping shadows.
        let rect = expand_for_shadow_margin(hwnd_win, rect);
        self.place_window_frame(hwnd, rect, flags)
    }

    /// Put the window's frame exactly on `rect`: no shadow-margin compensation.  For a
    /// window that has no frame (fullscreen: caption and sizing border are stripped), where
    /// the frame *is* the visible window - compensating with the margins measured while it
    /// still had a frame made a fullscreen window 7 px larger than the monitor.
    pub fn set_window_position_exact(
        &self,
        hwnd: isize,
        rect: Rect,
        flags: SET_WINDOW_POS_FLAGS,
    ) -> Result<()> {
        self.place_window_frame(hwnd, rect, flags)
    }

    /// `SetWindowPos` for the frame rectangle `rect`, with the fallbacks for stubborn windows.
    fn place_window_frame(
        &self,
        hwnd: isize,
        rect: Rect,
        flags: SET_WINDOW_POS_FLAGS,
    ) -> Result<()> {
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);

        // SWP_NOCOPYBITS forces a full repaint of the client area instead of
        // bit-blitting old content — fixes black-content rendering on
        // Windows Terminal / Edge / Electron after a tile move.
        let base_flags = flags | SWP_NOCOPYBITS;
        // Try SetWindowPos with SWP_ASYNCWINDOWPOS
        let async_flags = base_flags | SWP_ASYNCWINDOWPOS;
        let result = unsafe {
            SetWindowPos(
                hwnd_win,
                HWND_TOP,
                rect.loc.x,
                rect.loc.y,
                rect.size.w as i32,
                rect.size.h as i32,
                async_flags,
            )
        };
        if result.is_ok() {
            debug!(
                "SetWindowPos SUCCESS: hwnd={} rect=({},{}) {}x{}",
                hwnd, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h
            );
            self.mark_self_applied(hwnd);
            request_redraw(hwnd_win);
            return Ok(());
        }
        // Fallback 1: MoveWindow (works on some stubborn windows)
        let move_result = unsafe {
            MoveWindow(
                hwnd_win,
                rect.loc.x,
                rect.loc.y,
                rect.size.w as i32,
                rect.size.h as i32,
                windows::Win32::Foundation::BOOL(1),
            )
        };
        if move_result.is_ok() {
            debug!(
                "MoveWindow SUCCESS: hwnd={} rect=({},{}) {}x{}",
                hwnd, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h
            );
            self.mark_self_applied(hwnd);
            request_redraw(hwnd_win);
            return Ok(());
        }
        // Fallback 2: Try without SWP_ASYNCWINDOWPOS
        let sync_result = unsafe {
            SetWindowPos(
                hwnd_win,
                HWND_TOP,
                rect.loc.x,
                rect.loc.y,
                rect.size.w as i32,
                rect.size.h as i32,
                base_flags,
            )
        };
        if sync_result.is_ok() {
            debug!("SetWindowPos sync SUCCESS: hwnd={}", hwnd);
            self.mark_self_applied(hwnd);
            request_redraw(hwnd_win);
            return Ok(());
        }
        debug!(
            "All position methods FAILED: hwnd={} rect=({},{}) {}x{}",
            hwnd, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h
        );
        Err(anyhow::anyhow!("Failed to position window {}", hwnd))
    }

    /// Position multiple windows atomically in a single render frame.
    ///
    /// Uses `BeginDeferWindowPos` / `DeferWindowPos` / `EndDeferWindowPos` under
    /// the hood. Failures for individual windows are logged but do not abort the
    /// whole batch. Returns the number of windows successfully deferred.
    pub fn set_window_positions_batched(
        &self,
        positions: &[(isize, Rect, SET_WINDOW_POS_FLAGS)],
    ) -> usize {
        self.set_window_positions_batched_with_results(positions)
            .len()
    }

    /// Batched positioning with per-HWND success reporting.
    ///
    /// `BeginDeferWindowPos` is still used for smooth multi-window updates, but
    /// callers that need circuit-breaker behavior can distinguish windows that
    /// actually made it into the committed batch from those that rejected the
    /// move/resize request.
    pub fn set_window_positions_batched_with_results(
        &self,
        positions: &[(isize, Rect, SET_WINDOW_POS_FLAGS)],
    ) -> HashSet<isize> {
        let n = positions.len();
        let mut applied = HashSet::new();
        if n == 0 {
            return applied;
        }

        #[cfg(test)]
        {
            // Unit tests use fabricated HWNDs with BackendHandle::default_for_test().
            // Treat batch attempts as successful there so engine state-machine tests
            // are not coupled to live Win32 windows.
            applied.extend(positions.iter().map(|(hwnd, _, _)| *hwnd));
        }

        #[cfg(not(test))]
        {
            // One asynchronous SetWindowPos per window (see `set_window_position`).
            // This used to be a BeginDeferWindowPos/EndDeferWindowPos batch, which
            // moves every window synchronously inside one call: a single slow or
            // hung application then stalled the whole layout pass while the engine
            // lock was held.  SWP_ASYNCWINDOWPOS (valid for SetWindowPos only -
            // DeferWindowPos rejects it) posts each move to the window's own thread.
            for (hwnd, rect, flags) in positions {
                match self.set_window_position(*hwnd, *rect, *flags) {
                    Ok(()) => {
                        applied.insert(*hwnd);
                    }
                    Err(e) => debug!("SetWindowPos failed for hwnd={}: {}", hwnd, e),
                }
            }
        }

        applied
    }

    /// Show or hide a window without ever blocking on it and without stealing
    /// the focus.
    ///
    /// Showing uses `SW_SHOWNA` ("show in its current state, do not activate")
    /// rather than `SW_SHOWNORMAL`, which activates the window and restores a
    /// maximised or minimised one - each of those made wiri grab the focus away
    /// from whatever the user was typing into.  The request goes through
    /// `ShowWindowAsync`, which posts to the window's own thread: a hung
    /// application can no longer freeze wiri (and, through the engine lock, its
    /// IPC server) inside a synchronous `ShowWindow`.
    ///
    /// Hiding is `SW_HIDE`, or - with `hide-method "cloak"` - shell cloaking, which keeps
    /// the window in the taskbar and in Alt+Tab (see `cloak`); a window that cannot be
    /// cloaked is hidden the plain way.  Showing undoes whichever was done.
    pub fn show_window(&self, hwnd: isize, show: bool) -> Result<()> {
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        if show {
            hidden::note_hidden(hwnd, false, false);
            // A no-op unless this run cloaked the window.
            cloak::uncloak(hwnd);
            unsafe {
                let _ = ShowWindowAsync(hwnd_win, SW_SHOWNA);
            }
            return Ok(());
        }
        // Record the hide first: if wiri dies right after the call, the next
        // start still knows this window must be shown again (and uncloaked).
        if cloak::wanted() {
            hidden::note_hidden(hwnd, true, true);
            if cloak::cloak(hwnd) {
                return Ok(());
            }
        }
        hidden::note_hidden(hwnd, true, false);
        // The method may have been changed while the window was cloaked.
        cloak::uncloak(hwnd);
        unsafe {
            let _ = ShowWindowAsync(hwnd_win, SW_HIDE);
        }
        Ok(())
    }

    pub fn send_event(&self, event: BackendEvent) {
        if let Err(_) = self.event_tx.send(event) {
            debug!("send_event: channel closed (receiver dropped)");
        }
    }

    pub fn add_window(&self, hwnd: isize, info: WindowInfo) {
        self.windows.write().insert(hwnd, info);
    }

    pub fn remove_window(&self, hwnd: isize) {
        self.windows.write().remove(&hwnd);
        SHADOW_MARGINS.lock().remove(&hwnd);
    }

    pub fn update_window(&self, hwnd: isize, info: WindowInfo) {
        self.windows.write().insert(hwnd, info);
    }

    /// Bring `hwnd` to the foreground, defeating the foreground lock.
    ///
    /// Windows only lets a process set the foreground window when it owns the
    /// "foreground lock" (it received the last input event, for example).  wiri
    /// usually does not - it acts on IPC requests, timers and WinEvents - so a
    /// plain `SetForegroundWindow` is routinely refused.  The escalation, from
    /// cheapest to most invasive:
    ///   1. nothing to do when the window already is the foreground window;
    ///   2. plain `SetForegroundWindow`;
    ///   3. inject a harmless key event (F24): the process that injected the
    ///      last input is the "input provider" and is allowed to switch;
    ///   4. `AttachThreadInput` to the thread that owns the foreground;
    ///   5. `SwitchToThisWindow`, the call Alt+Tab itself uses.
    /// Returns an error only when every step failed; callers log that once and
    /// do not retry (see `TilingEngine::activate_now`).
    pub fn activate_window(&self, hwnd: isize) -> Result<()> {
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            if !IsWindow(hwnd_win).as_bool() {
                return Err(anyhow::anyhow!("activate_window: invalid HWND {}", hwnd));
            }
            if GetForegroundWindow() == hwnd_win {
                return Ok(());
            }
            if IsIconic(hwnd_win).as_bool() {
                let _ = ShowWindowAsync(hwnd_win, SW_RESTORE);
            }

            // 2. Plain request.
            if SetForegroundWindow(hwnd_win).as_bool() {
                debug!("activate_window: SetForegroundWindow OK hwnd={}", hwnd);
                return Ok(());
            }

            // 3. Injected input lifts the foreground lock for this process.
            nudge_foreground_lock();
            if SetForegroundWindow(hwnd_win).as_bool() {
                debug!(
                    "activate_window: input nudge + SetForegroundWindow OK hwnd={}",
                    hwnd
                );
                return Ok(());
            }

            // 4. Share the input queue of the thread that owns the foreground.
            let current_thread = windows::Win32::System::Threading::GetCurrentThreadId();
            let fg_hwnd = GetForegroundWindow();
            if !fg_hwnd.0.is_null() {
                let fg_thread = GetWindowThreadProcessId(fg_hwnd, None);
                if fg_thread != 0 && fg_thread != current_thread {
                    let _ = windows::Win32::System::Threading::AttachThreadInput(
                        current_thread,
                        fg_thread,
                        true,
                    );
                    let ok = SetForegroundWindow(hwnd_win).as_bool();
                    let _ = windows::Win32::System::Threading::AttachThreadInput(
                        current_thread,
                        fg_thread,
                        false,
                    );
                    if ok {
                        debug!(
                            "activate_window: AttachThreadInput+SetForegroundWindow OK hwnd={}",
                            hwnd
                        );
                        return Ok(());
                    }
                }
            }

            // 5. Alt+Tab's own entry point.
            SwitchToThisWindow(hwnd_win, windows::Win32::Foundation::BOOL(1));
            // It takes effect asynchronously; give it a moment before judging.
            std::thread::sleep(std::time::Duration::from_millis(40));
            if GetForegroundWindow() == hwnd_win {
                debug!("activate_window: SwitchToThisWindow OK hwnd={}", hwnd);
                return Ok(());
            }
        }
        Err(anyhow::anyhow!(
            "Windows refused to bring hwnd={} to the foreground (foreground lock)",
            hwnd
        ))
    }

    /// Look up the process executable name for a PID via QueryFullProcessImageNameW.
    /// Returns the file name (e.g. "firefox.exe" for "C:\Program Files\Mozilla Firefox\firefox.exe");
    /// window rules match it with and without the extension.
    /// Returns None on failure or for PID 0.
    pub fn process_name_for_pid(&self, pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        unsafe {
            use windows::Win32::System::Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
                PROCESS_QUERY_LIMITED_INFORMATION,
            };
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 260];
            let mut size = buf.len() as u32;
            let result = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut size,
            );
            let _ = windows::Win32::Foundation::CloseHandle(handle);
            if result.is_err() {
                return None;
            }
            let path = String::from_utf16_lossy(&buf[..size as usize]);
            std::path::Path::new(&path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
        }
    }

    /// The AppUserModelID of the window (`Microsoft.WindowsTerminal_8wekyb3d8bbwe!App`),
    /// when it has one: what `app-id` in a window rule can match besides the process's
    /// name.  One call to the shell per window; ask once, when the window opens.
    pub fn aumid_for_window(&self, hwnd: isize) -> Option<String> {
        aumid::of_window(hwnd)
    }

    /// The narrowest `hwnd` can be made, as a *visible* width in pixels (what the layout
    /// works in), or `None` when it does not say.
    ///
    /// The window is asked with `WM_GETMINMAXINFO`, the question the system itself puts to
    /// it before every resize (the system marshals the answer across processes; the call is
    /// bounded by a timeout and gives up at once on a hung or higher-integrity window).  The
    /// answer is a *window* width, so the invisible border is taken off.  A Windows Terminal
    /// window answers about 476 px (462 visible); without knowing that, three terminals on a
    /// 1366 px screen are asked for 440 px each, refuse, and lie on top of one another.
    ///
    /// Call it once per window (and again, at most twice, if it did not answer), outside the
    /// engine lock: the window answers on its own thread.
    pub fn min_track_width(&self, hwnd: isize) -> Option<u32> {
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        let outer = query_min_track_width(hwnd_win)?;
        let border = measured_shadow_margins(hwnd_win).map_or(0, |m| m[0] + m[2]);
        visible_min_width(outer, border)
    }

    /// Put a window back exactly where it was before wiri tiled it.  `rect` is
    /// the plain `GetWindowRect` rectangle recorded at that time, so unlike
    /// [`Self::set_window_position`] no shadow-margin compensation is applied.
    pub fn restore_window_rect(&self, hwnd: isize, rect: Rect) -> Result<()> {
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            SetWindowPos(
                hwnd_win,
                HWND::default(), // ignored: SWP_NOZORDER
                rect.loc.x,
                rect.loc.y,
                rect.size.w as i32,
                rect.size.h as i32,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
            .map_err(|e| anyhow::anyhow!("restore_window_rect: hwnd={}: {}", hwnd, e))
        }
    }
}

// ---------------------------------------------------------------------------
// Self-applied tracker — concrete inherent methods, NOT part of BackendApi.
// ---------------------------------------------------------------------------
impl BackendHandle {
    /// Record that wiri just called SetWindowPos for `hwnd`.
    /// The WinEvent hook will suppress the resulting EVENT_OBJECT_LOCATIONCHANGE
    /// for up to SELF_APPLIED_RECENT_MS milliseconds.
    pub fn mark_self_applied(&self, hwnd: isize) {
        let now = std::time::Instant::now();
        let mut map = self.self_applied.write();
        map.insert(hwnd, now);
        // Opportunistically prune stale entries to prevent unbounded growth.
        if map.len() > 256 {
            let cutoff = now - std::time::Duration::from_millis(SELF_APPLIED_PRUNE_MS as u64);
            map.retain(|_, t| *t > cutoff);
        }
    }

    /// Returns true if wiri applied a position to `hwnd` within the last
    /// SELF_APPLIED_RECENT_MS milliseconds. Also prunes entries older than
    /// SELF_APPLIED_PRUNE_MS.
    pub fn is_self_applied_recent(&self, hwnd: isize) -> bool {
        let now = std::time::Instant::now();
        let mut map = self.self_applied.write();
        // Prune stale entry for this hwnd (frees memory, keeps semantics clean).
        if let Some(&t) = map.get(&hwnd) {
            if now.duration_since(t).as_millis() > SELF_APPLIED_PRUNE_MS {
                map.remove(&hwnd);
                return false;
            }
            now.duration_since(t).as_millis() < SELF_APPLIED_RECENT_MS
        } else {
            false
        }
    }

    /// Expose the `self_applied` Arc so hooks.rs can clone it without coupling
    /// to BackendHandle's internal structure.
    pub fn self_applied_map(&self) -> Arc<RwLock<HashMap<isize, std::time::Instant>>> {
        self.self_applied.clone()
    }
}

// ---------------------------------------------------------------------------
// Additional concrete helpers on BackendHandle (used by BackendApi impl below)
// ---------------------------------------------------------------------------
impl BackendHandle {
    /// Send WM_CLOSE to the window identified by `hwnd`.
    pub fn close_window(&self, hwnd: isize) -> Result<()> {
        unsafe {
            PostMessageW(
                HWND(hwnd as *mut std::ffi::c_void),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            )
            .map_err(|e| {
                anyhow::anyhow!("close_window: PostMessageW failed for hwnd={}: {}", hwnd, e)
            })
        }
    }

    /// Find which monitor covers the given screen point. Returns the `OutputId` of
    /// the monitor whose bounds contain `(x, y)`, or `None` if no match.
    pub fn monitor_at_point(&self, x: i32, y: i32) -> Option<crate::utils::OutputId> {
        use crate::utils::Point;
        let pt = Point::new(x, y);
        self.monitors
            .read()
            .values()
            .find(|m| m.bounds.contains_point(pt))
            .map(|m| m.id)
    }

    /// Find the monitor a window is currently on by calling `MonitorFromWindow` and
    /// matching the result against the cached monitor list by device name.
    pub fn monitor_for_window(&self, hwnd: isize) -> Option<crate::utils::OutputId> {
        let hmon = unsafe {
            MonitorFromWindow(
                HWND(hwnd as *mut std::ffi::c_void),
                MONITOR_DEFAULTTONEAREST,
            )
        };
        // Retrieve the device name from the HMONITOR and look it up in the cache.
        let name = unsafe {
            let mut info: MONITORINFOEXW = std::mem::zeroed();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            let info_ptr = &mut info as *mut MONITORINFOEXW as *mut MONITORINFO;
            if GetMonitorInfoW(hmon, info_ptr).as_bool() {
                Some(String::from_utf16_lossy(
                    &info.szDevice[..info
                        .szDevice
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(info.szDevice.len())],
                ))
            } else {
                None
            }
        }?;
        self.monitors
            .read()
            .values()
            .find(|m| m.name == name)
            .map(|m| m.id)
    }

    /// Minimize the window identified by `hwnd`.
    pub fn minimize_window(&self, hwnd: isize) -> Result<()> {
        unsafe {
            let _ = ShowWindow(HWND(hwnd as *mut std::ffi::c_void), SW_MINIMIZE);
        }
        Ok(())
    }

    /// Maximize the window identified by `hwnd`.
    pub fn maximize_window(&self, hwnd: isize) -> Result<()> {
        unsafe {
            let _ = ShowWindow(HWND(hwnd as *mut std::ffi::c_void), SW_MAXIMIZE);
        }
        Ok(())
    }

    /// Restore a minimized or maximized window identified by `hwnd`.
    pub fn restore_window(&self, hwnd: isize) -> Result<()> {
        unsafe {
            let _ = ShowWindow(HWND(hwnd as *mut std::ffi::c_void), SW_RESTORE);
        }
        Ok(())
    }

    /// Take the native maximized state away from a window without waiting for it
    /// (`ShowWindowAsync`): an application that does not answer must not hold wiri up.
    pub fn restore_window_async(&self, hwnd: isize) {
        unsafe {
            let _ = ShowWindowAsync(HWND(hwnd as *mut std::ffi::c_void), SW_RESTORE);
        }
    }
}

// ---------------------------------------------------------------------------
// BackendApi trait
// ---------------------------------------------------------------------------

/// Abstract interface for the platform window-manager backend.
///
/// The current concrete impl is `crate::backend::Backend` (Windows / Win32).
/// This trait was added per the spec for testability and future portability;
/// callers may opt into `&dyn BackendApi` for indirection but most code uses
/// the concrete type directly for ergonomics.
///
/// The `WindowsBackend` type alias below documents the intended name from the
/// spec; the original `Backend` identifier is preserved for API stability.
/// If a second backend implementation is ever added, the public symbol can be
/// renamed in one step (alias → struct, struct → `WindowsBackend`).
pub trait BackendApi: Send + Sync {
    /// Bring a window to the foreground. Implementations may use SetForegroundWindow + AttachThreadInput.
    fn activate_window(&self, hwnd: isize) -> Result<()>;

    /// Position a window. Flags are platform-specific (Win32 SET_WINDOW_POS_FLAGS).
    fn set_window_position(&self, hwnd: isize, rect: Rect, flags_raw: u32) -> Result<()>;

    /// Show or hide a window.
    fn show_window(&self, hwnd: isize, show: bool) -> Result<()>;

    /// Send WM_CLOSE to a window.
    fn close_window(&self, hwnd: isize) -> Result<()>;

    /// Cached snapshot of all currently-tracked windows.
    fn get_windows(&self) -> Vec<WindowInfo>;

    /// Look up a window by HWND.
    fn get_window(&self, hwnd: isize) -> Option<WindowInfo>;

    /// Cached monitor list.
    fn get_monitors(&self) -> Vec<MonitorInfo>;

    /// Find which monitor (by OutputId) covers the given screen point.
    fn monitor_at_point(&self, x: i32, y: i32) -> Option<crate::utils::OutputId>;

    /// Find the monitor a window is currently on.
    fn monitor_for_window(&self, hwnd: isize) -> Option<crate::utils::OutputId>;

    /// Look up the process executable name for a PID.
    fn process_name_for_pid(&self, pid: u32) -> Option<String>;

    /// Mark a window as minimized.
    fn minimize_window(&self, hwnd: isize) -> Result<()>;

    /// Mark a window as maximized.
    fn maximize_window(&self, hwnd: isize) -> Result<()>;

    /// Restore a minimized/maximized window.
    fn restore_window(&self, hwnd: isize) -> Result<()>;
}

impl BackendApi for BackendHandle {
    fn activate_window(&self, hwnd: isize) -> Result<()> {
        self.activate_window(hwnd)
    }

    fn set_window_position(&self, hwnd: isize, rect: Rect, flags_raw: u32) -> Result<()> {
        self.set_window_position(hwnd, rect, SET_WINDOW_POS_FLAGS(flags_raw))
    }

    fn show_window(&self, hwnd: isize, show: bool) -> Result<()> {
        self.show_window(hwnd, show)
    }

    fn close_window(&self, hwnd: isize) -> Result<()> {
        self.close_window(hwnd)
    }

    fn get_windows(&self) -> Vec<WindowInfo> {
        self.get_windows()
    }

    fn get_window(&self, hwnd: isize) -> Option<WindowInfo> {
        self.get_window(hwnd)
    }

    fn get_monitors(&self) -> Vec<MonitorInfo> {
        self.get_monitors()
    }

    fn monitor_at_point(&self, x: i32, y: i32) -> Option<crate::utils::OutputId> {
        self.monitor_at_point(x, y)
    }

    fn monitor_for_window(&self, hwnd: isize) -> Option<crate::utils::OutputId> {
        self.monitor_for_window(hwnd)
    }

    fn process_name_for_pid(&self, pid: u32) -> Option<String> {
        self.process_name_for_pid(pid)
    }

    fn minimize_window(&self, hwnd: isize) -> Result<()> {
        self.minimize_window(hwnd)
    }

    fn maximize_window(&self, hwnd: isize) -> Result<()> {
        self.maximize_window(hwnd)
    }

    fn restore_window(&self, hwnd: isize) -> Result<()> {
        self.restore_window(hwnd)
    }
}

// ---------------------------------------------------------------------------
// LayoutRequest enum + channel helper
// ---------------------------------------------------------------------------

/// Requests sent from the layout engine to the backend.
///
/// The engine continues to call `BackendApi` methods directly for ops that
/// need immediate feedback (set_window_position → applied_state cache update),
/// and opts into this channel via `TilingEngine::dispatch_request` for
/// fire-and-forget ops. `main.rs` wires the drainer that calls `apply()` on
/// each incoming request.
#[derive(Debug, Clone)]
pub enum LayoutRequest {
    PositionWindow { hwnd: isize, rect: Rect, flags: u32 },
    ActivateWindow { hwnd: isize },
    ShowWindow { hwnd: isize, show: bool },
    CloseWindow { hwnd: isize },
    MinimizeWindow { hwnd: isize },
    MaximizeWindow { hwnd: isize },
    RestoreWindow { hwnd: isize },
}

impl LayoutRequest {
    /// Apply this request to a backend implementing `BackendApi`.
    pub fn apply(&self, backend: &dyn BackendApi) -> Result<()> {
        match self {
            LayoutRequest::PositionWindow { hwnd, rect, flags } => {
                backend.set_window_position(*hwnd, *rect, *flags)
            }
            LayoutRequest::ActivateWindow { hwnd } => backend.activate_window(*hwnd),
            LayoutRequest::ShowWindow { hwnd, show } => backend.show_window(*hwnd, *show),
            LayoutRequest::CloseWindow { hwnd } => backend.close_window(*hwnd),
            LayoutRequest::MinimizeWindow { hwnd } => backend.minimize_window(*hwnd),
            LayoutRequest::MaximizeWindow { hwnd } => backend.maximize_window(*hwnd),
            LayoutRequest::RestoreWindow { hwnd } => backend.restore_window(*hwnd),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `BackendHandle` can be used as `&dyn BackendApi` and that
    /// the delegating methods don't panic on an empty (test) handle.
    #[test]
    fn test_backend_api_trait_object_dispatch() {
        let handle = BackendHandle::default_for_test();
        let api: &dyn BackendApi = &handle;
        // Both return empty vecs in test mode — no panics expected.
        let monitors = api.get_monitors();
        let windows = api.get_windows();
        assert!(monitors.is_empty());
        assert!(windows.is_empty());
    }

    /// Verify that `LayoutRequest::apply` dispatches correctly to the backend
    /// without panicking (the Win32 call may fail in test; that's fine).
    #[test]
    fn test_layout_request_apply_show_window() {
        let handle = BackendHandle::default_for_test();
        let request = LayoutRequest::ShowWindow {
            hwnd: 0,
            show: true,
        };
        // Result may be Ok or Err (hwnd=0 is invalid); must not panic.
        let _ = request.apply(&handle as &dyn BackendApi);
    }

    /// After mark_self_applied, is_self_applied_recent must return true.
    /// After the PRUNE threshold is passed (simulated by direct map manipulation),
    /// is_self_applied_recent must return false and prune the entry.
    #[test]
    fn test_self_applied_tracker_prunes_old_entries() {
        let handle = BackendHandle::default_for_test();
        let hwnd: isize = 12345;

        // Not marked yet — must be false.
        assert!(!handle.is_self_applied_recent(hwnd));

        // Mark as applied — must be immediately recent.
        handle.mark_self_applied(hwnd);
        assert!(handle.is_self_applied_recent(hwnd));

        // Manually overwrite the timestamp with one that is older than PRUNE_MS.
        {
            let stale = std::time::Instant::now()
                - std::time::Duration::from_millis(SELF_APPLIED_PRUNE_MS as u64 + 1);
            handle.self_applied.write().insert(hwnd, stale);
        }

        // Now the entry is stale — must return false AND be pruned.
        assert!(!handle.is_self_applied_recent(hwnd));
        assert!(
            !handle.self_applied.read().contains_key(&hwnd),
            "stale entry must be removed from the map"
        );
    }

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    #[test]
    fn test_shadow_margins_from_a_normal_window() {
        // A typical Windows 11 window: 7 px of invisible border left, right
        // and bottom, none on top.
        let frame = rect(93, 100, 807, 507);
        let visible = rect(100, 100, 800, 500);
        assert_eq!(shadow_margins_from(&frame, &visible), Some([7, 0, 7, 7]));
    }

    #[test]
    fn test_shadow_margins_reject_implausible_measurements() {
        // Hidden / minimised window: DWM reports an empty rectangle.
        assert_eq!(
            shadow_margins_from(&rect(19, 26, 553, 413), &rect(0, 0, 0, 0)),
            None
        );
        // Visible rect far outside the frame (mid-move disagreement).
        assert_eq!(
            shadow_margins_from(&rect(100, 100, 700, 500), &rect(10, 10, 600, 400)),
            None
        );
        // Custom non-client area: more than 32 px of "margin".
        assert_eq!(
            shadow_margins_from(&rect(0, 0, 800, 600), &rect(40, 0, 760, 560)),
            None
        );
    }

    #[test]
    fn test_shadow_margins_tolerate_a_pixel_of_rounding() {
        let frame = rect(100, 100, 700, 500);
        let visible = rect(99, 100, 700, 500);
        assert_eq!(shadow_margins_from(&frame, &visible), Some([0, 0, 0, 0]));
    }

    #[test]
    fn test_forget_shadow_margins_drops_the_measurement() {
        // A window that lost its frame (fullscreen) or got it back must be measured again.
        let hwnd: isize = 0x5EED_0001;
        SHADOW_MARGINS.lock().insert(hwnd, [7, 0, 7, 7]);
        assert!(SHADOW_MARGINS.lock().contains_key(&hwnd));
        forget_shadow_margins(hwnd);
        assert!(!SHADOW_MARGINS.lock().contains_key(&hwnd));
        // Forgetting a window that was never measured is harmless.
        forget_shadow_margins(0x5EED_0002);
    }

    #[test]
    fn test_visible_min_width_takes_the_invisible_border_off() {
        // A Windows Terminal window answers 476 px, 14 of which are invisible border:
        // the visible window is at least 462 px wide.
        assert_eq!(visible_min_width(476, 14), Some(462));
        // No border known: the answer is used as it is (a column slightly too wide is
        // better than one that overlaps).
        assert_eq!(visible_min_width(476, 0), Some(476));
        // Nothing, or garbage, is not a minimum.
        assert_eq!(visible_min_width(0, 14), None);
        assert_eq!(visible_min_width(-5, 0), None);
        assert_eq!(visible_min_width(20_000, 14), None);
        // A border as wide as the window leaves nothing to say.
        assert_eq!(visible_min_width(14, 14), None);
    }

    #[test]
    fn test_min_track_width_of_a_window_that_is_not_there_is_none() {
        let handle = BackendHandle::default_for_test();
        assert_eq!(handle.min_track_width(0), None);
        assert_eq!(handle.min_track_width(0x0DEA_D001), None);
    }

    #[test]
    fn test_apply_shadow_margins_puts_the_visible_edges_on_the_target() {
        // The visible rect of the window below must land on (100, 50) 600x400:
        // the frame is 7 px wider on both sides and 7 px taller at the bottom.
        let wanted = Rect::new(100, 50, 600, 400);
        let frame = apply_shadow_margins(wanted, [7, 0, 7, 7]);
        assert_eq!(frame, Rect::new(93, 50, 614, 407));
        // And it is a no-op without margins (WinUI windows have none).
        assert_eq!(apply_shadow_margins(wanted, [0, 0, 0, 0]), wanted);
    }

    /// mark_self_applied for many HWNDs triggers the internal prune when len > 256.
    #[test]
    fn test_self_applied_tracker_bulk_prune() {
        let handle = BackendHandle::default_for_test();

        // Insert 257 stale entries manually.
        {
            let stale = std::time::Instant::now()
                - std::time::Duration::from_millis(SELF_APPLIED_PRUNE_MS as u64 + 1);
            let mut map = handle.self_applied.write();
            for i in 0..257isize {
                map.insert(i, stale);
            }
        }
        assert_eq!(handle.self_applied.read().len(), 257);

        // mark_self_applied triggers the len > 256 prune path, removing stale entries.
        handle.mark_self_applied(9999);
        // After prune all stale entries should be gone; only the fresh one remains.
        let len = handle.self_applied.read().len();
        assert_eq!(
            len, 1,
            "only the freshly-marked hwnd should remain after prune"
        );
    }
}

/// Adjust a target visible-bounds rect to compensate for the invisible DWM
/// drop-shadow margin on Windows 10/11.
///
/// `GetWindowRect` returns the frame including the ~11 px transparent shadow
/// on left/right/bottom (top is usually 0).  `DwmGetWindowAttribute(
/// DWMWA_EXTENDED_FRAME_BOUNDS)` returns the *visible* edges.  We compute the
/// per-side delta from the current window state and apply it as a margin so
/// that after `SetWindowPos(rect.expanded)` the visible edges land at `rect`.
///
/// Without this, configured `inner_gaps` between tiles are eaten by the two
/// adjacent windows' overlapping shadows, making the tiles appear flush.
fn expand_for_shadow_margin(hwnd: HWND, rect: Rect) -> Rect {
    match measured_shadow_margins(hwnd) {
        Some(margins) => apply_shadow_margins(rect, margins),
        // Hidden, minimised or mid-move window, or a custom non-client area:
        // placing it uncompensated is better than guessing.
        None => rect,
    }
}

/// The invisible border of `hwnd` as `[left, top, right, bottom]`: cached, or measured now.
///
/// The invisible border of a window does not change while it is managed, but
/// measuring it needs `GetWindowRect` and DWM's frame bounds to agree - and
/// right after a (now asynchronous) move they briefly do not.  Measure once,
/// while the window is at rest, and reuse the answer.
fn measured_shadow_margins(hwnd: HWND) -> Option<[i32; 4]> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let key = hwnd.0 as isize;
    if let Some(margins) = SHADOW_MARGINS.lock().get(&key).copied() {
        return Some(margins);
    }

    let mut frame = RECT::default();
    let mut visible = RECT::default();
    unsafe {
        if GetWindowRect(hwnd, &mut frame).is_err() {
            return None;
        }
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut visible as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if hr.is_err() {
            return None;
        }
    }

    let margins = shadow_margins_from(&frame, &visible)?;
    SHADOW_MARGINS.lock().insert(key, margins);
    Some(margins)
}

/// How long to wait for a window to answer `WM_GETMINMAXINFO` (a window that has just been
/// created may be busy for a moment).
const MIN_TRACK_QUERY_TIMEOUT_MS: u32 = 120;

/// The minimum *window* width (`ptMinTrackSize.x`) `hwnd` answers to `WM_GETMINMAXINFO`.
/// `None` when it does not answer: it is gone, it is a window of wiri itself, it is hung
/// (`SMTO_ABORTIFHUNG` returns at once), it belongs to a process wiri may not send messages
/// to, or it is too busy to answer within the timeout.
fn query_min_track_width(hwnd: HWND) -> Option<i32> {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, MINMAXINFO, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_GETMINMAXINFO,
    };

    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return None;
        }
        // The structure is zeroed: a window that answers sets its own minimum, one that does
        // not leaves 0 (or the system default, which no layout is ever narrower than).
        let mut info = MINMAXINFO::default();
        let mut result = 0usize;
        let answered = SendMessageTimeoutW(
            hwnd,
            WM_GETMINMAXINFO,
            WPARAM(0),
            LPARAM(&mut info as *mut MINMAXINFO as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            MIN_TRACK_QUERY_TIMEOUT_MS,
            Some(&mut result),
        );
        if answered.0 == 0 {
            return None;
        }
        Some(info.ptMinTrackSize.x)
    }
}

/// The visible minimum width of a window that answered `outer` (its minimum window width)
/// and has `border` px of invisible border on its two sides together.  `None` when the
/// answer is nothing, or not plausible (no screen is 16384 px wide).
fn visible_min_width(outer: i32, border: i32) -> Option<u32> {
    const MAX_PLAUSIBLE_PX: i32 = 16_384;
    if outer <= 0 || outer > MAX_PLAUSIBLE_PX {
        return None;
    }
    let visible = outer - border.max(0);
    (visible > 0).then_some(visible as u32)
}

/// Invisible-border margins `[left, top, right, bottom]` per window.
static SHADOW_MARGINS: std::sync::LazyLock<parking_lot::Mutex<HashMap<isize, [i32; 4]>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

/// Forget the invisible-border margins measured for `hwnd`.  Call it when the window's
/// frame changes (fullscreen strips the caption and sizing border, leaving it restores
/// them): the next placement measures the window again instead of compensating for a
/// border it does not have (or has again).
pub fn forget_shadow_margins(hwnd: isize) {
    SHADOW_MARGINS.lock().remove(&hwnd);
}

/// Margins between the full window rect (`frame`, what `GetWindowRect` reports)
/// and the visible one (`visible`, DWM's extended frame bounds), ordered
/// `[left, top, right, bottom]`.  `None` when the pair is not a plausible
/// measurement: the visible rect must lie inside the frame (give or take a
/// pixel of DPI rounding) and no margin may exceed 32 px - anything else is a
/// hidden or minimised window (DWM reports an empty rect) or a window with a
/// custom non-client area where compensating would hurt more than help.
fn shadow_margins_from(
    frame: &windows::Win32::Foundation::RECT,
    visible: &windows::Win32::Foundation::RECT,
) -> Option<[i32; 4]> {
    let raw = [
        visible.left - frame.left,
        visible.top - frame.top,
        frame.right - visible.right,
        frame.bottom - visible.bottom,
    ];
    if raw.iter().all(|m| (-2..=32).contains(m)) {
        Some(raw.map(|m| m.max(0)))
    } else {
        None
    }
}

/// Grow `rect` (the wanted *visible* rectangle) by the invisible margins so
/// that, once the window frame occupies the result, the visible edges land on
/// `rect`.
fn apply_shadow_margins(rect: Rect, margins: [i32; 4]) -> Rect {
    let [left, top, right, bottom] = margins;
    Rect::new(
        rect.loc.x - left,
        rect.loc.y - top,
        rect.size.w.saturating_add((left + right) as u32),
        rect.size.h.saturating_add((top + bottom) as u32),
    )
}

/// Returns the default flags for batched SetWindowPos calls.
/// The `mirror` parameter was removed — it was dead code that produced identical
/// output in both branches.
pub fn swp_flags() -> SET_WINDOW_POS_FLAGS {
    SWP_NOZORDER | SWP_NOACTIVATE
}

pub fn default_flags() -> SET_WINDOW_POS_FLAGS {
    SWP_NOZORDER | SWP_NOACTIVATE
}

/// Return `flags` with SWP_NOSIZE cleared so that SetWindowPos WILL resize the window.
/// Previously this accidentally OR-ed in SWP_NOSIZE (0x0001), inverting the intended meaning.
pub fn with_size(flags: SET_WINDOW_POS_FLAGS) -> SET_WINDOW_POS_FLAGS {
    SET_WINDOW_POS_FLAGS(flags.0 & !SWP_NOSIZE.0)
}

/// Return `flags` with SWP_NOMOVE cleared so that SetWindowPos WILL move the window.
/// Previously this accidentally OR-ed in SWP_NOMOVE (0x0002), inverting the intended meaning.
pub fn with_move(flags: SET_WINDOW_POS_FLAGS) -> SET_WINDOW_POS_FLAGS {
    SET_WINDOW_POS_FLAGS(flags.0 & !SWP_NOMOVE.0)
}
