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

#[derive(Default)]
pub struct Pointer {
    pressed: Option<(OverviewCommand, i32, i32)>,
    current: (i32, i32),
    dragging: bool,
    right: Option<i32>,
    wheel_remainder: i32,
    anchor: Option<Rect>,
}
impl Pointer {
    pub fn press(&mut self, command: OverviewCommand, x: i32, y: i32) {
        self.pressed = Some((command, x, y));
        self.current = (x, y);
        self.dragging = false;
        self.anchor = None;
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
        match self.pressed {
            Some((OverviewCommand::SelectWindow(id), x, y)) if self.dragging => {
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
        self.dragging = false;
        if let Some((window, _, _)) = dragged {
            return context
                .and_then(|c| drop_target(c, x, y))
                .map(|target| OverviewCommand::DropWindow { window, target });
        }
        pressed.map(|(command, _, _)| command)
    }
    pub fn cancel(&mut self) {
        self.pressed = None;
        self.right = None;
        self.dragging = false;
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
                OverviewWorkspace {
                    id: 0,
                    rect: Rect::new(100, 100, 400, 250),
                    active: true,
                },
                OverviewWorkspace {
                    id: 1,
                    rect: Rect::new(100, 400, 400, 250),
                    active: false,
                },
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
