//! Native ownership stays on the existing hotkey/message thread. Commands never
//! wait for that thread under the engine lock, and never call back into the engine.
use super::{crop, geometry, Effect, Scene, Snapshot, Tile};
use crate::utils::Rect;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId, GetProcessTimes};
use windows::Win32::UI::WindowsAndMessaging::*;

const WORK: u32 = WM_APP + 0x24b;
const CLASS: PCWSTR = w!("WiriAnimationLayer");
static THREAD: AtomicU32 = AtomicU32::new(0);
static COMMANDS: Mutex<Vec<Command>> = Mutex::new(Vec::new());
// Only active sources live here. Backend show/hide owns final visibility; a show
// during layout must not accidentally remove the compositor's temporary cloak.
static SOURCES: std::sync::LazyLock<Mutex<HashMap<isize, bool>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
pub fn preserve_cloak(hwnd: isize, show: bool) -> bool {
    if let Some(visible) = SOURCES.lock().get_mut(&hwnd) {
        *visible = show;
        return show;
    }
    false
}
thread_local! { static STATE: RefCell<Option<Host>> = const { RefCell::new(None) }; }
enum Command {
    Start(Scene),
    Frame(u64, f64),
    End(u64),
}

pub fn ready() -> bool {
    THREAD.load(Ordering::Acquire) != 0
}
fn send(command: Command) {
    let thread = THREAD.load(Ordering::Acquire);
    if thread == 0 {
        return;
    }
    COMMANDS.lock().push(command);
    unsafe {
        let _ = PostThreadMessageW(thread, WORK, WPARAM(0), LPARAM(0));
    }
}
pub fn attach_message_thread() {
    THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
}
pub fn detach_message_thread() {
    THREAD.store(0, Ordering::Release);
    COMMANDS.lock().clear();
    STATE.with(|s| {
        s.borrow_mut().take();
    });
}
pub fn start(scene: Scene) -> bool {
    if !ready() || scene.tiles.is_empty() {
        return false;
    }
    send(Command::Start(scene));
    true
}
pub fn frame(id: u64, progress: f64) {
    send(Command::Frame(id, progress));
}
pub fn finish(id: u64) {
    send(Command::End(id));
}

pub fn handle_message(message: u32) -> bool {
    if message != WORK {
        return false;
    }
    let commands = std::mem::take(&mut *COMMANDS.lock());
    STATE.with(|state| unsafe {
        let mut state = state.borrow_mut();
        for command in commands {
            match command {
                Command::Start(scene) => {
                    if state.is_none() {
                        *state = Host::new();
                    }
                    if let Some(host) = state.as_mut() {
                        host.add(scene);
                    }
                }
                Command::Frame(id, progress) => {
                    if let Some(host) = state.as_mut() {
                        if let Some(scene) = host.scenes.get_mut(&id) {
                            scene.progress = progress;
                            scene.frames += 1;
                        }
                    }
                }
                Command::End(id) => {
                    if let Some(host) = state.as_mut() {
                        host.scenes.remove(&id);
                    }
                }
            }
        }
        if state.as_ref().is_some_and(|s| s.scenes.is_empty()) {
            state.take();
        }
        if let Some(host) = state.as_mut() {
            host.paint();
        }
    });
    true
}

fn cpu_ms() -> f64 {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return 0.0;
        }
        let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        (ticks(k) + ticks(u)) as f64 / 10_000.0
    }
}
unsafe fn prop(hwnd: HWND, name: PCWSTR, value: usize) {
    let _ = SetPropW(hwnd, name, HANDLE(value as *mut _));
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    width: usize,
    height: usize,
}
impl Canvas {
    unsafe fn new(width: usize, height: usize) -> Option<Self> {
        if width == 0 || height == 0 || width.checked_mul(height)? > 16_777_216 {
            return None;
        }
        let dc = CreateCompatibleDC(HDC::default());
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap =
            match CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
        let old = SelectObject(dc, bitmap);
        Some(Self {
            dc,
            bitmap,
            old,
            bits: bits.cast(),
            width,
            height,
        })
    }
    unsafe fn clear(&mut self) {
        std::slice::from_raw_parts_mut(self.bits, self.width * self.height).fill(0);
    }
    unsafe fn draw(&mut self, image: &Snapshot, dst: Rect, clip: Rect, origin: Rect, alpha: u8) {
        if alpha == 0 {
            return;
        }
        let Some((visible, _)) = crop(dst, clip, image.width, image.height) else {
            return;
        };
        let Some((visible, _)) = crop(visible, origin, visible.size.w, visible.size.h) else {
            return;
        };
        let pixels = std::slice::from_raw_parts_mut(self.bits, self.width * self.height);
        for y in 0..visible.size.h {
            let sy = ((visible.loc.y as i64 + y as i64 - dst.loc.y as i64) * image.height as i64
                / dst.size.h as i64) as usize;
            let ty = (visible.loc.y - origin.loc.y) as usize + y as usize;
            for x in 0..visible.size.w {
                let sx = ((visible.loc.x as i64 + x as i64 - dst.loc.x as i64) * image.width as i64
                    / dst.size.w as i64) as usize;
                let tx = (visible.loc.x - origin.loc.x) as usize + x as usize;
                let src = image.pixels[sy * image.width as usize + sx];
                let target = &mut pixels[ty * self.width + tx];
                let blend = |shift: u32| {
                    (((src >> shift) & 255) * alpha as u32 / 255)
                        + (((*target >> shift) & 255) * (255 - alpha as u32) / 255)
                };
                *target = (blend(24) << 24) | (blend(16) << 16) | (blend(8) << 8) | blend(0);
            }
        }
    }
}
impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

struct NativeTile {
    spec: Tile,
    thumbnail: Option<isize>,
    source_size: SIZE,
    cloaked: bool,
    pid: u32,
}
impl NativeTile {
    unsafe fn release(&mut self) {
        if self.cloaked {
            let visible = SOURCES
                .lock()
                .remove(&self.spec.source)
                .unwrap_or(!self.spec.outgoing);
            // HWND reuse must not uncloak another application. An outgoing
            // workspace transfers its cloak to the normal hide-method backend.
            let mut pid = 0;
            GetWindowThreadProcessId(HWND(self.spec.source as *mut _), Some(&mut pid));
            if pid == self.pid && pid != 0 {
                if visible || !crate::backend::cloak::wanted() {
                    crate::backend::cloak::uncloak(self.spec.source);
                }
                if visible {
                    crate::backend::hidden::note_hidden(self.spec.source, false, true);
                }
            }
            self.cloaked = false;
        }
        if let Some(handle) = self.thumbnail.take() {
            let _ = DwmUnregisterThumbnail(handle);
        }
    }
}
impl Drop for NativeTile {
    fn drop(&mut self) {
        unsafe {
            self.release();
        }
    }
}
struct NativeScene {
    effect: Effect,
    tiles: Vec<NativeTile>,
    progress: f64,
    frames: u32,
    since: Instant,
    cpu: f64,
    id: u64,
}
impl Drop for NativeScene {
    fn drop(&mut self) {
        tracing::info!("compositor transition id={} effect={:?} frames={} lifetime_ms={:.3} cpu_ms={:.3} thumbnails={}",
            self.id, self.effect, self.frames, self.since.elapsed().as_secs_f64()*1000.0, cpu_ms()-self.cpu,
            self.tiles.iter().filter(|t| t.thumbnail.is_some()).count());
    }
}
struct Host {
    hwnd: HWND,
    bounds: Rect,
    canvas: Canvas,
    scenes: BTreeMap<u64, NativeScene>,
}
impl Host {
    unsafe fn new() -> Option<Self> {
        static CLASS_INIT: std::sync::Once = std::sync::Once::new();
        let instance = GetModuleHandleW(None).ok()?;
        CLASS_INIT.call_once(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        let bounds = Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32,
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32,
        );
        let canvas = Canvas::new(bounds.size.w as usize, bounds.size.h as usize)?;
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            CLASS,
            w!("wiri animation"),
            WS_POPUP,
            bounds.loc.x,
            bounds.loc.y,
            bounds.size.w as i32,
            bounds.size.h as i32,
            None,
            None,
            instance,
            None,
        )
        .ok()?;
        Some(Self {
            hwnd,
            bounds,
            canvas,
            scenes: BTreeMap::new(),
        })
    }
    unsafe fn add(&mut self, scene: Scene) {
        let mut tiles = Vec::new();
        for mut spec in scene.tiles {
            let mut thumbnail = None;
            let mut source_size = SIZE::default();
            let mut cloaked = false;
            let mut pid = 0;
            if scene.effect != Effect::Close {
                // A successful registration alone says nothing about protected/empty pixels.
                let probe = super::snapshot::capture(spec.source);
                if probe.is_none() {
                    continue;
                }
                if let Ok(handle) = DwmRegisterThumbnail(self.hwnd, HWND(spec.source as *mut _)) {
                    if let Ok(size) = DwmQueryThumbnailSourceSize(handle) {
                        if size.cx > 0 && size.cy > 0 {
                            source_size = size;
                            thumbnail = Some(handle);
                        }
                    }
                    if thumbnail.is_none() {
                        let _ = DwmUnregisterThumbnail(handle);
                    }
                }
                if thumbnail.is_none() {
                    continue;
                }
                GetWindowThreadProcessId(HWND(spec.source as *mut _), Some(&mut pid));
                // Register and validate before hiding a source. Never SW_HIDE a live source:
                // DWM needs its redirection surface while the thumbnail is visible.
                crate::backend::hidden::note_hidden(spec.source, true, true);
                cloaked = crate::backend::cloak::cloak(spec.source);
                if !cloaked {
                    crate::backend::hidden::note_hidden(spec.source, false, true);
                    if let Some(handle) = thumbnail {
                        let _ = DwmUnregisterThumbnail(handle);
                    }
                    continue;
                }
                SOURCES.lock().insert(spec.source, !spec.outgoing);
                // Retain one validated frame as a safety image for cloak/late repaint.
                if spec.snapshot.is_none() {
                    spec.snapshot = probe;
                }
            } else if spec.snapshot.is_none() {
                continue;
            }
            tiles.push(NativeTile {
                spec,
                thumbnail,
                source_size,
                cloaked,
                pid,
            });
        }
        if !tiles.is_empty() {
            self.scenes.insert(
                scene.id,
                NativeScene {
                    effect: scene.effect,
                    tiles,
                    progress: 0.0,
                    frames: 0,
                    since: Instant::now(),
                    cpu: cpu_ms(),
                    id: scene.id,
                },
            );
        }
    }
    unsafe fn paint(&mut self) {
        self.canvas.clear();
        let mut registrations = 0;
        let mut last_effect = 0;
        let mut first_source = 0;
        let mut frames = 0;
        for scene in self.scenes.values_mut() {
            last_effect = scene.effect as usize;
            frames += scene.frames as usize;
            for tile in &mut scene.tiles {
                if first_source == 0 {
                    first_source = tile.spec.source as usize;
                }
                let (from, to) = if scene.effect == Effect::Open {
                    let r = super::snapshot::frame(tile.spec.source).unwrap_or(tile.spec.to);
                    (r, r)
                } else {
                    (tile.spec.from, tile.spec.to)
                };
                let (dst, alpha) = geometry(scene.effect, from, to, scene.progress);
                if let Some(image) = &tile.spec.snapshot {
                    let opacity = if scene.effect == Effect::Resize {
                        255u8.saturating_sub(alpha)
                    } else {
                        alpha
                    };
                    self.canvas
                        .draw(image, dst, tile.spec.clip, self.bounds, opacity);
                }
                if let Some(handle) = tile.thumbnail {
                    let mut props = DWM_THUMBNAIL_PROPERTIES {
                        dwFlags: DWM_TNP_VISIBLE,
                        ..Default::default()
                    };
                    if let Some((dst, src)) = crop(
                        dst,
                        tile.spec.clip,
                        tile.source_size.cx as u32,
                        tile.source_size.cy as u32,
                    ) {
                        props.dwFlags |= DWM_TNP_RECTDESTINATION
                            | DWM_TNP_RECTSOURCE
                            | DWM_TNP_OPACITY
                            | DWM_TNP_SOURCECLIENTAREAONLY;
                        props.rcDestination = RECT {
                            left: dst.loc.x - self.bounds.loc.x,
                            top: dst.loc.y - self.bounds.loc.y,
                            right: dst.right() - self.bounds.loc.x,
                            bottom: dst.bottom() - self.bounds.loc.y,
                        };
                        props.rcSource = RECT {
                            left: src.loc.x,
                            top: src.loc.y,
                            right: src.right(),
                            bottom: src.bottom(),
                        };
                        props.fVisible = BOOL(1);
                        props.opacity = alpha;
                        props.fSourceClientAreaOnly = BOOL(0);
                    }
                    if DwmUpdateThumbnailProperties(handle, &props).is_err() {
                        // Failure mid-flight releases this source immediately; no black layer.
                        tile.release();
                        tile.spec.snapshot = None;
                    } else {
                        registrations += 1;
                    }
                }
            }
        }
        let pos = POINT {
            x: self.bounds.loc.x,
            y: self.bounds.loc.y,
        };
        let size = SIZE {
            cx: self.bounds.size.w as i32,
            cy: self.bounds.size.h as i32,
        };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
            ..Default::default()
        };
        let _ = UpdateLayeredWindow(
            self.hwnd,
            HDC::default(),
            Some(&pos),
            Some(&size),
            self.canvas.dc,
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        prop(self.hwnd, w!("WiriAnimationKind"), last_effect);
        prop(self.hwnd, w!("WiriAnimationSource"), first_source);
        prop(self.hwnd, w!("WiriAnimationFrames"), frames);
        prop(self.hwnd, w!("WiriAnimationThumbnailCount"), registrations);
        let _ = SetWindowPos(
            self.hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            // Uncloak and unregister in the same pump turn before destroying the layer.
            // DWM presents this batch; there is no foreign-HWND atomic-present API.
            self.scenes.clear();
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
