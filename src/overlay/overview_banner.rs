//! Overview-mode banner overlay — a translucent strip at the top of the
//! focused monitor that reads
//! `Overview — Space or Esc to exit` while overview is active.
//!
//! Unlike `workspace_indicator.rs` (which auto-fades after ~1.5 s) the
//! banner is a persistent visible/hidden toggle: it appears the moment the
//! engine calls [`OverviewBanner::show`] and disappears on
//! [`OverviewBanner::hide`].  No timer is needed.
//!
//! Architecture mirrors `WorkspaceIndicator`:
//! 1. The public handle (`OverviewBanner`) holds an `Arc<BannerState>`.
//! 2. [`OverviewBanner::spawn`] starts a dedicated Win32 message-pump
//!    thread that owns the layered popup HWND.
//! 3. [`OverviewBanner::show`] / `hide` mutate the shared state and post a
//!    custom message (via the HWND) so the pump thread reconfigures the
//!    window without us touching the HWND from the engine thread.
//!
//! Wired from `main.rs`: the engine calls `OverviewBanner::show_at` on
//! overview enter and `OverviewBanner::hide` on overview exit (see the
//! `Action::OverviewToggle` dispatch site).

use std::sync::Arc;

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
    GetMessageW, GetSystemMetrics, MSG, PostQuitMessage,
    RegisterClassExW, SetLayeredWindowAttributes, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    LWA_ALPHA, SM_CXSCREEN, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_HIDE, SW_SHOWNOACTIVATE,
    WNDCLASSEXW,
    WM_CREATE, WM_DESTROY, WM_PAINT, WM_USER, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::PCWSTR;

use super::MonitorBounds;

/// Banner geometry — wide enough to comfortably hold the default text at
/// the typical Win11 system font size.  Height stays slim so it doesn't
/// occlude useful overview content.
const BANNER_W: i32 = 520;
const BANNER_H: i32 = 44;
/// Fixed alpha while shown — opaque enough to read but enough transparency
/// that you can still see workspace previews underneath.
const BANNER_ALPHA: u8 = 220;

/// Custom message used by `show/hide` to wake the pump thread.
const WM_BANNER_UPDATE: u32 = WM_USER + 1;

/// Shared mutable state between the public API and the pump thread.
struct BannerState {
    /// Text rendered inside the banner.  Updated by `show()` (default text)
    /// or `show_with_text()` (caller-supplied).
    text: Mutex<String>,
    /// Whether the banner should be visible right now.  Toggled by
    /// `show()` / `hide()`.
    visible: Mutex<bool>,
    /// HWND of the popup window.  Populated by the pump thread on WM_CREATE.
    hwnd: Mutex<Option<isize>>,
    /// Monitor bounds to anchor against (None = primary).
    monitor_bounds: Mutex<Option<MonitorBounds>>,
}

/// Public handle to the overview banner overlay.
///
/// Construct once in `main.rs`, call `.clone().spawn()` to start the pump
/// thread, then `show_at` / `hide` from anywhere.  Internally `Arc`-shared
/// so cloning is cheap.
pub struct OverviewBanner {
    state: Arc<BannerState>,
}

impl OverviewBanner {
    pub fn new() -> Self {
        Self {
            state: Arc::new(BannerState {
                text: Mutex::new(
                    "Overview — Space or Esc to exit".to_string(),
                ),
                visible: Mutex::new(false),
                hwnd: Mutex::new(None),
                monitor_bounds: Mutex::new(None),
            }),
        }
    }

    /// Show the banner anchored to the top of the supplied monitor.
    pub fn show_at(&self, bounds: MonitorBounds) {
        *self.state.visible.lock() = true;
        *self.state.monitor_bounds.lock() = Some(bounds);
        self.wake();
    }

    /// Show the banner on the primary monitor (fallback when caller has no
    /// monitor bounds to hand — e.g. early-startup smoke paths).
    pub fn show(&self) {
        *self.state.visible.lock() = true;
        self.wake();
    }

    /// Hide the banner — used by the engine on overview exit.
    pub fn hide(&self) {
        *self.state.visible.lock() = false;
        self.wake();
    }

    /// Replace the displayed text.  No-op while the banner is hidden.
    pub fn set_text(&self, text: &str) {
        *self.state.text.lock() = text.to_string();
        if *self.state.visible.lock() {
            self.wake();
        }
    }

    /// Spawn the dedicated Win32 message-pump thread.  Drop the returned
    /// JoinHandle — the thread lives until the process exits.
    pub fn spawn(self: Arc<Self>) -> std::thread::JoinHandle<()> {
        let state = self.state.clone();
        std::thread::spawn(move || run_window_loop(state))
    }

    /// Post the wake-up message so the pump thread re-reads `visible` and
    /// reconfigures the window without us touching the HWND.
    fn wake(&self) {
        let hwnd_raw = *self.state.hwnd.lock();
        if let Some(raw) = hwnd_raw {
            unsafe {
                let hwnd = HWND(raw as *mut _);
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    hwnd,
                    WM_BANNER_UPDATE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }
}

impl Default for OverviewBanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Position the HWND centred horizontally on either the supplied monitor
/// or the primary monitor (when bounds is None).  Top-edge offset is a
/// small gap so the banner doesn't sit flush against the screen edge.
fn reposition_to_monitor(hwnd: HWND, bounds: Option<MonitorBounds>) {
    let (x, y) = match bounds {
        Some(b) => (b.x + (b.w - BANNER_W) / 2, b.y + 16),
        None => {
            let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            ((screen_w - BANNER_W) / 2, 16)
        }
    };
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x, y, BANNER_W, BANNER_H,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

// ---------------------------------------------------------------------------
// Pump thread
// ---------------------------------------------------------------------------

/// Wide-string helper with trailing NUL.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn run_window_loop(state: Arc<BannerState>) {
    unsafe {
        let class_name = wide("WiriOverviewBanner");
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

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let x = (screen_w - BANNER_W) / 2;
        let y = 16;

        let ex_style = WS_EX_LAYERED
            | WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW
            | WS_EX_TOPMOST
            | WS_EX_TRANSPARENT;

        let state_ptr = Arc::into_raw(state.clone()) as *mut BannerState;

        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("").as_ptr()),
            WS_POPUP,
            x,
            y,
            BANNER_W,
            BANNER_H,
            None,
            None,
            hinstance,
            Some(state_ptr as *const _),
        )
        .expect("CreateWindowExW failed");

        // Round corners on Windows 11+; older builds silently ignore.
        let pref: DWM_WINDOW_CORNER_PREFERENCE = DWM_WINDOW_CORNER_PREFERENCE(2); // DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );

        *state.hwnd.lock() = Some(hwnd.0 as isize);

        // Start hidden — only `show()` should bring us on screen.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);

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
            let cs = &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(0)
        }

        WM_BANNER_UPDATE => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const BannerState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = &*ptr;
            let visible = *state.visible.lock();
            let bounds = *state.monitor_bounds.lock();
            if visible {
                reposition_to_monitor(hwnd, bounds);
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), BANNER_ALPHA, LWA_ALPHA);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = InvalidateRect(hwnd, None, true);
            } else {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }

        WM_PAINT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const BannerState;
            if ptr.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let state = &*ptr;

            let text_guard = state.text.lock();
            let mut text: Vec<u16> = text_guard.encode_utf16().collect();
            drop(text_guard);

            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            // Dark navy background (#1A2438) — distinct from the workspace
            // indicator so the two overlays don't look identical when both
            // appear briefly during a workspace switch in overview.
            let bg_brush = CreateSolidBrush(COLORREF(0x00_38_24_1A));
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: BANNER_W,
                bottom: BANNER_H,
            };
            FillRect(hdc, &rc, bg_brush);
            let _ = DeleteObject(bg_brush);

            let font_height: i32 = -16;
            let hfont: HFONT = CreateFontW(
                font_height,
                0,
                0,
                0,
                600, // FW_SEMIBOLD
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
            SetTextColor(hdc, COLORREF(0x00_FF_FF_FF));

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

        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut BannerState;
            if !ptr.is_null() {
                drop(Arc::from_raw(ptr));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---------------------------------------------------------------------------
// Process-wide singleton helper (for callers that can't thread the Arc
// through their own state — primarily `engine.rs::toggle_overview`).
// ---------------------------------------------------------------------------

use std::sync::OnceLock;

static BANNER: OnceLock<Arc<OverviewBanner>> = OnceLock::new();

/// Install a process-wide singleton `OverviewBanner`.  Called once from
/// `main.rs` after [`OverviewBanner::spawn`].  Idempotent on subsequent
/// calls (the first banner wins).
pub fn install_global(banner: Arc<OverviewBanner>) {
    let _ = BANNER.set(banner);
}

/// Show the global banner if one has been installed.  Safe no-op when
/// `install_global` was never called (e.g. in unit tests).
pub fn global_show(bounds: Option<MonitorBounds>) {
    if let Some(b) = BANNER.get() {
        match bounds {
            Some(mb) => b.show_at(mb),
            None => b.show(),
        }
    }
}

/// Hide the global banner if one has been installed.
pub fn global_hide() {
    if let Some(b) = BANNER.get() {
        b.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_banner_default_text() {
        let b = OverviewBanner::new();
        let t = b.state.text.lock();
        assert!(t.contains("Overview"));
        assert!(t.contains("exit"));
    }

    #[test]
    fn test_set_text_updates_state() {
        let b = OverviewBanner::new();
        b.set_text("custom");
        assert_eq!(*b.state.text.lock(), "custom");
    }

    #[test]
    fn test_show_hide_toggles_visible() {
        let b = OverviewBanner::new();
        assert!(!*b.state.visible.lock());
        b.show();
        assert!(*b.state.visible.lock());
        b.hide();
        assert!(!*b.state.visible.lock());
    }

    #[test]
    fn test_global_no_panic_without_install() {
        // global_show / global_hide must not panic when no banner is
        // installed (covers the early-startup path before main.rs wires
        // the singleton).
        global_show(None);
        global_hide();
    }
}
