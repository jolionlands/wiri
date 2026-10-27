//! Lane L2: the column/tile arrangement actions that were still missing from the parity
//! matrix — `swap-window-left`/`-right`, `move-column-to-index` and
//! `center-visible-columns`.
//!
//! Every one of them is a small, pure list/maths operation on the workspace the user is
//! looking at; nothing here starts a timer, a thread or a poll.  Floating windows have no
//! column, so the column actions are the dispatcher's business (it skips them for a float,
//! where niri does nothing either).
use super::*;

impl TilingEngine {
    /// niri `swap-window-left` (`right` false) and `-right`: the focused window changes
    /// places with the window its neighbouring column shows; the moved window keeps the
    /// focus and its column is kept in view.  Nothing happens at the first or the last
    /// column, in a single-column workspace or on an empty one.
    pub fn swap_window_toward(&mut self, right: bool, backend: &BackendHandle) {
        let Some((output, window)) = self.column_op_target() else {
            return;
        };
        let delta = if right { 1 } else { -1 };
        let moved = self
            .monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspace_mut())
            .and_then(|workspace| workspace.swap_window_with_neighbour(window, delta));
        if moved.is_some() {
            self.finish_column_op(output, backend);
        }
    }

    /// niri `move-column-to-index N`: the focused column goes to the 0-based index `N` of its
    /// workspace (niri counts column indexes from zero), the columns in between shift by one.
    /// An index past the end puts it last; the column stays in view.
    pub fn move_column_to_index(&mut self, index: u32, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let moved = {
            let Some(monitor) = self.monitors.get_mut(&output) else {
                return;
            };
            let Some(column) = monitor.focus_column else {
                return;
            };
            let Some(workspace) = monitor.workspace_mut() else {
                return;
            };
            let Some(last) = workspace.columns.len().checked_sub(1) else {
                return;
            };
            let target = (index as usize).min(last);
            workspace.move_column_to(column, target)
        };
        if let Some(new_column) = moved {
            if let Some(monitor) = self.monitors.get_mut(&output) {
                monitor.focus_column = Some(new_column);
            }
            self.reveal_focused_column(output);
            self.apply_layout_for_monitor(output, backend);
        }
    }

    /// niri `center-visible-columns`: centre the columns that are fully in view.  The view
    /// moves only when the span really is off centre, so pressing the bind twice changes
    /// nothing the second time.
    pub fn center_visible_columns(&mut self, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        self.refresh_strip_widths(output);
        let cfg = self.effective_config(output).clone();
        let (view, default_width) = {
            let Some(monitor) = self.monitors.get(&output) else {
                return;
            };
            let view = Self::view_width_for_monitor(monitor, &cfg);
            let default_width = monitor
                .workspace()
                .map(|workspace| Self::workspace_default_width(workspace, view, &cfg))
                .unwrap_or_else(|| Self::effective_column_width(1, view, &cfg));
            (view, default_width)
        };
        let moved = self
            .monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspace_mut())
            .map_or(false, |workspace| {
                workspace.center_visible_columns(default_width, cfg.column_gap, view)
            });
        if moved {
            self.apply_layout_for_monitor(output, backend);
        }
    }

    /// niri `move-window-down-or-to-workspace-down` (`down`) and `-up`: the focused window
    /// moves down/up inside its column; when it is already at the bottom/top of the column,
    /// it moves to the workspace below/above instead (the view follows it, like
    /// `move-window-to-workspace-down`, whose default it is).  At the last workspace of the
    /// list a move down has nowhere to go and changes nothing.
    pub fn move_window_or_to_workspace(&mut self, down: bool, backend: &BackendHandle) {
        let delta = if down { 1 } else { -1 };
        let can_move = match self.column_op_target() {
            Some((_, window)) => self
                .locate_window(window)
                .and_then(|(output, workspace, column)| {
                    let monitor = self.monitors.get(&output)?;
                    let tiles = monitor.workspaces.get(&workspace)?.columns.get(column)?;
                    let index = tiles.index_of(window)?;
                    Some(if down {
                        index + 1 < tiles.tiles.len()
                    } else {
                        index > 0
                    })
                })
                .unwrap_or(false),
            None => false,
        };
        if can_move {
            self.move_window_in_column(delta, backend);
            return;
        }
        self.move_window_to_workspace_relative(None, down, true, backend);
    }

    /// niri `move-column-left-or-to-monitor-left` (`right` false) and `-right`: the focused
    /// column moves one place inside the strip, or — when it already sits at that end of the
    /// strip — its whole column goes to the monitor in that direction.  A floating window has
    /// no column: it goes to the monitor, like the monitor leg of the action does for a tile.
    pub fn move_column_or_to_monitor(&mut self, right: bool, backend: &BackendHandle) {
        let direction = if right {
            ScrollDirection::Right
        } else {
            ScrollDirection::Left
        };
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let focused = self
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.focus_window.or(self.os_foreground));
        if let Some(window) = focused.filter(|window| self.floating_windows.contains(window)) {
            let target = if right {
                super::monitor_actions::MonitorTarget::Right
            } else {
                super::monitor_actions::MonitorTarget::Left
            };
            self.move_window_to_monitor_action(Some(window), &target, backend);
            return;
        }
        let at_the_end = {
            let Some(monitor) = self.monitors.get(&output) else {
                return;
            };
            let Some(column) = monitor.focus_column else {
                return;
            };
            let Some(workspace) = monitor.workspace() else {
                return;
            };
            if right {
                column + 1 >= workspace.columns.len()
            } else {
                column == 0
            }
        };
        if at_the_end {
            self.move_column_to_monitor(direction, backend);
        } else {
            self.move_column(direction, backend);
        }
    }

    /// niri `focus-floating`, `focus-tiling` and `switch-focus-between-floating-and-tiling`:
    /// the floating and the tiling windows of the active workspace are two focus layers.
    /// Focus is kept on each one separately, so switching back and forth returns to the
    /// window that had it last.  Nothing happens when the wanted layer is empty.
    pub fn focus_layer_action(&mut self, layer: FocusLayer, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let Some(workspace) = self
            .monitors
            .get(&output)
            .map(|monitor| monitor.active_workspace)
        else {
            return;
        };
        let current_is_float = self
            .os_foreground
            .or_else(|| self.monitors.get(&output).and_then(|m| m.focus_window))
            .filter(|window| self.floating_windows.contains(window))
            .and_then(|window| self.float_location(window))
            .is_some_and(|home| home == (output, workspace));
        let floating = match layer {
            FocusLayer::Floating => true,
            FocusLayer::Tiling => false,
            FocusLayer::Switch => !current_is_float,
        };
        let target = if floating {
            self.most_recent_float(output, workspace)
        } else {
            self.most_recent_tile(output, workspace)
        };
        if let Some(target) = target {
            self.focus_action_window(target, backend);
        }
    }

    /// The floating window of `(output, workspace)` that was raised most recently.
    fn most_recent_float(&self, output: OutputId, workspace: i32) -> Option<WindowId> {
        self.float_homes
            .iter()
            .filter(|(window, home)| {
                home.output == output
                    && home.workspace == workspace
                    && !self.minimized.contains_key(*window)
            })
            .max_by_key(|(window, home)| (home.seq, std::cmp::Reverse(window.as_isize())))
            .map(|(window, _)| *window)
    }

    /// The tiling window of `(output, workspace)` that had the focus last: the workspace's
    /// remembered active tile, then the focus ring, then the first tile of the strip.
    fn most_recent_tile(&self, output: OutputId, workspace: i32) -> Option<WindowId> {
        let monitor = self.monitors.get(&output)?;
        if monitor.active_workspace != workspace {
            return None;
        }
        let ws = monitor.workspaces.get(&workspace)?;
        let remembered = ws.remembered_focus().map(|(_, window)| window);
        remembered
            .filter(|window| ws.find_window_column(*window).is_some())
            .or_else(|| {
                monitor
                    .focus_ring
                    .iter_mru()
                    .find(|window| ws.find_window_column(*window).is_some())
            })
            .or_else(|| {
                ws.columns
                    .iter()
                    .find_map(|column| column.active_window())
            })
    }
}

/// Which focus layer an action asks for (niri's `focus-floating` / `focus-tiling` pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusLayer {
    Floating,
    Tiling,
    /// `switch-focus-between-floating-and-tiling`: the other layer, whichever is focused.
    Switch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{parse_action_name, Action};
    use crate::layout::{Column, ColumnDisplay, Tile, WorkspaceDirection};

    fn fixture() -> (TilingEngine, BackendHandle, OutputId) {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let output = OutputId::from_name("lane-l");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 760),
        );
        engine.set_output_name(output, "lane-l");
        (engine, backend, output)
    }

    /// Build the strip of the active workspace directly: `[[1, 2], [3], ...]`.
    fn strip(engine: &mut TilingEngine, output: OutputId, columns: &[&[isize]]) {
        let monitor = engine.monitors.get_mut(&output).expect("output");
        let active = monitor.active_workspace;
        let workspace = monitor.workspaces.get_mut(&active).expect("workspace");
        workspace.columns.clear();
        for ids in columns {
            let mut column = Column::new();
            for id in *ids {
                column.add_tile(Tile::new(WindowId::new(*id)));
            }
            workspace.columns.push(column);
        }
        let first = columns
            .first()
            .and_then(|ids| ids.first())
            .map(|id| WindowId::new(*id));
        monitor.focus_column = if columns.is_empty() { None } else { Some(0) };
        monitor.focus_window = first;
    }

    fn focus(engine: &mut TilingEngine, output: OutputId, window: isize) {
        let window = WindowId::new(window);
        let monitor = engine.monitors.get_mut(&output).expect("output");
        monitor.focus_window = Some(window);
        let column = monitor
            .workspace()
            .and_then(|workspace| workspace.find_window_column(window));
        monitor.focus_column = column;
    }

    fn order(engine: &TilingEngine, output: OutputId) -> Vec<Vec<isize>> {
        engine
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace())
            .map(|workspace| {
                workspace
                    .columns
                    .iter()
                    .map(|column| column.tiles.iter().map(|t| t.window_id.as_isize()).collect())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn workspace_order(engine: &TilingEngine, output: OutputId, workspace: i32) -> Vec<Vec<isize>> {
        engine
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspaces.get(&workspace))
            .map(|workspace| {
                workspace
                    .columns
                    .iter()
                    .map(|column| column.tiles.iter().map(|t| t.window_id.as_isize()).collect())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn second_monitor(engine: &mut TilingEngine) -> OutputId {
        let second = OutputId::from_name("lane-l-right");
        engine.register_monitor(
            second,
            Rect::new(1200, 0, 1200, 800),
            Rect::new(1200, 0, 1200, 760),
        );
        engine.set_output_name(second, "lane-l-right");
        second
    }

    fn run(engine: &mut TilingEngine, backend: &BackendHandle, name: &str, words: &[&str]) {
        let args: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        match parse_action_name(name, &args).unwrap_or_else(|| panic!("cannot parse {name}")) {
            Action::SwapWindowLeft => engine.swap_window_toward(false, backend),
            Action::SwapWindowRight => engine.swap_window_toward(true, backend),
            Action::MoveColumnToIndex(index) => engine.move_column_to_index(index, backend),
            Action::CenterVisibleColumns => engine.center_visible_columns(backend),
            Action::MoveWindowDownOrToWorkspaceDown => {
                engine.move_window_or_to_workspace(true, backend)
            }
            Action::MoveWindowUpOrToWorkspaceUp => {
                engine.move_window_or_to_workspace(false, backend)
            }
            Action::MoveColumnLeftOrToMonitorLeft => {
                engine.move_column_or_to_monitor(false, backend)
            }
            Action::MoveColumnRightOrToMonitorRight => {
                engine.move_column_or_to_monitor(true, backend)
            }
            Action::FocusFloating => engine.focus_layer_action(FocusLayer::Floating, backend),
            Action::FocusTiling => engine.focus_layer_action(FocusLayer::Tiling, backend),
            Action::SwitchFocusBetweenFloatingAndTiling => {
                engine.focus_layer_action(FocusLayer::Switch, backend)
            }
            other => panic!("unexpected action {other:?}"),
        }
    }

    #[test]
    fn swap_window_exchanges_with_the_neighbour_and_keeps_the_focus() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1, 2], &[3, 4], &[5]]);
        // The neighbour shows its remembered active tile.
        if let Some(workspace) = engine
            .monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspace_mut())
        {
            workspace.columns[1].active_window = Some(WindowId::new(4));
        }
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "swap-window-right", &[]);
        assert_eq!(order(&engine, output), vec![vec![4, 2], vec![3, 1], vec![5]]);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(1)));
        assert_eq!(
            monitor.focus_column,
            Some(1),
            "the view follows the window that moved"
        );
        run(&mut engine, &backend, "swap-window-left", &[]);
        assert_eq!(order(&engine, output), vec![vec![1, 2], vec![3, 4], vec![5]]);
    }

    #[test]
    fn swap_window_is_a_no_op_at_the_edges_and_on_an_empty_workspace() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1], &[2]]);
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "swap-window-left", &[]);
        assert_eq!(order(&engine, output), vec![vec![1], vec![2]]);
        focus(&mut engine, output, 2);
        run(&mut engine, &backend, "swap-window-right", &[]);
        assert_eq!(order(&engine, output), vec![vec![1], vec![2]]);
        // A single column has nothing to swap with.
        strip(&mut engine, output, &[&[1, 2]]);
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "swap-window-right", &[]);
        assert_eq!(order(&engine, output), vec![vec![1, 2]]);
        // The empty workspace stays empty, whatever the focus says.
        strip(&mut engine, output, &[]);
        run(&mut engine, &backend, "swap-window-right", &[]);
        assert!(order(&engine, output).is_empty());
    }

    #[test]
    fn move_column_to_index_uses_zero_based_indexes_and_clamps() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1], &[2], &[3]]);
        focus(&mut engine, output, 3);
        run(&mut engine, &backend, "move-column-to-index", &["0"]);
        assert_eq!(order(&engine, output), vec![vec![3], vec![1], vec![2]]);
        assert_eq!(
            engine.monitors.get(&output).unwrap().focus_column,
            Some(0),
            "the focused column keeps the focus"
        );
        run(&mut engine, &backend, "move-column-to-index", &["99"]);
        assert_eq!(
            order(&engine, output),
            vec![vec![1], vec![2], vec![3]],
            "an index past the end goes last"
        );
        // An index that is already the place of the column changes nothing.
        run(&mut engine, &backend, "move-column-to-index", &["2"]);
        assert_eq!(order(&engine, output), vec![vec![1], vec![2], vec![3]]);
    }

    #[test]
    fn move_column_to_index_handles_an_empty_workspace_and_a_bad_argument() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[]);
        run(&mut engine, &backend, "move-column-to-index", &["2"]);
        assert!(order(&engine, output).is_empty());
        // The same words that reach the engine through a bind.
        let args = vec!["x".to_string()];
        assert_eq!(parse_action_name("move-column-to-index", &args), None);
    }

    #[test]
    fn center_visible_columns_moves_the_view_once_and_is_idempotent() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1], &[2], &[3]]);
        for index in 0..3 {
            engine.monitors.get_mut(&output).unwrap().workspaces.values_mut().for_each(|ws| {
                if index < ws.columns.len() {
                    ws.columns[index].set_width(Some(400));
                }
            });
        }
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "center-visible-columns", &[]);
        let after_first = engine
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace())
            .map_or(0, |workspace| workspace.scroll_offset.x);
        assert_ne!(after_first, 0, "the span was not centred before");
        run(&mut engine, &backend, "center-visible-columns", &[]);
        let after_second = engine
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace())
            .map_or(0, |workspace| workspace.scroll_offset.x);
        assert_eq!(after_first, after_second, "nothing left to centre");
    }

    #[test]
    fn center_visible_columns_on_an_empty_workspace_does_nothing() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[]);
        run(&mut engine, &backend, "center-visible-columns", &[]);
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .and_then(|monitor| monitor.workspace())
                .map_or(0, |workspace| workspace.scroll_offset.x),
            0
        );
    }

    #[test]
    fn the_three_actions_reject_surplus_arguments() {
        for name in ["swap-window-left", "swap-window-right", "center-visible-columns"] {
            let args = vec!["extra".to_string()];
            assert_eq!(parse_action_name(name, &args), None, "{name}");
        }
    }

    // --- move-window-down-or-to-workspace-down / -up ---

    #[test]
    fn move_window_moves_inside_the_column_first_then_to_the_workspace() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1, 2]]);
        engine.maintain_empty_workspace(output);
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "move-window-down-or-to-workspace-down", &[]);
        assert_eq!(
            order(&engine, output),
            vec![vec![2, 1]],
            "inside the column while it can"
        );
        // Now at the bottom: the window goes to the workspace below and the view follows.
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "move-window-down-or-to-workspace-down", &[]);
        assert_eq!(order(&engine, output), vec![vec![2]]);
        let active = engine
            .monitors
            .get(&output)
            .map_or(0, |monitor| monitor.active_workspace);
        assert_ne!(active, 0, "the view followed the window");
        assert_eq!(workspace_order(&engine, output, active), vec![vec![1]]);
    }

    #[test]
    fn move_window_up_stops_at_the_end_of_the_workspace_list() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1, 2]]);
        engine.maintain_empty_workspace(output);
        focus(&mut engine, output, 2);
        run(&mut engine, &backend, "move-window-up-or-to-workspace-up", &[]);
        assert_eq!(order(&engine, output), vec![vec![2, 1]]);
        // Already first in its column and on the first workspace: nothing above.
        focus(&mut engine, output, 2);
        run(&mut engine, &backend, "move-window-up-or-to-workspace-up", &[]);
        assert_eq!(order(&engine, output), vec![vec![2, 1]]);
        assert_eq!(
            engine.monitors.get(&output).map(|m| m.active_workspace),
            Some(0)
        );
    }

    #[test]
    fn move_window_to_a_workspace_keeps_a_tabbed_column_whole() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1, 2, 3]]);
        engine.maintain_empty_workspace(output);
        if let Some(workspace) = engine
            .monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspace_mut())
        {
            workspace.columns[0].display = ColumnDisplay::Tabbed { active_tab: 0 };
        }
        focus(&mut engine, output, 2);
        run(&mut engine, &backend, "move-window-down-or-to-workspace-down", &[]);
        // The tabbed column keeps showing a window of its own.
        let shown = engine
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace())
            .map(|workspace| workspace.columns[0].active_window());
        assert!(shown.is_some());
    }

    // --- move-column-left-or-to-monitor-left / -right ---

    #[test]
    fn move_column_moves_inside_the_strip_before_it_changes_monitors() {
        let (mut engine, backend, output) = fixture();
        let second = second_monitor(&mut engine);
        strip(&mut engine, output, &[&[1], &[2]]);
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "move-column-right-or-to-monitor-right", &[]);
        assert_eq!(order(&engine, output), vec![vec![2], vec![1]]);
        // The first column is already at the left end: the whole column changes monitor.
        run(&mut engine, &backend, "move-column-left-or-to-monitor-left", &[]);
        assert_eq!(
            order(&engine, output),
            vec![vec![2]],
            "the monitor on the left does not exist, so nothing moves"
        );
        // Off the right end it goes to the second monitor.
        focus(&mut engine, output, 2);
        run(&mut engine, &backend, "move-column-right-or-to-monitor-right", &[]);
        assert_eq!(order(&engine, output), Vec::<Vec<isize>>::new());
        assert_eq!(workspace_order(&engine, second, 0), vec![vec![2]]);
    }

    #[test]
    fn move_column_with_a_single_column_still_changes_monitors() {
        let (mut engine, backend, output) = fixture();
        let second = second_monitor(&mut engine);
        strip(&mut engine, output, &[&[1, 2]]);
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "move-column-right-or-to-monitor-right", &[]);
        assert!(order(&engine, output).is_empty());
        assert_eq!(
            workspace_order(&engine, second, 0),
            vec![vec![1, 2]],
            "a single column is at both ends of the strip"
        );
    }

    #[test]
    fn move_column_takes_a_floating_window_to_the_monitor() {
        let (mut engine, backend, output) = fixture();
        let second = second_monitor(&mut engine);
        strip(&mut engine, output, &[]);
        let float = WindowId::new(70);
        engine.register_float(float, output, 0, Rect::new(10, 10, 400, 300));
        engine.monitors.get_mut(&output).unwrap().focus_window = Some(float);
        run(&mut engine, &backend, "move-column-right-or-to-monitor-right", &[]);
        assert_eq!(
            engine.float_location(float).map(|(output, _)| output),
            Some(second)
        );
    }

    #[test]
    fn empty_workspace_above_first_keeps_one_empty_workspace_at_the_top() {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig {
            empty_workspace_above_first: true,
            ..LayoutConfig::default()
        });
        let output = OutputId::from_name("lane-l-above-first");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 760),
        );
        engine.set_output_name(output, "lane-l-above-first");
        strip(&mut engine, output, &[&[1], &[2]]);
        engine.maintain_empty_workspace(output);
        let ids = engine
            .monitors
            .get(&output)
            .map(|monitor| monitor.workspace_ids())
            .unwrap_or_default();
        assert_eq!(ids.len(), 3, "one above, the occupied one, the trailing one");
        assert!(
            workspace_order(&engine, output, ids[0]).is_empty(),
            "the first workspace is the empty one above"
        );
        assert_eq!(
            workspace_order(&engine, output, ids[1]),
            vec![vec![1], vec![2]]
        );
        assert!(workspace_order(&engine, output, ids[2]).is_empty());
        // Calling it again changes nothing: the empty workspace above is reused.
        engine.maintain_empty_workspace(output);
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .map(|monitor| monitor.workspace_ids()),
            Some(ids.clone())
        );
        // Going up from the first occupied workspace reaches the empty one.
        engine.switch_workspace(ids[1], &backend);
        engine.focus_workspace_relative(WorkspaceDirection::Previous, &backend);
        assert_eq!(
            engine.monitors.get(&output).map(|monitor| monitor.active_workspace),
            Some(ids[0])
        );
        // A window opening there makes the workspace the first occupied one: a fresh empty
        // workspace takes its place above.
        strip(&mut engine, output, &[&[7]]);
        engine.maintain_empty_workspace(output);
        let ids = engine
            .monitors
            .get(&output)
            .map(|monitor| monitor.workspace_ids())
            .unwrap_or_default();
        assert!(workspace_order(&engine, output, ids[0]).is_empty());
        assert_eq!(workspace_order(&engine, output, ids[1]), vec![vec![7]]);
    }

    #[test]
    fn focus_floating_and_tiling_switch_between_the_two_layers() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1], &[2]]);
        let float = WindowId::new(70);
        engine.register_float(float, output, 0, Rect::new(10, 10, 400, 300));
        focus(&mut engine, output, 2);
        // `focus-floating` reaches the float even though the engine focus is a tile.
        run(&mut engine, &backend, "focus-floating", &[]);
        assert_eq!(engine.os_foreground, Some(float));
        // `focus-tiling` goes back to the tiling layer.
        run(&mut engine, &backend, "focus-tiling", &[]);
        assert_eq!(engine.os_foreground, Some(WindowId::new(2)));
        // `switch-focus-between-floating-and-tiling` toggles from wherever it is.
        run(
            &mut engine,
            &backend,
            "switch-focus-between-floating-and-tiling",
            &[],
        );
        assert_eq!(engine.os_foreground, Some(float));
        run(
            &mut engine,
            &backend,
            "switch-focus-between-floating-and-tiling",
            &[],
        );
        assert_eq!(engine.os_foreground, Some(WindowId::new(2)));
    }

    #[test]
    fn a_layer_that_is_empty_keeps_the_focus_where_it_is() {
        let (mut engine, backend, output) = fixture();
        strip(&mut engine, output, &[&[1]]);
        focus(&mut engine, output, 1);
        run(&mut engine, &backend, "focus-floating", &[]);
        assert_eq!(
            engine.monitors.get(&output).and_then(|m| m.focus_window),
            Some(WindowId::new(1)),
            "no float to focus"
        );
        // A float-only workspace has no tiling window to go back to.
        strip(&mut engine, output, &[]);
        let float = WindowId::new(71);
        engine.register_float(float, output, 0, Rect::new(10, 10, 400, 300));
        engine.os_foreground = Some(float);
        run(&mut engine, &backend, "focus-tiling", &[]);
        assert_eq!(engine.os_foreground, Some(float));
        // The most recently raised float wins over an older one.
        let older = WindowId::new(72);
        engine.register_float(older, output, 0, Rect::new(20, 20, 400, 300));
        let newer = WindowId::new(73);
        engine.register_float(newer, output, 0, Rect::new(30, 30, 400, 300));
        run(&mut engine, &backend, "focus-floating", &[]);
        assert_eq!(engine.os_foreground, Some(newer));
    }

    #[test]
    fn the_layer_actions_reject_surplus_arguments() {
        for name in [
            "focus-floating",
            "focus-tiling",
            "switch-focus-between-floating-and-tiling",
            "toggle-window-rule-opacity",
        ] {
            let args = vec!["extra".to_string()];
            assert_eq!(parse_action_name(name, &args), None, "{name}");
        }
    }

    #[test]
    fn without_empty_workspace_above_first_the_list_starts_with_the_windows() {        let (mut engine, _, output) = fixture();
        strip(&mut engine, output, &[&[1], &[2]]);
        engine.maintain_empty_workspace(output);
        let ids = engine
            .monitors
            .get(&output)
            .map(|monitor| monitor.workspace_ids())
            .unwrap_or_default();
        assert_eq!(ids.len(), 2);
        assert_eq!(workspace_order(&engine, output, ids[0]), vec![vec![1], vec![2]]);
    }

    #[test]
    fn the_composite_actions_reject_surplus_arguments() {        for name in [
            "move-window-down-or-to-workspace-down",
            "move-window-up-or-to-workspace-up",
            "move-column-left-or-to-monitor-left",
            "move-column-right-or-to-monitor-right",
        ] {
            let args = vec!["extra".to_string()];
            assert_eq!(parse_action_name(name, &args), None, "{name}");
        }
    }
}
