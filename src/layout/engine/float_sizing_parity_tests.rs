//! Floating windows versus fullscreen and maximized-to-edges.
//!
//! Oracle: niri e19df12cf81ffe805609fee497da1a097066ad2f, `src/layout/`:
//! workspace.rs:1288-1342 (`set_fullscreen`), 1353-1401 (`set_maximized`), 1403-1416
//! (`toggle_maximized`), 1418-1476 (`toggle_window_floating`), 636-657 (`add_tile`),
//! floating.rs:425-449 (`add_tile_at`), 515-552 (`remove_tile_by_idx`), scrolling.rs:999-1014
//! (`add_column`), 2884-2944 (`set_fullscreen` / `set_maximized`), tile.rs:62-76 and mod.rs:3542-3634.
//!
//! A floating window is never fullscreen or maximized: asking for either moves it into the
//! strip (a column of its own right of the active column, active only when the target was the
//! active window), remembers that it came from the floating layer, and leaving the state
//! floats it again at the geometry it floated at. `toggle-window-floating` on such a tile
//! floats it with both states gone. Windowed fullscreen (mod.rs:3586-3604, window/mapped.rs:1321-1351)
//! has no Windows counterpart (matrix row 120). Not covered here: niri hides the floating layer
//! under a fullscreen scrolling window (workspace.rs:1722-1728), which wiri does not do.
//!
//! Engine-level only. Model HWNDs are negative and invalid: no native window, pump, hook or
//! input fixture is created, and every activation attempt is refused.

use super::*;
use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;
use crate::backend::message_loop::perform_action_on;
use crate::backend::WindowState;
use crate::config::types::{MatchGroup, Matcher, RegexPat};
use crate::config::WindowRule;
use crate::input::{parse_action_name, MonitorTarget};
use std::time::{Duration, Instant};

const T1: isize = -9501;
const T2: isize = -9502;
const F: isize = -9503;
/// Where the float sits before it is made fullscreen or maximized: not the centred default,
/// and not the size the window opened with.
fn geometry() -> Rect {
    Rect::new(240, 130, 640, 380)
}

fn id(hwnd: isize) -> WindowId {
    WindowId::new(hwnd)
}

fn guarded<T>(run: impl FnOnce() -> T) -> T {
    with_refused_activation_attempts(run).0
}

fn info(hwnd: isize, title: &str) -> WindowInfo {
    WindowInfo {
        hwnd,
        title: title.into(),
        class_name: "FloatSizingModel".into(),
        process_id: 1,
        bounds: Rect::new(120, 90, 700, 500),
        state: WindowState::Normal,
        is_visible: true,
    }
}

/// Two tiles (`T1` active column 0, `T2` column 1) and the window `F`, still a tile.
fn fixture() -> (TilingEngine, BackendHandle, OutputId) {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let output = OutputId::from_name("float-sizing-model");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1920, 1080),
        Rect::new(0, 0, 1920, 1040),
    );
    let backend = BackendHandle::default_for_test();
    for hwnd in [T1, T2, F] {
        engine.add_window(info(hwnd, "float sizing model"), &backend);
    }
    (engine, backend, output)
}

/// `F` floats at `geometry()` and is the foreground window; `T1` is the active tile.
fn with_float(engine: &mut TilingEngine, backend: &BackendHandle, output: OutputId) -> Rect {
    engine.toggle_floating_for(id(F), backend);
    engine.place_float(id(F), geometry(), backend);
    engine.monitors.get_mut(&output).unwrap().set_focus(id(T1));
    engine.record_ipc_foreground(id(F));
    let rect = engine.float_homes[&id(F)].rect;
    assert_eq!(
        rect,
        geometry(),
        "the model monitor holds the fixture float"
    );
    rect
}

fn strip(engine: &TilingEngine, output: OutputId, workspace: i32) -> Vec<Vec<isize>> {
    engine.monitors.get(&output).unwrap().workspaces[&workspace]
        .columns
        .iter()
        .map(|column| {
            column
                .tiles
                .iter()
                .map(|t| t.window_id.as_isize())
                .collect()
        })
        .collect()
}

fn focus_of(engine: &TilingEngine, output: OutputId) -> Option<WindowId> {
    engine.monitors.get(&output).unwrap().focus_window
}

fn assert_plain_tile(engine: &TilingEngine, hwnd: isize) {
    assert!(!engine.is_floating(id(hwnd)));
    assert!(!engine.is_fullscreen(id(hwnd)));
    assert!(!engine.is_maximized_to_edges(id(hwnd)));
    assert!(!engine.restore_to_floating.contains_key(&id(hwnd)));
}

#[test]
fn parity_float_fullscreen_enters_the_strip_right_of_the_active_column_and_takes_the_focus() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        assert_eq!(strip(&engine, output, 0), [[T1], [T2]]);

        // workspace.rs:1290-1293: a floating window that goes fullscreen is toggled into the
        // scrolling layout; scrolling.rs:1008-1014 inserts the column right of the active one;
        // workspace.rs:1420,1433-1443: the target was the active window, so it is activated.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert_eq!(strip(&engine, output, 0), [vec![T1], vec![F], vec![T2]]);
        assert!(!engine.is_floating(id(F)));
        assert!(engine.is_fullscreen(id(F)));
        assert_eq!(focus_of(&engine, output), Some(id(F)));
        assert_eq!(engine.focus_layer, Some(FocusLayer::Tiling));
        // workspace.rs:1339-1341 and floating.rs:538-543: it remembers the float to return to.
        assert!(engine.restore_to_floating.contains_key(&id(F)));
        assert_eq!(engine.float_memory[&id(F)], rect);
        assert_plain_tile(&engine, T1);
        assert_plain_tile(&engine, T2);
    });
}

#[test]
fn parity_leaving_fullscreen_floats_the_window_again_at_its_remembered_geometry() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.toggle_fullscreen_for(id(F), &backend);

        // workspace.rs:1309-1320: unfullscreening a tile with `restore_to_floating` floats it in
        // the same call; floating.rs:425-449 restores its size, and its stored position holds.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert!(!engine.is_fullscreen(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect, "position and size");
        assert!(engine.restore_to_floating.is_empty());
        assert_eq!(strip(&engine, output, 0), [[T1], [T2]], "no stray column");
        // The float has the focus again (workspace.rs:1465-1467), a tile has it no longer.
        assert_eq!(engine.focus_layer, Some(FocusLayer::Floating));
        assert_eq!(engine.os_foreground, Some(id(F)));
        assert_ne!(focus_of(&engine, output), Some(id(F)));
        assert!(engine.state_target().is_some_and(|t| t.floating));
    });
}

#[test]
fn parity_float_maximize_enters_the_strip_and_unmaximize_floats_it_again() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);

        // workspace.rs:1355-1358 and 1364-1379, toggle_maximized at 1403-1416.
        engine.toggle_maximize_window_to_edges_for(id(F), &backend);
        assert_eq!(strip(&engine, output, 0), [vec![T1], vec![F], vec![T2]]);
        assert!(engine.is_maximized_to_edges(id(F)));
        assert!(!engine.is_floating(id(F)));
        assert_eq!(focus_of(&engine, output), Some(id(F)));
        assert!(engine.restore_to_floating.contains_key(&id(F)));

        engine.toggle_maximize_window_to_edges_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert!(!engine.is_maximized_to_edges(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert!(engine.restore_to_floating.is_empty());
        assert_eq!(strip(&engine, output, 0), [[T1], [T2]]);
    });
}

#[test]
fn parity_maximize_then_fullscreen_floats_only_when_both_states_are_gone() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.toggle_maximize_window_to_edges_for(id(F), &backend);
        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_fullscreen(id(F)) && engine.is_maximized_to_edges(id(F)));

        // workspace.rs:1373-1375: with both states the pending sizing mode is fullscreen, so
        // unmaximizing does not float; the flag set at the first change (1398-1400) survives
        // the second one, whose `was_normal` is false.
        engine.toggle_maximize_window_to_edges_for(id(F), &backend);
        assert!(!engine.is_floating(id(F)));
        assert!(engine.is_fullscreen(id(F)));
        assert!(engine.restore_to_floating.contains_key(&id(F)));

        // Unfullscreen with nothing else left: floats.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert!(engine.restore_to_floating.is_empty());
    });
}

#[test]
fn parity_fullscreen_then_maximize_leaves_fullscreen_into_maximized_not_into_floating() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.toggle_fullscreen_for(id(F), &backend);
        engine.toggle_maximize_window_to_edges_for(id(F), &backend);

        // workspace.rs:1309-1311: "when going from fullscreen to maximized, don't consider
        // restore_to_floating yet": leaving fullscreen of a maximized column stays a tile.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(!engine.is_floating(id(F)));
        assert!(!engine.is_fullscreen(id(F)));
        assert!(engine.is_maximized_to_edges(id(F)));
        assert!(engine.restore_to_floating.contains_key(&id(F)));

        // workspace.rs:1375-1379: now the pending mode is maximized: unmaximize floats.
        engine.toggle_maximize_window_to_edges_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert!(!engine.is_maximized_to_edges(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert!(engine.restore_to_floating.is_empty());
    });
}

#[test]
fn parity_toggle_window_floating_on_a_sizing_tile_floats_it_with_both_states_gone() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.toggle_fullscreen_for(id(F), &backend);
        engine.toggle_maximize_window_to_edges_for(id(F), &backend);

        // workspace.rs:1444-1467: a tile in the scrolling layout (any sizing mode) is removed and
        // added to the floating layout, whose `add_tile_at` (floating.rs:428-449) asks for the
        // floating size with no fullscreen or maximized state.
        engine.toggle_floating_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert!(!engine.is_fullscreen(id(F)));
        assert!(!engine.is_maximized_to_edges(id(F)));
        assert!(engine.restore_to_floating.is_empty());
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert_eq!(strip(&engine, output, 0), [[T1], [T2]]);
    });
}

#[test]
fn parity_a_tile_that_floated_before_does_not_return_to_floating() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        with_float(&mut engine, &backend, output);
        engine.toggle_fullscreen_for(id(F), &backend);
        engine.toggle_floating_for(id(F), &backend);
        // Tiled by the user, then fullscreen and out of it: workspace.rs:1339-1341 re-assigns
        // the flag from "was it floating" at every change from normal, so this one is false.
        engine.toggle_floating_for(id(F), &backend);
        assert!(!engine.is_floating(id(F)));
        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(!engine.restore_to_floating.contains_key(&id(F)));
        engine.toggle_fullscreen_for(id(F), &backend);
        assert_plain_tile(&engine, F);
        engine.toggle_maximize_window_to_edges_for(id(F), &backend);
        engine.toggle_maximize_window_to_edges_for(id(F), &backend);
        assert_plain_tile(&engine, F);
        assert_eq!(strip(&engine, output, 0).len(), 3);
    });
}

#[test]
fn parity_leaving_the_state_of_a_floating_window_does_nothing() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        // workspace.rs:1294-1297 and 1360-1363: "floating windows are never fullscreen, so this
        // is an unfullscreen request for an already unfullscreen window": return.
        engine.set_fullscreen(id(F), false, false, &backend);
        engine.set_maximized_to_edges(id(F), false, &backend);
        assert!(engine.is_floating(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert!(engine.restore_to_floating.is_empty());
        assert_eq!(strip(&engine, output, 0), [[T1], [T2]]);
    });
}

#[test]
fn parity_a_tile_that_never_floated_stays_a_tile_when_it_leaves_fullscreen_or_maximized() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        engine.toggle_fullscreen_for(id(T2), &backend);
        assert!(!engine.restore_to_floating.contains_key(&id(T2)));
        engine.toggle_fullscreen_for(id(T2), &backend);
        engine.toggle_maximize_window_to_edges_for(id(T2), &backend);
        engine.toggle_maximize_window_to_edges_for(id(T2), &backend);
        assert_plain_tile(&engine, T2);
        assert_eq!(strip(&engine, output, 0).len(), 3);
    });
}

#[test]
fn parity_an_explicit_inactive_float_target_does_not_take_the_focus() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.monitors.get_mut(&output).unwrap().set_focus(id(T2));
        engine.record_ipc_foreground(id(T2));

        // workspace.rs:1420,1433-1440: `target_is_active` is false for a window that is not the
        // active one, so the new column sits right of the active column but is not activated.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert_eq!(strip(&engine, output, 0), [vec![T1], vec![T2], vec![F]]);
        assert!(engine.is_fullscreen(id(F)));
        assert_eq!(focus_of(&engine, output), Some(id(T2)));
        let workspace = &engine.monitors.get(&output).unwrap().workspaces[&0];
        assert_eq!(workspace.active_window, Some(id(T2)));
        assert_eq!(engine.os_foreground, Some(id(T2)));

        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert_eq!(focus_of(&engine, output), Some(id(T2)));
        assert_eq!(engine.os_foreground, Some(id(T2)));
    });
}

#[test]
fn parity_a_float_on_an_inactive_workspace_enters_that_workspaces_strip() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        let other = crate::layout::workspace_id_from_number(2);
        engine.float_move_to_workspace(id(F), other, &backend);
        engine.os_foreground = None;
        let active_before = engine.monitors.get(&output).unwrap().active_workspace;

        // mod.rs:3563-3567: the request reaches the workspace that has the window, active or
        // not; workspace.rs:1290-1293 moves it into that workspace's strip.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert_eq!(engine.locate_window(id(F)), Some((output, other, 0)));
        assert!(engine.is_fullscreen(id(F)));
        assert_eq!(
            strip(&engine, output, 0),
            [[T1], [T2]],
            "the active strip is left alone"
        );
        assert_eq!(
            engine.monitors.get(&output).unwrap().active_workspace,
            active_before
        );
        assert_ne!(focus_of(&engine, output), Some(id(F)));

        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert_eq!(engine.float_homes[&id(F)].workspace, other);
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert_eq!(strip(&engine, output, 0), [[T1], [T2]]);
    });
}

#[test]
fn parity_float_fullscreen_while_another_tile_is_fullscreen() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.toggle_fullscreen_for(id(T1), &backend);
        assert!(engine.is_fullscreen(id(T1)));
        engine.record_ipc_foreground(id(F));

        // Each column has its own state (scrolling.rs:2891-2907): the float gets a column of its
        // own next to the fullscreen tile's and the tile stays fullscreen.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert_eq!(strip(&engine, output, 0), [vec![T1], vec![F], vec![T2]]);
        assert!(engine.is_fullscreen(id(T1)));
        assert!(engine.is_fullscreen(id(F)));
        assert_eq!(focus_of(&engine, output), Some(id(F)));
        assert!(!engine.restore_to_floating.contains_key(&id(T1)));

        // Only the window that came from the floating layer returns to it.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert!(engine.is_fullscreen(id(T1)));
        engine.toggle_fullscreen_for(id(T1), &backend);
        assert_plain_tile(&engine, T1);
    });
}

fn two_monitor_fixture() -> (TilingEngine, BackendHandle, OutputId, OutputId) {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let a = OutputId::from_name("float-sizing-a");
    let b = OutputId::from_name("float-sizing-b");
    engine.register_monitor(a, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));
    engine.register_monitor(
        b,
        Rect::new(1920, 0, 1280, 720),
        Rect::new(1920, 0, 1280, 680),
    );
    let backend = BackendHandle::default_for_test();
    engine.add_window_with_target(info(T1, "a"), AddWindowTarget::Output(a), &backend, false);
    engine.add_window_with_target(info(T2, "b"), AddWindowTarget::Output(b), &backend, false);
    engine.add_window_with_target(info(F, "f"), AddWindowTarget::Output(b), &backend, false);
    (engine, backend, a, b)
}

#[test]
fn parity_float_fullscreen_on_the_second_monitor_stays_on_that_monitor() {
    guarded(|| {
        let (mut engine, backend, a, b) = two_monitor_fixture();
        engine.toggle_floating_for(id(F), &backend);
        engine.place_float(id(F), Rect::new(2200, 100, 500, 300), &backend);
        engine.record_ipc_foreground(id(F));
        let rect = engine.float_homes[&id(F)].rect;
        let a_before = strip(&engine, a, 0);

        // The window's own workspace receives it (mod.rs:3563-3567): its monitor's strip, and
        // the fullscreen rectangle is that monitor's, not the focused one's.
        engine.toggle_fullscreen_for(id(F), &backend);
        assert_eq!(engine.locate_window(id(F)).map(|place| place.0), Some(b));
        assert_eq!(
            strip(&engine, a, 0),
            a_before,
            "the other monitor is untouched"
        );
        assert_eq!(strip(&engine, b, 0), [vec![T2], vec![F]]);
        let ids: HashSet<WindowId> = [id(F)].into_iter().collect();
        assert_eq!(engine.sizing_draw_modes(&ids)[&id(F)], DrawMode::Exact);
        assert_eq!(
            engine.focus_layer,
            Some(FocusLayer::Tiling),
            "the tile of monitor B has the focus"
        );
        assert_eq!(
            engine.monitors.get(&b).unwrap().focus_window,
            Some(id(F)),
            "monitor B's own focus, not the first monitor's"
        );
        assert_ne!(engine.monitors.get(&a).unwrap().focus_window, Some(id(F)));

        engine.toggle_fullscreen_for(id(F), &backend);
        assert_eq!(engine.float_location(id(F)).map(|home| home.0), Some(b));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
    });
}

#[test]
fn parity_a_float_that_goes_fullscreen_returns_at_the_same_fraction_of_another_work_area() {
    guarded(|| {
        let (mut engine, backend, a, b) = two_monitor_fixture();
        // A float on monitor A at a quarter of its work area in both axes.
        engine.add_window_with_target(
            info(-9504, "g"),
            AddWindowTarget::Output(a),
            &backend,
            false,
        );
        let moved = id(-9504);
        engine.toggle_floating_for(moved, &backend);
        engine.place_float(moved, Rect::new(480, 260, 400, 300), &backend);
        engine.record_ipc_foreground(moved);
        engine.toggle_fullscreen_for(moved, &backend);
        assert!(engine.is_fullscreen(moved));

        // The fullscreen tile is sent to monitor B and leaves fullscreen there.
        engine.move_window_to_monitor_action(Some(moved), &MonitorTarget::Right, &backend);
        assert_eq!(engine.locate_window(moved).map(|place| place.0), Some(b));
        assert!(engine.is_fullscreen(moved), "the column keeps its state");
        engine.toggle_fullscreen_for(moved, &backend);

        // floating.rs:115-135 keeps the position as a fraction of the working area: 480/1920
        // and 260/1040 of B's 1280 x 680 area at (1920, 0); the size is kept as it was.
        assert!(engine.is_floating(moved));
        assert_eq!(engine.float_location(moved).map(|home| home.0), Some(b));
        assert_eq!(
            engine.float_homes[&moved].rect,
            Rect::new(1920 + 320, 170, 400, 300)
        );
    });
}

#[test]
fn parity_a_minimized_sizing_tile_still_returns_to_floating() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.toggle_fullscreen_for(id(F), &backend);

        engine.minimize_window(id(F), &backend);
        assert!(!engine.is_fullscreen(id(F)));
        assert!(
            engine.restore_to_floating.contains_key(&id(F)),
            "what the tile came from is not forgotten by a minimize"
        );
        engine.restore_window(id(F), &backend);
        assert!(engine.is_fullscreen(id(F)));
        assert!(!engine.is_floating(id(F)));

        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert!(engine.restore_to_floating.is_empty());
    });
}

#[test]
fn parity_a_closed_sizing_tile_leaves_no_floating_memory_behind() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        with_float(&mut engine, &backend, output);
        engine.toggle_fullscreen_for(id(F), &backend);
        engine.remove_window(id(F), &backend);
        assert!(engine.restore_to_floating.is_empty());
        assert!(!engine.float_memory.contains_key(&id(F)));
    });
}

#[test]
fn parity_a_default_tabbed_column_does_not_change_the_float_round_trip() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.config.default_tabbed = true;
        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_fullscreen(id(F)));
        assert!(!engine.is_floating(id(F)));
        engine.toggle_fullscreen_for(id(F), &backend);
        assert!(engine.is_floating(id(F)));
        assert!(!engine.is_fullscreen(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
        assert!(engine.restore_to_floating.is_empty());
    });
}

#[test]
fn parity_the_application_leaving_fullscreen_floats_the_window_again() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        engine.toggle_fullscreen_for(id(F), &backend);
        // The application unfullscreens itself some time after (a Wayland client's unfullscreen
        // request reaches the same `set_fullscreen(false)`, workspace.rs:1299-1320).
        engine
            .fullscreen_since
            .insert(id(F), Instant::now() - Duration::from_secs(5));
        engine.window_size_class_changed(id(F), false, false, &backend);
        assert!(engine.is_floating(id(F)));
        assert!(!engine.is_fullscreen(id(F)));
        assert_eq!(engine.float_homes[&id(F)].rect, rect);
    });
}

fn rule_for(title: &str) -> WindowRule {
    WindowRule {
        match_groups: vec![MatchGroup {
            conds: vec![Matcher::TitleSearch(RegexPat::new(title).unwrap())],
        }],
        ..WindowRule::default()
    }
}

#[test]
fn parity_a_window_that_opens_floating_and_fullscreen_opens_in_the_strip() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        engine.set_window_rules(vec![WindowRule {
            open_floating: Some(true),
            open_fullscreen: Some(true),
            ..rule_for("^opens floating and fullscreen$")
        }]);
        let opened = id(-9505);
        engine.add_window(info(-9505, "opens floating and fullscreen"), &backend);

        // workspace.rs:647 and 654-657: opened floating and pending fullscreen, the tile goes
        // to the scrolling layout and remembers to float when it leaves fullscreen.
        assert!(!engine.is_floating(opened));
        assert!(engine.is_fullscreen(opened));
        assert!(engine.restore_to_floating.contains_key(&opened));
        let opened_rect = engine.float_memory[&opened];
        assert_eq!(strip(&engine, output, 0).len(), 4);

        engine.toggle_fullscreen_for(opened, &backend);
        assert!(engine.is_floating(opened));
        assert!(!engine.is_fullscreen(opened));
        assert_eq!(engine.float_homes[&opened].rect, opened_rect);
        assert_eq!(strip(&engine, output, 0).len(), 3);
    });
}

#[test]
fn parity_a_window_that_opens_floating_and_maximized_opens_in_the_strip() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        engine.set_window_rules(vec![WindowRule {
            open_floating: Some(true),
            open_maximized_to_edges: Some(true),
            ..rule_for("^opens floating and maximized$")
        }]);
        let opened = id(-9506);
        engine.add_window(info(-9506, "opens floating and maximized"), &backend);
        assert!(!engine.is_floating(opened));
        assert!(engine.is_maximized_to_edges(opened));
        assert!(engine.restore_to_floating.contains_key(&opened));

        engine.toggle_maximize_window_to_edges_for(opened, &backend);
        assert!(engine.is_floating(opened));
        assert_eq!(strip(&engine, output, 0).len(), 3);
    });
}

#[test]
fn parity_the_actions_move_a_float_into_the_strip_through_the_shared_dispatcher() {
    guarded(|| {
        let (mut engine, backend, output) = fixture();
        let rect = with_float(&mut engine, &backend, output);
        let engine = Arc::new(parking_lot::RwLock::new(engine));
        let quit = Arc::new(parking_lot::Mutex::new(None));
        let run = |name: &str| {
            let action = parse_action_name(name, &[]).expect("niri action name");
            perform_action_on(&action, &engine, &backend, &quit, Some(F));
        };

        run("maximize-window-to-edges");
        assert!(engine.read().is_maximized_to_edges(id(F)));
        assert!(!engine.read().is_floating(id(F)));
        run("maximize-window-to-edges");
        assert!(engine.read().is_floating(id(F)));
        run("fullscreen-window");
        assert!(engine.read().is_fullscreen(id(F)));
        assert!(!engine.read().is_floating(id(F)));
        run("fullscreen-window");
        assert!(engine.read().is_floating(id(F)));
        assert_eq!(engine.read().float_homes[&id(F)].rect, rect);
    });
}

#[test]
fn parity_move_floating_window_on_a_sizing_tile_adjusts_from_the_fraction_in_its_work_area() {
    guarded(|| {
        let (mut engine, backend, a, b) = two_monitor_fixture();
        engine.add_window_with_target(
            info(-9504, "g"),
            AddWindowTarget::Output(a),
            &backend,
            false,
        );
        let moved = id(-9504);
        engine.toggle_floating_for(moved, &backend);
        engine.place_float(moved, Rect::new(480, 260, 400, 300), &backend);
        engine.record_ipc_foreground(moved);
        engine.toggle_fullscreen_for(moved, &backend);
        engine.move_window_to_monitor_action(Some(moved), &MonitorTarget::Right, &backend);
        assert_eq!(engine.locate_window(moved).map(|place| place.0), Some(b));

        // workspace.rs:1541-1595: a tile's stored floating position is scaled by the working
        // area it is used in, then adjusted and stored again as a fraction of that area.
        use crate::ipc::PositionChange::AdjustFixed;
        engine
            .move_floating_window(Some(moved), AdjustFixed(10.0), AdjustFixed(0.0), None, &backend)
            .unwrap();
        assert_eq!(
            engine.restore_to_floating.get(&moved),
            Some(&engine.monitors.get(&b).unwrap().work_area)
        );
        engine.toggle_fullscreen_for(moved, &backend);
        assert_eq!(
            engine.float_homes[&moved].rect,
            Rect::new(1920 + 320 + 10, 170, 400, 300)
        );
    });
}
