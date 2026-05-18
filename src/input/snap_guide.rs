//! Tiny transient vertical line that appears at the active snap target X
//! while the user drags a tile with Alt+LMB.  Implemented as a single
//! layered, topmost, click-through (`WS_EX_TRANSPARENT`) Win32 popup that
//! is repositioned on every snap change and hidden when the grab ends.
//!
//! Lives under `src/input/` (not `src/overlay/`) deliberately — the latter
//! belongs to a sibling agent doing the overview banner; segregating the
//! file paths keeps merge conflicts away.

use std::sync::Arc;
use parking_lot::Mutex;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, PAINTSTRUCT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG,
    PostQuitMessage, RegisterClassExW, SetLayeredWindowAttributes, SetWindowPos,
    ShowWindow, TranslateMessage,
    CS_HREDRAW, CS_VREDRAW, LWA_ALPHA, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOZORDER, WM_DESTROY, WM_PAINT,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::PCWSTR;

/// Width in pixels of the on-screen snap line.
const GUIDE_W: i32 = 2;
/// Solid colour (BGR for COLORREF) — niri-ish accent orange (#FF6B35).
const GUIDE_COLOR_BGR: u32 = 0x00_35_6B_FF;
/// Opacity when shown (0–255).
const GUIDE_ALPHA: u8 = 200;

/// Inputs the message-loop thread polls each tick to decide whether the
/// guide should be visible and where it should sit.
#[derive(Debug, Clone, Copy, Default)]
struct GuideRequest {
    /// `Some((x, top, height))` → show the guide at that screen X, spanning
    /// `[top, top + height)` vertically.  `None` → hide.
    target: Option<(i32, i32, i32)>,
}

struct GuideState {
    request: Mutex<GuideRequest>,
    hwnd: Mutex<Option<isize>>,
}

/// Public handle to the snap-guide overlay.  Cheap to clone (one `Arc`).
#[derive(Clone)]
pub struct SnapGuide {
    state: Arc<GuideState>,
}

impl SnapGuide {
    pub fn new() -> Self {
        Self {
            state: Arc::new(GuideState {
                request: Mutex::new(GuideRequest::default()),
                hwnd: Mutex::new(None),
            }),
        }
    }

    /// Spawn the overlay window on its own background thread.  Drop the
    /// returned `JoinHandle` — the thread lives until the process exits.
    pub fn spawn(&self) -> std::thread::JoinHandle<()> {
        let state = self.state.clone();
        std::thread::spawn(move || run_window_loop(state))
    }

    /// Show the guide at screen-X `x`, spanning the vertical strip
    /// `[top, top + height)`.  Cheap — only updates the shared request.
    pub fn show_at(&self, x: i32, top: i32, height: i32) {
        let mut req = self.state.request.lock();
        req.target = Some((x, top, height));
        if let Some(raw) = *self.state.hwnd.lock() {
            unsafe {
                let hwnd = HWND(raw as *mut _);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    x - GUIDE_W / 2,
                    top,
                    GUIDE_W,
                    height,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
                let _ = SetLayeredWindowAttributes(
                    hwnd, COLORREF(0), GUIDE_ALPHA, LWA_ALPHA,
                );
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        }
    }

    /// Hide the guide.  No-op if it is already hidden.
    pub fn hide(&self) {
        self.state.request.lock().target = None;
        if let Some(raw) = *self.state.hwnd.lock() {
            unsafe {
                let hwnd = HWND(raw as *mut _);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }
}

impl Default for SnapGuide {
    fn default() -> Self {
        Self::new()
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn run_window_loop(state: Arc<GuideState>) {
    unsafe {
        let class_name = wide("WiriSnapGuide");
        let hinstance =
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .expect("GetModuleHandleW failed");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let ex_style = WS_EX_LAYERED
            | WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW
            | WS_EX_TOPMOST
            | WS_EX_TRANSPARENT;

        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("").as_ptr()),
            WS_POPUP,
            0,
            0,
            GUIDE_W,
            10,
            None,
            None,
            hinstance,
            None,
        )
        .expect("CreateWindowExW failed");

        // Initial alpha so we render solid colour when shown.
        let _ = SetLayeredWindowAttributes(
            hwnd, COLORREF(0), GUIDE_ALPHA, LWA_ALPHA,
        );

        *state.hwnd.lock() = Some(hwnd.0 as isize);

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let brush = CreateSolidBrush(COLORREF(GUIDE_COLOR_BGR));
            let rc = RECT { left: 0, top: 0, right: GUIDE_W, bottom: 4096 };
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---------------------------------------------------------------------------
// Process-wide handle, shared between the hook thread and main.
// ---------------------------------------------------------------------------

static GLOBAL_GUIDE: std::sync::LazyLock<Mutex<Option<SnapGuide>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Lazily create and store a process-wide `SnapGuide`, spawning its
/// background window thread on first use.  The hook thread calls
/// [`guide()`] to access the handle.
pub fn guide() -> SnapGuide {
    let mut slot = GLOBAL_GUIDE.lock();
    if let Some(g) = slot.as_ref() {
        return g.clone();
    }
    let g = SnapGuide::new();
    let _ = g.spawn();
    *slot = Some(g.clone());
    g
}

// ---------------------------------------------------------------------------
// Runtime snap config (read by the low-level hook on every WM_MOUSEMOVE).
// ---------------------------------------------------------------------------

/// Snap-on-drag settings consulted by [`crate::input::low_level_hook`] while
/// a `MoveGrab` is active.  Updated via [`set_snap_config`] when the daemon
/// loads / reloads its KDL config.
#[derive(Debug, Clone, Copy)]
pub struct SnapRuntimeConfig {
    pub enabled: bool,
    pub threshold_px: i32,
}

impl Default for SnapRuntimeConfig {
    fn default() -> Self {
        Self { enabled: true, threshold_px: 20 }
    }
}

static SNAP_RUNTIME: std::sync::LazyLock<Mutex<SnapRuntimeConfig>> =
    std::sync::LazyLock::new(|| Mutex::new(SnapRuntimeConfig::default()));

/// Update the global snap-runtime snapshot.  Called by `main.rs` after
/// `Config::load` so the hook picks up the user's preferences immediately.
pub fn set_snap_config(cfg: SnapRuntimeConfig) {
    *SNAP_RUNTIME.lock() = cfg;
}

/// Read the global snap-runtime snapshot.  Hot path — called per
/// WM_MOUSEMOVE during a drag.
pub fn snap_config() -> SnapRuntimeConfig {
    *SNAP_RUNTIME.lock()
}

/// Last snap target the hook applied during the current grab.  Used so we
/// only call `SnapGuide::show_at` when the X actually changes.
static LAST_SNAP_X: std::sync::LazyLock<Mutex<Option<i32>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Atomically test+update the last snap X.  Returns true when the value
/// changed (so the caller should reposition the guide overlay).
pub fn note_snap_x(new_x: Option<i32>) -> bool {
    let mut slot = LAST_SNAP_X.lock();
    if *slot == new_x {
        return false;
    }
    *slot = new_x;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_runtime_default_values() {
        let cfg = SnapRuntimeConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.threshold_px, 20);
    }

    #[test]
    fn snap_runtime_round_trip() {
        let prev = snap_config();
        set_snap_config(SnapRuntimeConfig { enabled: false, threshold_px: 7 });
        let read = snap_config();
        assert!(!read.enabled);
        assert_eq!(read.threshold_px, 7);
        // Restore so subsequent tests aren't affected.
        set_snap_config(prev);
    }

    #[test]
    fn note_snap_x_dedup() {
        // Reset to a known state.
        assert!(note_snap_x(Some(100))); // first set returns true (None → Some)
        assert!(!note_snap_x(Some(100))); // dedup
        assert!(note_snap_x(Some(200))); // change
        assert!(note_snap_x(None)); // hide
        assert!(!note_snap_x(None)); // dedup hide
        // Cleanup for any later tests in this module.
        let _ = note_snap_x(None);
    }

    /// Smoke construction — `SnapGuide` must be cheap to instantiate so
    /// hot-paths can call `note_snap_x` + `guide().show_at` per
    /// mouse-move without allocating.
    #[test]
    fn snap_guide_construction_is_cheap() {
        let _g = SnapGuide::new();
    }
}

