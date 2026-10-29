//! TEMPORARY DRAFT: future child of layout::engine, not registered in src.
//!
//! Source oracle: niri e19df12cf81ffe805609fee497da1a097066ad2f,
//! src/layout/floating.rs:701-751 (selection, wrap, remembered index),
//! :797-854 (explicit height clears index; fixed/logical vs proportional),
//! :1425-1429 (preset units); layout/workspace.rs:1271-1279 and
//! layout/mod.rs:3065-3086 (current/explicit routing without focusing).
//! tile.rs:229-235 clears the remembered index only when the preset list changes.
//!
//! Safety: all model HWNDs are negative, excluding Win32 pseudo handles.
//! Every shared dispatcher invocation is enclosed in the EXISTING refused
//! activation-attempt guard, which returns before SetForegroundWindow/input
//! nudges/keyboard escalation. default_for_test alone is NOT that guard.
//! Batch positioning uses the existing cfg(test) stub; direct float placement
//! can attempt position calls only on invalid model HWNDs, with NOACTIVATE.
//! Current dispatch may read GetForegroundWindow; no real HWND can match these
//! negative fixture IDs, so it resolves the model's existing state target.
//! No fixture starts an input pump, creates an HWND, or installs a hook.
//!
//! These are core/shared-dispatch tests, NOT end-to-end IPC tests. The IPC
//! server currently has an additional explicit-ID pre-focus path to audit.

use super::*;
use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;
use crate::backend::message_loop::{perform_action, perform_action_on};
use crate::backend::WindowState;
use crate::config::types::SizeSpec;
use crate::config::{Config, LayoutConfigPartial, WorkspaceConfig};
use crate::input::{parse_action_name, Action};
use crate::layout::{Column, SizeAxis, Tile, Workspace};

type SharedHeightEngine = Arc<parking_lot::RwLock<TilingEngine>>;

fn model_window(id: WindowId, rect: Rect) -> WindowInfo {
    WindowInfo {
        hwnd: id.as_isize(),
        title: format!("height model {}", id.as_isize()),
        class_name: "FloatingHeightModel".into(),
        process_id: 1,
        bounds: rect,
        state: WindowState::Normal,
        is_visible: true,
    }
}

fn add_model_float(
    engine: &mut TilingEngine,
    id: WindowId,
    output: OutputId,
    workspace: i32,
    rect: Rect,
) {
    engine.tiled_windows.insert(id, model_window(id, rect));
    engine.register_float(id, output, workspace, rect);
}

fn height_fixture(
    presets: Vec<SizeSpec>,
    height: u32,
    scale: f64,
) -> (SharedHeightEngine, BackendHandle, OutputId, WindowId) {
    let mut engine = TilingEngine::new(LayoutConfig {
        outer_gaps: (0, 0, 0, 0),
        column_gap: 0,
        window_gap: 0,
        border_width: 0,
        reserve_top: 0,
        reserve_bottom: 0,
        reserve_left: 0,
        reserve_right: 0,
        preset_window_heights: presets,
        ..LayoutConfig::default()
    });
    let output = OutputId::from_name("floating-height-model");
    engine.register_monitor_with_scale(
        output,
        Rect::new(0, 0, 1200, 1200),
        Rect::new(0, 0, 1200, 1000),
        scale,
    );
    engine.set_output_name(output, "floating-height-model");
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&0)
        .unwrap()
        .name = Some("height active".into());
    let id = WindowId::new(-9101);
    add_model_float(&mut engine, id, output, 0, Rect::new(30, 40, 450, height));
    engine.record_ipc_foreground(id);
    (
        Arc::new(parking_lot::RwLock::new(engine)),
        BackendHandle::default_for_test(),
        output,
        id,
    )
}

fn fixed_presets() -> Vec<SizeSpec> {
    vec![
        SizeSpec::Fixed(200),
        SizeSpec::Fixed(400),
        SizeSpec::Fixed(600),
    ]
}

fn run_height_action(
    engine: &SharedHeightEngine,
    backend: &BackendHandle,
    forwards: bool,
    explicit: Option<WindowId>,
) -> Vec<isize> {
    let name = if forwards {
        "switch-preset-window-height"
    } else {
        "switch-preset-window-height-back"
    };
    let action = parse_action_name(name, &[]).expect("existing canonical action");
    assert!(matches!(
        action,
        Action::WindowHeightPresetCycle | Action::WindowHeightPresetCycleBack
    ));
    let quit = Arc::new(parking_lot::Mutex::new(None));
    let ((), attempts) = with_refused_activation_attempts(|| match explicit {
        Some(id) => perform_action_on(&action, engine, backend, &quit, Some(id.as_isize())),
        None => perform_action(&action, engine, backend, &quit),
    });
    attempts
}

fn float_rect(engine: &SharedHeightEngine, id: WindowId) -> Rect {
    engine.read().float_homes[&id].rect
}

#[test]
fn parity_floating_height_current_forward_selects_first_larger_nonmember() {
    let (engine, backend, _, id) = height_fixture(fixed_presets(), 300, 1.0);
    let _ = run_height_action(&engine, &backend, true, None);
    assert_eq!(float_rect(&engine, id), Rect::new(30, 40, 450, 400));
}

#[test]
fn parity_floating_height_explicit_back_selects_last_smaller_nonmember() {
    let (engine, backend, _, id) = height_fixture(fixed_presets(), 300, 1.0);
    let _ = run_height_action(&engine, &backend, false, Some(id));
    assert_eq!(float_rect(&engine, id), Rect::new(30, 40, 450, 200));
}

#[test]
fn parity_floating_height_forward_and_back_wrap() {
    let (engine, backend, _, id) = height_fixture(fixed_presets(), 300, 1.0);
    for (forwards, expected) in [
        (true, 400),
        (true, 600),
        (true, 200),
        (false, 600),
        (false, 400),
        (false, 200),
        (false, 600),
    ] {
        let _ = run_height_action(&engine, &backend, forwards, None);
        assert_eq!(float_rect(&engine, id).size.h, expected);
    }
}

#[test]
fn parity_floating_height_nonmember_comparison_has_one_pixel_allowance() {
    // Pinned floating.rs uses current + 1 < preset, or preset + 1 < current.
    for (current, forwards, expected) in [(399, true, 600), (401, false, 200)] {
        let (engine, backend, _, id) = height_fixture(fixed_presets(), current, 1.0);
        let _ = run_height_action(&engine, &backend, forwards, Some(id));
        assert_eq!(float_rect(&engine, id).size.h, expected);
    }
}

#[test]
fn parity_floating_height_remembers_unsorted_order_across_move_and_refresh() {
    let presets = vec![
        SizeSpec::Fixed(600),
        SizeSpec::Fixed(200),
        SizeSpec::Fixed(400),
    ];
    let (engine, backend, _, id) = height_fixture(presets, 300, 1.0);
    for expected in [600, 200] {
        let _ = run_height_action(&engine, &backend, true, Some(id));
        assert_eq!(float_rect(&engine, id).size.h, expected);
    }
    engine.write().float_move_by(id, 0, 7, &backend);
    // Refresh uses the real shared dispatch, with activation still refused.
    let quit = Arc::new(parking_lot::Mutex::new(None));
    let _ = with_refused_activation_attempts(|| {
        perform_action(&Action::Refresh, &engine, &backend, &quit)
    });
    let _ = run_height_action(&engine, &backend, true, Some(id));
    // Recomputing from 200 would choose 600; remembered index 1 must choose 400.
    assert_eq!(float_rect(&engine, id), Rect::new(30, 47, 450, 400));
}

#[test]
fn parity_floating_height_remembered_index_belongs_to_each_window() {
    let presets = vec![
        SizeSpec::Fixed(600),
        SizeSpec::Fixed(200),
        SizeSpec::Fixed(400),
    ];
    let (engine, backend, output, first) = height_fixture(presets, 300, 1.0);
    let second = WindowId::new(-9102);
    add_model_float(
        &mut engine.write(),
        second,
        output,
        0,
        Rect::new(500, 40, 450, 300),
    );
    let _ = run_height_action(&engine, &backend, true, Some(first));
    assert_eq!(float_rect(&engine, first).size.h, 600);
    let _ = run_height_action(&engine, &backend, true, Some(first));
    assert_eq!(float_rect(&engine, first).size.h, 200);
    let _ = run_height_action(&engine, &backend, true, Some(second));
    assert_eq!(float_rect(&engine, second).size.h, 600);
    let _ = run_height_action(&engine, &backend, true, Some(first));
    assert_eq!(float_rect(&engine, first).size.h, 400);
    assert_eq!(float_rect(&engine, second).size.h, 600);
}

#[test]
fn parity_floating_height_explicit_height_discards_remembered_index() {
    let presets = vec![
        SizeSpec::Fixed(600),
        SizeSpec::Fixed(200),
        SizeSpec::Fixed(400),
    ];
    let (engine, backend, _, id) = height_fixture(presets, 300, 1.0);
    for expected in [600, 200] {
        let _ = run_height_action(&engine, &backend, true, Some(id));
        assert_eq!(float_rect(&engine, id).size.h, expected);
    }
    engine.write().float_apply_size_change(
        id,
        SizeAxis::Height,
        SizeChange::SetFixed(200),
        &backend,
    );
    assert_eq!(float_rect(&engine, id).size.h, 200);
    let _ = run_height_action(&engine, &backend, true, Some(id));
    // Equal pixels still discard the index: search list order again, not index 2.
    assert_eq!(float_rect(&engine, id).size.h, 600);
}

#[test]
fn parity_floating_height_reload_with_same_list_retains_remembered_index() {
    let presets = vec![
        SizeSpec::Fixed(600),
        SizeSpec::Fixed(200),
        SizeSpec::Fixed(400),
    ];
    let (engine, backend, _, id) = height_fixture(presets.clone(), 300, 1.0);
    for expected in [600, 200] {
        let _ = run_height_action(&engine, &backend, true, Some(id));
        assert_eq!(float_rect(&engine, id).size.h, expected);
    }
    let mut config = Config::default();
    config.layout.preset_window_heights = presets;
    engine.write().set_full_config(config);
    let _ = run_height_action(&engine, &backend, true, Some(id));
    assert_eq!(
        float_rect(&engine, id).size.h,
        400,
        "same list must retain index 1"
    );
}

#[test]
fn parity_floating_height_reload_changed_list_restarts_nonmember_search() {
    let presets = vec![
        SizeSpec::Fixed(600),
        SizeSpec::Fixed(200),
        SizeSpec::Fixed(400),
    ];
    let (engine, backend, _, id) = height_fixture(presets, 300, 1.0);
    for expected in [600, 200] {
        let _ = run_height_action(&engine, &backend, true, Some(id));
        assert_eq!(float_rect(&engine, id).size.h, expected);
    }
    let mut config = Config::default();
    config.layout.preset_window_heights = vec![
        SizeSpec::Fixed(700),
        SizeSpec::Fixed(300),
        SizeSpec::Fixed(500),
    ];
    engine.write().set_full_config(config);
    let _ = run_height_action(&engine, &backend, true, Some(id));
    assert_eq!(
        float_rect(&engine, id).size.h,
        700,
        "changed list discards old index 1"
    );
}

#[test]
fn parity_floating_height_fixed_presets_scale_logical_pixels_without_tile_border() {
    let (engine, backend, _, id) = height_fixture(vec![SizeSpec::Fixed(300)], 200, 1.5);
    engine.write().config.border_width = 10;
    let _ = run_height_action(&engine, &backend, true, Some(id));
    // 300 logical px at 150%; floating windows do not add a Wiri tile border.
    assert_eq!(float_rect(&engine, id), Rect::new(30, 40, 450, 450));
}

#[test]
fn parity_floating_height_proportions_use_work_area_without_tiled_gaps() {
    let (engine, backend, _, id) = height_fixture(vec![SizeSpec::Proportion(0.5)], 200, 1.5);
    {
        let mut engine = engine.write();
        engine.config.outer_gaps = (30, 30, 30, 30);
        engine.config.window_gap = 40;
        engine.config.border_width = 10;
    }
    let _ = run_height_action(&engine, &backend, true, Some(id));
    // Physical work area is 1000 high (not full output 1200), already DPI-scaled.
    assert_eq!(float_rect(&engine, id), Rect::new(30, 40, 450, 500));
}

fn target_config_fixture(
    workspace_override: bool,
) -> (
    SharedHeightEngine,
    BackendHandle,
    OutputId,
    OutputId,
    WindowId,
    WindowId,
) {
    let (engine, backend, source_output, source) = height_fixture(fixed_presets(), 300, 1.0);
    let target_output = OutputId::from_name("floating-height-target-identity");
    let target = WindowId::new(-9103);
    {
        let mut engine = engine.write();
        engine.register_monitor_with_scale(
            target_output,
            Rect::new(1200, 0, 1200, 1200),
            Rect::new(1200, 0, 1200, 1000),
            1.5,
        );
        engine.set_output_name(target_output, "height-target-connector");
        let monitor = engine.monitors.get_mut(&target_output).unwrap();
        monitor.workspaces.get_mut(&0).unwrap().name = Some("target active".into());
        let mut inactive = Workspace::new();
        inactive.name = Some("height-special".into());
        monitor.workspaces.insert(7, inactive);

        let mut config = Config::default();
        config.layout.preset_window_heights = vec![SizeSpec::Fixed(120)];
        let mut output_config = config.output[0].clone();
        output_config.name = "height-target-connector".into();
        output_config.layout_override = Some(LayoutConfigPartial {
            preset_window_heights: Some(vec![SizeSpec::Fixed(260)]),
            ..Default::default()
        });
        config.output = vec![output_config];
        if workspace_override {
            let mut workspace_config = WorkspaceConfig::default_config();
            workspace_config.name = "height-special".into();
            workspace_config.layout_override = Some(LayoutConfigPartial {
                preset_window_heights: Some(vec![SizeSpec::Fixed(380)]),
                ..Default::default()
            });
            config.workspace = vec![workspace_config];
        }
        engine.set_full_config(config);
        add_model_float(
            &mut engine,
            target,
            target_output,
            7,
            Rect::new(1250, 40, 450, 300),
        );
        engine.monitors.set_focused(source_output);
        engine.record_ipc_foreground(source);
    }
    (
        engine,
        backend,
        source_output,
        target_output,
        source,
        target,
    )
}

#[test]
fn parity_floating_height_explicit_target_uses_own_output_presets_and_dpi() {
    let (engine, backend, _, _, source, target) = target_config_fixture(false);
    let _ = run_height_action(&engine, &backend, true, Some(target));
    assert_eq!(
        float_rect(&engine, target).size.h,
        390,
        "target output fixed 260 at 150%"
    );
    assert_eq!(float_rect(&engine, source).size.h, 300);
}

#[test]
fn parity_floating_height_explicit_target_workspace_overrides_own_output_presets() {
    let (engine, backend, _, _, source, target) = target_config_fixture(true);
    let _ = run_height_action(&engine, &backend, true, Some(target));
    assert_eq!(
        float_rect(&engine, target).size.h,
        570,
        "target workspace fixed 380 at 150%"
    );
    assert_eq!(float_rect(&engine, source).size.h, 300);
}

#[test]
fn parity_floating_height_inactive_explicit_dispatch_preserves_focus() {
    let (engine, backend, source_output, target_output, source, target) =
        target_config_fixture(true);
    let before = engine.read().ipc_snapshot().focused_window();
    let before_source = engine
        .read()
        .monitors
        .get(&source_output)
        .unwrap()
        .active_workspace;
    let before_target = engine
        .read()
        .monitors
        .get(&target_output)
        .unwrap()
        .active_workspace;
    let attempts = run_height_action(&engine, &backend, true, Some(target));
    // Assert focus separately before height so the existing resolver mutation is
    // exposed independently of the floating no-op. This does not cover IPC's
    // private process_message pre-focus; that needs its own server-child test.
    assert_eq!(engine.read().ipc_snapshot().focused_window(), before);
    assert_eq!(engine.read().monitors.focused_id(), Some(source_output));
    assert_eq!(
        engine
            .read()
            .monitors
            .get(&source_output)
            .unwrap()
            .active_workspace,
        before_source
    );
    assert_eq!(
        engine
            .read()
            .monitors
            .get(&target_output)
            .unwrap()
            .active_workspace,
        before_target
    );
    assert!(
        attempts.is_empty(),
        "a non-focusing explicit action requested activation: {attempts:?}"
    );
    assert_eq!(float_rect(&engine, source).size.h, 300);
    assert_eq!(float_rect(&engine, target).size.h, 570);
}

#[test]
fn parity_floating_height_existing_tiled_dispatch_still_cycles_and_wraps() {
    let (engine, backend, output, floating) = height_fixture(fixed_presets(), 300, 1.0);
    let tiled = WindowId::new(-9111);
    let other = WindowId::new(-9112);
    {
        let mut engine = engine.write();
        engine
            .tiled_windows
            .insert(tiled, model_window(tiled, Rect::new(0, 0, 450, 400)));
        engine
            .tiled_windows
            .insert(other, model_window(other, Rect::new(0, 400, 450, 400)));
        let mut column = Column::new();
        column.add_tile(Tile::new(tiled));
        column.add_tile(Tile::new(other));
        let monitor = engine.monitors.get_mut(&output).unwrap();
        monitor.workspaces.get_mut(&0).unwrap().columns.push(column);
        monitor.focus_column = Some(0);
        monitor.focus_window = Some(tiled);
        engine.record_ipc_foreground(tiled);
    }
    // Equal weights start at 500: first larger fixed preset is 600.
    for (forwards, expected, index) in [(true, 600, 2), (true, 200, 0), (false, 600, 2)] {
        let _ = run_height_action(&engine, &backend, forwards, None);
        let engine = engine.read();
        let tile = &engine.monitors.get(&output).unwrap().workspaces[&0].columns[0].tiles[0];
        assert_eq!(tile.preferred_height, Some(expected));
        assert_eq!(tile.height_preset_idx, Some(index));
        assert_eq!(engine.float_homes[&floating].rect.size.h, 300);
    }
}

#[test]
fn parity_floating_height_existing_tiled_proportional_preset_remains_a_share() {
    let (engine, backend, output, _) = height_fixture(vec![SizeSpec::Proportion(0.25)], 300, 1.0);
    let first = WindowId::new(-9121);
    let second = WindowId::new(-9122);
    {
        let mut engine = engine.write();
        for id in [first, second] {
            engine
                .tiled_windows
                .insert(id, model_window(id, Rect::new(0, 0, 450, 400)));
        }
        let mut column = Column::new();
        column.add_tile(Tile::new(first));
        column.add_tile(Tile::new(second));
        let monitor = engine.monitors.get_mut(&output).unwrap();
        monitor.workspaces.get_mut(&0).unwrap().columns.push(column);
        monitor.focus_column = Some(0);
        monitor.focus_window = Some(first);
        engine.record_ipc_foreground(first);
    }
    let _ = run_height_action(&engine, &backend, true, None);
    let engine = engine.read();
    let tile = &engine.monitors.get(&output).unwrap().workspaces[&0].columns[0].tiles[0];
    assert_eq!(tile.height_proportion, Some(0.25));
    assert_eq!(tile.preferred_height, None);
    assert_eq!(tile.height_preset_idx, Some(0));
}
// TEMP ONLY: append to the existing floating_height_parity_tests child for a
// separately pinned baseline. No production test registration is changed.
// Oracle: niri e19df12 tile.rs229-235 invalidates on each changed list;
// floating.rs724-741 applies the one-pixel allowance in logical units.

#[test]
fn parity_floating_height_followup_changed_back_list_still_discards_remembered_index() {
    let presets = vec![
        SizeSpec::Fixed(600),
        SizeSpec::Fixed(200),
        SizeSpec::Fixed(400),
    ];
    let (engine, backend, _, id) = height_fixture(presets.clone(), 300, 1.0);
    for expected in [600, 200] {
        let _ = run_height_action(&engine, &backend, true, Some(id));
        assert_eq!(float_rect(&engine, id).size.h, expected);
    }
    let mut replacement = Config::default();
    replacement.layout.preset_window_heights = vec![
        SizeSpec::Fixed(700),
        SizeSpec::Fixed(300),
        SizeSpec::Fixed(500),
    ];
    engine.write().set_full_config(replacement);
    let mut restored = Config::default();
    restored.layout.preset_window_heights = presets;
    engine.write().set_full_config(restored);
    let _ = run_height_action(&engine, &backend, true, Some(id));
    assert_eq!(
        float_rect(&engine, id).size.h,
        600,
        "each changed list discards the index, even when changed back before cycling"
    );
}

#[test]
fn parity_floating_height_followup_nonmember_allowance_is_logical_at_nonunit_scale() {
    for (scale, current_physical, presets, forwards, expected_physical) in [
        (
            1.5,
            600,
            vec![
                SizeSpec::Fixed(201),
                SizeSpec::Fixed(401),
                SizeSpec::Fixed(601),
            ],
            true,
            902,
        ),
        (2.0, 802, fixed_presets(), false, 400),
    ] {
        let (engine, backend, _, id) = height_fixture(presets, current_physical, scale);
        let _ = run_height_action(&engine, &backend, forwards, Some(id));
        // Forward: current 400 logical +1 is not <401, so choose601 (902px).
        // Back: 400+1 is not <current401 logical, so choose200 (400px).
        assert_eq!(
            float_rect(&engine, id),
            Rect::new(30, 40, 450, expected_physical)
        );
    }
}

// Oracle: pinned floating.rs818-833 resolves a proportional logical height and
// rounds it before requesting window size. At work1000physical / scale1.5,
// .4 gives266.666...logical ->267logical ->400.5physical ->401native.
#[test]
fn parity_floating_height_followup_proportion_rounds_logical_height_before_physical_request() {
    let (engine, backend, _, id) = height_fixture(vec![SizeSpec::Proportion(0.4)], 200, 1.5);
    let _ = run_height_action(&engine, &backend, true, Some(id));
    assert_eq!(float_rect(&engine, id), Rect::new(30, 40, 450, 401));
}
// TEMP ONLY: append after the frozen height child for a separately observed RED.
// Uses its existing height_fixture/run_height_action/float_rect and imports.
// No registration or production changes. All model HWNDs remain negative.
//
// Pin e19df12: workspace.rs636-657 transfers a floating tile into the destination
// floating layout; floating.rs425-426 calls tile.update_config on destination
// insertion; tile.rs229-235 invalidates the height index when that list differs.
// No cycle occurs on B: lazy validation only at the next cycle cannot observe it.

fn assert_height_transfer_roundtrip(destination_presets: Vec<SizeSpec>, expected: u32) {
    // Cover setup and both real transfer calls as well as the shared dispatcher.
    let _ = with_refused_activation_attempts(|| {
        let presets = vec![
            SizeSpec::Fixed(600),
            SizeSpec::Fixed(200),
            SizeSpec::Fixed(400),
        ];
        let (engine, backend, output, id) = height_fixture(presets.clone(), 300, 1.0);
        let source_name = "height active";
        let destination_name = "height-transfer-other";
        let workspace_config = |name: &str, list: Vec<SizeSpec>| {
            let mut workspace = WorkspaceConfig::default_config();
            workspace.name = name.into();
            workspace.monitor = "floating-height-model".into();
            workspace.layout_override = Some(LayoutConfigPartial {
                preset_window_heights: Some(list),
                ..Default::default()
            });
            workspace
        };
        let mut config = Config::default();
        config.layout.preset_window_heights = presets.clone();
        config.workspace = vec![
            workspace_config(source_name, presets),
            workspace_config(destination_name, destination_presets),
        ];
        engine.write().set_full_config(config);
        let source_ref = crate::input::WorkspaceRef::Name(source_name.into());
        let destination_ref = crate::input::WorkspaceRef::Name(destination_name.into());
        let source = engine
            .read()
            .resolve_workspace_reference(output, &source_ref)
            .unwrap();
        let destination = engine
            .read()
            .resolve_workspace_reference(output, &destination_ref)
            .unwrap();
        assert_eq!(source.0, output);
        assert_eq!(
            destination.0, output,
            "the round trip must stay on one output"
        );
        assert_ne!(source.1, destination.1);
        assert_eq!(engine.read().float_location(id), Some(source));
        assert_eq!(float_rect(&engine, id), Rect::new(30, 40, 450, 300));

        // Configured order is deliberately unsorted: the index is now 1, while
        // a fresh nonmember search from 200 would choose list entry 0 (600).
        for height in [600, 200] {
            let _ = run_height_action(&engine, &backend, true, Some(id));
            assert_eq!(float_rect(&engine, id).size.h, height);
        }
        let before_transfer = float_rect(&engine, id);
        engine
            .write()
            .move_window_to_workspace_ref(Some(id), destination_ref, false, &backend);
        assert_eq!(engine.read().float_location(id), Some(destination));
        assert_eq!(float_rect(&engine, id), before_transfer);
        // Deliberately no preset action, reload, refresh or config change on B.
        engine
            .write()
            .move_window_to_workspace_ref(Some(id), source_ref, false, &backend);
        assert_eq!(engine.read().float_location(id), Some(source));
        assert_eq!(float_rect(&engine, id), before_transfer);

        let _ = run_height_action(&engine, &backend, true, Some(id));
        assert_eq!(
            float_rect(&engine, id),
            Rect::new(
                before_transfer.loc.x,
                before_transfer.loc.y,
                before_transfer.size.w,
                expected
            ),
            "each destination list must be applied at transfer time, even without cycling there"
        );
    });
}

#[test]
fn parity_floating_height_transfer_changed_list_roundtrip_discards_index() {
    assert_height_transfer_roundtrip(
        vec![
            SizeSpec::Fixed(700),
            SizeSpec::Fixed(300),
            SizeSpec::Fixed(500),
        ],
        600,
    );
}

#[test]
fn parity_floating_height_transfer_same_list_roundtrip_preserves_index() {
    assert_height_transfer_roundtrip(
        vec![
            SizeSpec::Fixed(600),
            SizeSpec::Fixed(200),
            SizeSpec::Fixed(400),
        ],
        400,
    );
}
