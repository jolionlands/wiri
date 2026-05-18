//! DWM-thumbnail-based overview renderer (niri-parity).
//!
//! Historic overview: enter overview → resize every tracked HWND down to
//! a tiny rect via `SetWindowPos` so every workspace's columns fit on
//! one screen.  This works but it's brutal — applications run real
//! layout passes at 100 × 80 px, some refuse to shrink at all, and
//! exiting overview triggers another full re-layout that some apps
//! contest.
//!
//! niri's overview leaves the underlying surfaces untouched and instead
//! draws scaled live previews into a separate render pass.  Windows
//! gives us the same superpower for free via
//! [`DwmRegisterThumbnail`](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmregisterthumbnail):
//! the compositor produces an accurately-scaled live thumbnail of any
//! HWND inside a destination window, without the source application
//! needing to know anything about it.
//!
//! This module ships a [`ThumbnailOverviewSink`] trait that the engine
//! can call into during overview mode, plus a [`DwmThumbnailOverview`]
//! Win32 implementation that owns:
//!
//! * one fullscreen layered topmost transparent host HWND, created on
//!   [`enter`](ThumbnailOverviewSink::enter) and destroyed on
//!   [`exit`](ThumbnailOverviewSink::exit);
//! * a `HashMap<WindowId, HTHUMBNAIL>` of every registered thumbnail.
//!
//! For unit-testing the engine wiring without standing up a real Win32
//! window, the trait is also implemented by [`MockThumbnailOverview`]
//! which simply counts the registration / update / unregistration
//! calls.

use std::collections::HashMap;
use std::sync::Arc;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;

use parking_lot::Mutex;

use crate::utils::{Rect, WindowId};

// ---------------------------------------------------------------------------
// Trait surface
// ---------------------------------------------------------------------------

/// Abstract sink that the layout engine talks to while overview mode is
/// active.
///
/// All methods are infallible from the engine's perspective — a sink
/// that fails internally should log the failure and continue.  The
/// engine treats `None` (no sink installed) as "fall back to the
/// historic SetWindowPos-based overview".
pub trait ThumbnailOverviewSink: Send + Sync {
    /// Called from `TilingEngine::enter_overview` before any
    /// `update_thumbnail` calls.  Implementations create the host
    /// window(s) here and clear any state from a previous session.
    fn enter(&self);

    /// Register a window for inclusion in the overview.  Called once
    /// per visible tile during `enter_overview`.  Implementations should
    /// allocate a thumbnail handle for the source HWND but defer
    /// positioning until [`update_thumbnail`] supplies a destination
    /// rect.  Passing the same `window_id` twice is allowed (and
    /// idempotent).
    fn register(&self, window_id: WindowId, source_hwnd: isize);

    /// Position the previously-registered thumbnail for `window_id` at
    /// `dst_rect` (in screen pixels).  Called every layout pass while
    /// overview is active.  Implementations should treat a missing
    /// registration as a no-op (the engine may compute a position for a
    /// tile before the sink learns about it during very quick
    /// switches).
    fn update_thumbnail(&self, window_id: WindowId, dst_rect: Rect);

    /// Tear down every thumbnail registered since the last
    /// [`enter`](Self::enter) call, and destroy the host window(s).
    fn exit(&self);

    /// Returns the number of currently-registered thumbnails.  Used by
    /// the engine for diagnostics + the unit-test mock to assert
    /// register/unregister counts.
    fn registered_count(&self) -> usize;
}

// ---------------------------------------------------------------------------
// Mock implementation (test-only by convention)
// ---------------------------------------------------------------------------

/// Lightweight in-memory sink used in tests.  Tracks every call so
/// tests can assert that the engine wires the trait correctly without
/// touching Win32 / DWM.
///
/// `Arc<MockThumbnailOverview>` is cheap to clone and `Send + Sync`, so
/// tests can keep one reference for inspection while handing another to
/// the engine.
#[derive(Default)]
pub struct MockThumbnailOverview {
    inner: Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    /// Total `register` calls received since construction.
    pub register_calls: u32,
    /// Total `unregister` calls (one per registered window) — driven by
    /// the `exit` call clearing the registration map.
    pub unregister_calls: u32,
    /// Total `enter` calls received.
    pub enter_calls: u32,
    /// Total `exit` calls received.
    pub exit_calls: u32,
    /// Last destination rect supplied per window.
    pub last_dst_rect: HashMap<WindowId, Rect>,
    /// Currently-active registrations (cleared on `exit`).
    pub registered: HashMap<WindowId, isize>,
}

impl MockThumbnailOverview {
    /// Construct a fresh mock with all counters at zero.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Snapshot the number of `register` calls received so far.
    pub fn register_calls(&self) -> u32 {
        self.inner.lock().register_calls
    }

    /// Snapshot the number of `unregister` calls (one per cleared
    /// registration during `exit`).
    pub fn unregister_calls(&self) -> u32 {
        self.inner.lock().unregister_calls
    }

    /// Snapshot the number of `enter` calls received.
    pub fn enter_calls(&self) -> u32 {
        self.inner.lock().enter_calls
    }

    /// Snapshot the number of `exit` calls received.
    pub fn exit_calls(&self) -> u32 {
        self.inner.lock().exit_calls
    }

    /// Snapshot the last `dst_rect` for a window, if `update_thumbnail`
    /// has been called for it.
    pub fn last_dst_rect(&self, window_id: WindowId) -> Option<Rect> {
        self.inner.lock().last_dst_rect.get(&window_id).copied()
    }

    /// Inherent accessor mirroring [`ThumbnailOverviewSink::registered_count`]
    /// so test callers don't need to import the trait into scope.
    pub fn registered_count(&self) -> usize {
        self.inner.lock().registered.len()
    }
}

impl ThumbnailOverviewSink for MockThumbnailOverview {
    fn enter(&self) {
        let mut s = self.inner.lock();
        s.enter_calls += 1;
    }

    fn register(&self, window_id: WindowId, source_hwnd: isize) {
        let mut s = self.inner.lock();
        // Allow idempotent re-registration; only count the first one.
        if s.registered.insert(window_id, source_hwnd).is_none() {
            s.register_calls += 1;
        }
    }

    fn update_thumbnail(&self, window_id: WindowId, dst_rect: Rect) {
        let mut s = self.inner.lock();
        // Only count updates for windows the engine has registered —
        // matches the production sink's behaviour where DWM would
        // otherwise log a "no such thumbnail" error.
        if s.registered.contains_key(&window_id) {
            s.last_dst_rect.insert(window_id, dst_rect);
        }
    }

    fn exit(&self) {
        let mut s = self.inner.lock();
        s.exit_calls += 1;
        let n = s.registered.len() as u32;
        s.unregister_calls += n;
        s.registered.clear();
        s.last_dst_rect.clear();
    }

    fn registered_count(&self) -> usize {
        self.inner.lock().registered.len()
    }
}

// ---------------------------------------------------------------------------
// Real Win32 implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
        DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
        DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, RegisterClassExW,
        SetLayeredWindowAttributes, ShowWindow, CS_HREDRAW, CS_VREDRAW, LWA_ALPHA,
        SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, WNDCLASSEXW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT, WS_POPUP,
    };

    /// Win32-backed thumbnail overview.
    pub struct DwmThumbnailOverview {
        state: Arc<Mutex<DwmState>>,
    }

    #[derive(Default)]
    struct DwmState {
        /// Host HWND raw value; `None` while no overview session is
        /// active.  Created on `enter`, destroyed on `exit`.
        host: Option<isize>,
        /// Map from layout-engine window id → DWM thumbnail handle.
        /// Each `HTHUMBNAIL` is an `isize` in `windows 0.58`.
        thumbnails: HashMap<WindowId, isize>,
    }

    impl DwmThumbnailOverview {
        /// Construct a new sink.  No Win32 resources are touched until
        /// the first `enter` call.
        pub fn new() -> Arc<Self> {
            Arc::new(Self { state: Arc::new(Mutex::new(DwmState::default())) })
        }

        fn ensure_host_class() {
            static REGISTERED: OnceLock<()> = OnceLock::new();
            REGISTERED.get_or_init(|| {
                unsafe {
                    let class_name = wide("WiriThumbnailOverviewHost");
                    let hinstance =
                        windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                            .expect("GetModuleHandleW failed");
                    let wc = WNDCLASSEXW {
                        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                        style: CS_HREDRAW | CS_VREDRAW,
                        lpfnWndProc: Some(host_wnd_proc),
                        hInstance: hinstance.into(),
                        lpszClassName: PCWSTR(class_name.as_ptr()),
                        ..Default::default()
                    };
                    let _ = RegisterClassExW(&wc);
                }
            });
        }
    }

    impl ThumbnailOverviewSink for DwmThumbnailOverview {
        fn enter(&self) {
            Self::ensure_host_class();
            let mut s = self.state.lock();
            if s.host.is_some() {
                // Already entered — clear residual thumbnails so a
                // re-entry starts fresh.
                for (_wid, hthumb) in s.thumbnails.drain() {
                    unsafe { let _ = DwmUnregisterThumbnail(hthumb); }
                }
                return;
            }
            unsafe {
                let class_name = wide("WiriThumbnailOverviewHost");
                let hinstance =
                    windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                        .expect("GetModuleHandleW failed");
                let screen_w = GetSystemMetrics(SM_CXSCREEN);
                let screen_h = GetSystemMetrics(SM_CYSCREEN);
                // Topmost layered transparent host.  WS_EX_TRANSPARENT
                // lets mouse events pass through so the engine's own
                // overview hit-testing keeps working.
                let ex_style = WS_EX_LAYERED
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT;
                let hwnd = match CreateWindowExW(
                    ex_style,
                    PCWSTR(class_name.as_ptr()),
                    PCWSTR(wide("").as_ptr()),
                    WS_POPUP,
                    0, 0, screen_w, screen_h,
                    None, None, hinstance, None,
                ) {
                    Ok(h) => h,
                    Err(_) => {
                        // Couldn't create the host; bail without
                        // leaving state half-set.
                        return;
                    }
                };
                // Fully transparent background — only the thumbnails
                // composited by DWM are visible.
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                s.host = Some(hwnd.0 as isize);
            }
        }

        fn register(&self, window_id: WindowId, source_hwnd: isize) {
            let mut s = self.state.lock();
            let host_raw = match s.host {
                Some(h) => h,
                None => return,
            };
            if s.thumbnails.contains_key(&window_id) {
                return; // idempotent
            }
            unsafe {
                let host = HWND(host_raw as *mut _);
                let src = HWND(source_hwnd as *mut _);
                match DwmRegisterThumbnail(host, src) {
                    Ok(hthumb) => {
                        s.thumbnails.insert(window_id, hthumb);
                    }
                    Err(_) => {
                        // Source window may have died between the
                        // engine snapshot and our call; skip silently.
                    }
                }
            }
        }

        fn update_thumbnail(&self, window_id: WindowId, dst_rect: Rect) {
            let s = self.state.lock();
            let hthumb = match s.thumbnails.get(&window_id) {
                Some(h) => *h,
                None => return,
            };
            let rc = RECT {
                left: dst_rect.loc.x,
                top: dst_rect.loc.y,
                right: dst_rect.loc.x + dst_rect.size.w as i32,
                bottom: dst_rect.loc.y + dst_rect.size.h as i32,
            };
            let props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION
                    | DWM_TNP_OPACITY
                    | DWM_TNP_VISIBLE
                    | DWM_TNP_SOURCECLIENTAREAONLY,
                rcDestination: rc,
                rcSource: RECT::default(),
                opacity: 255,
                fVisible: windows::Win32::Foundation::BOOL(1),
                fSourceClientAreaOnly: windows::Win32::Foundation::BOOL(1),
            };
            unsafe {
                let _ = DwmUpdateThumbnailProperties(hthumb, &props);
            }
        }

        fn exit(&self) {
            let mut s = self.state.lock();
            for (_wid, hthumb) in s.thumbnails.drain() {
                unsafe { let _ = DwmUnregisterThumbnail(hthumb); }
            }
            if let Some(raw) = s.host.take() {
                unsafe {
                    let hwnd = HWND(raw as *mut _);
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    let _ = DestroyWindow(hwnd);
                }
            }
        }

        fn registered_count(&self) -> usize {
            self.state.lock().thumbnails.len()
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn host_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // The host window is purely a DWM target; there's nothing for
        // us to paint or handle directly.  Default-process every
        // message.
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

#[cfg(target_os = "windows")]
pub use win::DwmThumbnailOverview;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::Rect;

    #[test]
    fn mock_register_increments_count() {
        let sink = MockThumbnailOverview::new();
        sink.enter();
        let wid = WindowId::new(1);
        sink.register(wid, 100);
        assert_eq!(sink.register_calls(), 1);
        assert_eq!(sink.registered_count(), 1);
        // Idempotent.
        sink.register(wid, 100);
        assert_eq!(sink.register_calls(), 1);
    }

    #[test]
    fn mock_update_thumbnail_records_last_rect() {
        let sink = MockThumbnailOverview::new();
        sink.enter();
        let wid = WindowId::new(7);
        sink.register(wid, 999);
        let r = Rect::new(100, 200, 300, 400);
        sink.update_thumbnail(wid, r);
        assert_eq!(sink.last_dst_rect(wid), Some(r));
    }

    #[test]
    fn mock_exit_unregisters_all_thumbnails() {
        let sink = MockThumbnailOverview::new();
        sink.enter();
        sink.register(WindowId::new(1), 100);
        sink.register(WindowId::new(2), 200);
        sink.register(WindowId::new(3), 300);
        assert_eq!(sink.registered_count(), 3);
        sink.exit();
        assert_eq!(sink.unregister_calls(), 3);
        assert_eq!(sink.registered_count(), 0);
        assert_eq!(sink.exit_calls(), 1);
    }
}
