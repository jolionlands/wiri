//! Spatial gestures share Lane B's ordered dispatcher and Phase 2B's active-only clock.
//! The pointer follows 1:1; only the release has a timeline. Constants and projection
//! follow niri e19df12cf8's spatial_movement_grab, swipe_tracker and rubber_band.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use crate::backend::BackendHandle;
use crate::layout::TilingEngine;
use crate::utils::{OutputId, Point};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// A bounded 150 ms history, including the idle interval before release.
#[derive(Debug, Default, Clone)]
pub struct SwipeTracker {
    events: VecDeque<(Duration, f64)>,
    position: f64,
}
impl SwipeTracker {
    pub fn push(&mut self, delta: f64, time: Duration) {
        if !delta.is_finite() || self.events.back().is_some_and(|e| time < e.0) {
            return;
        }
        self.position += delta;
        self.events.push_back((time, delta));
        while self
            .events
            .front()
            .is_some_and(|e| time.saturating_sub(e.0) > Duration::from_millis(150))
        {
            self.events.pop_front();
        }
        // Bound even a malicious stream with identical timestamps.
        while self.events.len() > 512 {
            self.events.pop_front();
        }
    }
    pub fn position(&self) -> f64 {
        self.position
    }
    pub fn velocity(&self) -> f64 {
        let (Some(first), Some(last)) = (self.events.front(), self.events.back()) else {
            return 0.0;
        };
        let seconds = last.0.saturating_sub(first.0).as_secs_f64();
        if seconds == 0.0 {
            0.0
        } else {
            self.events.iter().map(|e| e.1).sum::<f64>() / seconds
        }
    }
    pub fn projected(&self) -> f64 {
        self.position - self.velocity() / (1000.0 * 0.997_f64.ln())
    }
}

/// niri's saturating rational band (stiffness 0.5, finite asymptote).
pub fn rubber_band(value: f64, min: f64, max: f64, limit: f64) -> f64 {
    let clamped = value.clamp(min, max);
    let diff = value - clamped;
    clamped + diff.signum() * limit * (1.0 - 1.0 / (diff.abs() * 0.5 / limit + 1.0))
}
pub fn rubber_derivative(value: f64, min: f64, max: f64, limit: f64) -> f64 {
    let diff = value - value.clamp(min, max);
    if diff == 0.0 {
        1.0
    } else {
        0.5 * limit.powi(2) / (0.5 * diff.abs() + limit).powi(2)
    }
}

#[derive(Debug, Default)]
pub struct SpatialGesture {
    pub axis: Option<Axis>,
    pub x: SwipeTracker,
    pub y: SwipeTracker,
}
impl SpatialGesture {
    pub fn update(&mut self, dx: f64, dy: f64, time: Duration) {
        self.x.push(dx, time);
        self.y.push(dy, time);
        if self.axis.is_none() && self.x.position().hypot(self.y.position()) >= 8.0 {
            self.axis = Some(if self.x.position().abs() > self.y.position().abs() {
                Axis::Horizontal
            } else {
                Axis::Vertical
            });
        }
    }
    pub fn release(&mut self, time: Duration) {
        self.update(0.0, 0.0, time);
    }
}

pub(crate) struct GestureContext {
    pub output: OutputId,
    pub workspace: i32,
    pub view: f64,
    pub width: f64,
    pub height: f64,
    pub snaps: Vec<(f64, usize)>,
    pub workspaces: Vec<i32>,
    pub workspace_index: usize,
}
impl GestureContext {
    fn range(&self) -> (f64, f64) {
        self.snaps
            .iter()
            .fold((self.view, self.view), |(lo, hi), s| {
                (lo.min(s.0), hi.max(s.0))
            })
    }
}
struct Session {
    context: GestureContext,
    gesture: SpatialGesture,
    cursor: Point,
    touchpad: bool,
}
#[derive(Clone)]
struct Service {
    engine: Arc<RwLock<TilingEngine>>,
    backend: BackendHandle,
}
static SERVICE: Mutex<Option<Service>> = Mutex::new(None);
static MIDDLE: AtomicBool = AtomicBool::new(false);
static CLOCK: LazyLock<Instant> = LazyLock::new(Instant::now);
thread_local! { static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) }; }

pub fn configure(engine: Arc<RwLock<TilingEngine>>, backend: BackendHandle) {
    cancel();
    *SERVICE.lock() = Some(Service { engine, backend });
}
pub fn shutdown() {
    cancel();
    SERVICE.lock().take();
}
pub fn is_active() -> bool {
    MIDDLE.load(Ordering::Acquire)
}

pub fn begin_middle(cursor: Point) -> bool {
    let Some(service) = SERVICE.lock().clone() else {
        return false;
    };
    MIDDLE.store(true, Ordering::Release);
    if !super::dispatch::send_work(Box::new(move || {
        begin(&service.engine, &service.backend, cursor, false)
    })) {
        MIDDLE.store(false, Ordering::Release);
        return false;
    }
    true
}
/// Only our motion and paired release. Never consume motion: Windows must move its cursor.
pub fn captured_event(message: u32, cursor: Point) -> Option<bool> {
    if !is_active() || !matches!(message, 0x200 | 0x208) {
        return None;
    }
    let Some(service) = SERVICE.lock().clone() else {
        return None;
    };
    let release = message == 0x208;
    let time = CLOCK.elapsed();
    if release {
        MIDDLE.store(false, Ordering::Release);
        super::pointer::ENGINE
            .lock()
            .button(0, 2, false, Instant::now());
    }
    super::dispatch::send_work(Box::new(move || {
        SESSION.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(s) = slot.as_mut().filter(|s| !s.touchpad) {
                let dx = (cursor.x - s.cursor.x) as f64;
                let dy = (cursor.y - s.cursor.y) as f64;
                s.cursor = cursor;
                s.gesture.update(dx, dy, time);
                display(s, &service.engine, &service.backend);
            }
        });
        if release {
            end(&service.engine, &service.backend, time, false);
        }
    }));
    Some(release)
}
fn begin(
    engine: &Arc<RwLock<TilingEngine>>,
    backend: &BackendHandle,
    point: Point,
    touchpad: bool,
) {
    end(engine, backend, CLOCK.elapsed(), true);
    let context = engine.write().gesture_begin(point);
    SESSION.with(|slot| {
        *slot.borrow_mut() = context.map(|context| Session {
            context,
            gesture: SpatialGesture::default(),
            cursor: point,
            touchpad,
        })
    });
}
fn display(s: &Session, engine: &Arc<RwLock<TilingEngine>>, backend: &BackendHandle) {
    let c = &s.context;
    let mut e = engine.write();
    match s.gesture.axis {
        Some(Axis::Horizontal) => {
            let (min, max) = c.range();
            let raw = c.view - s.gesture.x.position();
            e.gesture_view_update(
                c.output,
                c.workspace,
                rubber_band(raw, min, max, c.width * 0.5),
                backend,
            );
        }
        Some(Axis::Vertical) => {
            let max = (c.workspaces.len().saturating_sub(1)) as f64 * c.height;
            let raw = c.workspace_index as f64 * c.height - s.gesture.y.position();
            let value = rubber_band(raw, 0.0, max, c.height * 0.5);
            e.gesture_workspace_update(
                c.output,
                c.workspace_index as f64 * c.height - value,
                backend,
            );
        }
        None => {}
    }
}
fn end(
    engine: &Arc<RwLock<TilingEngine>>,
    backend: &BackendHandle,
    time: Duration,
    cancelled: bool,
) {
    let session = SESSION.with(|slot| slot.borrow_mut().take());
    let Some(mut s) = session else {
        return;
    };
    s.gesture.release(time);
    let c = s.context;
    let mut e = engine.write();
    match s.gesture.axis {
        Some(Axis::Horizontal) => {
            let (min, max) = c.range();
            let raw = c.view - s.gesture.x.position();
            let current = rubber_band(raw, min, max, c.width * 0.5);
            let projected = if cancelled {
                c.view
            } else {
                c.view - s.gesture.x.projected()
            };
            let target = c
                .snaps
                .iter()
                .min_by(|a, b| (a.0 - projected).abs().total_cmp(&(b.0 - projected).abs()))
                .copied();
            if let Some((target, column)) = target {
                let velocity = if cancelled {
                    0.0
                } else {
                    -s.gesture.x.velocity() * rubber_derivative(raw, min, max, c.width * 0.5)
                };
                e.gesture_view_end(
                    c.output,
                    c.workspace,
                    current,
                    target,
                    column,
                    velocity,
                    backend,
                );
            }
        }
        Some(Axis::Vertical) => {
            let max = (c.workspaces.len().saturating_sub(1)) as f64 * c.height;
            let raw = c.workspace_index as f64 * c.height - s.gesture.y.position();
            let value = rubber_band(raw, 0.0, max, c.height * 0.5);
            let projected = if cancelled {
                c.workspace_index as f64
            } else {
                (c.workspace_index as f64 - s.gesture.y.projected() / c.height)
                    .clamp(0.0, (c.workspaces.len() - 1) as f64)
            };
            let index = projected.round() as usize;
            let offset = index as f64 * c.height - value;
            let velocity = if cancelled {
                0.0
            } else {
                s.gesture.y.velocity() * rubber_derivative(raw, 0.0, max, c.height * 0.5)
            };
            e.gesture_workspace_end(c.output, c.workspaces[index], offset, velocity, backend);
        }
        None => e.gesture_end_idle(),
    }
    crate::ipc::notify_state_changed();
}
pub fn cancel() {
    MIDDLE.store(false, Ordering::Release);
    if let Some(service) = SERVICE.lock().clone() {
        end(&service.engine, &service.backend, CLOCK.elapsed(), true);
    }
}

/// Raw touchpad reports call these on the dispatcher thread. Deltas are in
/// logical gesture units (300 horizontal / 300 vertical = a full view/workspace).
pub fn external_begin(engine: &Arc<RwLock<TilingEngine>>, backend: &BackendHandle) {
    let mut position = windows::Win32::Foundation::POINT::default();
    let point = if unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut position) }
        .is_ok()
    {
        Point::new(position.x, position.y)
    } else {
        Point::new(0, 0)
    };
    begin(engine, backend, point, true);
}
pub fn external_update(
    engine: &Arc<RwLock<TilingEngine>>,
    backend: &BackendHandle,
    dx: f64,
    dy: f64,
    time: Duration,
) {
    SESSION.with(|slot| {
        if let Some(s) = slot.borrow_mut().as_mut().filter(|s| s.touchpad) {
            s.gesture.update(
                dx * s.context.width / 300.0,
                dy * s.context.height / 300.0,
                time,
            );
            display(s, engine, backend);
        }
    });
}
pub fn external_end(
    engine: &Arc<RwLock<TilingEngine>>,
    backend: &BackendHandle,
    time: Duration,
    cancelled: bool,
) {
    end(engine, backend, time, cancelled);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn axis_locks_after_eight_pixels_and_never_flips() {
        let mut g = SpatialGesture::default();
        g.update(3.0, 2.0, Duration::ZERO);
        assert_eq!(g.axis, None);
        g.update(6.0, 0.0, Duration::from_millis(10));
        assert_eq!(g.axis, Some(Axis::Horizontal));
        g.update(0.0, 100.0, Duration::from_millis(20));
        assert_eq!(g.axis, Some(Axis::Horizontal));
    }
    #[test]
    fn release_pause_removes_fling_and_timestamp_regressions_are_ignored() {
        let mut t = SwipeTracker::default();
        t.push(0.0, Duration::ZERO);
        t.push(50.0, Duration::from_millis(50));
        assert_eq!(t.velocity(), 1000.0);
        assert!((t.projected() - 382.83).abs() < 1.0);
        t.push(900.0, Duration::from_millis(1));
        assert_eq!(t.position(), 50.0);
        t.push(0.0, Duration::from_millis(210));
        assert_eq!(t.velocity(), 0.0);
        assert_eq!(t.projected(), 50.0);
    }
    #[test]
    fn rubber_band_is_bounded_symmetric_and_has_the_correct_derivative() {
        assert_eq!(rubber_band(50.0, 0.0, 100.0, 200.0), 50.0);
        for x in [1.0, 50.0, 5000.0] {
            let v = rubber_band(-x, 0.0, 100.0, 200.0);
            assert!(v < 0.0 && v > -200.0);
            assert!((rubber_band(100.0 + x, 0.0, 100.0, 200.0) - 100.0 + v).abs() < 1e-9);
            let numerical = (rubber_band(-x + 0.001, 0.0, 100.0, 200.0)
                - rubber_band(-x - 0.001, 0.0, 100.0, 200.0))
                / 0.002;
            assert!((numerical - rubber_derivative(-x, 0.0, 100.0, 200.0)).abs() < 1e-7);
        }
    }
}
