//! Session-lifetime Win32 host. Only this thread touches its HWND/DWM handles.
use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use windows::core::w;
use windows::Win32::Foundation::{
    CloseHandle, BOOL, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM,
    LRESULT, RECT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
    DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetStockObject, InvalidateRect,
    RoundRect, SelectObject, SetBkMode, SetTextColor, TextOutW, NULL_PEN, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::*;

const REFRESH: u32 = WM_APP + 74;
const FADE_BEGIN: u32 = WM_APP + 75;
const OPEN_TIMER: usize = 1;
const FADE_TIMER: usize = 2;
const BACKDROP: u32 = 0x202020;
#[derive(Default)]
struct MailboxState {
    frame: Option<RecentPreviewFrame>,
    hwnd: Option<isize>,
    closed: bool,
    queued: bool,
    /// The `recent-windows-close` fade this session was started with (`0` = none).
    fade_ms: u32,
    /// The session is fading out: it still owns its window, but a new session needs one of
    /// its own.
    fading: bool,
}
struct Mailbox {
    state: Mutex<MailboxState>,
    count: AtomicUsize,
    // Owned manual-reset event, stored as an integer so the mailbox can cross threads.
    cancel_event: isize,
}
impl Mailbox {
    fn new() -> windows::core::Result<Self> {
        let event = unsafe { CreateEventW(None, true, false, None)? };
        Ok(Self {
            state: Mutex::default(),
            count: AtomicUsize::new(0),
            cancel_event: event.0 as isize,
        })
    }
    fn cancel_handle(&self) -> HANDLE {
        HANDLE(self.cancel_event as *mut _)
    }
    // Called with the mailbox locked. REFRESH carries frame updates; shutdown
    // uses the session event independently of message-queue capacity.
    fn wake(state: &mut MailboxState) {
        if let Some(raw) = state.hwnd {
            if !state.queued {
                state.queued = unsafe {
                    PostMessageW(HWND(raw as *mut _), REFRESH, WPARAM(0), LPARAM(0)).is_ok()
                };
            }
        }
    }
    fn close(&self) {
        let mut state = self.state.lock();
        // niri `recent-windows-close`: when a fade is configured the host window fades its
        // layered alpha out and only then ends the session (a timer runs for that long and
        // is killed at the end).  Without one, this is the immediate close it always was.
        if state.fade_ms > 0 && !state.closed {
            if let Some(raw) = state.hwnd {
                state.fading = true;
                state.frame = None;
                let posted = unsafe {
                    PostMessageW(HWND(raw as *mut _), FADE_BEGIN, WPARAM(0), LPARAM(0)).is_ok()
                };
                if posted {
                    return;
                }
                state.fading = false;
            }
        }
        state.closed = true;
        state.frame = None;
        drop(state);
        unsafe {
            let _ = SetEvent(self.cancel_handle());
        }
    }
    fn unpublish(&self) {
        let mut state = self.state.lock();
        state.hwnd = None; // before destruction: never post to a reused HWND
        state.closed = true;
        state.queued = false;
        state.frame = None;
    }
    fn closed(&self) -> bool {
        self.state.lock().closed
    }
}
impl Drop for Mailbox {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.cancel_handle());
        }
    }
}

#[derive(Default)]
pub struct DwmRecentWindows {
    current: Mutex<Option<Arc<Mailbox>>>,
}
impl DwmRecentWindows {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}
impl RecentPreviewSink for DwmRecentWindows {
    fn show(&self, frame: RecentPreviewFrame, delay: u32) {
        let mut current = self.current.lock();
        if let Some(mailbox) = current.as_ref() {
            let mut state = mailbox.state.lock();
            if !state.closed && !state.fading {
                state.frame = Some(frame);
                Mailbox::wake(&mut state);
                return;
            }
        }
        let Ok(mailbox) = Mailbox::new() else {
            return;
        };
        let mailbox = Arc::new(mailbox);
        {
            let mut state = mailbox.state.lock();
            state.frame = Some(frame);
            state.fade_ms = super::close_fade().map_or(0, |fade| fade.duration_ms());
        }
        *current = Some(mailbox.clone());
        if std::thread::Builder::new()
            .name("wiri-recent-preview".into())
            .spawn(move || unsafe {
                run_host(mailbox, delay);
            })
            .is_err()
        {
            *current = None;
        }
    }
    fn hide(&self) {
        if let Some(mailbox) = self.current.lock().take() {
            mailbox.close();
        }
    }
    fn registered_count(&self) -> usize {
        self.current
            .lock()
            .as_ref()
            .map_or(0, |m| m.count.load(Ordering::Acquire))
    }
}
impl Drop for DwmRecentWindows {
    fn drop(&mut self) {
        self.hide();
    }
}

struct Host {
    mailbox: Arc<Mailbox>,
    frame: Option<RecentPreviewFrame>,
    thumbnails: HashMap<WindowId, isize>,
    /// The running `recent-windows-close` fade: when it started and how it looks.
    fade: Option<(std::time::Instant, super::CloseFade)>,
}
impl Drop for Host {
    fn drop(&mut self) {
        for (_, h) in self.thumbnails.drain() {
            unsafe {
                let _ = DwmUnregisterThumbnail(h);
            }
        }
        self.mailbox.count.store(0, Ordering::Release);
        self.mailbox.unpublish();
    }
}

unsafe fn ensure_class() -> windows::core::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    if REGISTERED.get().is_some() {
        return Ok(());
    }
    let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: module.into(),
        lpszClassName: w!("WiriRecentWindowsHost"),
        ..Default::default()
    };
    if RegisterClassExW(&wc) == 0 {
        let error = windows::core::Error::from_win32();
        if error.code() != windows::core::HRESULT::from_win32(ERROR_CLASS_ALREADY_EXISTS.0) {
            return Err(error);
        }
        let mut existing = WNDCLASSEXW {
            cbSize: wc.cbSize,
            ..Default::default()
        };
        GetClassInfoExW(wc.hInstance, wc.lpszClassName, &mut existing)?;
        if existing.hInstance != wc.hInstance
            || existing.lpfnWndProc.map(|callback| callback as usize)
                != Some(wnd_proc as *const () as usize)
        {
            return Err(error);
        }
    }
    let _ = REGISTERED.set(());
    Ok(())
}
unsafe fn run_host(mailbox: Arc<Mailbox>, delay: u32) {
    if let Err(error) = ensure_class() {
        mailbox.unpublish();
        tracing::warn!(?error, "recent-windows host class registration failed");
        return;
    }
    let mut host = Box::new(Host {
        mailbox: mailbox.clone(),
        frame: None,
        thumbnails: HashMap::new(),
        fade: None,
    });
    if mailbox.state.lock().closed {
        return;
    }
    let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok();
    // Derive once: do not reborrow the Box while GWLP_USERDATA holds this pointer.
    let ptr = &mut *host as *mut Host;
    let hwnd = match CreateWindowExW(
        WS_EX_TOPMOST
            | WS_EX_TOOLWINDOW
            | WS_EX_NOACTIVATE
            | WS_EX_TRANSPARENT
            | WS_EX_LAYERED,
        w!("WiriRecentWindowsHost"),
        w!("Wiri Recent Windows"),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        module.map(Into::<HINSTANCE>::into).unwrap_or_default(),
        Some(ptr.cast()),
    ) {
        Ok(hwnd) => hwnd,
        Err(_) => return,
    };
    {
        let mut state = mailbox.state.lock();
        if !state.closed {
            state.hwnd = Some(hwnd.0 as isize);
        }
    }
    if mailbox.closed() || !refresh(hwnd, ptr) {
        close_host(hwnd, &mailbox);
        return;
    }
    if delay == 0 || SetTimer(hwnd, OPEN_TIMER, delay, None) == 0 {
        show_host(hwnd, &mailbox);
    }
    let cancel = [mailbox.cancel_handle()];
    let mut msg = MSG::default();
    'messages: loop {
        let wake = MsgWaitForMultipleObjects(Some(&cancel), false, INFINITE, QS_ALLINPUT);
        // Cancellation (the first handle) or a failed wait ends this session.
        if wake.0 != WAIT_OBJECT_0.0 + cancel.len() as u32 {
            break;
        }
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT || mailbox.closed() {
                break 'messages;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // Unpublish before destruction, including wait failure. Never join
    // this session thread from hide()/the engine lock.
    close_host(hwnd, &mailbox);
}

unsafe fn close_host(hwnd: HWND, mailbox: &Mailbox) {
    mailbox.unpublish();
    let _ = KillTimer(hwnd, OPEN_TIMER);
    let _ = KillTimer(hwnd, FADE_TIMER);
    if IsWindow(hwnd).as_bool() {
        let _ = DestroyWindow(hwnd);
    }
}
unsafe fn show_host(hwnd: HWND, mailbox: &Mailbox) {
    if mailbox.closed() {
        close_host(hwnd, mailbox);
        return;
    }
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    // A cancellation can race the nonblocking show. Do not leave that window
    // visible while the session thread handles its cancellation event.
    if mailbox.closed() {
        close_host(hwnd, mailbox);
    }
}

// SetWindowPos/ShowWindow/BeginPaint can synchronously reenter wnd_proc. Never
// keep a reference to Host (especially &mut Host) alive across a Win32 call.
unsafe fn refresh(hwnd: HWND, ptr: *mut Host) -> bool {
    let mailbox = (*ptr).mailbox.clone();
    let frame = {
        let mut state = mailbox.state.lock();
        state.queued = false;
        if state.closed {
            return false;
        }
        state.frame.take()
    };
    let Some(frame) = frame else {
        return true;
    };
    let mut thumbnails = std::mem::take(&mut (*ptr).thumbnails);
    let wanted: HashSet<_> = frame.previews.iter().map(|p| p.id).collect();
    thumbnails.retain(|id, handle| {
        if wanted.contains(id) {
            true
        } else {
            let _ = DwmUnregisterThumbnail(*handle);
            false
        }
    });
    let _ = SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        frame.monitor.loc.x,
        frame.monitor.loc.y,
        frame.monitor.size.w as i32,
        frame.monitor.size.h as i32,
        SWP_NOACTIVATE,
    );
    for preview in &frame.previews {
        if !thumbnails.contains_key(&preview.id) {
            match DwmRegisterThumbnail(hwnd, HWND(preview.id.as_isize() as *mut _)) {
                Ok(handle) => {
                    thumbnails.insert(preview.id, handle);
                }
                Err(_) => continue,
            }
        }
        let handle = thumbnails[&preview.id];
        let clipped = DwmQueryThumbnailSourceSize(handle).ok().and_then(|size| {
            clip_preview(
                preview.rect,
                Rect::new(0, 0, frame.monitor.size.w, frame.monitor.size.h),
                size.cx.max(0) as u32,
                size.cy.max(0) as u32,
            )
        });
        let Some((destination, source)) = clipped else {
            let _ = DwmUnregisterThumbnail(handle);
            thumbnails.remove(&preview.id);
            continue;
        };
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION
                | DWM_TNP_RECTSOURCE
                | DWM_TNP_OPACITY
                | DWM_TNP_VISIBLE
                | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: rc(destination),
            rcSource: rc(source),
            opacity: 255,
            fVisible: BOOL(1),
            fSourceClientAreaOnly: BOOL(0),
            ..Default::default()
        };
        if DwmUpdateThumbnailProperties(handle, &properties).is_err() {
            let _ = DwmUnregisterThumbnail(handle);
            thumbnails.remove(&preview.id);
        }
    }
    mailbox.count.store(thumbnails.len(), Ordering::Release);
    // Session-local, read-only diagnostics for the external live harness. These
    // reflect successful real DWM registrations, never merely requested tiles.
    let _ = SetPropW(
        hwnd,
        w!("WiriRecentThumbnailCount"),
        HANDLE(thumbnails.len() as *mut _),
    );
    let selected = frame.previews.iter().find(|p| p.selected);
    let _ = SetPropW(
        hwnd,
        w!("WiriRecentSelectedWindow"),
        HANDLE(selected.map_or(0, |p| p.id.as_isize()) as *mut _),
    );
    let _ = SetPropW(
        hwnd,
        w!("WiriRecentScope"),
        HANDLE(frame.scope as isize as *mut _),
    );
    let rect = selected.map(|p| p.rect).unwrap_or_default();
    let _ = SetPropW(
        hwnd,
        w!("WiriRecentPreviewWidth"),
        HANDLE(rect.size.w as usize as *mut _),
    );
    let _ = SetPropW(
        hwnd,
        w!("WiriRecentPreviewHeight"),
        HANDLE(rect.size.h as usize as *mut _),
    );
    tracing::debug!(previews = thumbnails.len(), scope = ?frame.scope, "recent-windows previews updated");
    (*ptr).thumbnails = thumbnails;
    (*ptr).frame = Some(frame);
    let _ = InvalidateRect(hwnd, None, false);
    !mailbox.closed()
}
fn rc(rect: Rect) -> RECT {
    RECT {
        left: rect.loc.x,
        top: rect.loc.y,
        right: rect.right(),
        bottom: rect.bottom(),
    }
}
fn highlight_color(text: &str, dim: bool) -> COLORREF {
    let rgba = crate::config::effects::parse_css_color(text).unwrap_or([153, 153, 153, 255]);
    let alpha = rgba[3] as f64 / 255.0 * if dim { 0.35 } else { 1.0 };
    let channel = |c: u8| (c as f64 * alpha + 32.0 * (1.0 - alpha)).round() as u32;
    COLORREF(channel(rgba[0]) | (channel(rgba[1]) << 8) | (channel(rgba[2]) << 16))
}
unsafe fn paint(hwnd: HWND, frame: Option<RecentPreviewFrame>) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let brush = CreateSolidBrush(COLORREF(BACKDROP));
    FillRect(hdc, &ps.rcPaint, brush);
    let _ = DeleteObject(brush);
    if let Some(frame) = &frame {
        let pad = (frame.highlight.padding * frame.scale).round() as i32;
        for p in frame.previews.iter().filter(|p| p.selected || p.urgent) {
            let brush = CreateSolidBrush(highlight_color(
                if p.urgent {
                    &frame.highlight.urgent_color
                } else {
                    &frame.highlight.active_color
                },
                !p.selected,
            ));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
            let radius = (2.0 * frame.highlight.corner_radius * frame.scale).round() as i32;
            let _ = RoundRect(
                hdc,
                p.rect.loc.x - pad,
                p.rect.loc.y - pad,
                p.rect.right() + pad,
                p.rect.bottom() + pad,
                radius,
                radius,
            );
            let _ = SelectObject(hdc, old_pen);
            let _ = SelectObject(hdc, old_brush);
            let _ = DeleteObject(brush);
        }
        let text = match frame.scope {
            RecentScope::All => "[All]   Output   Workspace",
            RecentScope::Output => "All   [Output]   Workspace",
            RecentScope::Workspace => "All   Output   [Workspace]",
        };
        let text: Vec<_> = text.encode_utf16().collect();
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(0xeeeeee));
        let _ = TextOutW(hdc, 30, 24, &text);
    }
    let _ = EndPaint(hwnd, &ps);
}
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Host;
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_TIMER if wp.0 == OPEN_TIMER => {
            let _ = KillTimer(hwnd, OPEN_TIMER); // one-shot; never an idle wakeup
            if !ptr.is_null() {
                let mailbox = (*ptr).mailbox.clone();
                show_host(hwnd, &mailbox);
            }
            return LRESULT(0);
        }
        // niri `recent-windows-close`: the overlay fades out.  The timer exists for the
        // length of the fade only (it is the one the open delay uses, re-armed) and is
        // killed when the alpha reaches zero, so nothing wakes up at rest.
        FADE_BEGIN => {
            if !ptr.is_null() {
                let mailbox = (*ptr).mailbox.clone();
                let Some(fade) = super::close_fade() else {
                    let _ = SetEvent(mailbox.cancel_handle());
                    return LRESULT(0);
                };
                (*ptr).fade = Some((std::time::Instant::now(), fade));
                if SetTimer(hwnd, FADE_TIMER, 16, None) == 0 {
                    // No timer: end the session rather than leave the overlay up.
                    let _ = SetEvent(mailbox.cancel_handle());
                }
            }
            return LRESULT(0);
        }
        WM_TIMER if wp.0 == FADE_TIMER => {
            if !ptr.is_null() {
                if let Some((started, fade)) = (*ptr).fade {
                    let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
                    let alpha = fade.alpha(elapsed);
                    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
                    if fade.finished(elapsed) {
                        let _ = KillTimer(hwnd, FADE_TIMER);
                        (*ptr).fade = None;
                        let mailbox = (*ptr).mailbox.clone();
                        let _ = SetEvent(mailbox.cancel_handle());
                    }
                } else {
                    let _ = KillTimer(hwnd, FADE_TIMER);
                }
            }
            return LRESULT(0);
        }
        REFRESH if !ptr.is_null() => {
            if !refresh(hwnd, ptr) {
                let mailbox = (*ptr).mailbox.clone();
                close_host(hwnd, &mailbox);
            }
            return LRESULT(0);
        }
        WM_PAINT if !ptr.is_null() => {
            let frame = (*ptr).frame.clone();
            paint(hwnd, frame);
            return LRESULT(0);
        }
        WM_CLOSE if !ptr.is_null() => {
            let mailbox = (*ptr).mailbox.clone();
            close_host(hwnd, &mailbox);
            return LRESULT(0);
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, OPEN_TIMER);
            let _ = KillTimer(hwnd, FADE_TIMER);
            if !ptr.is_null() {
                (*ptr).fade = None;
            }
            PostQuitMessage(0);
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            if !ptr.is_null() {
                (&(*ptr).mailbox).unpublish();
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
