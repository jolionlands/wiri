//! Important Hotkeys: a lazy, monitor-local, non-activating help surface.
//! No HWND, thread, timer or font survives dismissal. The existing input hooks
//! dismiss it; the key that dismisses it (including its release) is consumed.
use crate::overlay::markup_text::{self, FontCache, GdiMeasurer};
use crate::overlay::pango_markup::{self, Parsed, Style};
use crate::utils::Rect;
use parking_lot::Mutex;
use std::sync::{
    atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering},
    Arc,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetMonitorInfoW,
    MonitorFromWindow, SelectObject, SetBkMode, SetTextColor, DT_END_ELLIPSIS, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, HDC, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

pub type BindingRow = (String, String);

#[derive(Debug, Clone, Copy)]
struct SheetLayout {
    bounds: Rect,
    scale: f64,
    columns: usize,
    rows: usize,
    row_height: i32,
    header: i32,
    footer: i32,
}
impl SheetLayout {
    fn fit(work: Rect, scale: f64, count: usize) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let s = |n: f64| (n * scale).round().max(1.0) as u32;
        let margin = s(12.0).min(work.size.w / 8).min(work.size.h / 8);
        let max_w = work.size.w.saturating_sub(2 * margin).max(1);
        let max_h = work.size.h.saturating_sub(2 * margin).max(1);
        let header = s(64.0).min(max_h / 3) as i32;
        let footer = s(28.0).min(max_h / 4) as i32;
        let row_height = s(23.0).min(max_h.saturating_sub((header + footer) as u32).max(1)) as i32;
        let capacity = ((max_h as i32 - header - footer) / row_height).max(1) as usize;
        let columns = if count > capacity && max_w >= s(850.0) {
            2
        } else {
            1
        };
        let rows = count.div_ceil(columns).clamp(1, capacity);
        let width = s(if columns == 1 { 720.0 } else { 1200.0 }).min(max_w);
        let height = (header + footer + rows as i32 * row_height).max(1) as u32;
        let height = height.min(max_h);
        Self {
            bounds: Rect::new(
                work.loc.x + (work.size.w - width) as i32 / 2,
                work.loc.y + (work.size.h - height) as i32 / 2,
                width,
                height,
            ),
            scale,
            columns,
            rows,
            row_height,
            header,
            footer,
        }
    }
    fn page_size(self) -> usize {
        self.columns * self.rows
    }
}
struct Session {
    closed: AtomicBool,
    hwnd: AtomicIsize,
    page: AtomicUsize,
    rows: Vec<BindingRow>,
    /// The description of each row as Pango runs: niri paints a bind's
    /// `hotkey-overlay-title` with `set_markup`, so a title is markup and an invalid one
    /// is the whole original string, plain (see `pango_markup`).
    titles: Vec<Parsed>,
    /// One GDI font per distinct run style, created on first paint and deleted with the
    /// session: nothing survives dismissal.
    fonts: Mutex<FontCache>,
    layout: SheetLayout,
}
#[derive(Default)]
pub struct BindingsCheatsheet {
    session: Mutex<Option<Arc<Session>>>,
}
impl BindingsCheatsheet {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_visible(&self) -> bool {
        self.session
            .lock()
            .as_ref()
            .is_some_and(|s| !s.closed.load(Ordering::Acquire))
    }
    pub fn dismiss(&self) {
        if let Some(s) = self.session.lock().take() {
            s.closed.store(true, Ordering::Release);
            let hwnd = s.hwnd.load(Ordering::Acquire);
            if hwnd != 0 {
                unsafe {
                    let _ = PostMessageW(HWND(hwnd as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
    pub fn toggle(&self, rows: Vec<BindingRow>) {
        if self.is_visible() {
            self.dismiss();
            return;
        }
        let (work, scale) = focused_work_area();
        // Parse each row's description once, now, with the same base style the painter
        // uses; an invalid title falls back to the whole original string here, exactly
        // once, with a warning already logged at config load.
        let body = Style::overlay_body();
        let titles: Vec<Parsed> = rows
            .iter()
            .map(|(_, title)| pango_markup::title(title, &body))
            .collect();
        let session = Arc::new(Session {
            closed: AtomicBool::new(false),
            hwnd: AtomicIsize::new(0),
            page: AtomicUsize::new(0),
            layout: SheetLayout::fit(work, scale, rows.len()),
            rows,
            titles,
            fonts: Mutex::new(FontCache::new()),
        });
        *self.session.lock() = Some(session.clone());
        let failed = session.clone();
        if std::thread::Builder::new()
            .name("wiri-hotkeys".into())
            .spawn(move || unsafe { run(session) })
            .is_err()
        {
            failed.closed.store(true, Ordering::Release);
        }
    }
    pub fn scroll(&self, backwards: bool) {
        let guard = self.session.lock();
        let Some(s) = guard.as_ref().filter(|s| !s.closed.load(Ordering::Acquire)) else {
            return;
        };
        let pages = s.rows.len().div_ceil(s.layout.page_size()).max(1);
        let page = s.page.load(Ordering::Relaxed);
        let next = if backwards {
            page.saturating_sub(1)
        } else {
            (page + 1).min(pages - 1)
        };
        s.page.store(next, Ordering::Relaxed);
        let hwnd = s.hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(
                    HWND(hwnd as *mut _),
                    None,
                    false,
                );
            }
        }
    }
}
impl Drop for BindingsCheatsheet {
    fn drop(&mut self) {
        self.dismiss();
    }
}
static GLOBAL_CHEATSHEET: Mutex<Option<Arc<BindingsCheatsheet>>> = parking_lot::const_mutex(None);
pub fn set_global_cheatsheet(cs: Arc<BindingsCheatsheet>) {
    *GLOBAL_CHEATSHEET.lock() = Some(cs);
}
pub fn get_global_cheatsheet() -> Option<Arc<BindingsCheatsheet>> {
    GLOBAL_CHEATSHEET.lock().clone()
}
static CONSUMED: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
/// Runs before double-Alt, recent-windows and normal binding dispatch.
pub fn dismiss_on_key(vk: u32, down: bool) -> bool {
    let key = &CONSUMED[(vk & 255) as usize];
    if key.load(Ordering::Relaxed) {
        if !down {
            key.store(false, Ordering::Relaxed);
        }
        return true;
    }
    if down {
        if let Some(cs) = get_global_cheatsheet().filter(|cs| cs.is_visible()) {
            key.store(true, Ordering::Relaxed);
            cs.dismiss();
            return true;
        }
    }
    false
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn focused_work_area() -> (Rect, f64) {
    unsafe {
        let fg = GetForegroundWindow();
        let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let r = info.rcWork;
            let dpi = GetDpiForWindow(fg).max(96);
            (
                Rect::new(
                    r.left,
                    r.top,
                    (r.right - r.left).max(1) as u32,
                    (r.bottom - r.top).max(1) as u32,
                ),
                dpi as f64 / 96.0,
            )
        } else {
            (Rect::new(0, 0, 1366, 728), 1.0)
        }
    }
}
unsafe fn run(session: Arc<Session>) {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    let Ok(instance) = GetModuleHandleW(None) else {
        session.closed.store(true, Ordering::Release);
        return;
    };
    let class = wide("WiriBindingsCheatsheet");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc); // Class already registered on later sessions.
    let b = session.layout.bounds;
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        PCWSTR(class.as_ptr()),
        PCWSTR(wide("Important Hotkeys").as_ptr()),
        WS_POPUP,
        b.loc.x,
        b.loc.y,
        b.size.w as i32,
        b.size.h as i32,
        None,
        None,
        instance,
        Some(Arc::as_ptr(&session) as *const _),
    );
    let Ok(hwnd) = hwnd else {
        session.closed.store(true, Ordering::Release);
        return;
    };
    session.hwnd.store(hwnd.0 as isize, Ordering::Release);
    if session.closed.load(Ordering::Acquire) {
        let _ = DestroyWindow(hwnd);
        return;
    }
    crate::window::block_out::apply_to(
        crate::window::block_out::Overlay::HotkeyList,
        hwnd.0 as isize,
    );
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    session.closed.store(true, Ordering::Release);
    session.hwnd.store(0, Ordering::Release);
}
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(l.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Session;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let s = &*ptr;
    match msg {
        WM_PAINT => {
            paint(hwnd, s);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_CLOSE => {
            s.closed.store(true, Ordering::Release);
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            s.hwnd.store(0, Ordering::Release);
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, w, l)
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}
unsafe fn text(dc: HDC, value: &str, mut rect: RECT, color: u32) {
    SetTextColor(dc, COLORREF(color));
    let mut value: Vec<u16> = value.encode_utf16().collect();
    DrawTextW(
        dc,
        &mut value,
        &mut rect,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    );
}
unsafe fn paint(hwnd: HWND, s: &Session) {
    let g = s.layout;
    let width = g.bounds.size.w as i32;
    let height = g.bounds.size.h as i32;
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    let brush = CreateSolidBrush(COLORREF(0x00201c1c));
    FillRect(
        dc,
        &RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
        brush,
    );
    let _ = DeleteObject(brush);
    SetBkMode(dc, TRANSPARENT);
    // Every font this paint needs, created on first use and kept for the showing (never
    // beyond it): the sheet's title, the body text, and one per distinct run style.
    let body = Style::overlay_body();
    let mut fonts = s.fonts.lock();
    let margin = (16.0 * g.scale).round() as i32;
    let original = fonts.select(dc, &markup_text::heading_style(), g.scale);
    text(
        dc,
        "Important Hotkeys",
        RECT {
            left: margin,
            top: 0,
            right: width - margin,
            bottom: g.header,
        },
        0x00ffffff,
    );
    let _ = fonts.select(dc, &body, g.scale);
    let page_size = g.page_size();
    let first = s.page.load(Ordering::Relaxed) * page_size;
    let last = s.rows.len().min(first.saturating_add(page_size));
    let gap = (8.0 * g.scale).round().max(1.0) as i32;
    // The chord column is as wide as its widest painted chord; the measurement uses the
    // same cached font the chord is painted with, and `chord_split` keeps the description
    // column usable.
    let mut widest = 0;
    for index in first..last {
        widest = widest.max(fonts.advance(dc, &s.rows[index].0, &body, g.scale));
    }
    for position in 0..last.saturating_sub(first) {
        let index = first + position;
        let (chord, title) = &s.rows[index];
        let column = position / g.rows;
        let row = position % g.rows;
        let col_width = width / g.columns as i32;
        let x = column as i32 * col_width + margin;
        let y = g.header + row as i32 * g.row_height;
        let split = markup_text::chord_split(x, col_width, widest, gap);
        text(
            dc,
            chord,
            RECT {
                left: x,
                top: y,
                right: split - gap,
                bottom: y + g.row_height,
            },
            0x00ccddee,
        );
        let title_rect = RECT {
            left: split,
            top: y,
            right: (column as i32 + 1) * col_width - margin,
            bottom: y + g.row_height,
        };
        let parsed = &s.titles[index];
        if parsed.markup {
            // A configured title is Pango markup: fit its styled runs to the column with
            // the same fonts the painter then uses, and paint them run by run.
            let fitted = {
                let mut measurer = GdiMeasurer::new(dc, g.scale, &mut fonts);
                pango_markup::fit_runs(
                    &parsed.runs,
                    title_rect.right - title_rect.left,
                    &mut measurer,
                )
            };
            markup_text::draw(dc, &fitted, title_rect, 0x00eeeeee, g.scale, &mut fonts);
        } else {
            // The fast path every generated description (and every plain title) keeps:
            // one DrawTextW with ellipsis, exactly as before markup support.
            text(dc, title, title_rect, 0x00eeeeee);
        }
    }
    let pages = s.rows.len().div_ceil(page_size).max(1);
    let footer = if pages > 1 {
        format!(
            "Any key or click to close  |  Wheel: page {}/{}",
            first / page_size + 1,
            pages
        )
    } else {
        "Any key or click to close".into()
    };
    text(
        dc,
        &footer,
        RECT {
            left: margin,
            top: height - g.footer,
            right: width - margin,
            bottom: height,
        },
        0x00aaaaaa,
    );
    let _ = SelectObject(dc, original);
    drop(fonts);
    let _ = EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_dpi_and_negative_monitor_fits_with_no_dropped_page_rows() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            for count in [0, 15, 24, 50, 250] {
                let work = Rect::new(-1366, 32, 1366, 696);
                let g = SheetLayout::fit(work, dpi, count);
                assert!(g.bounds.loc.x >= work.loc.x && g.bounds.right() <= work.right());
                assert!(g.bounds.loc.y >= work.loc.y && g.bounds.bottom() <= work.bottom());
                assert!(g.page_size() > 0);
                assert!(
                    g.header + g.footer + g.rows as i32 * g.row_height <= g.bounds.size.h as i32
                );
                assert!(g.page_size() * count.div_ceil(g.page_size()) >= count);
            }
        }
    }
    #[test]
    fn unused_overlay_allocates_no_host_and_dismiss_is_idempotent() {
        let cs = BindingsCheatsheet::new();
        assert!(!cs.is_visible());
        cs.dismiss();
        cs.dismiss();
        assert!(cs.session.lock().is_none());
    }
}
