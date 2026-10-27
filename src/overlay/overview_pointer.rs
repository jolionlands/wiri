//! Pure overview pointer state: click vs drag, insertion gaps, and high-resolution wheel.
use super::thumbnail_overview::{OverviewCommand, OverviewContext};
use crate::utils::{Rect, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    Workspace(i32),
    /// Before this workspace; None means below the last workspace.
    InsertBefore(Option<i32>),
}

pub fn drop_target(context: &OverviewContext, x: i32, y: i32) -> Option<DropTarget> {
    if !super::thumbnail_overview::contains(context.bounds, x, y) {
        return None;
    }
    let mut workspaces: Vec<_> = context.workspaces.iter().collect();
    workspaces.sort_by_key(|ws| ws.rect.loc.y);
    for ws in workspaces {
        if y < ws.rect.loc.y {
            return Some(DropTarget::InsertBefore(Some(ws.id)));
        }
        if y < ws.rect.bottom() {
            return Some(DropTarget::Workspace(ws.id));
        }
    }
    Some(DropTarget::InsertBefore(None))
}

/// How long a touch must rest on a window before it picks the window up (niri: long press).
pub const TOUCH_LONG_PRESS_MS: u32 = 500;
/// Vertical finger travel, in pixels, that steps one workspace.
const TOUCH_WORKSPACE_STEP: i32 = 150;

/// A touch contact that Windows promoted to mouse input (the primary finger only). One
/// finger pans the overview like niri's touch overview; resting without moving picks the
/// pressed window up so the same drag-and-drop path moves it.
#[derive(Default)]
struct Touch {
    last: (i32, i32),
    panning: bool,
    long: bool,
    vertical: i32,
}

#[derive(Default)]
pub struct Pointer {
    pressed: Option<(OverviewCommand, i32, i32)>,
    current: (i32, i32),
    dragging: bool,
    right: Option<i32>,
    wheel_remainder: i32,
    anchor: Option<Rect>,
    touch: Option<Touch>,
}
impl Pointer {
    pub fn press(&mut self, command: OverviewCommand, x: i32, y: i32) {
        self.pressed = Some((command, x, y));
        self.current = (x, y);
        self.dragging = false;
        self.anchor = None;
        self.touch = None;
    }
    /// A press from a promoted touch contact. The caller arms a one-shot
    /// [`TOUCH_LONG_PRESS_MS`] timer for it and calls [`Self::long_press`] when it fires.
    pub fn press_touch(&mut self, command: OverviewCommand, x: i32, y: i32) {
        self.press(command, x, y);
        self.touch = Some(Touch {
            last: (x, y),
            ..Touch::default()
        });
    }
    /// The long-press timer fired. True when the touched window was picked up (the caller
    /// repaints the drag image); false when the touch already moved, ended or hit no window.
    pub fn long_press(&mut self) -> bool {
        let on_window = matches!(self.pressed, Some((OverviewCommand::SelectWindow(_), _, _)));
        match &mut self.touch {
            Some(touch) if on_window && !touch.panning && !touch.long => {
                touch.long = true;
                self.dragging = true;
                true
            }
            _ => false,
        }
    }
    /// True while the current press is a touch that is still a tap or a long press
    /// candidate, i.e. the long-press timer is worth keeping armed.
    pub fn touch_pending(&self) -> bool {
        self.touch.as_ref().is_some_and(|t| !t.panning && !t.long)
    }
    pub fn set_anchor(&mut self, rect: Option<Rect>) {
        self.anchor = rect;
    }
    pub fn move_to(&mut self, x: i32, y: i32) -> Option<OverviewCommand> {
        self.current = (x, y);
        if let Some((_, ox, oy)) = self.pressed {
            self.dragging |= i64::from(x - ox).pow(2) + i64::from(y - oy).pow(2) >= 64;
        }
        if let Some(last) = &mut self.right {
            let delta = last.saturating_sub(x);
            *last = x;
            return Some(OverviewCommand::ScrollView(delta));
        }
        if let (Some(touch), Some((_, ox, oy))) = (&mut self.touch, self.pressed) {
            if touch.long {
                return None;
            }
            if !touch.panning && i64::from(x - ox).pow(2) + i64::from(y - oy).pow(2) >= 64 {
                touch.panning = true;
                touch.last = (x, y);
                return None;
            }
            if touch.panning {
                let (dx, dy) = (touch.last.0 - x, y - touch.last.1);
                touch.last = (x, y);
                touch.vertical = touch.vertical.saturating_add(dy);
                if dx != 0 && dx.abs() >= dy.abs() {
                    return Some(OverviewCommand::ScrollView(dx));
                }
                if touch.vertical.abs() >= TOUCH_WORKSPACE_STEP {
                    // Dragging the content down reveals the workspace above.
                    let down = touch.vertical > 0;
                    touch.vertical -= TOUCH_WORKSPACE_STEP * touch.vertical.signum();
                    return Some(if down {
                        OverviewCommand::PreviousWorkspace
                    } else {
                        OverviewCommand::NextWorkspace
                    });
                }
                if dx != 0 {
                    return Some(OverviewCommand::ScrollView(dx));
                }
            }
        }
        None
    }
    pub fn motion(&mut self, x: i32, y: i32) -> Option<OverviewCommand> {
        self.move_to(x, y)
    }
    pub fn right_press(&mut self, x: i32) {
        self.right = Some(x);
    }
    pub fn right_release(&mut self) {
        self.right = None;
    }
    pub fn dragging_window(&self) -> Option<(WindowId, i32, i32)> {
        // A touch pans the overview; only a long press picks a window up.
        let may_drag = self.touch.as_ref().map_or(true, |touch| touch.long);
        match self.pressed {
            Some((OverviewCommand::SelectWindow(id), x, y)) if self.dragging && may_drag => {
                Some((id, self.current.0 - x, self.current.1 - y))
            }
            _ => None,
        }
    }
    pub fn release(
        &mut self,
        context: Option<&OverviewContext>,
        x: i32,
        y: i32,
    ) -> Option<OverviewCommand> {
        self.motion(x, y);
        let dragged = self.dragging_window();
        let pressed = self.pressed.take();
        let panned = self.touch.take().is_some_and(|touch| touch.panning);
        self.dragging = false;
        if let Some((window, _, _)) = dragged {
            return context
                .and_then(|c| drop_target(c, x, y))
                .map(|target| OverviewCommand::DropWindow { window, target });
        }
        if panned {
            // A pan is never a tap: lifting the finger must not select what it started on.
            return None;
        }
        pressed.map(|(command, _, _)| command)
    }
    pub fn cancel(&mut self) {
        self.pressed = None;
        self.right = None;
        self.dragging = false;
        self.touch = None;
    }
    pub fn wheel(&mut self, delta: i32) -> Vec<OverviewCommand> {
        self.wheel_remainder = self.wheel_remainder.saturating_add(delta);
        let steps = (self.wheel_remainder / 120).clamp(-32, 32);
        self.wheel_remainder %= 120;
        (0..steps.unsigned_abs())
            .map(|_| {
                if steps < 0 {
                    OverviewCommand::NextWorkspace
                } else {
                    OverviewCommand::PreviousWorkspace
                }
            })
            .collect()
    }
    pub fn dragged_rect(&self, id: WindowId, mut rect: Rect) -> Rect {
        if let Some((window, dx, dy)) = self.dragging_window() {
            if window == id {
                rect = self.anchor.unwrap_or(rect);
                rect.loc.x = rect.loc.x.saturating_add(dx);
                rect.loc.y = rect.loc.y.saturating_add(dy);
            }
        }
        rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::thumbnail_overview::OverviewWorkspace;
    fn context() -> OverviewContext {
        OverviewContext {
            bounds: Rect::new(-100, 0, 800, 800),
            focused: None,
            workspaces: vec![
                OverviewWorkspace::new(0, Rect::new(100, 100, 400, 250), true),
                OverviewWorkspace::new(1, Rect::new(100, 400, 400, 250), false),
            ],
        }
    }
    #[test]
    fn drop_distinguishes_workspaces_and_all_three_insertion_regions() {
        let c = context();
        for (y, target) in [
            (50, DropTarget::InsertBefore(Some(0))),
            (200, DropTarget::Workspace(0)),
            (375, DropTarget::InsertBefore(Some(1))),
            (500, DropTarget::Workspace(1)),
            (700, DropTarget::InsertBefore(None)),
        ] {
            assert_eq!(drop_target(&c, 300, y), Some(target));
        }
        assert_eq!(drop_target(&c, -101, 50), None);
    }
    #[test]
    fn click_does_not_move_and_real_drag_does_not_close() {
        let id = WindowId::new(5);
        let mut p = Pointer::default();
        p.press(OverviewCommand::SelectWindow(id), 300, 200);
        assert_eq!(
            p.release(Some(&context()), 303, 202),
            Some(OverviewCommand::SelectWindow(id))
        );
        p.press(OverviewCommand::SelectWindow(id), 300, 200);
        p.motion(300, 500);
        assert_eq!(
            p.release(Some(&context()), 300, 500),
            Some(OverviewCommand::DropWindow {
                window: id,
                target: DropTarget::Workspace(1)
            })
        );
        assert!(p.dragging_window().is_none());
    }
    #[test]
    fn dragged_window_follows_pointer_when_workspace_camera_changes() {
        let id = WindowId::new(9);
        let mut p = Pointer::default();
        p.press(OverviewCommand::SelectWindow(id), 100, 100);
        p.set_anchor(Some(Rect::new(80, 80, 200, 200)));
        p.motion(200, 300);
        assert_eq!(
            p.dragged_rect(id, Rect::new(80, -100, 200, 200)),
            Rect::new(180, 280, 200, 200)
        );
        p.cancel();
        assert_eq!(
            p.dragged_rect(id, Rect::new(80, -100, 200, 200)),
            Rect::new(80, -100, 200, 200)
        );
    }
    #[test]
    fn touch_tap_selects_but_a_pan_scrolls_and_never_selects_on_release() {
        let id = WindowId::new(5);
        let mut p = Pointer::default();
        p.press_touch(OverviewCommand::SelectWindow(id), 300, 200);
        assert!(p.touch_pending());
        // Inside the slop it is still a tap.
        assert_eq!(p.motion(303, 202), None);
        assert_eq!(
            p.release(Some(&context()), 303, 202),
            Some(OverviewCommand::SelectWindow(id))
        );
        // Past the slop the finger pans (dragging left scrolls the view right) and the
        // window is not picked up, so lifting the finger over it selects nothing.
        p.press_touch(OverviewCommand::SelectWindow(id), 300, 200);
        assert_eq!(
            p.motion(280, 200),
            None,
            "crossing the slop only arms the pan"
        );
        assert!(!p.touch_pending());
        assert_eq!(p.motion(250, 200), Some(OverviewCommand::ScrollView(30)));
        assert!(p.dragging_window().is_none());
        assert!(
            !p.long_press(),
            "a moving finger cannot become a long press"
        );
        assert_eq!(p.release(Some(&context()), 250, 500), None);
    }

    #[test]
    fn touch_vertical_pan_steps_workspaces_per_150_pixels() {
        let mut p = Pointer::default();
        p.press_touch(OverviewCommand::SelectWorkspace(0), 300, 100);
        assert_eq!(p.motion(300, 110), None);
        assert_eq!(p.motion(300, 200), None);
        assert_eq!(p.motion(300, 270), Some(OverviewCommand::PreviousWorkspace));
        // The remainder carries over; moving back up steps the other way.
        assert_eq!(p.motion(300, 100), Some(OverviewCommand::NextWorkspace));
        assert_eq!(p.release(Some(&context()), 300, 100), None);
    }

    #[test]
    fn touch_long_press_picks_a_window_up_and_the_drop_uses_the_mouse_path() {
        let id = WindowId::new(7);
        let mut p = Pointer::default();
        p.press_touch(OverviewCommand::SelectWindow(id), 300, 200);
        assert!(p.long_press());
        assert!(!p.long_press(), "the timer fires once");
        assert!(!p.touch_pending());
        assert_eq!(p.dragging_window(), Some((id, 0, 0)));
        // While held it does not pan.
        assert_eq!(p.motion(300, 500), None);
        assert_eq!(
            p.release(Some(&context()), 300, 500),
            Some(OverviewCommand::DropWindow {
                window: id,
                target: DropTarget::Workspace(1)
            })
        );
        // Neither an empty workspace nor an ended touch can be picked up.
        p.press_touch(OverviewCommand::SelectWorkspace(1), 300, 500);
        assert!(!p.long_press());
        p.cancel();
        assert!(!p.long_press());
        p.press(OverviewCommand::SelectWindow(id), 300, 200);
        assert!(!p.long_press(), "a mouse press has no long-press pickup");
        assert!(!p.touch_pending());
    }

    #[test]
    fn wheel_accumulates_and_right_drag_alone_scrolls_view() {
        let mut p = Pointer::default();
        assert!(p.wheel(-60).is_empty());
        assert_eq!(p.wheel(-60), vec![OverviewCommand::NextWorkspace]);
        assert_eq!(p.wheel(240).len(), 2);
        assert_eq!(p.motion(40, 40), None);
        assert_eq!(p.motion(80, 80), None);
        p.right_press(80);
        assert_eq!(p.motion(100, 80), Some(OverviewCommand::ScrollView(-20)));
        p.cancel();
        assert_eq!(p.motion(200, 80), None);
    }
}
