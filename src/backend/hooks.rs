use anyhow::Result;
use parking_lot::Mutex;
use std::time::Duration;
use tracing::{info, warn};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_USER,
};

use super::{BackendHandle, WindowInfo};
use crate::backend::BackendEvent;

// Event constants for SetWinEventHook
const EVENT_SYSTEM_ALERT: u32 = 0x0002;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MOVESIZESTART: u32 = 0x000A;
const EVENT_SYSTEM_MOVESIZEEND: u32 = 0x000B;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_CREATE: u32 = 0x8000;
const EVENT_OBJECT_DESTROY: u32 = 0x8001;
const EVENT_OBJECT_SHOW: u32 = 0x8002;
const EVENT_OBJECT_HIDE: u32 = 0x8003;
const EVENT_OBJECT_REORDER: u32 = 0x8004;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800C;

const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;
const WM_STOP_WINEVENT_LOOP: u32 = WM_USER + 401;

#[derive(Debug)]
pub struct WinEventHook {
    #[allow(dead_code)]
    thread_id: u32,
    hook_count: usize,
}

impl WinEventHook {
    pub fn new(_backend: BackendHandle) -> Result<Self> {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("wiri-winevent-hook".to_string())
            .spawn(move || {
                let thread_id = unsafe { GetCurrentThreadId() };
                let hooks = register_winevent_hooks();
                if hooks.is_empty() {
                    let _ = ready_tx.send(Err(anyhow::anyhow!(
                        "All SetWinEventHook registrations failed"
                    )));
                    return;
                }

                let hook_count = hooks.len();
                let _ = ready_tx.send(Ok((thread_id, hook_count)));
                run_winevent_message_loop();
                for hook in hooks {
                    unsafe {
                        let _ = windows::Win32::UI::Accessibility::UnhookWinEvent(hook);
                    }
                }
                info!(
                    "WinEventHook thread exited, {} hooks unregistered",
                    hook_count
                );
            })?;

        let (thread_id, hook_count) = ready_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| anyhow::anyhow!("WinEventHook did not start: {}", e))??;

        Ok(Self {
            thread_id,
            hook_count,
        })
    }

    /// Hook registration and pumping start in `new`; retained for callers that
    /// still invoke `run` after construction.
    pub fn run(&self) {
        info!("WinEventHook running ({} hooks active)", self.hook_count);
    }
}

impl Drop for WinEventHook {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_STOP_WINEVENT_LOOP, WPARAM(0), LPARAM(0));
        }
        info!("WinEventHook stop requested");
    }
}

fn register_winevent_hooks() -> Vec<HWINEVENTHOOK> {
    let handle = match unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) } {
        Ok(handle) => handle,
        Err(e) => {
            warn!("GetModuleHandleW failed for WinEventHook: {}", e);
            windows::Win32::Foundation::HMODULE::default()
        }
    };

    let event_ranges: &[(u32, u32)] = &[
        (EVENT_SYSTEM_ALERT, EVENT_SYSTEM_ALERT),
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_CREATE, EVENT_OBJECT_CREATE),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
        (EVENT_OBJECT_SHOW, EVENT_OBJECT_REORDER),
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
            warn!(
                "SetWinEventHook failed for range 0x{:04X}-0x{:04X}",
                event_min, event_max
            );
        } else {
            info!(
                "Registered WinEvent hook for range 0x{:04X}-0x{:04X}",
                event_min, event_max
            );
            hooks.push(hook);
        }
    }
    hooks
}

fn run_winevent_message_loop() {
    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 == 0 || ret.0 == -1 || msg.message == WM_STOP_WINEVENT_LOOP {
            break;
        }
        let _ = unsafe { TranslateMessage(&msg) };
        unsafe {
            DispatchMessageW(&msg);
        }
    }
}

/// USER reports top-level reorders on OBJID_WINDOW or the desktop's OBJID_CLIENT.
/// Other accessibility controls/children must not turn into whole-desktop repairs.
fn window_event_object(event: u32, object: i32, child: i32) -> bool {
    child == 0 && (object == 0 || (event == EVENT_OBJECT_REORDER && object == -4))
}

#[cfg(test)]
mod event_tests {
    use super::*;

    #[test]
    fn reorder_accepts_desktop_client_but_not_accessibility_children() {
        assert!(window_event_object(EVENT_OBJECT_REORDER, 0, 0));
        assert!(window_event_object(EVENT_OBJECT_REORDER, -4, 0));
        assert!(!window_event_object(EVENT_OBJECT_REORDER, -4, 1));
        assert!(!window_event_object(EVENT_OBJECT_REORDER, -8, 0));
        assert!(!window_event_object(EVENT_OBJECT_LOCATIONCHANGE, -4, 0));
    }

    #[test]
    fn show_hide_and_reorder_use_distinct_native_event_ids() {
        assert_eq!(EVENT_OBJECT_SHOW, 0x8002);
        assert_eq!(EVENT_OBJECT_HIDE, 0x8003);
        assert_eq!(EVENT_OBJECT_REORDER, 0x8004);
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
    if !window_event_object(event_type, id_object, id_child) {
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
        0x0002 => {
            // EVENT_SYSTEM_ALERT — record window as flashing/urgent.
            let hwnd_isize = hwnd.0 as isize;
            URGENT_WINDOWS.lock().insert(hwnd_isize);
            // Notify the engine so it can update window rules / border colour.
            backend.send_event(BackendEvent::WindowUrgent {
                hwnd: hwnd_isize,
                urgent: true,
            });
        }
        0x0003 => {
            // EVENT_SYSTEM_FOREGROUND
            // Bringing a window to the foreground implicitly clears its urgent state
            // — the user is now looking at it, no need to keep nagging.
            let hwnd_isize = hwnd.0 as isize;
            let was_urgent = URGENT_WINDOWS.lock().remove(&hwnd_isize);
            if was_urgent {
                // Emit the cleared signal so the engine can un-highlight the border.
                backend.send_event(BackendEvent::WindowUrgentCleared { hwnd: hwnd_isize });
            }
            backend.send_event(BackendEvent::ForegroundChanged { hwnd: hwnd_isize });
            // The window came to the front: whatever is drawn around it goes directly below it.
            if let Some(decor) = crate::overlay::decor::global() {
                decor.restack(hwnd_isize);
            }
        }
        0x000A => {
            // EVENT_SYSTEM_MOVESIZESTART
            backend.send_event(BackendEvent::WindowMoveResizeStart {
                hwnd: hwnd.0 as isize,
            });
        }
        0x000B => {
            // EVENT_SYSTEM_MOVESIZEEND
            backend.send_event(BackendEvent::WindowMoveResizeEnd {
                hwnd: hwnd.0 as isize,
            });
        }
        0x0016 => {
            // EVENT_SYSTEM_MINIMIZESTART
            backend.send_event(BackendEvent::WindowMinimized {
                hwnd: hwnd.0 as isize,
            });
        }
        0x0017 => {
            // EVENT_SYSTEM_MINIMIZEEND
            backend.send_event(BackendEvent::WindowRestored {
                hwnd: hwnd.0 as isize,
            });
        }
        0x8000 => {
            // EVENT_OBJECT_CREATE
            let info = WindowInfo::new(hwnd);
            backend.send_event(BackendEvent::WindowCreated {
                hwnd: hwnd.0 as isize,
            });
            backend.add_window(hwnd.0 as isize, info);
        }
        0x8001 => {
            // EVENT_OBJECT_DESTROY
            let hwnd_isize = hwnd.0 as isize;
            // Prune LAST_LOCATION entry for closed windows (Bug #6)
            LAST_LOCATION.lock().remove(&hwnd_isize);
            SIZE_CLASS.lock().remove(&hwnd_isize);
            // Also drop urgent flag — destroyed windows can't be flashing.
            URGENT_WINDOWS.lock().remove(&hwnd_isize);
            backend.send_event(BackendEvent::WindowDestroyed { hwnd: hwnd_isize });
            backend.remove_window(hwnd_isize);
        }
        EVENT_OBJECT_SHOW => {
            // EVENT_OBJECT_SHOW
            // A SHOW event is the first reliable point at which a top-level
            // window has its real title / class / styles set. The earlier
            // EVENT_OBJECT_CREATE often fires before WS_VISIBLE and before
            // WM_SETTEXT, so the cached WindowInfo there is half-populated
            // (empty title => filtered out by `should_tile_window`).
            //
            // Refresh from the live HWND on every SHOW so the engine sees the
            // current title/class/bounds. If the window was never seen before
            // (CREATE missed it — e.g. raced with the hook install, or it was
            // created hidden and only now made visible), insert it fresh.
            let hwnd_isize = hwnd.0 as isize;
            let mut info = WindowInfo::new(hwnd);
            info.is_visible = true;
            backend.update_window(hwnd_isize, info);
            backend.send_event(BackendEvent::WindowShown { hwnd: hwnd_isize });
        }
        EVENT_OBJECT_HIDE => {
            // EVENT_OBJECT_HIDE
            if let Some(mut info) = backend.get_window(hwnd.0 as isize) {
                info.is_visible = false;
                backend.update_window(hwnd.0 as isize, info.clone());
                backend.send_event(BackendEvent::WindowHidden {
                    hwnd: hwnd.0 as isize,
                });
            }
        }
        EVENT_OBJECT_REORDER => {
            // Top-level z-order changes may name the desktop's client object, not
            // the tile that moved. Re-read the actual stack, never classify this as HIDE.
            backend.send_event(BackendEvent::ZOrderChanged);
        }
        0x800B => {
            // EVENT_OBJECT_LOCATIONCHANGE
            let hwnd_isize = hwnd.0 as isize;

            // Decorations also follow our own moves (interactive grabs and animation frames),
            // before the layout feedback guard. The layer never requests a layout itself.
            if let Some(decor) = crate::overlay::decor::global() {
                decor.moved(hwnd_isize);
            }

            // Fast-path: if wiri itself just moved this window, skip the layout handler.
            // This breaks SetWindowPos → LOCATIONCHANGE → apply_all ping-pong.
            if backend.is_self_applied_recent(hwnd_isize) {
                return;
            }

            // A window that became maximized or as big as its monitor (or stopped being) on
            // its own is reported at once: the throttle below drops events, and the one that
            // is dropped can be the last of the burst.
            detect_size_class_change(&backend, hwnd);

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
        0x800C => {
            // EVENT_OBJECT_NAMECHANGE
            // Refresh the cached WindowInfo's title (and class/bounds for
            // good measure — apps occasionally update class via SetClassLongPtr
            // mid-life). Then emit BOTH WindowTitleChanged (for listeners) and
            // WindowShown (to retry classification): a window whose title was
            // empty at CREATE time is rejected by should_tile_window, and
            // NAMECHANGE is the signal that the title is now real.
            let hwnd_isize = hwnd.0 as isize;
            if unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(hwnd).as_bool() } {
                let info = WindowInfo::new(hwnd);
                backend.update_window(hwnd_isize, info);
                backend.send_event(BackendEvent::WindowTitleChanged { hwnd: hwnd_isize });
                backend.send_event(BackendEvent::WindowShown { hwnd: hwnd_isize });
            }
        }
        _ => {}
    }
}

use crate::utils::Rect as UtilRect;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
/// Per-window location throttle state: (last_processed_time, last_known_rect)
static LAST_LOCATION: LazyLock<Mutex<HashMap<isize, (std::time::Instant, UtilRect)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// What the size of a window amounts to for the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SizeClass {
    /// The window has the native maximized state (title bar button, double click).
    zoomed: bool,
    /// The window is as big as the monitor it is on (a video player, F11 in a browser).
    covers_monitor: bool,
}

/// The size class last seen of each window the engine tracks.
static SIZE_CLASS: LazyLock<Mutex<HashMap<isize, SizeClass>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// How far (in pixels) the edges of a window may be from the edges of its monitor for the window
/// to count as covering it: the frames of some applications are a pixel off.
const COVER_TOLERANCE_PX: i32 = 3;

/// True when the visible frame `frame` is the size and place of `monitor` (within a few pixels).
/// A window that is bigger than its monitor, or only most of it, does not cover it.
pub fn covers_monitor(frame: UtilRect, monitor: UtilRect) -> bool {
    let near = |a: i32, b: i32| (a - b).abs() <= COVER_TOLERANCE_PX;
    near(frame.loc.x, monitor.loc.x)
        && near(frame.loc.y, monitor.loc.y)
        && near(frame.right(), monitor.right())
        && near(frame.bottom(), monitor.bottom())
}

/// Look at a window the engine tracks after a location change and send
/// `BackendEvent::WindowSizeClassChanged` when its size class differs from the one seen last.
/// A window seen for the first time that is nothing special sends nothing.
fn detect_size_class_change(backend: &BackendHandle, hwnd: HWND) {
    let hwnd_isize = hwnd.0 as isize;
    let Some(info) = backend.get_window(hwnd_isize) else {
        return; // not a window wiri tracks
    };
    let zoomed = unsafe { windows::Win32::UI::WindowsAndMessaging::IsZoomed(hwnd).as_bool() };
    let covers = match info.get_dwm_bounds() {
        Some(frame) => backend
            .monitor_for_window(hwnd_isize)
            .and_then(|id| backend.get_monitor(id))
            .map_or(false, |monitor| covers_monitor(frame, monitor.bounds)),
        None => false,
    };
    let now = SizeClass {
        zoomed,
        covers_monitor: covers,
    };
    let changed = {
        let mut seen = SIZE_CLASS.lock();
        let previous = seen.insert(hwnd_isize, now);
        match previous {
            Some(previous) => previous != now,
            None => zoomed || covers,
        }
    };
    if changed {
        backend.send_event(BackendEvent::WindowSizeClassChanged {
            hwnd: hwnd_isize,
            zoomed,
            covers_monitor: covers,
        });
    }
}

/// HWNDs currently in the urgent/flashing state. Populated by the
/// EVENT_SYSTEM_ALERT WinEvent callback, cleared on EVENT_SYSTEM_FOREGROUND
/// and EVENT_OBJECT_DESTROY. Consulted by `urgent_hwnds()` and the
/// `crate::window::is_window_flashing` helper.
static URGENT_WINDOWS: LazyLock<Mutex<HashSet<isize>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// An actual shell flash notification. Kept separate from generic state changes.
pub(crate) fn attention_requested(hwnd: isize) {
    let backend = BACKEND_STORAGE.lock().clone();
    let Some(backend) = backend else {
        return;
    };
    if backend.get_window(hwnd).is_some() && URGENT_WINDOWS.lock().insert(hwnd) {
        backend.send_event(BackendEvent::WindowUrgent { hwnd, urgent: true });
    }
}

/// Return true if `hwnd` is currently in the urgent/flashing state. Cheap to
/// call — just a HashSet lookup behind a Mutex.
pub fn is_urgent_hwnd(hwnd: isize) -> bool {
    URGENT_WINDOWS.lock().contains(&hwnd)
}

/// Resolve a queued attention event against foreground state. A late shell
/// flash can arrive after foreground clearing; rejecting it must also clear the
/// native registry, or the next background flash is deduplicated and lost.
pub(crate) fn resolve_attention(hwnd: isize, foreground: bool) -> bool {
    pending_attention(&mut URGENT_WINDOWS.lock(), hwnd, foreground)
}

fn pending_attention(urgent: &mut HashSet<isize>, hwnd: isize, foreground: bool) -> bool {
    if foreground {
        urgent.remove(&hwnd);
        false
    } else {
        urgent.contains(&hwnd)
    }
}

/// Returns a snapshot of all HWNDs currently flagged as urgent.
pub fn urgent_hwnds() -> Vec<isize> {
    URGENT_WINDOWS.lock().iter().copied().collect()
}

/// Set or reset a window's urgent flag in the same registry used by WinEvents.
/// This only changes bookkeeping; it does not flash or focus the window.
pub fn set_urgent(hwnd: isize, urgent: bool) {
    let mut windows = URGENT_WINDOWS.lock();
    if urgent {
        windows.insert(hwnd);
    } else {
        windows.remove(&hwnd);
    }
}

/// Manually clear the urgent flag for a window (e.g. when the engine
/// programmatically focuses it without a WinEvent firing).
pub fn clear_urgent(hwnd: isize) {
    set_urgent(hwnd, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_urgent_updates_the_shared_registry() {
        let hwnd = -73_900_001;
        clear_urgent(hwnd);
        assert!(!is_urgent_hwnd(hwnd));
        set_urgent(hwnd, true);
        set_urgent(hwnd, true);
        assert!(is_urgent_hwnd(hwnd));
        assert!(urgent_hwnds().contains(&hwnd));
        set_urgent(hwnd, false);
        assert!(!is_urgent_hwnd(hwnd));
        assert!(!urgent_hwnds().contains(&hwnd));
        set_urgent(hwnd, true);
        clear_urgent(hwnd);
        assert!(!is_urgent_hwnd(hwnd));
    }
}

#[cfg(test)]
mod attention_tests {
    use super::*;

    #[test]
    fn late_foreground_flash_does_not_swallow_the_next_background_request() {
        let mut urgent = HashSet::from([7, 9]);
        assert!(!pending_attention(&mut urgent, 7, true));
        assert!(!urgent.contains(&7));
        assert!(urgent.contains(&9));
        // The next shell flash must produce a fresh insertion / engine event.
        assert!(urgent.insert(7));
        assert!(pending_attention(&mut urgent, 7, false));
        assert!(urgent.contains(&7));
        assert!(!pending_attention(&mut urgent, 8, false));
    }
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
