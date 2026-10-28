//! Windows DnD approximation: held left button + source-process capture + drag distance.
//! No OLE hook/injection, no idle timer. Active edge motion alone owns a frame timer.
//! Both axes are live in the normal view: the strip scrolls at the left/right edge and a held
//! drag at the top/bottom edge steps the workspace (the overview uses the same code path).
use crate::backend::BackendHandle;
use crate::config::gestures_config::{EdgeScroll, GesturesConfig};
use crate::layout::TilingEngine;
use crate::utils::{OutputId, Point, WindowId};
use parking_lot::{Mutex, RwLock};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::debug;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Window(WindowId),
    Workspace(OutputId, i32),
}
#[derive(Default)]
struct Detector {
    down: Option<(Point, u32, u32)>,
}
static DETECTOR: Mutex<Detector> = Mutex::new(Detector { down: None });
static ENABLED: AtomicBool = AtomicBool::new(false);
fn is_drag(start: Point, point: Point, held: bool, foreign_capture: bool, own_grab: bool) -> bool {
    held && foreign_capture
        && !own_grab
        && ((point.x - start.x).abs() >= 8 || (point.y - start.y).abs() >= 8)
}
unsafe fn capture_matches(tid: u32, pid: u32) -> bool {
    let mut gui = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let mut capture_pid = 0;
    GetGUIThreadInfo(tid, &mut gui).is_ok()
        && !gui.hwndCapture.is_invalid()
        && {
            GetWindowThreadProcessId(gui.hwndCapture, Some(&mut capture_pid));
            capture_pid == pid
        }
        && pid != std::process::id()
}
pub fn observe(message: u32, point: Point, own: bool) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    if !matches!(message, 0x200..=0x202) {
        return;
    }
    let mut detector = DETECTOR.lock();
    if message == 0x201 {
        let mut pid = 0;
        let tid = unsafe {
            GetWindowThreadProcessId(
                WindowFromPoint(POINT {
                    x: point.x,
                    y: point.y,
                }),
                Some(&mut pid),
            )
        };
        detector.down = Some((point, tid, pid));
        return;
    }
    if message == 0x202 || own {
        detector.down = None;
    }
    let source = detector.down;
    let active = source.is_some_and(|(start, tid, pid)| {
        is_drag(
            start,
            point,
            true,
            unsafe { capture_matches(tid, pid) },
            own,
        )
    });
    drop(detector);
    super::dispatch::send_work(Box::new(move || {
        SERVICE.with(|s| {
            if let Some(s) = s.borrow_mut().as_mut() {
                s.motion(
                    point,
                    if active {
                        source.map(|(_, t, p)| (t, p))
                    } else {
                        None
                    },
                );
            }
        })
    }));
}

/// niri's one-screen threshold: a held drag travels 1500 units per workspace (one second at
/// `max-speed 1500`), so a stationary pointer steps once per screen instead of once per frame.
const WORKSPACE_STEP_UNITS: f64 = 1500.0;

/// Accumulates vertical edge travel into workspace steps. The direction is the sign of the
/// travel, and only a crossing of `WORKSPACE_STEP_UNITS` produces a step, so holding still at
/// the edge cannot switch once per frame.
#[derive(Default)]
struct WorkspaceSteps {
    pending: f64,
}
impl WorkspaceSteps {
    fn push(&mut self, dy: f64) -> Option<i32> {
        self.pending += dy;
        if self.pending.abs() < WORKSPACE_STEP_UNITS {
            return None;
        }
        let direction = if self.pending > 0.0 { 1 } else { -1 };
        self.pending -= WORKSPACE_STEP_UNITS * direction as f64;
        Some(direction)
    }
    fn reset(&mut self) {
        self.pending = 0.0;
    }
}

fn edge_speed(position: i32, origin: i32, extent: u32, scale: f64, cfg: &EdgeScroll) -> f64 {
    let trigger = cfg.trigger * scale;
    if trigger <= 0.0 || extent == 0 {
        return 0.0;
    }
    let p = (position - origin) as f64;
    let left = ((trigger - p) / trigger).clamp(0.0, 1.0);
    let right = ((p - (extent as f64 - trigger)) / trigger).clamp(0.0, 1.0);
    (right - left) * cfg.max_speed * scale
}

/// Vertical travel for `dnd-edge-workspace-switch`. niri switches the workspace when a held
/// drag reaches the top or bottom edge of the screen, in the normal (scrolling) view as well
/// as in the overview; only an exhausted strip suppresses the axis.
fn workspace_edge_speed(stopped: bool, y: i32, origin: i32, extent: u32, cfg: &EdgeScroll) -> f64 {
    if stopped {
        0.0
    } else {
        edge_speed(y, origin, extent, 1.0, cfg)
    }
}

/// The direction word of the edge-frame debug line: one axis is live at a time.
fn axis_of(dx: f64, dy: f64) -> &'static str {
    if dx > 0.0 {
        "right"
    } else if dx < 0.0 {
        "left"
    } else if dy > 0.0 {
        "down"
    } else {
        "up"
    }
}

#[derive(Default)]
struct EdgeClock {
    sign: i8,
    entered: Option<Instant>,
    last: Option<Instant>,
}
impl EdgeClock {
    fn step(&mut self, speed: f64, now: Instant, delay: u32) -> (f64, Option<Duration>) {
        let sign = if speed == 0.0 {
            0
        } else if speed < 0.0 {
            -1
        } else {
            1
        };
        if sign != self.sign {
            self.sign = sign;
            self.entered = Some(now);
            self.last = Some(now);
        }
        if sign == 0 {
            return (0.0, None);
        }
        let ready = self.entered.unwrap() + Duration::from_millis(delay as u64);
        if now < ready {
            return (0.0, Some(ready - now));
        }
        let from = self.last.unwrap().max(ready);
        self.last = Some(now);
        (
            speed * now.saturating_duration_since(from).as_secs_f64().min(0.1),
            Some(Duration::from_millis(16)),
        )
    }
}
struct Service {
    hwnd: HWND,
    cfg: GesturesConfig,
    engine: Arc<RwLock<TilingEngine>>,
    backend: BackendHandle,
    source: Option<(u32, u32)>,
    point: Point,
    x: EdgeClock,
    y: EdgeClock,
    hover: Option<(Target, Instant, bool)>,
    steps: WorkspaceSteps,
    stopped_x: bool,
    stopped_y: bool,
}
thread_local! { static SERVICE: RefCell<Option<Service>> = const { RefCell::new(None) }; }
impl Service {
    fn stop_timer(&self) {
        unsafe {
            let _ = KillTimer(self.hwnd, 1);
        }
    }
    fn motion(&mut self, point: Point, source: Option<(u32, u32)>) {
        self.stop_timer();
        self.point = point;
        self.source = source;
        self.stopped_x = false;
        self.stopped_y = false;
        if source.is_none() {
            self.x = EdgeClock::default();
            self.y = EdgeClock::default();
            self.hover = None;
            self.steps.reset();
            return;
        }
        self.tick();
    }
    fn tick(&mut self) {
        self.stop_timer();
        let Some((tid, pid)) = self.source else {
            return;
        };
        if unsafe { GetAsyncKeyState(1) } as u16 & 0x8000 == 0
            || !unsafe { capture_matches(tid, pid) }
            || super::low_level_hook::is_grab_active()
        {
            self.source = None;
            self.hover = None;
            return;
        }
        let now = Instant::now();
        let mut e = self.engine.write();
        let context = e
            .monitors()
            .iter()
            .find(|(_, m)| m.bounds.contains_point(self.point))
            .map(|(o, m)| (*o, m.bounds, m.scale_factor));
        let Some((output, bounds, scale)) = context else {
            return;
        };
        e.set_focused_output(output);
        let sx = if self.stopped_x {
            0.0
        } else {
            edge_speed(
                self.point.x,
                bounds.loc.x,
                bounds.size.w,
                scale,
                &self.cfg.view,
            )
        };
        let sy = workspace_edge_speed(
            self.stopped_y,
            self.point.y,
            bounds.loc.y,
            bounds.size.h,
            &self.cfg.workspace,
        );
        let (dx, wx) = self.x.step(sx, now, self.cfg.view.delay_ms);
        let (dy, wy) = self.y.step(sy, now, self.cfg.workspace.delay_ms);
        let mut wait = wx.into_iter().chain(wy).min();
        if dx != 0.0 && !e.dnd_scroll(dx, &self.backend) {
            self.stopped_x = true;
            wait = None;
        }
        // One line per *applied* edge frame, so the live gate has evidence beyond rectangle
        // deltas: the direction it moved, and that the frames stop when the zone is left.
        if dx != 0.0 || dy != 0.0 {
            debug!(
                "dnd edge frame dir={} dx={dx:.1} dy={dy:.1}",
                axis_of(dx, dy)
            );
        }
        if sy == 0.0 {
            // Left the zone (or the strip ended): a partial step must not survive the hold.
            self.steps.reset();
        } else if let Some(direction) = self.steps.push(dy) {
            if !e.dnd_workspace_step(direction, &self.backend) {
                self.stopped_y = true;
                wait = None;
            } else {
                // Exactly one line per workspace step, never one per frame.
                debug!("dnd workspace step delta={direction}");
            }
        }
        let target = if sx == 0.0 && sy == 0.0 {
            e.dnd_target(self.point)
        } else {
            None
        };
        if self.hover.map(|h| h.0) != target {
            self.hover = target.map(|t| (t, now, false));
        }
        if let Some((target, entered, activated)) = self.hover {
            if !activated {
                let delay = Duration::from_millis(500)
                    .saturating_sub(now.saturating_duration_since(entered));
                if delay.is_zero() {
                    e.dnd_activate(target, &self.backend);
                    self.hover = Some((target, entered, true));
                } else {
                    wait = Some(wait.map_or(delay, |w| w.min(delay)));
                }
            }
        }
        drop(e);
        if let Some(wait) = wait {
            unsafe {
                SetTimer(self.hwnd, 1, wait.as_millis().clamp(1, 60000) as u32, None);
            }
        }
    }
}
pub fn configure(engine: Arc<RwLock<TilingEngine>>, backend: BackendHandle) {
    shutdown();
    let cfg = engine
        .read()
        .full_config()
        .map(|c| c.gestures.clone())
        .unwrap_or_default();
    if !cfg.enabled {
        return;
    }
    unsafe {
        let class = w!("WiriDragGestures");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: class,
            ..Default::default()
        });
        let Ok(hwnd) = CreateWindowExW(
            Default::default(),
            class,
            class,
            Default::default(),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            None,
            None,
        ) else {
            return;
        };
        SERVICE.with(|s| {
            *s.borrow_mut() = Some(Service {
                hwnd,
                cfg,
                engine,
                backend,
                source: None,
                point: Point::new(0, 0),
                x: EdgeClock::default(),
                y: EdgeClock::default(),
                hover: None,
                steps: WorkspaceSteps::default(),
                stopped_x: false,
                stopped_y: false,
            })
        });
        ENABLED.store(true, Ordering::Release);
    }
}
pub fn shutdown() {
    ENABLED.store(false, Ordering::Release);
    DETECTOR.lock().down = None;
    if let Some(s) = SERVICE.with(|s| s.borrow_mut().take()) {
        s.stop_timer();
        unsafe {
            let _ = DestroyWindow(s.hwnd);
        }
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_TIMER && wp.0 == 1 {
        SERVICE.with(|s| {
            if let Some(s) = s.borrow_mut().as_mut() {
                s.tick();
            }
        });
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wp, lp)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drag_needs_distance_held_foreign_capture_and_no_wiri_grab() {
        let a = Point::new(100, 100);
        let b = Point::new(130, 100);
        assert!(is_drag(a, b, true, true, false));
        for (held, capture, own) in [
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            assert!(!is_drag(a, b, held, capture, own));
        }
        assert!(!is_drag(a, Point::new(103, 102), true, true, false));
    }
    #[test]
    fn edge_speed_is_linear_signed_scaled_and_zero_outside_zone() {
        let cfg = GesturesConfig::default().view;
        assert_eq!(edge_speed(0, 0, 1000, 1.0, &cfg), -1500.0);
        assert_eq!(edge_speed(15, 0, 1000, 1.0, &cfg), -750.0);
        assert_eq!(edge_speed(500, 0, 1000, 1.0, &cfg), 0.0);
        assert_eq!(edge_speed(1000, 0, 1000, 1.0, &cfg), 1500.0);
        assert_eq!(edge_speed(-970, -1000, 2000, 2.0, &cfg), -1500.0);
    }
    #[test]
    fn workspace_edge_speed_is_live_outside_the_overview_and_stops_at_the_ends() {
        let cfg = GesturesConfig::default().workspace;
        // niri switches the workspace when a held drag reaches the top or bottom edge of the
        // screen; that gesture is not the overview's. Guarding it on the overview (as this
        // module did) made `dnd-edge-workspace-switch` dead in the normal scrolling view.
        assert!(workspace_edge_speed(false, 1195, 0, 1200, &cfg) > 0.0);
        assert!(workspace_edge_speed(false, 5, 0, 1200, &cfg) < 0.0);
        assert_eq!(workspace_edge_speed(false, 600, 0, 1200, &cfg), 0.0);
        // An exhausted strip (no workspace left in that direction) stops the axis.
        assert_eq!(workspace_edge_speed(true, 1195, 0, 1200, &cfg), 0.0);
        assert_eq!(workspace_edge_speed(true, 5, 0, 1200, &cfg), 0.0);
    }

    #[test]
    fn workspace_steps_are_directional_one_screen_apart_and_do_not_survive_leaving() {
        let mut s = WorkspaceSteps::default();
        // Holding still at the edge steps once per screen of travel, not once per frame.
        assert_eq!(s.push(700.0), None);
        assert_eq!(s.push(700.0), None);
        assert_eq!(s.push(700.0), Some(1));
        assert_eq!(
            s.push(-600.0),
            None,
            "the remainder is kept across the step"
        );
        assert_eq!(s.push(-1600.0), Some(-1));
        // A partial step cannot be carried across a leave-and-return of the zone.
        s.reset();
        assert_eq!(s.push(1400.0), None);
        s.reset();
        assert_eq!(s.push(1400.0), None);
        assert_eq!(s.push(100.0), Some(1));
    }

    #[test]
    fn edge_delay_resets_on_direction_change_and_no_idle_frame_is_requested() {
        let mut c = EdgeClock::default();
        let t = Instant::now();
        assert_eq!(c.step(0.0, t, 100), (0.0, None));
        assert_eq!(
            c.step(1000.0, t, 100),
            (0.0, Some(Duration::from_millis(100)))
        );
        assert_eq!(c.step(1000.0, t + Duration::from_millis(120), 100).0, 20.0);
        assert_eq!(c.step(-1000.0, t + Duration::from_millis(130), 100).0, 0.0);
        assert_eq!(
            c.step(0.0, t + Duration::from_millis(140), 100),
            (0.0, None)
        );
    }
}
