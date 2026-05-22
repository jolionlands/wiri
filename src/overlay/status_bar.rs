//! Persistent top-of-screen status bar overlay.
//!
//! Shown permanently when `layout { status-bar true }` is set in the config.
//! Displays three zones on a single 32-px-high layered window:
//!   - Left:   `[N] WorkspaceName`  (workspace indicator)
//!   - Centre: focused window title  (ellipsis-truncated)
//!   - Right:  `HH:MM`              (local time, updates every second)
//!
//! The window is click-through (`WS_EX_TRANSPARENT`) and never steals focus
//! (`WS_EX_NOACTIVATE`).  A 1-second `WM_TIMER` refreshes the time and
//! triggers a repaint.  Callers push workspace / title updates at their own
//! cadence via [`StatusBar::update`].

use std::sync::Arc;

use parking_lot::Mutex;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, HFONT, PAINTSTRUCT, DT_END_ELLIPSIS,
    DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics,
    KillTimer, MSG, PostQuitMessage, RegisterClassExW, SetLayeredWindowAttributes, SetTimer,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, LWA_ALPHA, SM_CXSCREEN,
    SW_SHOWNOACTIVATE, WNDCLASSEXW, WM_CREATE, WM_DESTROY, WM_PAINT, WM_TIMER, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::PCWSTR;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Logical (96-DPI) height of the bar in pixels.
const BAR_H_LOGICAL: i32 = 32;

/// Win32 timer ID for the 1-second clock refresh.
const CLOCK_TIMER_ID: usize = 42;

/// Horizontal padding (logical px) on the left/right edges.
const PAD_H: i32 = 12;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Content to display in the status bar.  Updated from outside the Win32
/// thread via [`StatusBar::update`]; read inside `WM_PAINT`.
pub struct StatusBarContent {
    /// Left zone — e.g. `"[1] Work"`.
    pub workspace_label: String,
    /// Centre zone — focused window title (may be empty).
    pub focused_title: String,
    /// Right zone — e.g. `"14:07"`.
    pub time_str: String,
}

impl Default for StatusBarContent {
    fn default() -> Self {
        Self {
            workspace_label: String::new(),
            focused_title: String::new(),
            time_str: String::new(),
        }
    }
}

struct BarState {
    content: Mutex<StatusBarContent>,
    hwnd: Mutex<Option<isize>>,
}

// ---------------------------------------------------------------------------
// Public handle
// ---------------------------------------------------------------------------

/// Handle to the persistent status bar overlay.
///
/// # Usage
/// ```
/// let bar = Arc::new(StatusBar::new());
/// let _ = bar.clone().spawn(); // start Win32 thread; drops JoinHandle intentionally
/// bar.update(StatusBarContent { ... });
/// ```
pub struct StatusBar {
    state: Arc<BarState>,
}

impl StatusBar {
    /// Create a new (not-yet-shown) status bar.
    pub fn new() -> Self {
        Self {
            state: Arc::new(BarState {
                content: Mutex::new(StatusBarContent::default()),
                hwnd: Mutex::new(None),
            }),
        }
    }

    /// Push new content to the bar.  The next paint cycle reflects the update.
    /// Safe to call from any thread (Tokio task, main loop, etc.).
    pub fn update(&self, content: StatusBarContent) {
        *self.state.content.lock() = content;
        if let Some(raw) = *self.state.hwnd.lock() {
            unsafe {
                let hwnd = HWND(raw as *mut _);
                let _ = InvalidateRect(hwnd, None, true);
            }
        }
    }

    /// Spawn the background Win32 message-pump thread that owns the HWND.
    /// Drop the returned `JoinHandle` — the thread lives until the process exits.
    pub fn spawn(self: Arc<Self>) -> std::thread::JoinHandle<()> {
        let state = self.state.clone();
        std::thread::spawn(move || run_window_loop(state))
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DPI helper
// ---------------------------------------------------------------------------

fn primary_monitor_scale() -> f64 {
    let dpi = unsafe { GetDpiForSystem() };
    if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
}

// ---------------------------------------------------------------------------
// Wide-string helper
// ---------------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// Win32 window loop (dedicated thread)
// ---------------------------------------------------------------------------

fn run_window_loop(state: Arc<BarState>) {
    unsafe {
        // ----- Register window class ----------------------------------------
        let class_name = wide("WiriStatusBar");
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
        // Ignore failure — class may already be registered on a second call.
        let _ = RegisterClassExW(&wc);

        // ----- DPI-scaled dimensions ----------------------------------------
        let scale = primary_monitor_scale();
        let bar_h = (BAR_H_LOGICAL as f64 * scale) as i32;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);

        // ----- Extended style flags -----------------------------------------
        // WS_EX_TRANSPARENT  → click-through (mouse events fall through)
        // WS_EX_LAYERED      → allows alpha/opacity via SetLayeredWindowAttributes
        // WS_EX_NOACTIVATE   → never steals keyboard focus
        // WS_EX_TOOLWINDOW   → excluded from Alt+Tab list
        // WS_EX_TOPMOST      → always on top of normal windows
        let ex_style = WS_EX_LAYERED
            | WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW
            | WS_EX_TOPMOST
            | WS_EX_TRANSPARENT;

        // Pass state as lpCreateParams so WM_CREATE can store it in GWLP_USERDATA.
        let state_ptr = Arc::into_raw(state.clone()) as *mut BarState;

        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("").as_ptr()),
            WS_POPUP,
            0,           // x: left edge of primary monitor
            0,           // y: top edge of primary monitor
            screen_w,    // width: full screen width
            bar_h,       // height: 32 logical px × scale
            None,
            None,
            hinstance,
            Some(state_ptr as *const _),
        )
        .expect("CreateWindowExW failed for WiriStatusBar");

        // 90 % opacity — dark but still clearly reads text.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 230, LWA_ALPHA);

        // Store HWND so update() can post InvalidateRect.
        *state.hwnd.lock() = Some(hwnd.0 as isize);

        // Show immediately (bar is persistent, not transient like workspace_indicator).
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        // 1-second timer to refresh the clock.
        SetTimer(hwnd, CLOCK_TIMER_ID, 1000, None);

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
            // lparam → CREATESTRUCTW; lpCreateParams is our Arc<BarState> raw ptr.
            let cs = &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            LRESULT(0)
        }

        WM_TIMER => {
            if wparam.0 == CLOCK_TIMER_ID {
                // Time has advanced — repaint so the clock zone updates.
                let _ = InvalidateRect(hwnd, None, true);
            }
            LRESULT(0)
        }

        WM_PAINT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const BarState;
            if ptr.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let state = &*ptr;

            // Read current content + compute the time right now so the clock
            // is always fresh regardless of when update() was last called.
            let (workspace_label, focused_title) = {
                let c = state.content.lock();
                (c.workspace_label.clone(), c.focused_title.clone())
            };

            // Local time via GetLocalTime (windows-rs 0.58: no args, returns SYSTEMTIME).
            let time_str = {
                let st = windows::Win32::System::SystemInformation::GetLocalTime();
                format!("{:02}:{:02}", st.wHour, st.wMinute)
            };

            // ----- GDI paint pass ------------------------------------------
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            let scale = primary_monitor_scale();
            let bar_h = (BAR_H_LOGICAL as f64 * scale) as i32;
            let bar_w = GetSystemMetrics(SM_CXSCREEN);

            // Background: #1e1e1e (charcoal).  The 90% alpha set via
            // SetLayeredWindowAttributes provides the semi-transparency effect.
            let bg = CreateSolidBrush(COLORREF(0x00_1E_1E_1E));
            let full_rc = RECT { left: 0, top: 0, right: bar_w, bottom: bar_h };
            FillRect(hdc, &full_rc, bg);
            let _ = DeleteObject(bg);

            // Font: Segoe UI 11 pt, regular weight.
            // Negative height = point size in GDI logical units at 96 DPI.
            let font_height = (-11f64 * scale) as i32;
            let hfont: HFONT = CreateFontW(
                font_height,
                0, 0, 0,
                400, // FW_NORMAL
                0, 0, 0,
                1,   // DEFAULT_CHARSET
                0, 0, 0, 0,
                PCWSTR(wide("Segoe UI").as_ptr()),
            );
            let old_font = SelectObject(hdc, hfont);
            SetBkMode(hdc, TRANSPARENT);

            let pad = (PAD_H as f64 * scale) as i32;

            // ---- Left: workspace label (white) ------------------------------
            SetTextColor(hdc, COLORREF(0x00_FF_FF_FF));
            let mut left_rc = RECT {
                left: pad,
                top: 0,
                right: bar_w / 3,
                bottom: bar_h,
            };
            let mut ws_text: Vec<u16> = workspace_label.encode_utf16().collect();
            let _ = DrawTextW(
                hdc,
                &mut ws_text,
                &mut left_rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );

            // ---- Centre: focused window title (slightly dimmed white) --------
            SetTextColor(hdc, COLORREF(0x00_CC_CC_CC)); // #cccccc
            let mut centre_rc = RECT {
                left: bar_w / 3,
                top: 0,
                right: (bar_w * 2) / 3,
                bottom: bar_h,
            };
            let mut title_text: Vec<u16> = focused_title.encode_utf16().collect();
            // DT_END_ELLIPSIS truncates with "…" when the title overflows.
            let _ = DrawTextW(
                hdc,
                &mut title_text,
                &mut centre_rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );

            // ---- Right: time (white) ----------------------------------------
            SetTextColor(hdc, COLORREF(0x00_FF_FF_FF));
            let mut right_rc = RECT {
                left: (bar_w * 2) / 3,
                top: 0,
                right: bar_w - pad,
                bottom: bar_h,
            };
            let mut time_text: Vec<u16> = time_str.encode_utf16().collect();
            let _ = DrawTextW(
                hdc,
                &mut time_text,
                &mut right_rc,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
            );

            SelectObject(hdc, old_font);
            let _ = DeleteObject(hfont);
            let _ = EndPaint(hwnd, &ps);

            LRESULT(0)
        }

        WM_DESTROY => {
            let _ = KillTimer(hwnd, CLOCK_TIMER_ID);
            // Reclaim the Arc we leaked on WM_CREATE.
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut BarState;
            if !ptr.is_null() {
                drop(Arc::from_raw(ptr));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
