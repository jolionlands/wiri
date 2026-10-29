use super::*;
use crate::config::types::SizeSpec;

/// Old effective options and apparent geometry, retained only while publishing
/// an infrequent configuration change. The window IDs guard column identity.
pub(super) struct TiledHeightSnapshot {
    output: OutputId,
    workspace: i32,
    column: usize,
    windows: Vec<WindowId>,
    presets: Vec<SizeSpec>,
    rendered: Vec<i32>,
}

impl TilingEngine {
    pub(super) fn column_presets(&self, output: OutputId) -> Vec<SizeSpec> {
        Self::column_presets_from_config(self.active_workspace_config(output))
    }

    pub(super) fn column_presets_for_workspace(
        &self,
        output: OutputId,
        workspace: i32,
    ) -> Vec<SizeSpec> {
        Self::column_presets_from_config(self.effective_config_for_workspace(output, workspace))
    }

    fn column_presets_from_config(cfg: &LayoutConfig) -> Vec<SizeSpec> {
        let configured = &cfg.preset_column_widths;
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
        Self::window_height_presets_from_config(self.active_workspace_config(output))
    }

    fn window_height_presets_for_workspace(
        &self,
        output: OutputId,
        workspace: i32,
    ) -> Vec<SizeSpec> {
        Self::window_height_presets_from_config(
            self.effective_config_for_workspace(output, workspace),
        )
    }

    fn window_height_presets_from_config(cfg: &LayoutConfig) -> Vec<SizeSpec> {
        let configured = &cfg.preset_window_heights;
        if configured.is_empty() {
            Self::PRESET_WINDOW_PROPORTIONS
                .iter()
                .map(|p| SizeSpec::Proportion(*p))
                .collect()
        } else {
            configured.clone()
        }
    }

    /// Read the old views before publishing either the global layout or caches.
    /// Hidden tabs are projected independently by the pure geometry helper.
    pub(super) fn capture_tiled_height_presets(&self) -> Vec<TiledHeightSnapshot> {
        let mut snapshots = Vec::new();
        for (output, monitor) in self.monitors.iter() {
            for (workspace, strip) in &monitor.workspaces {
                let presets = self.window_height_presets_for_workspace(*output, *workspace);
                for (column, tiles) in strip.columns.iter().enumerate() {
                    if tiles.tiles.is_empty() {
                        continue;
                    }
                    let Some(rendered) =
                        self.tiled_column_rendered_heights(*output, *workspace, column)
                    else {
                        continue;
                    };
                    snapshots.push(TiledHeightSnapshot {
                        output: *output,
                        workspace: *workspace,
                        column,
                        windows: tiles.tiles.iter().map(|tile| tile.window_id).collect(),
                        presets: presets.clone(),
                        rendered,
                    });
                }
            }
        }
        snapshots
    }

    /// niri Column::update_config converts every height to Auto when its
    /// effective list changes, including Fixed requests unrelated to presets.
    pub(super) fn refresh_tiled_height_presets(&mut self, snapshots: Vec<TiledHeightSnapshot>) {
        for snapshot in snapshots {
            if snapshot.presets
                == self.window_height_presets_for_workspace(snapshot.output, snapshot.workspace)
            {
                continue;
            }
            let Some(column) = self
                .monitors
                .get_mut(&snapshot.output)
                .and_then(|monitor| monitor.workspaces.get_mut(&snapshot.workspace))
                .and_then(|workspace| workspace.columns.get_mut(snapshot.column))
            else {
                continue;
            };
            if column.tiles.len() != snapshot.rendered.len()
                || !column
                    .tiles
                    .iter()
                    .map(|tile| tile.window_id)
                    .eq(snapshot.windows.iter().copied())
            {
                continue;
            }
            let mut sorted = snapshot.rendered.clone();
            sorted.sort_unstable();
            let median = sorted[sorted.len() / 2].max(1) as f32;
            for (tile, height) in column.tiles.iter_mut().zip(snapshot.rendered) {
                tile.height_weight = height.max(1) as f32 / median;
                tile.preferred_height = None;
                tile.height_proportion = None;
                tile.height_preset_idx = None;
            }
            // Clear the source as well as derived metadata: a later limit sync
            // must not restore an opening default after the Auto conversion.
            for window in snapshot.windows {
                self.rule_heights.remove(&window);
            }
        }
    }

    /// Preset indices survive position changes and reloads of the same list, but
    /// never refer to a replacement list, even if it is changed back before use.
    pub(super) fn refresh_float_presets(&mut self) {
        let lists: Vec<_> = self
            .float_homes
            .iter()
            .map(|(window, home)| {
                (
                    *window,
                    self.column_presets_for_workspace(home.output, home.workspace),
                    self.window_height_presets_for_workspace(home.output, home.workspace),
                )
            })
            .collect();
        for (window, width_presets, height_presets) in lists {
            if let Some(home) = self.float_homes.get_mut(&window) {
                if home.width_preset_list != width_presets {
                    home.width_preset_idx = None;
                    home.width_preset_list = width_presets;
                }
                if home.height_preset_list != height_presets {
                    home.height_preset_idx = None;
                    home.height_preset_list = height_presets;
                }
            }
        }
    }

    /// Floating heights use their own output's work area and logical preset
    /// list, without the gaps or border band belonging to tiled columns.
    pub fn float_cycle_height(
        &mut self,
        window: WindowId,
        forwards: bool,
        backend: &BackendHandle,
    ) {
        let Some((output, workspace)) = self.float_location(window) else {
            return;
        };
        let Some((work, scale)) = self.monitors.get(&output).map(|m| {
            (
                m.work_area,
                if m.scale_factor.is_finite() && m.scale_factor > 0.0 {
                    m.scale_factor
                } else {
                    1.0
                },
            )
        }) else {
            return;
        };
        let Some(rect) = self.float_action_rect(window, backend) else {
            return;
        };
        let presets = self.window_height_presets_for_workspace(output, workspace);
        let resolved: Vec<f64> = presets
            .iter()
            .map(|spec| match spec {
                SizeSpec::Proportion(p) => work.size.h as f64 / scale * p,
                SizeSpec::Fixed(px) => *px as f64,
            })
            .collect();
        let remembered = self.float_homes.get(&window).and_then(|home| {
            if home.height_preset_list == presets {
                home.height_preset_idx
            } else {
                None
            }
        });
        let current = rect.size.h as f64 / scale;
        let len = presets.len();
        let next = match remembered {
            Some(index) if forwards => (index + 1) % len,
            Some(index) => (index + len - 1) % len,
            None if forwards => resolved
                .iter()
                .position(|height| current + 1.0 < *height)
                .unwrap_or(0),
            None => resolved
                .iter()
                .rposition(|height| *height + 1.0 < current)
                .unwrap_or(len - 1),
        };
        let height = (resolved[next].round().clamp(1.0, 100_000.0) * scale)
            .round()
            .max(1.0) as u32;
        self.place_float_rect(
            window,
            Rect::new(rect.loc.x, rect.loc.y, rect.size.w, height),
            true,
            backend,
        );
        if let Some(home) = self.float_homes.get_mut(&window) {
            home.height_preset_idx = Some(next);
            home.height_preset_list = presets;
        }
    }

    /// niri `switch-preset-window-width` / `-back`: in the scrolling layout a window fills
    /// its column horizontally, so the window presets are the column presets (niri resolves
    /// them to the same width request).
    pub fn switch_preset_window_width(&mut self, forwards: bool, backend: &BackendHandle) {
        self.switch_preset_column_width(forwards, backend);
    }

    /// Cycle the focused tile's configured height. Sole and tabbed columns
    /// retain the request and may leave blank space below it.
    pub fn switch_preset_window_height(&mut self, forwards: bool, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let Some(monitor) = self.monitors.get(&output) else {
            return;
        };
        let (Some(column_index), Some(window)) = (monitor.focus_column, monitor.focus_window)
        else {
            return;
        };
        let workspace = monitor.active_workspace;
        let Some(column) = monitor
            .workspace()
            .and_then(|workspace| workspace.columns.get(column_index))
        else {
            return;
        };
        let Some(tile_index) = column.index_of(window) else {
            return;
        };
        let remembered = column.tiles[tile_index].height_preset_idx;
        let cfg = self.effective_config_for_workspace(output, workspace);
        let view_h = Self::precise_tiling_area(monitor, cfg).height;
        let gap = cfg.window_gap;
        let scale = monitor.scale_factor;
        let border = 2 * self.rule_border_inset(window, cfg);
        let indicator = self.vertical_tab_indicator_space(column, cfg, 1.0);
        let Some(rendered) = self.tiled_column_rendered_heights(output, workspace, column_index)
        else {
            return;
        };
        let current = rendered[tile_index];
        let presets = self.window_height_presets(output);
        // Proportions use the full inner view, including the gap basis; fixed
        // entries are client sizes with the target's physical border added.
        let resolved: Vec<i32> = presets
            .iter()
            .map(|spec| match spec {
                SizeSpec::Proportion(share) => proportion_px(view_h, gap, *share)
                    .saturating_sub(indicator)
                    .max(1),
                SizeSpec::Fixed(px) => ((*px as f64 * scale).round() as i32 + border).max(1),
            })
            .collect();
        let preset = Self::next_preset_index(remembered, current, &resolved, forwards);
        if !self.prepare_tiled_height_owner(output, workspace, column_index, window, &rendered) {
            return;
        }
        let Some(tile) = self
            .monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspaces.get_mut(&workspace))
            .and_then(|workspace| workspace.columns.get_mut(column_index))
            .and_then(|column| column.tiles.get_mut(tile_index))
        else {
            return;
        };
        match presets[preset] {
            SizeSpec::Proportion(share) => {
                tile.height_proportion = Some(share);
                tile.preferred_height = None;
            }
            SizeSpec::Fixed(_) => {
                tile.height_proportion = None;
                tile.preferred_height = Some(resolved[preset] as u32);
            }
        }
        tile.height_preset_idx = Some(preset);
        self.rule_heights.remove(&window);
        self.apply_layout_for_monitor_with_activation(output, backend, false);
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
        // A fixed preset is a *window* height; the default 4 px border comes on top of it.
        assert_eq!(tile.preferred_height, Some(200 + 8));
        assert_eq!(tile.height_preset_idx, Some(1));
        // The end of the list wraps to the first preset.
        engine.switch_preset_window_height(true, &be);
        assert_eq!(focused_tile(&engine, output).height_proportion, Some(0.25));
        // ... and backwards from the first one to the last.
        engine.switch_preset_window_height(false, &be);
        let tile = focused_tile(&engine, output);
        assert_eq!(tile.preferred_height, Some(200 + 8));
        assert_eq!(tile.height_preset_idx, Some(1));
    }

    #[test]
    fn a_fixed_height_preset_is_the_height_of_the_window_with_the_border_on_top() {
        let (mut engine, be, output) = height_fixture(vec![SizeSpec::Fixed(300)]);
        engine.switch_preset_window_height(true, &be);
        let tile = focused_tile(&engine, output);
        // The layout draws the window inside its tile, inset by the border band on both
        // sides, so a 300 px window is a 308 px tile at the default 4 px border.  This is
        // the number `set-window-height "300"` (SizeChange::SetFixed) and a
        // `default-window-height` rule already store; a preset used to store 300 and leave
        // a 292 px window.
        assert_eq!(engine.config.border_width, 4);
        assert_eq!(tile.preferred_height, Some(308));
        assert_eq!(tile.height_proportion, None);
        assert_eq!(tile.height_preset_idx, Some(0));
        // With the border off there is no band to add: the tile *is* the window.
        engine.config.border_width = 0;
        engine.switch_preset_window_height(false, &be);
        assert_eq!(focused_tile(&engine, output).preferred_height, Some(300));
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
        // A column of two windows, a 744 px view and 12 px gaps gives each 366 px, which
        // sits between the half (360 px) and the two-thirds preset (484 px): niri takes the
        // next preset *wider* than the window, so the first switch lands on two thirds.
        let tile = focused_tile(&engine, output);
        assert_eq!(tile.height_proportion, Some(2.0 / 3.0));
        assert_eq!(tile.height_preset_idx, Some(2));
        // The thirds of `preset-window-heights` resolve against the same room.
        assert_eq!(proportion_px(732, 12, 1.0 / 3.0), 236);
        assert_eq!(proportion_px(732, 12, 2.0 / 3.0), 484);
        // Backwards from there goes to the half, then to the third.
        engine.switch_preset_window_height(false, &be);
        assert_eq!(focused_tile(&engine, output).height_proportion, Some(0.5));
        engine.switch_preset_window_height(false, &be);
        assert_eq!(
            focused_tile(&engine, output).height_proportion,
            Some(1.0 / 3.0)
        );
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
    fn a_single_window_column_remembers_the_preset_and_leaves_blank_space() {
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
            "the choice is remembered even for a column of one window"
        );
        // A lone preset tile keeps its requested extent rather than filling the view.
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
            vec![294]
        );
    }
}
