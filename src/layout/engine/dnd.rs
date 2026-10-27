//! Native DnD adapters; overview workspace hits use the overview's own configured stack.
use super::*;
use crate::input::dnd::Target;
impl TilingEngine {
    pub(crate) fn dnd_scroll(&mut self, dx: f64, backend: &BackendHandle) -> bool {
        let before = self
            .monitors
            .focused()
            .and_then(|m| m.workspace())
            .map(|w| w.scroll_offset.x);
        self.gesture_scroll_by(dx, backend);
        before
            != self
                .monitors
                .focused()
                .and_then(|m| m.workspace())
                .map(|w| w.scroll_offset.x)
    }
    pub(crate) fn dnd_workspace_step(&mut self, delta: i32, backend: &BackendHandle) -> bool {
        let Some(m) = self.monitors.focused() else {
            return false;
        };
        let ids = m.workspace_ids();
        let Some(index) = ids.iter().position(|id| *id == m.active_workspace) else {
            return false;
        };
        let next = (index as i32 + delta).clamp(0, ids.len().saturating_sub(1) as i32) as usize;
        if next == index {
            return false;
        }
        if let Some(m) = self.monitors.focused_mut() {
            m.switch_workspace(ids[next]);
        }
        self.apply_all(backend);
        crate::ipc::notify_state_changed();
        true
    }
    pub(crate) fn dnd_target(&self, point: Point) -> Option<Target> {
        if self.is_overview() {
            // The configured/animated overview stack, not a fixed half-scale copy of it.
            return self
                .overview_workspace_at(point)
                .map(|(output, id)| Target::Workspace(output, id));
        }
        let child = unsafe {
            windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(
                windows::Win32::Foundation::POINT {
                    x: point.x,
                    y: point.y,
                },
            )
        };
        let root = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                child,
                windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
            )
        };
        let id = WindowId::new(root.0 as isize);
        (self.tiled_windows.contains_key(&id) && !self.native_moves.contains_key(&id))
            .then_some(Target::Window(id))
    }
    pub(crate) fn dnd_activate(&mut self, target: Target, backend: &BackendHandle) {
        match target {
            Target::Workspace(output, ws) => {
                if !self.is_overview() {
                    return;
                }
                self.monitors.set_focused(output);
                if let Some(m) = self.monitors.get_mut(&output) {
                    m.switch_workspace(ws);
                }
                self.apply_all(backend);
            }
            Target::Window(id) => {
                if self.is_floating(id) {
                    self.focus_float(id, backend);
                } else if let Some((output, ws, _)) = self.locate_window(id) {
                    if !self
                        .monitors
                        .get(&output)
                        .is_some_and(|m| m.active_workspace == ws)
                    {
                        return;
                    }
                    self.monitors.set_focused(output);
                    if let Some(m) = self.monitors.get_mut(&output) {
                        m.set_focus(id);
                    }
                    self.activate_now(id, backend);
                }
            }
        }
        crate::ipc::notify_state_changed();
    }
}
