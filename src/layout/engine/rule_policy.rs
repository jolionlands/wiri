//! Placement defaults are used only at a transition, never to mutate an existing column.
use super::{BackendHandle, TilingEngine};
use crate::utils::{OutputId, Rect, WindowId};
use crate::window::rule_policy::{OnXdgActivate, RulePolicy};

impl TilingEngine {
    pub(super) fn window_policy(&self, window: WindowId) -> RulePolicy {
        self.rule_state
            .get(&window)
            .map_or_else(RulePolicy::default, |s| s.resolved.policy)
    }

    pub(super) fn apply_new_column_policy(&mut self, window: WindowId) {
        let Some((output, workspace, index)) = self.locate_window(window) else {
            return;
        };
        let tabbed = self
            .window_policy(window)
            .column_is_tabbed(self.effective_config(output).default_tabbed);
        if let Some(column) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspaces.get_mut(&workspace))
            .and_then(|w| w.columns.get_mut(index))
        {
            if column.tiles.len() == 1 {
                if tabbed {
                    column.switch_to_tabbed();
                } else {
                    column.switch_to_stacked();
                }
            }
            column.select_window(window);
        }
    }

    pub(super) fn first_float_position(
        &self,
        window: WindowId,
        output: OutputId,
        fallback: Rect,
    ) -> Rect {
        let Some(monitor) = self.monitors.get(&output) else {
            return fallback;
        };
        self.window_policy(window)
            .floating_rect(monitor.work_area, fallback, monitor.scale_factor)
    }

    /// Only actual shell flash/ALERT requests reach here, never arbitrary foreground changes.
    /// Windows supplies no Wayland serial provenance, so the default is attention, not focus.
    pub(super) fn requested_activation(&mut self, window: WindowId, backend: &BackendHandle) {
        match self
            .window_policy(window)
            .activation_action(OnXdgActivate::SetUrgent)
        {
            OnXdgActivate::SetUrgent => {
                self.mark_urgent(window);
            }
            action => {
                crate::backend::hooks::clear_urgent(window.as_isize());
                self.clear_urgent(window);
                if action == OnXdgActivate::Focus {
                    if self.is_minimized(window) {
                        self.restore_window(window, backend);
                    }
                    if !self.focus_float(window, backend) {
                        if let Some((output, workspace, _)) = self.locate_window(window) {
                            self.monitors.set_focused(output);
                            if let Some(monitor) = self.monitors.get_mut(&output) {
                                monitor.active_workspace = workspace;
                                monitor.set_focus(window);
                            }
                            self.reveal_focused_column(output);
                        }
                    }
                    self.last_activated = None;
                    self.apply_all(backend);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::engine::rules::RuleState;
    use crate::layout::workspace::ColumnDisplay;
    use crate::layout::Tile;
    use crate::window::rule_policy::{DefaultColumnDisplay, FloatingPosition, RelativeTo};

    fn fixture() -> (TilingEngine, WindowId, OutputId) {
        let mut engine = TilingEngine::new(Default::default());
        let output = OutputId::from_name("policy-test");
        engine.register_monitor(
            output,
            Rect::new(100, 50, 1000, 800),
            Rect::new(100, 50, 1000, 760),
        );
        let window = WindowId::new(716_001);
        engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .add_window_with_width(window, None);
        engine.rule_state.insert(
            window,
            RuleState {
                resolved: Default::default(),
                process_name: None,
                aumid: None,
            },
        );
        (engine, window, output)
    }

    #[test]
    fn explicit_normal_overrides_global_tabbed_and_transitions_can_use_changed_default() {
        let (mut engine, window, output) = fixture();
        engine.config.default_tabbed = true;
        engine
            .rule_state
            .get_mut(&window)
            .unwrap()
            .resolved
            .policy
            .default_column_display = Some(DefaultColumnDisplay::Normal);
        engine.apply_new_column_policy(window);
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .display,
            ColumnDisplay::Stacked
        );
        engine
            .rule_state
            .get_mut(&window)
            .unwrap()
            .resolved
            .policy
            .default_column_display = Some(DefaultColumnDisplay::Tabbed);
        engine.apply_new_column_policy(window);
        assert!(matches!(
            engine
                .monitors
                .get(&output)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .display,
            ColumnDisplay::Tabbed { active_tab: 0 }
        ));
    }

    #[test]
    fn adding_to_an_existing_column_never_changes_its_display() {
        let (mut engine, window, output) = fixture();
        let ws = engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspace_mut()
            .unwrap();
        ws.columns[0].add_tile(Tile::new(WindowId::new(716_002)));
        engine
            .rule_state
            .get_mut(&window)
            .unwrap()
            .resolved
            .policy
            .default_column_display = Some(DefaultColumnDisplay::Tabbed);
        engine.apply_new_column_policy(window);
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .display,
            ColumnDisplay::Stacked
        );
    }

    #[test]
    fn first_float_uses_work_origin_and_dpi_without_replacing_size() {
        let (mut engine, window, output) = fixture();
        engine
            .rule_state
            .get_mut(&window)
            .unwrap()
            .resolved
            .policy
            .default_floating_position = Some(FloatingPosition {
            x: 20.0,
            y: -10.0,
            relative_to: RelativeTo::BottomRight,
        });
        engine.monitors.get_mut(&output).unwrap().scale_factor = 1.5;
        assert_eq!(
            engine.first_float_position(window, output, Rect::new(0, 0, 200, 100)),
            Rect::new(870, 725, 200, 100)
        );
    }
}
