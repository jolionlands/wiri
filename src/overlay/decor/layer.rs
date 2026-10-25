//! The Win32 side of the overlay layer.
//!
//! One layered, click-through, never-activated tool window per decoration, stacked directly
//! below the window it belongs to.  The windows live on a thread of their own that is started
//! when the first decoration is needed and ends when the last one goes, so a desktop with no
//! decoration has no overlay window and no thread.  Nothing here runs on a timer: the thread
//! sleeps in a message wait until it is sent work, and checks the windows against their
//! targets once, shortly after each change.
//!
//! The engine hands over the whole set of decorations of a monitor ([`DecorSink::set_output`]);
//! the layer compares it with what it shows and creates, moves, repaints and destroys the
//! windows to match.  A window that only moved is moved with `SetWindowPos`; one whose look
//! changed is painted again and handed to the compositor with `UpdateLayeredWindow`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tracing::{debug, warn};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    COLORREF, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, IsWindow,
    MsgWaitForMultipleObjectsEx, PeekMessageW, PostThreadMessageW, RegisterClassExW, SetWindowPos,
    TranslateMessage, UpdateLayeredWindow, HTTRANSPARENT, HWND_TOP, MA_NOACTIVATE, MSG,
    MWMO_INPUTAVAILABLE, PM_NOREMOVE, PM_REMOVE, QS_ALLINPUT, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, ULW_ALPHA, WM_APP, WM_ERASEBKGND, WM_MOUSEACTIVATE,
    WM_NCHITTEST, WM_USER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use super::paint::Surface;
use super::spec::{DecorContent, DecorId, DecorSpec};
use super::DecorSink;
use crate::utils::Rect;

/// The window class of every decoration.
pub const CLASS_NAME: &str = "WiriDecor";

/// "There are commands in the queue", posted to the thread of the layer.
const WM_WORK: u32 = WM_APP + 1;

/// How long after a change the layer checks the windows against the frames of their targets.
const VERIFY_DELAY: Duration = Duration::from_millis(80);

static PAINTS: AtomicU64 = AtomicU64::new(0);
static MOVES: AtomicU64 = AtomicU64::new(0);

/// How often the layer repainted a window and how often it only moved one (for the live test).
pub fn stats() -> (u64, u64) {
    (
        PAINTS.load(Ordering::Relaxed),
        MOVES.load(Ordering::Relaxed),
    )
}

enum Cmd {
    Set(Vec<DecorSpec>),
    Moved(isize),
    Restack(isize),
    Quit,
}

struct Worker {
    tx: Sender<Cmd>,
    thread_id: u32,
    handle: std::thread::JoinHandle<()>,
}

impl Worker {
    fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_ok() {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_WORK, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[derive(Default)]
struct LayerState {
    /// The specs of every output, in the order of the output keys.
    per_output: BTreeMap<u64, Vec<DecorSpec>>,
    /// What the worker was last told.
    sent: Vec<DecorSpec>,
    /// The windows the sent specs are anchored to, for the hooks' cheap test.
    anchors: HashSet<isize>,
    worker: Option<Worker>,
}

/// The overlay layer: see the module documentation.
pub struct DecorLayer {
    state: Mutex<LayerState>,
}

impl DecorLayer {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LayerState::default()),
        }
    }
}

impl Default for DecorLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl DecorSink for DecorLayer {
    fn set_output(&self, output: u64, specs: Vec<DecorSpec>) {
        let mut st = self.state.lock();
        if specs.is_empty() {
            st.per_output.remove(&output);
        } else {
            st.per_output.insert(output, specs);
        }
        let merged: Vec<DecorSpec> = st.per_output.values().flatten().cloned().collect();
        if merged == st.sent {
            return;
        }
        if merged.is_empty() {
            // Nothing left to show: the windows and the thread go.
            if let Some(worker) = st.worker.take() {
                worker.send(Cmd::Set(Vec::new()));
                worker.send(Cmd::Quit);
                // The thread ends by itself once it has destroyed its windows.
                drop(worker.handle);
            }
            st.sent.clear();
            st.anchors.clear();
            return;
        }
        if st.worker.is_none() {
            st.worker = spawn_worker();
        }
        match &st.worker {
            Some(worker) => worker.send(Cmd::Set(merged.clone())),
            None => {
                warn!("the overlay layer could not start its thread; no decorations are drawn");
                return;
            }
        }
        st.anchors = merged
            .iter()
            .map(|s| s.anchor)
            .filter(|a| *a != 0)
            .collect();
        st.sent = merged;
    }

    fn moved(&self, hwnd: isize) {
        let st = self.state.lock();
        if st.anchors.contains(&hwnd) {
            if let Some(worker) = &st.worker {
                worker.send(Cmd::Moved(hwnd));
            }
        }
    }

    fn restack(&self, hwnd: isize) {
        let st = self.state.lock();
        if st.anchors.contains(&hwnd) {
            if let Some(worker) = &st.worker {
                worker.send(Cmd::Restack(hwnd));
            }
        }
    }

    fn shutdown(&self) {
        let worker = {
            let mut st = self.state.lock();
            st.per_output.clear();
            st.sent.clear();
            st.anchors.clear();
            st.worker.take()
        };
        if let Some(worker) = worker {
            worker.send(Cmd::Set(Vec::new()));
            worker.send(Cmd::Quit);
            let _ = worker.handle.join();
        }
    }
}

fn spawn_worker() -> Option<Worker> {
    let (tx, rx) = channel::<Cmd>();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<u32>(1);
    let handle = std::thread::Builder::new()
        .name("wiri-decor".to_string())
        .spawn(move || unsafe { worker_main(rx, ready_tx) })
        .ok()?;
    let thread_id = ready_rx.recv_timeout(Duration::from_secs(2)).ok()?;
    Some(Worker {
        tx,
        thread_id,
        handle,
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn hwnd_of(raw: isize) -> HWND {
    HWND(raw as *mut std::ffi::c_void)
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // Mouse input goes to whatever is below.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        // Never take the focus.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // The content comes from `UpdateLayeredWindow`.
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn ensure_class() {
    use std::sync::OnceLock;
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| unsafe {
        let class_name = wide(CLASS_NAME);
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => return,
        };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
    });
}

/// A top-down 32-bit bitmap the decorations are painted into.
struct Dib {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: i32,
    h: i32,
}

impl Dib {
    unsafe fn new(w: i32, h: i32) -> Option<Dib> {
        if w <= 0 || h <= 0 {
            return None;
        }
        let screen = GetDC(HWND::default());
        let dc = CreateCompatibleDC(screen);
        if dc.is_invalid() {
            let _ = ReleaseDC(HWND::default(), screen);
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: the first row is the top one
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default(); 1],
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0);
        let _ = ReleaseDC(HWND::default(), screen);
        let bitmap = match bitmap {
            Ok(b) if !b.is_invalid() && !bits.is_null() => b,
            _ => {
                let _ = DeleteDC(dc);
                return None;
            }
        };
        let old = SelectObject(dc, HGDIOBJ::from(bitmap));
        Some(Dib {
            dc,
            bitmap,
            old,
            bits: bits as *mut u32,
            w,
            h,
        })
    }

    fn surface(&mut self) -> Surface<'_> {
        let len = (self.w as usize) * (self.h as usize);
        // SAFETY: `bits` points to the `w * h` pixels CreateDIBSection allocated; they live
        // as long as the bitmap, which `self` owns.
        let px = unsafe { std::slice::from_raw_parts_mut(self.bits, len) };
        Surface::new(self.w, self.h, px)
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ::from(self.bitmap));
            let _ = DeleteDC(self.dc);
        }
    }
}

/// One layered window.
struct Overlay {
    hwnd: HWND,
    spec: DecorSpec,
    bounds: Rect,
    dib: Option<Dib>,
}

impl Overlay {
    unsafe fn create(spec: &DecorSpec) -> Option<Overlay> {
        ensure_class();
        let class_name = wide(CLASS_NAME);
        let hinstance = GetModuleHandleW(None).ok()?;
        let b = spec.bounds();
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("").as_ptr()),
            WS_POPUP,
            b.loc.x,
            b.loc.y,
            b.size.w as i32,
            b.size.h as i32,
            None,
            None,
            hinstance,
            None,
        )
        .ok()?;
        Some(Overlay {
            hwnd,
            spec: spec.clone(),
            bounds: Rect::new(b.loc.x, b.loc.y, 0, 0),
            dib: None,
        })
    }

    /// Paint `spec` and hand the bitmap to the compositor at its place.  Returns false when
    /// the bitmap could not be made.
    unsafe fn paint(&mut self, spec: &DecorSpec) -> bool {
        let b = spec.bounds();
        let (w, h) = (b.size.w as i32, b.size.h as i32);
        let reuse = matches!(&self.dib, Some(d) if d.w == w && d.h == h);
        if !reuse {
            self.dib = Dib::new(w, h);
        }
        let Some(dib) = self.dib.as_mut() else {
            return false;
        };
        {
            let mut surf = dib.surface();
            surf.clear();
            spec.render(&mut surf, (b.loc.x, b.loc.y));
        }
        let pos = POINT {
            x: b.loc.x,
            y: b.loc.y,
        };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let updated = UpdateLayeredWindow(
            self.hwnd,
            HDC::default(),
            Some(&pos as *const POINT),
            Some(&size as *const SIZE),
            dib.dc,
            Some(&src as *const POINT),
            COLORREF(0),
            Some(&blend as *const BLENDFUNCTION),
            ULW_ALPHA,
        );
        if updated.is_err() {
            return false;
        }
        PAINTS.fetch_add(1, Ordering::Relaxed);
        self.bounds = b;
        true
    }

    /// Stack the window directly below its anchor (above every ordinary window when it has
    /// none) and make sure it is shown, without activating anything.
    unsafe fn stack(&self, spec: &DecorSpec, show: bool) {
        let mut flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        if show {
            flags |= SWP_SHOWWINDOW;
        }
        let anchor = spec.anchor;
        if anchor != 0 && IsWindow(hwnd_of(anchor)).as_bool() {
            if SetWindowPos(self.hwnd, hwnd_of(anchor), 0, 0, 0, 0, flags).is_ok() {
                return;
            }
        }
        let _ = SetWindowPos(self.hwnd, HWND_TOP, 0, 0, 0, 0, flags);
    }

    unsafe fn move_to(&mut self, x: i32, y: i32) {
        let _ = SetWindowPos(
            self.hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        self.bounds.loc.x = x;
        self.bounds.loc.y = y;
        MOVES.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

struct WorkerState {
    windows: HashMap<DecorId, Overlay>,
    quit: bool,
    verify_at: Option<Instant>,
}

impl WorkerState {
    fn new() -> Self {
        Self {
            windows: HashMap::new(),
            quit: false,
            verify_at: None,
        }
    }

    /// Take every command that is waiting and act on the last set and on the notices after it.
    unsafe fn drain(&mut self, rx: &Receiver<Cmd>) {
        let mut latest: Option<Vec<DecorSpec>> = None;
        let mut moved: Vec<isize> = Vec::new();
        let mut restack: Vec<isize> = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Set(v) => {
                    latest = Some(v);
                    moved.clear();
                }
                Cmd::Moved(h) => {
                    if !moved.contains(&h) {
                        moved.push(h);
                    }
                }
                Cmd::Restack(h) => {
                    if !restack.contains(&h) {
                        restack.push(h);
                    }
                }
                Cmd::Quit => self.quit = true,
            }
        }
        if let Some(set) = latest {
            self.reconcile(set);
        }
        for h in moved {
            self.follow(h);
        }
        for h in restack {
            self.restack(h);
        }
        if self.quit {
            self.windows.clear();
            self.verify_at = None;
        }
    }

    unsafe fn reconcile(&mut self, set: Vec<DecorSpec>) {
        let mut want: HashMap<DecorId, DecorSpec> = HashMap::with_capacity(set.len());
        for spec in set {
            want.insert(spec.id, spec);
        }
        let gone: Vec<DecorId> = self
            .windows
            .keys()
            .filter(|id| !want.contains_key(id))
            .copied()
            .collect();
        for id in gone {
            self.windows.remove(&id);
        }
        let mut any_frame = false;
        for (id, spec) in want {
            if matches!(spec.content, DecorContent::Frame(_)) {
                any_frame = true;
            }
            self.apply(id, spec);
        }
        if any_frame {
            self.verify_at = Some(Instant::now() + VERIFY_DELAY);
        }
    }

    /// Make the window of `id` show `spec`.
    unsafe fn apply(&mut self, id: DecorId, spec: DecorSpec) {
        let b = spec.bounds();
        if b.size.w == 0 || b.size.h == 0 {
            self.windows.remove(&id);
            return;
        }
        if let Some(o) = self.windows.get_mut(&id) {
            if o.spec == spec {
                return;
            }
            let same_size = o.bounds.size == b.size;
            if same_size && o.spec.looks_like(&spec) {
                if o.bounds.loc != b.loc {
                    o.move_to(b.loc.x, b.loc.y);
                }
            } else if !o.paint(&spec) {
                debug!("overlay layer: could not repaint {:?}", id);
                return;
            }
            if o.spec.anchor != spec.anchor {
                o.stack(&spec, false);
            }
            o.spec = spec;
            return;
        }
        let Some(mut o) = Overlay::create(&spec) else {
            warn!("overlay layer: could not create the window of {:?}", id);
            return;
        };
        if !o.paint(&spec) {
            debug!("overlay layer: could not paint {:?}", id);
            return;
        }
        o.stack(&spec, true);
        self.windows.insert(id, o);
    }

    /// The window `hwnd` moved or changed size: the frames around it follow what it really is.
    unsafe fn follow(&mut self, hwnd: isize) {
        let ids: Vec<DecorId> = self
            .windows
            .iter()
            .filter(|(_, o)| {
                o.spec.anchor == hwnd && matches!(o.spec.content, DecorContent::Frame(_))
            })
            .map(|(id, _)| *id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let Some(actual) = frame_bounds(hwnd) else {
            return;
        };
        for id in ids {
            let spec = match self.windows.get(&id) {
                Some(o) => match &o.spec.content {
                    DecorContent::Frame(f) => {
                        let band = f.band.max(0);
                        let tile = Rect::new(
                            actual.loc.x - band,
                            actual.loc.y - band,
                            (actual.size.w as i32 + 2 * band).max(0) as u32,
                            (actual.size.h as i32 + 2 * band).max(0) as u32,
                        );
                        if tile == f.tile {
                            continue;
                        }
                        let mut moved = f.clone();
                        moved.tile = tile;
                        DecorSpec {
                            content: DecorContent::Frame(moved),
                            ..o.spec.clone()
                        }
                    }
                    _ => continue,
                },
                None => continue,
            };
            self.apply(id, spec);
        }
    }

    unsafe fn restack(&mut self, hwnd: isize) {
        for o in self.windows.values() {
            if o.spec.anchor == hwnd {
                o.stack(&o.spec, false);
            }
        }
    }

    /// Once, a little after a change: frames sit where their windows are, and a decoration
    /// whose window is gone is dropped.
    unsafe fn verify(&mut self) {
        self.verify_at = None;
        let dead: Vec<DecorId> = self
            .windows
            .iter()
            .filter(|(_, o)| o.spec.anchor != 0 && !IsWindow(hwnd_of(o.spec.anchor)).as_bool())
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            self.windows.remove(&id);
        }
        let anchors: Vec<isize> = self
            .windows
            .values()
            .filter(|o| matches!(o.spec.content, DecorContent::Frame(_)))
            .map(|o| o.spec.anchor)
            .collect();
        for a in anchors {
            self.follow(a);
        }
    }
}

/// The visible frame of a window (without the invisible resize border), in screen pixels.
unsafe fn frame_bounds(hwnd: isize) -> Option<Rect> {
    let mut r = RECT::default();
    DwmGetWindowAttribute(
        hwnd_of(hwnd),
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut r as *mut RECT as *mut std::ffi::c_void,
        std::mem::size_of::<RECT>() as u32,
    )
    .ok()?;
    if r.right <= r.left || r.bottom <= r.top {
        return None;
    }
    Some(Rect::new(
        r.left,
        r.top,
        (r.right - r.left) as u32,
        (r.bottom - r.top) as u32,
    ))
}

unsafe fn worker_main(rx: Receiver<Cmd>, ready: SyncSender<u32>) {
    // A thread has no message queue until it asks for one; the thread id is only handed out
    // once messages can be posted to it.
    let mut msg = MSG::default();
    let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
    ensure_class();
    let _ = ready.send(GetCurrentThreadId());
    let mut state = WorkerState::new();
    loop {
        let timeout = match state.verify_at {
            Some(at) => at.saturating_duration_since(Instant::now()).as_millis() as u32,
            None => u32::MAX, // INFINITE: nothing to do until a command comes
        };
        let _ = MsgWaitForMultipleObjectsEx(None, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_WORK {
                state.drain(&rx);
            } else {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // A command can arrive between the last message and the wait: look once more.
        state.drain(&rx);
        if state.quit {
            break;
        }
        if let Some(at) = state.verify_at {
            if Instant::now() >= at {
                state.verify();
            }
        }
    }
    state.windows.clear();
}
