//! Workspace-switcher overlay — a small painted top-center window that fades
//! in over ~100 ms when `show()` is called, holds for ~1.5 s, then fades out
//! over the last ~300 ms.  Re-calling `show()` resets the timer.
//!
//! Multi-monitor: callers may pass an optional `monitor_bounds` rect via
//! [`WorkspaceIndicator::show_at`] to place the overlay over a specific
//! monitor.  [`WorkspaceIndicator::show`] keeps the legacy primary-monitor
//! placement for backwards compatibility (main.rs is owned by another agent).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, FillRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor, HFONT,
    PAINTSTRUCT, DT_CENTER, DT_SINGLELINE, DT_VCENTER, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GetMessageW, GetSystemMetrics, KillTimer, MSG, PostQuitMessage,
    RegisterClassExW, SetLayeredWindowAttributes, SetTimer, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    LWA_ALPHA, SM_CXSCREEN, SWP_NOSIZE, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_HIDE, SW_SHOWNOACTIVATE,
    WNDCLASSEXW,
    WM_CREATE, WM_DESTROY, WM_PAINT, WM_TIMER, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::PCWSTR;

/// Convenience rectangle in screen space (logical pixels).  Mirrors the small
/// data we want from `crate::utils::Rect` without pulling its full API into
/// the overlay module.
#[derive(Debug, Clone, Copy)]
pub struct MonitorBounds {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

// Overlay window geometry
const OVERLAY_W: i32 = 280;
const OVERLAY_H: i32 = 64;
// Timer ID and interval
const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 16; // ~60 Hz so fade-in feels smooth
// Fade-out begins this many ms before expiry
const FADE_OUT_MS: u128 = 300;
// Fade-in completes this many ms after show()
const FADE_IN_MS: u128 = 100;
// Total display time (including fade-in + fade-out)
const DISPLAY_DURATION: Duration = Duration::from_millis(1500);

/// State shared between the public API and the window procedure.
struct IndicatorState {
    text: Mutex<String>,
    /// Wall-clock when the current `show()` call was issued. Used to drive
    /// the fade-in curve.
    shown_at: Mutex<Option<Instant>>,
    /// Wall-clock when the overlay should be fully hidden.
    expires_at: Mutex<Option<Instant>>,
    hwnd: Mutex<Option<isize>>,
    /// Optional monitor bounds to centre on (None = primary monitor).
    /// Re-read on every WM_TIMER tick so updates take effect immediately.
    monitor_bounds: Mutex<Option<MonitorBounds>>,
}

/// Public handle to the workspace indicator overlay.
///
/// Create with `WorkspaceIndicator::new()`, then call `overlay.clone().spawn()`
/// once from `main`.  After that, call `overlay.show("Workspace 1")` whenever
/// the user switches workspaces.
pub struct WorkspaceIndicator {
    state: Arc<IndicatorState>,
}

impl WorkspaceIndicator {
    pub fn new() -> Self {
        Self {
            state: Arc::new(IndicatorState {
                text: Mutex::new(String::new()),
                shown_at: Mutex::new(None),
                expires_at: Mutex::new(None),
                hwnd: Mutex::new(None),
                monitor_bounds: Mutex::new(None),
            }),
        }
    }

    /// Show the overlay with `label` on the primary monitor for ~1.5 s.
    /// Safe to call repeatedly; each call resets the fade-in + fade-out timer.
    pub fn show(&self, label: &str) {
        self.show_internal(label, None);
    }

    /// Show the overlay with `label` centred horizontally on the monitor
    /// described by `bounds`.  Use this from multi-monitor setups so the
    /// indicator always lands above the workspace it relates to.
    pub fn show_at(&self, label: &str, bounds: MonitorBounds) {
        self.show_internal(label, Some(bounds));
    }

    fn show_internal(&self, label: &str, bounds: Option<MonitorBounds>) {
        let now = Instant::now();
        *self.state.text.lock() = label.to_string();
        *self.state.shown_at.lock() = Some(now);
        *self.state.expires_at.lock() = Some(now + DISPLAY_DURATION);
        if let Some(b) = bounds {
            *self.state.monitor_bounds.lock() = Some(b);
        }

        if let Some(raw) = *self.state.hwnd.lock() {
            unsafe {
                let hwnd = HWND(raw as *mut _);
                // Start invisible — the timer-driven fade-in will lerp alpha
                // from 0 → 255 over FADE_IN_MS.
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
                // Reposition to the requested monitor BEFORE showing so the
                // window never flashes at the previous location.
                reposition_to_monitor(hwnd, bounds);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = InvalidateRect(hwnd, None, true);
            }
        }
    }

    /// Spawn the background thread that owns the overlay HWND and pumps its
    /// messages.  Drop the returned `JoinHandle` — the thread lives until the
    /// process exits.
    pub fn spawn(self: Arc<Self>) -> std::thread::JoinHandle<()> {
        let state = self.state.clone();
        std::thread::spawn(move || run_window_loop(state))
    }
}

/// Centre an HWND of `OVERLAY_W × OVERLAY_H` at the top of either the
/// supplied monitor bounds or — when `bounds` is None — the primary monitor.
fn reposition_to_monitor(hwnd: HWND, bounds: Option<MonitorBounds>) {
    let (x, y) = match bounds {
        Some(b) => (b.x + (b.w - OVERLAY_W) / 2, b.y + 24),
        None => {
            let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            ((screen_w - OVERLAY_W) / 2, 24)
        }
    };
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x, y, 0, 0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

impl Default for WorkspaceIndicator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Win32 window loop (runs on its own thread)
// ---------------------------------------------------------------------------

/// Wide-string helper — appends a NUL terminator.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn run_window_loop(state: Arc<IndicatorState>) {
    unsafe {
        // ----- Register window class ----------------------------------------
        let class_name = wide("WiriWorkspaceIndicator");
        let hinstance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
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

        // ----- Compute initial position --------------------------------------
        // We default to the primary monitor; show_at()/show_internal() will
        // reposition before each fade-in if a monitor bounds was supplied.
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let x = (screen_w - OVERLAY_W) / 2;
        let y = 24;

        // ----- Create layered, topmost, no-activate popup --------------------
        let ex_style = WS_EX_LAYERED
            | WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW
            | WS_EX_TOPMOST
            | WS_EX_TRANSPARENT;

        // We pass a raw pointer to `state` as the creation parameter so the
        // window proc can store it on WM_CREATE before we call SetWindowLongPtrW.
        // Leak a clone; we'll reclaim it on WM_DESTROY via Box::from_raw.
        let state_ptr = Arc::into_raw(state.clone()) as *mut IndicatorState;

        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("").as_ptr()),
            WS_POPUP,
            x,
            y,
            OVERLAY_W,
            OVERLAY_H,
            None,
            None,
            hinstance,
            Some(state_ptr as *const _),
        )
        .expect("CreateWindowExW failed");

        // Ask DWM to round the corners on Windows 11+. Older builds (Win10
        // and below) silently ignore the attribute (the call returns HRESULT
        // E_INVALIDARG / ERROR_INVALID_PARAMETER and we just don't get
        // rounding) — no fallback work is needed.
        let pref: DWM_WINDOW_CORNER_PREFERENCE = DWM_WINDOW_CORNER_PREFERENCE(2); // DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );

        // Store the HWND in shared state so `show()` can use it.
        *state.hwnd.lock() = Some(hwnd.0 as isize);

        // Start the polling timer (~60 Hz so the fade-in feels smooth).
        SetTimer(hwnd, TIMER_ID, TIMER_MS, None);

        // ----- Message pump --------------------------------------------------
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

// ---------------------------------------------------------------------------
// Window procedure
// ---------------------------------------------------------------------------

/// Per-window userdata — a raw pointer to `Arc<IndicatorState>` stored in
/// GWLP_USERDATA.  We avoid unsafe global statics by threading this through the
/// Win32 userdata slot.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWLP_USERDATA,
    };

    match msg {
        WM_CREATE => {
            // lparam points to CREATESTRUCTW whose lpCreateParams is our state ptr.
            let cs = &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(0)
        }

        WM_PAINT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const IndicatorState;
            if ptr.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let state = &*ptr;

            let text_guard = state.text.lock();
            let mut text: Vec<u16> = text_guard.encode_utf16().collect();
            drop(text_guard);

            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            // Background: dark charcoal rounded-looking rectangle.
            // Win32 GDI doesn't do GPU rounded-rects cheaply so we fill a plain
            // rect; visual rounding is done via the window alpha channel.
            let bg_brush = CreateSolidBrush(COLORREF(0x00_1E_1E_1E)); // #1e1e1e
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: OVERLAY_W,
                bottom: OVERLAY_H,
            };
            FillRect(hdc, &rc, bg_brush);
            let _ = DeleteObject(bg_brush);

            // Font: 22pt bold; height negative = point size in logical units.
            let font_height: i32 = -22;
            let hfont: HFONT = CreateFontW(
                font_height,
                0,
                0,
                0,
                700, // FW_BOLD
                0,
                0,
                0,
                1,   // DEFAULT_CHARSET
                0,
                0,
                0,
                0,
                PCWSTR(wide("Segoe UI").as_ptr()),
            );
            let old_font = SelectObject(hdc, hfont);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x00_FF_FF_FF)); // white

            let _ = DrawTextW(
                hdc,
                &mut text,
                &mut rc,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );

            SelectObject(hdc, old_font);
            let _ = DeleteObject(hfont);

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }

        WM_TIMER => {
            if wparam.0 != TIMER_ID {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }

            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const IndicatorState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = &*ptr;

            let expires = *state.expires_at.lock();
            let shown_at = *state.shown_at.lock();
            match expires {
                None => {
                    // Nothing shown — hide just in case.
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        // Expired: hide and clear the deadline.
                        *state.expires_at.lock() = None;
                        *state.shown_at.lock() = None;
                        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    } else {
                        // Fade-in (over FADE_IN_MS) combined with fade-out
                        // (over the last FADE_OUT_MS).  The displayed alpha is
                        // the minimum of the two so each phase is preserved.
                        let elapsed_ms = shown_at
                            .map(|s| now.duration_since(s).as_millis())
                            .unwrap_or(FADE_IN_MS); // assume already in
                        let remaining_ms = deadline.duration_since(now).as_millis();

                        let alpha_in = if elapsed_ms >= FADE_IN_MS {
                            255u8
                        } else {
                            // Ease-in: 1 - (1-t)^2, t ∈ [0,1]
                            let t = elapsed_ms as f32 / FADE_IN_MS as f32;
                            let eased = 1.0 - (1.0 - t).powi(2);
                            (eased * 255.0).clamp(0.0, 255.0) as u8
                        };
                        let alpha_out = if remaining_ms >= FADE_OUT_MS {
                            255u8
                        } else {
                            (remaining_ms as f32 / FADE_OUT_MS as f32 * 255.0)
                                .clamp(0.0, 255.0) as u8
                        };
                        let alpha = alpha_in.min(alpha_out);

                        let _ =
                            SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
                        // Trigger repaint so the new alpha is applied.
                        let _ = InvalidateRect(hwnd, None, true);
                    }
                }
            }
            LRESULT(0)
        }

        WM_DESTROY => {
            let _ = KillTimer(hwnd, TIMER_ID);
            // Reclaim the Arc we leaked on WM_CREATE.
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut IndicatorState;
            if !ptr.is_null() {
                // Reconstruct and immediately drop — decrements the Arc refcount.
                drop(Arc::from_raw(ptr));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
