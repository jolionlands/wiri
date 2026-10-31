//! Floating width preset memory across config reloads and workspace transfers.
//!
//! Oracle: niri e19df12cf81ffe805609fee497da1a097066ad2f,
//! tile.rs229-235 invalidates a remembered floating width index on every changed
//! preset list; floating.rs648-687 selects by current size when no index remains.
//! workspace.rs636-657/floating.rs425-426 apply destination config on insertion.
//!
//! All setup, reload, action and transfer calls run inside the existing refused
//! activation-attempt guard. Model HWND -9301 is invalid and is not a Win32 pseudo
//! handle; no native HWND, pump, hook or input fixture is created. default_for_test
//! alone is not an activation guard. Existing cfg(test) batch positioning is a
//! stub; direct float positioning can only address the invalid model HWND with
//! NOACTIVATE. These tests cover engine reload/dispatch, not IPC or native focus.

use super::*;
use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;
use crate::backend::message_loop::perform_action_on;
use crate::backend::WindowState;
use crate::config::types::SizeSpec;
use crate::config::{Config, LayoutConfigPartial, WorkspaceConfig};
use crate::input::{parse_action_name, Action, WorkspaceRef};

type WidthEngine = Arc<parking_lot::RwLock<TilingEngine>>;

fn widths_a() -> Vec<SizeSpec> {
    vec![
        SizeSpec::Fixed(900),
        SizeSpec::Fixed(300),
        SizeSpec::Fixed(600),
    ]
}

fn widths_b() -> Vec<SizeSpec> {
    vec![
        SizeSpec::Fixed(700),
        SizeSpec::Fixed(200),
        SizeSpec::Fixed(500),
    ]
}

fn width_config(global: Vec<SizeSpec>, destination: Option<Vec<SizeSpec>>) -> Config {
    let mut source = WorkspaceConfig::default_config();
    source.name = "width active".into();
    source.monitor = "floating-width-model".into();
    // Deliberately no source override: reload must reach the named workspace
    // through the real global -> output -> named-workspace inheritance path.
    let mut other = WorkspaceConfig::default_config();
    other.name = "width destination".into();
    other.monitor = "floating-width-model".into();
    other.layout_override = destination.map(|list| LayoutConfigPartial {
        preset_column_widths: Some(list),
        ..Default::default()
    });
    let mut config = Config::default();
    config.layout.preset_column_widths = global;
    config.workspace = vec![source, other];
    config
}

fn width_fixture() -> (WidthEngine, BackendHandle, OutputId, WindowId) {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let output = OutputId::from_name("floating-width-model");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1200, 900),
        Rect::new(0, 0, 1200, 860),
    );
    engine.set_output_name(output, "floating-width-model");
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&0)
        .unwrap()
        .name = Some("width active".into());
    let window = WindowId::new(-9301);
    let rect = Rect::new(30, 40, 450, 300);
    engine.tiled_windows.insert(
        window,
        WindowInfo {
            hwnd: window.as_isize(),
            title: "width lifecycle model".into(),
            class_name: "FloatingWidthModel".into(),
            process_id: 1,
            bounds: rect,
            state: WindowState::Normal,
            is_visible: true,
        },
    );
    engine.register_float(window, output, 0, rect);
    engine.record_ipc_foreground(window);
    (
        Arc::new(parking_lot::RwLock::new(engine)),
        BackendHandle::default_for_test(),
        output,
        window,
    )
}

fn reload_config(engine: &WidthEngine, backend: &BackendHandle, config: Config) {
    // These are the real engine publication/reconciliation calls in
    // main.rs apply_loaded_config. No global input/output backend reconfiguration.
    let layout = LayoutConfig::from_config(&config);
    let mut engine = engine.write();
    engine.update_config(layout);
    engine.set_full_config(config);
    engine.apply_reloaded_layout(backend);
}

fn cycle_width(engine: &WidthEngine, backend: &BackendHandle, window: WindowId) {
    let action = parse_action_name("switch-preset-window-width", &[])
        .expect("existing canonical width action");
    assert!(matches!(action, Action::WindowWidthPresetCycle));
    let quit = Arc::new(parking_lot::Mutex::new(None));
    perform_action_on(&action, engine, backend, &quit, Some(window.as_isize()));
}

fn assert_width(engine: &WidthEngine, window: WindowId, expected: u32) {
    assert_eq!(
        engine.read().float_homes[&window].rect,
        Rect::new(30, 40, expected, 300),
    );
}

#[test]
fn parity_floating_width_list_same_reload_retains_index() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (engine, backend, output, window) = width_fixture();
        reload_config(&engine, &backend, width_config(widths_a(), None));
        assert_eq!(
            engine
                .read()
                .config_for_window(window, output)
                .preset_column_widths,
            widths_a()
        );
        cycle_width(&engine, &backend, window);
        assert_width(&engine, window, 900);
        reload_config(&engine, &backend, width_config(widths_a(), None));
        assert_width(&engine, window, 900);
        cycle_width(&engine, &backend, window);
        assert_width(&engine, window, 300);
    });
    assert!(
        attempts.is_empty(),
        "width fixture requested activation: {attempts:?}"
    );
}

#[test]
fn parity_floating_width_list_changed_back_reload_discards_index() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (engine, backend, output, window) = width_fixture();
        reload_config(&engine, &backend, width_config(widths_a(), None));
        cycle_width(&engine, &backend, window);
        assert_width(&engine, window, 900);
        reload_config(&engine, &backend, width_config(widths_b(), None));
        assert_eq!(
            engine
                .read()
                .config_for_window(window, output)
                .preset_column_widths,
            widths_b()
        );
        assert_width(&engine, window, 900);
        // No cycle on B: checking list identity only at the next cycle is too late.
        reload_config(&engine, &backend, width_config(widths_a(), None));
        assert_eq!(
            engine
                .read()
                .config_for_window(window, output)
                .preset_column_widths,
            widths_a()
        );
        assert_width(&engine, window, 900);
        cycle_width(&engine, &backend, window);
        assert_width(&engine, window, 900);
    });
    assert!(
        attempts.is_empty(),
        "width fixture requested activation: {attempts:?}"
    );
}

fn assert_width_transfer(destination_list: Option<Vec<SizeSpec>>, expected: u32) {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (engine, backend, output, window) = width_fixture();
        reload_config(
            &engine,
            &backend,
            width_config(widths_a(), destination_list),
        );
        let source_ref = WorkspaceRef::Name("width active".into());
        let other_ref = WorkspaceRef::Name("width destination".into());
        let source = engine
            .read()
            .resolve_workspace_reference(output, &source_ref)
            .unwrap();
        let other = engine
            .read()
            .resolve_workspace_reference(output, &other_ref)
            .unwrap();
        assert_eq!(source.0, output);
        assert_eq!(other.0, output);
        assert_ne!(source.1, other.1);
        assert_eq!(engine.read().float_location(window), Some(source));
        cycle_width(&engine, &backend, window);
        assert_width(&engine, window, 900);
        engine
            .write()
            .move_window_to_workspace_ref(Some(window), other_ref, false, &backend);
        assert_eq!(engine.read().float_location(window), Some(other));
        assert_width(&engine, window, 900);
        // No cycle or reload while on the destination workspace.
        engine
            .write()
            .move_window_to_workspace_ref(Some(window), source_ref, false, &backend);
        assert_eq!(engine.read().float_location(window), Some(source));
        assert_width(&engine, window, 900);
        cycle_width(&engine, &backend, window);
        assert_width(&engine, window, expected);
    });
    assert!(
        attempts.is_empty(),
        "width fixture requested activation: {attempts:?}"
    );
}

#[test]
fn parity_floating_width_list_same_transfer_retains_index() {
    assert_width_transfer(None, 300);
}

#[test]
fn parity_floating_width_list_changed_transfer_discards_index() {
    assert_width_transfer(Some(widths_b()), 900);
}
