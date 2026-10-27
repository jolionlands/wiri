//! Event-driven, one-physical-pixel hot corners (niri e19df12cf8).
//!
//! WH_MOUSE_LL only publishes coordinates and a travel generation. All policy,
//! hit testing, timers and engine work run on the existing input message thread.
//! A timer exists only during the 120ms dwell; there is no idle poll or worker.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, KillTimer, PostThreadMessageW, SetTimer, MSG, WM_TIMER, WM_USER,
};

use crate::backend::{outputs::OutputMatcher, BackendHandle, MonitorInfo};
use crate::config::hot_corner_config::HotCorners;
use crate::layout::TilingEngine;
use crate::utils::{OutputId, Point, Rect};

const WM_HOT_CORNER_MOTION: u32 = WM_USER + 314;
const WM_HOT_CORNER_RELOAD: u32 = WM_USER + 315;
const WM_HOT_CORNER_GEOMETRY: u32 = WM_USER + 316;
const HOT_CORNER_TIMER: usize = (WM_USER + 317) as usize;
const DWELL: Duration = Duration::from_millis(120);

static OWNER_THREAD: AtomicU32 = AtomicU32::new(0);
static ENABLED: AtomicBool = AtomicBool::new(false);
static MOTION_PENDING: AtomicBool = AtomicBool::new(false);
// A single writer (WH_MOUSE_LL) publishes a coherent point/generation pair.
// Counting travel, not messages, preserves leave/reentry across coalescing.
static SAMPLE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SAMPLE_POINT: AtomicU64 = AtomicU64::new(0);
static SAMPLE_TRAVEL: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
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

#[derive(Clone, Copy)]
struct Sample {
    point: Point,
    travel: u64,
}

fn sample() -> Sample {
    loop {
        let before = SAMPLE_SEQUENCE.load(Ordering::Acquire);
        if before & 1 != 0 {
            std::hint::spin_loop();
            continue;
        }
        let point = SAMPLE_POINT.load(Ordering::Acquire);
        let travel = SAMPLE_TRAVEL.load(Ordering::Acquire);
        if before == SAMPLE_SEQUENCE.load(Ordering::Acquire) {
            return Sample {
                point: unpack(point),
                travel,
            };
        }
    }
}

fn post(message: u32) -> bool {
    let owner = OWNER_THREAD.load(Ordering::Acquire);
    // SAFETY: no pointers are sent; initialization has created the owner queue.
    owner != 0 && unsafe { PostThreadMessageW(owner, message, WPARAM(0), LPARAM(0)).is_ok() }
}

/// Hook-safe: no engine lock, grab mutex, OS query, timer or layout work.
pub fn on_mouse_move(point: Point) {
    // Keep the physical sample fresh even while every corner is disabled. A
    // later reload must not mistake an old point for a leave/reentry.
    let packed = pack(point);
    SAMPLE_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    if SAMPLE_POINT.load(Ordering::Acquire) != packed {
        SAMPLE_POINT.store(packed, Ordering::Release);
        SAMPLE_TRAVEL.fetch_add(1, Ordering::Release);
    }
    SAMPLE_SEQUENCE.fetch_add(1, Ordering::Release);
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    if !MOTION_PENDING.swap(true, Ordering::AcqRel) && !post(WM_HOT_CORNER_MOTION) {
        MOTION_PENDING.store(false, Ordering::Release);
    }
}

pub fn initialize(engine: Arc<RwLock<TilingEngine>>, backend: BackendHandle, hwnd: HWND) {
    shutdown();
    // SAFETY: the caller owns the hidden hotkey HWND on this thread.
    OWNER_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let mut state = State {
        engine,
        backend,
        hwnd,
        corners: Vec::new(),
        dwell: Dwell::default(),
        timer_armed: false,
    };
    state.reload();
    STATE.with(|slot| *slot.borrow_mut() = Some(state));
}

pub fn reload() {
    let _ = post(WM_HOT_CORNER_RELOAD);
}

pub fn display_changed() {
    let _ = post(WM_HOT_CORNER_GEOMETRY);
}

pub fn shutdown() {
    ENABLED.store(false, Ordering::Release);
    OWNER_THREAD.store(0, Ordering::Release);
    MOTION_PENDING.store(false, Ordering::Release);
    STATE.with(|slot| {
        if let Some(mut state) = slot.borrow_mut().take() {
            state.stop_timer();
        }
    });
}

pub fn handle_message(message: &MSG) -> bool {
    STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return false;
        };
        match message.message {
            WM_HOT_CORNER_MOTION if message.hwnd.is_invalid() => {
                if MOTION_PENDING.swap(false, Ordering::AcqRel) {
                    state.motion(sample());
                }
                true
            }
            WM_HOT_CORNER_RELOAD | WM_HOT_CORNER_GEOMETRY if message.hwnd.is_invalid() => {
                state.reload();
                true
            }
            WM_TIMER if message.hwnd == state.hwnd && message.wParam.0 == HOT_CORNER_TIMER => {
                state.expired();
                true
            }
            _ => false,
        }
    })
}

#[derive(Clone, Copy)]
struct Corner {
    output: OutputId,
    bounds: Rect,
}

impl Corner {
    fn contains(self, point: Point) -> bool {
        // Bounds are physical rcMonitor, not work-area or layout/DPI coordinates.
        self.bounds.size.w != 0 && self.bounds.size.h != 0 && point == self.bounds.loc
    }
}

fn configured_corners(
    output: OutputId,
    rect: Rect,
    policy: HotCorners,
) -> impl Iterator<Item = Corner> {
    let right = (i64::from(rect.loc.x) + i64::from(rect.size.w) - 1)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let bottom = (i64::from(rect.loc.y) + i64::from(rect.size.h) - 1)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    [
        (rect.loc.x, rect.loc.y),
        (right, rect.loc.y),
        (rect.loc.x, bottom),
        (right, bottom),
    ]
    .into_iter()
    .zip(policy.enabled())
    .filter_map(move |((x, y), enabled)| {
        (enabled && rect.size.w != 0 && rect.size.h != 0).then_some(Corner {
            output,
            bounds: Rect::new(x, y, 1, 1),
        })
    })
}

fn target_at(corners: &[Corner], point: Point) -> Option<OutputId> {
    corners
        .iter()
        .find(|corner| corner.contains(point))
        .map(|corner| corner.output)
}

fn cursor_position() -> Option<Point> {
    let mut point = POINT::default();
    // SAFETY: `point` is writable storage for the Win32 result.
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some(Point {
        x: point.x,
        y: point.y,
    })
}

fn physical_monitors() -> Vec<MonitorInfo> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        if let Some(info) = MonitorInfo::from_hmonitor(monitor) {
            // SAFETY: enumeration synchronously borrows the vector below.
            (&mut *(data.0 as *mut Vec<MonitorInfo>)).push(info);
        }
        BOOL(1)
    }
    let mut monitors = Vec::<MonitorInfo>::new();
    // SAFETY: callback data is live for this synchronous enumeration.
    let ok = unsafe {
        EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(collect),
            LPARAM(&mut monitors as *mut _ as isize),
        )
    };
    if ok.as_bool() {
        monitors
    } else {
        Vec::new()
    }
}

fn suppressed() -> bool {
    super::low_level_hook::is_grab_active()
        || crate::ipc::pick::is_active()
        || crate::overlay::screenshot_ui::is_active()
        || super::recent::is_active()
}

struct State {
    engine: Arc<RwLock<TilingEngine>>,
    backend: BackendHandle,
    hwnd: HWND,
    corners: Vec<Corner>,
    dwell: Dwell,
    timer_armed: bool,
}

impl State {
    fn reload(&mut self) {
        self.stop_timer();
        let config = self
            .engine
            .read()
            .full_config()
            .cloned()
            .unwrap_or_default();
        let matcher = OutputMatcher::snapshot();
        // Enumerate here: the backend's display-change event may still be queued.
        self.corners = physical_monitors()
            .into_iter()
            .filter_map(|monitor| {
                let output = matcher.matching_config(&config, &monitor);
                if output.is_some_and(|output| !output.enable) {
                    return None;
                }
                let policy = HotCorners::for_output(
                    config.gestures.hot_corners,
                    output.and_then(|output| output.hot_corners),
                );
                Some((monitor, policy))
            })
            .flat_map(|(monitor, policy)| configured_corners(monitor.id, monitor.bounds, policy))
            .collect();
        // Startup/reload/hotplug are not a pointer entry. In particular, do not
        // reopen an overview closed over IPC while the pointer remains parked.
        let current = sample();
        let target = cursor_position().and_then(|point| target_at(&self.corners, point));
        self.dwell.park(target, current.travel);
        ENABLED.store(!self.corners.is_empty(), Ordering::Release);
    }

    fn stop_timer(&mut self) {
        if self.timer_armed {
            // SAFETY: this module exclusively owns the ID on this live HWND.
            let _ = unsafe { KillTimer(self.hwnd, HOT_CORNER_TIMER) };
            self.timer_armed = false;
        }
    }

    fn motion(&mut self, current: Sample) {
        let target = target_at(&self.corners, current.point);
        let decision = self
            .dwell
            .observe(target, current.travel, Instant::now(), suppressed());
        self.perform(decision);
    }

    fn expired(&mut self) {
        // Win32 timers repeat; remove ours before observing or invoking layout.
        self.stop_timer();
        let current = sample();
        let Some(point) = cursor_position() else {
            self.dwell.park(None, current.travel);
            return;
        };
        if current.point != point {
            // A newer OS position without its hook notification is not evidence
            // of a completed dwell. Fail closed until a real motion arrives.
            self.dwell
                .park(target_at(&self.corners, point), current.travel);
            return;
        }
        self.motion(current);
    }

    fn perform(&mut self, decision: Decision) {
        self.stop_timer();
        match decision {
            Decision::Idle => {}
            Decision::Wait(remaining) => {
                let ms = remaining.as_millis().saturating_add(1).clamp(1, 60_000) as u32;
                // SAFETY: same owner HWND/ID as stop_timer; no timer callback.
                self.timer_armed = unsafe { SetTimer(self.hwnd, HOT_CORNER_TIMER, ms, None) } != 0;
                if !self.timer_armed {
                    self.dwell.consume();
                    tracing::warn!("Unable to arm hot-corner dwell timer");
                }
            }
            Decision::Activate(output) => {
                // Dwell was consumed BEFORE toggling. Staying parked (including
                // after an IPC close) must not rearm it; a fresh entry toggles.
                let changed = {
                    let mut engine = self.engine.write();
                    if engine.monitors().get(&output).is_none() {
                        false
                    } else {
                        engine.set_focused_output(output);
                        engine.toggle_overview(&self.backend);
                        true
                    }
                };
                if changed {
                    crate::ipc::notify_state_changed();
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    output: OutputId,
    travel: u64,
}

#[derive(Default)]
struct Dwell {
    entry: Option<Entry>,
    since: Option<Instant>,
    consumed: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Idle,
    Wait(Duration),
    Activate(OutputId),
}

impl Dwell {
    fn park(&mut self, output: Option<OutputId>, travel: u64) {
        self.entry = output.map(|output| Entry { output, travel });
        self.consume();
    }

    fn consume(&mut self) {
        self.since = None;
        self.consumed = true;
    }

    fn observe(
        &mut self,
        output: Option<OutputId>,
        travel: u64,
        now: Instant,
        blocked: bool,
    ) -> Decision {
        let entry = output.map(|output| Entry { output, travel });
        if entry != self.entry {
            self.entry = entry;
            self.since = entry.map(|_| now);
            self.consumed = false;
        }
        if blocked {
            self.consume();
        }
        let Some(entry) = self.entry else {
            return Decision::Idle;
        };
        if self.consumed {
            return Decision::Idle;
        }
        let Some(since) = self.since else {
            return Decision::Idle;
        };
        let elapsed = now.saturating_duration_since(since);
        if elapsed < DWELL {
            Decision::Wait(DWELL - elapsed)
        } else {
            self.consume();
            Decision::Activate(entry.output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(name: &str) -> OutputId {
        OutputId::from_name(name)
    }

    #[test]
    fn named_corners_use_the_last_physical_pixel_and_replace_the_default() {
        let out = output("left");
        let bounds = Rect::new(-1920, -200, 1920, 1080);
        let policy = HotCorners {
            top_right: true,
            bottom_left: true,
            bottom_right: true,
            ..HotCorners::default()
        };
        let corners: Vec<_> = configured_corners(out, bounds, policy).collect();
        assert_eq!(corners.len(), 3);
        assert_eq!(target_at(&corners, Point::new(-1920, -200)), None);
        for point in [
            Point::new(-1, -200),
            Point::new(-1920, 879),
            Point::new(-1, 879),
        ] {
            assert_eq!(target_at(&corners, point), Some(out));
            assert_eq!(
                target_at(&corners, Point::new(point.x - 1, point.y + 1)),
                None
            );
        }
        assert_eq!(
            configured_corners(
                out,
                bounds,
                HotCorners {
                    off: true,
                    ..policy
                }
            )
            .count(),
            0
        );
        assert_eq!(
            configured_corners(out, Rect::new(0, 0, 0, 0), policy).count(),
            0
        );
    }

    #[test]
    fn exact_physical_pixel_supports_negative_monitor_origins() {
        let corner = Corner {
            output: output("left"),
            bounds: Rect::new(-2560, -200, 2560, 1440),
        };
        assert!(corner.contains(Point { x: -2560, y: -200 }));
        for (x, y) in [(-2559, -200), (-2560, -199), (-2559, -199), (-2561, -200)] {
            assert!(!corner.contains(Point { x, y }));
        }
        // Taskbar/work-area offsets and logical scaling are not inputs at all.
        assert!(!corner.contains(Point { x: -2560, y: -160 }));
        assert!(!Corner {
            bounds: Rect::new(0, 0, 0, 0),
            ..corner
        }
        .contains(Point { x: 0, y: 0 }));
    }

    #[test]
    fn corners_are_isolated_by_output_not_virtual_desktop_origin() {
        let left = output("left");
        let right = output("right");
        let corners = [
            Corner {
                output: left,
                bounds: Rect::new(-1920, 0, 1920, 1080),
            },
            Corner {
                output: right,
                bounds: Rect::new(0, 100, 2560, 1440),
            },
        ];
        assert_eq!(target_at(&corners, Point { x: -1920, y: 0 }), Some(left));
        assert_eq!(target_at(&corners, Point { x: 0, y: 100 }), Some(right));
        assert_eq!(target_at(&corners, Point { x: 0, y: 0 }), None);
    }

    #[test]
    fn dwell_waits_120ms_and_fires_only_once_until_real_reentry() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        let out = output("one");
        assert_eq!(
            dwell.observe(Some(out), 1, now, false),
            Decision::Wait(DWELL)
        );
        assert_eq!(
            dwell.observe(Some(out), 1, now + Duration::from_millis(119), false),
            Decision::Wait(Duration::from_millis(1))
        );
        assert_eq!(
            dwell.observe(Some(out), 1, now + DWELL, false),
            Decision::Activate(out)
        );
        // The consumer may ignore Open (already open) or close via IPC. Neither
        // can make another same-pixel motion or expiry toggle/reopen it.
        assert_eq!(
            dwell.observe(Some(out), 1, now + Duration::from_secs(30), false),
            Decision::Idle
        );
        assert_eq!(
            dwell.observe(None, 2, now + Duration::from_secs(31), false),
            Decision::Idle
        );
        let reentry = now + Duration::from_secs(32);
        assert_eq!(
            dwell.observe(Some(out), 3, reentry, false),
            Decision::Wait(DWELL)
        );
        assert_eq!(
            dwell.observe(Some(out), 3, reentry + DWELL, false),
            Decision::Activate(out)
        );
    }

    #[test]
    fn leaving_before_expiry_cancels_the_pending_dwell() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        let out = output("one");
        dwell.observe(Some(out), 1, now, false);
        assert_eq!(
            dwell.observe(None, 2, now + Duration::from_millis(100), false),
            Decision::Idle
        );
        assert_eq!(dwell.observe(None, 2, now + DWELL, false), Decision::Idle);
        assert_eq!(
            dwell.observe(Some(out), 3, now + DWELL, false),
            Decision::Wait(DWELL)
        );
    }

    #[test]
    fn coalesced_leave_and_return_restart_dwell_and_rearm_a_fired_corner() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        let out = output("one");
        dwell.observe(Some(out), 1, now, false);
        let returned = now + Duration::from_millis(119);
        assert_eq!(
            dwell.observe(Some(out), 3, returned, false),
            Decision::Wait(DWELL)
        );
        assert_eq!(
            dwell.observe(Some(out), 3, returned + DWELL, false),
            Decision::Activate(out)
        );
        assert_eq!(
            dwell.observe(Some(out), 5, returned + DWELL, false),
            Decision::Wait(DWELL)
        );
    }

    #[test]
    fn suppression_and_reload_consume_parked_corner_until_leave() {
        let mut dwell = Dwell::default();
        let now = Instant::now();
        let out = output("one");
        dwell.observe(Some(out), 1, now, false);
        assert_eq!(
            dwell.observe(Some(out), 1, now + DWELL, true),
            Decision::Idle
        );
        assert_eq!(
            dwell.observe(Some(out), 1, now + Duration::from_secs(5), false),
            Decision::Idle
        );
        dwell.park(Some(out), 3);
        assert_eq!(
            dwell.observe(Some(out), 3, now + Duration::from_secs(6), false),
            Decision::Idle
        );
        assert_eq!(
            dwell.observe(Some(out), 5, now + Duration::from_secs(7), false),
            Decision::Wait(DWELL)
        );
    }

    #[test]
    fn coordinate_publication_round_trips_signed_physical_pixels() {
        for point in [
            Point { x: -3840, y: -2160 },
            Point { x: 0, y: 0 },
            Point {
                x: i32::MAX,
                y: i32::MIN,
            },
        ] {
            assert_eq!(unpack(pack(point)), point);
        }
    }
}
