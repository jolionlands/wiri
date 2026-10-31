//! TEMP draft, not registered or executed. Future child of ipc::server.
//!
//! COMPOSITION ONLY: two real typed-wire normalization controls, then two real
//! common RunAction-handler regressions using negative model HWNDs. This is not
//! typed end-to-end IPC or observed desktop-foreground evidence. Typed wire IDs
//! are u64, so negative model IDs cannot pass that boundary without a new seam.
//!
//! Pin e19df12: niri-ipc/src/lib.rs:747-761; src/input/mod.rs:1704-1717;
//! src/layout/mod.rs:3065-3086; src/layout/workspace.rs:1271-1279.
//! Wiri wire.rs:186-193 lowers both typed preset actions to RunAction; the
//! shared server.rs:1094 pre-focus happens before message_loop.rs:1043's
//! explicit floating-target preservation. This draft changes no production API.
//!
//! Every model HWND is negative and excludes Win32 pseudo handles. An outer
//! existing refused-activation guard covers all fixture setup, and a nested one
//! covers the WHOLE actual process_message invocation, including its pre-focus.
//! No input pump, hooks, real/foreign HWNDs, global ENGINE, or new backend seam.

use super::{IpcMessage, IpcServer};
use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;
use crate::backend::{BackendHandle, WindowInfo, WindowState};
use crate::config::types::SizeSpec;
use crate::layout::{Column, LayoutConfig, Tile, TilingEngine, Workspace};
use crate::utils::{OutputId, Rect, WindowId};
use std::sync::Arc;

const SOURCE: isize = -9301;
const OTHER: isize = -9302;
const TARGET: isize = -9303;

fn assert_typed_normalization(variant: &str, canonical: &str) {
    let request = crate::ipc::wire::decode(serde_json::json!({
        "Action": {(variant): {"id": 9303u64}}
    }))
    .expect("a positive typed ID must reach normalization, not validation failure");
    assert_eq!(request.dialect, crate::ipc::wire::Dialect::Niri);
    assert_eq!(request.name, "run_action");
    let IpcMessage::RunAction { name, args, quoted } = request.message else {
        panic!("typed height preset must enter the common RunAction route");
    };
    assert_eq!(name, canonical);
    assert_eq!(args, vec!["--id".to_string(), "9303".to_string()]);
    assert_eq!(quoted, vec![false, false]);
    let (rest, rest_quoted, target) = super::split_window_target(&args, &quoted).unwrap();
    assert!(rest.is_empty());
    assert!(rest_quoted.is_empty());
    assert_eq!(target, Some(9303));
    let action = crate::input::parse_action_name(&name, &rest).unwrap();
    match canonical {
        "switch-preset-window-height" => assert!(matches!(
            action,
            crate::input::Action::WindowHeightPresetCycle
        )),
        "switch-preset-window-height-back" => assert!(matches!(
            action,
            crate::input::Action::WindowHeightPresetCycleBack
        )),
        _ => unreachable!("the fixture supplies the two canonical preset actions"),
    }
}

#[test]
fn parity_floating_height_ipc_typed_forward_normalizes_to_common_route() {
    assert_typed_normalization("SwitchPresetWindowHeight", "switch-preset-window-height");
}

#[test]
fn parity_floating_height_ipc_typed_back_normalizes_to_common_route() {
    assert_typed_normalization(
        "SwitchPresetWindowHeightBack",
        "switch-preset-window-height-back",
    );
}

type SharedEngine = Arc<parking_lot::RwLock<TilingEngine>>;

fn handler_fixture() -> (IpcServer, SharedEngine, OutputId, OutputId) {
    let backend = BackendHandle::default_for_test();
    let mut eng = TilingEngine::new(LayoutConfig {
        outer_gaps: (0, 0, 0, 0),
        column_gap: 0,
        window_gap: 0,
        border_width: 0,
        reserve_top: 0,
        reserve_bottom: 0,
        reserve_left: 0,
        reserve_right: 0,
        preset_window_heights: vec![
            SizeSpec::Fixed(200),
            SizeSpec::Fixed(400),
            SizeSpec::Fixed(600),
        ],
        ..LayoutConfig::default()
    });
    let source = OutputId::from_name("height-ipc-source");
    let target = OutputId::from_name("height-ipc-target");
    for (output, x) in [(source, 0), (target, 1200)] {
        eng.register_monitor_with_scale(
            output,
            Rect::new(x, 0, 1200, 1200),
            Rect::new(x, 0, 1200, 1000),
            1.0,
        );
    }
    eng.set_output_name(source, "height-ipc-source");
    eng.set_output_name(target, "height-ipc-target");
    for (id, height) in [(SOURCE, 300), (OTHER, 300), (TARGET, 350)] {
        eng.add_window(
            WindowInfo {
                hwnd: id,
                title: format!("IPC height model {id}"),
                class_name: "FloatingHeightIpcModel".into(),
                process_id: 1,
                bounds: Rect::new(30, 40, 450, height),
                state: WindowState::Normal,
                is_visible: true,
            },
            &backend,
        );
    }
    let workspace = |name: &str, id: isize| {
        let mut workspace = Workspace::new();
        workspace.name = Some(name.to_string());
        let mut column = Column::new();
        column.tiles.push(Tile::new(WindowId::new(id)));
        column.select_window(WindowId::new(id));
        workspace.columns.push(column);
        workspace
    };
    {
        let monitor = eng.monitors_mut().get_mut(&source).unwrap();
        monitor.workspaces.clear();
        monitor.insert_workspace(0, workspace("height-source-active", SOURCE));
        monitor.switch_workspace(0);
        monitor.set_focus(WindowId::new(SOURCE));
    }
    {
        let monitor = eng.monitors_mut().get_mut(&target).unwrap();
        monitor.workspaces.clear();
        monitor.insert_workspace(0, workspace("height-target-active", OTHER));
        monitor.insert_workspace(7, workspace("height-target-inactive", TARGET));
        monitor.switch_workspace(0);
        monitor.set_focus(WindowId::new(OTHER));
    }
    eng.set_focused_output(source);
    eng.toggle_floating_for(WindowId::new(SOURCE), &backend);
    eng.toggle_floating_for(WindowId::new(TARGET), &backend);
    assert!(eng.update_window_bounds(WindowId::new(SOURCE), Rect::new(30, 40, 450, 300)));
    assert!(eng.update_window_bounds(WindowId::new(TARGET), Rect::new(1230, 40, 450, 350)));
    eng.sync_os_foreground(WindowId::new(SOURCE), &backend);
    let engine = Arc::new(parking_lot::RwLock::new(eng));
    let mut server = IpcServer::new();
    server.set_engine(engine.clone());
    server.set_backend(backend);
    (server, engine, source, target)
}

fn cursor(engine: &TilingEngine, output: OutputId) -> (i32, Option<usize>, Option<WindowId>) {
    let monitor = engine.monitors().get(&output).unwrap();
    (
        monitor.active_workspace,
        monitor.focus_column,
        monitor.focus_window,
    )
}

fn assert_common_handler_preserves_focus(name: &str, expected_height: i32) {
    // Fixture operations can try activation too; never leave setup outside the guard.
    let _ = with_refused_activation_attempts(|| {
        let (server, engine, source, target) = handler_fixture();
        let before = engine.read().ipc_snapshot();
        let before_source = before
            .windows
            .iter()
            .find(|w| w.id == SOURCE as u64)
            .unwrap();
        let before_target = before
            .windows
            .iter()
            .find(|w| w.id == TARGET as u64)
            .unwrap();
        assert_eq!(engine.read().monitors().focused_id(), Some(source));
        assert_eq!(before.focused_window(), Some(SOURCE as u64));
        assert!(before_source.is_floating && before_target.is_floating);
        assert_eq!(before_source.layout.window_size, (450, 300));
        assert_eq!(before_target.layout.window_size, (450, 350));
        assert_eq!(before_target.output.as_deref(), Some("height-ipc-target"));
        let cursors = (
            cursor(&engine.read(), source),
            cursor(&engine.read(), target),
        );
        assert_eq!(cursors.0 .0, 0);
        assert_eq!(cursors.1 .0, 0);
        assert_ne!(
            before_target.workspace_id,
            before
                .windows
                .iter()
                .find(|w| w.id == OTHER as u64)
                .unwrap()
                .workspace_id
        );
        let request = IpcMessage::RunAction {
            name: name.to_string(),
            args: vec!["--id".to_string(), TARGET.to_string()],
            quoted: Vec::new(),
        };

        let (reply, attempts) =
            with_refused_activation_attempts(|| server.process_message(request));

        assert_eq!(reply["success"], true, "{reply}");
        let eng = engine.read();
        let after = eng.ipc_snapshot();
        let after_target = after
            .windows
            .iter()
            .find(|w| w.id == TARGET as u64)
            .unwrap();
        let after_source = after
            .windows
            .iter()
            .find(|w| w.id == SOURCE as u64)
            .unwrap();
        assert_eq!(
            after_target.layout.window_size,
            (450, expected_height),
            "the actual handler must resize the explicit float"
        );
        assert!(after_target.is_floating);
        assert_eq!(after_target.workspace_id, before_target.workspace_id);
        assert_eq!(after_target.output, before_target.output);
        assert_eq!(after_source.layout, before_source.layout);
        assert_eq!(
            eng.monitors().focused_id(),
            Some(source),
            "resizing an explicit float must not select its output"
        );
        assert_eq!((cursor(&eng, source), cursor(&eng, target)), cursors);
        assert_eq!(after.workspaces, before.workspaces);
        assert_eq!(
            after.focused_window(),
            Some(SOURCE as u64),
            "the model OS-foreground observation must remain unchanged"
        );
        assert!(
            attempts.is_empty(),
            "explicit height must not request native activation: {attempts:?}"
        );
    });
}

#[test]
fn parity_floating_height_ipc_common_forward_preserves_inactive_target_focus() {
    assert_common_handler_preserves_focus("switch-preset-window-height", 400);
}

#[test]
fn parity_floating_height_ipc_common_back_preserves_inactive_target_focus() {
    assert_common_handler_preserves_focus("switch-preset-window-height-back", 200);
}
