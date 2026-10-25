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
use tracing::warn;
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
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const WINEVENT_SKIPOWNPROCESS: u32 = 2;

static OWNER_THREAD: AtomicU32 = AtomicU32::new(0);
static FOLLOWS_MOUSE: AtomicBool = AtomicBool::new(false);
static MOTION_PENDING: AtomicBool = AtomicBool::new(false);
static MOTION_POINT: AtomicU64 = AtomicU64::new(0);
static FOREGROUND_PENDING: AtomicBool = AtomicBool::new(false);
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
    Point { x: (value >> 32) as i32, y: value as i32 }
}

fn post(message: u32) -> bool {
    let thread = OWNER_THREAD.load(Ordering::Acquire);
    if thread == 0 {
        return false;
    }
    // SAFETY: no pointers are sent; the owner has already created its message queue.
    unsafe { PostThreadMessageW(thread, message, WPARAM(0), LPARAM(0)).is_ok() }
}

/// Called by WH_MOUSE_LL, including while its grab-state mutex is held.
/// Must never lock the engine, acquire that mutex, or call into the layout.
pub fn on_mouse_move(point: Point) {
    let packed = pack(point);
    if WARP_PENDING.swap(false, Ordering::AcqRel)
        && WARP_POINT.load(Ordering::Acquire) == packed
    {
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
        last_foreground: 0,
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
    FOREGROUND_PENDING.store(false, Ordering::Release);
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
        let Some(state) = slot.as_mut() else { return false };
        match message.message {
            WM_POINTER_FOCUS if message.hwnd.is_invalid() => {
                if MOTION_PENDING.swap(false, Ordering::AcqRel) {
                    let point = unpack(MOTION_POINT.load(Ordering::Acquire));
                    state.motion(point);
                }
                true
            }
            WM_FOREGROUND_WARP if message.hwnd.is_invalid() => {
                if FOREGROUND_PENDING.swap(false, Ordering::AcqRel) {
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
    last_foreground: isize,
}

impl FocusState {
    fn reload(&mut self) {
        self.cancel();
        self.policy = self.engine.read().full_config().as_ref()
            .map(|config| config.input.focus()).unwrap_or_default();
        self.policy.focus_delay_ms = self.policy.focus_delay_ms.min(60_000);
        FOLLOWS_MOUSE.store(self.policy.follows_mouse, Ordering::Release);
        MOTION_PENDING.store(false, Ordering::Release);
        WARP_PENDING.store(false, Ordering::Release);
        if self.policy.warp_on_focus && self.foreground_hook.is_none() {
            // SAFETY: out-of-context callbacks are delivered on this pumping
            // thread. The callback only publishes an HWND, never borrowed state.
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND,
                    HMODULE::default(), Some(foreground_event), 0, 0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                warn!("Unable to install pointer-warp foreground hook");
            } else {
                self.foreground_hook = Some(hook);
            }
        } else if !self.policy.warp_on_focus {
            self.remove_foreground_hook();
        }
        // Reload/startup itself is not a foreground change and must not warp.
        // SAFETY: querying the current foreground window has no preconditions.
        self.last_foreground = unsafe { GetForegroundWindow() }.0 as isize;
    }

    fn remove_foreground_hook(&mut self) {
        if let Some(hook) = self.foreground_hook.take() {
            // SAFETY: this thread owns the successfully installed hook.
            let _ = unsafe { UnhookWinEvent(hook) };
        }
        FOREGROUND_PENDING.store(false, Ordering::Release);
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
        self.dwell = Dwell::default();
    }

    fn motion(&mut self, point: Point) {
        if !self.policy.follows_mouse || super::low_level_hook::is_grab_active() {
            self.cancel();
            return;
        }
        let target = target_at(&self.engine.read(), point);
        let previous = self.dwell.pending.map(|pending| pending.target);
        let decision = self.dwell.observe(target, Instant::now(), self.policy.focus_delay_ms);
        // A move within the same target must NOT restart the one-shot timer.
        if previous != self.dwell.pending.map(|pending| pending.target) {
            self.stop_timer();
        }
        self.perform(decision);
    }

    fn perform(&mut self, decision: DwellDecision) {
        match decision {
            DwellDecision::Idle => self.stop_timer(),
            DwellDecision::Wait(remaining) => {
                if !self.timer_armed {
                    let ms = remaining.as_millis().clamp(1, 60_000) as u32;
                    // SAFETY: the owner HWND is live and this ID is module-local.
                    self.timer_armed = unsafe { SetTimer(self.hwnd, FOCUS_TIMER_ID, ms, None) } != 0;
                    if !self.timer_armed {
                        self.cancel();
                        warn!("Unable to arm focus-follows-mouse delay timer");
                    }
                }
            }
            DwellDecision::Focus(target) => {
                self.stop_timer();
                self.focus(target);
            }
        }
    }

    fn expired(&mut self) {
        // Win32 timers repeat by default; remove it BEFORE processing expiry.
        self.stop_timer();
        let Some(pending) = self.dwell.pending else { return };
        if !self.policy.follows_mouse || super::low_level_hook::is_grab_active() {
            self.cancel();
            return;
        }
        let target = cursor_position().and_then(|point| target_at(&self.engine.read(), point));
        if target != Some(pending.target) {
            self.cancel();
            return;
        }
        let decision = self.dwell.observe(target, Instant::now(), self.policy.focus_delay_ms);
        self.perform(decision);
    }

    fn focus(&mut self, target: FocusTarget) {
        // Revalidate even zero-delay requests: a queued move may be stale, and
        // windows, workspaces or outputs may have disappeared while waiting.
        if super::low_level_hook::is_grab_active() {
            self.cancel();
            return;
        }
        let Some(point) = cursor_position() else { return };
        let mut engine = self.engine.write();
        if target_at(&engine, point) != Some(target) {
            return;
        }
        // SAFETY: these are value-only HWND queries; no application memory is read.
        let foreground = unsafe { GetForegroundWindow() };
        let owner = root_owner(foreground);
        if engine.managed_window_for(foreground.0 as isize, owner.0 as isize) == Some(target.window) {
            return;
        }
        if target.column.is_none() {
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
            return;
        };
        let already_selected = engine.focused_output() == Some(target.output)
            && engine.monitors().get(&target.output)
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
        if !self.policy.warp_on_focus || super::low_level_hook::is_grab_active() {
            return;
        }
        // SAFETY: querying the foreground HWND has no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0 as isize != event_hwnd || event_hwnd == self.last_foreground {
            return;
        }
        self.last_foreground = event_hwnd;
        // Focus changes must not leave a delayed hover waiting to steal focus.
        self.cancel();
        // SAFETY: GetAncestor accepts an HWND and returns a value, not a pointer to memory.
        let root = unsafe { GetAncestor(foreground, GA_ROOT) };
        let owner = root_owner(foreground);
        if self.engine.read().managed_window_for(root.0 as isize, owner.0 as isize).is_none() {
            return;
        }
        if !visible_window(root) {
            return;
        }
        let Some(rect) = window_rect(root) else { return };
        let mut cursor = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: cursor is correctly sized, writable stack storage.
        if unsafe { GetCursorInfo(&mut cursor) }.is_err()
            || cursor.flags.0 & CURSOR_SHOWING.0 == 0
        {
            // In particular, never call ShowCursor to reveal a hidden pointer.
            return;
        }
        let point = Point { x: cursor.ptScreenPos.x, y: cursor.ptScreenPos.y };
        let Some(destination) = warp_destination(rect, point, self.policy.warp_mode) else { return };
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
    if event != EVENT_SYSTEM_FOREGROUND || hwnd.is_invalid() {
        return;
    }
    FOREGROUND_HWND.store(hwnd.0 as isize, Ordering::Release);
    if !FOREGROUND_PENDING.swap(true, Ordering::AcqRel) && !post(WM_FOREGROUND_WARP) {
        FOREGROUND_PENDING.store(false, Ordering::Release);
    }
}

fn cursor_position() -> Option<Point> {
    let mut point = POINT::default();
    // SAFETY: point is writable stack storage of the required size.
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some(Point { x: point.x, y: point.y })
}

fn root_owner(hwnd: HWND) -> HWND {
    // Follow GW_OWNER too: some owned dialogs are not WS_POPUP windows.
    HWND(crate::layout::engine::root_owner_of(hwnd.0 as isize) as *mut std::ffi::c_void)
}

fn visible_window(hwnd: HWND) -> bool {
    // SAFETY: these APIs validate HWND values and do not dereference Rust memory.
    (unsafe { IsWindow(hwnd).as_bool() && IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() })
        && !cloak::is_cloaked(hwnd.0 as isize)
}

fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: rect is writable stack storage of the required size.
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    if width <= 0 || height <= 0 { return None }
    Some(Rect::new(rect.left, rect.top, width as u32, height as u32))
}

fn target_at(engine: &TilingEngine, point: Point) -> Option<FocusTarget> {
    // SAFETY: screen-space hit testing returns HWND values, not borrowed memory.
    let hit = unsafe { WindowFromPoint(POINT { x: point.x, y: point.y }) };
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

/// Pure half of hit testing: root/owned-dialog normalization, output membership
/// and active-workspace checks. Never fall through an unmanaged overlay to a tile.
fn resolve_target(engine: &TilingEngine, point: Point, roots: [isize; 2]) -> Option<FocusTarget> {
    if engine.is_overview() { return None }
    let (output, monitor) = engine.monitors().iter()
        .find(|(_, monitor)| monitor.bounds.contains_point(point))?;
    for root in roots {
        if root == 0 { continue }
        let window = WindowId::new(root);
        if let Some((home, workspace, column)) = engine.locate_window(window) {
            if home == *output && workspace == monitor.active_workspace {
                return Some(FocusTarget { window, output: home, workspace, column: Some(column) });
            }
        } else if engine.is_floating(window)
            && (engine.is_sticky(window)
                || engine.float_workspace(window) == Some(monitor.active_workspace))
        {
            return Some(FocusTarget { window, output: *output, workspace: monitor.active_workspace, column: None });
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
    let view_width = monitor.work_area.size.w as i32 - config.outer_gaps.1 - config.outer_gaps.3
        - config.reserve_left as i32 - config.reserve_right as i32;
    if view_width <= 0 { return None }
    let explicit: Vec<_> = workspace.columns.iter().map(|column| {
        if column.maximized { Some(view_width as u32) } else { column.width }
    }).collect();
    let mins: Vec<_> = workspace.columns.iter().map(|column| column.min_width()).collect();
    let maxs: Vec<_> = workspace.columns.iter().map(|column| column.max_width()).collect();
    let (widths, default_width) = TilingEngine::column_widths_with_limits(
        &explicit, &mins, &maxs, view_width, config.column_gap, config,
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

fn warp_destination(rect: Rect, point: Point, mode: WarpMode) -> Option<Point> {
    if rect.size.w == 0 || rect.size.h == 0 { return None }
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
    fn observe(&mut self, target: Option<FocusTarget>, now: Instant, delay_ms: u32) -> DwellDecision {
        let Some(target) = target else {
            *self = Self::default();
            return DwellDecision::Idle;
        };
        if self.settled == Some(target) { return DwellDecision::Idle }
        if self.pending.is_none_or(|pending| pending.target != target) {
            self.settled = None;
            self.pending = Some(Pending { target, entered: now });
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
        FocusTarget { window: WindowId::new(window), output: 1, workspace: 0, column: Some(0) }
    }

    #[test]
    fn dwell_zero_delay_focuses_once_per_entry() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        assert_eq!(dwell.observe(Some(target(1)), now, 0), DwellDecision::Focus(target(1)));
        assert_eq!(dwell.observe(Some(target(1)), now, 0), DwellDecision::Idle);
        assert_eq!(dwell.observe(None, now, 0), DwellDecision::Idle);
        assert_eq!(dwell.observe(Some(target(1)), now, 0), DwellDecision::Focus(target(1)));
    }

    #[test]
    fn dwell_motion_within_target_preserves_entry_deadline() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        assert_eq!(dwell.observe(Some(target(1)), now, 100), DwellDecision::Wait(Duration::from_millis(100)));
        assert_eq!(dwell.observe(Some(target(1)), now + Duration::from_millis(80), 100), DwellDecision::Wait(Duration::from_millis(20)));
        assert_eq!(dwell.observe(Some(target(1)), now + Duration::from_millis(100), 100), DwellDecision::Focus(target(1)));
        assert_eq!(dwell.observe(Some(target(1)), now + Duration::from_millis(200), 100), DwellDecision::Idle);
    }

    #[test]
    fn dwell_new_target_restarts_delay() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        dwell.observe(Some(target(1)), now, 100);
        assert_eq!(dwell.observe(Some(target(2)), now + Duration::from_millis(90), 100), DwellDecision::Wait(Duration::from_millis(100)));
        assert_eq!(dwell.observe(Some(target(2)), now + Duration::from_millis(100), 100), DwellDecision::Wait(Duration::from_millis(90)));
    }

    #[test]
    fn dwell_unmanaged_hit_cancels_pending_target() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        dwell.observe(Some(target(1)), now, 100);
        dwell.observe(None, now + Duration::from_millis(90), 100);
        assert!(dwell.pending.is_none());
        assert_eq!(dwell.observe(Some(target(1)), now + Duration::from_millis(100), 100), DwellDecision::Wait(Duration::from_millis(100)));
    }

    #[test]
    fn dwell_workspace_or_output_change_is_a_new_target() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        let first = target(1);
        let other_output = FocusTarget { output: 2, ..first };
        let other_workspace = FocusTarget { workspace: 1, ..first };
        assert_eq!(dwell.observe(Some(first), now, 0), DwellDecision::Focus(first));
        assert_eq!(dwell.observe(Some(other_output), now, 0), DwellDecision::Focus(other_output));
        assert_eq!(dwell.observe(Some(other_workspace), now, 0), DwellDecision::Focus(other_workspace));
    }

    #[test]
    fn dwell_delay_is_defensively_bounded() {
        let mut dwell = Dwell::default();
        assert_eq!(dwell.observe(Some(target(1)), Instant::now(), u32::MAX), DwellDecision::Wait(Duration::from_secs(60)));
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
    fn warp_default_centers_only_outside_axes() {
        let rect = Rect::new(-200, 100, 200, 100);
        assert_eq!(warp_destination(rect, Point { x: -150, y: 99 }, WarpMode::default()), Some(Point { x: -150, y: 150 }));
        assert_eq!(warp_destination(rect, Point { x: 0, y: 125 }, WarpMode::default()), Some(Point { x: -100, y: 125 }));
        assert_eq!(warp_destination(rect, Point { x: 20, y: 90 }, WarpMode::default()), Some(Point { x: -100, y: 150 }));
        assert_eq!(warp_destination(rect, Point { x: -200, y: 100 }, WarpMode::default()), None);
    }

    #[test]
    fn warp_center_modes_distinguish_inside_pointer() {
        let rect = Rect::new(100, 200, 200, 100);
        let inside = Point { x: 110, y: 220 };
        let outside = Point { x: 110, y: 300 };
        let center = Point { x: 200, y: 250 };
        assert_eq!(warp_destination(rect, inside, WarpMode::CenterXy), None);
        assert_eq!(warp_destination(rect, inside, WarpMode::CenterXyAlways), Some(center));
        assert_eq!(warp_destination(rect, outside, WarpMode::CenterXy), Some(center));
        assert_eq!(warp_destination(rect, center, WarpMode::CenterXyAlways), None);
        assert_eq!(warp_destination(Rect::new(0, 0, 0, 10), outside, WarpMode::CenterXyAlways), None);
    }

    #[test]
    fn motion_coordinate_packing_preserves_negative_and_extreme_points() {
        for point in [Point { x: -1920, y: -1080 }, Point { x: i32::MIN, y: i32::MAX }] {
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
        for (mode, expected) in [("center-xy", WarpMode::CenterXy), ("center-xy-always", WarpMode::CenterXyAlways)] {
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
        for value in ["10", "\"10\"", "\"-1%\"", "\"NaN%\"", "\"inf%\"", "\"hello%\""] {
            let text = format!("input {{ focus-follows-mouse max-scroll-amount={value}; }}\n");
            let decoded = crate::config::decode::decode_text(&text, "", None);
            assert!(decoded.has_errors(), "accepted {value}");
        }
    }

    #[test]
    fn focus_decoder_rejects_unknown_modes_and_out_of_range_delay() {
        for text in ["input { warp-mouse-to-focus mode=\"unknown\"; }", "input { focus-delay-ms 60001; }"] {
            let decoded = crate::config::decode::decode_text(text, "", None);
            assert!(decoded.has_errors(), "accepted {text}");
        }
    }
}
