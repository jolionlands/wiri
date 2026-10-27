//! Interactive grabs share native-loop exclusion and W's insertion transaction.
use super::*;
use crate::input::interactive::EdgeAction;

impl TilingEngine {
    pub fn pointer_edge_action(
        &mut self,
        id: WindowId,
        action: EdgeAction,
        backend: &BackendHandle,
    ) {
        let Some((output, workspace, _)) = self.locate_window(id) else {
            return;
        };
        if !self
            .monitors
            .get(&output)
            .is_some_and(|m| m.active_workspace == workspace)
            || self.is_floating(id)
            || self.is_fullscreen(id)
        {
            return;
        }
        self.monitors.set_focused(output);
        if let Some(m) = self.monitors.get_mut(&output) {
            m.set_focus(id);
        }
        match action {
            EdgeAction::ResetHeight => self.reset_window_height(backend),
            EdgeAction::FullWidth => self.toggle_maximize_focused_column(backend),
        }
    }

    pub fn pointer_resize_end(&mut self, id: WindowId, rect: Rect, backend: &BackendHandle) {
        self.finish_native_move(id, Some(rect));
        self.sync_native_clip(id);
        self.apply_all(backend);
    }

    /// Promoted touch/pen drops use the same destination as mouse insertion.
    /// Never introduce another insert-hint owner or source-column stride heuristic.
    pub fn pointer_move_end(
        &mut self,
        id: WindowId,
        point: Point,
        rect: Rect,
        backend: &BackendHandle,
    ) {
        if self.is_floating(id) {
            self.clear_window_insert(id);
            self.place_float_from_pointer(id, rect, backend);
        } else if !self.finish_window_insert(id, point, backend) {
            self.cancel_window_insert(id, backend);
        }
    }
}
