use super::*;
use crate::config::types::SizeSpec;

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
