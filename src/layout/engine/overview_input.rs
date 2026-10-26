//! Overview commands act on the normal workspace model, never on thumbnail HWNDs.
use super::{AppliedState, BackendHandle, TilingEngine};
use crate::overlay::overview_pointer::DropTarget;
use crate::utils::{OutputId, WindowId};

impl TilingEngine {
    pub(super) fn refresh_overview_sources(&mut self, output: OutputId, backend: &BackendHandle) {
        let Some(sink) = self.thumbnail_overview.clone() else {
            return;
        };
        // Only one session-local host is currently supported (the focused output).
        if Some(output) != self.monitors.focused_id() {
            return;
        }
        let mut sources: Vec<_> = self
            .monitors
            .get(&output)
            .into_iter()
            .flat_map(|m| m.workspaces.values())
            .flat_map(|ws| &ws.columns)
            .flat_map(|c| {
                c.visible_tile_indices()
                    .into_iter()
                    .map(|i| c.tiles[i].window_id)
            })
            .collect();
        sources.extend(
            self.float_homes
                .iter()
                .filter(|(_, h)| h.output == output)
                .map(|(id, _)| *id),
        );
        sink.retain_windows(&sources);
        for wid in sources {
            sink.register(wid, wid.as_isize());
            if self.applied_state.get(&wid).is_none_or(|s| !s.visible) {
                let _ = backend.show_window(wid.as_isize(), true);
                self.applied_state
                    .entry(wid)
                    .or_insert_with(AppliedState::unset)
                    .visible = true;
            }
        }
    }

    pub(super) fn switch_workspace_in_overview(
        &mut self,
        workspace: i32,
        backend: &BackendHandle,
    ) -> bool {
        if !self.is_overview() || self.thumbnail_overview.is_none() {
            return false;
        }
        let Some(output) = self.monitors.focused_id() else {
            return true;
        };
        if let Some(m) = self.monitors.get_mut(&output) {
            m.switch_workspace(workspace);
        }
        // Do not hide sources or slide real application HWNDs under the camera.
        self.apply_layout_for_monitor(output, backend);
        true
    }

    pub(super) fn overview_scroll_view(&mut self, delta: i32, backend: &BackendHandle) {
        let delta = (delta as f64 / self.overview_zoom().max(0.01)).round() as i32;
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let cfg = self.effective_config(output).clone();
        if let Some(m) = self.monitors.get_mut(&output) {
            let width = Self::view_width_for_monitor(m, &cfg);
            let col_width = m
                .workspace()
                .map(|ws| Self::workspace_default_width(ws, width, &cfg))
                .unwrap_or(1);
            if let Some(ws) = m.workspace_mut() {
                ws.scroll_offset.x = ws.scroll_offset.x.saturating_add(delta);
                ws.clamp_scroll(col_width, cfg.column_gap, width);
            }
        }
        self.apply_layout_for_monitor(output, backend);
    }

    /// Insert by rotating a new empty workspace through the existing ordered places.
    /// This deliberately reuses the engine's workspace move/tie model (including floats,
    /// minimized slots, remembered activity and single-output names).
    fn insert_overview_workspace(&mut self, output: OutputId, before: Option<i32>) -> Option<i32> {
        let m = self.monitors.get_mut(&output)?;
        let mut ids = m.workspace_ids();
        let new_id = ids.last().copied().unwrap_or(-1).checked_add(1)?;
        let index = match before {
            Some(id) => ids.iter().position(|w| *w == id)?,
            None => ids.len(),
        };
        m.workspaces.insert(new_id, crate::layout::Workspace::new());
        ids.push(new_id);
        for i in (index..ids.len() - 1).rev() {
            let (a, b) = (ids[i], ids[i + 1]);
            let m = self.monitors.get_mut(&output)?;
            let left = m.workspaces.remove(&a)?;
            let right = m.workspaces.remove(&b)?;
            m.workspaces.insert(a, right);
            m.workspaces.insert(b, left);
            let swap = |id: i32| {
                if id == a {
                    b
                } else if id == b {
                    a
                } else {
                    id
                }
            };
            m.active_workspace = swap(m.active_workspace);
            m.previous_workspace = m.previous_workspace.map(swap);
            self.swap_workspace_ties(output, a, b);
        }
        ids.get(index).copied()
    }

    pub(super) fn overview_drop_window(
        &mut self,
        window: WindowId,
        target: DropTarget,
        backend: &BackendHandle,
    ) {
        let source = self
            .locate_window(window)
            .map(|(o, w, _)| (o, w))
            .or_else(|| {
                self.float_homes
                    .get(&window)
                    .map(|h| (h.output, h.workspace))
            });
        let Some((output, _)) = source else {
            return;
        };
        let destination = match target {
            DropTarget::Workspace(id)
                if self
                    .monitors
                    .get(&output)
                    .is_some_and(|m| m.workspaces.contains_key(&id)) =>
            {
                id
            }
            DropTarget::Workspace(_) => return,
            DropTarget::InsertBefore(before) => {
                match self.insert_overview_workspace(output, before) {
                    Some(id) => id,
                    None => return,
                }
            }
        };
        if let Some(home) = self.float_homes.get_mut(&window) {
            home.workspace = destination;
        } else {
            let Some((_, source, _)) = self.locate_window(window) else {
                return;
            };
            if source != destination {
                let slot = self
                    .monitors
                    .get(&output)
                    .and_then(|m| m.workspaces.get(&source))
                    .and_then(|ws| ws.slot_of(window));
                let Some(slot) = slot else {
                    return;
                };
                if let Some(m) = self.monitors.get_mut(&output) {
                    m.remove_window(window);
                    m.workspaces
                        .entry(destination)
                        .or_default()
                        .insert_slot_after_active(slot);
                }
            }
        }
        self.monitors.set_focused(output);
        if let Some(m) = self.monitors.get_mut(&output) {
            m.switch_workspace(destination);
            m.set_focus(window);
        }
        self.sync_min_widths();
        self.maintain_empty_workspace(output);
        self.apply_layout_for_monitor(output, backend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Column, Monitor, Tile, Workspace};
    use crate::overlay::MockThumbnailOverview;
    use crate::utils::Rect;
    fn engine() -> (TilingEngine, OutputId) {
        let output = OutputId::from_name("overview-input");
        let mut e = TilingEngine::new(Default::default());
        let mut m = Monitor::new(
            output,
            Rect::new(0, 0, 1366, 768),
            Rect::new(0, 0, 1366, 728),
        );
        for ws in 0..2 {
            let mut w = Workspace::new();
            for n in 1..3 {
                let mut col = Column::new();
                col.add_tile(Tile::new(WindowId::new(ws * 2 + n)));
                w.columns.push(col);
            }
            m.workspaces.insert(ws as i32, w);
        }
        m.set_focus(WindowId::new(1));
        e.monitors.insert(output, m);
        e.monitors.set_focused(output);
        e.install_thumbnail_overview(MockThumbnailOverview::new());
        (e, output)
    }
    #[test]
    fn drag_to_existing_workspace_retains_window_slot_and_overview() {
        let (mut e, o) = engine();
        let b = BackendHandle::default_for_test();
        e.enter_overview(&b);
        e.overview_drop_window(WindowId::new(1), DropTarget::Workspace(1), &b);
        assert_eq!(e.locate_window(WindowId::new(1)).unwrap().1, 1);
        assert_eq!(e.monitors.get(&o).unwrap().active_workspace, 1);
        assert!(e.is_overview());
    }
    #[test]
    fn insertion_above_between_and_below_preserves_order_and_selection() {
        for (before, expected) in [(Some(0), 0), (Some(1), 1), (None, 2)] {
            let (mut e, o) = engine();
            let b = BackendHandle::default_for_test();
            e.enter_overview(&b);
            e.overview_drop_window(WindowId::new(1), DropTarget::InsertBefore(before), &b);
            assert_eq!(e.locate_window(WindowId::new(1)).unwrap().1, expected);
            assert_eq!(
                e.monitors.get(&o).unwrap().focus_window,
                Some(WindowId::new(1))
            );
            assert_eq!(
                e.locate_window(WindowId::new(3)).unwrap().1,
                if before.is_some() { 2 } else { 1 }
            );
            assert!(e.is_overview());
        }
    }
    #[test]
    fn overview_refresh_tracks_windows_opened_closed_and_tabs_selected_by_binds() {
        let (mut e, o) = engine();
        let b = BackendHandle::default_for_test();
        let sink = MockThumbnailOverview::new();
        e.install_thumbnail_overview(sink.clone());
        e.enter_overview(&b);
        assert_eq!(sink.registered_count(), 4);
        let m = e.monitors.get_mut(&o).unwrap();
        m.remove_window(WindowId::new(1));
        let mut c = Column::new();
        c.add_tile(Tile::new(WindowId::new(10)));
        m.workspace_mut().unwrap().columns.push(c);
        e.apply_layout_for_monitor(o, &b);
        assert_eq!(sink.registered_count(), 4);
        assert!(sink.last_dst_rect(WindowId::new(1)).is_none());
        assert!(sink.last_dst_rect(WindowId::new(10)).is_some());
        assert!(e.is_overview());
    }
    #[test]
    fn workspace_insertion_carries_names_with_their_content() {
        let (mut e, o) = engine();
        e.workspace_names.insert(0, "first".into());
        e.workspace_names.insert(1, "second".into());
        assert_eq!(e.insert_overview_workspace(o, Some(0)), Some(0));
        assert_eq!(e.workspace_name(0), None);
        assert_eq!(e.workspace_name(1).as_deref(), Some("first"));
        assert_eq!(e.workspace_name(2).as_deref(), Some("second"));
        assert_eq!(e.monitors.get(&o).unwrap().active_workspace, 1);
    }
    #[test]
    fn right_scroll_and_normal_workspace_bind_keep_camera_open() {
        let (mut e, o) = engine();
        let b = BackendHandle::default_for_test();
        e.enter_overview(&b);
        e.overview_scroll_view(30, &b);
        assert_eq!(
            e.monitors
                .get(&o)
                .unwrap()
                .workspace()
                .unwrap()
                .scroll_offset
                .x,
            60
        );
        e.switch_workspace(1, &b);
        assert!(e.is_overview());
        assert_eq!(e.monitors.get(&o).unwrap().active_workspace, 1);
    }
}
