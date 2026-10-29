//! Monitor actions use the existing output snapshot, never action-time Win32 queries.
use super::*;
pub(super) use crate::input::MonitorTarget;
use crate::input::WorkspaceRef;

/// Edge/overlap distance in widened coordinates, with stable enumeration as the tie-break.
fn directional_score(from: Rect, to: Rect, target: &MonitorTarget) -> Option<(bool, i128, i64)> {
    let edges = |rect: Rect| {
        (
            i64::from(rect.loc.x),
            i64::from(rect.loc.y),
            i64::from(rect.loc.x) + i64::from(rect.size.w),
            i64::from(rect.loc.y) + i64::from(rect.size.h),
        )
    };
    let (ax, ay, ar, ab) = edges(from);
    let (bx, by, br, bb) = edges(to);
    let (ahead, gap, cross_gap, cross_center) = match target {
        MonitorTarget::Left => (
            bx + br < ax + ar,
            (ax - br).max(0),
            (ay - bb).max(by - ab).max(0),
            (by + bb - ay - ab).abs(),
        ),
        MonitorTarget::Right => (
            bx + br > ax + ar,
            (bx - ar).max(0),
            (ay - bb).max(by - ab).max(0),
            (by + bb - ay - ab).abs(),
        ),
        MonitorTarget::Up => (
            by + bb < ay + ab,
            (ay - bb).max(0),
            (ax - br).max(bx - ar).max(0),
            (bx + br - ax - ar).abs(),
        ),
        MonitorTarget::Down => (
            by + bb > ay + ab,
            (by - ab).max(0),
            (ax - br).max(bx - ar).max(0),
            (bx + br - ax - ar).abs(),
        ),
        _ => return None,
    };
    ahead.then_some((
        cross_gap > 0,
        i128::from(gap).pow(2) + i128::from(cross_gap).pow(2),
        cross_center,
    ))
}

impl TilingEngine {
    pub(super) fn monitor_target(
        &self,
        source: OutputId,
        target: &MonitorTarget,
    ) -> Option<OutputId> {
        let monitor = self.monitors.get(&source)?;
        let found = match target {
            MonitorTarget::Name(name) => self.find_output_by_name(name),
            MonitorTarget::Previous | MonitorTarget::Next => {
                let order: Vec<_> = self.monitors.keys().copied().collect();
                let position = order.iter().position(|output| *output == source)?;
                let next = if matches!(target, MonitorTarget::Next) {
                    (position + 1) % order.len()
                } else {
                    (position + order.len() - 1) % order.len()
                };
                order.get(next).copied()
            }
            _ => self
                .monitors
                .iter()
                .filter(|(id, _)| **id != source)
                .filter_map(|(id, candidate)| {
                    directional_score(monitor.bounds, candidate.bounds, target)
                        .map(|score| (score, *id))
                })
                .min_by_key(|(score, _)| *score)
                .map(|(_, id)| id),
        };
        found.filter(|output| *output != source)
    }

    pub fn focus_monitor_action(&mut self, target: &MonitorTarget, backend: &BackendHandle) {
        let Some(source) = self.monitors.focused_id() else {
            return;
        };
        let Some(destination) = self.monitor_target(source, target) else {
            return;
        };
        self.set_focused_output(destination);
        let window = self
            .monitors
            .get(&destination)
            .and_then(|monitor| monitor.focus_window);
        if let Some(window) = window {
            self.focus_action_window(window, backend);
        }
        self.apply_layout_for_monitor(source, backend);
        self.apply_layout_for_monitor(destination, backend);
    }

    pub fn move_window_to_monitor_action(
        &mut self,
        window: Option<WindowId>,
        target: &MonitorTarget,
        backend: &BackendHandle,
    ) {
        let window = match window {
            Some(window) => window,
            None => match self.resolve_action_target(backend) {
                Some(target) => target.window,
                None => return,
            },
        };
        let source = self.float_location(window).or_else(|| {
            self.locate_window(window)
                .map(|(output, workspace, _)| (output, workspace))
        });
        let Some(source) = source else {
            return;
        };
        let Some(output) = self.monitor_target(source.0, target) else {
            return;
        };
        let Some(workspace) = self
            .monitors
            .get(&output)
            .map(|monitor| monitor.active_workspace)
        else {
            return;
        };
        // An explicit inactive window must not steal either output's selection.
        let follow = self.os_foreground == Some(window)
            || (self.monitors.focused_id() == Some(source.0)
                && self.monitors.get(&source.0).is_some_and(|monitor| {
                    monitor.focus_window == Some(window) && monitor.active_workspace == source.1
                }));
        self.transfer_workspace_window(window, source, (output, workspace), follow, backend);
    }

    pub fn move_column_to_monitor_action(
        &mut self,
        target: &MonitorTarget,
        backend: &BackendHandle,
    ) {
        let Some(source) = self.active_workspace_column() else {
            return;
        };
        let Some(output) = self.monitor_target(source.0, target) else {
            return;
        };
        let Some(workspace) = self
            .monitors
            .get(&output)
            .map(|monitor| monitor.active_workspace)
        else {
            return;
        };
        self.transfer_workspace_column(source, (output, workspace), true, backend);
    }

    pub fn move_workspace_to_monitor_action(
        &mut self,
        reference: Option<WorkspaceRef>,
        target: &MonitorTarget,
        backend: &BackendHandle,
    ) {
        let Some((source, workspace)) = self.workspace_action_target(reference.as_ref()) else {
            return;
        };
        let Some(destination) = self.monitor_target(source, target) else {
            return;
        };
        self.relocate_workspace(source, workspace, destination, Some(backend));
    }

    /// Move one whole workspace object (identity, name, columns, floating and minimized
    /// homes) to the end of `destination`'s stack and return its new local key.
    ///
    /// `backend` is `None` for event-driven output-preference changes: floating windows
    /// then keep their (shifted) bookkeeping rectangle and the next layout pass places
    /// them, and no layout pass is requested here.
    pub(super) fn relocate_workspace(
        &mut self,
        source: OutputId,
        workspace: i32,
        destination: OutputId,
        backend: Option<&BackendHandle>,
    ) -> Option<i32> {
        if source == destination {
            return None;
        }
        let Some((source_work, destination_work)) = self
            .monitors
            .get(&source)
            .zip(self.monitors.get(&destination))
            .map(|(from, to)| (from.work_area, to.work_area))
        else {
            return None;
        };
        let follow = self.monitors.focused_id() == Some(source)
            && self
                .monitors
                .get(&source)
                .is_some_and(|monitor| monitor.active_workspace == workspace);
        let (moved, was_active) = {
            let monitor = self.monitors.get_mut(&source).expect("resolved source");
            let ids = monitor.workspace_ids();
            let position = ids.iter().position(|id| *id == workspace).unwrap_or(0);
            let Some(moved) = monitor.remove_workspace(workspace) else {
                return None;
            };
            let active = monitor.active_workspace == workspace;
            if active {
                let remaining = monitor.workspace_ids();
                if let Some(next) = remaining.get(position.min(remaining.len().saturating_sub(1))) {
                    monitor.active_workspace = *next;
                }
                monitor.focus_column = None;
                monitor.focus_window = None;
                monitor.restore_focus_in_active_workspace();
            }
            if monitor.previous_workspace == Some(workspace) {
                monitor.previous_workspace = None;
            }
            for tile in moved.columns.iter().flat_map(|column| &column.tiles) {
                monitor.focus_ring.remove(tile.window_id);
            }
            (moved, active)
        };
        let ipc_id = moved.ipc_id;
        let selected = moved.remembered_focus().map(|(_, window)| window);
        let mut windows: Vec<_> = moved
            .columns
            .iter()
            .flat_map(|column| column.tiles.iter().map(|tile| tile.window_id))
            .collect();
        let mut moved_float_rects = Vec::new();
        let destination_key = {
            let monitor = self
                .monitors
                .get_mut(&destination)
                .expect("resolved destination");
            let ids = monitor.workspace_ids();
            let position = ids
                .last()
                .filter(|id| {
                    monitor
                        .workspaces
                        .get(id)
                        .is_some_and(|ws| ws.name.is_none() && ws.columns.is_empty())
                })
                .map_or(ids.len(), |_| ids.len() - 1);
            let key = monitor.next_workspace_id();
            monitor.insert_workspace(key, moved);
            monitor.move_workspace_to_position(key, position);
            key
        };
        for (window, home) in &mut self.float_homes {
            if home.output == source && home.workspace == workspace {
                home.output = destination;
                home.workspace = destination_key;
                home.rect.loc.x = home
                    .rect
                    .loc
                    .x
                    .saturating_add(destination_work.loc.x.saturating_sub(source_work.loc.x));
                home.rect.loc.y = home
                    .rect
                    .loc
                    .y
                    .saturating_add(destination_work.loc.y.saturating_sub(source_work.loc.y));
                home.rect = Self::fit_rect_into(home.rect, destination_work);
                moved_float_rects.push((*window, home.rect));
                windows.push(*window);
            }
        }
        if let Some(backend) = backend {
            for (window, rect) in moved_float_rects {
                if !self.fullscreen_windows.contains(&window)
                    && !self.minimized.contains_key(&window)
                {
                    // Floats do not pass through the strip layout; without this the
                    // bookkeeping moves to the other output but the HWND stays put.
                    let _ = backend.set_window_position(
                        window.as_isize(),
                        rect,
                        windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                            | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                    );
                }
            }
        }
        for minimized in self.minimized.values_mut() {
            if minimized.output == source && minimized.workspace == workspace {
                minimized.output = destination;
                minimized.workspace = destination_key;
            }
        }
        // Explicit re-homing overrides a pending automatic return to an unplugged output.
        for unplugged in self.unplugged.values_mut() {
            let keys: Vec<_> = unplugged
                .monitor
                .workspaces
                .iter()
                .filter_map(|(key, ws)| (ws.ipc_id == ipc_id).then_some(*key))
                .collect();
            for key in keys {
                unplugged.monitor.remove_workspace(key);
                unplugged.moved.retain(|(from, _)| *from != key);
            }
        }
        self.refresh_float_height_presets();
        self.maintain_empty_workspace(source);
        self.maintain_empty_workspace(destination);
        if was_active {
            // Restore a floating-only source workspace's selection as well.
            let active = self
                .monitors
                .get(&source)
                .map(|monitor| monitor.active_workspace);
            let floating = self
                .float_homes
                .iter()
                .filter(|(_, home)| home.output == source && Some(home.workspace) == active)
                .max_by_key(|(_, home)| home.seq)
                .map(|(window, _)| *window);
            if let Some(monitor) = self.monitors.get_mut(&source) {
                if monitor.focus_window.is_none() {
                    monitor.focus_window = floating;
                }
            }
        }
        if follow {
            self.set_focused_output(destination);
            match backend {
                Some(backend) => {
                    self.switch_workspace(destination_key, backend);
                    if let Some(window) = selected {
                        self.focus_action_window(window, backend);
                    }
                }
                // State only: the keyboard focus follows the workspace without a
                // layout pass, and the OS focus is reconciled by the next event.
                None => {
                    if let Some(monitor) = self.monitors.get_mut(&destination) {
                        monitor.switch_workspace(destination_key);
                        if let Some(window) = selected {
                            monitor.set_focus(window);
                        }
                    }
                }
            }
        }
        for window in windows {
            self.applied_state.remove(&window);
            self.animating_rects.remove(&window);
        }
        if let Some(backend) = backend {
            self.apply_layout_for_monitor(source, backend);
            self.apply_layout_for_monitor(destination, backend);
        }
        Some(destination_key)
    }
}
