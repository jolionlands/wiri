//! Zoomed workspace overview, rendered with live DWM thumbnails.
//!
//! Source HWNDs are never resized here. All HWND/DWM/GDI operations belong to
//! a session-local message thread, which is destroyed on exit (no idle thread
//! or timer). Engine calls only publish snapshots and post messages: in
//! particular, `exit` never waits while the engine lock is held.
//!
//! The engine supplies workspace backgrounds and its keyboard focus through
//! `set_context`, and keeps using `register`/`update_thumbnail` for tile geometry.
//! Mouse selections are delivered to `set_handler` on the overlay thread. The
//! handler must route them through the engine, not activate source HWNDs directly.

use crate::utils::{Rect, WindowId};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// A zoomed workspace background, in screen pixels, including empty workspaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewWorkspace {
    pub id: i32,
    pub rect: Rect,
    pub active: bool,
}

/// Publish this after computing an overview layout, including after keyboard
/// navigation. Bounds are the focused monitor, not the primary screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewContext {
    pub bounds: Rect,
    pub workspaces: Vec<OverviewWorkspace>,
    pub focused: Option<WindowId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewCommand {
    /// Focus the window (switching workspace if needed), then exit overview.
    SelectWindow(WindowId),
    /// Activate even an empty workspace, then exit overview.
    SelectWorkspace(i32),
    Close,
}

pub type OverviewHandler = Arc<dyn Fn(OverviewCommand) + Send + Sync>;

pub trait ThumbnailOverviewSink: Send + Sync {
    fn enter(&self);
    fn register(&self, window_id: WindowId, source_hwnd: isize);
    /// Destination in screen coordinates, not host-client coordinates.
    fn update_thumbnail(&self, window_id: WindowId, dst_rect: Rect);
    /// Default keeps existing/test sinks source-compatible.
    fn set_context(&self, _context: OverviewContext) {}
    /// Asynchronous: safe while holding the engine lock.
    fn exit(&self);
    fn registered_count(&self) -> usize;
}

#[derive(Default)]
pub struct MockThumbnailOverview {
    inner: Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    register_calls: u32,
    unregister_calls: u32,
    enter_calls: u32,
    exit_calls: u32,
    last_dst_rect: HashMap<WindowId, Rect>,
    registered: HashMap<WindowId, isize>,
    context: Option<OverviewContext>,
}

impl MockThumbnailOverview {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn register_calls(&self) -> u32 {
        self.inner.lock().register_calls
    }
    pub fn unregister_calls(&self) -> u32 {
        self.inner.lock().unregister_calls
    }
    pub fn enter_calls(&self) -> u32 {
        self.inner.lock().enter_calls
    }
    pub fn exit_calls(&self) -> u32 {
        self.inner.lock().exit_calls
    }
    pub fn last_dst_rect(&self, id: WindowId) -> Option<Rect> {
        self.inner.lock().last_dst_rect.get(&id).copied()
    }
    pub fn last_context(&self) -> Option<OverviewContext> {
        self.inner.lock().context.clone()
    }
    pub fn registered_count(&self) -> usize {
        self.inner.lock().registered.len()
    }
}

impl ThumbnailOverviewSink for MockThumbnailOverview {
    fn enter(&self) {
        let mut s = self.inner.lock();
        s.enter_calls += 1;
        s.unregister_calls += s.registered.len() as u32;
        s.registered.clear();
        s.last_dst_rect.clear();
        s.context = None;
    }
    fn register(&self, id: WindowId, source: isize) {
        let mut s = self.inner.lock();
        if s.registered.insert(id, source).is_none() {
            s.register_calls += 1;
        }
    }
    fn update_thumbnail(&self, id: WindowId, rect: Rect) {
        let mut s = self.inner.lock();
        if s.registered.contains_key(&id) {
            s.last_dst_rect.insert(id, rect);
        }
    }
    fn set_context(&self, context: OverviewContext) {
        self.inner.lock().context = Some(context);
    }
    fn exit(&self) {
        let mut s = self.inner.lock();
        s.exit_calls += 1;
        s.unregister_calls += s.registered.len() as u32;
        s.registered.clear();
        s.last_dst_rect.clear();
        s.context = None;
    }
    fn registered_count(&self) -> usize {
        self.inner.lock().registered.len()
    }
}

// Geometry/hit-testing deliberately contain no Win32 calls.
#[cfg(any(target_os = "windows", test))]
fn contains(rect: Rect, x: i32, y: i32) -> bool {
    let (x, y) = (i64::from(x), i64::from(y));
    x >= i64::from(rect.loc.x)
        && y >= i64::from(rect.loc.y)
        && x < i64::from(rect.loc.x) + i64::from(rect.size.w)
        && y < i64::from(rect.loc.y) + i64::from(rect.size.h)
}

#[cfg(any(target_os = "windows", test))]
fn hit_test(
    context: Option<&OverviewContext>,
    tiles: impl IntoIterator<Item = (WindowId, Rect)>,
    x: i32,
    y: i32,
) -> OverviewCommand {
    if let Some(context) = context {
        if !contains(context.bounds, x, y) {
            return OverviewCommand::Close;
        }
        // Focused tile is visually on top when a layout supplies overlapping rects.
        let tiles: Vec<_> = tiles.into_iter().collect();
        if let Some((id, _)) = tiles
            .iter()
            .find(|(id, r)| Some(*id) == context.focused && contains(*r, x, y))
        {
            return OverviewCommand::SelectWindow(*id);
        }
        if let Some((id, _)) = tiles.iter().find(|(_, r)| contains(*r, x, y)) {
            return OverviewCommand::SelectWindow(*id);
        }
        if let Some(ws) = context.workspaces.iter().find(|w| contains(w.rect, x, y)) {
            return OverviewCommand::SelectWorkspace(ws.id);
        }
    } else if let Some((id, _)) = tiles.into_iter().find(|(_, r)| contains(*r, x, y)) {
        return OverviewCommand::SelectWindow(id);
    }
    OverviewCommand::Close
}

/// Clip to the monitor and convert screen -> host coordinates, cropping the
/// source proportionally. Merely clipping rcDestination would squash previews.
#[cfg(any(target_os = "windows", test))]
fn thumbnail_geometry(dst: Rect, bounds: Rect, source: (i32, i32)) -> Option<(Rect, Rect)> {
    if dst.size.w == 0 || dst.size.h == 0 || source.0 <= 0 || source.1 <= 0 {
        return None;
    }
    let x = i64::from(dst.loc.x).max(i64::from(bounds.loc.x));
    let y = i64::from(dst.loc.y).max(i64::from(bounds.loc.y));
    let right = (i64::from(dst.loc.x) + i64::from(dst.size.w))
        .min(i64::from(bounds.loc.x) + i64::from(bounds.size.w));
    let bottom = (i64::from(dst.loc.y) + i64::from(dst.size.h))
        .min(i64::from(bounds.loc.y) + i64::from(bounds.size.h));
    if right <= x || bottom <= y {
        return None;
    }
    let sx =
        |v: i64| ((v - i64::from(dst.loc.x)) * i64::from(source.0) / i64::from(dst.size.w)) as i32;
    let sy =
        |v: i64| ((v - i64::from(dst.loc.y)) * i64::from(source.1) / i64::from(dst.size.h)) as i32;
    let src = Rect::new(
        sx(x),
        sy(y),
        (sx(right) - sx(x)).max(1) as u32,
        (sy(bottom) - sy(y)).max(1) as u32,
    );
    Some((
        Rect::new(
            (x - i64::from(bounds.loc.x)) as i32,
            (y - i64::from(bounds.loc.y)) as i32,
            (right - x) as u32,
            (bottom - y) as u32,
        ),
        src,
    ))
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        OnceLock,
    };
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        BOOL, COLORREF, HANDLE, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM,
    };
    use windows::Win32::Graphics::Dwm::{
        DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
        DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
        DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
    };
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, SetBkMode,
        SetTextColor, TextOutW, HDC, PAINTSTRUCT, TRANSPARENT,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;

    const REFRESH: u32 = WM_APP + 0x219;
    const BACKDROP: u32 = 0x0018_1210;
    const WORKSPACE: u32 = 0x0036_2b24;
    const FOCUS: u32 = 0x00ff_c878;
    const HOVER: u32 = 0x00aa_aaa0;

    #[derive(Clone)]
    struct Tile {
        source: isize,
        rect: Option<Rect>,
    }
    #[derive(Clone, Default)]
    struct Scene {
        context: Option<OverviewContext>,
        tiles: HashMap<WindowId, Tile>,
    }
    struct Session {
        scene: Mutex<Scene>,
        hwnd: AtomicIsize,
        queued: AtomicBool,
        closed: AtomicBool,
        handler: Arc<Mutex<Option<OverviewHandler>>>,
    }
    impl Session {
        fn wake(&self) {
            let raw = self.hwnd.load(Ordering::Acquire);
            if raw != 0 && !self.queued.swap(true, Ordering::AcqRel) {
                if unsafe { PostMessageW(HWND(raw as *mut _), REFRESH, WPARAM(0), LPARAM(0)) }
                    .is_err()
                {
                    self.queued.store(false, Ordering::Release);
                }
            }
        }
        fn close(&self) {
            self.closed.store(true, Ordering::Release);
            self.wake();
        }
        fn command(&self, command: OverviewCommand) {
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            // Do not hold any overlay mutex across a callback into the engine.
            let handler = self.handler.lock().clone();
            if let Some(handler) = handler {
                handler(command);
            }
        }
    }

    pub struct DwmThumbnailOverview {
        session: Mutex<Option<Arc<Session>>>,
        handler: Arc<Mutex<Option<OverviewHandler>>>,
    }
    impl DwmThumbnailOverview {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                session: Mutex::new(None),
                handler: Arc::new(Mutex::new(None)),
            })
        }
        /// Called once daemon engine/backend handles are available. Use a weak
        /// engine reference to avoid an engine -> sink -> engine reference cycle.
        /// Calls execute on the session thread; enter/update/exit never wait on it.
        pub fn set_handler(&self, handler: OverviewHandler) {
            *self.handler.lock() = Some(handler);
        }
        fn publish(&self, update: impl FnOnce(&mut Scene)) {
            if let Some(session) = self.session.lock().as_ref() {
                update(&mut session.scene.lock());
                session.wake();
            }
        }
    }
    impl ThumbnailOverviewSink for DwmThumbnailOverview {
        fn enter(&self) {
            self.exit();
            let session = Arc::new(Session {
                scene: Mutex::new(Scene::default()),
                hwnd: AtomicIsize::new(0),
                queued: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                handler: self.handler.clone(),
            });
            *self.session.lock() = Some(session.clone());
            let worker = session.clone();
            if std::thread::Builder::new()
                .name("wiri-overview-host".into())
                .spawn(move || unsafe { run(worker) })
                .is_err()
            {
                session.close();
            }
        }
        fn register(&self, id: WindowId, source: isize) {
            self.publish(|s| {
                s.tiles
                    .entry(id)
                    .and_modify(|t| t.source = source)
                    .or_insert(Tile { source, rect: None });
            });
        }
        fn update_thumbnail(&self, id: WindowId, rect: Rect) {
            self.publish(|s| {
                if let Some(tile) = s.tiles.get_mut(&id) {
                    tile.rect = Some(rect);
                }
            });
        }
        fn set_context(&self, context: OverviewContext) {
            self.publish(|s| s.context = Some(context));
        }
        fn exit(&self) {
            if let Some(session) = self.session.lock().take() {
                session.close();
            }
        }
        fn registered_count(&self) -> usize {
            self.session
                .lock()
                .as_ref()
                .map(|s| s.scene.lock().tiles.len())
                .unwrap_or(0)
        }
    }
    impl Drop for DwmThumbnailOverview {
        fn drop(&mut self) {
            self.exit();
        }
    }

    struct Thumbnail {
        source: isize,
        handle: isize,
    }
    struct Host {
        session: Arc<Session>,
        scene: Scene,
        bounds: Rect,
        thumbnails: HashMap<WindowId, Thumbnail>,
        hovered: Option<WindowId>,
    }
    impl Drop for Host {
        fn drop(&mut self) {
            self.session.hwnd.store(0, Ordering::Release);
            for (_, thumbnail) in self.thumbnails.drain() {
                unsafe {
                    let _ = DwmUnregisterThumbnail(thumbnail.handle);
                }
            }
        }
    }
    impl Host {
        fn tiles(&self) -> impl Iterator<Item = (WindowId, Rect)> + '_ {
            self.scene.tiles.iter().filter_map(|(id, t)| {
                // Do not select dead/failed DWM sources.
                if !self.thumbnails.contains_key(id) {
                    return None;
                }
                t.rect.map(|r| (*id, r))
            })
        }
        unsafe fn update(&mut self, hwnd: HWND) {
            self.scene = self.session.scene.lock().clone();
            if let Some(context) = &self.scene.context {
                self.bounds = context.bounds;
            }
            self.thumbnails.retain(|id, thumbnail| {
                let keep = self.scene.tiles.get(id).map(|t| t.source) == Some(thumbnail.source);
                if !keep {
                    let _ = DwmUnregisterThumbnail(thumbnail.handle);
                }
                keep
            });
            for (&id, tile) in &self.scene.tiles {
                if let std::collections::hash_map::Entry::Vacant(entry) = self.thumbnails.entry(id)
                {
                    if let Ok(handle) = DwmRegisterThumbnail(hwnd, HWND(tile.source as *mut _)) {
                        entry.insert(Thumbnail {
                            source: tile.source,
                            handle,
                        });
                    }
                }
                let Some(thumbnail) = self.thumbnails.get(&id) else {
                    continue;
                };
                let geometry = tile.rect.and_then(|rect| {
                    let size: SIZE = DwmQueryThumbnailSourceSize(thumbnail.handle).ok()?;
                    thumbnail_geometry(rect, self.bounds, (size.cx, size.cy))
                });
                let mut props = DWM_THUMBNAIL_PROPERTIES {
                    dwFlags: DWM_TNP_VISIBLE,
                    fVisible: BOOL(0),
                    ..Default::default()
                };
                if let Some((dst, src)) = geometry {
                    props.dwFlags |= DWM_TNP_RECTDESTINATION
                        | DWM_TNP_RECTSOURCE
                        | DWM_TNP_OPACITY
                        | DWM_TNP_SOURCECLIENTAREAONLY;
                    props.rcDestination = native_rect(dst);
                    props.rcSource = native_rect(src);
                    props.opacity = 255;
                    props.fVisible = BOOL(1);
                    props.fSourceClientAreaOnly = BOOL(0);
                }
                let _ = DwmUpdateThumbnailProperties(thumbnail.handle, &props);
            }
            // Session-local properties let the live harness assert real DWM
            // registrations and focus geometry, rather than trusting screenshots.
            property(
                hwnd,
                "WiriOverviewThumbnailCount",
                self.thumbnails.len() as isize,
            );
            let focused = self.scene.context.as_ref().and_then(|c| c.focused);
            property(
                hwnd,
                "WiriOverviewFocusedWindow",
                focused.map(WindowId::as_isize).unwrap_or(0),
            );
            property(
                hwnd,
                "WiriOverviewWorkspaceCount",
                self.scene
                    .context
                    .as_ref()
                    .map(|c| c.workspaces.len())
                    .unwrap_or(0) as isize,
            );
            let rect = focused
                .and_then(|id| self.scene.tiles.get(&id))
                .and_then(|t| t.rect)
                .unwrap_or_default();
            property(hwnd, "WiriOverviewFocusX", rect.loc.x as isize);
            property(hwnd, "WiriOverviewFocusY", rect.loc.y as isize);
            property(hwnd, "WiriOverviewFocusW", rect.size.w as isize);
            property(hwnd, "WiriOverviewFocusH", rect.size.h as isize);
            let _ = InvalidateRect(hwnd, None, false);
        }
        unsafe fn paint(&self, hwnd: HWND) {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            fill(
                dc,
                Rect::new(0, 0, self.bounds.size.w, self.bounds.size.h),
                BACKDROP,
            );
            if let Some(context) = &self.scene.context {
                for ws in &context.workspaces {
                    let r = local(ws.rect, self.bounds);
                    fill(dc, r, WORKSPACE);
                    ring(dc, r, if ws.active { HOVER } else { BACKDROP }, 1);
                    let text: Vec<u16> = format!("Workspace {}", ws.id).encode_utf16().collect();
                    let _ = SetBkMode(dc, TRANSPARENT);
                    let _ = SetTextColor(dc, COLORREF(0x00df_d8d0));
                    let _ = TextOutW(dc, r.loc.x + 6, r.loc.y + 5, &text);
                }
                if let Some(id) = self.hovered.filter(|id| Some(*id) != context.focused) {
                    if let Some(rect) = self.scene.tiles.get(&id).and_then(|t| t.rect) {
                        ring(dc, local(rect, self.bounds), HOVER, 2);
                    }
                }
                if let Some(rect) = context
                    .focused
                    .and_then(|id| self.scene.tiles.get(&id))
                    .and_then(|t| t.rect)
                {
                    ring(dc, local(rect, self.bounds), FOCUS, 4);
                }
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }

    unsafe fn refresh(ptr: *mut Host, hwnd: HWND) {
        let session = (*ptr).session.clone();
        session.queued.store(false, Ordering::Release);
        if session.closed.load(Ordering::Acquire) {
            let _ = DestroyWindow(hwnd);
            return;
        }
        (*ptr).update(hwnd);
        let bounds = (*ptr).bounds;
        // SetWindowPos/DestroyWindow may re-enter WndProc. No Host borrow is
        // alive across them, and no engine/scene lock is held here.
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            bounds.loc.x,
            bounds.loc.y,
            bounds.size.w.max(1) as i32,
            bounds.size.h.max(1) as i32,
            SWP_NOACTIVATE,
        );
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn native_rect(r: Rect) -> RECT {
        RECT {
            left: r.loc.x,
            top: r.loc.y,
            right: r.loc.x.saturating_add(r.size.w.min(i32::MAX as u32) as i32),
            bottom: r.loc.y.saturating_add(r.size.h.min(i32::MAX as u32) as i32),
        }
    }
    fn local(r: Rect, bounds: Rect) -> Rect {
        Rect::new(
            r.loc.x.saturating_sub(bounds.loc.x),
            r.loc.y.saturating_sub(bounds.loc.y),
            r.size.w,
            r.size.h,
        )
    }
    unsafe fn fill(dc: HDC, r: Rect, color: u32) {
        let brush = CreateSolidBrush(COLORREF(color));
        let _ = FillRect(dc, &native_rect(r), brush);
        let _ = DeleteObject(brush);
    }
    /// Outside the thumbnail: DWM composites on top of the host's GDI content.
    unsafe fn ring(dc: HDC, r: Rect, color: u32, width: u32) {
        let d = width as i32;
        fill(
            dc,
            Rect::new(r.loc.x - d, r.loc.y - d, r.size.w + width * 2, width),
            color,
        );
        fill(
            dc,
            Rect::new(
                r.loc.x - d,
                r.loc.y + r.size.h as i32,
                r.size.w + width * 2,
                width,
            ),
            color,
        );
        fill(dc, Rect::new(r.loc.x - d, r.loc.y, width, r.size.h), color);
        fill(
            dc,
            Rect::new(r.loc.x + r.size.w as i32, r.loc.y, width, r.size.h),
            color,
        );
    }
    unsafe fn property(hwnd: HWND, name: &str, value: isize) {
        let _ = SetPropW(hwnd, PCWSTR(wide(name).as_ptr()), HANDLE(value as *mut _));
    }
    unsafe fn register_class() {
        static CLASS: OnceLock<()> = OnceLock::new();
        CLASS.get_or_init(|| {
            let Ok(instance) = windows::Win32::System::LibraryLoader::GetModuleHandleW(None) else {
                return;
            };
            let name = wide("WiriThumbnailOverviewHost");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);
        });
    }
    unsafe fn run(session: Arc<Session>) {
        if session.closed.load(Ordering::Acquire) {
            return;
        }
        register_class();
        let Ok(instance) = windows::Win32::System::LibraryLoader::GetModuleHandleW(None) else {
            return;
        };
        let bounds = Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32,
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32,
        );
        let mut host = Box::new(Host {
            session: session.clone(),
            scene: Scene::default(),
            bounds,
            thumbnails: HashMap::new(),
            hovered: None,
        });
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PCWSTR(wide("WiriThumbnailOverviewHost").as_ptr()),
            PCWSTR(wide("").as_ptr()),
            WS_POPUP,
            bounds.loc.x,
            bounds.loc.y,
            bounds.size.w as i32,
            bounds.size.h as i32,
            None,
            None,
            instance,
            Some((&mut *host as *mut Host).cast()),
        ) else {
            return;
        };
        session.hwnd.store(hwnd.0 as isize, Ordering::Release);
        // Catches enter/exit racing before window creation; no ready-channel wait.
        refresh(&mut *host, hwnd);
        if !session.closed.load(Ordering::Acquire) {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // DWM handles and the HWND are destroyed by this thread only.
        if IsWindow(hwnd).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
    }
    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == WM_NCCREATE {
            let create = &*(l.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Host;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        // Do not hold a &mut Host across DestroyWindow/SetWindowPos: they can
        // synchronously re-enter this WndProc on the owning thread.
        match msg {
            REFRESH => {
                refresh(ptr, hwnd);
                LRESULT(0)
            }
            WM_PAINT => {
                (*ptr).paint(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_MOUSEMOVE | WM_LBUTTONUP => {
                let x = (l.0 as u16 as i16 as i32) + (*ptr).bounds.loc.x;
                let y = ((l.0 >> 16) as u16 as i16 as i32) + (*ptr).bounds.loc.y;
                let command = hit_test((*ptr).scene.context.as_ref(), (*ptr).tiles(), x, y);
                if msg == WM_LBUTTONUP {
                    let session = (*ptr).session.clone();
                    session.command(command);
                } else {
                    let hovered = match command {
                        OverviewCommand::SelectWindow(id) => Some(id),
                        _ => None,
                    };
                    if (*ptr).hovered != hovered {
                        (*ptr).hovered = hovered;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN if w.0 == 0x1b => {
                (*ptr).session.clone().command(OverviewCommand::Close);
                LRESULT(0)
            }
            WM_CLOSE => {
                (&(*ptr).session).closed.store(true, Ordering::Release);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
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
}

#[cfg(target_os = "windows")]
pub use win::DwmThumbnailOverview;

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> OverviewContext {
        OverviewContext {
            bounds: Rect::new(-1920, 0, 1920, 1080),
            focused: Some(WindowId::new(1)),
            workspaces: vec![OverviewWorkspace {
                id: 2,
                rect: Rect::new(-1800, 100, 900, 400),
                active: true,
            }],
        }
    }
    #[test]
    fn mock_registration_is_idempotent_and_exit_clears_everything() {
        let sink = MockThumbnailOverview::new();
        sink.enter();
        sink.register(WindowId::new(1), 100);
        sink.register(WindowId::new(1), 100);
        sink.update_thumbnail(WindowId::new(1), Rect::new(4, 5, 6, 7));
        sink.set_context(context());
        assert_eq!(sink.register_calls(), 1);
        assert_eq!(
            sink.last_dst_rect(WindowId::new(1)),
            Some(Rect::new(4, 5, 6, 7))
        );
        assert_eq!(sink.last_context(), Some(context()));
        sink.exit();
        assert_eq!(sink.registered_count(), 0);
        assert_eq!(sink.unregister_calls(), 1);
        assert_eq!(sink.last_context(), None);
        assert_eq!(sink.last_dst_rect(WindowId::new(1)), None);
        sink.enter();
        assert_eq!(sink.enter_calls(), 2);
        assert_eq!(sink.exit_calls(), 1);
    }
    #[test]
    fn new_session_drops_stale_focus_and_geometry() {
        let sink = MockThumbnailOverview::new();
        sink.enter();
        sink.register(WindowId::new(1), 100);
        sink.set_context(context());
        sink.enter();
        assert_eq!(sink.registered_count(), 0);
        assert_eq!(sink.last_context(), None);
        sink.update_thumbnail(WindowId::new(7), Rect::new(1, 2, 3, 4));
        assert_eq!(sink.last_dst_rect(WindowId::new(7)), None);
    }
    #[test]
    fn negative_monitor_origin_is_translated_and_source_is_cropped() {
        let bounds = context().bounds;
        let (dst, src) =
            thumbnail_geometry(Rect::new(-2020, -50, 400, 200), bounds, (800, 400)).unwrap();
        assert_eq!(dst, Rect::new(0, 0, 300, 150));
        assert_eq!(src, Rect::new(200, 100, 600, 300));
        assert_eq!(
            thumbnail_geometry(Rect::new(1, 0, 400, 200), bounds, (800, 400)),
            None
        );
        assert_eq!(
            thumbnail_geometry(Rect::new(-1800, 0, 0, 200), bounds, (800, 400)),
            None
        );
        assert_eq!(
            thumbnail_geometry(Rect::new(-1800, 0, 100, 200), bounds, (0, 400)),
            None
        );
    }
    #[test]
    fn entire_thumbnail_has_uncropped_source() {
        assert_eq!(
            thumbnail_geometry(
                Rect::new(-1800, 100, 400, 200),
                context().bounds,
                (800, 400)
            ),
            Some((Rect::new(120, 100, 400, 200), Rect::new(0, 0, 800, 400)))
        );
    }
    #[test]
    fn mouse_targets_window_before_workspace_and_supports_empty_workspace() {
        let c = context();
        let tiles = [(WindowId::new(1), Rect::new(-1750, 150, 100, 100))];
        assert_eq!(
            hit_test(Some(&c), tiles, -1700, 200),
            OverviewCommand::SelectWindow(WindowId::new(1))
        );
        assert_eq!(
            hit_test(Some(&c), tiles, -1100, 200),
            OverviewCommand::SelectWorkspace(2)
        );
        assert_eq!(hit_test(Some(&c), tiles, -1900, 20), OverviewCommand::Close);
        // Half-open edges and out-of-monitor coordinates must not select a tile.
        assert_eq!(
            hit_test(Some(&c), tiles, -1650, 250),
            OverviewCommand::SelectWorkspace(2)
        );
        assert_eq!(hit_test(Some(&c), tiles, 0, 200), OverviewCommand::Close);
    }
    #[test]
    fn keyboard_focus_wins_overlapping_hit_targets() {
        let c = context();
        let tiles = [
            (WindowId::new(2), Rect::new(-1750, 150, 100, 100)),
            (WindowId::new(1), Rect::new(-1750, 150, 100, 100)),
        ];
        assert_eq!(
            hit_test(Some(&c), tiles, -1700, 200),
            OverviewCommand::SelectWindow(WindowId::new(1))
        );
    }
}
