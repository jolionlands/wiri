//! Event-driven hookups for sparse appearance rules. No timers or separate resources.
use super::{BackendHandle, TilingEngine};
use crate::utils::{OutputId, Rect, WindowId};

impl TilingEngine {
    pub(super) fn rule_border_inset(&self, window: WindowId, cfg: &super::LayoutConfig) -> i32 {
        self.rule_state
            .get(&window)
            .map_or(cfg.border_inset(), |state| {
                cfg.border_inset_with(&state.resolved.effects.border)
            })
    }

    pub(super) fn rule_corner_radius(&self, window: WindowId, _output: OutputId) -> Option<u32> {
        self.rule_state.get(&window)?.resolved.effects.dwm_radius()
    }

    /// Reconcile an opt-in clip against the HWND's observed physical frame.
    /// Removing the request must restore even when no live rectangle is readable.
    pub(super) fn sync_native_clip(&mut self, window: WindowId) {
        let corners = self.rule_state.get(&window).and_then(|state| {
            let effects = &state.resolved.effects;
            (effects.clip_to_geometry == Some(true))
                .then_some(effects.geometry_corner_radius.unwrap_or([0.0; 4]))
        });
        let Some(corners) = corners else {
            self.native_clips.sync(window, Rect::default(), None);
            return;
        };
        let output = self
            .locate_window(window)
            .map(|(output, _, _)| output)
            .or_else(|| self.float_location(window).map(|(output, _)| output));
        let scale = output
            .and_then(|output| self.monitors.get(&output))
            .map_or(1.0, |monitor| monitor.scale_factor as f32);
        // live_window_rect prefers DWM's extended (visible) frame bounds.
        // ClipState also rechecks DWM/native measurements, rejecting a native
        // fallback of a different size or an asynchronously pending target.
        if let Some(visible) = Self::live_window_rect(window) {
            self.native_clips
                .sync(window, visible, Some(corners.map(|r| r * scale)));
        }
    }

    pub(super) fn refresh_rule_effects(&mut self, window: WindowId, backend: &BackendHandle) {
        self.sync_min_widths();
        if let Some((output, _)) = self.float_location(window) {
            let radius = self
                .rule_corner_radius(window, output)
                .or(self.effective_config(output).effects.corner_radius);
            self.apply_corner_for_window(
                window.as_isize(),
                radius.map_or(0, |r| super::decor::dwm_corner_for(r).0),
            );
            self.apply_layout_for_monitor(output, backend);
        } else if let Some((output, _, _)) = self.locate_window(window) {
            self.apply_layout_for_monitor(output, backend);
        }
        self.sync_native_clip(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::engine::rules::RuleState;
    #[test]
    fn border_keys_are_per_window_not_per_output() {
        let mut engine = TilingEngine::new(Default::default());
        let window = WindowId::new(123);
        let mut resolved = crate::window::ResolvedWindowRules::default();
        resolved.effects.border.width = Some(12.0);
        engine.rule_state.insert(
            window,
            RuleState {
                resolved,
                process_name: None,
                aumid: None,
            },
        );
        let cfg = super::super::LayoutConfig::default();
        assert_eq!(engine.rule_border_inset(window, &cfg), 12);
        assert_eq!(engine.rule_border_inset(WindowId::new(124), &cfg), 4);
        engine
            .rule_state
            .get_mut(&window)
            .unwrap()
            .resolved
            .effects
            .border
            .off = Some(true);
        assert_eq!(engine.rule_border_inset(window, &cfg), 0);
    }
}
