use super::*;
use crate::config::types::SizeSpec;

impl TilingEngine {
    pub(super) fn column_presets(&self, output: OutputId) -> Vec<SizeSpec> {
        let configured = &self.effective_config(output).preset_column_widths;
        if configured.is_empty() {
            Self::PRESET_COLUMN_PROPORTIONS
                .iter()
                .map(|p| SizeSpec::Proportion(*p))
                .collect()
        } else {
            configured.clone()
        }
    }

    /// `layout { preset-window-heights }`, or niri's default thirds/half/two-thirds.
    pub(super) fn window_height_presets(&self, output: OutputId) -> Vec<SizeSpec> {
        let configured = &self.effective_config(output).preset_window_heights;
        if configured.is_empty() {
            Self::PRESET_WINDOW_PROPORTIONS
                .iter()
                .map(|p| SizeSpec::Proportion(*p))
                .collect()
        } else {
            configured.clone()
        }
    }

    /// niri `switch-preset-window-width` / `-back`: in the scrolling layout a window fills
    /// its column horizontally, so the window presets are the column presets (niri resolves
    /// them to the same width request).
    pub fn switch_preset_window_width(&mut self, forwards: bool, backend: &BackendHandle) {
        self.switch_preset_column_width(forwards, backend);
    }

    /// niri `switch-preset-window-height` (`forwards`) and `-back`: the focused window takes
    /// the next of `preset-window-heights` (a third, a half and two thirds of its column by
    /// default).  The other windows of the column share what is left by their weights.  A
    /// column of one window (and a tabbed column) always fills the view, so nothing visible
    /// changes there; the choice is remembered all the same (niri leaves the blank space).
    pub fn switch_preset_window_height(&mut self, forwards: bool, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let cfg = self.effective_config(output).clone();
        let scale = self.monitors.get(&output).map_or(1.0, |m| m.scale_factor);
        // The column's room: the view height less the outer gaps, borders and the gaps
        // between the tiles — the same numbers the layout pass uses.
        let metrics = {
            let Some(monitor) = self.monitors.get(&output) else {
                return;
            };
            let view_h = monitor.work_area.size.h as i32
                - cfg.outer_gaps.0
                - cfg.outer_gaps.2
                - cfg.reserve_top as i32
                - cfg.reserve_bottom as i32;
            let gap = cfg.window_gap;
            let Some(column) = monitor
                .workspace()
                .and_then(|w| monitor.focus_column.and_then(|i| w.columns.get(i)))
            else {
                return;
            };
            let Some(window) = monitor.focus_window else {
                return;
            };
            let Some(index) = column.index_of(window) else {
                return;
            };
            let available = (view_h - gap * (column.tiles.len() as i32 - 1)).max(1);
            let tile = &column.tiles[index];
            let current = match (tile.preferred_height, tile.height_proportion) {
                (Some(px), _) => px as i32,
                (None, Some(proportion)) => proportion_px(available, gap, proportion),
                (None, None) => {
                    let weights: f32 = column
                        .tiles
                        .iter()
                        .map(|t| t.height_weight.max(0.1))
                        .sum();
                    let mine = column.tiles[index].height_weight.max(0.1);
                    (available as f32 * mine / weights.max(0.1)).round() as i32
                }
            };
            (
                monitor.focus_column.unwrap_or(0),
                index,
                current,
                available,
                gap,
                tile.height_preset_idx,
                window,
            )
        };
        let (column_index, tile_index, current, available, gap, remembered, window) = metrics;
        let presets = self.window_height_presets(output);
        let resolved: Vec<i32> = presets
            .iter()
            .map(|spec| match spec {
                SizeSpec::Proportion(p) => proportion_px(available, gap, *p),
                SizeSpec::Fixed(px) => (*px as f64 * scale).round() as i32,
            })
            .collect();
        let preset = Self::next_preset_index(remembered, current, &resolved, forwards);
        let changed = self
            .monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspace_mut())
            .and_then(|workspace| workspace.columns.get_mut(column_index))
            .and_then(|column| column.tiles.get_mut(tile_index))
            .map(|tile| {
                match presets[preset] {
                    SizeSpec::Proportion(share) => {
                        tile.height_proportion = Some(share);
                        tile.preferred_height = None;
                    }
                    SizeSpec::Fixed(px) => {
                        tile.height_proportion = None;
                        tile.preferred_height = Some((px as f64 * scale).round().max(1.0) as u32);
                    }
                }
                tile.height_preset_idx = Some(preset);
            })
            .is_some();
        if !changed {
            return;
        }
        // The height the user chose replaces the one a window rule asked for.
        self.rule_heights.remove(&window);
        self.apply_layout_for_monitor(output, backend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Column, Tile};

    #[test]
    fn mixed_presets_step_from_nonpreset_and_wrap_both_ways() {
        // Fixed widths are window sizes; tile borders come on top.
        let widths = [308, proportion_px(1200, 20, 0.5), 908];
        assert_eq!(TilingEngine::next_preset_index(None, 450, &widths, true), 1);
        assert_eq!(
            TilingEngine::next_preset_index(None, 450, &widths, false),
            0
        );
        assert_eq!(TilingEngine::next_preset_index(None, 908, &widths, true), 0);
        assert_eq!(
            TilingEngine::next_preset_index(None, 308, &widths, false),
            2
        );
        assert_eq!(
            TilingEngine::next_preset_index(Some(1), 600, &widths, true),
            2
        );
        // Preset order is user-configured, not sorted by the compositor.
        assert_eq!(
            TilingEngine::next_preset_index(Some(2), 900, &[900, 300, 600], true),
            0
        );
    }

    #[test]
    fn floats_remember_unsorted_preset_order_in_both_directions() {
        let mut engine = TilingEngine::new(LayoutConfig {
            preset_column_widths: vec![
                SizeSpec::Fixed(900),
                SizeSpec::Fixed(300),
                SizeSpec::Proportion(0.5),
            ],
            ..LayoutConfig::default()
        });
        let output = OutputId::from_name("float-presets");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 760),
        );
        let id = WindowId::new(910);
        engine.register_float(id, output, 0, Rect::new(10, 10, 450, 300));
        let be = BackendHandle::default_for_test();
        for expected in [900, 300, 600, 900] {
            engine.float_cycle_width(id, true, &be);
            assert_eq!(engine.float_homes[&id].rect.size.w, expected);
        }
        engine.float_move_by(id, 0, 5, &be);
        for expected in [600, 300, 900] {
            engine.float_cycle_width(id, false, &be);
            assert_eq!(engine.float_homes[&id].rect.size.w, expected);
        }
        // An explicit width discards preset state, even when its pixels match.
        engine.float_apply_size_change(id, SizeAxis::Width, SizeChange::SetFixed(900), &be);
        assert_eq!(engine.float_homes[&id].width_preset_idx, None);
        engine.float_cycle_width(id, true, &be);
        assert_eq!(engine.float_homes[&id].rect.size.w, 900);
    }

    fn height_fixture(presets: Vec<SizeSpec>) -> (TilingEngine, BackendHandle, OutputId) {
        let mut engine = TilingEngine::new(LayoutConfig {
            preset_window_heights: presets,
            ..LayoutConfig::default()
        });
        let output = OutputId::from_name("height-presets");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 760),
        );
        engine.set_output_name(output, "height-presets");
        let be = BackendHandle::default_for_test();
        let monitor = engine.monitors.get_mut(&output).unwrap();
        let active = monitor.active_workspace;
        let workspace = monitor.workspaces.get_mut(&active).unwrap();
        let mut column = Column::new();
        column.add_tile(Tile::new(WindowId::new(1)));
        column.add_tile(Tile::new(WindowId::new(2)));
        workspace.columns.push(column);
        monitor.focus_column = Some(0);
        monitor.focus_window = Some(WindowId::new(1));
        (engine, be, output)
    }

    fn focused_tile(engine: &TilingEngine, output: OutputId) -> Tile {
        let monitor = engine.monitors.get(&output).unwrap();
        let column = monitor.focus_column.unwrap();
        let window = monitor.focus_window.unwrap();
        let column = &monitor.workspaces[&monitor.active_workspace].columns[column];
        column.tiles[column.index_of(window).unwrap()].clone()
    }

    #[test]
    fn window_height_presets_cycle_both_ways_and_wrap() {
        let (mut engine, be, output) =
            height_fixture(vec![SizeSpec::Proportion(0.25), SizeSpec::Fixed(200)]);
        engine.switch_preset_window_height(true, &be);
        let tile = focused_tile(&engine, output);
        assert_eq!(tile.height_proportion, Some(0.25));
        assert_eq!(tile.preferred_height, None);
        assert_eq!(tile.height_preset_idx, Some(0));
        engine.switch_preset_window_height(true, &be);
        let tile = focused_tile(&engine, output);
        assert_eq!(tile.height_proportion, None);
        assert_eq!(tile.preferred_height, Some(200));
        assert_eq!(tile.height_preset_idx, Some(1));
        // The end of the list wraps to the first preset.
        engine.switch_preset_window_height(true, &be);
        assert_eq!(focused_tile(&engine, output).height_proportion, Some(0.25));
        // ... and backwards from the first one to the last.
        engine.switch_preset_window_height(false, &be);
        let tile = focused_tile(&engine, output);
        assert_eq!(tile.preferred_height, Some(200));
        assert_eq!(tile.height_preset_idx, Some(1));
    }

    #[test]
    fn the_default_window_height_presets_are_the_thirds_of_a_column() {
        let (mut engine, be, output) = height_fixture(Vec::new());
        let heights = engine.window_height_presets(output);
        assert_eq!(
            heights,
            vec![
                SizeSpec::Proportion(1.0 / 3.0),
                SizeSpec::Proportion(0.5),
                SizeSpec::Proportion(2.0 / 3.0)
            ]
        );
        engine.switch_preset_window_height(true, &be);
        // A column with two windows, a 744 px view and 12 px gaps: a third of (732 + 12).
        let tile = focused_tile(&engine, output);
        assert_eq!(tile.height_proportion, Some(1.0 / 3.0));
        assert_eq!(proportion_px(732, 12, 1.0 / 3.0), 236);
    }

    #[test]
    fn window_height_presets_leave_an_unknown_or_empty_strip_alone() {
        let (mut engine, be, output) = height_fixture(vec![SizeSpec::Proportion(0.5)]);
        // No focused window: nothing to give a height to.
        engine.monitors.get_mut(&output).unwrap().focus_window = None;
        engine.switch_preset_window_height(true, &be);
        assert!(engine
            .monitors
            .get(&output)
            .and_then(|m| m.workspace())
            .map_or(true, |w| w.columns[0].tiles[0].height_preset_idx.is_none()));
        // An empty workspace is safe too.
        let monitor = engine.monitors.get_mut(&output).unwrap();
        let active = monitor.active_workspace;
        monitor.workspaces.get_mut(&active).unwrap().columns.clear();
        monitor.focus_window = Some(WindowId::new(1));
        engine.switch_preset_window_height(true, &be);
    }

    #[test]
    fn a_single_window_column_remembers_the_preset_but_fills_the_view() {
        let (mut engine, be, output) = height_fixture(vec![SizeSpec::Proportion(0.5)]);
        {
            let monitor = engine.monitors.get_mut(&output).unwrap();
            let active = monitor.active_workspace;
            monitor.workspaces.get_mut(&active).unwrap().columns[0]
                .tiles
                .truncate(1);
            monitor.focus_window = Some(WindowId::new(1));
        }
        engine.switch_preset_window_height(true, &be);
        let tile = focused_tile(&engine, output);
        assert_eq!(
            tile.height_proportion,
            Some(0.5),
            "the choice is remembered for a column of several windows"
        );
        // The layout keeps the historic rule: a lone window fills the column.
        assert_eq!(
            super::super::rules::distribute_heights(
                600,
                &[super::super::rules::HeightSpec {
                    fixed: Some(proportion_px(600, 12, 0.5)),
                    weight: 1.0,
                    min: 0,
                    max: 0,
                }]
            ),
            vec![600]
        );
    }
}
