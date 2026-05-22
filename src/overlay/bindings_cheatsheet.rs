//! Key-bindings cheatsheet overlay — a large layered window that lists all
//! registered hotkeys in a two-column table.  Toggle on/off via
//! `BindingsCheatsheet::toggle`; the overlay is transparent to input and never
//! steals focus.
//!
//! The global singleton is installed once from `main.rs` via
//! `set_global_cheatsheet` and retrieved inside `execute_action` via
//! `get_global_cheatsheet`.

use std::sync::Arc;
use parking_lot::Mutex;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, FillRect, SelectObject, SetBkMode, SetTextColor, HFONT,
    PAINTSTRUCT, DT_LEFT, DT_SINGLELINE, DT_VCENTER, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GetMessageW, GetSystemMetrics, RegisterClassExW,
    SetWindowPos, ShowWindow, TranslateMessage,
    CS_HREDRAW, CS_VREDRAW, MSG, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SW_HIDE, SW_SHOWNOACTIVATE,
    WNDCLASSEXW, WM_CREATE, WM_DESTROY, WM_PAINT,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::PCWSTR;

// Overlay logical dimensions at 96 DPI baseline (scaled up for HiDPI).
const OVERLAY_W: i32 = 560;
const OVERLAY_H: i32 = 720;

// Column layout within the overlay (left margin + widths).
const COL_MARGIN: i32 = 18;
const COL_CHORD_W: i32 = 200; // left column: chord string
const COL_GAP: i32 = 12;
const ROW_H: i32 = 22; // row height (logical px at 96 DPI)
const TITLE_H: i32 = 38; // title row height
const HEADER_H: i32 = 26; // column-header row height

/// One row in the cheatsheet: (chord, action-description).
pub type BindingRow = (String, String);

/// State shared between the public API and the window procedure.
struct CheatsheetState {
    /// Whether the overlay is currently visible.
    visible: Mutex<bool>,
    /// Binding rows to render — swapped on every `toggle(true)` call.
    bindings: Mutex<Vec<BindingRow>>,
    /// Raw HWND as `isize` so the struct stays `Send`.
    hwnd: Mutex<Option<isize>>,
}

/// Public handle to the cheatsheet overlay.
///
/// Create with `BindingsCheatsheet::new()`, then call `overlay.clone().spawn()`
/// once from `main`.  Afterwards, call `overlay.toggle(rows)` to show or hide.
pub struct BindingsCheatsheet {
    state: Arc<CheatsheetState>,
}

impl BindingsCheatsheet {
    pub fn new() -> Self {
        Self {
            state: Arc::new(CheatsheetState {
                visible: Mutex::new(false),
                bindings: Mutex::new(Vec::new()),
                hwnd: Mutex::new(None),
            }),
        }
    }

    /// Toggle visibility.  When becoming visible, `bindings` replaces the
    /// current row set and the window is shown.  When hiding, the window is
    /// hidden (the rows are kept so a fast re-show avoids a re-query).
    pub fn toggle(&self, bindings: Vec<BindingRow>) {
        let mut vis = self.state.visible.lock();
        *vis = !*vis;
        let showing = *vis;
        drop(vis);

        if showing {
            *self.state.bindings.lock() = bindings;
        }

        if let Some(raw) = *self.state.hwnd.lock() {
            unsafe {
                let hwnd = HWND(raw as *mut _);
                if showing {
                    reposition_center(hwnd);
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    // Force a repaint so the new bindings list is drawn.
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
                } else {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
    }

    /// Spawn the background thread that owns the HWND and pumps its messages.
    /// Drop the returned `JoinHandle` — the thread lives until process exit.
    pub fn spawn(self: Arc<Self>) -> std::thread::JoinHandle<()> {
        let state = self.state.clone();
        std::thread::spawn(move || run_window_loop(state))
    }
}

impl Default for BindingsCheatsheet {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Global singleton (set by main.rs, read by message_loop execute_action)
// ---------------------------------------------------------------------------

static GLOBAL_CHEATSHEET: parking_lot::Mutex<Option<Arc<BindingsCheatsheet>>> =
    parking_lot::const_mutex(None);

/// Install the global cheatsheet singleton.  Call once from `main.rs` after
/// creating the overlay.
pub fn set_global_cheatsheet(cs: Arc<BindingsCheatsheet>) {
    *GLOBAL_CHEATSHEET.lock() = Some(cs);
}

/// Retrieve the global cheatsheet (if installed).  Used by `execute_action`.
pub fn get_global_cheatsheet() -> Option<Arc<BindingsCheatsheet>> {
    GLOBAL_CHEATSHEET.lock().clone()
}

// ---------------------------------------------------------------------------
// Win32 helpers
// ---------------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn primary_monitor_scale() -> f64 {
    let dpi = unsafe { GetDpiForSystem() };
    if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
}

/// Move the overlay so it is centred on the primary monitor.
fn reposition_center(hwnd: HWND) {
    let scale = primary_monitor_scale();
    let sw = (OVERLAY_W as f64 * scale) as i32;
    let sh = (OVERLAY_H as f64 * scale) as i32;
    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let x = (screen_w - sw) / 2;
    let y = (screen_h - sh) / 2;
    unsafe {
        let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);
    }
}

// ---------------------------------------------------------------------------
// Window loop (background thread)
// ---------------------------------------------------------------------------

fn run_window_loop(state: Arc<CheatsheetState>) {
    unsafe {
        let class_name = wide("WiriBindingsCheatsheet");
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

        let scale = primary_monitor_scale();
        let sw = (OVERLAY_W as f64 * scale) as i32;
        let sh = (OVERLAY_H as f64 * scale) as i32;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let x = (screen_w - sw) / 2;
        let y = (screen_h - sh) / 2;

        let ex_style = WS_EX_LAYERED
            | WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW
            | WS_EX_TOPMOST
            | WS_EX_TRANSPARENT;

        let state_ptr = Arc::into_raw(state.clone()) as *mut CheatsheetState;

        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("").as_ptr()),
            WS_POPUP,
            x, y, sw, sh,
            None, None,
            hinstance,
            Some(state_ptr as *const _),
        )
        .expect("CreateWindowExW failed for BindingsCheatsheet");

        // Set opacity to 220/255 (~86%) so the desktop shines through slightly.
        let _ = windows::Win32::UI::WindowsAndMessaging::SetLayeredWindowAttributes(
            hwnd,
            COLORREF(0),
            220,
            windows::Win32::UI::WindowsAndMessaging::LWA_ALPHA,
        );

        *state.hwnd.lock() = Some(hwnd.0 as isize);

        // Start hidden; caller decides when to show.
        let _ = ShowWindow(hwnd, SW_HIDE);

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 <= 0 { break; }
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

        WM_PAINT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CheatsheetState;
            if ptr.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let state = &*ptr;

            let bindings_guard = state.bindings.lock();
            let rows: Vec<BindingRow> = bindings_guard.clone();
            drop(bindings_guard);

            let scale = primary_monitor_scale();
            let win_w = (OVERLAY_W as f64 * scale) as i32;
            let win_h = (OVERLAY_H as f64 * scale) as i32;

            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            // ---- Background ----
            let bg_brush = CreateSolidBrush(COLORREF(0x00_1E_1E_1E));
            let mut full_rc = RECT { left: 0, top: 0, right: win_w, bottom: win_h };
            FillRect(hdc, &full_rc, bg_brush);
            let _ = DeleteObject(bg_brush);

            // ---- Helper: scale logical px ----
            let s = |px: i32| -> i32 { (px as f64 * scale) as i32 };

            SetBkMode(hdc, TRANSPARENT);

            // ---- Title ----
            let title_font: HFONT = CreateFontW(
                -(s(17)),
                0, 0, 0,
                700, // FW_BOLD
                0, 0, 0, 1, 0, 0, 0, 0,
                PCWSTR(wide("Segoe UI").as_ptr()),
            );
            let body_font: HFONT = CreateFontW(
                -(s(13)),
                0, 0, 0,
                400, // FW_NORMAL
                0, 0, 0, 1, 0, 0, 0, 0,
                PCWSTR(wide("Segoe UI").as_ptr()),
            );
            let header_font: HFONT = CreateFontW(
                -(s(12)),
                0, 0, 0,
                600, // semi-bold
                0, 0, 0, 1, 0, 0, 0, 0,
                PCWSTR(wide("Segoe UI").as_ptr()),
            );

            let old_font = SelectObject(hdc, title_font);
            SetTextColor(hdc, COLORREF(0x00_FF_FF_FF));
            let title_margin = s(COL_MARGIN);
            let title_h_px = s(TITLE_H);
            let mut title_rc = RECT {
                left: title_margin,
                top: 0,
                right: win_w - title_margin,
                bottom: title_h_px,
            };
            let mut title_text = "wiri key bindings".encode_utf16().collect::<Vec<u16>>();
            let _ = DrawTextW(hdc, &mut title_text, &mut title_rc, DT_LEFT | DT_SINGLELINE | DT_VCENTER);

            // ---- Divider line under title ----
            let div_brush = CreateSolidBrush(COLORREF(0x00_44_44_44));
            let div_rc = RECT {
                left: title_margin,
                top: title_h_px,
                right: win_w - title_margin,
                bottom: title_h_px + 1,
            };
            FillRect(hdc, &div_rc, div_brush);
            let _ = DeleteObject(div_brush);

            // ---- Column headers ----
            let header_top = title_h_px + s(4);
            let header_bot = header_top + s(HEADER_H);
            SelectObject(hdc, header_font);
            SetTextColor(hdc, COLORREF(0x00_88_88_88));

            let col1_x = title_margin;
            let col2_x = col1_x + s(COL_CHORD_W) + s(COL_GAP);

            let mut hdr1 = "Chord".encode_utf16().collect::<Vec<u16>>();
            let mut rc_hdr1 = RECT { left: col1_x, top: header_top, right: col1_x + s(COL_CHORD_W), bottom: header_bot };
            let _ = DrawTextW(hdc, &mut hdr1, &mut rc_hdr1, DT_LEFT | DT_SINGLELINE | DT_VCENTER);

            let mut hdr2 = "Action".encode_utf16().collect::<Vec<u16>>();
            let mut rc_hdr2 = RECT { left: col2_x, top: header_top, right: win_w - title_margin, bottom: header_bot };
            let _ = DrawTextW(hdc, &mut hdr2, &mut rc_hdr2, DT_LEFT | DT_SINGLELINE | DT_VCENTER);

            // ---- Divider below headers ----
            let div2_brush = CreateSolidBrush(COLORREF(0x00_33_33_33));
            let div2_rc = RECT {
                left: title_margin,
                top: header_bot,
                right: win_w - title_margin,
                bottom: header_bot + 1,
            };
            FillRect(hdc, &div2_rc, div2_brush);
            let _ = DeleteObject(div2_brush);

            // ---- Binding rows ----
            SelectObject(hdc, body_font);
            let row_h_px = s(ROW_H);
            let rows_start_y = header_bot + s(4);
            let max_rows = ((win_h - rows_start_y - s(8)) / row_h_px).max(0) as usize;

            for (i, (chord, action)) in rows.iter().take(max_rows).enumerate() {
                let row_y = rows_start_y + (i as i32) * row_h_px;

                // Alternate row background for readability
                if i % 2 == 0 {
                    let row_brush = CreateSolidBrush(COLORREF(0x00_28_28_28));
                    let row_bg = RECT {
                        left: 0,
                        top: row_y,
                        right: win_w,
                        bottom: row_y + row_h_px,
                    };
                    FillRect(hdc, &row_bg, row_brush);
                    let _ = DeleteObject(row_brush);
                }

                // Chord (left column, muted cyan)
                SetTextColor(hdc, COLORREF(0x00_88_DD_CC));
                let mut chord_text = chord.encode_utf16().collect::<Vec<u16>>();
                let mut rc_chord = RECT {
                    left: col1_x,
                    top: row_y,
                    right: col1_x + s(COL_CHORD_W),
                    bottom: row_y + row_h_px,
                };
                let _ = DrawTextW(hdc, &mut chord_text, &mut rc_chord, DT_LEFT | DT_SINGLELINE | DT_VCENTER);

                // Action (right column, white)
                SetTextColor(hdc, COLORREF(0x00_DD_DD_DD));
                let mut action_text = action.encode_utf16().collect::<Vec<u16>>();
                let mut rc_action = RECT {
                    left: col2_x,
                    top: row_y,
                    right: win_w - title_margin,
                    bottom: row_y + row_h_px,
                };
                let _ = DrawTextW(hdc, &mut action_text, &mut rc_action, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
            }

            // ---- Footer ----
            let footer_y = win_h - s(22);
            SelectObject(hdc, header_font);
            SetTextColor(hdc, COLORREF(0x00_55_55_55));
            let mut footer_text = "Press prefix+? again to close".encode_utf16().collect::<Vec<u16>>();
            let mut rc_footer = RECT {
                left: title_margin,
                top: footer_y,
                right: win_w - title_margin,
                bottom: win_h,
            };
            let _ = DrawTextW(hdc, &mut footer_text, &mut rc_footer, DT_LEFT | DT_SINGLELINE | DT_VCENTER);

            // Clean up fonts
            SelectObject(hdc, old_font);
            let _ = DeleteObject(title_font);
            let _ = DeleteObject(body_font);
            let _ = DeleteObject(header_font);

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }

        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut CheatsheetState;
            if !ptr.is_null() {
                drop(Arc::from_raw(ptr));
            }
            windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---------------------------------------------------------------------------
// Default bindings table (fallback when engine has no config binds)
// ---------------------------------------------------------------------------

/// Human-readable default binding table matching `default_hotkeys` in
/// `message_loop.rs`.  `prefix_name` is a label like `"Ctrl+Alt"` used as
/// the modifier prefix in the chord column.
pub fn default_bindings_table(prefix_name: &str) -> Vec<BindingRow> {
    let p = prefix_name;
    let s = format!("{}+Shift", p);
    vec![
        (format!("{}+Left", p),         "focus-column-left".into()),
        (format!("{}+Right", p),        "focus-column-right".into()),
        (format!("{}+Up", p),           "focus-up".into()),
        (format!("{}+Down", p),         "focus-down".into()),
        (format!("{}+Left", s),         "move-column-left".into()),
        (format!("{}+Right", s),        "move-column-right".into()),
        (format!("{}+Q", p),            "close-window".into()),
        (format!("{}+Enter", p),        "spawn (terminal)".into()),
        (format!("{}+F", p),            "toggle-fullscreen".into()),
        (format!("{}+T", p),            "toggle-floating".into()),
        (format!("{}+H", p),            "scroll-left".into()),
        (format!("{}+L", p),            "scroll-right".into()),
        (format!("{}+Q", s),            "quit".into()),
        (format!("{}+Space", p),        "overview-toggle".into()),
        ("Escape".into(),               "overview-select / exit-resize".into()),
        (format!("{}+O", p),            "overview-select".into()),
        (format!("{}+\\", p),           "column-toggle-tabbed".into()),
        (format!("{}+]", p),            "tab-next".into()),
        (format!("{}+[", p),            "tab-prev".into()),
        (format!("{}+1…9", p),          "focus-workspace-N".into()),
        (format!("{}+1…9", s),          "move-to-workspace-N".into()),
        (format!("{}+PageUp", p),       "focus-workspace-previous".into()),
        (format!("{}+PageDown", p),     "focus-workspace-next".into()),
        (format!("{}+R", p),            "enter-resize-mode".into()),
        (format!("{}+R", s),            "center-column".into()),
        (format!("{}+W", p),            "column-width-cycle".into()),
        (format!("{}+-", p),            "resize-column-left".into()),
        (format!("{}++", p),            "resize-column-right".into()),
        (format!("{}+Tab", p),          "focus-previous (alt-tab)".into()),
        (format!("{}+P", p),            "screenshot".into()),
        (format!("{}+A", p),            "toggle-always-on-top".into()),
        (format!("{}+,", p),            "consume-window-into-column".into()),
        (format!("{}+.", p),            "expel-window-from-column".into()),
        (format!("{}+E", p),            "expand-column-to-available".into()),
        (format!("{}+F", s),            "maximize-column".into()),
        (format!("{}+L", s),            "grow-column-width".into()),
        (format!("{}+H", s),            "shrink-column-width".into()),
        (format!("{}+K", s),            "grow-tile-height".into()),
        (format!("{}+J", s),            "shrink-tile-height".into()),
        (format!("{}+PageUp", s),       "move-workspace-up".into()),
        (format!("{}+PageDown", s),     "move-workspace-down".into()),
        (format!("{}+Up", s),           "move-column-to-workspace-up".into()),
        (format!("{}+Down", s),         "move-column-to-workspace-down".into()),
        (format!("{}+,", s),            "move-column-to-monitor-left".into()),
        (format!("{}+.", s),            "move-column-to-monitor-right".into()),
        (format!("{}+S", p),            "toggle-sticky".into()),
        (format!("{}+?", s),            "show-key-bindings (this overlay)".into()),
    ]
}
