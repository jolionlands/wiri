//! wiri extension `unified-grid`: one horizontal camera per monitor, the x-matched
//! vertical moves and the diagonal grid actions.
//!
//! niri gives every workspace row its own horizontal camera, so moving a row up or down
//! lands on whatever column *that* row was showing. `layout { unified-grid }` gives a
//! monitor **one** camera shared by every row, which makes the desktop a single XY grid:
//! x is each row's endless column strip, y is the monitor's workspace stack. Rule 3 (a
//! vertical move lands on the column under the camera's x anchor), rule 4 (the eight
//! diagonal actions) and rule 5 (two arrows together) are implemented here; the camera
//! itself keeps living in the active workspace's `scroll_offset.x`, so every existing
//! consumer (`calculate_positions`, `is_over_gap`, the gestures, the animations, the IPC
//! geometry) is untouched.
//!
//! Nothing in this module runs when the mode is off, which is the default.
use super::*;
use crate::input::GridAction;
use crate::layout::Column;

/// Where a chordable vertical arrow started: a merged chord continues from here instead
/// of from where the first arrow already moved the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GridAnchor {
    pub output: OutputId,
    pub workspace: i32,
    /// Strip-content x of the centre of the focus the vertical arrow started on.
    pub anchor_x: Option<i32>,
    /// `true` for a row-up step, `false` for a row-down step.
    pub up: bool,
    /// `true` when the vertical leg moved a window, `false` when it moved the focus.
    pub move_window: bool,
}

impl TilingEngine {
    /// Whether this output runs the unified XY grid. Reads the projected per-output
    /// layout, so a per-output `layout { unified-grid }` override works too.
    pub fn unified_grid_for(&self, output: OutputId) -> bool {
        self.effective_config(output).unified_grid
    }

    /// How long two arrow presses count as one diagonal chord, in milliseconds.
    pub fn unified_grid_chord_ms(&self) -> u32 {
        self.monitors
            .focused_id()
            .map(|output| self.effective_config(output).unified_grid_chord_ms)
            .unwrap_or(0)
            .min(1000)
    }

    /// The horizontal camera offset of a monitor, in strip-content coordinates.
    ///
    /// In unified mode this is the one camera every row of the monitor shares; with the
    /// mode off it is niri's per-row camera of the active workspace. This is the hook a
    /// future overview needs to lay the other rows out at the same x.
    pub fn camera_offset(&self, output: OutputId) -> i32 {
        self.monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace())
            .map(|workspace| workspace.scroll_offset.x)
            .unwrap_or(0)
    }

    /// Move the shared camera of `output` to `x` without animating it. Only the active
    /// workspace's offset is authoritative; the other rows are hidden.
    pub(super) fn set_camera_offset(&mut self, output: OutputId, x: i32) {
        if let Some(workspace) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspace_mut())
        {
            workspace.scroll_offset.x = x.max(0);
        }
    }

    /// The strip-content start x and width of every column of a row, in row order.
    fn column_strip(&self, output: OutputId, workspace: i32) -> Option<(Vec<i32>, Vec<i32>)> {
        let monitor = self.monitors.get(&output)?;
        let ws = monitor.workspaces.get(&workspace)?;
        let cfg = self.effective_config_for_workspace(output, workspace);
        let (explicit, mins, maxs) = Self::column_inputs(ws);
        let (widths, _) = Self::column_widths_with_limits(
            &explicit,
            &mins,
            &maxs,
            Self::precise_tiling_area(monitor, cfg).width,
            cfg.column_gap,
            cfg,
        );
        let gap = cfg.column_gap;
        let mut starts = Vec::with_capacity(widths.len());
        let mut x = 0i32;
        for (index, width) in widths.iter().enumerate() {
            starts.push(x);
            if index + 1 < widths.len() {
                x = x.saturating_add(*width).saturating_add(gap);
            }
        }
        Some((starts, widths))
    }

    /// The column of `workspace` whose strip x-range contains `anchor_x`; when the anchor
    /// falls in a gap, in the leading/trailing margin or past the row's ends, the nearest
    /// column by distance (ties to the earlier one). `None` for an empty row.
    fn x_matched_column(&self, output: OutputId, workspace: i32, anchor_x: i32) -> Option<usize> {
        let (starts, widths) = self.column_strip(output, workspace)?;
        if widths.is_empty() {
            return None;
        }
        let mut best: Option<(u128, usize)> = None;
        for (index, width) in widths.iter().enumerate() {
            let start = starts[index];
            let end = start.saturating_add(*width);
            let distance = if anchor_x < start {
                (start as i64 - anchor_x as i64).unsigned_abs()
            } else if anchor_x >= end {
                (anchor_x as i64 - end as i64).unsigned_abs()
            } else {
                0
            } as u128;
            if best.is_none_or(|(current, _)| distance < current) {
                best = Some((distance, index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// The strip-content x of the centre of one column of a row.
    pub(super) fn grid_column_anchor(
        &self,
        output: OutputId,
        workspace: i32,
        column: usize,
    ) -> Option<i32> {
        let (starts, widths) = self.column_strip(output, workspace)?;
        Some(
            starts.get(column).copied().unwrap_or(0) + widths.get(column).copied().unwrap_or(0) / 2,
        )
    }

    /// The strip-content x of the centre of the focused column of `output`'s active row.
    pub(super) fn focus_anchor_x(&self, output: OutputId) -> Option<i32> {
        let monitor = self.monitors.get(&output)?;
        let workspace = monitor.active_workspace;
        let ws = monitor.workspace()?;
        let column = monitor
            .focus_window
            .and_then(|window| ws.find_window_column(window))
            .or(monitor.focus_column)?;
        let (starts, widths) = self.column_strip(output, workspace)?;
        let width = *widths.get(column)?;
        Some(starts.get(column).copied().unwrap_or(0) + width / 2)
    }

    /// Make `column` of `workspace` the focused one, remembering the tile that owns it.
    fn select_row_column(&mut self, output: OutputId, workspace: i32, column: usize) {
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return;
        };
        let Some(ws) = monitor.workspaces.get_mut(&workspace) else {
            return;
        };
        let Some(target) = ws.columns.get(column).and_then(Column::active_window) else {
            return;
        };
        ws.remember_focus(target);
        if monitor.active_workspace == workspace {
            monitor.focus_column = Some(column);
            monitor.focus_window = Some(target);
        }
    }

    /// Unified-grid row change: carry the shared camera across and land on the column
    /// under the camera's x anchor. The vertical slide animates only vertically because
    /// x does not change.
    pub(super) fn unified_switch_workspace(&mut self, output: OutputId, workspace: i32) {
        let anchor = self.focus_anchor_x(output);
        let camera = self.camera_offset(output);
        if let Some(monitor) = self.monitors.get_mut(&output) {
            monitor.switch_workspace(workspace);
            if let Some(ws) = monitor.workspaces.get_mut(&workspace) {
                ws.scroll_offset.x = camera;
            }
        }
        if let Some(anchor) = anchor {
            if let Some(column) = self.x_matched_column(output, workspace, anchor) {
                self.select_row_column(output, workspace, column);
            }
        }
    }

    /// Where a moved window or column enters `workspace`: the column index the anchor x
    /// falls into. `None` (use niri's "next to the active column") when the mode is off.
    pub(super) fn grid_insert_index(
        &self,
        output: OutputId,
        workspace: i32,
        anchor_x: Option<i32>,
    ) -> Option<usize> {
        if !self.unified_grid_for(output) {
            return None;
        }
        let anchor_x = anchor_x?;
        let (_, widths) = self.column_strip(output, workspace)?;
        let index = self.x_matched_column(output, workspace, anchor_x)?;
        // An anchor past the end of the row enters after the last column.
        let (starts, _) = self.column_strip(output, workspace)?;
        let past_end = starts
            .last()
            .zip(widths.last())
            .is_some_and(|(start, width)| anchor_x >= start.saturating_add(*width));
        Some(if past_end { widths.len() } else { index })
    }

    /// Record the state the last chordable vertical arrow started from, so a merged
    /// chord can continue from there. Only unified mode arms a chord.
    pub(super) fn note_grid_vertical(&mut self, up: bool, move_window: bool) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        if !self.unified_grid_for(output) {
            self.grid_chord = None;
            return;
        }
        let workspace = self
            .monitors
            .get(&output)
            .map(|monitor| monitor.active_workspace)
            .unwrap_or(0);
        self.grid_chord = Some(GridAnchor {
            output,
            workspace,
            anchor_x: self.focus_anchor_x(output),
            up,
            move_window,
        });
    }

    /// Mark the next grid action as the second half of a merged chord.
    pub fn note_grid_chord(&mut self) {
        self.grid_chorded = true;
    }

    /// Whether the next grid action continues an arrow chord.
    pub fn grid_chord_pending(&self) -> bool {
        self.grid_chorded
    }

    /// Resolve the row and column index a grid step lands on, without changing anything.
    ///
    /// `chorded` continues from the state the vertical arrow started on (rule 5);
    /// otherwise the step starts from what is focused now.
    fn grid_target(
        &mut self,
        step: GridAction,
        chorded: bool,
    ) -> Option<(OutputId, i32, usize, i32)> {
        let chord = chorded
            .then_some(self.grid_chord)
            .flatten()
            .filter(|anchor| anchor.up == step.up && anchor.move_window == step.move_window);
        let (output, from_workspace, anchor_x) = match chord {
            Some(anchor) => (
                anchor.output,
                anchor.workspace,
                anchor
                    .anchor_x
                    .or_else(|| self.focus_anchor_x(anchor.output))?,
            ),
            None => {
                let output = self.monitors.focused_id()?;
                let workspace = self.monitors.get(&output)?.active_workspace;
                let anchor_x = self.focus_anchor_x(output)?;
                (output, workspace, anchor_x)
            }
        };
        let target_workspace = self
            .monitors
            .get(&output)?
            .workspace_next_to(from_workspace, !step.up)?;
        let (_, widths) = self.column_strip(output, target_workspace)?;
        if widths.is_empty() {
            // An empty row just becomes active (rule 3).
            return Some((output, target_workspace, 0, anchor_x));
        }
        let matched = self.x_matched_column(output, target_workspace, anchor_x)?;
        let last = widths.len().saturating_sub(1);
        let column = if step.right {
            (matched + 1).min(last)
        } else {
            matched.saturating_sub(1)
        };
        Some((output, target_workspace, column, anchor_x))
    }

    /// wiri `unified-grid` rule 6: a diagonal pan.
    ///
    /// The x leg is the existing view scroll (`scroll_step`, clamped against the focused
    /// row's strip); the y leg is one row in the same direction the workspace scroll would
    /// go. Both are resolved before anything moves, so the camera scroll and the row slide
    /// are one motion rather than two. With the mode off the two legs run sequentially, the
    /// same actions `scroll-left`/`scroll-right` and `focus-workspace-up`/`-down` perform.
    pub fn unified_grid_pan(&mut self, pan: crate::input::GridPan, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        if !self.unified_grid_for(output) {
            self.scroll(
                if pan.right {
                    ScrollDirection::Right
                } else {
                    ScrollDirection::Left
                },
                backend,
            );
            self.focus_workspace_relative(
                if pan.down {
                    WorkspaceDirection::Next
                } else {
                    WorkspaceDirection::Previous
                },
                backend,
            );
            return;
        }
        // Resolve the row and the camera target before anything moves, so the x scroll and
        // the row slide are one motion rather than two.
        let target_row = self
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace_next_to(monitor.active_workspace, pan.down));
        let from = self.camera_offset(output);
        let step = self.effective_config(output).scroll_step;
        let target = if pan.right {
            from.saturating_add(step)
        } else {
            from.saturating_sub(step)
        };
        // The shared camera keeps x across the row change and lands on the x-matched
        // column; the pan below is then the existing view scroll, applied on top.
        if let Some(row) = target_row {
            self.unified_switch_workspace(output, row);
        }
        self.set_camera_offset(output, target.max(0));
        self.queue_view(output, from, target.max(0));
        let cfg = self.active_workspace_config(output).clone();
        if let Some(monitor) = self.monitors.get_mut(&output) {
            let view_width = Self::view_width_for_monitor(monitor, &cfg);
            let col_w = monitor
                .workspace()
                .map(|ws| Self::workspace_default_width(ws, view_width, &cfg))
                .unwrap_or_else(|| Self::effective_column_width(0, view_width, &cfg));
            if let Some(workspace) = monitor.workspace_mut() {
                workspace.clamp_scroll(col_w, cfg.column_gap, view_width);
            }
        }
        self.apply_layout_for_monitor(output, backend);
    }

    /// One of the eight diagonal grid actions.
    ///
    /// Unified mode resolves the row and the column before anything moves, so the row
    /// change and the horizontal reveal happen as one motion; with the mode off the
    /// action falls back to the sequential vertical-then-horizontal niri actions.
    pub fn apply_grid_action(&mut self, step: GridAction, chorded: bool, backend: &BackendHandle) {
        let chorded = std::mem::take(&mut self.grid_chorded) && chorded;
        let output_is_unified = self
            .monitors
            .focused_id()
            .is_some_and(|output| self.unified_grid_for(output));
        if !output_is_unified {
            self.grid_sequential_fallback(step, backend);
            return;
        }
        let Some((output, target_workspace, column, _anchor_x)) = self.grid_target(step, chorded)
        else {
            return;
        };
        // The chord has been consumed either way.
        self.grid_chord = None;
        if step.move_window {
            self.grid_move_window(output, target_workspace, column, backend);
        } else {
            self.grid_focus(output, target_workspace, column, backend);
        }
    }

    fn grid_focus(
        &mut self,
        output: OutputId,
        workspace: i32,
        column: usize,
        backend: &BackendHandle,
    ) {
        if self.monitors.focused_id() != Some(output) {
            self.set_focused_output(output);
        }
        let target = self
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspaces.get(&workspace))
            .and_then(|ws| ws.columns.get(column))
            .and_then(Column::active_window);
        if let Some(target) = target {
            // One reveal: `focus_action_window` retargets the in-flight camera animation
            // to this column instead of starting a second one.
            self.focus_action_window(target, backend);
            return;
        }
        if self.monitors.get(&output).map(|m| m.active_workspace) != Some(workspace) {
            if self.unified_grid_for(output) {
                self.unified_switch_workspace(output, workspace);
            } else if let Some(monitor) = self.monitors.get_mut(&output) {
                monitor.switch_workspace(workspace);
            }
        }
        self.reveal_focused_column(output);
        self.apply_layout_for_monitor(output, backend);
    }

    /// Move the focused window one row and one column position, keeping the focus on it.
    fn grid_move_window(
        &mut self,
        output: OutputId,
        target_workspace: i32,
        column: usize,
        backend: &BackendHandle,
    ) {
        if self.monitors.focused_id() != Some(output) {
            self.set_focused_output(output);
        }
        let Some(window) = self.workspace_move_window_target(backend) else {
            return;
        };
        if self.floating_windows.contains(&window) {
            // A float has no column: the row leg is the whole step.
            let down = self
                .monitors
                .get(&output)
                .is_some_and(|monitor| target_workspace > monitor.active_workspace);
            self.move_window_to_workspace_relative(Some(window), down, true, backend);
            return;
        }
        let Some((window_output, source_workspace, source_column)) = self.locate_window(window)
        else {
            return;
        };
        if window_output != output || source_workspace == target_workspace {
            return;
        }
        let Some(moved) = self.detach_workspace_transfer_column(
            (output, source_workspace, source_column),
            Some(window),
        ) else {
            return;
        };
        let windows: Vec<WindowId> = moved.tiles.iter().map(|tile| tile.window_id).collect();
        self.repair_workspace_move_cursor(output, source_workspace, source_column);
        if let Some(monitor) = self.monitors.get_mut(&output) {
            for window in &windows {
                monitor.focus_ring.remove(*window);
            }
        }
        self.insert_workspace_column_at(output, target_workspace, moved, column);
        self.finish_workspace_transfer(
            output,
            (output, target_workspace),
            &windows,
            Some(window),
            backend,
        );
    }

    /// With the mode off, a diagonal action is the sequential vertical-then-horizontal
    /// niri pair, which is what a parity user expects.
    fn grid_sequential_fallback(&mut self, step: GridAction, backend: &BackendHandle) {
        if step.move_window {
            if let Some(window) = self.workspace_move_window_target(backend) {
                self.move_window_to_workspace_relative(Some(window), !step.up, true, backend);
            }
            self.move_column(
                if step.right {
                    crate::layout::ScrollDirection::Right
                } else {
                    crate::layout::ScrollDirection::Left
                },
                backend,
            );
            return;
        }
        self.focus_step(
            if step.up {
                focus_actions::FocusStep::UpOrWorkspace
            } else {
                focus_actions::FocusStep::DownOrWorkspace
            },
            backend,
        );
        self.focus_step(
            if step.right {
                focus_actions::FocusStep::Right
            } else {
                focus_actions::FocusStep::Left
            },
            backend,
        );
    }

    /// The window a window-moving action acts on: a floating one if it has the focus,
    /// else the focused tile.
    fn workspace_move_window_target(&self, _backend: &BackendHandle) -> Option<WindowId> {
        self.os_foreground
            .filter(|window| self.floating_windows.contains(window))
            .or_else(|| {
                self.monitors
                    .focused_id()
                    .and_then(|output| self.monitors.get(&output))
                    .and_then(|monitor| monitor.focus_window)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WindowState;
    use crate::layout::ColumnWidthMode;
    use crate::layout::Workspace;

    const OUTPUT: &str = "grid";

    fn engine(unified: bool) -> (TilingEngine, BackendHandle, OutputId) {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig {
            unified_grid: unified,
            unified_grid_chord_ms: 150,
            column_width: 400,
            column_width_mode: ColumnWidthMode::Fixed,
            column_gap: 16,
            ..LayoutConfig::default()
        });
        let output = OutputId::from_name(OUTPUT);
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1600, 800),
            Rect::new(0, 0, 1600, 800),
        );
        engine.set_output_name(output, OUTPUT);
        (engine, backend, output)
    }

    /// Build one row: `widths.len()` tiles, one per column, with the given widths.
    fn add_row(
        engine: &mut TilingEngine,
        backend: &BackendHandle,
        output: OutputId,
        workspace: i32,
        first_id: isize,
        widths: &[u32],
    ) {
        if let Some(monitor) = engine.monitors.get_mut(&output) {
            if !monitor.workspaces.contains_key(&workspace) {
                monitor.insert_workspace(workspace, Workspace::new());
            }
            monitor.switch_workspace(workspace);
        }
        for (offset, width) in widths.iter().enumerate() {
            let id = first_id + offset as isize;
            engine.add_window(
                WindowInfo {
                    hwnd: id,
                    title: id.to_string(),
                    class_name: "grid-fixture".into(),
                    process_id: 1,
                    bounds: Rect::new(0, 0, 400, 400),
                    state: WindowState::Normal,
                    is_visible: true,
                },
                backend,
            );
            if let Some(monitor) = engine.monitors.get_mut(&output) {
                if let Some(ws) = monitor.workspaces.get_mut(&workspace) {
                    let last = ws.columns.len() - 1;
                    ws.columns[last].width = Some(*width);
                }
            }
        }
    }

    fn focus(
        engine: &mut TilingEngine,
        output: OutputId,
        workspace: i32,
        column: usize,
    ) -> WindowId {
        let window = {
            let monitor = engine.monitors.get_mut(&output).unwrap();
            monitor.switch_workspace(workspace);
            let window = monitor
                .workspaces
                .get(&workspace)
                .and_then(|ws| ws.columns.get(column))
                .and_then(Column::active_window)
                .unwrap();
            monitor.focus_column = Some(column);
            monitor.focus_window = Some(window);
            window
        };
        if let Some(ws) = engine
            .monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspaces.get_mut(&workspace))
        {
            ws.remember_focus(window);
        }
        window
    }

    fn strip(engine: &TilingEngine, output: OutputId, workspace: i32) -> Vec<(i32, i32)> {
        let (starts, widths) = engine.column_strip(output, workspace).unwrap();
        starts.into_iter().zip(widths).collect()
    }

    /// Three rows of varied widths: narrow, wide, narrow-wide-narrow.
    fn three_rows() -> (TilingEngine, BackendHandle, OutputId) {
        let (mut engine, backend, output) = engine(true);
        add_row(
            &mut engine,
            &backend,
            output,
            0,
            100,
            &[300, 500, 400, 600, 350],
        );
        add_row(&mut engine, &backend, output, 1, 200, &[600, 400, 700]);
        add_row(&mut engine, &backend, output, 2, 300, &[400, 400]);
        (engine, backend, output)
    }

    #[test]
    fn the_shared_camera_is_carried_across_a_row_change() {
        let (mut engine, backend, output) = three_rows();
        focus(&mut engine, output, 0, 1);
        engine.set_camera_offset(output, 320);
        // Row 1 gets the camera, not its own remembered offset.
        engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspaces
            .get_mut(&1)
            .unwrap()
            .scroll_offset
            .x = 0;
        engine.switch_workspace(1, &backend);
        assert_eq!(engine.camera_offset(output), 320);
        assert_eq!(engine.monitors.get(&output).unwrap().active_workspace, 1);
    }

    #[test]
    fn a_vertical_move_lands_on_the_column_under_the_anchor() {
        let (mut engine, backend, output) = three_rows();
        // Row 0 column 1 spans [316, 816): its centre is 566, which row 1 shows in
        // column 0 ([0, 600)).
        focus(&mut engine, output, 0, 1);
        let anchor = engine.focus_anchor_x(output).unwrap();
        assert_eq!(anchor, 316 + 500 / 2);
        engine.focus_workspace_relative(WorkspaceDirection::Next, &backend);
        assert_eq!(engine.monitors.get(&output).unwrap().active_workspace, 1);
        assert_eq!(engine.monitors.get(&output).unwrap().focus_column, Some(0));
    }

    #[test]
    fn an_anchor_in_a_gap_takes_the_nearest_column() {
        let (engine, _backend, output) = three_rows();
        // Row 2 is [0,400) and [416,816): 408 is the middle of the 16 px gap and is
        // 8 px from both, so the earlier column wins.
        assert_eq!(engine.x_matched_column(output, 2, 408), Some(0));
        // 412 is nearer the second column.
        assert_eq!(engine.x_matched_column(output, 2, 412), Some(1));
    }

    #[test]
    fn an_anchor_past_the_end_of_a_row_takes_the_last_column() {
        let (engine, _backend, output) = three_rows();
        assert_eq!(engine.x_matched_column(output, 2, 5_000), Some(1));
        // Before the first column it is the first one.
        assert_eq!(engine.x_matched_column(output, 2, -40), Some(0));
    }

    #[test]
    fn an_empty_row_just_becomes_active_and_keeps_the_camera() {
        let (mut engine, backend, output) = three_rows();
        add_row(&mut engine, &backend, output, 3, 400, &[]);
        focus(&mut engine, output, 2, 1);
        engine.set_camera_offset(output, 480);
        engine.focus_workspace_relative(WorkspaceDirection::Next, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.active_workspace, 3);
        assert_eq!(monitor.focus_column, None);
        assert_eq!(engine.camera_offset(output), 480);
    }

    #[test]
    fn a_moved_window_enters_at_the_anchor_column_and_keeps_its_width() {
        let (mut engine, backend, output) = three_rows();
        // Row 0 column 3 spans [1248, 1848): centre 1548, which row 1 shows in column 2
        // ([1032, 1732)).
        let moved = focus(&mut engine, output, 0, 3);
        engine.move_window_to_workspace_relative(Some(moved), true, true, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        let row1 = monitor.workspaces.get(&1).unwrap();
        assert_eq!(row1.find_window_column(moved), Some(2));
        assert_eq!(row1.columns[2].width, Some(600));
        assert_eq!(monitor.active_workspace, 1);
        assert_eq!(monitor.focus_window, Some(moved));
    }

    #[test]
    fn a_moved_column_enters_at_the_anchor_column() {
        let (mut engine, backend, output) = three_rows();
        focus(&mut engine, output, 0, 0);
        engine.move_column_to_workspace_relative(true, true, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        let row1 = monitor.workspaces.get(&1).unwrap();
        // [0, 300) centre 150 lands in row 1's first column.
        assert_eq!(row1.find_window_column(WindowId::new(100)), Some(0));
        assert_eq!(row1.columns[0].width, Some(300));
    }

    #[test]
    fn a_diagonal_focus_moves_one_row_and_one_column_atomically() {
        let (mut engine, backend, output) = three_rows();
        focus(&mut engine, output, 1, 1);
        // Row 0 matches column 1 for the anchor, then one column right.
        engine.apply_grid_action(GridAction::new(true, true, false), false, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.active_workspace, 0);
        assert_eq!(monitor.focus_column, Some(2));
    }

    #[test]
    fn a_chorded_diagonal_continues_from_the_row_the_first_arrow_left() {
        let (mut engine, backend, output) = three_rows();
        focus(&mut engine, output, 1, 1);
        // First arrow: one row down. It lands on row 2 column 1 (row 2 is [0,400) and
        // [416,816) and the anchor is 566).
        engine.focus_step(focus_actions::FocusStep::DownOrWorkspace, &backend);
        assert_eq!(engine.monitors.get(&output).unwrap().active_workspace, 2);
        // The chorded diagonal must not take a second row down: it continues from row 1.
        engine.note_grid_chord();
        engine.apply_grid_action(GridAction::new(false, true, false), true, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.active_workspace, 2);
        assert_eq!(monitor.focus_column, Some(1));
    }

    #[test]
    fn a_direct_diagonal_without_a_chord_starts_from_the_current_row() {
        let (mut engine, backend, output) = three_rows();
        focus(&mut engine, output, 0, 1);
        engine.focus_step(focus_actions::FocusStep::DownOrWorkspace, &backend);
        assert_eq!(engine.monitors.get(&output).unwrap().active_workspace, 1);
        // A direct diagonal starts from row 1: one more row down and one column right.
        engine.apply_grid_action(GridAction::new(false, true, false), false, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.active_workspace, 2);
        assert_eq!(monitor.focus_column, Some(1));
    }

    #[test]
    fn a_diagonal_move_keeps_the_focus_on_the_window() {
        let (mut engine, backend, output) = three_rows();
        let moved = focus(&mut engine, output, 0, 1);
        // Down-left: row 1 matches column 0 for the anchor, one position left keeps it there.
        engine.apply_grid_action(GridAction::new(false, false, true), false, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.active_workspace, 1);
        let row1 = monitor.workspaces.get(&1).unwrap();
        assert_eq!(row1.find_window_column(moved), Some(0));
        assert_eq!(monitor.focus_window, Some(moved));
        assert_eq!(row1.columns[0].width, Some(500));
    }

    #[test]
    fn a_diagonal_move_into_a_row_one_position_right_inserts_there() {
        let (mut engine, backend, output) = three_rows();
        let moved = focus(&mut engine, output, 0, 1);
        // Down-right: row 1 matches column 0, one position right is index 1.
        engine.apply_grid_action(GridAction::new(false, true, true), false, &backend);
        let row1 = engine
            .monitors
            .get(&output)
            .unwrap()
            .workspaces
            .get(&1)
            .unwrap();
        assert_eq!(row1.find_window_column(moved), Some(1));
        assert_eq!(row1.columns[1].width, Some(500));
    }

    #[test]
    fn mode_off_keeps_niris_own_remembered_column() {
        let (mut engine, backend, output) = engine(false);
        add_row(
            &mut engine,
            &backend,
            output,
            0,
            100,
            &[300, 500, 400, 600, 350],
        );
        add_row(&mut engine, &backend, output, 1, 200, &[600, 400, 700]);
        // Row 1 remembers the last column it focused (its own last tile).
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .unwrap()
                .workspaces
                .get(&1)
                .unwrap()
                .active_window,
            Some(WindowId::new(202))
        );
        focus(&mut engine, output, 0, 1);
        engine.focus_workspace_relative(WorkspaceDirection::Next, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.active_workspace, 1);
        // niri parity: the row's own remembered column, not the x-matched one (which
        // would be column 0 for this anchor).
        assert_eq!(monitor.focus_column, Some(2));
    }

    #[test]
    fn mode_off_diagonal_falls_back_to_the_sequential_niri_actions() {
        let (mut engine, backend, output) = engine(false);
        add_row(
            &mut engine,
            &backend,
            output,
            0,
            100,
            &[300, 500, 400, 600, 350],
        );
        add_row(&mut engine, &backend, output, 1, 200, &[600, 400, 700]);
        focus(&mut engine, output, 0, 1);
        engine.apply_grid_action(GridAction::new(true, true, false), false, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        // up-or-workspace does nothing at row 0 (no row above), then one column right.
        assert_eq!(monitor.active_workspace, 0);
        assert_eq!(monitor.focus_column, Some(2));
    }

    #[test]
    fn a_diagonal_pan_scrolls_x_and_moves_one_row_in_one_pass() {
        let (mut engine, backend, output) = three_rows();
        focus(&mut engine, output, 1, 1);
        engine.set_camera_offset(output, 100);
        engine.unified_grid_pan(crate::input::GridPan::new(true, true), &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.active_workspace, 2);
        assert!(engine.camera_offset(output) >= 100 + engine.effective_config(output).scroll_step);
    }

    #[test]
    fn the_strip_widths_are_the_configured_ones() {
        let (engine, _backend, output) = three_rows();
        assert_eq!(
            strip(&engine, output, 0),
            vec![(0, 300), (316, 500), (832, 400), (1248, 600), (1864, 350)]
        );
        assert_eq!(
            strip(&engine, output, 1),
            vec![(0, 600), (616, 400), (1032, 700)]
        );
    }
}
