//! Admission-time width intent and output-dependent resolution. The legacy `width`
//! remains the pixel cache used by all existing layout algorithms.
use super::*;
use crate::config::types::SizeSpec;

/// A client request and its settled, rasterized physical width are distinct.
pub(super) struct ResolvedWidth {
    pub tile_px: u32,
    pub window_px: u32,
    pub window_logical: Option<i32>,
}

/// Shared geometry for native placement and IPC. A missing logical dimension
/// retains the existing physical policy for that dimension (including heights).
pub(super) struct ResolvedWindowGeometry {
    pub tile: Rect,
    pub window: Rect,
    pub logical_window_size: (Option<i32>, Option<i32>),
}

impl TilingEngine {
    pub(super) fn proportional_width(
        view: f64,
        cfg: &LayoutConfig,
        proportion: f64,
        inset: i32,
        min: u32,
        max: u32,
    ) -> ResolvedWidth {
        let border = inset.max(0).saturating_mul(2) as u32;
        if let Some(scale) = cfg.projection_scale {
            let desired = (proportion * (view + cfg.column_gap as f64) - cfg.column_gap as f64)
                .clamp(Self::MIN_COLUMN_PX as f64, Self::MAX_COLUMN_PX as f64)
                .max(min as f64);
            let max = if max == 0 { u32::MAX } else { max.max(min) };
            let desired = desired.min(max as f64);
            // niri tile.rs:942-963 subtracts borders, then floors the LOGICAL
            // client request. Its settled visual size is physically rounded
            // (845-851); the unrounded request is never a retained tile frame.
            // Native/rule limits are physical in this engine. Bound the logical
            // request so its final rounded physical client honors those limits;
            // the minimum wins if this scale cannot represent both limits.
            let min_client = min.saturating_sub(border).max(1) as f64;
            let max_client = max.saturating_sub(border).max(1) as f64;
            let logical_min = ((min_client - 0.5) / scale).ceil().max(1.0) as i32;
            let logical_max =
                (((max_client + 0.5) / scale).ceil() - 1.0).max(logical_min as f64) as i32;
            let logical = (((desired - border as f64) / scale).floor() as i32)
                .clamp(logical_min, logical_max);
            let window_px = (logical as f64 * scale).round().max(1.0) as u32;
            ResolvedWidth {
                tile_px: window_px.saturating_add(border),
                window_px,
                window_logical: Some(logical),
            }
        } else {
            let tile_px = proportion_px(view, cfg.column_gap, proportion)
                .clamp(Self::MIN_COLUMN_PX, Self::MAX_COLUMN_PX) as u32;
            ResolvedWidth {
                tile_px,
                window_px: tile_px.saturating_sub(border),
                window_logical: None,
            }
        }
    }

    pub(super) fn resolved_window_geometry(
        &self,
        window: WindowId,
        mut tile: Rect,
        cfg: &LayoutConfig,
        inset: i32,
        resolve_proportion: bool,
    ) -> ResolvedWindowGeometry {
        let mut native = Rect::new(
            tile.loc.x + inset,
            tile.loc.y + inset,
            tile.size.w.saturating_sub(inset.max(0) as u32 * 2),
            tile.size.h.saturating_sub(inset.max(0) as u32 * 2),
        );
        let mut logical_window_size = (None, None);
        if resolve_proportion && cfg.projection_scale.is_some() {
            if let Some((output, workspace, column)) = self.locate_window(window) {
                if let Some(monitor) = self.monitors.get(&output) {
                    if let Some(ws) = monitor.workspaces.get(&workspace) {
                        if ws.layout_mode == crate::layout::workspace::WorkspaceLayout::Scrolling {
                            if let Some(column) = ws.columns.get(column).filter(|c| !c.maximized) {
                                if let Some(p) = column.width_proportion {
                                    let width = Self::proportional_width(
                                        Self::precise_tiling_area(monitor, cfg).width,
                                        cfg,
                                        p,
                                        inset,
                                        column.min_width(),
                                        column.max_width(),
                                    );
                                    tile.size.w = width.tile_px;
                                    native.size.w = width.window_px;
                                    logical_window_size.0 = width.window_logical;
                                }
                            }
                        }
                    }
                }
            }
        }
        ResolvedWindowGeometry {
            tile,
            window: native,
            logical_window_size,
        }
    }

    /// Compose signed struts and outer gaps without casting negative insets to u32.
    pub(super) fn tiling_work_rect(monitor: &crate::layout::Monitor, cfg: &LayoutConfig) -> Rect {
        Self::tiling_rect_in(monitor.work_area, cfg)
    }

    /// [`Self::tiling_work_rect`] for a bare work area (no monitor borrowed).
    pub(super) fn tiling_rect_in(work_area: Rect, cfg: &LayoutConfig) -> Rect {
        let area = Self::precise_tiling_area_in(work_area, cfg);
        // Explicit integer viewport adapter. Sizing consumes the precise area below.
        Rect::new(
            area.loc.x,
            area.loc.y,
            area.width.floor().clamp(0.0, i32::MAX as f64) as u32,
            area.height.floor().clamp(0.0, i32::MAX as f64) as u32,
        )
    }

    pub(crate) fn precise_tiling_area(
        monitor: &crate::layout::Monitor,
        cfg: &LayoutConfig,
    ) -> crate::layout::metrics::PhysicalWorkArea {
        Self::precise_tiling_area_in(monitor.work_area, cfg)
    }

    fn precise_tiling_area_in(
        work_area: Rect,
        cfg: &LayoutConfig,
    ) -> crate::layout::metrics::PhysicalWorkArea {
        use crate::layout::metrics::{spacing_work_rect, PhysicalSpacing};
        let struts = cfg.projected_struts.unwrap_or(crate::config::Edges {
            top: cfg.reserve_top as f64,
            right: cfg.reserve_right as f64,
            bottom: cfg.reserve_bottom as f64,
            left: cfg.reserve_left as f64,
        });
        // Keep the standalone engine's asymmetric physical outer-gap API.
        spacing_work_rect(
            work_area,
            &PhysicalSpacing {
                inner_gap_px: cfg.column_gap,
                outer_gap_px: 0,
                struts_px: crate::config::Edges {
                    top: struts.top + cfg.outer_gaps.0 as f64,
                    right: struts.right + cfg.outer_gaps.1 as f64,
                    bottom: struts.bottom + cfg.outer_gaps.2 as f64,
                    left: struts.left + cfg.outer_gaps.3 as f64,
                },
            },
        )
    }

    pub(super) fn refresh_strip_widths(&mut self, output: OutputId) {
        let base = self.config_per_monitor.get(&output).unwrap_or(&self.config);
        let per_workspace = &self.config_per_workspace;
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return;
        };
        let work_area = monitor.work_area;
        let scale = monitor.scale_factor;
        for ws in monitor.workspaces.values_mut() {
            let cfg = Self::workspace_layout_in(per_workspace, output, ws).unwrap_or(base);
            let view = Self::precise_tiling_area_in(work_area, cfg).width;
            for column in &mut ws.columns {
                // Fullscreen/maximize temporarily override the cached width.
                if column.maximized
                    || column.tiles.iter().any(|t| {
                        self.window_sizing
                            .get(&t.window_id)
                            .is_some_and(|s| *s != SizingMode::Normal)
                    })
                {
                    continue;
                }
                if let Some(p) = column.width_proportion {
                    let min = column.min_width();
                    let max = column.max_width();
                    column.width = Some(
                        column
                            .tiles
                            .iter()
                            .map(|tile| {
                                let inset = self.rule_state.get(&tile.window_id).map_or(
                                    cfg.border_inset(),
                                    |state| {
                                        state
                                            .resolved
                                            .effects
                                            .border
                                            .inset(&cfg.effects.border, cfg.border_inset())
                                    },
                                );
                                Self::proportional_width(view, cfg, p, inset, min, max).tile_px
                            })
                            .max()
                            .unwrap_or_else(|| {
                                Self::proportional_width(view, cfg, p, cfg.border_inset(), min, max)
                                    .tile_px
                            }),
                    );
                } else if let Some(logical) = column.width_fixed_logical {
                    // A target that left the column must not supply an unrelated
                    // window's border. Untargeted/inherited intent uses the workspace.
                    column.width_fixed_window = column
                        .width_fixed_window
                        .filter(|window| column.index_of(*window).is_some());
                    let inset = column
                        .width_fixed_window
                        .map_or(cfg.border_inset(), |window| {
                            self.rule_state
                                .get(&window)
                                .map_or(cfg.border_inset(), |state| {
                                    state
                                        .resolved
                                        .effects
                                        .border
                                        .inset(&cfg.effects.border, cfg.border_inset())
                                })
                        });
                    column.width = Some(
                        ((logical * scale).round() as i32)
                            .saturating_add(2 * inset)
                            .clamp(Self::MIN_COLUMN_PX, Self::MAX_COLUMN_PX)
                            as u32,
                    );
                }
            }
        }
    }

    pub(super) fn remember_column_width_spec(
        &mut self,
        id: WindowId,
        rule: Option<Option<SizeSpec>>,
        legacy_rule_width: Option<u32>,
    ) {
        // The legacy per-window rule still outranks a layout-wide default.
        if rule.is_none() && legacy_rule_width.is_some() {
            return;
        }
        let Some((output, ws, col)) = self.locate_window(id) else {
            return;
        };
        let cfg = self.effective_config_for_workspace(output, ws).clone();
        let spec = rule.or(cfg.default_column_width);
        let inset = self.rule_border_inset(id, &cfg);
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return;
        };
        let view = Self::precise_tiling_area(monitor, &cfg).width;
        let scale = monitor.scale_factor;
        let Some(column) = monitor
            .workspaces
            .get_mut(&ws)
            .and_then(|w| w.columns.get_mut(col))
        else {
            return;
        };
        // Explicit insertion into a stack does not resize its established column.
        if column.tiles.len() != 1 {
            return;
        }
        // A workspace-level default must not overwrite a width the user (or a window)
        // already gave this column; a window rule still does.
        if rule.is_none()
            && (column.width_proportion.is_some() || column.width_fixed_logical.is_some())
        {
            return;
        }
        match spec {
            Some(Some(SizeSpec::Proportion(p))) => {
                column.set_proportion(p, view, cfg.column_gap);
                column.width = Some(
                    Self::proportional_width(
                        view,
                        &cfg,
                        p,
                        inset,
                        column.min_width(),
                        column.max_width(),
                    )
                    .tile_px,
                );
            }
            Some(Some(SizeSpec::Fixed(px))) => {
                column.set_fixed_logical(px as f64, scale, 2 * cfg.border_width.max(1))
            }
            Some(None) => {
                if let Some(window) = self.tiled_windows.get(&id) {
                    column.set_width(Some(
                        (window.bounds.size.w as i32 + 2 * cfg.border_width.max(1))
                            .min(view.floor() as i32)
                            .max(50) as u32,
                    ));
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Column, Monitor, Workspace};

    #[test]
    fn signed_struts_expand_or_contract_the_work_area_before_gaps() {
        let m = Monitor::new(
            OutputId::from_name("struts"),
            Rect::new(0, 0, 1200, 800),
            Rect::new(40, 30, 1100, 700),
        );
        let cfg = LayoutConfig {
            reserve_left: -20,
            reserve_right: 30,
            reserve_top: 15,
            reserve_bottom: -5,
            outer_gaps: (10, 10, 10, 10),
            ..LayoutConfig::default()
        };
        assert_eq!(
            TilingEngine::tiling_work_rect(&m, &cfg),
            Rect::new(30, 55, 1070, 670)
        );
        let cfg = LayoutConfig {
            reserve_left: 5000,
            ..cfg
        };
        assert_eq!(TilingEngine::tiling_work_rect(&m, &cfg).size.w, 0);
    }

    #[test]
    fn requested_widths_survive_insert_remove_and_view_change() {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let out = OutputId::from_name("width-memory");
        engine.register_monitor(out, Rect::new(0, 0, 1200, 800), Rect::new(0, 0, 1200, 760));
        let m = engine.monitors.get_mut(&out).unwrap();
        let ws = m.workspace_mut().unwrap();
        ws.insert_window_after_active(WindowId::new(1), None);
        ws.columns[0].set_proportion(0.5, 1184, 16);
        ws.insert_window_after_active(WindowId::new(2), Some(300));
        ws.remove_window(WindowId::new(2));
        assert_eq!(ws.columns[0].width_proportion, Some(0.5));
        engine.monitors.get_mut(&out).unwrap().work_area.size.w = 1600;
        engine.refresh_strip_widths(out);
        assert_eq!(
            engine
                .monitors
                .get(&out)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .width,
            Some(784)
        );
    }

    #[test]
    fn fixed_logical_width_resolves_at_output_scale() {
        let mut column = Column::new();
        column.set_fixed_logical(400.0, 1.5, 8);
        assert_eq!(column.width, Some(608));
        column.set_width(Some(500));
        assert_eq!(column.width_fixed_logical, None);
    }

    #[test]
    fn column_and_workspace_memory_is_not_bounded_by_mru_capacity() {
        let mut m = Monitor::new(
            OutputId::from_name("memory"),
            Rect::new(0, 0, 1000, 800),
            Rect::new(0, 0, 1000, 760),
        );
        m.add_window(WindowId::new(1));
        m.workspace_mut()
            .unwrap()
            .add_window_to_column(0, WindowId::new(2));
        m.set_focus(WindowId::new(2));
        m.add_window(WindowId::new(3));
        m.focus_left();
        assert_eq!(m.focus_window, Some(WindowId::new(2)));
        m.switch_workspace(1);
        for id in 4..80 {
            m.add_window(WindowId::new(id));
        }
        m.switch_workspace(0);
        assert_eq!(m.focus_window, Some(WindowId::new(2)));
        m.focus_right();
        assert_eq!(m.focus_window, Some(WindowId::new(3)));
        // Removing the remembered tile returns to the surviving tile in that column.
        m.workspace_mut().unwrap().remove_window(WindowId::new(2));
        m.focus_left();
        assert_eq!(m.focus_window, Some(WindowId::new(1)));
    }

    #[test]
    fn insertion_is_right_of_active_in_middle_at_end_and_when_empty() {
        let mut ws = Workspace::new();
        for id in [1, 2, 3] {
            ws.insert_window_after_active(WindowId::new(id), Some(id as u32 * 100));
        }
        ws.remember_focus(WindowId::new(1));
        assert_eq!(
            ws.insert_window_after_active(WindowId::new(4), Some(400)),
            1
        );
        assert_eq!(
            ws.columns
                .iter()
                .map(|c| c.tiles[0].window_id.as_isize())
                .collect::<Vec<_>>(),
            vec![1, 4, 2, 3]
        );
        ws.remember_focus(WindowId::new(3));
        assert_eq!(ws.insert_window_after_active(WindowId::new(5), None), 4);
        assert_eq!(ws.active_window, Some(WindowId::new(5)));
        assert_eq!(ws.columns[0].width, Some(100));
    }

    #[test]
    fn expel_and_minimize_keep_proportion_not_only_pixels() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_column(0, WindowId::new(2));
        ws.columns[0].set_proportion(0.5, 1000, 20);
        ws.consume_or_expel_right(WindowId::new(2));
        assert_eq!(ws.columns[1].width_proportion, Some(0.5));
        let slot = ws.slot_of(WindowId::new(2)).unwrap();
        ws.remove_window(WindowId::new(2));
        let col = ws.put_back(slot);
        assert_eq!(ws.columns[col].width_proportion, Some(0.5));
    }

    fn one_window() -> (TilingEngine, OutputId, WindowId, BackendHandle) {
        let be = BackendHandle::default_for_test();
        let out = OutputId::from_name("width-regression");
        let id = WindowId::new(905);
        let mut engine = TilingEngine::new(LayoutConfig {
            outer_gaps: (0, 0, 0, 0),
            column_gap: 0,
            ..LayoutConfig::default()
        });
        engine.register_monitor(out, Rect::new(0, 0, 600, 800), Rect::new(0, 0, 600, 760));
        engine.add_window(
            WindowInfo {
                hwnd: id.as_isize(),
                title: "width regression".into(),
                class_name: "test".into(),
                process_id: 0,
                bounds: Rect::new(10, 10, 200, 300),
                state: crate::backend::WindowState::Normal,
                is_visible: true,
            },
            &be,
        );
        (engine, out, id, be)
    }

    #[test]
    fn small_percentage_remains_requested_proportion_after_minimum_clamp() {
        let (mut engine, out, _, be) = one_window();
        engine.set_column_width_by(SizeChange::SetProportion(5_000), &be);
        let ws = engine.monitors.get(&out).unwrap().workspace().unwrap();
        assert_eq!(ws.columns[0].width, Some(50));
        assert_eq!(ws.columns[0].width_proportion, Some(0.05));
        engine.monitors.get_mut(&out).unwrap().work_area.size.w = 1200;
        engine.apply_all(&be);
        assert_eq!(
            engine
                .monitors
                .get(&out)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .width,
            Some(60)
        );
        engine.set_column_width_by(SizeChange::AdjustProportion(5_000), &be);
        assert_eq!(
            engine
                .monitors
                .get(&out)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .width_proportion,
            Some(0.1)
        );
    }

    #[test]
    fn restore_into_another_workspace_keeps_width_intent() {
        let (mut engine, out, id, be) = one_window();
        engine.set_column_width_by(SizeChange::SetProportion(50_000), &be);
        engine.minimize_window(id, &be);
        engine.monitors.get_mut(&out).unwrap().switch_workspace(1);
        engine.monitors.get_mut(&out).unwrap().work_area.size.w = 1200;
        engine.restore_window(id, &be);
        let ws = engine.monitors.get(&out).unwrap().workspace().unwrap();
        assert_eq!(ws.columns[0].width_proportion, Some(0.5));
        assert_eq!(ws.columns[0].width, Some(600));
    }

    #[test]
    fn workspace_transfer_keeps_width_request_and_source_focus() {
        let (mut engine, out, id, be) = one_window();
        engine.set_column_width_by(SizeChange::SetProportion(50_000), &be);
        engine.move_window_to_workspace(1, &be);
        let m = engine.monitors.get(&out).unwrap();
        assert_eq!(m.active_workspace, 0);
        assert_ne!(m.focus_window, Some(id));
        assert_eq!(m.workspaces[&1].columns[0].width_proportion, Some(0.5));
        engine.monitors.get_mut(&out).unwrap().work_area.size.w = 1200;
        engine.refresh_strip_widths(out);
        assert_eq!(
            engine.monitors.get(&out).unwrap().workspaces[&1].columns[0].width,
            Some(600)
        );
    }

    #[test]
    fn monitor_transfer_resolves_fixed_logical_width_at_destination_scale() {
        let (mut engine, out, id, be) = one_window();
        engine.set_column_width_by(SizeChange::SetFixed(200), &be);
        let other = OutputId::from_name("other-scale");
        engine.register_monitor(
            other,
            Rect::new(600, 0, 1200, 800),
            Rect::new(600, 0, 1200, 760),
        );
        engine.monitors.get_mut(&other).unwrap().scale_factor = 1.5;
        engine.monitors.set_focused(out);
        engine.move_window_to_monitor(ScrollDirection::Right, &be);
        let m = engine.monitors.get(&other).unwrap();
        assert_eq!(m.focus_window, Some(id));
        assert_eq!(
            m.workspace().unwrap().columns[0].width_fixed_logical,
            Some(200.0)
        );
        assert_eq!(m.workspace().unwrap().columns[0].width, Some(308));
    }

    #[test]
    fn center_action_centers_first_and_last_even_in_never_policy() {
        let (mut engine, out, first, be) = one_window();
        // The action must use the effective output's struts and gap, too.
        engine.config_per_monitor.insert(
            out,
            LayoutConfig {
                outer_gaps: (0, 20, 0, 30),
                reserve_right: 50,
                column_gap: 40,
                ..engine.config.clone()
            },
        );
        let m = engine.monitors.get_mut(&out).unwrap();
        m.workspace_mut().unwrap().columns[0].set_width(Some(200));
        m.add_window_with_width(WindowId::new(906), Some(200));
        for (id, expected) in [(first, -150), (WindowId::new(906), 90)] {
            engine.monitors.get_mut(&out).unwrap().set_focus(id);
            engine.center_focused_column(&be);
            assert_eq!(
                engine
                    .monitors
                    .get(&out)
                    .unwrap()
                    .workspace()
                    .unwrap()
                    .scroll_offset
                    .x,
                expected
            );
        }
    }

    #[test]
    fn inactive_admission_keeps_the_workspaces_remembered_focus() {
        let (mut engine, out, first, _) = one_window();
        {
            let m = engine.monitors.get_mut(&out).unwrap();
            m.switch_workspace(1);
            m.add_window(WindowId::new(907));
            m.switch_workspace(0);
        }
        engine.place_new_tile(WindowId::new(908), out, Some(1), None);
        let m = engine.monitors.get_mut(&out).unwrap();
        assert_eq!(m.focus_window, Some(first));
        m.switch_workspace(1);
        assert_eq!(m.focus_window, Some(WindowId::new(907)));
        m.switch_workspace(0);
        assert_eq!(m.focus_window, Some(first));
    }

    #[test]
    fn legacy_rule_width_outranks_layout_default_but_niri_rule_wins() {
        let (mut engine, out, id, _) = one_window();
        engine.config.default_column_width = Some(Some(SizeSpec::Proportion(0.5)));
        engine
            .monitors
            .get_mut(&out)
            .unwrap()
            .workspace_mut()
            .unwrap()
            .columns[0]
            .set_width(Some(240));
        engine.remember_column_width_spec(id, None, Some(240));
        assert_eq!(
            engine
                .monitors
                .get(&out)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .width,
            Some(240)
        );
        engine.remember_column_width_spec(id, Some(Some(SizeSpec::Fixed(300))), Some(240));
        assert_eq!(
            engine
                .monitors
                .get(&out)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .width_fixed_logical,
            Some(300.0)
        );
    }
}
