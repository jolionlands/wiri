//! One compositor-wide startup rule phase, independent of initial window discovery.
//!
//! Pinned niri `niri.rs`: one 60-second timer changes the matcher context and
//! recomputes existing rules. Reloads and individual admissions never restart it.

use super::TilingEngine;
use crate::backend::BackendHandle;
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub(super) struct StartupRules {
    deadline: Instant,
    active: bool,
}

impl StartupRules {
    pub(super) fn new(start: Instant) -> Self {
        Self {
            deadline: start + Duration::from_secs(60),
            active: true,
        }
    }

    pub(super) fn active(&self) -> bool {
        self.active
    }

    fn finish(&mut self, now: Instant) -> bool {
        if !self.active || now < self.deadline {
            return false;
        }
        self.active = false;
        true
    }
}

impl TilingEngine {
    /// The fixed deadline established before initial enumeration and armed once.
    pub fn startup_rules_deadline(&self) -> Instant {
        self.startup_rules.deadline
    }

    /// Consume the startup timeout and update dynamic rules without replaying opens.
    /// Returns true only for the first expiry at or after the fixed deadline.
    pub fn finish_startup_rules(&mut self, now: Instant, backend: &BackendHandle) -> bool {
        if self.shut_down || !self.startup_rules.finish(now) {
            return false;
        }
        // Opacity, paint-only effects, and admission-only rules do not change
        // the strip. Remember the tiled geometry inputs so those updates leave
        // user-panned views alone, including views on other outputs.
        let geometry_before: Vec<_> = self
            .rule_state
            .keys()
            .filter_map(|&window| {
                let (output, workspace, _) = self.locate_window(window)?;
                let cfg = self.effective_config_for_workspace(output, workspace);
                Some((
                    window,
                    output,
                    workspace,
                    self.effective_window_limits(window),
                    self.rule_border_inset(window, cfg),
                ))
            })
            .collect();
        self.reevaluate_startup_window_rules(backend);
        let mut outputs = HashSet::new();
        for (window, output, workspace, limits, inset) in geometry_before {
            let cfg = self.effective_config_for_workspace(output, workspace);
            if self.effective_window_limits(window) != limits
                || self.rule_border_inset(window, cfg) != inset
            {
                outputs.insert(output);
            }
        }
        // Individual rule updates can submit intermediate layouts while other
        // windows still have startup limits. Reconcile the viewport only after
        // every window has its final limits, then submit that settled layout.
        for &output in &outputs {
            self.reveal_focused_column(output);
        }
        // An intermediate configure (or one just before expiry) must not rate
        // limit this final submission: the one-shot timeout has no later retry.
        for output in outputs {
            if let Some(monitor) = self.monitors.get_mut(&output) {
                for workspace in monitor.workspaces.values_mut() {
                    for column in &mut workspace.columns {
                        for tile in &mut column.tiles {
                            tile.configure_throttle.last_send = None;
                        }
                    }
                }
            }
            self.apply_layout_for_monitor(output, backend);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{WindowInfo, WindowState};
    use crate::config::types::{MatchGroup, Matcher, RegexPat, WindowRule};
    use crate::layout::engine::{AddWindowTarget, LayoutConfig};
    use crate::layout::ScrollDirection;
    use crate::utils::{OutputId, Rect, WindowId};

    fn expiry_applies_final_geometry(throttled_before_expiry: bool) {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig {
            outer_gaps: (0, 0, 0, 0),
            column_gap: 0,
            window_gap: 0,
            border_width: 0,
            focus_ring_width: 0,
            ..LayoutConfig::default()
        });
        engine.configure_motion(&crate::config::AnimationsConfig {
            enabled: false,
            ..crate::config::AnimationsConfig::default()
        });
        let output = OutputId::from_name("startup-expiry");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        engine.set_window_rules(vec![
            WindowRule {
                match_groups: vec![MatchGroup {
                    conds: vec![Matcher::AtStartup],
                }],
                min_width: Some(360),
                max_width: Some(360),
                min_height: Some(300),
                max_height: Some(300),
                // Fixed-height windows float by default; this regression needs
                // explicitly tiled windows so it exercises configure throttling.
                open_floating: Some(false),
                ..WindowRule::default()
            },
            WindowRule {
                match_groups: vec![MatchGroup {
                    conds: vec![Matcher::Not(Box::new(Matcher::AtStartup))],
                }],
                min_width: Some(240),
                max_width: Some(240),
                min_height: Some(400),
                max_height: Some(400),
                open_floating: Some(false),
                ..WindowRule::default()
            },
        ]);
        for hwnd in [1, 2] {
            engine.add_window_at_startup(
                WindowInfo {
                    hwnd,
                    title: format!("startup window {hwnd}"),
                    class_name: "TestClass".to_string(),
                    process_id: 1000,
                    bounds: Rect::new(100, 100, 700, 500),
                    state: WindowState::Normal,
                    is_visible: true,
                },
                &backend,
            );
        }

        let workspace = engine.monitors.get(&output).unwrap().workspace().unwrap();
        assert_eq!(
            workspace.columns.len(),
            2,
            "fixture needs two tiled columns"
        );
        for (index, id) in [1, 2].into_iter().enumerate() {
            let window = WindowId::new(id);
            assert!(!engine.floating_windows.contains(&window));
            assert_eq!(workspace.columns[index].tiles.len(), 1);
            assert_eq!(workspace.columns[index].tiles[0].window_id, window);
        }

        // Establish a fully submitted startup layout without depending on how
        // quickly admission ran relative to the normal 16 ms rate limit.
        for workspace in engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspaces
            .values_mut()
        {
            for column in &mut workspace.columns {
                for tile in &mut column.tiles {
                    tile.configure_throttle.min_interval = Duration::ZERO;
                }
            }
        }
        engine.apply_all(&backend);
        for (id, x) in [(1, 0), (2, 360)] {
            assert_eq!(
                engine.applied_state[&WindowId::new(id)].rect,
                Rect::new(x, 0, 360, 300),
                "startup geometry must be established before testing expiry"
            );
        }

        // Duration::MAX keeps the condition deterministic without sleeping. In
        // the unthrottled case, the first partial rule update can send geometry
        // for both tiles, throttling later passes before all limits are final.
        for workspace in engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspaces
            .values_mut()
        {
            for column in &mut workspace.columns {
                for tile in &mut column.tiles {
                    tile.configure_throttle.min_interval = Duration::MAX;
                    tile.configure_throttle.last_send = throttled_before_expiry.then(Instant::now);
                }
            }
        }
        let deadline = engine.startup_rules_deadline();
        assert!(engine.finish_startup_rules(deadline, &backend));

        let workspace = engine.monitors.get(&output).unwrap().workspace().unwrap();
        for tile in workspace.columns.iter().flat_map(|column| &column.tiles) {
            assert_eq!((tile.min_width, tile.max_width), (240, 240));
            assert_eq!((tile.min_height, tile.max_height), (400, 400));
        }
        // No unrelated event or subsequent layout pass should be necessary:
        // expiry must submit the layout after every window has its final limits.
        let actual: Vec<_> = [1, 2]
            .into_iter()
            .map(|id| engine.applied_state[&WindowId::new(id)].rect)
            .collect();
        assert_eq!(
            actual,
            vec![Rect::new(0, 0, 240, 400), Rect::new(240, 0, 240, 400)],
            "expiry must submit both final native rectangles despite configure throttling"
        );
        assert!(!engine.finish_startup_rules(deadline, &backend));
    }

    #[test]
    fn startup_rule_phase_expiry_submits_final_geometry_when_already_throttled() {
        expiry_applies_final_geometry(true);
    }

    #[test]
    fn startup_rule_phase_expiry_submits_final_geometry_after_partial_pass() {
        expiry_applies_final_geometry(false);
    }

    fn fixture_window(hwnd: isize, title: &str) -> WindowInfo {
        WindowInfo {
            hwnd,
            title: title.to_string(),
            class_name: "TestClass".to_string(),
            process_id: 1000,
            bounds: Rect::new(100, 100, 700, 500),
            state: WindowState::Normal,
            is_visible: true,
        }
    }

    fn set_fixture_throttle(engine: &mut TilingEngine, interval: Duration) {
        for monitor in engine.monitors.values_mut() {
            for workspace in monitor.workspaces.values_mut() {
                for column in &mut workspace.columns {
                    for tile in &mut column.tiles {
                        tile.configure_throttle.min_interval = interval;
                        tile.configure_throttle.last_send = Some(Instant::now());
                    }
                }
            }
        }
    }

    fn panned_fixture(rules: Vec<WindowRule>) -> (TilingEngine, BackendHandle, OutputId) {
        panned_fixture_with_native_bounds(rules, None, None)
    }

    fn panned_fixture_with_native_bounds(
        rules: Vec<WindowRule>,
        seed: Option<crate::backend::NativeBoundsSample>,
        target_title: Option<&str>,
    ) -> (TilingEngine, BackendHandle, OutputId) {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig {
            outer_gaps: (0, 0, 0, 0),
            column_gap: 0,
            window_gap: 0,
            border_width: 0,
            focus_ring_width: 0,
            ..LayoutConfig::default()
        });
        engine.configure_motion(&crate::config::AnimationsConfig {
            enabled: false,
            ..crate::config::AnimationsConfig::default()
        });
        let output = OutputId::from_name("panned");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        engine.set_window_rules(rules);
        for hwnd in 10..18 {
            engine.add_window_with_native_bounds(
                fixture_window(
                    hwnd,
                    if hwnd == 10 {
                        target_title.unwrap_or("panned window")
                    } else {
                        "panned window"
                    },
                ),
                &backend,
                true,
                seed,
            );
        }
        engine.reveal_focused();
        set_fixture_throttle(&mut engine, Duration::ZERO);
        engine.apply_all(&backend);
        let focused = engine.monitors.get(&output).unwrap().focus_window;
        let revealed = scroll_offset(&engine, output);
        assert!(revealed >= 200, "fixture must have a scrollable strip");
        engine.scroll(ScrollDirection::Left, &backend);
        assert_eq!(scroll_offset(&engine, output), revealed - 200);
        assert_eq!(engine.monitors.get(&output).unwrap().focus_window, focused);
        (engine, backend, output)
    }

    fn scroll_offset(engine: &TilingEngine, output: OutputId) -> i32 {
        engine
            .monitors
            .get(&output)
            .unwrap()
            .workspace()
            .unwrap()
            .scroll_offset
            .x
    }

    fn assert_expiry_keeps_pan(rules: Vec<WindowRule>, expected_opacity: Option<u32>) {
        let (mut engine, backend, output) = panned_fixture(rules);
        let before = scroll_offset(&engine, output);
        let focused = engine.monitors.get(&output).unwrap().focus_window;
        let rectangles: Vec<_> = (10..18)
            .map(|id| engine.applied_state[&WindowId::new(id)].rect)
            .collect();

        assert!(engine.finish_startup_rules(engine.startup_rules_deadline(), &backend));

        assert_eq!(
            scroll_offset(&engine, output),
            before,
            "expiry must preserve user pan"
        );
        assert_eq!(engine.monitors.get(&output).unwrap().focus_window, focused);
        for (id, rectangle) in (10..18).zip(rectangles) {
            let applied = &engine.applied_state[&WindowId::new(id)];
            assert_eq!(
                applied.rect, rectangle,
                "unchanged layout must keep native geometry"
            );
            if let Some(opacity) = expected_opacity {
                assert_eq!(applied.opacity, opacity);
            }
        }
    }

    fn startup_opacity_rules(startup: f64, runtime: f64) -> Vec<WindowRule> {
        vec![
            WindowRule {
                match_groups: vec![MatchGroup {
                    conds: vec![Matcher::AtStartup],
                }],
                opacity: Some(startup),
                ..WindowRule::default()
            },
            WindowRule {
                match_groups: vec![MatchGroup {
                    conds: vec![Matcher::Not(Box::new(Matcher::AtStartup))],
                }],
                opacity: Some(runtime),
                ..WindowRule::default()
            },
        ]
    }

    #[test]
    fn startup_rule_phase_expiry_preserves_panned_view_without_rules() {
        assert_expiry_keeps_pan(Vec::new(), None);
    }

    #[test]
    fn startup_rule_phase_expiry_preserves_panned_view_with_unchanged_rules() {
        assert_expiry_keeps_pan(startup_opacity_rules(0.4, 0.4), Some(102));
    }

    #[test]
    fn startup_rule_phase_expiry_preserves_panned_view_on_opacity_change() {
        assert_expiry_keeps_pan(startup_opacity_rules(0.4, 0.8), Some(204));
    }

    #[test]
    fn startup_rule_phase_expiry_preserves_panned_view_when_native_minimum_masks_rule_change() {
        use crate::backend::{NativeBounds, NativeBoundsContext, NativeBoundsSample};
        let rules = [
            (Matcher::AtStartup, 100),
            (Matcher::Not(Box::new(Matcher::AtStartup)), 200),
        ]
        .into_iter()
        .map(|(phase, minimum)| WindowRule {
            match_groups: vec![MatchGroup {
                conds: vec![
                    phase,
                    Matcher::TitleSearch(RegexPat::new("^masked target$").unwrap()),
                ],
            }],
            min_width: Some(minimum),
            ..WindowRule::default()
        })
        .collect();
        let seed = NativeBoundsSample {
            bounds: NativeBounds {
                min_width: 600,
                ..NativeBounds::default()
            },
            context: NativeBoundsContext {
                dpi: 96,
                process_id: 1000,
                thread_id: 7,
                ..NativeBoundsContext::default()
            },
        };
        let (mut engine, backend, output) =
            panned_fixture_with_native_bounds(rules, Some(seed), Some("masked target"));
        let before = scroll_offset(&engine, output);
        assert_eq!(before, 2680, "eight 600px columns, then a 200px user pan");
        let focused = engine.monitors.get(&output).unwrap().focus_window;
        let target = WindowId::new(10);
        assert_eq!(focused, Some(WindowId::new(17)));
        assert_ne!(
            focused,
            Some(target),
            "only an inactive column changes its rule"
        );
        let effective_before = engine.effective_window_limits(target);
        assert_eq!(engine.rule_state[&target].resolved.min_width, Some(100));
        let mut rectangles = Vec::new();
        for id in 10..18 {
            let window = WindowId::new(id);
            assert!(!engine.floating_windows.contains(&window));
            assert_eq!(
                engine.rule_state[&window].resolved.min_width,
                (window == target).then_some(100)
            );
            assert_eq!(engine.effective_window_limits(window).min_width, 600);
            let rectangle = engine.applied_state[&window].rect;
            assert_eq!((rectangle.size.w, rectangle.size.h), (600, 1040));
            rectangles.push(rectangle);
        }
        assert_eq!(rectangles[7], Rect::new(1520, 0, 600, 1040));

        assert!(engine.finish_startup_rules(engine.startup_rules_deadline(), &backend));

        // The runtime rule must be stored even though the native minimum wins.
        assert_eq!(engine.rule_state[&target].resolved.min_width, Some(200));
        assert_eq!(
            engine.effective_window_limits(target),
            effective_before,
            "the inactive column's effective geometry has zero width delta"
        );
        for (id, rectangle) in (10..18).zip(rectangles) {
            let window = WindowId::new(id);
            assert_eq!(
                engine.rule_state[&window].resolved.min_width,
                (window == target).then_some(200)
            );
            assert_eq!(engine.effective_window_limits(window).min_width, 600);
            assert_eq!(
                engine.applied_state[&window].rect, rectangle,
                "a masked rule change must preserve submitted native geometry"
            );
        }
        assert_eq!(
            scroll_offset(&engine, output),
            before,
            "an intermediate raw-limit update must not undo user pan"
        );
        assert_eq!(engine.monitors.get(&output).unwrap().focus_window, focused);
    }

    #[test]
    fn startup_rule_phase_expiry_preserves_panned_view_on_unaffected_output() {
        let rules = [
            (Matcher::AtStartup, 360),
            (Matcher::Not(Box::new(Matcher::AtStartup)), 240),
        ]
        .into_iter()
        .map(|(phase, width)| WindowRule {
            match_groups: vec![MatchGroup {
                conds: vec![
                    phase,
                    Matcher::TitleSearch(RegexPat::new("^geometry").unwrap()),
                ],
            }],
            min_width: Some(width),
            max_width: Some(width),
            ..WindowRule::default()
        })
        .collect();
        let (mut engine, backend, panned) = panned_fixture(rules);
        let changed = OutputId::from_name("geometry-changing");
        engine.register_monitor(
            changed,
            Rect::new(1920, 0, 1920, 1080),
            Rect::new(1920, 0, 1920, 1040),
        );
        for id in [1, 2] {
            engine.add_window_with_target(
                fixture_window(id, "geometry changes at expiry"),
                AddWindowTarget::Output(changed),
                &backend,
                true,
            );
        }
        set_fixture_throttle(&mut engine, Duration::ZERO);
        engine.apply_all(&backend);
        for (id, x) in [(1, 1920), (2, 2280)] {
            assert!(!engine.floating_windows.contains(&WindowId::new(id)));
            assert_eq!(
                engine.applied_state[&WindowId::new(id)].rect,
                Rect::new(x, 0, 360, 1040)
            );
        }
        // Exercise the unaffected output as the secondary (unfocused) output.
        engine.set_focused_output(changed);
        let before = scroll_offset(&engine, panned);
        let focused = engine.monitors.get(&panned).unwrap().focus_window;
        set_fixture_throttle(&mut engine, Duration::MAX);

        assert!(engine.finish_startup_rules(engine.startup_rules_deadline(), &backend));

        for (id, x) in [(1, 1920), (2, 2160)] {
            assert_eq!(
                engine.applied_state[&WindowId::new(id)].rect,
                Rect::new(x, 0, 240, 1040),
                "the geometry-changing output still needs its final unthrottled layout"
            );
        }
        assert_eq!(
            scroll_offset(&engine, panned),
            before,
            "another output's rule change must preserve user pan"
        );
        assert_eq!(engine.monitors.get(&panned).unwrap().focus_window, focused);
    }
}
