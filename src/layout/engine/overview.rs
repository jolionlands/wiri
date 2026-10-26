//! The overview is a camera over a vertically stacked set of normal workspace views.
//! It never reconfigures applications to thumbnail-sized rectangles.
use super::{AppliedState, BackendHandle, LayoutConfig, OverviewState, TilingEngine};
use crate::overlay::thumbnail_overview::{OverviewCommand, OverviewContext, OverviewWorkspace};
use crate::utils::{OutputId, Rect, WindowId};

const ZOOM: f64 = 0.5;
const WORKSPACE_GAP: i32 = 50;

fn zoom_rect(rect: Rect, view: Rect, section_y: i32) -> Rect {
    Rect::new(
        view.loc.x
            + (view.size.w as f64 * (1.0 - ZOOM) / 2.0).round() as i32
            + ((rect.loc.x - view.loc.x) as f64 * ZOOM).round() as i32,
        section_y + ((rect.loc.y - view.loc.y) as f64 * ZOOM).round() as i32,
        (rect.size.w as f64 * ZOOM).round().max(1.0) as u32,
        (rect.size.h as f64 * ZOOM).round().max(1.0) as u32,
    )
}

impl TilingEngine {
    pub(super) fn enter_workspace_overview(&mut self, backend: &BackendHandle) {
        self.cancel_composited_transitions(backend);
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let Some(sink) = self.thumbnail_overview.clone() else {
            return;
        };
        // Stop in-flight geometry before opening the camera; source surfaces keep normal size.
        self.animation.cancel_all();
        for wid in self.animating_rects.keys() {
            if let Some(state) = self.applied_state.get_mut(wid) {
                state.rect = Rect::default();
            }
        }
        self.animating_rects.clear();
        self.overview = Some(OverviewState {
            zoom: ZOOM,
            workspace_offsets: Vec::new(),
        });
        sink.enter();
        let sources: Vec<_> = self
            .monitors
            .get(&output)
            .into_iter()
            .flat_map(|m| m.workspaces.values())
            .flat_map(|ws| &ws.columns)
            .flat_map(|col| {
                col.visible_tile_indices()
                    .into_iter()
                    .map(|i| col.tiles[i].window_id)
            })
            .collect();
        for wid in sources {
            sink.register(wid, wid.as_isize());
            // Hidden HWNDs yield stale/blank DWM surfaces. Let them render behind the opaque
            // topmost host. SW_SHOWNOACTIVATE never steals keyboard focus.
            let _ = backend.show_window(wid.as_isize(), true);
            self.applied_state
                .entry(wid)
                .or_insert_with(AppliedState::unset)
                .visible = true;
        }
        self.apply_layout_for_monitor(output, backend);
    }

    pub(super) fn workspace_overview_scene(
        &self,
        output: OutputId,
        view: Rect,
        cfg: &LayoutConfig,
    ) -> Vec<(WindowId, Rect)> {
        let Some(monitor) = self.monitors.get(&output) else {
            return Vec::new();
        };
        let mut ids: Vec<_> = monitor.workspaces.keys().copied().collect();
        ids.sort_unstable();
        let active = ids
            .iter()
            .position(|id| *id == monitor.active_workspace)
            .unwrap_or(0) as i32;
        let height = (view.size.h as f64 * ZOOM).round() as i32;
        let stride = height + (WORKSPACE_GAP as f64 * ZOOM).round() as i32;
        let center_y = view.loc.y + (view.size.h as i32 - height) / 2;
        let mut workspaces = Vec::new();
        let mut positions = Vec::new();
        for (index, id) in ids.into_iter().enumerate() {
            let Some(ws) = monitor.workspaces.get(&id) else {
                continue;
            };
            let y = center_y + (index as i32 - active) * stride;
            workspaces.push(OverviewWorkspace {
                id,
                active: id == monitor.active_workspace,
                rect: zoom_rect(view, view, y),
            });
            let normal =
                self.calculate_positions_at_zoom(ws, view, 0, view.size.h as i32, cfg, 1.0);
            positions.extend(
                normal
                    .into_iter()
                    .map(|(wid, rect)| (wid, zoom_rect(rect, view, y))),
            );
        }
        if let Some(sink) = &self.thumbnail_overview {
            sink.set_context(OverviewContext {
                bounds: monitor.bounds,
                workspaces,
                focused: monitor.focus_window,
            });
        }
        positions
    }

    /// Called by the session-local host, never while holding a host mutex.
    pub fn overview_command(&mut self, command: OverviewCommand, backend: &BackendHandle) {
        if !self.is_overview() {
            return;
        }
        match command {
            OverviewCommand::SelectWindow(wid) => {
                if let Some((output, ws, _)) = self.locate_window(wid) {
                    self.monitors.set_focused(output);
                    if let Some(m) = self.monitors.get_mut(&output) {
                        m.switch_workspace(ws);
                        m.set_focus(wid);
                    }
                    self.exit_overview(backend);
                }
            }
            OverviewCommand::SelectWorkspace(ws) => {
                if let Some(m) = self.monitors.focused_mut() {
                    m.switch_workspace(ws);
                }
                self.exit_overview(backend);
            }
            OverviewCommand::Close => self.exit_overview(backend),
            OverviewCommand::PreviousWorkspace | OverviewCommand::NextWorkspace => {
                let Some(output) = self.monitors.focused_id() else {
                    return;
                };
                if let Some(m) = self.monitors.get_mut(&output) {
                    let mut ids: Vec<_> = m.workspaces.keys().copied().collect();
                    ids.sort_unstable();
                    let index = ids
                        .iter()
                        .position(|id| *id == m.active_workspace)
                        .unwrap_or(0);
                    let next = if command == OverviewCommand::NextWorkspace {
                        (index + 1).min(ids.len().saturating_sub(1))
                    } else {
                        index.saturating_sub(1)
                    };
                    if let Some(id) = ids.get(next) {
                        m.switch_workspace(*id);
                    }
                }
                self.apply_layout_for_monitor(output, backend);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overview_scales_half_and_preserves_normal_aspect_and_negative_origins() {
        let view = Rect::new(-1366, 0, 1366, 728);
        let tile = Rect::new(-1358, 8, 500, 700);
        assert_eq!(zoom_rect(tile, view, 182), Rect::new(-1020, 186, 250, 350));
        assert_eq!(zoom_rect(view, view, 182), Rect::new(-1024, 182, 683, 364));
    }
}
