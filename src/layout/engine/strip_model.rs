//! Admission-time width intent and output-dependent resolution. The legacy `width`
//! remains the pixel cache used by all existing layout algorithms.
use super::*;
use crate::config::types::SizeSpec;

impl TilingEngine {
    /// Compose signed struts and outer gaps without casting negative insets to u32.
    pub(super) fn tiling_work_rect(monitor: &crate::layout::Monitor, cfg: &LayoutConfig) -> Rect {
        let left = cfg.reserve_left as i64 + cfg.outer_gaps.3 as i64;
        let right = cfg.reserve_right as i64 + cfg.outer_gaps.1 as i64;
        let top = cfg.reserve_top as i64 + cfg.outer_gaps.0 as i64;
        let bottom = cfg.reserve_bottom as i64 + cfg.outer_gaps.2 as i64;
        let coord = |value: i64| value.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let size = |value: i64| value.clamp(0, i32::MAX as i64) as u32;
        Rect::new(
            coord(monitor.work_area.loc.x as i64 + left),
            coord(monitor.work_area.loc.y as i64 + top),
            size(monitor.work_area.size.w as i64 - left - right),
            size(monitor.work_area.size.h as i64 - top - bottom),
        )
    }

    pub(super) fn refresh_strip_widths(&mut self, output: OutputId) {
        let cfg = self.effective_config(output).clone();
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return;
        };
        let view = Self::view_width_for_monitor(monitor, &cfg);
        let scale = monitor.scale_factor;
        for ws in monitor.workspaces.values_mut() {
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
                    column.width = Some(
                        proportion_px(view, cfg.column_gap, p)
                            .clamp(Self::MIN_COLUMN_PX, Self::MAX_COLUMN_PX)
                            as u32,
                    );
                } else if let Some(logical) = column.width_fixed_logical {
                    column.width = Some(
                        ((logical * scale).round() as i32 + 2 * cfg.border_width.max(1))
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
        let cfg = self.effective_config(output).clone();
        let spec = rule.or(cfg.default_column_width);
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return;
        };
        let view = Self::view_width_for_monitor(monitor, &cfg);
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
            Some(Some(SizeSpec::Proportion(p))) => column.set_proportion(p, view, cfg.column_gap),
            Some(Some(SizeSpec::Fixed(px))) => {
                column.set_fixed_logical(px as f64, scale, 2 * cfg.border_width.max(1))
            }
            Some(None) => {
                if let Some(window) = self.tiled_windows.get(&id) {
                    column.set_width(Some(
                        (window.bounds.size.w as i32 + 2 * cfg.border_width.max(1))
                            .min(view)
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
