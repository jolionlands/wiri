pub mod hooks;
pub mod message_loop;
pub mod cbt_hook;
pub mod hotkey_conflicts;
pub mod accent;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
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
    EnumDisplayMonitors, GetMonitorInfoW, HDC, MONITORINFOEXW, MONITORINFO,
    MonitorFromWindow, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, IsWindow, MoveWindow, PostMessageW, SetForegroundWindow, SetWindowPos, ShowWindow,
    HWND_TOP, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE, SWP_NOZORDER, SWP_NOSIZE, SWP_NOMOVE,
    SWP_ASYNCWINDOWPOS, SWP_NOCOPYBITS, SHOW_WINDOW_CMD, SW_MINIMIZE, SW_MAXIMIZE, SW_RESTORE, WM_CLOSE,
};
use windows::Win32::Graphics::Gdi::{RedrawWindow, RDW_INVALIDATE, RDW_ALLCHILDREN, RDW_UPDATENOW};

/// Force a window to repaint after a tile move. Fixes apps (Cascadia/Windows
/// Terminal, Edge, some Electron) that show a black client area after
/// SetWindowPos because they don't process WM_SIZE eagerly.
fn request_redraw(hwnd: HWND) {
    unsafe {
        let _ = RedrawWindow(
            hwnd,
            None,
            None,
            RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
    }
}

pub use hooks::WinEventHook;
pub use message_loop::MessageLoop;
use crate::utils::{OutputId, Rect};

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
    WindowCreated { hwnd: isize },
    WindowDestroyed { hwnd: isize },
    WindowMoved { hwnd: isize, rect: Rect },
    WindowResized { hwnd: isize, rect: Rect },
    WindowShown { hwnd: isize },
    WindowHidden { hwnd: isize },
    MonitorConnected { id: u64 },
    MonitorDisconnected { id: u64 },
    ForegroundChanged { hwnd: isize },
    WindowTitleChanged { hwnd: isize },
    // CBT Hook events (fire BEFORE window operations)
    CbtCreateWindow { hwnd: isize },
    CbtDestroyWindow { hwnd: isize },
    CbtActivate { hwnd: isize },
    CbtMoveSize { hwnd: isize },
    CbtSetFocus { hwnd: isize },
    /// Fires when the user starts a manual move/resize via window chrome.
    WindowMoveResizeStart { hwnd: isize },
    /// Fires when the user finishes a manual move/resize via window chrome.
    WindowMoveResizeEnd { hwnd: isize },
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
                    &info.szDevice[..info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len())]
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
                if hr.is_ok() { dpi_x as f64 / 96.0 } else { 1.0 }
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

    fn enumerate_monitors(&self, monitors: Arc<RwLock<HashMap<OutputId, MonitorInfo>>>) -> Result<()> {
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

    unsafe extern "system" fn window_enum_callback(
        hwnd: HWND,
        lparam: LPARAM,
    ) -> BOOL {
        let windows = &*(lparam.0 as *const RwLock<HashMap<isize, WindowInfo>>);

        // Only include visible windows
        if IsWindowVisible(hwnd).as_bool() {
            let info = WindowInfo::new(hwnd);
            // Skip tool windows and other special windows
            if !info.class_name.is_empty()
                && !info.class_name.starts_with("Shell_")
                && info.class_name != "Progman"
                && info.class_name != "WorkerW"
            {
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
        self.event_rx.write().take().expect("event_rx already taken")
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

    pub fn get_monitors(&self) -> Vec<MonitorInfo> {
        self.monitors.read().values().cloned().collect()
    }

    pub fn get_monitor(&self, id: OutputId) -> Option<MonitorInfo> {
        self.monitors.read().get(&id).cloned()
    }

    pub fn get_primary_monitor(&self) -> Option<MonitorInfo> {
        self.monitors.read().values().find(|m| m.is_primary).cloned()
    }

    pub fn set_window_position(&self, hwnd: isize, rect: Rect, flags: SET_WINDOW_POS_FLAGS) -> Result<()> {
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
            debug!("SetWindowPos SUCCESS: hwnd={} rect=({},{}) {}x{}", hwnd, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
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
            debug!("MoveWindow SUCCESS: hwnd={} rect=({},{}) {}x{}", hwnd, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
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
        debug!("All position methods FAILED: hwnd={} rect=({},{}) {}x{}", hwnd, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
        Err(anyhow::anyhow!("Failed to position window {}", hwnd))
    }


    pub fn show_window(&self, hwnd: isize, show: bool) -> Result<()> {
        unsafe {
            let _ = ShowWindow(
                HWND(hwnd as *mut std::ffi::c_void),
                if show { SHOW_WINDOW_CMD(1) } else { SHOW_WINDOW_CMD(0) },
            );
        }
        Ok(())
    }

    /// Bring `hwnd` to the foreground. Uses the AttachThreadInput trick as a fallback
    /// when another process holds the foreground lock.
    pub fn activate_window(&self, hwnd: isize) -> Result<()> {
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            if !IsWindow(hwnd_win).as_bool() {
                return Err(anyhow::anyhow!("activate_window: invalid HWND {}", hwnd));
            }
            if SetForegroundWindow(hwnd_win).as_bool() {
                debug!("activate_window: SetForegroundWindow OK hwnd={}", hwnd);
                return Ok(());
            }
            // Fallback: AttachThreadInput trick to overcome the foreground lock.
            let current_thread = windows::Win32::System::Threading::GetCurrentThreadId();
            let fg_hwnd = GetForegroundWindow();
            if fg_hwnd.0.is_null() {
                return Err(anyhow::anyhow!("activate_window: SetForegroundWindow failed and no current foreground window"));
            }
            let fg_thread = GetWindowThreadProcessId(fg_hwnd, None);
            if fg_thread != 0 && fg_thread != current_thread {
                let _ = windows::Win32::System::Threading::AttachThreadInput(
                    current_thread, fg_thread, true,
                );
                let ok = SetForegroundWindow(hwnd_win).as_bool();
                let _ = windows::Win32::System::Threading::AttachThreadInput(
                    current_thread, fg_thread, false,
                );
                if ok {
                    debug!("activate_window: AttachThreadInput+SetForegroundWindow OK hwnd={}", hwnd);
                    return Ok(());
                }
            }
        }
        Err(anyhow::anyhow!("activate_window: failed to bring hwnd={} to foreground", hwnd))
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
    }

    pub fn update_window(&self, hwnd: isize, info: WindowInfo) {
        self.windows.write().insert(hwnd, info);
    }

    /// Look up the process executable name for a PID via QueryFullProcessImageNameW.
    /// Returns the file stem (e.g. "firefox" for "C:\Program Files\Mozilla Firefox\firefox.exe").
    /// Returns None on failure or for PID 0.
    pub fn process_name_for_pid(&self, pid: u32) -> Option<String> {
        if pid == 0 { return None; }
        unsafe {
            use windows::Win32::System::Threading::{
                OpenProcess, QueryFullProcessImageNameW,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_NAME_FORMAT,
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
            if result.is_err() { return None; }
            let path = String::from_utf16_lossy(&buf[..size as usize]);
            std::path::Path::new(&path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
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
            .map_err(|e| anyhow::anyhow!("close_window: PostMessageW failed for hwnd={}: {}", hwnd, e))
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
                    &info.szDevice[..info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len())]
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
        let request = LayoutRequest::ShowWindow { hwnd: 0, show: true };
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
        assert_eq!(len, 1, "only the freshly-marked hwnd should remain after prune");
    }
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
