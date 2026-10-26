//! Workspace mutations use stable keys; visible indices are resolved only at the edge.

use super::*;
use crate::input::WorkspaceRef;
use crate::layout::{Column, Workspace};

impl TilingEngine {
    pub(crate) fn resolve_workspace_reference(
        &self,
        source_output: OutputId,
        reference: &WorkspaceRef,
    ) -> Option<(OutputId, i32)> {
        match reference {
            WorkspaceRef::Index(index) => {
                let ids = self.monitors.get(&source_output)?.workspace_ids();
                let position = index.saturating_sub(1) as usize;
                ids.get(position.min(ids.len().checked_sub(1)?))
                    .copied()
                    .map(|id| (source_output, id))
            }
            WorkspaceRef::Name(name) => self.monitors.iter().find_map(|(output, monitor)| {
                monitor.workspace_ids().into_iter().find_map(|id| {
                    (monitor.workspaces.get(&id)?.name.as_deref() == Some(name.as_str()))
                        .then_some((*output, id))
                })
            }),
        }
    }

    pub(super) fn workspace_action_target(
        &self,
        reference: Option<&WorkspaceRef>,
    ) -> Option<(OutputId, i32)> {
        let output = self.monitors.focused_id()?;
        match reference {
            Some(reference) => self.resolve_workspace_reference(output, reference),
            None => Some((output, self.monitors.get(&output)?.active_workspace)),
        }
    }

    fn workspace_move_window(
        &mut self,
        window: Option<WindowId>,
        backend: &BackendHandle,
    ) -> Option<(WindowId, OutputId, i32)> {
        let window = match window {
            Some(window) => window,
            None => self.resolve_action_target(backend)?.window,
        };
        if self.floating_windows.contains(&window) {
            let (output, workspace) = self.float_location(window)?;
            Some((window, output, workspace))
        } else {
            let (output, workspace, _) = self.locate_window(window)?;
            Some((window, output, workspace))
        }
    }

    /// Indices are local to the window's output; names are global and may cross outputs.
    pub fn move_window_to_workspace_ref(
        &mut self,
        window: Option<WindowId>,
        reference: WorkspaceRef,
        focus: bool,
        backend: &BackendHandle,
    ) {
        let Some((window, output, workspace)) = self.workspace_move_window(window, backend) else {
            return;
        };
        let Some(destination) = self.resolve_workspace_reference(output, &reference) else {
            return;
        };
        self.transfer_workspace_window(window, (output, workspace), destination, focus, backend);
    }

    pub fn move_window_to_workspace_relative(
        &mut self,
        window: Option<WindowId>,
        down: bool,
        focus: bool,
        backend: &BackendHandle,
    ) {
        let Some((window, output, workspace)) = self.workspace_move_window(window, backend) else {
            return;
        };
        let Some(destination) = self
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace_next_to(workspace, down))
        else {
            return;
        };
        self.transfer_workspace_window(
            window,
            (output, workspace),
            (output, destination),
            focus,
            backend,
        );
    }

    /// Compatibility entry point: the old API takes a stable local key, not an index.
    pub fn move_window_to_workspace(&mut self, workspace: i32, backend: &BackendHandle) {
        let Some((window, output, source)) = self.workspace_move_window(None, backend) else {
            return;
        };
        if let Some(monitor) = self.monitors.get_mut(&output) {
            if !monitor.workspaces.contains_key(&workspace) {
                monitor.insert_workspace(workspace, Workspace::new());
            }
        }
        self.transfer_workspace_window(
            window,
            (output, source),
            (output, workspace),
            false,
            backend,
        );
    }

    fn workspace_insertion_column(&self, output: OutputId, workspace: i32) -> Option<usize> {
        let monitor = self.monitors.get(&output)?;
        let ws = monitor.workspaces.get(&workspace)?;
        if ws.columns.is_empty() {
            return Some(0);
        }
        let active = ws
            .remembered_focus()
            .map(|(column, _)| column)
            .or_else(|| {
                (monitor.active_workspace == workspace)
                    .then_some(monitor.focus_column)
                    .flatten()
            })
            .unwrap_or(0);
        Some(active.saturating_add(1).min(ws.columns.len()))
    }

    pub(super) fn transfer_workspace_window(
        &mut self,
        window: WindowId,
        source: (OutputId, i32),
        destination: (OutputId, i32),
        focus: bool,
        backend: &BackendHandle,
    ) {
        if source == destination
            || self
                .workspace_insertion_column(destination.0, destination.1)
                .is_none()
        {
            return;
        }
        if self.floating_windows.contains(&window) {
            let source_work = self
                .monitors
                .get(&source.0)
                .map(|monitor| monitor.work_area);
            let destination_work = self
                .monitors
                .get(&destination.0)
                .map(|monitor| monitor.work_area);
            let Some(home) = self.float_homes.get_mut(&window) else {
                return;
            };
            let mut place_on_destination = None;
            if source.0 != destination.0 {
                if let (Some(from), Some(to)) = (source_work, destination_work) {
                    home.rect.loc.x += to.loc.x - from.loc.x;
                    home.rect.loc.y += to.loc.y - from.loc.y;
                    home.rect = Self::fit_rect_into(home.rect, to);
                    place_on_destination = Some(home.rect);
                }
            }
            home.output = destination.0;
            home.workspace = destination.1;
            if let Some(rect) = place_on_destination {
                // A float is outside the strip layout: moving its home is not
                // enough, the window itself has to follow to the other display.
                if !self.fullscreen_windows.contains(&window)
                    && !self.minimized.contains_key(&window)
                {
                    let _ = backend.set_window_position(
                        window.as_isize(),
                        rect,
                        windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                            | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                    );
                }
            }
            self.repair_workspace_move_cursor(source.0, source.1, 0);
        } else {
            let Some((_, _, source_column)) = self.locate_window(window) else {
                return;
            };
            let column = {
                let Some(ws) = self
                    .monitors
                    .get_mut(&source.0)
                    .and_then(|monitor| monitor.workspaces.get_mut(&source.1))
                else {
                    return;
                };
                let Some(column) = ws.columns.get_mut(source_column) else {
                    return;
                };
                if column.index_of(window).is_none() {
                    return;
                }
                if column.tiles.len() == 1 {
                    ws.columns.remove(source_column)
                } else {
                    // Keep the requested proportion/logical width, not just its
                    // source-output pixel cache, when splitting a stack.
                    let mut moved = column.clone();
                    let Some(tile) = column.remove_tile(window) else {
                        return;
                    };
                    moved.tiles = vec![tile];
                    moved.select_window(window);
                    moved
                }
            };
            self.repair_workspace_move_cursor(source.0, source.1, source_column);
            self.insert_workspace_column(destination.0, destination.1, column);
        }
        if let Some(monitor) = self.monitors.get_mut(&source.0) {
            monitor.focus_ring.remove(window);
        }
        self.finish_workspace_transfer(
            source.0,
            destination,
            &[window],
            focus.then_some(window),
            backend,
        );
    }

    pub(super) fn active_workspace_column(&self) -> Option<(OutputId, i32, usize)> {
        let output = self.monitors.focused_id()?;
        let monitor = self.monitors.get(&output)?;
        let workspace = monitor.workspaces.get(&monitor.active_workspace)?;
        let column = monitor
            .focus_window
            .and_then(|window| workspace.find_window_column(window))
            .or(monitor.focus_column)?;
        (column < workspace.columns.len()).then_some((output, monitor.active_workspace, column))
    }

    /// Compatibility entry point for the legacy relative, focus-following action.
    pub fn move_focused_column_to_workspace(&mut self, delta: i32, backend: &BackendHandle) {
        if delta != 0 {
            self.move_column_to_workspace_relative(delta > 0, true, backend);
        }
    }

    pub fn move_column_to_workspace_ref(
        &mut self,
        reference: WorkspaceRef,
        focus: bool,
        backend: &BackendHandle,
    ) {
        let Some((output, workspace, column)) = self.active_workspace_column() else {
            return;
        };
        let Some(destination) = self.resolve_workspace_reference(output, &reference) else {
            return;
        };
        self.transfer_workspace_column((output, workspace, column), destination, focus, backend);
    }

    pub fn move_column_to_workspace_relative(
        &mut self,
        down: bool,
        focus: bool,
        backend: &BackendHandle,
    ) {
        let Some((output, workspace, column)) = self.active_workspace_column() else {
            return;
        };
        let Some(destination) = self
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace_next_to(workspace, down))
        else {
            return;
        };
        self.transfer_workspace_column(
            (output, workspace, column),
            (output, destination),
            focus,
            backend,
        );
    }

    pub(super) fn transfer_workspace_column(
        &mut self,
        source: (OutputId, i32, usize),
        destination: (OutputId, i32),
        focus: bool,
        backend: &BackendHandle,
    ) {
        if (source.0, source.1) == destination
            || self
                .workspace_insertion_column(destination.0, destination.1)
                .is_none()
        {
            return;
        }
        let Some(ws) = self
            .monitors
            .get_mut(&source.0)
            .and_then(|monitor| monitor.workspaces.get_mut(&source.1))
        else {
            return;
        };
        if source.2 >= ws.columns.len() {
            return;
        }
        let column = ws.columns.remove(source.2);
        let windows: Vec<WindowId> = column.tiles.iter().map(|tile| tile.window_id).collect();
        let selected = column.active_window();
        self.repair_workspace_move_cursor(source.0, source.1, source.2);
        if let Some(monitor) = self.monitors.get_mut(&source.0) {
            for window in &windows {
                monitor.focus_ring.remove(*window);
            }
        }
        self.insert_workspace_column(destination.0, destination.1, column);
        self.finish_workspace_transfer(
            source.0,
            destination,
            &windows,
            selected.filter(|_| focus),
            backend,
        );
    }

    fn insert_workspace_column(&mut self, output: OutputId, workspace: i32, column: Column) {
        let Some(position) = self.workspace_insertion_column(output, workspace) else {
            return;
        };
        if let Some(monitor) = self.monitors.get_mut(&output) {
            if let Some(ws) = monitor.workspaces.get_mut(&workspace) {
                let selected = ws
                    .remembered_focus()
                    .map(|(_, window)| window)
                    .or_else(|| column.active_window());
                ws.columns.insert(position, column);
                if let Some(window) = selected {
                    ws.remember_focus(window);
                }
                // Keep the existing target selected even if insertion shifted its index.
                if monitor.active_workspace == workspace {
                    if let Some(window) = monitor.focus_window {
                        if let Some(column) = ws.find_window_column(window) {
                            monitor.focus_column = Some(column);
                        }
                    }
                }
            }
        }
    }

    /// Repair only cursors whose window left. Untouched source/target cursors are stable.
    fn repair_workspace_move_cursor(&mut self, output: OutputId, workspace: i32, near: usize) {
        let active_float = self
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.focus_window)
            .filter(|window| self.floating_windows.contains(window))
            .filter(|window| self.float_location(*window) == Some((output, workspace)));
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return;
        };
        let Some(ws) = monitor.workspaces.get_mut(&workspace) else {
            return;
        };
        let replacement = ws
            .columns
            .get(near.min(ws.columns.len().saturating_sub(1)))
            .and_then(Column::active_window);
        if ws
            .active_window
            .and_then(|window| ws.find_window_column(window))
            .is_none()
        {
            if let Some(window) = replacement {
                ws.remember_focus(window);
            } else {
                ws.active_window = None;
            }
        }
        if monitor.active_workspace == workspace {
            if let Some(window) = monitor.focus_window {
                if let Some(column) = ws.find_window_column(window) {
                    ws.remember_focus(window);
                    monitor.focus_column = Some(column);
                    return;
                }
            }
            let remembered = ws.remembered_focus();
            monitor.focus_column = remembered.map(|(column, _)| column);
            // A float owns monitor focus, not the workspace's remembered tile.
            if active_float.is_none() {
                monitor.focus_window = remembered.map(|(_, window)| window);
            }
        }
    }

    fn finish_workspace_transfer(
        &mut self,
        source: OutputId,
        destination: (OutputId, i32),
        windows: &[WindowId],
        focus: Option<WindowId>,
        backend: &BackendHandle,
    ) {
        self.maintain_empty_workspace(source);
        if source != destination.0 {
            self.maintain_empty_workspace(destination.0);
        }
        for window in windows {
            self.applied_state.remove(window);
            self.animating_rects.remove(window);
        }
        if let Some(window) = focus {
            self.focus_action_window(window, backend);
        }
        let visible = self
            .monitors
            .get(&destination.0)
            .map_or(false, |monitor| monitor.active_workspace == destination.1);
        for window in windows {
            let visible = visible || self.sticky_windows.contains(window);
            let _ = backend.show_window(window.as_isize(), visible);
            if !visible {
                self.note_hidden(*window);
            }
        }
        self.apply_layout_for_monitor(source, backend);
        if source != destination.0 {
            self.apply_layout_for_monitor(destination.0, backend);
        }
    }

    /// Reorder a stable workspace object, never swap map entries or IPC identities.
    pub fn move_workspace_to_index(
        &mut self,
        index: u32,
        reference: Option<WorkspaceRef>,
        backend: &BackendHandle,
    ) {
        let Some((output, workspace)) = self.workspace_action_target(reference.as_ref()) else {
            return;
        };
        let occupied: HashSet<i32> = self
            .float_homes
            .values()
            .filter(|home| home.output == output)
            .map(|home| home.workspace)
            .chain(
                self.minimized
                    .values()
                    .filter(|home| home.output == output)
                    .map(|home| home.workspace),
            )
            .collect();
        if let Some(monitor) = self.monitors.get_mut(&output) {
            let ids = monitor.workspace_ids();
            let trailing = ids.last().copied().filter(|id| {
                monitor.workspaces.get(id).map_or(false, |ws| {
                    ws.columns.is_empty() && ws.name.is_none() && !occupied.contains(id)
                })
            });
            let max_position = ids
                .len()
                .saturating_sub(1 + usize::from(trailing.is_some() && trailing != Some(workspace)));
            let position = (index.saturating_sub(1) as usize).min(max_position);
            monitor.move_workspace_to_position(workspace, position);
        }
        self.maintain_empty_workspace(output);
        self.apply_layout_for_monitor(output, backend);
    }

    pub fn move_active_workspace(&mut self, delta: i32, backend: &BackendHandle) {
        if delta == 0 {
            return;
        }
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let Some(monitor) = self.monitors.get(&output) else {
            return;
        };
        let ids = monitor.workspace_ids();
        let Some(position) = ids.iter().position(|id| *id == monitor.active_workspace) else {
            return;
        };
        let target =
            (position as i64 + i64::from(delta)).clamp(0, ids.len().saturating_sub(1) as i64);
        self.move_workspace_to_index(target as u32 + 1, None, backend);
    }

    /// One trailing empty workspace, plus an optional empty workspace being viewed.
    pub fn maintain_empty_workspace(&mut self, output: OutputId) {
        let occupied: HashSet<i32> = self
            .float_homes
            .values()
            .filter(|home| home.output == output)
            .map(|home| home.workspace)
            .chain(
                self.minimized
                    .values()
                    .filter(|home| home.output == output)
                    .map(|home| home.workspace),
            )
            .collect();
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return;
        };
        let ids = monitor.workspace_ids();
        let has_content = |id: &i32| {
            monitor.workspaces.get(id).map_or(false, |ws| {
                ws.columns.iter().any(|column| !column.tiles.is_empty())
                    || ws.name.is_some()
                    || occupied.contains(id)
            })
        };
        let last_occupied = ids.iter().rposition(has_content);
        let after = last_occupied.map_or(0, |position| position + 1);
        let trailing = ids
            .iter()
            .skip(after)
            .copied()
            .find(|id| *id == monitor.active_workspace)
            .or_else(|| ids.get(after).copied());
        let trailing = match trailing {
            Some(id) => id,
            None => {
                let mut id = monitor.next_workspace_id();
                if monitor.workspaces.contains_key(&id) {
                    id = (0..i32::MAX)
                        .find(|id| !monitor.workspaces.contains_key(id))
                        .unwrap_or(i32::MIN);
                }
                monitor.insert_workspace(id, Workspace::new());
                id
            }
        };
        let remove: Vec<i32> = monitor
            .workspace_ids()
            .into_iter()
            .filter(|id| {
                *id != trailing
                    && *id != monitor.active_workspace
                    && monitor.workspaces.get(id).map_or(false, |ws| {
                        ws.columns.iter().all(|column| column.tiles.is_empty())
                            && ws.name.is_none()
                            && !occupied.contains(id)
                    })
            })
            .collect();
        for id in remove {
            monitor.remove_workspace(id);
            if monitor.previous_workspace == Some(id) {
                monitor.previous_workspace = None;
            }
        }
        if !monitor.workspaces.contains_key(&monitor.active_workspace) {
            monitor.active_workspace = trailing;
            monitor.focus_column = None;
            monitor.focus_window = None;
        }
    }
}
