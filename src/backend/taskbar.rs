//! The taskbar and fullscreen windows.
//!
//! The shell hides the taskbar by itself for a window that looks fullscreen to it (no frame,
//! the size of its monitor), but only when it looks at the window at the right moment: a window
//! that wiri stripped and placed over the monitor, or one that an application made fullscreen on
//! its own, is not always noticed, and the taskbar stays on top of its bottom edge.
//! `ITaskbarList2::MarkFullscreenWindow` tells the shell: this window is fullscreen (the taskbar
//! is hidden while it is the foreground window) or it is not any more.

use std::cell::Cell;
use std::ffi::c_void;
use tracing::{debug, warn};
use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Shell::{ITaskbarList2, TaskbarList};

thread_local! {
    static COM_READY: Cell<bool> = Cell::new(false);
}

/// The shell object is COM: the thread joins the multithreaded apartment first (a thread that
/// is a single-threaded apartment already keeps what it has).
fn ensure_com() {
    COM_READY.with(|ready| {
        if !ready.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            ready.set(true);
        }
    });
}

/// Tell the shell that `hwnd` is (or is not any more) a fullscreen window.  Best effort: it
/// returns whether the shell took the call, and a failure is only logged (the taskbar stays as
/// the shell decides by itself).
pub fn mark_fullscreen(hwnd: isize, fullscreen: bool) -> bool {
    // The engine's unit tests use window handles that are not windows: the shell is left out.
    if cfg!(test) {
        return true;
    }
    ensure_com();
    unsafe {
        let list: ITaskbarList2 = match CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) {
            Ok(list) => list,
            Err(e) => {
                warn!("taskbar: the shell's taskbar list is not available: {}", e);
                return false;
            }
        };
        if let Err(e) = list.HrInit() {
            warn!("taskbar: HrInit failed: {}", e);
            return false;
        }
        match list.MarkFullscreenWindow(HWND(hwnd as *mut c_void), BOOL::from(fullscreen)) {
            Ok(()) => {
                debug!(
                    "taskbar: window {} marked {}",
                    hwnd,
                    if fullscreen { "fullscreen" } else { "not fullscreen" }
                );
                true
            }
            Err(e) => {
                warn!("taskbar: MarkFullscreenWindow({}, {}) failed: {}", hwnd, fullscreen, e);
                false
            }
        }
    }
}
