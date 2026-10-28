//! Edge double-clicks use the system's time/rectangle, without an idle timer.
use parking_lot::Mutex;
use std::time::{Duration, Instant};

use crate::input::grab::{resize_edge_from_point, ResizeEdge};
use crate::utils::{Point, WindowId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeAction {
    ResetHeight,
    FullWidth,
}
#[derive(Default)]
pub struct EdgeClicks {
    last: Option<(WindowId, ResizeEdge, u8, Point, Instant)>,
}
impl EdgeClicks {
    pub fn press(
        &mut self,
        id: WindowId,
        edge: ResizeEdge,
        button: u8,
        point: Point,
        now: Instant,
        limits: (Duration, i32, i32),
    ) -> Option<EdgeAction> {
        let previous = self.last.replace((id, edge, button, point, now));
        if previous.is_some_and(|(w, e, b, p, t)| {
            w == id
                && e == edge
                && b == button
                && now.saturating_duration_since(t) <= limits.0
                && (p.x - point.x).abs() <= limits.1 / 2
                && (p.y - point.y).abs() <= limits.2 / 2
        }) {
            self.last = None;
            Some(match edge {
                ResizeEdge::Left | ResizeEdge::Right => EdgeAction::FullWidth,
                _ => EdgeAction::ResetHeight,
            })
        } else {
            None
        }
    }
}
static CLICKS: Mutex<EdgeClicks> = Mutex::new(EdgeClicks { last: None });

/// Called only after explicit binds have declined the down event.
pub fn edge_click(
    engine: &std::sync::Arc<parking_lot::RwLock<crate::layout::TilingEngine>>,
    backend: &crate::backend::BackendHandle,
    id: WindowId,
    point: Point,
    button: u8,
) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXDOUBLECLK, SM_CYDOUBLECLK,
    };
    let Some(e) = engine.try_read() else {
        return false;
    };
    if !e.tiled_windows().contains_key(&id) || e.is_floating(id) || e.is_fullscreen(id) {
        return false;
    }
    let Some(rect) = backend
        .get_window(id.as_isize())
        .and_then(|w| w.get_dwm_bounds())
    else {
        return false;
    };
    let Some(edge) = resize_edge_from_point(point, rect, e.config().border_width) else {
        return false;
    };
    drop(e);
    let limits = unsafe {
        (
            Duration::from_millis(GetDoubleClickTime() as u64),
            GetSystemMetrics(SM_CXDOUBLECLK),
            GetSystemMetrics(SM_CYDOUBLECLK),
        )
    };
    let action = CLICKS
        .lock()
        .press(id, edge, button, point, Instant::now(), limits);
    if let Some(action) = action {
        let engine = engine.clone();
        let backend = backend.clone();
        super::dispatch::send_work(Box::new(move || {
            engine.write().pointer_edge_action(id, action, &backend);
            crate::ipc::notify_state_changed();
        }));
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn double_click_matches_window_button_edge_time_and_distance() {
        let mut c = EdgeClicks::default();
        let now = Instant::now();
        let id = WindowId::new(7);
        let p = Point::new(100, 200);
        let limits = (Duration::from_millis(500), 8, 8);
        assert_eq!(c.press(id, ResizeEdge::Top, 1, p, now, limits), None);
        assert_eq!(
            c.press(
                id,
                ResizeEdge::Top,
                1,
                p,
                now + Duration::from_millis(200),
                limits
            ),
            Some(EdgeAction::ResetHeight)
        );
        assert_eq!(c.press(id, ResizeEdge::Right, 1, p, now, limits), None);
        assert_eq!(
            c.press(id, ResizeEdge::Right, 1, Point::new(110, 200), now, limits),
            None
        );
        assert_eq!(
            c.press(
                id,
                ResizeEdge::Right,
                1,
                Point::new(110, 200),
                now + Duration::from_millis(600),
                limits
            ),
            None
        );
        assert_eq!(
            c.press(
                id,
                ResizeEdge::Right,
                0,
                Point::new(110, 200),
                now + Duration::from_millis(700),
                limits
            ),
            None
        );
        assert_eq!(
            c.press(
                id,
                ResizeEdge::Right,
                0,
                Point::new(110, 200),
                now + Duration::from_millis(800),
                limits
            ),
            Some(EdgeAction::FullWidth)
        );
    }
}
