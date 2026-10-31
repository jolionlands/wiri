//! niri's `*ById` actions change the named window and never focus (niri:src/input/mod.rs
//! 1110-1136, 1684-1736, 2111-2117, 2213-2250). Each test drives the real `RunAction`
//! handler with `--id` against a two-monitor engine and proves, besides the effect, that
//! native focus (no activation request; a wrongly activated window would silently take
//! the simulated foreground) and managed focus (the IPC focus, the focused output, every
//! monitor's workspace and focus selection) are exactly what they were.
//!
//! Model HWNDs are negative, so no real window is touched.

use super::{IpcMessage, IpcServer};
use crate::backend::activation_attempt_test_support::{with_native_foreground, Activation};
use crate::backend::{BackendHandle, WindowInfo, WindowState};
use crate::layout::{AddWindowTarget, LayoutConfig, TilingEngine};
use crate::utils::{OutputId, Rect, WindowId};
use std::sync::Arc;

const P: isize = -9501;
const Q: isize = -9502; // the focused tile
const R: isize = -9503;
const S: isize = -9504;
const U: isize = -9511; // monitor B, active workspace
const V: isize = -9512;
const W: isize = -9513; // monitor B, inactive workspace 5
const W2: isize = -9514;

type SharedEngine = Arc<parking_lot::RwLock<TilingEngine>>;
/// Per monitor: the active workspace and the focused window (the focus column is an index
/// that legitimately shifts when a column ahead of it appears or vanishes).
type Selection = Vec<(i32, Option<WindowId>)>;

struct Fx {
    server: IpcServer,
    engine: SharedEngine,
    backend: BackendHandle,
    a: OutputId,
    b: OutputId,
}

fn info(hwnd: isize) -> WindowInfo {
    WindowInfo {
        hwnd,
        title: format!("by-id model {hwnd}"),
        class_name: "ByIdFocusModel".into(),
        process_id: 1,
        bounds: Rect::new(0, 0, 600, 500),
        state: WindowState::Normal,
        is_visible: true,
    }
}

/// Monitor A (focused): columns P Q R S with Q focused. Monitor B: U V on its active
/// workspace, W W2 on its inactive workspace 5. The live focus mirrors native foreground Q.
fn fixture() -> Fx {
    let backend = BackendHandle::default_for_test();
    let mut eng = TilingEngine::new(LayoutConfig::default());
    let a = OutputId::from_name("by-id-a");
    let b = OutputId::from_name("by-id-b");
    eng.register_monitor(a, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));
    eng.register_monitor(
        b,
        Rect::new(1920, 0, 1920, 1080),
        Rect::new(1920, 0, 1920, 1040),
    );
    eng.set_focused_output(a);
    for hwnd in [P, Q, R, S] {
        eng.add_window(info(hwnd), &backend);
    }
    for hwnd in [U, V] {
        eng.add_window_with_target(info(hwnd), AddWindowTarget::Output(b), &backend, false);
    }
    for hwnd in [W, W2] {
        eng.add_window_with_target(
            info(hwnd),
            AddWindowTarget::Workspace {
                output: b,
                workspace_id: 5,
            },
            &backend,
            false,
        );
    }
    eng.set_focused_output(a);
    assert!(eng.focus_action_window(WindowId::new(Q), &backend));
    eng.sync_os_foreground(WindowId::new(Q), &backend);
    let engine = Arc::new(parking_lot::RwLock::new(eng));
    let mut server = IpcServer::new();
    server.set_engine(engine.clone());
    server.set_backend(backend.clone());
    Fx {
        server,
        engine,
        backend,
        a,
        b,
    }
}

fn selection(fx: &Fx) -> Selection {
    let engine = fx.engine.read();
    [fx.a, fx.b]
        .iter()
        .map(|output| {
            let monitor = engine.monitors().get(output).unwrap();
            (monitor.active_workspace, monitor.focus_window)
        })
        .collect()
}

/// Run `name --id <id>` through the real handler. Returns the activation attempts, the
/// simulated native foreground afterwards, and the managed state before and after.
fn run(fx: &Fx, name: &str, id: isize) -> (Vec<isize>, isize, Selection, Selection) {
    let before = selection(fx);
    let ((reply, native), attempts) =
        with_native_foreground(Q, Activation::SwitchWithoutNotice, || {
            let reply = fx.server.process_message(IpcMessage::RunAction {
                name: name.into(),
                args: vec!["--id".into(), id.to_string()],
                quoted: Vec::new(),
            });
            (reply, fx.backend.foreground_window())
        });
    assert_eq!(reply["success"], true, "{name} --id {id}: {reply}");
    (attempts, native, before, selection(fx))
}

/// Native and managed focus are what the fixture made them.
fn assert_focus_neutral(fx: &Fx, run: &(Vec<isize>, isize, Selection, Selection)) {
    let (attempts, native, before, after) = run;
    assert!(attempts.is_empty(), "activation requested for {attempts:?}");
    assert_eq!(*native, Q, "the native foreground moved");
    assert_eq!(before, after, "a workspace or focus selection moved");
    let engine = fx.engine.read();
    assert_eq!(engine.monitors().focused_id(), Some(fx.a));
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(Q as u64));
}

fn column_of(fx: &Fx, hwnd: isize) -> Option<(OutputId, i32, usize)> {
    fx.engine.read().locate_window(WindowId::new(hwnd))
}

fn is_floating(fx: &Fx, hwnd: isize) -> bool {
    fx.engine.read().is_floating(WindowId::new(hwnd))
}

#[test]
fn the_fixture_starts_with_q_focused_everywhere() {
    let fx = fixture();
    let engine = fx.engine.read();
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(Q as u64));
    assert_eq!(engine.monitors().focused_id(), Some(fx.a));
    assert_eq!(
        engine.monitors().get(&fx.a).unwrap().focus_window,
        Some(WindowId::new(Q))
    );
    assert_eq!(column_of(&fx, R).map(|c| c.2), Some(2));
    assert_eq!(column_of(&fx, W).map(|c| c.1), Some(5));
}

#[test]
fn consume_or_expel_window_left_by_id_moves_the_named_tile_and_nothing_else() {
    let fx = fixture();
    let outcome = run(&fx, "consume-or-expel-window-left", R);
    assert_focus_neutral(&fx, &outcome);
    // R was alone in column 2: it now shares Q's column (column 1).
    assert_eq!(
        column_of(&fx, R).map(|c| c.2),
        column_of(&fx, Q).map(|c| c.2)
    );
    assert_eq!(column_of(&fx, R).map(|c| c.2), Some(1));
}

#[test]
fn consume_or_expel_window_right_by_id_acts_on_another_output() {
    let fx = fixture();
    // U is alone in column 0 of monitor B's active workspace: it joins V on its right.
    let outcome = run(&fx, "consume-or-expel-window-right", U);
    assert_focus_neutral(&fx, &outcome);
    assert_eq!(
        column_of(&fx, U).map(|c| c.2),
        column_of(&fx, V).map(|c| c.2)
    );
    assert_eq!(column_of(&fx, U).map(|c| c.0), Some(fx.b));
    // A floating window has no column to leave: nothing happens, focus stays.
    let engine_floating = {
        let mut engine = fx.engine.write();
        engine.set_floating_by_id(WindowId::new(S), Some(true), &fx.backend);
        true
    };
    assert!(engine_floating && is_floating(&fx, S));
    let outcome = run(&fx, "consume-or-expel-window-left", S);
    assert_focus_neutral(&fx, &outcome);
    assert!(is_floating(&fx, S));
}

#[test]
fn consume_or_expel_window_by_id_expels_a_tile_that_shares_its_column() {
    let fx = fixture();
    run(&fx, "consume-or-expel-window-left", R);
    assert_eq!(column_of(&fx, R).map(|c| c.2), Some(1));
    // R shares Q's column now; expelling it right gives R a column of its own again.
    let outcome = run(&fx, "consume-or-expel-window-right", R);
    assert_focus_neutral(&fx, &outcome);
    assert_eq!(column_of(&fx, R).map(|c| c.2), Some(2));
    assert_eq!(column_of(&fx, Q).map(|c| c.2), Some(1));
}

#[test]
fn center_window_by_id_centres_only_the_active_column_of_its_workspace() {
    let fx = fixture();
    let scroll = |fx: &Fx, output: OutputId, workspace: i32| {
        fx.engine.read().monitors().get(&output).unwrap().workspaces[&workspace]
            .scroll_offset
            .x
    };
    let set = |fx: &Fx, output: OutputId, workspace: i32, x: i32| {
        fx.engine
            .write()
            .monitors_mut()
            .get_mut(&output)
            .unwrap()
            .workspaces
            .get_mut(&workspace)
            .unwrap()
            .scroll_offset
            .x = x;
    };
    // W2 is the active column of monitor B's inactive workspace 5, W is not.
    set(&fx, fx.b, 5, 12345);
    let outcome = run(&fx, "center-window", W);
    assert_focus_neutral(&fx, &outcome);
    assert_eq!(
        scroll(&fx, fx.b, 5),
        12345,
        "a column that is not active stays"
    );
    let outcome = run(&fx, "center-window", W2);
    assert_focus_neutral(&fx, &outcome);
    assert_ne!(scroll(&fx, fx.b, 5), 12345, "the active column is centred");
    // A tile of the focused workspace that is not its focus is not centred either.
    set(&fx, fx.a, 0, 777);
    let outcome = run(&fx, "center-window", S);
    assert_focus_neutral(&fx, &outcome);
    assert_eq!(scroll(&fx, fx.a, 0), 777);
}

#[test]
fn center_window_by_id_centres_a_floating_window_without_focusing_it() {
    let fx = fixture();
    fx.engine
        .write()
        .set_floating_by_id(WindowId::new(S), Some(true), &fx.backend);
    let outcome = run(&fx, "center-window", S);
    assert_focus_neutral(&fx, &outcome);
    assert!(is_floating(&fx, S));
}

#[test]
fn reset_window_height_by_id_resets_only_the_named_tile() {
    let fx = fixture();
    run(&fx, "consume-or-expel-window-left", R);
    {
        let mut engine = fx.engine.write();
        let strip = engine
            .monitors_mut()
            .get_mut(&fx.a)
            .unwrap()
            .workspaces
            .get_mut(&0)
            .unwrap();
        for tile in &mut strip.columns[1].tiles {
            tile.preferred_height = Some(300);
        }
    }
    let outcome = run(&fx, "reset-window-height", R);
    assert_focus_neutral(&fx, &outcome);
    let engine = fx.engine.read();
    let column = &engine.monitors().get(&fx.a).unwrap().workspaces[&0].columns[1];
    let height = |hwnd: isize| {
        column
            .tiles
            .iter()
            .find(|tile| tile.window_id == WindowId::new(hwnd))
            .unwrap()
            .preferred_height
    };
    assert_eq!(height(R), None);
    assert_eq!(height(Q), Some(300), "the focused tile keeps its height");
}

#[test]
fn switch_preset_window_width_by_id_cycles_the_named_tiles_column() {
    let fx = fixture();
    let preset = |fx: &Fx, hwnd: isize| {
        let (output, workspace, column) = column_of(fx, hwnd).unwrap();
        fx.engine.read().monitors().get(&output).unwrap().workspaces[&workspace].columns[column]
            .preset_idx
    };
    assert_eq!(preset(&fx, R), None);
    let outcome = run(&fx, "switch-preset-window-width", R);
    assert_focus_neutral(&fx, &outcome);
    assert!(preset(&fx, R).is_some(), "R's column took a preset");
    assert_eq!(preset(&fx, Q), None, "the focused column is untouched");
    let first = preset(&fx, R);
    let outcome = run(&fx, "switch-preset-window-width-back", R);
    assert_focus_neutral(&fx, &outcome);
    assert_ne!(preset(&fx, R), first, "back steps the other way");
    // The same on a tile of another output's inactive workspace.
    let outcome = run(&fx, "switch-preset-window-width", W);
    assert_focus_neutral(&fx, &outcome);
    assert!(preset(&fx, W).is_some());
}

#[test]
fn switch_preset_window_height_by_id_cycles_the_named_tile() {
    let fx = fixture();
    run(&fx, "consume-or-expel-window-left", R);
    let preset = |fx: &Fx| {
        let (output, workspace, column) = column_of(fx, R).unwrap();
        let engine = fx.engine.read();
        let tile = engine.monitors().get(&output).unwrap().workspaces[&workspace].columns[column]
            .tiles
            .iter()
            .find(|tile| tile.window_id == WindowId::new(R))
            .unwrap()
            .height_preset_idx;
        tile
    };
    assert_eq!(preset(&fx), None);
    let outcome = run(&fx, "switch-preset-window-height", R);
    assert_focus_neutral(&fx, &outcome);
    assert!(preset(&fx).is_some());
}

#[test]
fn toggle_window_floating_by_id_floats_and_tiles_without_taking_focus() {
    let fx = fixture();
    let outcome = run(&fx, "toggle-window-floating", R);
    assert_focus_neutral(&fx, &outcome);
    assert!(is_floating(&fx, R));
    // Back into the strip: it sits right of the active column but does not become active.
    let outcome = run(&fx, "toggle-window-floating", R);
    assert_focus_neutral(&fx, &outcome);
    assert!(!is_floating(&fx, R));
    assert!(column_of(&fx, R).is_some());
    // A window of monitor B's inactive workspace: that workspace, output and focus stay.
    let outcome = run(&fx, "toggle-window-floating", W);
    assert_focus_neutral(&fx, &outcome);
    assert!(is_floating(&fx, W));
    let outcome = run(&fx, "toggle-window-floating", W);
    assert_focus_neutral(&fx, &outcome);
    assert!(!is_floating(&fx, W));
}

#[test]
fn move_window_to_floating_and_tiling_by_id_are_idempotent_and_focus_neutral() {
    let fx = fixture();
    let outcome = run(&fx, "move-window-to-floating", S);
    assert_focus_neutral(&fx, &outcome);
    assert!(is_floating(&fx, S));
    // Already floating: nothing changes.
    let outcome = run(&fx, "move-window-to-floating", S);
    assert_focus_neutral(&fx, &outcome);
    assert!(is_floating(&fx, S));
    let outcome = run(&fx, "move-window-to-tiling", S);
    assert_focus_neutral(&fx, &outcome);
    assert!(!is_floating(&fx, S));
    // Already tiled: nothing changes.
    let outcome = run(&fx, "move-window-to-tiling", P);
    assert_focus_neutral(&fx, &outcome);
    assert!(!is_floating(&fx, P));
    // Another output's window.
    let outcome = run(&fx, "move-window-to-floating", V);
    assert_focus_neutral(&fx, &outcome);
    assert!(is_floating(&fx, V));
}

#[test]
fn an_unknown_id_is_refused_and_changes_nothing() {
    let fx = fixture();
    let before = selection(&fx);
    let ((reply, native), attempts) =
        with_native_foreground(Q, Activation::SwitchWithoutNotice, || {
            let reply = fx.server.process_message(IpcMessage::RunAction {
                name: "toggle-window-floating".into(),
                args: vec!["--id".into(), "-9999".into()],
                quoted: Vec::new(),
            });
            (reply, fx.backend.foreground_window())
        });
    assert_eq!(reply["success"], false, "{reply}");
    assert!(attempts.is_empty());
    assert_eq!(native, Q);
    assert_eq!(before, selection(&fx));
}
