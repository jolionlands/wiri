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
//! * one fullscreen layered topmost host HWND, created on its own
//!   message-pump thread by the first [`enter`](ThumbnailOverviewSink::enter),
//!   shown for each overview session and hidden again on
//!   [`exit`](ThumbnailOverviewSink::exit) (a window belongs to the thread that
//!   created it, and the engine enters and leaves overview from several threads);
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
    /// [`enter`](Self::enter) call, and hide the host window(s).  Must not
    /// block on another thread: the engine lock is held across this call.
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
    use windows::Win32::Graphics::Gdi::CreateSolidBrush;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics, MSG,
        RegisterClassExW, SetLayeredWindowAttributes, SetWindowPos, ShowWindowAsync,
        TranslateMessage, CS_HREDRAW, CS_VREDRAW, HWND_TOPMOST, LWA_ALPHA, SM_CXSCREEN,
        SM_CYSCREEN, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, WNDCLASSEXW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
        WS_POPUP,
    };

    /// Opacity of the host window (0-255).  The thumbnails are composited *inside* the
    /// host window, so a fully transparent host shows nothing at all; slightly below
    /// opaque keeps a hint of the desktop behind the dark backdrop.
    const HOST_ALPHA: u8 = 235;

    /// Backdrop colour of the host window (`0x00BBGGRR`).
    const HOST_BACKDROP: u32 = 0x0020_1810;

    /// Win32-backed thumbnail overview.
    pub struct DwmThumbnailOverview {
        state: Arc<Mutex<DwmState>>,
    }

    #[derive(Default)]
    struct DwmState {
        /// Host HWND raw value.  Created once, on a thread of its own that pumps its
        /// messages, and kept hidden between overview sessions.
        host: Option<isize>,
        /// An overview session is in progress (the host window is shown).
        active: bool,
        /// Map from layout-engine window id → DWM thumbnail handle.
        /// Each `HTHUMBNAIL` is an `isize` in `windows 0.58`.
        thumbnails: HashMap<WindowId, isize>,
    }

    impl DwmThumbnailOverview {
        /// Construct a new sink.  No Win32 resources are touched until
        /// the first `enter` call.
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                state: Arc::new(Mutex::new(DwmState::default())),
            })
        }
    }

    impl ThumbnailOverviewSink for DwmThumbnailOverview {
        fn enter(&self) {
            let mut s = self.state.lock();
            if s.host.is_none() {
                s.host = spawn_host_window();
            }
            let Some(raw) = s.host else {
                // Couldn't create the host; bail without leaving state half-set.
                return;
            };
            // A re-entry starts fresh.
            for (_wid, hthumb) in s.thumbnails.drain() {
                unsafe {
                    let _ = DwmUnregisterThumbnail(hthumb);
                }
            }
            if s.active {
                return;
            }
            s.active = true;
            // Every call below is asynchronous: the host window belongs to its own
            // thread, and a synchronous cross-thread call would wait for that thread
            // while the engine lock is held here.
            unsafe {
                let hwnd = HWND(raw as *mut _);
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    GetSystemMetrics(SM_CXSCREEN),
                    GetSystemMetrics(SM_CYSCREEN),
                    SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                );
                let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
            }
        }

        fn register(&self, window_id: WindowId, source_hwnd: isize) {
            let mut s = self.state.lock();
            if !s.active {
                return;
            }
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
                unsafe {
                    let _ = DwmUnregisterThumbnail(hthumb);
                }
            }
            if s.active {
                s.active = false;
                if let Some(raw) = s.host {
                    // Hide, don't destroy: the window is owned by its pump thread, and a
                    // hidden window costs nothing until the next overview.
                    unsafe {
                        let _ = ShowWindowAsync(HWND(raw as *mut _), SW_HIDE);
                    }
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

    fn ensure_host_class() {
        static REGISTERED: OnceLock<()> = OnceLock::new();
        REGISTERED.get_or_init(|| unsafe {
            let class_name = wide("WiriThumbnailOverviewHost");
            let hinstance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .expect("GetModuleHandleW failed");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(host_wnd_proc),
                hInstance: hinstance.into(),
                hbrBackground: CreateSolidBrush(COLORREF(HOST_BACKDROP)),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);
        });
    }

    /// Create the host window on a dedicated thread that pumps its messages for the
    /// life of the process, and return its handle (`None` when it could not be created).
    ///
    /// A window belongs to the thread that created it.  The engine enters and leaves
    /// overview from different threads (the hotkey thread, the keyboard-hook thread, ...),
    /// and a cross-thread `ShowWindow` / `DestroyWindow` waits for the owning thread to pump
    /// messages - a deadlock when that thread is itself waiting for the engine lock the
    /// caller holds (Escape does exactly that: it reaches wiri as a hotkey *and* through
    /// the keyboard hook).  `DestroyWindow` from another thread does not work at all.
    fn spawn_host_window() -> Option<isize> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Option<isize>>(1);
        let spawned = std::thread::Builder::new()
            .name("wiri-overview-host".to_string())
            .spawn(move || unsafe { host_thread(tx) });
        if spawned.is_err() {
            return None;
        }
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .ok()
            .flatten()
    }

    unsafe fn host_thread(ready: std::sync::mpsc::SyncSender<Option<isize>>) {
        ensure_host_class();
        let class_name = wide("WiriThumbnailOverviewHost");
        let hinstance = match windows::Win32::System::LibraryLoader::GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => {
                let _ = ready.send(None);
                return;
            }
        };
        // Topmost layered host.  WS_EX_TRANSPARENT lets mouse events pass through so the
        // engine's own overview hit-testing keeps working.
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
            0,
            0,
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
            None,
            None,
            hinstance,
            None,
        ) {
            Ok(h) => h,
            Err(_) => {
                let _ = ready.send(None);
                return;
            }
        };
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), HOST_ALPHA, LWA_ALPHA);
        let _ = ready.send(Some(hwnd.0 as isize));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
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
