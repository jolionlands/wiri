//! Event-driven pointer focus on the existing input/message thread.
//!
//! The low-level hook only publishes coordinates and posts a coalesced message.
//! Hit testing, engine locks, activation and the optional foreground hook belong
//! to the message thread. There is no polling thread or idle timer.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorInfo, GetCursorPos, GetForegroundWindow, GetWindowRect, IsIconic,
    IsWindow, IsWindowVisible, KillTimer, PostThreadMessageW, SetCursorPos, SetTimer,
    WindowFromPoint, CURSORINFO, CURSOR_SHOWING, GA_ROOT, MSG, WM_TIMER, WM_USER,
};

use crate::backend::{cloak, BackendHandle};
use crate::config::{FocusSubConfig, WarpMode};
use crate::layout::{TilingEngine, WorkspaceLayout};
use crate::utils::{OutputId, Point, Rect, WindowId};

const WM_POINTER_FOCUS: u32 = WM_USER + 310;
const WM_FOREGROUND_WARP: u32 = WM_USER + 311;
const WM_RELOAD_FOCUS: u32 = WM_USER + 312;
const FOCUS_TIMER_ID: usize = (WM_USER + 313) as usize;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_OBJECT_FOCUS: u32 = 0x8005;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const WINEVENT_SKIPOWNPROCESS: u32 = 2;

static OWNER_THREAD: AtomicU32 = AtomicU32::new(0);
static FOLLOWS_MOUSE: AtomicBool = AtomicBool::new(false);
static MOTION_PENDING: AtomicBool = AtomicBool::new(false);
static MOTION_POINT: AtomicU64 = AtomicU64::new(0);
// Zero means no outstanding post; otherwise the value identifies its owner.
static FOREGROUND_PENDING: AtomicU64 = AtomicU64::new(0);
static FOREGROUND_GENERATION: AtomicU64 = AtomicU64::new(0);
static FOREGROUND_HWND: AtomicIsize = AtomicIsize::new(0);
static WARP_PENDING: AtomicBool = AtomicBool::new(false);
static WARP_POINT: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static STATE: RefCell<Option<FocusState>> = const { RefCell::new(None) };
}

fn pack(point: Point) -> u64 {
    (u64::from(point.x as u32) << 32) | u64::from(point.y as u32)
}

fn unpack(value: u64) -> Point {
    Point {
        x: (value >> 32) as i32,
        y: value as i32,
    }
}

fn post(message: u32) -> bool {
    #[cfg(test)]
    if let Some(result) = activation_test_support::record_post(message) {
        return result;
    }
    let thread = OWNER_THREAD.load(Ordering::Acquire);
    if thread == 0 {
        return false;
    }
    // SAFETY: no pointers are sent; the owner has already created its message queue.
    unsafe { PostThreadMessageW(thread, message, WPARAM(0), LPARAM(0)).is_ok() }
}

/// A native activation may finish after its foreground event was delivered,
/// without another focus/location event when the target's geometry is unchanged.
/// Revalidate only that observed request; the input thread retains suppression,
/// completed-warp deduplication and all foreground/geometry checks.
pub(crate) fn activation_completed(window: isize) {
    if window == 0 || FOREGROUND_HWND.load(Ordering::Acquire) != window {
        return;
    }
    request_foreground_revalidation();
}

fn request_foreground_revalidation() {
    let mut generation = FOREGROUND_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    // One failed post may have hidden a newer coalesced publication. Recover
    // that publication once, without spinning when a queue stays unavailable.
    for _ in 0..2 {
        if FOREGROUND_PENDING
            .compare_exchange(0, generation, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        if post(WM_FOREGROUND_WARP) {
            return;
        }
        // A delivered message may already have released our claim and a newer
        // publisher acquired its own. Never clear that publisher's wake.
        if FOREGROUND_PENDING
            .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let latest = FOREGROUND_GENERATION.load(Ordering::Acquire);
        if latest == generation {
            return;
        }
        generation = latest;
    }
}

#[cfg(test)]
pub(crate) mod activation_test_support {
    use super::*;

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    thread_local! {
        static POSTS: RefCell<Option<Vec<isize>>> = const { RefCell::new(None) };
        static FAIL_WITH_FOREGROUND: RefCell<Option<(isize, bool)>> = const { RefCell::new(None) };
    }

    pub(crate) fn fail_next_post_with_foreground(window: isize) {
        FAIL_WITH_FOREGROUND.with(|next| *next.borrow_mut() = Some((window, false)));
    }

    pub(crate) fn fail_next_post_after_delivery_with_foreground(window: isize) {
        FAIL_WITH_FOREGROUND.with(|next| *next.borrow_mut() = Some((window, true)));
    }

    pub(crate) fn foreground_wake_pending() -> bool {
        FOREGROUND_PENDING.load(Ordering::Acquire) != 0
    }

    pub(super) fn record_post(message: u32) -> Option<bool> {
        if POSTS.with(|posts| posts.borrow().is_none()) {
            return None;
        }
        let interleaved = FAIL_WITH_FOREGROUND.with(|next| next.borrow_mut().take());
        if let Some((window, delivered)) = interleaved {
            if delivered {
                // Simulate the owner consuming an older queued notification
                // before a new publisher acquires its own posting claim.
                FOREGROUND_PENDING.store(0, Ordering::Release);
            }
            if window != 0 {
                // SYSTEM_FOREGROUND only publishes its HWND; unlike object
                // events, this callback branch makes no native queries.
                unsafe {
                    foreground_event(
                        HWINEVENTHOOK::default(),
                        EVENT_SYSTEM_FOREGROUND,
                        HWND(window as *mut std::ffi::c_void),
                        0,
                        0,
                        0,
                        0,
                    );
                }
            }
            return Some(false);
        }
        POSTS.with(|posts| {
            let mut posts = posts.borrow_mut();
            let posts = posts.as_mut()?;
            if message == WM_FOREGROUND_WARP {
                posts.push(FOREGROUND_HWND.load(Ordering::Acquire));
            }
            Some(true)
        })
    }

    /// The early foreground message has already been delivered while native
    /// activation is unfinished. Capture later wakes without touching Win32.
    pub(crate) fn after_early_foreground<T>(
        window: isize,
        finish: impl FnOnce() -> T,
    ) -> (T, Vec<isize>) {
        let _serial = SERIAL.lock().unwrap();
        let saved_window = FOREGROUND_HWND.swap(window, Ordering::AcqRel);
        let saved_pending = FOREGROUND_PENDING.swap(0, Ordering::AcqRel);
        let saved_generation = FOREGROUND_GENERATION.swap(0, Ordering::AcqRel);
        POSTS.with(|posts| *posts.borrow_mut() = Some(Vec::new()));
        FAIL_WITH_FOREGROUND.with(|next| *next.borrow_mut() = None);
        let result = finish();
        let posts = POSTS.with(|posts| posts.borrow_mut().take().unwrap());
        FAIL_WITH_FOREGROUND.with(|next| *next.borrow_mut() = None);
        FOREGROUND_HWND.store(saved_window, Ordering::Release);
        FOREGROUND_PENDING.store(saved_pending, Ordering::Release);
        FOREGROUND_GENERATION.store(saved_generation, Ordering::Release);
        (result, posts)
    }
}

/// Called by WH_MOUSE_LL, including while its grab-state mutex is held.
/// Must never lock the engine, acquire that mutex, or call into the layout.
pub fn on_mouse_move(point: Point) {
    let packed = pack(point);
    if WARP_PENDING.swap(false, Ordering::AcqRel) && WARP_POINT.load(Ordering::Acquire) == packed {
        return;
    }
    if !FOLLOWS_MOUSE.load(Ordering::Acquire) {
        return;
    }
    MOTION_POINT.store(packed, Ordering::Release);
    if !MOTION_PENDING.swap(true, Ordering::AcqRel) && !post(WM_POINTER_FOCUS) {
        MOTION_PENDING.store(false, Ordering::Release);
    }
}

/// Initialize on the input thread, after creation of its hidden hotkey HWND.
pub fn initialize(engine: Arc<RwLock<TilingEngine>>, backend: BackendHandle, hwnd: HWND) {
    shutdown();
    // SAFETY: querying the calling thread ID has no preconditions.
    OWNER_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let mut state = FocusState {
        engine,
        backend,
        hwnd,
        policy: FocusSubConfig::default(),
        dwell: Dwell::default(),
        timer_armed: false,
        foreground_hook: None,
        geometry_hook: None,
        foreground: ForegroundState::default(),
    };
    state.reload();
    STATE.with(|slot| *slot.borrow_mut() = Some(state));
}

/// Safe to call from the config reload thread: policy changes are serialized
/// with mouse, foreground and timer messages on the input thread.
pub fn reload() {
    let _ = post(WM_RELOAD_FOCUS);
}

/// Called before the owning message loop destroys the hidden HWND.
pub fn shutdown() {
    FOLLOWS_MOUSE.store(false, Ordering::Release);
    OWNER_THREAD.store(0, Ordering::Release);
    MOTION_PENDING.store(false, Ordering::Release);
    FOREGROUND_PENDING.store(0, Ordering::Release);
    WARP_PENDING.store(false, Ordering::Release);
    STATE.with(|slot| {
        if let Some(mut state) = slot.borrow_mut().take() {
            state.cancel();
            state.remove_foreground_hook();
        }
    });
}

/// Consume our thread messages and only our own HWND/timer combination.
/// Must run before TranslateMessage/DispatchMessage on the owning thread.
pub fn handle_message(message: &MSG) -> bool {
    STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return false;
        };
        match message.message {
            WM_POINTER_FOCUS if message.hwnd.is_invalid() => {
                if MOTION_PENDING.swap(false, Ordering::AcqRel) {
                    let point = unpack(MOTION_POINT.load(Ordering::Acquire));
                    state.motion(point);
                }
                true
            }
            WM_FOREGROUND_WARP if message.hwnd.is_invalid() => {
                if FOREGROUND_PENDING.swap(0, Ordering::AcqRel) != 0 {
                    state.foreground(FOREGROUND_HWND.load(Ordering::Acquire));
                }
                true
            }
            WM_RELOAD_FOCUS if message.hwnd.is_invalid() => {
                state.reload();
                true
            }
            WM_TIMER if message.hwnd == state.hwnd && message.wParam.0 == FOCUS_TIMER_ID => {
                state.expired();
                true
            }
            _ => false,
        }
    })
}

struct FocusState {
    engine: Arc<RwLock<TilingEngine>>,
    backend: BackendHandle,
    hwnd: HWND,
    policy: FocusSubConfig,
    dwell: Dwell,
    timer_armed: bool,
    foreground_hook: Option<HWINEVENTHOOK>,
    geometry_hook: Option<HWINEVENTHOOK>,
    foreground: ForegroundState,
}

/// Foreground events may precede both actual Win32 activation and asynchronous
/// SetWindowPos completion. Remember the request until focus/location confirms
/// it, rather than deduplicating an unfinished warp as if it had succeeded.
#[derive(Default)]
struct ForegroundState {
    last: isize,
    pending: Option<isize>,
}

impl ForegroundState {
    fn observe(&mut self, window: isize, warp: bool) -> bool {
        if window == 0 || window == self.last {
            return false;
        }
        self.last = window;
        self.pending = warp.then_some(window);
        true
    }

    fn suppress(&mut self, window: isize) {
        self.last = window;
        self.pending = None;
    }
}

impl FocusState {
    fn reload(&mut self) {
        self.cancel();
        self.policy = self
            .engine
            .read()
            .full_config()
            .as_ref()
            .map(|config| config.input.focus())
            .unwrap_or_default();
        self.policy.focus_delay_ms = self.policy.focus_delay_ms.min(60_000);
        info!(
            follows_mouse = self.policy.follows_mouse,
            delay_ms = self.policy.focus_delay_ms,
            warp = self.policy.warp_on_focus,
            "Pointer focus policy updated"
        );
        FOLLOWS_MOUSE.store(self.policy.follows_mouse, Ordering::Release);
        MOTION_PENDING.store(false, Ordering::Release);
        WARP_PENDING.store(false, Ordering::Release);
        let observe_foreground = self.policy.warp_on_focus || self.policy.follows_mouse;
        if observe_foreground && self.foreground_hook.is_none() {
            // SAFETY: out-of-context callbacks are delivered on this pumping
            // thread. The callback only publishes an HWND, never borrowed state.
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    HMODULE::default(),
                    Some(foreground_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                warn!("Unable to install pointer-warp foreground hook");
            } else {
                self.foreground_hook = Some(hook);
            }
        } else if !observe_foreground {
            self.remove_foreground_hook();
        }
        if self.policy.warp_on_focus && self.geometry_hook.is_none() {
            // Focus and location are completion notifications, not a frame clock.
            // Intermediate events in this small range are ignored by the callback.
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_OBJECT_FOCUS,
                    EVENT_OBJECT_LOCATIONCHANGE,
                    HMODULE::default(),
                    Some(foreground_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                warn!("Unable to install pointer-warp completion hook");
            } else {
                self.geometry_hook = Some(hook);
            }
        } else if !self.policy.warp_on_focus {
            self.remove_geometry_hook();
        }
        // Reload/startup itself is not a foreground change and must not warp.
        // SAFETY: querying the current foreground window has no preconditions.
        self.foreground
            .suppress(unsafe { GetForegroundWindow() }.0 as isize);
    }

    fn remove_geometry_hook(&mut self) {
        if let Some(hook) = self.geometry_hook.take() {
            // SAFETY: this thread owns the successfully installed hook.
            let _ = unsafe { UnhookWinEvent(hook) };
        }
    }

    fn remove_foreground_hook(&mut self) {
        self.remove_geometry_hook();
        if let Some(hook) = self.foreground_hook.take() {
            // SAFETY: this thread owns the successfully installed hook.
            let _ = unsafe { UnhookWinEvent(hook) };
        }
        FOREGROUND_PENDING.store(0, Ordering::Release);
    }

    fn stop_timer(&mut self) {
        if self.timer_armed {
            // SAFETY: only this module uses this timer ID on the live owner HWND.
            let _ = unsafe { KillTimer(self.hwnd, FOCUS_TIMER_ID) };
            self.timer_armed = false;
        }
    }

    fn cancel(&mut self) {
        self.stop_timer();
        self.dwell.invalidate();
    }

    fn motion(&mut self, point: Point) {
        if !self.policy.follows_mouse || super::low_level_hook::is_grab_active() {
            self.cancel();
            return;
        }
        let (target, output_change) = {
            let engine = self.engine.read();
            let output_change = pointer_output(&engine, point)
                .filter(|output| Some(*output) != engine.focused_output());
            (target_at(&engine, point), output_change)
        };
        // Output focus follows entry even over an empty workspace or an
        // unmanaged window. It must not activate the output's selected window
        // through an overlay, nor depend on finding a managed tile under it.
        if let Some(output) = output_change {
            let mut engine = self.engine.write();
            if engine.monitors().get(&output).is_some() {
                engine.set_focused_output(output);
                crate::ipc::notify_state_changed();
            }
        }
        let previous = self.dwell.pending.map(|pending| pending.target);
        let decision = self
            .dwell
            .observe(target, Instant::now(), self.policy.focus_delay_ms);
        debug!(?point, ?target, ?decision, "Pointer focus motion");
        // A move within the same target must NOT restart the one-shot timer.
        if previous != self.dwell.pending.map(|pending| pending.target) {
            self.stop_timer();
        }
        self.perform(decision, point);
    }

    fn perform(&mut self, decision: DwellDecision, point: Point) {
        match decision {
            DwellDecision::Idle => self.stop_timer(),
            DwellDecision::Wait(remaining) => {
                if !self.timer_armed {
                    let ms = remaining.as_millis().clamp(1, 60_000) as u32;
                    // SAFETY: the owner HWND is live and this ID is module-local.
                    self.timer_armed =
                        unsafe { SetTimer(self.hwnd, FOCUS_TIMER_ID, ms, None) } != 0;
                    if !self.timer_armed {
                        self.cancel();
                        warn!("Unable to arm focus-follows-mouse delay timer");
                    }
                }
            }
            DwellDecision::Focus(target) => {
                self.stop_timer();
                self.focus(target, point);
            }
        }
    }

    fn expired(&mut self) {
        // Win32 timers repeat by default; remove it BEFORE processing expiry.
        self.stop_timer();
        let Some(pending) = self.dwell.pending else {
            return;
        };
        if !self.policy.follows_mouse || super::low_level_hook::is_grab_active() {
            self.cancel();
            return;
        }
        let Some(point) = cursor_position() else {
            self.cancel();
            return;
        };
        let target = target_at(&self.engine.read(), point);
        if target != Some(pending.target) {
            self.cancel();
            return;
        }
        let decision = self
            .dwell
            .observe(target, Instant::now(), self.policy.focus_delay_ms);
        self.perform(decision, point);
    }

    fn focus(&mut self, target: FocusTarget, point: Point) {
        // Zero-delay motion uses the hook's logical point: WH_MOUSE_LL posts
        // BEFORE Windows updates GetCursorPos, so querying it here can see the
        // previous location and permanently settle a target without focusing.
        // Timer expiry instead supplies its freshly queried native point above.
        // Revalidate window/workspace membership at that point in either case.
        if super::low_level_hook::is_grab_active() {
            self.cancel();
            return;
        }
        let mut engine = self.engine.write();
        if target_at(&engine, point) != Some(target) {
            // Discard settlement too, so a later motion within a still-valid
            // target can retry after windows/output membership changed.
            self.dwell.invalidate();
            return;
        }
        // SAFETY: these are value-only HWND queries; no application memory is read.
        let foreground = unsafe { GetForegroundWindow() };
        let owner = root_owner(foreground);
        if engine.managed_window_for(foreground.0 as isize, owner.0 as isize) == Some(target.window)
        {
            return;
        }
        if target.column.is_none() {
            engine.set_focused_output(target.output);
            self.foreground.suppress(target.window.as_isize());
            // focus_float can switch the float's home workspace. Hover must not
            // switch workspaces: visible sticky floats may live elsewhere, and
            // workspace IDs alone do not identify an output. Let the existing
            // foreground event update float stacking after direct activation.
            if let Err(error) = self.backend.activate_window(target.window.as_isize()) {
                warn!("Pointer float activation failed: {error}");
            }
            return;
        }
        let Some(scroll) = reveal_scroll(&engine, target, self.policy.max_scroll_percent) else {
            debug!(?target, "Pointer focus reveal exceeds configured limit");
            return;
        };
        debug!(?target, scroll, "Pointer focus activation");
        let already_selected = engine.focused_output() == Some(target.output)
            && engine
                .monitors()
                .get(&target.output)
                .is_some_and(|monitor| monitor.focus_window == Some(target.window));
        if let Some(monitor) = engine.monitors_mut().get_mut(&target.output) {
            monitor.focus_window = Some(target.window);
            monitor.focus_column = target.column;
            monitor.focus_ring.push(target.window);
            if let Some(workspace) = monitor.workspace_mut() {
                workspace.scroll_offset.x = scroll;
            }
        }
        engine.set_focused_output(target.output);
        // Pointer-induced focus must never warp, even in center-xy-always mode.
        self.foreground.suppress(target.window.as_isize());
        // One layout/activation attempt on target transition, never per motion.
        // If an unmanaged foreground left the internal selection unchanged,
        // apply_layout's activation dedup deliberately does not reactivate it.
        if already_selected {
            if let Err(error) = self.backend.activate_window(target.window.as_isize()) {
                warn!("Pointer focus activation failed: {error}");
            }
        }
        engine.apply_layout_for_monitor(target.output, &self.backend);
        crate::ipc::notify_state_changed();
    }

    fn foreground(&mut self, event_hwnd: isize) {
        debug!(
            event_hwnd,
            last = self.foreground.last,
            "Pointer foreground notification"
        );
        if self
            .foreground
            .observe(event_hwnd, self.policy.warp_on_focus)
        {
            // Keyboard/IPC focus changes cancel hover even with warp disabled.
            self.cancel();
        }
        if super::low_level_hook::is_grab_active() {
            self.cancel();
            self.foreground.pending = None;
            return;
        }
        if self.foreground.pending != Some(event_hwnd) {
            return;
        }
        // A queued foreground event can precede actual activation. Keep the
        // pending request; native activation completion and object-focus/location
        // notifications revalidate it.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0 as isize != event_hwnd {
            return;
        }
        let root = unsafe { GetAncestor(foreground, GA_ROOT) };
        let owner = root_owner(foreground);
        let expected = {
            let engine = self.engine.read();
            if engine
                .managed_window_for(root.0 as isize, owner.0 as isize)
                .is_none()
            {
                self.foreground.pending = None;
                return;
            }
            if engine.is_overview() {
                self.foreground.pending = None;
                return;
            }
            // Owned dialogs have their own geometry, not the owner's tile.
            engine.pointer_warp_target(WindowId::new(root.0 as isize))
        };
        if !visible_window(root) {
            return;
        }
        let Some(rect) = self
            .backend
            .get_window(root.0 as isize)
            .and_then(|info| info.get_dwm_bounds())
            .or_else(|| window_rect(root))
        else {
            return;
        };
        // Native positioning is asynchronous. Never warp to a parking rectangle
        // or to old on-screen geometry while a layout move is still in flight.
        let ready = expected.is_none_or(|expected| warp_geometry_ready(rect, expected))
            && self
                .engine
                .read()
                .monitors()
                .values()
                .any(|monitor| monitor.bounds.contains_point(rect.center()));
        if !ready {
            return;
        }
        self.foreground.pending = None;
        let mut cursor = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: cursor is correctly sized, writable stack storage.
        if unsafe { GetCursorInfo(&mut cursor) }.is_err() || cursor.flags.0 & CURSOR_SHOWING.0 == 0
        {
            // In particular, never call ShowCursor to reveal a hidden pointer.
            debug!(
                flags = cursor.flags.0,
                "Pointer warp skipped for hidden or unavailable cursor"
            );
            return;
        }
        let point = Point {
            x: cursor.ptScreenPos.x,
            y: cursor.ptScreenPos.y,
        };
        let Some(destination) = warp_destination(rect, point, self.policy.warp_mode) else {
            return;
        };
        debug!(?point, ?destination, "Warping pointer to foreground window");
        WARP_POINT.store(pack(destination), Ordering::Release);
        WARP_PENDING.store(true, Ordering::Release);
        // SAFETY: destination is a validated screen-space point inside the window.
        if unsafe { SetCursorPos(destination.x, destination.y) }.is_err() {
            WARP_PENDING.store(false, Ordering::Release);
        }
    }
}

unsafe extern "system" fn foreground_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _event_thread: u32,
    _time: u32,
) {
    if hwnd.is_invalid() {
        return;
    }
    if event == EVENT_SYSTEM_FOREGROUND {
        FOREGROUND_HWND.store(hwnd.0 as isize, Ordering::Release);
    } else if event == EVENT_OBJECT_FOCUS
        || (event == EVENT_OBJECT_LOCATIONCHANGE && _object == 0 && _child == 0)
    {
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        if root.0 as isize != FOREGROUND_HWND.load(Ordering::Acquire) {
            return;
        }
    } else {
        return;
    }
    request_foreground_revalidation();
}

fn cursor_position() -> Option<Point> {
    let mut point = POINT::default();
    // SAFETY: point is writable stack storage of the required size.
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some(Point {
        x: point.x,
        y: point.y,
    })
}

fn root_owner(hwnd: HWND) -> HWND {
    // Follow GW_OWNER too: some owned dialogs are not WS_POPUP windows.
    HWND(crate::layout::engine::root_owner_of(hwnd.0 as isize) as *mut std::ffi::c_void)
}

fn visible_window(hwnd: HWND) -> bool {
    // SAFETY: these APIs validate HWND values and do not dereference Rust memory.
    (unsafe {
        IsWindow(hwnd).as_bool() && IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool()
    }) && !cloak::is_cloaked(hwnd.0 as isize)
}

fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: rect is writable stack storage of the required size.
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(Rect::new(rect.left, rect.top, width as u32, height as u32))
}

fn target_at(engine: &TilingEngine, point: Point) -> Option<FocusTarget> {
    // SAFETY: screen-space hit testing returns HWND values, not borrowed memory.
    let hit = unsafe {
        WindowFromPoint(POINT {
            x: point.x,
            y: point.y,
        })
    };
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    if !visible_window(root) {
        return None;
    }
    // SAFETY: root is the current hit-tested top-level HWND.
    let owner = root_owner(root);
    resolve_target(engine, point, [root.0 as isize, owner.0 as isize])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FocusTarget {
    window: WindowId,
    output: OutputId,
    workspace: i32,
    column: Option<usize>,
}

fn pointer_output(engine: &TilingEngine, point: Point) -> Option<OutputId> {
    if engine.is_overview() {
        return None;
    }
    engine
        .monitors()
        .iter()
        .find(|(_, monitor)| monitor.bounds.contains_point(point))
        .map(|(output, _)| *output)
}

/// Pure half of hit testing: root/owned-dialog normalization, output membership
/// and active-workspace checks. Never fall through an unmanaged overlay to a tile.
fn resolve_target(engine: &TilingEngine, point: Point, roots: [isize; 2]) -> Option<FocusTarget> {
    let output = pointer_output(engine, point)?;
    let monitor = engine.monitors().get(&output)?;
    for root in roots {
        if root == 0 {
            continue;
        }
        let window = WindowId::new(root);
        if let Some((home, workspace, column)) = engine.locate_window(window) {
            if home == output && workspace == monitor.active_workspace {
                return Some(FocusTarget {
                    window,
                    output: home,
                    workspace,
                    column: Some(column),
                });
            }
        } else if engine.is_floating(window)
            && (engine.is_sticky(window)
                || engine.float_workspace(window) == Some(monitor.active_workspace))
        {
            return Some(FocusTarget {
                window,
                output,
                workspace: monitor.active_workspace,
                column: None,
            });
        }
    }
    None
}

/// Calculate the minimal reveal without touching the live workspace. The limit
/// is relative to the output WORKING AREA, not the gap-adjusted viewport.
fn reveal_scroll(engine: &TilingEngine, target: FocusTarget, percent: Option<f64>) -> Option<i32> {
    let monitor = engine.monitors().get(&target.output)?;
    let workspace = monitor.workspaces.get(&target.workspace)?;
    let before = workspace.scroll_offset.x;
    if workspace.layout_mode != WorkspaceLayout::Scrolling {
        return Some(before);
    }
    let config = engine.effective_config(target.output);
    let view_width = monitor.work_area.size.w as i32
        - config.outer_gaps.1
        - config.outer_gaps.3
        - config.reserve_left as i32
        - config.reserve_right as i32;
    if view_width <= 0 {
        return None;
    }
    let explicit: Vec<_> = workspace
        .columns
        .iter()
        .map(|column| {
            if column.maximized {
                Some(view_width as u32)
            } else {
                column.width
            }
        })
        .collect();
    let mins: Vec<_> = workspace
        .columns
        .iter()
        .map(|column| column.min_width())
        .collect();
    let maxs: Vec<_> = workspace
        .columns
        .iter()
        .map(|column| column.max_width())
        .collect();
    let (widths, default_width) = TilingEngine::column_widths_with_limits(
        &explicit,
        &mins,
        &maxs,
        view_width,
        config.column_gap,
        config,
    );
    let mut candidate = workspace.clone();
    for (column, width) in candidate.columns.iter_mut().zip(widths) {
        column.width = Some(width.max(1) as u32);
    }
    candidate.reveal_column(target.column?, default_width, config.column_gap, view_width);
    candidate.fit_scroll(default_width, config.column_gap, view_width);
    let after = candidate.scroll_offset.x;
    scroll_allowed(before, after, monitor.work_area.size.w, percent).then_some(after)
}

fn scroll_allowed(before: i32, after: i32, work_width: u32, percent: Option<f64>) -> bool {
    let amount = (i64::from(after) - i64::from(before)).unsigned_abs() as f64;
    percent.is_none_or(|percent| {
        percent.is_finite() && percent >= 0.0 && amount <= f64::from(work_width) * percent / 100.0
    })
}

fn warp_geometry_ready(actual: Rect, expected: Rect) -> bool {
    // DWM frame coordinates allow rounding, not a stale on-screen/parked move.
    // App minimum sizes may differ from the request: compare position, not size.
    actual.size.w > 0
        && actual.size.h > 0
        && (i64::from(actual.loc.x) - i64::from(expected.loc.x)).abs() <= 2
        && (i64::from(actual.loc.y) - i64::from(expected.loc.y)).abs() <= 2
}

fn warp_destination(rect: Rect, point: Point, mode: WarpMode) -> Option<Point> {
    if rect.size.w == 0 || rect.size.h == 0 {
        return None;
    }
    let inside_x = point.x >= rect.loc.x && point.x < rect.right();
    let inside_y = point.y >= rect.loc.y && point.y < rect.bottom();
    let center = rect.center();
    let destination = match mode {
        WarpMode::CenterOutsideAxes => Point {
            x: if inside_x { point.x } else { center.x },
            y: if inside_y { point.y } else { center.y },
        },
        WarpMode::CenterXy if inside_x && inside_y => return None,
        WarpMode::CenterXy | WarpMode::CenterXyAlways => center,
    };
    (destination != point).then_some(destination)
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    target: FocusTarget,
    entered: Instant,
}

#[derive(Default)]
struct Dwell {
    pending: Option<Pending>,
    settled: Option<FocusTarget>,
}

#[derive(Debug, PartialEq, Eq)]
enum DwellDecision {
    Idle,
    Wait(Duration),
    Focus(FocusTarget),
}

impl Dwell {
    fn invalidate(&mut self) {
        *self = Self::default();
    }

    fn observe(
        &mut self,
        target: Option<FocusTarget>,
        now: Instant,
        delay_ms: u32,
    ) -> DwellDecision {
        let Some(target) = target else {
            *self = Self::default();
            return DwellDecision::Idle;
        };
        if self.settled == Some(target) {
            return DwellDecision::Idle;
        }
        if self.pending.is_none_or(|pending| pending.target != target) {
            self.settled = None;
            self.pending = Some(Pending {
                target,
                entered: now,
            });
        }
        let elapsed = now.saturating_duration_since(self.pending.expect("set above").entered);
        let delay = Duration::from_millis(u64::from(delay_ms.min(60_000)));
        if elapsed < delay {
            DwellDecision::Wait(delay - elapsed)
        } else {
            self.pending = None;
            self.settled = Some(target);
            DwellDecision::Focus(target)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(window: isize) -> FocusTarget {
        FocusTarget {
            window: WindowId::new(window),
            output: OutputId::from_name("primary"),
            workspace: 0,
            column: Some(0),
        }
    }

    #[test]
    fn dwell_zero_delay_focuses_once_per_entry() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        assert_eq!(
            dwell.observe(Some(target(1)), now, 0),
            DwellDecision::Focus(target(1))
        );
        assert_eq!(dwell.observe(Some(target(1)), now, 0), DwellDecision::Idle);
        assert_eq!(dwell.observe(None, now, 0), DwellDecision::Idle);
        assert_eq!(
            dwell.observe(Some(target(1)), now, 0),
            DwellDecision::Focus(target(1))
        );
    }

    #[test]
    fn dwell_invalidated_focus_can_retry_without_pointer_reentry() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        assert_eq!(
            dwell.observe(Some(target(1)), now, 0),
            DwellDecision::Focus(target(1))
        );
        dwell.invalidate(); // hit/membership changed during engine-lock acquisition
        assert_eq!(
            dwell.observe(Some(target(1)), now, 0),
            DwellDecision::Focus(target(1))
        );
        assert_eq!(dwell.observe(Some(target(1)), now, 0), DwellDecision::Idle);
    }

    #[test]
    fn dwell_motion_within_target_preserves_entry_deadline() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        assert_eq!(
            dwell.observe(Some(target(1)), now, 100),
            DwellDecision::Wait(Duration::from_millis(100))
        );
        assert_eq!(
            dwell.observe(Some(target(1)), now + Duration::from_millis(80), 100),
            DwellDecision::Wait(Duration::from_millis(20))
        );
        assert_eq!(
            dwell.observe(Some(target(1)), now + Duration::from_millis(100), 100),
            DwellDecision::Focus(target(1))
        );
        assert_eq!(
            dwell.observe(Some(target(1)), now + Duration::from_millis(200), 100),
            DwellDecision::Idle
        );
    }

    #[test]
    fn dwell_new_target_restarts_delay() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        dwell.observe(Some(target(1)), now, 100);
        assert_eq!(
            dwell.observe(Some(target(2)), now + Duration::from_millis(90), 100),
            DwellDecision::Wait(Duration::from_millis(100))
        );
        assert_eq!(
            dwell.observe(Some(target(2)), now + Duration::from_millis(100), 100),
            DwellDecision::Wait(Duration::from_millis(90))
        );
    }

    #[test]
    fn dwell_unmanaged_hit_cancels_pending_target() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        dwell.observe(Some(target(1)), now, 100);
        dwell.observe(None, now + Duration::from_millis(90), 100);
        assert!(dwell.pending.is_none());
        assert_eq!(
            dwell.observe(Some(target(1)), now + Duration::from_millis(100), 100),
            DwellDecision::Wait(Duration::from_millis(100))
        );
    }

    #[test]
    fn dwell_workspace_or_output_change_is_a_new_target() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        let first = target(1);
        let other_output = FocusTarget {
            output: OutputId::from_name("secondary"),
            ..first
        };
        let other_workspace = FocusTarget {
            workspace: 1,
            ..first
        };
        assert_eq!(
            dwell.observe(Some(first), now, 0),
            DwellDecision::Focus(first)
        );
        assert_eq!(
            dwell.observe(Some(other_output), now, 0),
            DwellDecision::Focus(other_output)
        );
        assert_eq!(
            dwell.observe(Some(other_workspace), now, 0),
            DwellDecision::Focus(other_workspace)
        );
    }

    #[test]
    fn dwell_delay_is_defensively_bounded() {
        let mut dwell = Dwell::default();
        assert_eq!(
            dwell.observe(Some(target(1)), Instant::now(), u32::MAX),
            DwellDecision::Wait(Duration::from_secs(60))
        );
    }

    #[test]
    fn scroll_zero_percent_allows_only_no_scroll() {
        assert!(scroll_allowed(55, 55, 1000, Some(0.0)));
        assert!(!scroll_allowed(55, 56, 1000, Some(0.0)));
        assert!(!scroll_allowed(55, 54, 1000, Some(0.0)));
    }

    #[test]
    fn scroll_limit_uses_working_area_percentage_in_either_direction() {
        assert!(scroll_allowed(0, 250, 1000, Some(25.0)));
        assert!(scroll_allowed(250, 0, 1000, Some(25.0)));
        assert!(!scroll_allowed(0, 251, 1000, Some(25.0)));
        assert!(!scroll_allowed(251, 0, 1000, Some(25.0)));
        assert!(scroll_allowed(0, 1500, 1000, Some(150.0)));
        assert!(scroll_allowed(i32::MIN, i32::MAX, 1000, None));
    }

    #[test]
    fn scroll_rejects_invalid_programmatic_limits() {
        for percent in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(!scroll_allowed(0, 0, 1000, Some(percent)));
        }
    }

    #[test]
    fn foreground_request_survives_early_duplicate_until_completion() {
        let mut state = ForegroundState::default();
        assert!(state.observe(10, true));
        // Early foreground/native-geometry guards leave the pending request.
        assert!(!state.observe(10, true));
        assert_eq!(state.pending, Some(10));
        state.pending = None;
        assert!(!state.observe(10, true));
        assert_eq!(state.pending, None);
    }

    #[test]
    fn foreground_request_is_replaced_or_suppressed_not_replayed() {
        let mut state = ForegroundState::default();
        state.observe(10, true);
        assert!(state.observe(20, true));
        assert_eq!(state.pending, Some(20));
        state.suppress(30); // pointer focus / startup / config reload
        assert!(!state.observe(30, true));
        assert_eq!(state.pending, None);
        assert!(state.observe(40, false));
        assert_eq!(state.pending, None);
    }

    #[test]
    fn warp_waits_for_native_layout_including_old_on_screen_geometry() {
        let expected = Rect::new(923, 12, 431, 704);
        assert!(!warp_geometry_ready(Rect::new(4, 836, 431, 704), expected));
        assert!(!warp_geometry_ready(Rect::new(12, 12, 431, 704), expected));
        assert!(warp_geometry_ready(expected, expected));
        assert!(warp_geometry_ready(Rect::new(924, 13, 466, 704), expected));
        assert!(!warp_geometry_ready(Rect::new(923, 12, 0, 704), expected));
    }

    #[test]
    fn warp_geometry_handles_negative_output_coordinates_without_overflow() {
        let expected = Rect::new(-1500, -700, 400, 300);
        assert!(warp_geometry_ready(expected, expected));
        assert!(!warp_geometry_ready(
            Rect::new(i32::MAX, 0, 1, 1),
            Rect::new(i32::MIN, 0, 1, 1)
        ));
    }

    #[test]
    fn warp_default_centers_only_outside_axes() {
        let rect = Rect::new(-200, 100, 200, 100);
        assert_eq!(
            warp_destination(rect, Point { x: -150, y: 99 }, WarpMode::default()),
            Some(Point { x: -150, y: 150 })
        );
        assert_eq!(
            warp_destination(rect, Point { x: 0, y: 125 }, WarpMode::default()),
            Some(Point { x: -100, y: 125 })
        );
        assert_eq!(
            warp_destination(rect, Point { x: 20, y: 90 }, WarpMode::default()),
            Some(Point { x: -100, y: 150 })
        );
        assert_eq!(
            warp_destination(rect, Point { x: -200, y: 100 }, WarpMode::default()),
            None
        );
    }

    #[test]
    fn warp_center_modes_distinguish_inside_pointer() {
        let rect = Rect::new(100, 200, 200, 100);
        let inside = Point { x: 110, y: 220 };
        let outside = Point { x: 110, y: 300 };
        let center = Point { x: 200, y: 250 };
        assert_eq!(warp_destination(rect, inside, WarpMode::CenterXy), None);
        assert_eq!(
            warp_destination(rect, inside, WarpMode::CenterXyAlways),
            Some(center)
        );
        assert_eq!(
            warp_destination(rect, outside, WarpMode::CenterXy),
            Some(center)
        );
        assert_eq!(
            warp_destination(rect, center, WarpMode::CenterXyAlways),
            None
        );
        assert_eq!(
            warp_destination(Rect::new(0, 0, 0, 10), outside, WarpMode::CenterXyAlways),
            None
        );
    }

    fn strip() -> (TilingEngine, OutputId) {
        let mut engine = TilingEngine::new(crate::layout::LayoutConfig {
            outer_gaps: (0, 0, 0, 0),
            column_gap: 10,
            ..Default::default()
        });
        let output = OutputId::from_name("pointer-test");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1000, 800),
            Rect::new(0, 0, 1000, 760),
        );
        let workspace = engine
            .monitors_mut()
            .get_mut(&output)
            .unwrap()
            .workspace_mut()
            .unwrap();
        workspace.add_window_to_new_column_with_width(WindowId::new(10), Some(500));
        workspace.add_window_to_new_column_with_width(WindowId::new(20), Some(500));
        (engine, output)
    }

    #[test]
    fn target_resolution_promotes_owned_roots_but_not_unmanaged_overlays() {
        let (engine, output) = strip();
        let point = Point::new(100, 100);
        let hit = resolve_target(&engine, point, [999, 10]).unwrap();
        assert_eq!(
            (hit.window, hit.output, hit.column),
            (WindowId::new(10), output, Some(0))
        );
        assert_eq!(resolve_target(&engine, point, [999, 998]), None);
        assert_eq!(resolve_target(&engine, Point::new(-1, 100), [10, 0]), None);
    }

    #[test]
    fn output_entry_does_not_require_a_managed_window() {
        let (mut engine, primary) = strip();
        let empty = OutputId::from_name("empty-left");
        engine.register_monitor(
            empty,
            Rect::new(-1000, 0, 1000, 800),
            Rect::new(-1000, 0, 1000, 760),
        );
        assert_eq!(pointer_output(&engine, Point::new(-500, 100)), Some(empty));
        assert_eq!(
            resolve_target(&engine, Point::new(-500, 100), [999, 0]),
            None
        );
        assert_eq!(pointer_output(&engine, Point::new(100, 100)), Some(primary));
        assert_eq!(pointer_output(&engine, Point::new(2000, 100)), None);
    }

    #[test]
    fn decoded_focus_policy_survives_engine_publication() {
        let decoded = crate::config::decode::decode_text(
            "input { focus-follows-mouse max-scroll-amount=\"0%\"; focus-delay-ms 1000; warp-mouse-to-focus mode=\"center-xy-always\"; }",
            "", None,
        );
        assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
        let (mut engine, _) = strip();
        engine.set_full_config(decoded.config);
        let policy = engine.full_config().unwrap().input.focus();
        assert!(policy.follows_mouse && policy.warp_on_focus);
        assert_eq!(policy.focus_delay_ms, 1000);
        assert_eq!(policy.max_scroll_percent, Some(0.0));
        assert_eq!(policy.warp_mode, WarpMode::CenterXyAlways);
    }

    #[test]
    fn target_resolution_refuses_inactive_workspaces_and_wrong_outputs() {
        let (mut engine, output) = strip();
        let other = OutputId::from_name("pointer-other");
        engine.register_monitor(
            other,
            Rect::new(1000, 0, 1000, 800),
            Rect::new(1000, 0, 1000, 760),
        );
        assert_eq!(
            resolve_target(&engine, Point::new(1100, 100), [10, 0]),
            None
        );
        engine
            .monitors_mut()
            .get_mut(&output)
            .unwrap()
            .active_workspace += 1;
        assert_eq!(resolve_target(&engine, Point::new(100, 100), [10, 0]), None);
    }

    #[test]
    fn reveal_limit_uses_real_column_widths_without_mutating_the_strip() {
        let (engine, output) = strip();
        let target = resolve_target(&engine, Point::new(100, 100), [20, 0]).unwrap();
        assert_eq!(reveal_scroll(&engine, target, Some(0.0)), None);
        assert_eq!(reveal_scroll(&engine, target, Some(0.999)), None);
        assert_eq!(reveal_scroll(&engine, target, Some(1.0)), Some(10));
        assert_eq!(reveal_scroll(&engine, target, None), Some(10));
        assert_eq!(
            engine
                .monitors()
                .get(&output)
                .unwrap()
                .workspace()
                .unwrap()
                .scroll_offset
                .x,
            0
        );
    }

    #[test]
    fn zero_scroll_limit_allows_an_already_visible_column() {
        let (engine, _) = strip();
        let target = resolve_target(&engine, Point::new(100, 100), [10, 0]).unwrap();
        assert_eq!(reveal_scroll(&engine, target, Some(0.0)), Some(0));
    }

    #[test]
    fn motion_coordinate_packing_preserves_negative_and_extreme_points() {
        for point in [
            Point { x: -1920, y: -1080 },
            Point {
                x: i32::MIN,
                y: i32::MAX,
            },
        ] {
            assert_eq!(unpack(pack(point)), point);
        }
    }

    #[test]
    fn focus_decoder_defaults_are_disabled() {
        let decoded = crate::config::decode::decode_text("", "", None);
        assert!(!decoded.has_errors());
        let focus = decoded.config.input.focus();
        assert!(!focus.follows_mouse);
        assert!(!focus.warp_on_focus);
        assert_eq!(focus.focus_delay_ms, 0);
        assert_eq!(focus.max_scroll_percent, None);
        assert_eq!(focus.warp_mode, WarpMode::CenterOutsideAxes);
    }

    #[test]
    fn focus_decoder_accepts_percentages_modes_and_nested_delay() {
        for (mode, expected) in [
            ("center-xy", WarpMode::CenterXy),
            ("center-xy-always", WarpMode::CenterXyAlways),
        ] {
            let text = format!("input {{\n focus-follows-mouse max-scroll-amount=\"150.5%\"\n warp-mouse-to-focus mode=\"{mode}\"\n focus {{ focus-delay-ms 125; }}\n}}\n");
            let decoded = crate::config::decode::decode_text(&text, "", None);
            assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
            let focus = decoded.config.input.focus();
            assert!(focus.follows_mouse && focus.warp_on_focus);
            assert_eq!(focus.focus_delay_ms, 125);
            assert_eq!(focus.max_scroll_percent, Some(150.5));
            assert_eq!(focus.warp_mode, expected);
        }
    }

    #[test]
    fn focus_decoder_rejects_bad_percentages() {
        for value in [
            "10",
            "\"10\"",
            "\"-1%\"",
            "\"NaN%\"",
            "\"inf%\"",
            "\"hello%\"",
        ] {
            let text = format!("input {{ focus-follows-mouse max-scroll-amount={value}; }}\n");
            let decoded = crate::config::decode::decode_text(&text, "", None);
            assert!(decoded.has_errors(), "accepted {value}");
        }
    }

    #[test]
    fn focus_decoder_rejects_unknown_modes_and_out_of_range_delay() {
        for text in [
            "input { warp-mouse-to-focus mode=\"unknown\"; }",
            "input { focus-delay-ms 60001; }",
        ] {
            let decoded = crate::config::decode::decode_text(text, "", None);
            assert!(decoded.has_errors(), "accepted {text}");
        }
    }
}
