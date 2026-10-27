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

    /// Insert a new empty workspace at the visible place `before` names (`None`: below the
    /// last one). Workspaces are stable objects: their names, IPC ids, floating homes,
    /// minimized slots and remembered activity are all keyed by the workspace key, so
    /// nothing is rotated. The new object gets a fresh key and only the visible order moves.
    fn insert_overview_workspace(&mut self, output: OutputId, before: Option<i32>) -> Option<i32> {
        let m = self.monitors.get_mut(&output)?;
        let index = match before {
            Some(id) => m.workspace_ids().iter().position(|w| *w == id)?,
            None => m.workspace_ids().len(),
        };
        let id = m.next_workspace_id();
        if m.workspaces.contains_key(&id) {
            return None;
        }
        m.insert_workspace(id, crate::layout::Workspace::new());
        m.move_workspace_to_position(id, index);
        Some(id)
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
    /// The visible 0-based place of the workspace `window` is on.
    fn place_of(e: &TilingEngine, o: OutputId, window: isize) -> usize {
        let (_, workspace, _) = e.locate_window(WindowId::new(window)).unwrap();
        let ids = e.monitors.get(&o).unwrap().workspace_ids();
        ids.iter().position(|id| *id == workspace).unwrap()
    }
    #[test]
    fn insertion_above_between_and_below_preserves_order_and_selection() {
        // The dropped window ends alone on the new workspace at the requested visible
        // place; the old workspaces keep their keys and their relative order.
        for (before, expected) in [(Some(0), 0), (Some(1), 1), (None, 2)] {
            let (mut e, o) = engine();
            let b = BackendHandle::default_for_test();
            e.enter_overview(&b);
            e.overview_drop_window(WindowId::new(1), DropTarget::InsertBefore(before), &b);
            assert_eq!(place_of(&e, o, 1), expected);
            assert_eq!(
                e.monitors.get(&o).unwrap().focus_window,
                Some(WindowId::new(1))
            );
            // Window 3 stays on workspace key 1; window 2 stays on workspace key 0.
            assert_eq!(e.locate_window(WindowId::new(2)).unwrap().1, 0);
            assert_eq!(e.locate_window(WindowId::new(3)).unwrap().1, 1);
            assert_eq!(place_of(&e, o, 3), if before.is_some() { 2 } else { 1 });
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
    fn workspace_insertion_keeps_names_ids_and_activity_with_their_workspace() {
        let (mut e, o) = engine();
        for (id, name) in [(0, "first"), (1, "second")] {
            let m = e.monitors.get_mut(&o).unwrap();
            m.workspaces.get_mut(&id).unwrap().name = Some(name.into());
        }
        let (ipc0, ipc1) = {
            let m = e.monitors.get(&o).unwrap();
            (m.workspaces[&0].ipc_id, m.workspaces[&1].ipc_id)
        };
        let new = e.insert_overview_workspace(o, Some(0)).unwrap();
        assert!(new != 0 && new != 1);
        assert_eq!(e.workspace_name_on(o, new), None);
        assert_eq!(e.workspace_name_on(o, 0).as_deref(), Some("first"));
        assert_eq!(e.workspace_name_on(o, 1).as_deref(), Some("second"));
        let m = e.monitors.get(&o).unwrap();
        // Only the visible order changed; each workspace keeps its key and IPC id.
        assert_eq!(m.workspace_ids(), vec![new, 0, 1]);
        assert_eq!(m.workspaces[&0].ipc_id, ipc0);
        assert_eq!(m.workspaces[&1].ipc_id, ipc1);
        assert!(m.workspaces[&new].ipc_id != ipc0 && m.workspaces[&new].ipc_id != ipc1);
        assert_eq!(m.active_workspace, 0);
        assert_eq!(e.insert_overview_workspace(o, Some(99)), None);
    }
    #[test]
    fn leaving_the_overview_drops_an_empty_workspace_left_behind_in_the_middle() {
        let (mut e, o) = engine();
        let b = BackendHandle::default_for_test();
        {
            let m = e.monitors.get_mut(&o).unwrap();
            m.insert_workspace(5, Workspace::new());
            m.move_workspace_to_position(5, 1);
            m.switch_workspace(5);
            assert_eq!(m.workspace_ids(), vec![0, 5, 1]);
        }
        e.enter_overview(&b);
        // Walking away inside the overview keeps the (now inactive) empty workspace, so the
        // stack does not jump under the pointer while the camera is up.
        e.switch_workspace(0, &b);
        assert!(e.monitors.get(&o).unwrap().workspaces.contains_key(&5));
        e.exit_overview(&b);
        e.tick_animations_with_backend(60_000, &b);
        assert!(!e.is_overview());
        let ids = e.monitors.get(&o).unwrap().workspace_ids();
        assert!(!ids.contains(&5), "{ids:?}");
        // Exactly one empty workspace remains, at the bottom.
        let m = e.monitors.get(&o).unwrap();
        assert_eq!(ids.len(), 3);
        assert!(m.workspaces[ids.last().unwrap()].columns.is_empty());
        assert_eq!(m.active_workspace, 0);
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
