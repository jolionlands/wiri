use anyhow::{Context, Result};
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, TranslateMessage,
    DispatchMessageW,
};

use crate::backend::BackendEvent;
use super::{BackendHandle, WindowInfo};

// Event constants for SetWinEventHook
const EVENT_SYSTEM_ALERT: u32 = 0x0002;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MOVESIZESTART: u32 = 0x000A;
const EVENT_SYSTEM_MOVESIZEEND: u32 = 0x000B;
const EVENT_OBJECT_CREATE: u32 = 0x8000;
const EVENT_OBJECT_DESTROY: u32 = 0x8001;
const EVENT_OBJECT_SHOW: u32 = 0x8003;
const EVENT_OBJECT_HIDE: u32 = 0x8004;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800C;

const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;

#[derive(Debug)]
pub struct WinEventHook {
    #[allow(dead_code)]
    handle: windows::Win32::Foundation::HMODULE,
    hooks: Vec<HWINEVENTHOOK>,
}

impl WinEventHook {
    pub fn new(_backend: BackendHandle) -> Result<Self> {
        let handle = unsafe {
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
        }
        .context("GetModuleHandleW failed")?;

        // Register separate hooks for each event range
        let event_ranges: &[(u32, u32)] = &[
            // EVENT_SYSTEM_ALERT (0x0002) — fires when an app calls FlashWindow(Ex)
            // to request user attention. We use it to populate the urgent-window
            // set so window rules with `is-urgent` matchers can react.
            (EVENT_SYSTEM_ALERT, EVENT_SYSTEM_ALERT),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_OBJECT_CREATE, EVENT_OBJECT_CREATE),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
            (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
        ];

        let mut hooks = Vec::new();
        for (event_min, event_max) in event_ranges {
            let hook = unsafe {
                SetWinEventHook(
                    *event_min,
                    *event_max,
                    handle,
                    Some(winevent_proc),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                warn!("SetWinEventHook failed for range 0x{:04X}-0x{:04X}", event_min, event_max);
            } else {
                info!("Registered WinEvent hook for range 0x{:04X}-0x{:04X}", event_min, event_max);
                hooks.push(hook);
            }
        }

        if hooks.is_empty() {
            return Err(anyhow::anyhow!("All SetWinEventHook registrations failed"));
        }

        Ok(Self { handle, hooks })
    }

    /// Start a message pump thread for WinEvent hook callbacks.
    /// GetMessageW blocks until a message arrives — no sleep needed.
    pub fn run(&self) {
        info!("WinEventHook running ({} hooks active)", self.hooks.len());
        std::thread::spawn(move || {
            let mut msg = MSG::default();
            loop {
                let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if ret.0 == 0 || ret.0 == -1 {
                    break;
                }
                let _ = unsafe { TranslateMessage(&msg) };
                unsafe { DispatchMessageW(&msg); }
            }
        });
    }
}

impl Drop for WinEventHook {
    fn drop(&mut self) {
        for hook in &self.hooks {
            unsafe {
                let _ = windows::Win32::UI::Accessibility::UnhookWinEvent(*hook);
            }
        }
        info!("WinEventHook dropped, {} hooks unregistered", self.hooks.len());
    }
}

unsafe extern "system" fn winevent_proc(
    _event_hook: HWINEVENTHOOK,
    event_type: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _id_thread: u32,
    _timestamp: u32,
) {
    if id_object != 0 || id_child != 0 {
        return;
    }

    // Clone handle out of the lock before making any calls to avoid holding
    // the mutex across potentially re-entrant add_window / send_event calls.
    let backend = {
        let guard = BACKEND_STORAGE.lock();
        match guard.as_ref() {
            Some(b) => b.clone(),
            None => return,
        }
    };

    match event_type {
        0x0002 => { // EVENT_SYSTEM_ALERT — record window as flashing/urgent.
            let hwnd_isize = hwnd.0 as isize;
            URGENT_WINDOWS.lock().insert(hwnd_isize);
        }
        0x0003 => { // EVENT_SYSTEM_FOREGROUND
            // Bringing a window to the foreground implicitly clears its urgent state
            // — the user is now looking at it, no need to keep nagging.
            URGENT_WINDOWS.lock().remove(&(hwnd.0 as isize));
            backend.send_event(BackendEvent::ForegroundChanged { hwnd: hwnd.0 as isize });
        }
        0x000A => { // EVENT_SYSTEM_MOVESIZESTART
            backend.send_event(BackendEvent::WindowMoveResizeStart { hwnd: hwnd.0 as isize });
        }
        0x000B => { // EVENT_SYSTEM_MOVESIZEEND
            backend.send_event(BackendEvent::WindowMoveResizeEnd { hwnd: hwnd.0 as isize });
        }
        0x8000 => { // EVENT_OBJECT_CREATE
            let info = WindowInfo::new(hwnd);
            backend.send_event(BackendEvent::WindowCreated { hwnd: hwnd.0 as isize });
            backend.add_window(hwnd.0 as isize, info);
        }
        0x8001 => { // EVENT_OBJECT_DESTROY
            let hwnd_isize = hwnd.0 as isize;
            // Prune LAST_LOCATION entry for closed windows (Bug #6)
            LAST_LOCATION.lock().remove(&hwnd_isize);
            // Also drop urgent flag — destroyed windows can't be flashing.
            URGENT_WINDOWS.lock().remove(&hwnd_isize);
            backend.send_event(BackendEvent::WindowDestroyed { hwnd: hwnd_isize });
            backend.remove_window(hwnd_isize);
        }
        0x8003 => { // EVENT_OBJECT_SHOW
            if let Some(mut info) = backend.get_window(hwnd.0 as isize) {
                info.is_visible = true;
                backend.update_window(hwnd.0 as isize, info.clone());
                backend.send_event(BackendEvent::WindowShown { hwnd: hwnd.0 as isize });
            }
        }
        0x8004 => { // EVENT_OBJECT_HIDE
            if let Some(mut info) = backend.get_window(hwnd.0 as isize) {
                info.is_visible = false;
                backend.update_window(hwnd.0 as isize, info.clone());
                backend.send_event(BackendEvent::WindowHidden { hwnd: hwnd.0 as isize });
            }
        }
        0x800B => { // EVENT_OBJECT_LOCATIONCHANGE
            let hwnd_isize = hwnd.0 as isize;

            // Fast-path: if wiri itself just moved this window, skip the entire
            // handler.  This breaks the SetWindowPos → LOCATIONCHANGE → send_event
            // → apply_all → SetWindowPos ping-pong without relying solely on the
            // 200 ms LAST_LOCATION throttle.
            if backend.is_self_applied_recent(hwnd_isize) {
                return;
            }

            // Throttle: skip if we processed a location change for this
            // window in the last 200ms. These fire 100s of times per second
            // during window movement and cause massive lag.
            let should_process = {
                let mut last = LAST_LOCATION.lock();
                let now = std::time::Instant::now();
                let prev = last.get(&hwnd_isize);
                let process = prev.map_or(true, |(t, _)| now.duration_since(*t).as_millis() > 200);
                if process {
                    // We'll update the stored rect after we get the new bounds below;
                    // for now just stamp the time with a placeholder rect that we'll
                    // overwrite immediately after the bounds refresh.
                    let placeholder = prev.map(|(_, r)| *r).unwrap_or(UtilRect::new(0, 0, 0, 0));
                    last.insert(hwnd_isize, (now, placeholder));
                    // Periodic prune: drop stale entries when map grows large (Bug #6)
                    if last.len() > 1024 {
                        let cutoff = now - std::time::Duration::from_secs(60);
                        last.retain(|_, (t, _)| *t > cutoff);
                    }
                }
                process
            };
            if should_process {
                if let Some(mut info) = backend.get_window(hwnd_isize) {
                    // Capture previous rect before refresh so we can compare.
                    let prev_rect = {
                        let last = LAST_LOCATION.lock();
                        last.get(&hwnd_isize).map(|(_, r)| *r)
                    };
                    info.refresh_bounds();
                    let new_rect = info.bounds;
                    // Store the updated rect for future comparisons.
                    {
                        let mut last = LAST_LOCATION.lock();
                        if let Some(entry) = last.get_mut(&hwnd_isize) {
                            entry.1 = new_rect;
                        }
                    }
                    // Determine what changed and emit the right event(s).
                    let size_changed = prev_rect.map_or(true, |pr| {
                        pr.size.w != new_rect.size.w || pr.size.h != new_rect.size.h
                    });
                    let loc_changed = prev_rect.map_or(true, |pr| {
                        pr.loc.x != new_rect.loc.x || pr.loc.y != new_rect.loc.y
                    });
                    if size_changed {
                        // Size change is layout-relevant; emit WindowResized (subsumes move).
                        backend.send_event(BackendEvent::WindowResized {
                            hwnd: hwnd_isize,
                            rect: new_rect,
                        });
                    } else if loc_changed {
                        backend.send_event(BackendEvent::WindowMoved {
                            hwnd: hwnd_isize,
                            rect: new_rect,
                        });
                    }
                    backend.update_window(hwnd_isize, info);
                }
            }
        }
        0x800C => { // EVENT_OBJECT_NAMECHANGE
            backend.send_event(BackendEvent::WindowTitleChanged { hwnd: hwnd.0 as isize });
        }
        _ => {}
    }
}

use std::sync::LazyLock;
use std::collections::{HashMap, HashSet};
use crate::utils::Rect as UtilRect;
/// Per-window location throttle state: (last_processed_time, last_known_rect)
static LAST_LOCATION: LazyLock<Mutex<HashMap<isize, (std::time::Instant, UtilRect)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// HWNDs currently in the urgent/flashing state. Populated by the
/// EVENT_SYSTEM_ALERT WinEvent callback, cleared on EVENT_SYSTEM_FOREGROUND
/// and EVENT_OBJECT_DESTROY. Consulted by `urgent_hwnds()` and the
/// `crate::window::is_window_flashing` helper.
static URGENT_WINDOWS: LazyLock<Mutex<HashSet<isize>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Return true if `hwnd` is currently in the urgent/flashing state. Cheap to
/// call — just a HashSet lookup behind a Mutex.
pub fn is_urgent_hwnd(hwnd: isize) -> bool {
    URGENT_WINDOWS.lock().contains(&hwnd)
}

/// Returns a snapshot of all HWNDs currently flagged as urgent.
pub fn urgent_hwnds() -> Vec<isize> {
    URGENT_WINDOWS.lock().iter().copied().collect()
}

/// Manually clear the urgent flag for a window (e.g. when the engine
/// programmatically focuses it without a WinEvent firing).
pub fn clear_urgent(hwnd: isize) {
    URGENT_WINDOWS.lock().remove(&hwnd);
}

lazy_static::lazy_static! {
    static ref BACKEND_STORAGE: Mutex<Option<BackendHandle>> = Mutex::new(None);
}

pub fn set_backend(backend: BackendHandle) {
    *BACKEND_STORAGE.lock() = Some(backend);
}

pub fn clear_backend() {
    *BACKEND_STORAGE.lock() = None;
}

/// Re-enumerate displays and emit `MonitorConnected` events for each detected
/// monitor. Called from the hotkey-window message loop when WM_DISPLAYCHANGE
/// fires. The main event handler diffs the result against the engine's
/// registered monitors and adds/removes as needed.
pub fn notify_display_change() {
    use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC};

    // Snapshot current monitors via EnumDisplayMonitors; each hit emits an event
    // through the BackendHandle. We don't try to detect which monitor went away
    // here — the main handler does a diff against the engine's registered set.
    unsafe extern "system" fn enum_proc(
        hmonitor: windows::Win32::Graphics::Gdi::HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        _lparam: LPARAM,
    ) -> BOOL {
        if let Some(info) = super::MonitorInfo::from_hmonitor(hmonitor) {
            if let Some(backend) = BACKEND_STORAGE.lock().as_ref() {
                backend.send_event(BackendEvent::MonitorConnected {
                    id: info.id.as_u64(),
                });
            }
        }
        BOOL(1)
    }

    unsafe {
        let _ = EnumDisplayMonitors(HDC::default(), None, Some(enum_proc), LPARAM(0));
    }

    // Also fire a synthetic MonitorDisconnected so the main handler runs its
    // diff pass and unregisters any monitors that vanished. The event handler
    // ignores the `id` field on Disconnected (it diffs the full set).
    if let Some(backend) = BACKEND_STORAGE.lock().as_ref() {
        backend.send_event(BackendEvent::MonitorDisconnected { id: 0 });
    }
}
