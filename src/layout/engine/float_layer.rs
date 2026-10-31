//! The floating layer under a fullscreen window.
//!
//! niri does not draw the floating windows of a workspace while the workspace's active window
//! is a fullscreen scrolling window and the floating layer is not the active one, and it does
//! not hit-test them either. Oracle: niri e19df12cf81ffe805609fee497da1a097066ad2f,
//! - `layout/workspace.rs:1722-1728` `is_floating_visible`: the floats are visible when the
//!   floating space is active (`FloatingActive::Yes`, or `NoButRaised`) or
//!   `fullscreen_window_above_top_layer()` is `None`; its callers are `tiles_with_render_positions`
//!   (:1621), `render_floating` (:1685) and `window_under` (:1798).
//! - `layout/scrolling.rs:2946-2962` `fullscreen_window_above_top_layer`: `None` for an empty
//!   strip, while the view offset is animating, and unless the ACTIVE column is fullscreen
//!   (`sizing_mode` is fullscreen when any of its tiles is, :4378-4400).
//! - `layout/workspace.rs:1875-1895`: `activate_window` of a float makes the floating space
//!   active, of a tile the scrolling space; `activate_window_without_raising` keeps the floats
//!   raised (`NoButRaised`) only for focus-follows-mouse.
//! - `layout/workspace.rs:655-662`: a window that opens does not take the focus from a fullscreen
//!   window (`is_active_pending_fullscreen`), so a new float under one stays hidden.
//!
//! wiri hides the floats with the mechanism it already has for the floats of other workspaces
//! (`show_window(false)`, which `layout { hide-method }` turns into SW_HIDE or a cloak) and shows
//! them again, at the same place, as soon as niri would draw them. "The floating layer is
//! active" is `Workspace::active_float` (set by every focus of a float, cleared by every focus
//! of a tile, native foreground reports included).
//!
//! Not modelled: `NoButRaised`. wiri's focus-follows-mouse activation is a normal native
//! activation, so hovering the fullscreen tile hides the floats as an explicit focus does.

use super::*;

/// What `is_floating_visible` reads, as plain data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FloatLayerFacts {
    /// The floating layer holds the workspace's active window (`FloatingActive::Yes`).
    pub floating_active: bool,
    /// The strip has at least one column (scrolling.rs:2948).
    pub has_columns: bool,
    /// The view offset is not animating (scrolling.rs:2952).
    pub view_static: bool,
    /// The active column holds a fullscreen tile (scrolling.rs:2956-2959).
    pub active_column_fullscreen: bool,
}

impl FloatLayerFacts {
    /// niri `fullscreen_window_above_top_layer().is_some()`.
    pub(super) fn fullscreen_above_top_layer(self) -> bool {
        self.has_columns && self.view_static && self.active_column_fullscreen
    }

    /// niri `Workspace::is_floating_visible`.
    pub(super) fn floating_visible(self) -> bool {
        self.floating_active || !self.fullscreen_above_top_layer()
    }
}

impl TilingEngine {
    /// The facts for the active workspace of `output`.
    pub(super) fn float_layer_facts(&self, output: OutputId) -> Option<FloatLayerFacts> {
        let monitor = self.monitors.get(&output)?;
        let workspace = monitor.workspace()?;
        let active = monitor.active_workspace;
        let floating_active = workspace.active_float.is_some_and(|window| {
            self.float_location(window) == Some((output, active))
                && !self.minimized.contains_key(&window)
        });
        let active_column = monitor
            .focus_column
            .filter(|column| *column < workspace.columns.len())
            .or_else(|| {
                workspace
                    .active_window
                    .and_then(|window| workspace.find_window_column(window))
            });
        let active_column_fullscreen = active_column
            .and_then(|column| workspace.columns.get(column))
            .is_some_and(|column| {
                column
                    .tiles
                    .iter()
                    .any(|tile| self.fullscreen_windows.contains(&tile.window_id))
            });
        Some(FloatLayerFacts {
            floating_active,
            has_columns: !workspace.columns.is_empty(),
            view_static: self
                .animation
                .get_value(&crate::layout::AnimationTarget::ScrollX(output))
                .is_none(),
            active_column_fullscreen,
        })
    }

    /// True while niri would not draw (or hit-test) the floating windows of the active
    /// workspace of `output`: its active window is a fullscreen tile and the floating layer
    /// is not active.
    pub fn floats_hidden_by_fullscreen(&self, output: OutputId) -> bool {
        self.float_layer_facts(output)
            .is_some_and(|facts| !facts.floating_visible())
    }

    /// Bring the floats of `output` in line with `floats_hidden_by_fullscreen`: hide them
    /// when the layer went away, show them (and restore their stack) when it came back.
    /// Event-driven and idle when nothing is out of date.
    pub(super) fn refresh_float_visibility(&mut self, output: OutputId, backend: &BackendHandle) {
        if self.is_overview() {
            return;
        }
        let Some(active) = self.monitors.get(&output).map(|m| m.active_workspace) else {
            return;
        };
        let want_visible = !self.floats_hidden_by_fullscreen(output);
        let stale = self.float_homes.iter().any(|(window, home)| {
            home.output == output
                && home.workspace == active
                && !self.sticky_windows.contains(window)
                && !self.minimized.contains_key(window)
                && !self.transitions.retained(*window)
                && self
                    .applied_state
                    .get(window)
                    .map_or(false, |state| state.visible)
                    != want_visible
        });
        if stale {
            self.reconcile_floats(output, backend);
        }
    }

    /// [`Self::refresh_float_visibility`] for the output `window` (a tile or a float) is on.
    pub(super) fn refresh_float_visibility_of_window(
        &mut self,
        window: WindowId,
        backend: &BackendHandle,
    ) {
        let output = self
            .float_location(window)
            .map(|(output, _)| output)
            .or_else(|| self.locate_window(window).map(|(output, _, _)| output));
        if let Some(output) = output {
            self.refresh_float_visibility(output, backend);
        }
    }

    /// A native foreground report for a float that wiri hid under a fullscreen tile. With
    /// `hide-method "cloak"` the window is still in Alt+Tab, and a report that comes after
    /// the hide grace is the user asking for it, not Windows picking a replacement for a
    /// window hidden a moment ago: the floating layer comes back.
    pub(super) fn float_reveal_requested(&self, window: WindowId) -> bool {
        if self.hide_method != crate::backend::cloak::HideMethod::Cloak {
            return false;
        }
        let Some((output, workspace)) = self.float_location(window) else {
            return false;
        };
        self.monitors
            .get(&output)
            .is_some_and(|monitor| monitor.active_workspace == workspace)
            && self.floats_hidden_by_fullscreen(output)
            && self
                .hidden_since
                .get(&window)
                .is_some_and(|since| since.elapsed() >= Self::HIDE_GRACE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;
    use crate::backend::cloak::HideMethod;
    use crate::backend::WindowState;
    use crate::layout::AnimationTarget;
    use crate::overlay::decor::{DecorContent, RecordingSink};
    use std::time::{Duration, Instant};

    const T1: isize = -9601;
    const T2: isize = -9602;
    const F: isize = -9603;
    const G: isize = -9604;

    fn id(hwnd: isize) -> WindowId {
        WindowId::new(hwnd)
    }

    /// Where the float sits: not the centred default, not the size it opened with.
    fn geometry() -> Rect {
        Rect::new(240, 130, 640, 380)
    }

    fn guarded<T>(run: impl FnOnce() -> T) -> T {
        with_refused_activation_attempts(run).0
    }

    fn info(hwnd: isize) -> WindowInfo {
        WindowInfo {
            hwnd,
            title: "float layer model".into(),
            class_name: "FloatLayerModel".into(),
            process_id: 1,
            bounds: Rect::new(120, 90, 700, 500),
            state: WindowState::Normal,
            is_visible: true,
        }
    }

    /// Two tiles (`T1` active) and a float `F` over them. Model HWNDs are negative and
    /// invalid: no native window exists and every activation attempt is refused.
    fn fixture() -> (TilingEngine, BackendHandle, OutputId, Rect) {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let output = OutputId::from_name("float-layer-model");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        let backend = BackendHandle::default_for_test();
        for hwnd in [T1, T2, F] {
            engine.add_window(info(hwnd), &backend);
        }
        engine.toggle_floating_for(id(F), &backend);
        engine.place_float(id(F), geometry(), &backend);
        let rect = engine.float_homes[&id(F)].rect;
        assert_eq!(rect, geometry());
        engine.focus_action_window(id(T1), &backend);
        (engine, backend, output, rect)
    }

    fn float_visible(engine: &TilingEngine, hwnd: isize) -> bool {
        engine
            .applied_state
            .get(&id(hwnd))
            .is_some_and(|state| state.visible)
    }

    fn active_float(engine: &TilingEngine, output: OutputId) -> Option<WindowId> {
        engine
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace())
            .and_then(|workspace| workspace.active_float)
    }

    #[test]
    fn parity_floating_visible_follows_niri_for_every_combination_of_facts() {
        for bits in 0..16u8 {
            let facts = FloatLayerFacts {
                floating_active: bits & 1 != 0,
                has_columns: bits & 2 != 0,
                view_static: bits & 4 != 0,
                active_column_fullscreen: bits & 8 != 0,
            };
            // niri workspace.rs:1722-1728 and scrolling.rs:2946-2962, written out.
            let fullscreen_above =
                facts.has_columns && facts.view_static && facts.active_column_fullscreen;
            let niri = facts.floating_active || !fullscreen_above;
            assert_eq!(facts.floating_visible(), niri, "{facts:?}");
        }
        // Only the one combination hides: strip not empty, view still, fullscreen active,
        // floats not active.
        let hidden = FloatLayerFacts {
            floating_active: false,
            has_columns: true,
            view_static: true,
            active_column_fullscreen: true,
        };
        assert!(!hidden.floating_visible());
        assert!(FloatLayerFacts {
            floating_active: true,
            ..hidden
        }
        .floating_visible());
        assert!(FloatLayerFacts {
            view_static: false,
            ..hidden
        }
        .floating_visible());
        assert!(FloatLayerFacts {
            has_columns: false,
            ..hidden
        }
        .floating_visible());
    }

    #[test]
    fn parity_a_fullscreen_tile_hides_the_floats_and_focus_or_unfullscreen_restores_them() {
        guarded(|| {
            let (mut engine, backend, output, rect) = fixture();
            assert_eq!(active_float(&engine, output), None);
            assert!(!engine.floats_hidden_by_fullscreen(output));
            assert!(float_visible(&engine, F), "a float over a tile is shown");

            engine.toggle_fullscreen_for(id(T1), &backend);
            assert!(engine.is_fullscreen(id(T1)));
            assert!(engine.floats_hidden_by_fullscreen(output));
            assert!(!float_visible(&engine, F), "the floating layer is hidden");
            assert!(
                engine.hidden_since.contains_key(&id(F)),
                "hidden by the ordinary hide path"
            );

            // The float takes the focus: the floating layer is active, so it comes back.
            assert!(engine.focus_float(id(F), &backend));
            assert_eq!(active_float(&engine, output), Some(id(F)));
            assert!(!engine.floats_hidden_by_fullscreen(output));
            assert!(float_visible(&engine, F));
            assert_eq!(engine.float_homes[&id(F)].rect, rect, "at its place");
            assert!(engine.is_fullscreen(id(T1)), "the tile is still fullscreen");

            // Focus back on the fullscreen tile: the floats go away again.
            engine.focus_action_window(id(T1), &backend);
            assert_eq!(active_float(&engine, output), None);
            assert!(engine.floats_hidden_by_fullscreen(output));
            assert!(!float_visible(&engine, F));

            // Leaving fullscreen shows them, at the same place, with the focus on the tile.
            engine.toggle_fullscreen_for(id(T1), &backend);
            assert!(!engine.is_fullscreen(id(T1)));
            assert!(!engine.floats_hidden_by_fullscreen(output));
            assert!(float_visible(&engine, F));
            assert_eq!(engine.float_homes[&id(F)].rect, rect, "restored exactly");
        });
    }

    #[test]
    fn parity_the_view_must_be_still_and_the_strip_not_empty() {
        guarded(|| {
            let (mut engine, backend, output, _) = fixture();
            engine.toggle_fullscreen_for(id(T1), &backend);
            assert!(engine.floats_hidden_by_fullscreen(output));

            // scrolling.rs:2952: while the view offset animates, the floats are drawn.
            engine.animation =
                crate::layout::AnimationManager::new(true, 200, crate::layout::Easing::Linear);
            engine
                .animation
                .animate(AnimationTarget::ScrollX(output), 0.0, 100.0);
            assert!(!engine.floats_hidden_by_fullscreen(output));
            engine.refresh_float_visibility(output, &backend);
            assert!(
                float_visible(&engine, F),
                "shown for the length of the scroll"
            );
            engine.animation.cancel_all();
            assert!(engine.floats_hidden_by_fullscreen(output));
            engine.refresh_float_visibility(output, &backend);
            assert!(!float_visible(&engine, F), "hidden once the view is still");
        });
    }

    #[test]
    fn parity_only_the_active_column_counts() {
        guarded(|| {
            let (mut engine, backend, output, _) = fixture();
            engine.toggle_fullscreen_for(id(T1), &backend);
            assert!(engine.floats_hidden_by_fullscreen(output));
            // Focusing the other (not fullscreen) column makes it the active one.
            engine.focus_action_window(id(T2), &backend);
            assert!(!engine.is_fullscreen(id(T2)));
            assert!(!engine.floats_hidden_by_fullscreen(output));
            assert!(float_visible(&engine, F));
            engine.focus_action_window(id(T1), &backend);
            assert!(engine.floats_hidden_by_fullscreen(output));
            assert!(!float_visible(&engine, F));
        });
    }

    #[test]
    fn parity_a_float_that_is_active_stays_through_a_workspace_round_trip() {
        guarded(|| {
            let (mut engine, backend, output, rect) = fixture();
            engine.toggle_fullscreen_for(id(T1), &backend);
            engine.focus_float(id(F), &backend);
            engine.switch_workspace(1, &backend);
            assert!(!float_visible(&engine, F), "another workspace hides it");
            engine.switch_workspace(0, &backend);
            assert_eq!(active_float(&engine, output), Some(id(F)));
            assert!(!engine.floats_hidden_by_fullscreen(output));
            assert!(float_visible(&engine, F));
            assert_eq!(engine.float_homes[&id(F)].rect, rect);
        });
    }

    #[test]
    fn parity_floats_without_a_fullscreen_tile_are_untouched() {
        guarded(|| {
            let (mut engine, backend, output, _) = fixture();
            // Maximized to the edges is not fullscreen (niri hides only for fullscreen).
            engine.toggle_maximize_window_to_edges_for(id(T1), &backend);
            assert!(engine.is_maximized_to_edges(id(T1)));
            assert!(!engine.floats_hidden_by_fullscreen(output));
            assert!(float_visible(&engine, F));
        });
    }

    #[test]
    fn parity_hidden_floats_are_not_decorated() {
        guarded(|| {
            let (mut engine, backend, output, _) = fixture();
            let sink = RecordingSink::new();
            engine.install_decor_sink(sink.clone());
            engine.apply_layout_for_monitor(output, &backend);
            let frames = || {
                sink.of_output(output.as_u64())
                    .iter()
                    .filter(|spec| matches!(spec.content, DecorContent::Frame(_)))
                    .map(|spec| spec.id.key)
                    .collect::<Vec<_>>()
            };
            assert!(frames().contains(&F), "the visible float has a frame");
            engine.toggle_fullscreen_for(id(T1), &backend);
            engine.apply_layout_for_monitor(output, &backend);
            assert!(!frames().contains(&F), "the hidden float has none");
            engine.focus_float(id(F), &backend);
            engine.apply_layout_for_monitor(output, &backend);
            assert!(frames().contains(&F), "it is decorated again");
        });
    }

    #[test]
    fn parity_a_second_float_hides_and_returns_with_the_first() {
        guarded(|| {
            let (mut engine, backend, output, _) = fixture();
            engine.add_window(info(G), &backend);
            engine.toggle_floating_for(id(G), &backend);
            engine.focus_action_window(id(T1), &backend);
            engine.toggle_fullscreen_for(id(T1), &backend);
            assert!(!float_visible(&engine, F) && !float_visible(&engine, G));
            // Either float brings the whole layer back, like niri's single floating space.
            engine.focus_float(id(G), &backend);
            assert!(float_visible(&engine, F) && float_visible(&engine, G));
            assert!(!engine.floats_hidden_by_fullscreen(output));
        });
    }

    #[test]
    fn parity_asking_for_a_cloaked_hidden_float_brings_the_layer_back() {
        guarded(|| {
            for (method, comes_back) in [(HideMethod::Cloak, true), (HideMethod::Hide, false)] {
                let (mut engine, backend, output, _) = fixture();
                engine.set_hide_method(method);
                engine.toggle_fullscreen_for(id(T1), &backend);
                assert!(!float_visible(&engine, F));
                // Right after the hide the report is Windows picking a replacement: ignored.
                engine.sync_os_foreground(id(F), &backend);
                assert!(!float_visible(&engine, F), "{method:?}: inside the grace");
                // Later it is the user (Alt+Tab to a cloaked window).
                engine.hidden_since.insert(
                    id(F),
                    Instant::now()
                        .checked_sub(Duration::from_secs(5))
                        .expect("monotonic clock has 5 s of history"),
                );
                engine.sync_os_foreground(id(F), &backend);
                assert_eq!(float_visible(&engine, F), comes_back, "{method:?}");
                assert_eq!(
                    active_float(&engine, output) == Some(id(F)),
                    comes_back,
                    "{method:?}"
                );
            }
        });
    }

    #[test]
    fn parity_a_native_foreground_on_the_tile_hides_and_on_the_float_shows() {
        guarded(|| {
            let (mut engine, backend, output, _) = fixture();
            engine.toggle_fullscreen_for(id(T1), &backend);
            engine.focus_float(id(F), &backend);
            assert!(float_visible(&engine, F));
            // The user clicks the fullscreen tile: the report moves the focus layer.
            engine.sync_os_foreground(id(T1), &backend);
            assert_eq!(active_float(&engine, output), None);
            assert!(!float_visible(&engine, F));
        });
    }
}
