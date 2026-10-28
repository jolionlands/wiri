//! Focused Lane F model tests. The parent registers this module under `cfg(test)`.
//! Sparse, deliberately unsorted keys distinguish workspace identity from list position.
//! Non-trailing workspaces are named or occupied so pruning cannot hide a bad target.

use super::*;
use crate::backend::WindowState;
use crate::input::WorkspaceRef;
use crate::layout::{Column, ColumnDisplay, Tile, Workspace};

fn window_info(hwnd: isize) -> WindowInfo {
    WindowInfo {
        hwnd,
        title: format!("Lane F window {hwnd}"),
        class_name: "WorkspaceActionFixture".to_string(),
        process_id: 1,
        bounds: Rect::new(40, 40, 400, 300),
        state: WindowState::Normal,
        is_visible: true,
    }
}

fn column(windows: &[isize]) -> Column {
    let mut column = Column::new();
    column.tiles = windows
        .iter()
        .map(|id| Tile::new(WindowId::new(*id)))
        .collect();
    if let Some(window) = windows.first() {
        column.select_window(WindowId::new(*window));
    }
    column
}

fn workspace(name: &str, columns: &[&[isize]]) -> Workspace {
    let mut workspace = Workspace::new();
    workspace.name = Some(name.to_string());
    workspace.columns = columns.iter().map(|windows| column(windows)).collect();
    workspace
}

fn install_monitor(
    engine: &mut TilingEngine,
    name: &str,
    x: i32,
    workspaces: Vec<(i32, Workspace)>,
    active: i32,
    focus: isize,
) -> OutputId {
    let output = OutputId::from_name(name);
    let bounds = Rect::new(x, 0, 1200, 800);
    let mut monitor = Monitor::new(output, bounds, bounds);
    monitor.remove_workspace(0);
    for (key, workspace) in workspaces {
        for tile in workspace.columns.iter().flat_map(|column| &column.tiles) {
            engine
                .tiled_windows
                .insert(tile.window_id, window_info(tile.window_id.as_isize()));
        }
        monitor.insert_workspace(key, workspace);
    }
    monitor.active_workspace = active;
    monitor.set_focus(WindowId::new(focus));
    engine.monitors.insert(output, monitor);
    if engine.monitors.focused_id().is_none() {
        engine.monitors.set_focused(output);
    }
    engine.set_output_name(output, name);
    engine.maintain_empty_workspace(output);
    output
}

fn ordered_fixture() -> (TilingEngine, BackendHandle, OutputId) {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let output = install_monitor(
        &mut engine,
        "lane-f-ordered",
        0,
        vec![
            (40, workspace("2", &[&[100], &[101]])),
            (7, workspace("middle", &[&[110, 111]])),
            (25, workspace("last", &[&[120]])),
        ],
        40,
        100,
    );
    (engine, BackendHandle::default_for_test(), output)
}

fn two_output_fixture() -> (TilingEngine, BackendHandle, OutputId, OutputId) {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let left = install_monitor(
        &mut engine,
        "lane-f-left",
        0,
        vec![
            (40, workspace("left", &[&[100], &[101]])),
            (7, workspace("left-spare", &[&[110]])),
        ],
        40,
        100,
    );
    let right = install_monitor(
        &mut engine,
        "lane-f-right",
        1200,
        vec![
            (40, workspace("right", &[&[200]])),
            (7, workspace("destination", &[&[210]])),
        ],
        40,
        200,
    );
    engine.monitors.set_focused(left);
    (engine, BackendHandle::default_for_test(), left, right)
}

fn scaled_height_fixture() -> (TilingEngine, BackendHandle, OutputId, OutputId) {
    let mut engine = TilingEngine::new(LayoutConfig {
        outer_gaps: (0, 0, 0, 0),
        column_gap: 0,
        window_gap: 0,
        border_width: 0,
        reserve_top: 0,
        reserve_bottom: 0,
        reserve_left: 0,
        reserve_right: 0,
        ..LayoutConfig::default()
    });
    let left = install_monitor(
        &mut engine,
        "height-scale-global",
        0,
        vec![(40, workspace("global", &[&[100]]))],
        40,
        100,
    );
    let right = install_monitor(
        &mut engine,
        "height-scale-target",
        1200,
        vec![
            (40, workspace("remote-active", &[&[210]])),
            (7, workspace("remote-inactive", &[&[200, 201]])),
        ],
        40,
        210,
    );
    let monitor = engine.monitors.get_mut(&right).unwrap();
    monitor.bounds = Rect::new(1200, 0, 1200, 900);
    monitor.work_area = monitor.bounds;
    monitor.scale_factor = 1.5;
    engine.monitors.set_focused(left);
    engine.record_ipc_foreground(WindowId::new(100));
    (engine, BackendHandle::default_for_test(), left, right)
}

fn scaled_height_tile_rect(engine: &TilingEngine, output: OutputId, window: isize) -> Rect {
    let cfg = engine.effective_config_for_workspace(output, 7);
    assert_eq!(cfg.border_inset(), 0, "tile and window sizes must coincide");
    sizing_tile_rect(engine, output, window)
}

fn sizing_tile_rect(engine: &TilingEngine, output: OutputId, window: isize) -> Rect {
    let monitor = engine.monitors.get(&output).unwrap();
    let cfg = engine.effective_config_for_workspace(output, 7);
    engine
        .calculate_positions(
            &monitor.workspaces[&7],
            TilingEngine::tiling_work_rect(monitor, cfg),
            cfg,
        )
        .into_iter()
        .find(|(id, _)| *id == WindowId::new(window))
        .unwrap()
        .1
}

#[test]
fn fixed_width_extremes_saturate_before_adding_tile_borders() {
    for (logical, expected) in [(i32::MAX, 100_000), (i32::MIN, 50), (160, 248)] {
        let mut col = column(&[200]);

        col.set_fixed_logical(logical as f64, 1.5, 8);

        assert_eq!(col.resolved_width(400), expected, "logical={logical}");
    }
}

#[test]
fn fixed_width_refresh_extremes_saturate_before_adding_tile_borders() {
    for (logical, expected) in [(i32::MAX, 100_000), (i32::MIN, 50), (160, 248)] {
        let (mut engine, _, left, right) = scaled_height_fixture();
        engine.config.border_width = 4;
        engine
            .monitors
            .get_mut(&right)
            .unwrap()
            .workspaces
            .get_mut(&7)
            .unwrap()
            .columns[0]
            .width_fixed_logical = Some(logical as f64);
        let before = (cursor(&engine, left), cursor(&engine, right));

        engine.refresh_strip_widths(right);

        assert_eq!(sizing_tile_rect(&engine, right, 200).size.w, expected);
        assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
    }
}

fn rule_border_sizing_fixture(
    off: Option<bool>,
    width: Option<u32>,
) -> (TilingEngine, BackendHandle, OutputId, OutputId) {
    let (mut engine, backend, left, right) = scaled_height_fixture();
    engine.config.border_width = 4;
    engine
        .tiled_windows
        .insert(WindowId::new(202), window_info(202));
    let ws = engine
        .monitors
        .get_mut(&right)
        .unwrap()
        .workspaces
        .get_mut(&7)
        .unwrap();
    ws.columns[0].set_width(Some(400));
    let mut sibling = column(&[202]);
    sibling.set_width(Some(400));
    ws.columns.insert(0, sibling);
    let mut resolved = crate::window::ResolvedWindowRules::default();
    resolved.effects.border.off = off;
    resolved.effects.border.width = width;
    engine.rule_state.insert(
        WindowId::new(201),
        rules::RuleState {
            process_name: None,
            aumid: None,
            resolved,
        },
    );
    (engine, backend, left, right)
}

#[test]
fn explicit_fixed_width_uses_the_target_windows_rule_border_at_high_dpi() {
    for (off, width, expected_tile_width) in [
        (Some(true), None, 240),
        (None, Some(12), 264),
        (None, None, 248),
    ] {
        let (mut engine, backend, left, right) = rule_border_sizing_fixture(off, width);
        let before = engine.ipc_snapshot();
        let cursors = (cursor(&engine, left), cursor(&engine, right));

        // 201 is not the column's selected tile (200), and its border differs.
        engine.set_window_width_for(WindowId::new(201), SizeChange::SetFixed(160), &backend);

        assert_eq!(
            sizing_tile_rect(&engine, right, 201).size.w,
            expected_tile_width
        );
        let after = engine.ipc_snapshot();
        assert_eq!(
            after
                .windows
                .iter()
                .find(|w| w.id == 201)
                .unwrap()
                .layout
                .window_size
                .0,
            160
        );
        for id in [100, 202, 210] {
            assert_eq!(
                after.windows.iter().find(|w| w.id == id).unwrap().layout,
                before.windows.iter().find(|w| w.id == id).unwrap().layout,
                "unrelated window {id}"
            );
        }
        assert_eq!((cursor(&engine, left), cursor(&engine, right)), cursors);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(after.focused_window(), Some(100));
    }
}

#[test]
fn fixed_width_target_survives_split_and_restore_without_affecting_its_old_column() {
    for expel in [false, true] {
        let (mut engine, backend, left, right) = rule_border_sizing_fixture(Some(true), None);
        let target = WindowId::new(201);
        engine.set_window_width_for(target, SizeChange::SetFixed(160), &backend);
        let cursors = (cursor(&engine, left), cursor(&engine, right));

        let ws = engine
            .monitors
            .get_mut(&right)
            .unwrap()
            .workspaces
            .get_mut(&7)
            .unwrap();
        if expel {
            assert!(ws.consume_or_expel_left(target).is_some());
        } else {
            assert!(ws.move_window(target, ws.columns.len(), 0));
        }
        engine.refresh_strip_widths(right);

        assert_eq!(sizing_tile_rect(&engine, right, 201).size.w, 240);
        assert_eq!(sizing_tile_rect(&engine, right, 200).size.w, 248);
        let ws = engine
            .monitors
            .get_mut(&right)
            .unwrap()
            .workspaces
            .get_mut(&7)
            .unwrap();
        let original = ws.find_window_column(WindowId::new(200)).unwrap();
        assert_eq!(ws.columns[original].width_fixed_window, None);
        let slot = ws.slot_of(target).unwrap();
        ws.remove_window(target).unwrap();
        ws.put_back(slot);
        engine.refresh_strip_widths(right);

        assert_eq!(sizing_tile_rect(&engine, right, 201).size.w, 240);
        assert_eq!(sizing_tile_rect(&engine, right, 200).size.w, 248);
        assert_eq!((cursor(&engine, left), cursor(&engine, right)), cursors);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
    }
}

#[test]
fn explicit_fixed_height_uses_the_target_windows_rule_border_at_high_dpi() {
    for (off, width, expected_tile_height) in [
        (Some(true), None, 240),
        (None, Some(12), 264),
        (None, None, 248),
    ] {
        let (mut engine, backend, left, right) = rule_border_sizing_fixture(off, width);
        let before = engine.ipc_snapshot();
        let cursors = (cursor(&engine, left), cursor(&engine, right));

        engine.set_window_height_for(WindowId::new(201), SizeChange::SetFixed(160), &backend);

        assert_eq!(
            sizing_tile_rect(&engine, right, 201).size.h,
            expected_tile_height
        );
        let after = engine.ipc_snapshot();
        assert_eq!(
            after
                .windows
                .iter()
                .find(|w| w.id == 201)
                .unwrap()
                .layout
                .window_size
                .1,
            160
        );
        for id in [100, 202, 210] {
            assert_eq!(
                after.windows.iter().find(|w| w.id == id).unwrap().layout,
                before.windows.iter().find(|w| w.id == id).unwrap().layout,
                "unrelated window {id}"
            );
        }
        assert_eq!((cursor(&engine, left), cursor(&engine, right)), cursors);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(after.focused_window(), Some(100));
    }
}

#[test]
fn fixed_column_width_control_scales_logical_pixels() {
    let (mut engine, backend, _, right) = scaled_height_fixture();
    engine.monitors.set_focused(right);
    let monitor = engine.monitors.get_mut(&right).unwrap();
    monitor.switch_workspace(7);
    monitor.set_focus(WindowId::new(200));

    engine.set_column_width_by(SizeChange::SetFixed(160), &backend);

    assert_eq!(scaled_height_tile_rect(&engine, right, 200).size.w, 240);
}

#[test]
fn floating_window_width_scales_fixed_changes_on_the_target_output() {
    for (change, width) in [
        (SizeChange::SetFixed(160), 240),
        (SizeChange::AdjustFixed(20), 430),
    ] {
        let (mut engine, backend, left, right) = scaled_height_fixture();
        let window = WindowId::new(300);
        engine.tiled_windows.insert(window, window_info(300));
        engine.register_float(window, right, 7, Rect::new(1400, 50, 400, 450));
        let before = (cursor(&engine, left), cursor(&engine, right));

        engine.float_apply_size_change(window, SizeAxis::Width, change, &backend);

        assert_eq!(
            engine.float_homes[&window].rect,
            Rect::new(1400, 50, width, 450)
        );
        assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
    }
}

#[test]
fn proportional_window_heights_use_the_physical_view_once_at_high_dpi() {
    for floating in [false, true] {
        for (change, height) in [
            (SizeChange::SetProportion(50_000), 450),
            (SizeChange::AdjustProportion(10_000), 540),
        ] {
            let (mut engine, backend, left, right) = scaled_height_fixture();
            let target = if floating {
                let window = WindowId::new(300);
                engine.tiled_windows.insert(window, window_info(300));
                engine.register_float(window, right, 7, Rect::new(1400, 50, 400, 450));
                window
            } else {
                WindowId::new(200)
            };
            let before = (cursor(&engine, left), cursor(&engine, right));

            engine.set_window_height_for(target, change, &backend);

            let actual = if floating {
                engine.float_homes[&target].rect.size.h
            } else {
                scaled_height_tile_rect(&engine, right, 200).size.h
            };
            assert_eq!(actual, height, "floating={floating}, change={change:?}");
            assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
            assert_eq!(engine.monitors.focused_id(), Some(left));
            assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
        }
    }
}

#[test]
fn fixed_tiled_window_height_scales_logical_pixels_on_inactive_output_target() {
    let (mut engine, backend, left, right) = scaled_height_fixture();
    let before = (cursor(&engine, left), cursor(&engine, right));

    engine.set_window_height_for(WindowId::new(200), SizeChange::SetFixed(160), &backend);

    assert_eq!(scaled_height_tile_rect(&engine, right, 200).size.h, 240);
    assert_eq!(scaled_height_tile_rect(&engine, right, 201).size.h, 660);
    assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
}

#[test]
fn adjusted_tiled_window_height_scales_logical_delta_on_inactive_output_target() {
    let (mut engine, backend, left, right) = scaled_height_fixture();
    let before = (cursor(&engine, left), cursor(&engine, right));
    assert_eq!(scaled_height_tile_rect(&engine, right, 200).size.h, 450);

    engine.set_window_height_for(WindowId::new(200), SizeChange::AdjustFixed(20), &backend);

    assert_eq!(scaled_height_tile_rect(&engine, right, 200).size.h, 480);
    assert_eq!(scaled_height_tile_rect(&engine, right, 201).size.h, 420);
    assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
}

#[test]
fn fixed_floating_window_height_scales_logical_pixels_on_inactive_output_target() {
    let (mut engine, backend, left, right) = scaled_height_fixture();
    let window = WindowId::new(300);
    engine.tiled_windows.insert(window, window_info(300));
    engine.register_float(window, right, 7, Rect::new(1400, 50, 400, 450));
    let before = (cursor(&engine, left), cursor(&engine, right));

    engine.set_window_height_for(window, SizeChange::SetFixed(160), &backend);

    assert_eq!(
        engine.float_homes[&window].rect,
        Rect::new(1400, 50, 400, 240)
    );
    assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
}

#[test]
fn adjusted_floating_window_height_scales_logical_delta_on_inactive_output_target() {
    let (mut engine, backend, left, right) = scaled_height_fixture();
    let window = WindowId::new(300);
    engine.tiled_windows.insert(window, window_info(300));
    engine.register_float(window, right, 7, Rect::new(1400, 50, 400, 450));
    let before = (cursor(&engine, left), cursor(&engine, right));

    engine.set_window_height_for(window, SizeChange::AdjustFixed(20), &backend);

    assert_eq!(
        engine.float_homes[&window].rect,
        Rect::new(1400, 50, 400, 480)
    );
    assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
}

fn cursor(engine: &TilingEngine, output: OutputId) -> (i32, Option<usize>, Option<WindowId>) {
    let monitor = engine.monitors.get(&output).unwrap();
    (
        monitor.active_workspace,
        monitor.focus_column,
        monitor.focus_window,
    )
}

fn transfer_view_fixture(stacked_source: bool) -> (TilingEngine, BackendHandle, OutputId) {
    // Pinned niri keeps view_offset relative to its selected column. Removing a
    // column to its left therefore preserves that column's visible position.
    // Wiri stores absolute strip scroll instead: 1182 must lose the removed span.
    let mut engine = TilingEngine::new(LayoutConfig {
        column_width: 500,
        column_gap: 8,
        window_gap: 8,
        outer_gaps: (8, 8, 8, 8),
        center_focused_column: CenterFocusedColumn::Never,
        ..LayoutConfig::default()
    });
    engine.update_animation_settings(false, 0, crate::layout::Easing::None);
    let moving: &[isize] = if stacked_source { &[100, 101] } else { &[100] };
    let output = install_monitor(
        &mut engine,
        "transfer-view-source",
        0,
        vec![
            (
                40,
                workspace(
                    "view-source",
                    &[&[90], &[91], moving, &[200], &[300], &[400]],
                ),
            ),
            (7, workspace("view-destination", &[&[500]])),
            (25, workspace("view-foreground", &[&[600]])),
        ],
        40,
        300,
    );
    let monitor = engine.monitors.get_mut(&output).unwrap();
    monitor.bounds = Rect::new(0, 0, 1366, 768);
    monitor.work_area = monitor.bounds;
    monitor.workspaces.get_mut(&40).unwrap().scroll_offset.x = 1182;
    engine.record_ipc_foreground(WindowId::new(300));
    (engine, BackendHandle::default_for_test(), output)
}

/// The native visible rectangle requested by the ordinary layout path, before
/// parking/animation. Synthetic HWNDs cannot prove a successful native apply;
/// assert this calculated allocation independently of the applied-state cache.
fn transfer_view_native_rect(
    engine: &TilingEngine,
    output: OutputId,
    workspace: i32,
    window: WindowId,
) -> Rect {
    let monitor = engine.monitors.get(&output).unwrap();
    let cfg = engine.effective_config_for_workspace(output, workspace);
    let tile = engine
        .calculate_positions(
            &monitor.workspaces[&workspace],
            TilingEngine::tiling_work_rect(monitor, cfg),
            cfg,
        )
        .into_iter()
        .find(|(id, _)| *id == window)
        .unwrap()
        .1;
    let inset = engine.rule_border_inset(window, cfg);
    Rect::new(
        tile.loc.x + inset,
        tile.loc.y + inset,
        tile.size.w.saturating_sub(2 * inset as u32),
        tile.size.h.saturating_sub(2 * inset as u32),
    )
}

#[test]
fn parity_transfer_view_anchor_singleton_id_preserves_selected_native_allocation() {
    for removed_width in [500, 420] {
        let (mut engine, backend, output) = transfer_view_fixture(false);
        engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspaces
            .get_mut(&40)
            .unwrap()
            .columns[2]
            .width = Some(removed_width);
        let selected = WindowId::new(300);
        let before = transfer_view_native_rect(&engine, output, 40, selected);
        let sibling = transfer_view_native_rect(&engine, output, 40, WindowId::new(200));
        let mut applied = AppliedState::unset();
        applied.rect = before;
        engine.applied_state.insert(selected, applied);
        let destination = engine.monitors.get(&output).unwrap().workspaces[&7].ipc_id;

        engine.move_window_to_workspace_ref(
            Some(WindowId::new(100)),
            WorkspaceRef::Id(destination),
            false,
            &backend,
        );

        assert_tile_at(&engine, 100, output, 7);
        assert_eq!(
            transfer_view_native_rect(&engine, output, 40, selected),
            before
        );
        assert_eq!(
            transfer_view_native_rect(&engine, output, 40, WindowId::new(200)),
            sibling
        );
        assert_eq!(
            engine.monitors.get(&output).unwrap().workspaces[&40]
                .scroll_offset
                .x,
            1182 - removed_width as i32 - 8
        );
        assert_eq!(cursor(&engine, output), (40, Some(3), Some(selected)));
        assert_eq!(engine.monitors.focused_id(), Some(output));
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(300));
        assert_eq!(
            engine.applied_state[&selected].rect, before,
            "cache remains consistent; calculated allocation above is the geometry oracle"
        );
    }
}

#[test]
fn parity_transfer_view_anchor_whole_column_detach_preserves_selected_allocation() {
    let (mut engine, backend, output) = transfer_view_fixture(true);
    let before = transfer_view_native_rect(&engine, output, 40, WindowId::new(300));
    // The internal detach contract also accepts a source column independent of
    // selection. This does not invent a typed API for explicitly naming columns.
    engine.transfer_workspace_column((output, 40, 2), (output, 7), false, &backend);

    assert_tile_at(&engine, 100, output, 7);
    assert_tile_at(&engine, 101, output, 7);
    assert_eq!(
        transfer_view_native_rect(&engine, output, 40, WindowId::new(300)),
        before
    );
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspaces[&40]
            .scroll_offset
            .x,
        674
    );
    assert_eq!(
        cursor(&engine, output),
        (40, Some(3), Some(WindowId::new(300)))
    );
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(300));
}

#[test]
fn parity_transfer_view_anchor_right_removal_and_stack_extraction_do_not_scroll() {
    for (stacked, moving) in [(false, 400), (true, 100)] {
        let (mut engine, backend, output) = transfer_view_fixture(stacked);
        let before = transfer_view_native_rect(&engine, output, 40, WindowId::new(300));
        let destination = engine.monitors.get(&output).unwrap().workspaces[&7].ipc_id;

        engine.move_window_to_workspace_ref(
            Some(WindowId::new(moving)),
            WorkspaceRef::Id(destination),
            false,
            &backend,
        );

        assert_tile_at(&engine, moving, output, 7);
        assert_eq!(
            transfer_view_native_rect(&engine, output, 40, WindowId::new(300)),
            before
        );
        assert_eq!(
            engine.monitors.get(&output).unwrap().workspaces[&40]
                .scroll_offset
                .x,
            1182
        );
        assert_eq!(
            cursor(&engine, output),
            (40, Some(4), Some(WindowId::new(300)))
        );
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(300));
    }
}

fn transfer_reflow_fixture(
    source_columns: &[&[isize]],
    mode: ColumnWidthMode,
) -> (TilingEngine, BackendHandle, OutputId) {
    let mut config = LayoutConfig {
        column_width_mode: mode,
        column_width: 500,
        column_gap: 8,
        window_gap: 8,
        outer_gaps: (8, 8, 8, 8),
        border_width: 0,
        max_visible_columns: 3,
        center_focused_column: CenterFocusedColumn::Never,
        ..LayoutConfig::default()
    };
    // Direct LayoutConfig fixtures must keep the resolved border-width cache in
    // sync with the effects flag, as LayoutConfig::from_config does in production.
    config.effects.border.off = true;
    let mut engine = TilingEngine::new(config);
    engine.update_animation_settings(false, 0, crate::layout::Easing::None);
    let output = install_monitor(
        &mut engine,
        "transfer-reflow-source",
        0,
        vec![
            (40, workspace("reflow-source", source_columns)),
            (7, workspace("reflow-destination", &[&[500]])),
        ],
        40,
        300,
    );
    let monitor = engine.monitors.get_mut(&output).unwrap();
    monitor.bounds = Rect::new(0, 0, 1366, 768);
    monitor.work_area = monitor.bounds;
    engine.record_ipc_foreground(WindowId::new(300));
    (engine, BackendHandle::default_for_test(), output)
}

#[test]
fn parity_transfer_view_anchor_proportional_right_removal_preserves_selected_origin() {
    let (mut engine, backend, output) =
        transfer_reflow_fixture(&[&[100], &[300], &[400]], ColumnWidthMode::Proportional);
    let selected = WindowId::new(300);
    let cfg = engine.effective_config_for_workspace(output, 40);
    let monitor = engine.monitors.get(&output).unwrap();
    let work = TilingEngine::tiling_work_rect(monitor, cfg);
    assert_eq!(work, Rect::new(8, 8, 1350, 752));
    assert_eq!(cfg.column_gap, 8);
    assert_eq!(cfg.border_inset(), 0);
    assert_eq!(engine.rule_border_inset(selected, cfg), 0);
    assert_eq!(
        transfer_view_native_rect(&engine, output, 40, WindowId::new(100))
            .size
            .w,
        444
    );
    let before = transfer_view_native_rect(&engine, output, 40, selected);
    // Work origin 8 + preceding 444px column + one 8px gap, without borders.
    assert_eq!(before.loc.x, 460);
    assert_eq!(before.size.w, 444);
    let destination = engine.monitors.get(&output).unwrap().workspaces[&7].ipc_id;

    engine.move_window_to_workspace_ref(
        Some(WindowId::new(400)),
        WorkspaceRef::Id(destination),
        false,
        &backend,
    );

    assert_tile_at(&engine, 400, output, 7);
    let after = transfer_view_native_rect(&engine, output, 40, selected);
    // The surviving proportional columns grow from 444 to 671px. Their width
    // changes, but niri's view remains relative to the selected column's origin.
    assert_eq!(after.size.w, 671);
    assert_eq!(after.loc, before.loc);
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspaces[&40]
            .scroll_offset
            .x,
        227
    );
    assert_eq!(cursor(&engine, output), (40, Some(1), Some(selected)));
    assert_eq!(engine.monitors.focused_id(), Some(output));
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(300));
}

#[test]
fn parity_transfer_view_anchor_stack_minimum_extraction_preserves_selected_origin() {
    let (mut engine, backend, output) =
        transfer_reflow_fixture(&[&[100, 101], &[300]], ColumnWidthMode::Fixed);
    engine.note_window_min_width(WindowId::new(100), 700);
    let selected = WindowId::new(300);
    let cfg = engine.effective_config_for_workspace(output, 40);
    let monitor = engine.monitors.get(&output).unwrap();
    assert_eq!(
        TilingEngine::tiling_work_rect(monitor, cfg),
        Rect::new(8, 8, 1350, 752)
    );
    assert_eq!(cfg.column_gap, 8);
    assert_eq!(cfg.border_inset(), 0);
    assert_eq!(engine.rule_border_inset(selected, cfg), 0);
    assert_eq!(monitor.workspaces[&40].columns[0].min_width(), 700);
    let before = transfer_view_native_rect(&engine, output, 40, selected);
    // Work origin 8 + the 700px tile minimum + one 8px gap.
    assert_eq!(before.loc.x, 716);
    assert_eq!(before.size.w, 500);
    let destination = engine.monitors.get(&output).unwrap().workspaces[&7].ipc_id;

    engine.move_window_to_workspace_ref(
        Some(WindowId::new(100)),
        WorkspaceRef::Id(destination),
        false,
        &backend,
    );

    assert_tile_at(&engine, 100, output, 7);
    assert_tile_at(&engine, 101, output, 40);
    assert_eq!(
        transfer_view_native_rect(&engine, output, 40, WindowId::new(101))
            .size
            .w,
        500,
        "the remaining stack loses the extracted window's dominant minimum"
    );
    assert_eq!(
        transfer_view_native_rect(&engine, output, 40, selected),
        before
    );
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspaces[&40]
            .scroll_offset
            .x,
        -200,
        "signed absolute scroll preserves the selected origin after the 200px shrink"
    );
    assert_eq!(cursor(&engine, output), (40, Some(1), Some(selected)));
    assert_eq!(engine.monitors.focused_id(), Some(output));
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(300));
}

#[test]
fn parity_transfer_view_anchor_inactive_source_preserves_its_remembered_view() {
    let (mut engine, backend, output) = transfer_view_fixture(false);
    {
        let monitor = engine.monitors.get_mut(&output).unwrap();
        monitor.switch_workspace(25);
        monitor.set_focus(WindowId::new(600));
    }
    engine.record_ipc_foreground(WindowId::new(600));
    engine.animation =
        crate::layout::AnimationManager::new(true, 1000, crate::layout::Easing::Linear);
    engine
        .animation
        .animate(crate::layout::AnimationTarget::ScrollX(output), 0.0, 100.0);
    engine.tick_animations(400);
    let before = transfer_view_native_rect(&engine, output, 40, WindowId::new(300));
    let foreground = transfer_view_native_rect(&engine, output, 25, WindowId::new(600));
    let destination = engine.monitors.get(&output).unwrap().workspaces[&7].ipc_id;

    engine.move_window_to_workspace_ref(
        Some(WindowId::new(100)),
        WorkspaceRef::Id(destination),
        false,
        &backend,
    );

    assert_eq!(
        transfer_view_native_rect(&engine, output, 40, WindowId::new(300)),
        before
    );
    assert_eq!(
        transfer_view_native_rect(&engine, output, 25, WindowId::new(600)),
        foreground
    );
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspaces[&40].remembered_focus(),
        Some((3, WindowId::new(300)))
    );
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspaces[&40]
            .scroll_offset
            .x,
        674
    );
    assert_eq!(
        cursor(&engine, output),
        (25, Some(0), Some(WindowId::new(600)))
    );
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(600));
    assert_eq!(
        engine
            .animation
            .get_value(&crate::layout::AnimationTarget::ScrollX(output)),
        Some(40.0),
        "the foreground workspace's camera must not inherit a hidden source's rebase"
    );
    engine.tick_animations(600);
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspaces[&25]
            .scroll_offset
            .x,
        100
    );
}

#[test]
fn parity_transfer_view_anchor_pending_absolute_scroll_keeps_its_relative_trajectory() {
    let (mut engine, backend, output) = transfer_view_fixture(false);
    engine.animation =
        crate::layout::AnimationManager::new(true, 1000, crate::layout::Easing::Linear);
    engine.animation.animate(
        crate::layout::AnimationTarget::ScrollX(output),
        1182.0,
        1282.0,
    );
    engine.animation.animate(
        crate::layout::AnimationTarget::WorkspaceX(output),
        83.0,
        0.0,
    );
    engine.tick_animations(400);
    let slide_before = engine
        .animation
        .get_value(&crate::layout::AnimationTarget::WorkspaceX(output));
    let before = transfer_view_native_rect(&engine, output, 40, WindowId::new(300));
    let destination = engine.monitors.get(&output).unwrap().workspaces[&7].ipc_id;

    engine.move_window_to_workspace_ref(
        Some(WindowId::new(100)),
        WorkspaceRef::Id(destination),
        false,
        &backend,
    );

    assert_eq!(
        transfer_view_native_rect(&engine, output, 40, WindowId::new(300)),
        before
    );
    assert_eq!(
        engine
            .animation
            .get_value(&crate::layout::AnimationTarget::WorkspaceX(output)),
        slide_before,
        "the additive workspace-slide camera is independent of absolute strip scroll"
    );
    engine.tick_animations(100);
    assert_eq!(
        transfer_view_native_rect(&engine, output, 40, WindowId::new(300))
            .loc
            .x,
        before.loc.x - 10,
        "rebasing must retain the timeline's elapsed 400ms"
    );
    engine.tick_animations(500);
    let terminal = transfer_view_native_rect(&engine, output, 40, WindowId::new(300));
    assert_eq!(
        terminal.loc.x,
        before.loc.x - 60,
        "the original 100px scroll must not resurrect the removed column's span"
    );
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspaces[&40]
            .scroll_offset
            .x,
        774
    );
    assert_eq!(
        cursor(&engine, output),
        (40, Some(3), Some(WindowId::new(300)))
    );
    assert_eq!(engine.ipc_snapshot().focused_window(), Some(300));
}

fn ipc_id_fixture() -> (TilingEngine, BackendHandle, OutputId, OutputId, u64) {
    let (mut engine, backend, left, right) = two_output_fixture();
    for (output, base) in [(left, u64::MAX - 100), (right, u64::MAX - 200)] {
        let monitor = engine.monitors.get_mut(&output).unwrap();
        for (position, key) in monitor.workspace_ids().into_iter().enumerate() {
            monitor.workspaces.get_mut(&key).unwrap().ipc_id = base - position as u64;
        }
    }
    let id = u64::MAX - 17;
    engine
        .monitors
        .get_mut(&right)
        .unwrap()
        .workspaces
        .get_mut(&7)
        .unwrap()
        .ipc_id = id;
    // Coercing Id into a name would select a different object on the left.
    engine
        .monitors
        .get_mut(&left)
        .unwrap()
        .workspaces
        .get_mut(&7)
        .unwrap()
        .name = Some(id.to_string());
    (engine, backend, left, right, id)
}

fn columns_on(engine: &TilingEngine, output: OutputId, key: i32) -> Vec<Vec<isize>> {
    engine.monitors.get(&output).unwrap().workspaces[&key]
        .columns
        .iter()
        .map(|column| {
            column
                .tiles
                .iter()
                .map(|tile| tile.window_id.as_isize())
                .collect()
        })
        .collect()
}

fn assert_tile_at(engine: &TilingEngine, id: isize, output: OutputId, key: i32) {
    let window = WindowId::new(id);
    let mut locations = Vec::new();
    for (output, monitor) in engine.monitors.iter() {
        for key in monitor.workspace_ids() {
            for tile in monitor.workspaces[&key]
                .columns
                .iter()
                .flat_map(|column| &column.tiles)
            {
                if tile.window_id == window {
                    locations.push((*output, key));
                }
            }
        }
    }
    assert_eq!(
        locations,
        vec![(output, key)],
        "one tile, on the intended workspace"
    );
    assert_eq!(
        engine.locate_window(window).map(|(o, w, _)| (o, w)),
        Some((output, key))
    );
    assert!(engine.tiled_windows.contains_key(&window));
    assert!(!engine.floating_windows.contains(&window));
}

fn assert_trailing_empty(engine: &TilingEngine, output: OutputId) -> i32 {
    let monitor = engine.monitors.get(&output).unwrap();
    let ids = monitor.workspace_ids();
    let trailing = *ids.last().unwrap();
    let empty: Vec<i32> = ids
        .iter()
        .copied()
        .filter(|key| {
            let workspace = &monitor.workspaces[key];
            workspace.columns.is_empty() && workspace.name.is_none()
        })
        .collect();
    assert_eq!(
        empty,
        vec![trailing],
        "exactly one unnamed empty workspace, at the end"
    );
    trailing
}

#[test]
fn indices_and_numeric_names_are_distinct() {
    let (mut engine, backend, output) = ordered_fixture();
    assert_eq!(
        &engine.monitors.get(&output).unwrap().workspace_ids()[..3],
        &[40, 7, 25]
    );
    let before = cursor(&engine, output);

    engine.move_window_to_workspace_ref(
        Some(WindowId::new(101)),
        WorkspaceRef::Index(2),
        false,
        &backend,
    );
    assert_tile_at(&engine, 101, output, 7);
    assert_eq!(cursor(&engine, output), before);

    // The name "2" denotes the first workspace, not the second workspace's index.
    engine.move_window_to_workspace_ref(
        Some(WindowId::new(101)),
        WorkspaceRef::Name("2".to_string()),
        false,
        &backend,
    );
    assert_tile_at(&engine, 101, output, 40);
    assert_eq!(cursor(&engine, output), before);
    assert_eq!(engine.workspace_name_on(output, 40).as_deref(), Some("2"));
    assert_eq!(
        engine.workspace_name_on(output, 7).as_deref(),
        Some("middle")
    );
}

#[test]
fn workspace_ipc_id_is_global_and_survives_rename_and_reorder() {
    let (mut engine, backend, left, right, id) = ipc_id_fixture();
    let reference = WorkspaceRef::Id(id);
    let before = (cursor(&engine, left), cursor(&engine, right));
    assert_eq!(
        engine.resolve_workspace_reference(left, &reference),
        Some((right, 7))
    );
    assert_eq!(
        engine.resolve_workspace_reference(left, &WorkspaceRef::Index(2)),
        Some((left, 7))
    );
    assert_eq!(
        engine.resolve_workspace_reference(left, &WorkspaceRef::Name(id.to_string())),
        Some((left, 7))
    );
    engine.set_workspace_name("renamed-by-id", Some(reference.clone()), &backend);
    engine.move_workspace_to_index(1, Some(reference.clone()), &backend);
    engine.unset_workspace_name(Some(reference.clone()), &backend);
    assert_eq!(engine.monitors.get(&right).unwrap().workspace_ids()[0], 7);
    assert_eq!(
        engine.resolve_workspace_reference(left, &reference),
        Some((right, 7))
    );
    assert_eq!(
        engine.monitors.get(&right).unwrap().workspaces[&7].ipc_id,
        id
    );
    assert_eq!(engine.workspace_name_on(right, 7), None);
    assert_eq!(engine.workspace_name_on(left, 7), Some(id.to_string()));
    assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
    let snapshot = engine.ipc_snapshot();
    let target = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.id == id)
        .unwrap();
    assert_eq!(target.idx, 1);
    assert_eq!(target.name, None);
    assert_eq!(
        snapshot
            .windows
            .iter()
            .find(|window| window.id == 210)
            .unwrap()
            .workspace_id,
        Some(id)
    );

    engine.workspace_layout_action(
        &crate::input::WorkspaceAction::Focus { reference },
        &backend,
    );
    assert_eq!(engine.monitors.focused_id(), Some(right));
    assert_eq!(cursor(&engine, right).0, 7);
    assert_eq!(cursor(&engine, right).2, Some(WindowId::new(210)));
}

#[test]
fn workspace_ipc_id_transfers_use_the_remote_object_without_following_focus() {
    for focus in [false, true] {
        let (mut engine, backend, left, right, id) = ipc_id_fixture();
        let before = (cursor(&engine, left), cursor(&engine, right));
        // The explicitly targeted window is not focused, so even focus=true
        // must leave both outputs' active workspace and selection alone.
        engine.move_window_to_workspace_ref(
            Some(WindowId::new(101)),
            WorkspaceRef::Id(id),
            focus,
            &backend,
        );
        assert_tile_at(&engine, 101, right, 7);
        assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(
            engine
                .ipc_snapshot()
                .windows
                .iter()
                .find(|window| window.id == 101)
                .unwrap()
                .workspace_id,
            Some(id)
        );
    }
    let (mut engine, backend, left, right, id) = ipc_id_fixture();
    let right_before = cursor(&engine, right);
    engine.move_column_to_workspace_ref(WorkspaceRef::Id(id), false, &backend);
    assert_tile_at(&engine, 100, right, 7);
    assert_tile_at(&engine, 101, left, 40);
    assert_eq!(cursor(&engine, left).0, 40);
    assert_eq!(cursor(&engine, right), right_before);
    assert_eq!(engine.monitors.focused_id(), Some(left));
}

#[test]
fn workspace_ipc_id_follows_relocation_after_its_old_local_key_is_reused() {
    let (mut engine, backend, left, right, id) = ipc_id_fixture();
    let reference = WorkspaceRef::Id(id);
    engine.move_workspace_to_monitor_action(
        Some(reference.clone()),
        &crate::input::MonitorTarget::Name("lane-f-left".into()),
        &backend,
    );
    let (output, key) = engine
        .resolve_workspace_reference(right, &reference)
        .unwrap();
    assert_eq!(output, left);
    assert_ne!(key, 7, "the destination already owns this local key");
    let mut replacement = workspace("replacement", &[&[220]]);
    replacement.ipc_id = u64::MAX - 300;
    engine
        .tiled_windows
        .insert(WindowId::new(220), window_info(220));
    engine
        .monitors
        .get_mut(&right)
        .unwrap()
        .insert_workspace(7, replacement);
    engine.maintain_empty_workspace(right);
    engine.set_workspace_name("followed-the-object", Some(reference.clone()), &backend);
    engine.move_workspace_to_index(1, Some(reference.clone()), &backend);
    assert_eq!(
        engine.workspace_name_on(left, key).as_deref(),
        Some("followed-the-object")
    );
    assert_eq!(
        engine.workspace_name_on(right, 7).as_deref(),
        Some("replacement")
    );
    assert_tile_at(&engine, 210, left, key);
    engine.focus_workspace_ref(&reference, &backend);
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(cursor(&engine, left).0, key);
    engine.move_workspace_to_monitor_action(
        Some(reference.clone()),
        &crate::input::MonitorTarget::Name("lane-f-right".into()),
        &backend,
    );
    let (output, moved_key) = engine
        .resolve_workspace_reference(left, &reference)
        .unwrap();
    assert_eq!(output, right);
    assert_ne!(moved_key, 7);
    assert_eq!(
        engine.monitors.get(&right).unwrap().workspaces[&moved_key].ipc_id,
        id
    );
    assert_tile_at(&engine, 210, right, moved_key);
    assert_tile_at(&engine, 220, right, 7);
    engine.move_window_to_workspace_ref(Some(WindowId::new(101)), reference, false, &backend);
    assert_tile_at(&engine, 101, right, moved_key);
}

#[test]
fn unknown_workspace_ipc_id_never_falls_back_to_a_local_key_or_focused_workspace() {
    let (mut engine, backend, left, right, _) = ipc_id_fixture();
    // Local key 7 exists on both outputs, but no workspace has IPC identity 7.
    let reference = WorkspaceRef::Id(7);
    let before = engine.ipc_snapshot();
    let cursors = (cursor(&engine, left), cursor(&engine, right));
    for action in [
        crate::input::WorkspaceAction::Focus {
            reference: reference.clone(),
        },
        crate::input::WorkspaceAction::MoveWindow {
            window: Some(WindowId::new(101)),
            reference: reference.clone(),
            focus: true,
        },
        crate::input::WorkspaceAction::MoveColumn {
            reference: reference.clone(),
            focus: true,
        },
        crate::input::WorkspaceAction::MoveToIndex {
            index: 1,
            workspace: Some(reference.clone()),
        },
        crate::input::WorkspaceAction::SetName {
            name: "must-not-rename".into(),
            workspace: Some(reference.clone()),
        },
        crate::input::WorkspaceAction::UnsetName {
            workspace: Some(reference.clone()),
        },
    ] {
        engine.workspace_layout_action(&action, &backend);
        assert_eq!(engine.ipc_snapshot(), before, "{action:?}");
    }
    engine.move_workspace_to_monitor_action(
        Some(reference),
        &crate::input::MonitorTarget::Name("lane-f-right".into()),
        &backend,
    );
    assert_eq!(engine.ipc_snapshot(), before);
    assert_eq!((cursor(&engine, left), cursor(&engine, right)), cursors);
    assert_eq!(engine.monitors.focused_id(), Some(left));
}

#[test]
fn explicit_height_targets_preserve_focus_across_outputs_and_inactive_workspaces() {
    for target_workspace in [40, 7] {
        for floating in [false, true] {
            let (mut engine, backend, left, right) = two_output_fixture();
            engine.record_ipc_foreground(WindowId::new(100));
            let target = if floating {
                let window = WindowId::new(300);
                engine.tiled_windows.insert(window, window_info(300));
                engine.register_float(
                    window,
                    right,
                    target_workspace,
                    Rect::new(1400, 50, 400, 500),
                );
                window
            } else {
                let (target, other) = if target_workspace == 40 {
                    (200, 201)
                } else {
                    (210, 211)
                };
                engine
                    .tiled_windows
                    .insert(WindowId::new(other), window_info(other));
                engine
                    .monitors
                    .get_mut(&right)
                    .unwrap()
                    .workspaces
                    .get_mut(&target_workspace)
                    .unwrap()
                    .columns[0] = column(&[target, other]);
                WindowId::new(target)
            };
            let before = (cursor(&engine, left), cursor(&engine, right));

            engine.set_window_height_for(target, SizeChange::SetFixed(300), &backend);

            if floating {
                assert_eq!(
                    engine.float_homes[&target].rect,
                    Rect::new(1400, 50, 400, 300)
                );
            } else {
                let tiles = &engine.monitors.get(&right).unwrap().workspaces[&target_workspace]
                    .columns[0]
                    .tiles;
                assert!(
                    tiles[0].height_weight < 1.0,
                    "300 px is less than half the column"
                );
                assert_eq!(tiles[1].height_weight, 1.0);
            }
            assert_eq!((cursor(&engine, left), cursor(&engine, right)), before);
            assert_eq!(engine.monitors.focused_id(), Some(left));
            assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
        }
    }
}

#[test]
fn explicit_index_transfer_activates_the_unfocused_outputs_local_selection() {
    for focus in [false, true] {
        for kind in ["index", "name", "id"] {
            let (mut engine, backend, left, right) = two_output_fixture();
            engine.record_ipc_foreground(WindowId::new(100));
            let left_before = cursor(&engine, left);
            let id = engine.monitors.get(&right).unwrap().workspaces[&7].ipc_id;
            let reference = match kind {
                "index" => WorkspaceRef::Index(2),
                "name" => WorkspaceRef::Name("destination".to_string()),
                _ => WorkspaceRef::Id(id),
            };

            // 200 is selected on the right output, while 100 has global focus.
            // The pinned Index path uses monitor-local smart activation; the
            // Name/Id path uses global smart activation even on the same output.
            engine.move_window_to_workspace_ref(
                Some(WindowId::new(200)),
                reference,
                focus,
                &backend,
            );

            let local_activation = kind == "index" && focus;
            assert_tile_at(&engine, 200, right, 7);
            assert_eq!(
                cursor(&engine, right).0,
                if local_activation { 7 } else { 40 },
                "{kind}, focus={focus}"
            );
            assert_eq!(
                cursor(&engine, right).2,
                local_activation.then_some(WindowId::new(200)),
                "{kind}, focus={focus}"
            );
            assert_eq!(cursor(&engine, left), left_before);
            assert_eq!(engine.monitors.focused_id(), Some(left));
            assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
        }
    }
}

#[test]
fn explicit_window_move_crosses_outputs_without_changing_focus() {
    for focus in [false, true] {
        let (mut engine, backend, left, right) = two_output_fixture();
        let left_before = cursor(&engine, left);
        let right_before = cursor(&engine, right);
        let destination_id = engine.monitors.get(&right).unwrap().workspaces[&7].ipc_id;

        // 101 is not focused; an explicit id must win over the focused tile 100.
        // niri's focus=true is smart activation: it only follows an already focused window.
        engine.move_window_to_workspace_ref(
            Some(WindowId::new(101)),
            WorkspaceRef::Name("destination".to_string()),
            focus,
            &backend,
        );

        assert_tile_at(&engine, 101, right, 7);
        assert_tile_at(&engine, 100, left, 40);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(cursor(&engine, left), left_before);
        assert_eq!(cursor(&engine, right), right_before);
        let snapshot = engine.ipc_snapshot();
        let moved = snapshot
            .windows
            .iter()
            .find(|window| window.id == 101)
            .unwrap();
        assert_eq!(moved.workspace_id, Some(destination_id));
        assert!(!moved.is_floating);

        // The same explicit id is now on an inactive workspace of the unfocused output.
        engine.move_window_to_workspace_ref(
            Some(WindowId::new(101)),
            WorkspaceRef::Name("left".to_string()),
            focus,
            &backend,
        );
        assert_tile_at(&engine, 101, left, 40);
        assert_tile_at(&engine, 100, left, 40);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(cursor(&engine, left), left_before);
        assert_eq!(cursor(&engine, right), right_before);
    }
}

#[test]
fn floating_window_move_crosses_outputs_without_becoming_tiled() {
    let (mut engine, backend, left, right) = two_output_fixture();
    let floating = WindowId::new(300);
    engine.tiled_windows.insert(floating, window_info(300));
    engine.register_float(floating, left, 7, Rect::new(80, 90, 400, 300));
    let left_before = cursor(&engine, left);
    let right_before = cursor(&engine, right);
    let destination_id = engine.monitors.get(&right).unwrap().workspaces[&7].ipc_id;

    engine.move_window_to_workspace_ref(
        Some(floating),
        WorkspaceRef::Name("destination".to_string()),
        false,
        &backend,
    );

    assert!(engine.floating_windows.contains(&floating));
    assert!(engine.tiled_windows.contains_key(&floating));
    let home = &engine.float_homes[&floating];
    assert_eq!((home.output, home.workspace), (right, 7));
    assert_eq!(
        engine.locate_window(floating),
        None,
        "a float must not acquire a tile"
    );
    // `install_monitor(right)` has its work area at x=1200, so the float follows
    // the destination display rather than staying at its source coordinates.
    assert_eq!(home.rect, Rect::new(1280, 90, 400, 300));
    assert_eq!(columns_on(&engine, right, 7), vec![vec![210]]);
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(cursor(&engine, left), left_before);
    assert_eq!(cursor(&engine, right), right_before);
    let snapshot = engine.ipc_snapshot();
    let moved = snapshot
        .windows
        .iter()
        .find(|window| window.id == 300)
        .unwrap();
    assert_eq!(moved.workspace_id, Some(destination_id));
    assert!(moved.is_floating);
    assert_eq!(moved.layout.pos_in_scrolling_layout, None);
}

#[test]
fn focus_workspace_named_keeps_the_destination_output() {
    let (mut engine, backend, left, right) = two_output_fixture();
    engine.set_focused_output(left);
    engine.focus_workspace_named("right", &backend);
    assert_eq!(engine.monitors.focused_id(), Some(right));
    assert_eq!(
        engine.monitors.get(&right).unwrap().active_workspace,
        40,
        "right's local key 40 is the named workspace"
    );
    assert_eq!(
        engine.monitors.get(&right).unwrap().focus_window,
        Some(WindowId::new(200))
    );
    assert_eq!(
        engine.monitors.get(&left).unwrap().active_workspace,
        40,
        "the source output is not switched to a same-numbered local key"
    );
}

#[test]
fn window_move_with_focus_selects_destination() {
    let (mut engine, backend, left, right) = two_output_fixture();
    assert!(engine.focus_action_window(WindowId::new(101), &backend));
    assert_eq!(cursor(&engine, left).2, Some(WindowId::new(101)));
    engine.move_window_to_workspace_ref(
        Some(WindowId::new(101)),
        WorkspaceRef::Name("destination".to_string()),
        true,
        &backend,
    );
    assert_tile_at(&engine, 101, right, 7);
    assert_tile_at(&engine, 100, left, 40);
    assert_eq!(engine.monitors.focused_id(), Some(right));
    let monitor = engine.monitors.get(&right).unwrap();
    assert_eq!(monitor.active_workspace, 7);
    assert_eq!(monitor.focus_window, Some(WindowId::new(101)));
    assert_eq!(
        monitor.focus_column,
        monitor
            .workspace()
            .unwrap()
            .find_window_column(WindowId::new(101))
    );
}

#[test]
fn transfer_keeps_inactive_workspace_selection_beyond_the_mru_ring() {
    let (mut engine, backend, output) = ordered_fixture();
    {
        let monitor = engine.monitors.get_mut(&output).unwrap();
        let ws = monitor.workspaces.get_mut(&7).unwrap();
        ws.columns[0].display = ColumnDisplay::Tabbed { active_tab: 0 };
        ws.remember_focus(WindowId::new(111));
        for id in 1000..1040 {
            monitor.focus_ring.push(WindowId::new(id));
        }
    }
    let before = cursor(&engine, output);

    engine.move_window_to_workspace_ref(
        Some(WindowId::new(101)),
        WorkspaceRef::Name("middle".to_string()),
        false,
        &backend,
    );

    assert_eq!(cursor(&engine, output), before);
    assert_eq!(
        columns_on(&engine, output, 7),
        vec![vec![110, 111], vec![101]]
    );
    let monitor = engine.monitors.get_mut(&output).unwrap();
    assert_eq!(
        monitor.workspaces[&7].remembered_focus(),
        Some((0, WindowId::new(111)))
    );
    assert_eq!(
        monitor.workspaces[&7].columns[0].active_window(),
        Some(WindowId::new(111))
    );
    monitor.switch_workspace(7);
    assert_eq!(monitor.focus_window, Some(WindowId::new(111)));
    assert_eq!(monitor.focus_column, Some(0));
}

#[test]
fn splitting_a_stack_preserves_width_intent_on_an_inactive_output_workspace() {
    for proportional in [false, true] {
        let (mut engine, backend, left, right) = two_output_fixture();
        let window = WindowId::new(102);
        engine.tiled_windows.insert(window, window_info(102));
        let source_cfg = engine.effective_config(left).clone();
        let source_view =
            TilingEngine::tiling_work_rect(engine.monitors.get(&left).unwrap(), &source_cfg)
                .size
                .w as i32;
        {
            let monitor = engine.monitors.get_mut(&left).unwrap();
            let column = &mut monitor.workspaces.get_mut(&40).unwrap().columns[0];
            column.tiles.push(Tile::new(window));
            if proportional {
                column.set_proportion(0.5, source_view, source_cfg.column_gap);
            } else {
                column.set_fixed_logical(320.0, 1.0, 2 * source_cfg.border_width.max(1));
            }
            column.preset_idx = Some(2);
        }
        let source_width = engine.monitors.get(&left).unwrap().workspaces[&40].columns[0].width;
        let destination_cfg = LayoutConfig {
            reserve_left: -30,
            reserve_right: 10,
            outer_gaps: (8, 20, 8, 10),
            column_gap: 24,
            ..engine.config.clone()
        };
        engine
            .config_per_monitor
            .insert(right, destination_cfg.clone());
        engine.monitors.get_mut(&right).unwrap().scale_factor = 1.5;
        let destination_view =
            TilingEngine::tiling_work_rect(engine.monitors.get(&right).unwrap(), &destination_cfg);
        assert_eq!(destination_view.size.w, 1190);
        let left_before = cursor(&engine, left);
        let right_before = cursor(&engine, right);

        engine.move_window_to_workspace_ref(
            Some(window),
            WorkspaceRef::Name("destination".to_string()),
            false,
            &backend,
        );

        assert_tile_at(&engine, 102, right, 7);
        assert_eq!(columns_on(&engine, left, 40), vec![vec![100], vec![101]]);
        assert_eq!(columns_on(&engine, right, 7), vec![vec![210], vec![102]]);
        let source = &engine.monitors.get(&left).unwrap().workspaces[&40].columns[0];
        let target = &engine.monitors.get(&right).unwrap().workspaces[&7].columns[1];
        assert_eq!(source.width, source_width);
        assert_eq!(target.width_proportion, source.width_proportion);
        assert_eq!(target.width_fixed_logical, source.width_fixed_logical);
        assert_eq!(target.preset_idx, source.preset_idx);
        assert_eq!(target.active_window(), Some(window));
        assert_eq!(target.width, Some(if proportional { 583 } else { 488 }));
        assert_eq!(
            engine.monitors.get(&right).unwrap().workspaces[&7].remembered_focus(),
            Some((0, WindowId::new(210)))
        );
        assert_eq!(cursor(&engine, left), left_before);
        assert_eq!(cursor(&engine, right), right_before);
        assert_eq!(engine.monitors.focused_id(), Some(left));
    }
}

#[test]
fn relative_window_move_uses_explicit_windows_workspace_order() {
    let (mut engine, backend, output) = ordered_fixture();
    let before = cursor(&engine, output);
    let window = Some(WindowId::new(111));

    // Focus is on key 40, but the explicit window lives on key 7: down goes to 25.
    engine.move_window_to_workspace_relative(window, true, false, &backend);
    assert_tile_at(&engine, 111, output, 25);
    assert_eq!(cursor(&engine, output), before);
    engine.move_window_to_workspace_relative(window, false, false, &backend);
    assert_tile_at(&engine, 111, output, 7);
    engine.move_window_to_workspace_relative(window, false, false, &backend);
    assert_tile_at(&engine, 111, output, 40);
    let columns_before_edge = columns_on(&engine, output, 40);
    engine.move_window_to_workspace_relative(window, false, false, &backend);
    assert_eq!(
        columns_on(&engine, output, 40),
        columns_before_edge,
        "up at the first workspace is a no-op"
    );
    assert_eq!(cursor(&engine, output), before);
}

#[test]
fn relative_workspace_top_and_bottom_are_noops_before_tile_detach() {
    for down in [false, true] {
        for focus in [false, true] {
            for whole_column in [false, true] {
                let (mut engine, backend, output) = ordered_fixture();
                let trailing = assert_trailing_empty(&engine, output);
                let (key, selected) = if down { (25, 121) } else { (40, 102) };
                engine
                    .tiled_windows
                    .insert(WindowId::new(selected), window_info(selected));
                {
                    let monitor = engine.monitors.get_mut(&output).unwrap();
                    if down {
                        // A populated last workspace is a transient state before tail
                        // maintenance. Remove the old empty tail to test the actual
                        // bottom, not a valid move from the last occupied workspace.
                        monitor.remove_workspace(trailing);
                    }
                    let ws = monitor.workspaces.get_mut(&key).unwrap();
                    let column = &mut ws.columns[0];
                    column.tiles.push(Tile::new(WindowId::new(selected)));
                    column.display = ColumnDisplay::Tabbed { active_tab: 0 };
                    column.set_proportion(0.5, 1184, 16);
                    column.select_window(WindowId::new(selected));
                    ws.scroll_offset = Point::new(37, 9);
                    monitor.active_workspace = key;
                    monitor.previous_workspace = Some(7);
                    monitor.set_focus(WindowId::new(selected));
                    assert_eq!(monitor.workspace_next_to(key, down), None);
                }
                let model_before = format!("{:?}", engine.monitors.get(&output).unwrap());
                let workspace_before = format!(
                    "{:?}",
                    engine.monitors.get(&output).unwrap().workspaces[&key]
                );
                let before = engine.ipc_snapshot();
                let applied_before = engine.applied_state.clone();
                let animating_before = format!("{:?}", engine.animating_rects);

                // Pinned niri monitor.rs:787-845 checks the boundary before detach,
                // regardless of focus-following. Neither path may detach/reinsert.
                if whole_column {
                    engine.move_column_to_workspace_relative(down, focus, &backend);
                } else {
                    engine.move_window_to_workspace_relative(
                        Some(WindowId::new(selected)),
                        down,
                        focus,
                        &backend,
                    );
                }

                assert_eq!(
                    format!("{:?}", engine.monitors.get(&output).unwrap()),
                    model_before,
                    "down={down}, focus={focus}, whole_column={whole_column}"
                );
                assert_eq!(engine.monitors.focused_id(), Some(output));
                let after = engine.ipc_snapshot();
                assert_eq!(after.workspaces, before.workspaces);
                assert_eq!(after.windows, before.windows);
                assert_eq!(engine.applied_state, applied_before);
                assert_eq!(format!("{:?}", engine.animating_rects), animating_before);
                assert_tile_at(&engine, selected, output, key);

                engine.maintain_empty_workspace(output);
                let tail = assert_trailing_empty(&engine, output);
                engine.maintain_empty_workspace(output);
                assert_eq!(assert_trailing_empty(&engine, output), tail);
                assert_eq!(
                    engine.monitors.get(&output).unwrap().workspace_ids(),
                    vec![40, 7, 25, tail]
                );
                assert_eq!(
                    format!(
                        "{:?}",
                        engine.monitors.get(&output).unwrap().workspaces[&key]
                    ),
                    workspace_before,
                    "tail maintenance must not change the boundary workspace"
                );
                if !down {
                    assert_eq!(tail, trailing, "a no-op preserves the existing tail");
                }
            }
        }
    }
}

#[test]
fn relative_moves_into_the_empty_tail_are_not_bottom_boundary_noops() {
    for focus in [false, true] {
        for whole_column in [false, true] {
            let (mut engine, backend, output) = ordered_fixture();
            let destination = assert_trailing_empty(&engine, output);
            {
                let monitor = engine.monitors.get_mut(&output).unwrap();
                monitor.active_workspace = 25;
                monitor.set_focus(WindowId::new(120));
                assert_eq!(monitor.workspace_next_to(25, true), Some(destination));
            }
            if whole_column {
                engine.move_column_to_workspace_relative(true, focus, &backend);
            } else {
                engine.move_window_to_workspace_relative(
                    Some(WindowId::new(120)),
                    true,
                    focus,
                    &backend,
                );
            }
            assert_tile_at(&engine, 120, output, destination);
            assert_eq!(
                engine.monitors.get(&output).unwrap().active_workspace,
                if focus { destination } else { 25 }
            );
            let trailing = assert_trailing_empty(&engine, output);
            assert_ne!(
                trailing, destination,
                "the occupied tail gets a new empty tail"
            );
            engine.maintain_empty_workspace(output);
            assert_eq!(assert_trailing_empty(&engine, output), trailing);
        }
    }
}

#[test]
fn column_move_preserves_metadata_and_inserts_after_target_active_column() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let backend = BackendHandle::default_for_test();
    let mut moved = column(&[101, 102]);
    moved.set_proportion(0.75, 1184, 16);
    moved.display = ColumnDisplay::Tabbed { active_tab: 0 };
    moved.select_window(WindowId::new(102));
    moved.maximized = true;
    moved.pre_max_width = Some(Some(433));
    moved.preset_idx = Some(2);
    moved.tiles[0].height_weight = 1.5;
    moved.tiles[1].height_weight = 2.5;
    moved.tiles[0].preferred_height = Some(275);
    engine.rule_heights.insert(WindowId::new(101), 275);
    let expected = moved.clone();
    let mut source = workspace("source", &[&[100], &[103, 104]]);
    source.columns[1].switch_to_tabbed();
    source.columns[1].next_tab();
    source.columns.insert(1, moved);
    let left = install_monitor(
        &mut engine,
        "lane-f-column-left",
        0,
        vec![(40, source)],
        40,
        102,
    );
    let right = install_monitor(
        &mut engine,
        "lane-f-column-right",
        1200,
        vec![(7, workspace("column-target", &[&[200], &[201], &[202]]))],
        7,
        201,
    );
    let right_before = cursor(&engine, right);

    engine.move_column_to_workspace_ref(
        WorkspaceRef::Name("column-target".to_string()),
        false,
        &backend,
    );

    assert_eq!(
        columns_on(&engine, left, 40),
        vec![vec![100], vec![103, 104]]
    );
    assert_eq!(
        columns_on(&engine, right, 7),
        vec![vec![200], vec![201], vec![101, 102], vec![202]]
    );
    assert_tile_at(&engine, 101, right, 7);
    assert_tile_at(&engine, 102, right, 7);
    let monitor = engine.monitors.get(&right).unwrap();
    let actual = &monitor.workspaces[&7].columns[2];
    assert_eq!(actual.active_window(), expected.active_window());
    // A maximized column reflows to the destination viewport during layout;
    // its requested/restore width and other metadata still survive the transfer.
    let view = TilingEngine::tiling_work_rect(monitor, engine.effective_config(right));
    assert_eq!(actual.width, Some(view.size.w));
    assert_eq!(actual.width_proportion, expected.width_proportion);
    assert_eq!(actual.width_fixed_logical, expected.width_fixed_logical);
    assert_eq!(actual.display, expected.display);
    assert_eq!(actual.maximized, expected.maximized);
    assert_eq!(actual.pre_max_width, expected.pre_max_width);
    assert_eq!(actual.preset_idx, expected.preset_idx);
    for (actual, expected) in actual.tiles.iter().zip(&expected.tiles) {
        assert_eq!(actual.window_id, expected.window_id);
        assert_eq!(actual.height_weight, expected.height_weight);
        assert_eq!(actual.preferred_height, expected.preferred_height);
    }
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(engine.monitors.get(&left).unwrap().active_workspace, 40);
    assert_eq!(cursor(&engine, right), right_before);
    let source = engine.monitors.get(&left).unwrap();
    assert_eq!(
        source.focus_window,
        Some(WindowId::new(104)),
        "source repair selects the surviving column's visible tab"
    );
    assert_eq!(
        source.workspaces[&40].remembered_focus(),
        Some((1, WindowId::new(104)))
    );
}

#[test]
fn relative_column_moves_follow_order_and_respect_focus() {
    let (mut engine, backend, output) = ordered_fixture();
    {
        let monitor = engine.monitors.get_mut(&output).unwrap();
        monitor.active_workspace = 7;
        monitor.set_focus(WindowId::new(111));
    }

    engine.move_column_to_workspace_relative(true, true, &backend);
    assert_eq!(columns_on(&engine, output, 7), Vec::<Vec<isize>>::new());
    assert_eq!(
        columns_on(&engine, output, 25),
        vec![vec![120], vec![110, 111]]
    );
    assert_eq!(
        cursor(&engine, output),
        (25, Some(1), Some(WindowId::new(111)))
    );
    assert_tile_at(&engine, 110, output, 25);
    assert_tile_at(&engine, 111, output, 25);

    engine.move_column_to_workspace_relative(false, false, &backend);
    assert_eq!(columns_on(&engine, output, 7), vec![vec![110, 111]]);
    assert_eq!(columns_on(&engine, output, 25), vec![vec![120]]);
    assert_eq!(
        cursor(&engine, output),
        (25, Some(0), Some(WindowId::new(120)))
    );
    assert_eq!(engine.monitors.focused_id(), Some(output));
}

#[test]
fn reorder_preserves_keys_ipc_ids_names_and_trailing_empty() {
    let (mut engine, backend, output) = ordered_fixture();
    {
        let monitor = engine.monitors.get_mut(&output).unwrap();
        monitor.active_workspace = 7;
        monitor.previous_workspace = Some(40);
        monitor.set_focus(WindowId::new(111));
    }
    let floating = WindowId::new(300);
    engine.tiled_windows.insert(floating, window_info(300));
    engine.register_float(floating, output, 7, Rect::new(80, 90, 400, 300));
    let trailing = assert_trailing_empty(&engine, output);
    let monitor = engine.monitors.get(&output).unwrap();
    let identities: Vec<_> = monitor
        .workspace_ids()
        .iter()
        .map(|key| {
            let workspace = &monitor.workspaces[key];
            (*key, workspace.ipc_id, workspace.name.clone())
        })
        .collect();
    let before = cursor(&engine, output);
    let windows_before: Vec<_> = engine
        .ipc_snapshot()
        .windows
        .iter()
        .map(|window| (window.id, window.workspace_id))
        .collect();

    engine.move_workspace_to_index(1, Some(WorkspaceRef::Name("middle".to_string())), &backend);
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspace_ids(),
        vec![7, 40, 25, trailing]
    );
    // Index references use the order before the move, not a stable key or numeric name.
    engine.move_workspace_to_index(2, Some(WorkspaceRef::Index(3)), &backend);
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspace_ids(),
        vec![7, 25, 40, trailing]
    );
    // An omitted reference selects the active workspace, even after its position changed.
    engine.move_workspace_to_index(3, None, &backend);
    let monitor = engine.monitors.get(&output).unwrap();
    assert_eq!(monitor.workspace_ids(), vec![25, 40, 7, trailing]);
    assert_eq!(cursor(&engine, output), before);
    assert_eq!(monitor.previous_workspace, Some(40));
    assert_eq!(monitor.workspaces.len(), identities.len());
    for (key, ipc_id, name) in &identities {
        let workspace = &monitor.workspaces[key];
        assert_eq!(workspace.ipc_id, *ipc_id);
        assert_eq!(&workspace.name, name);
        assert_eq!(engine.workspace_name_on(output, *key), name.clone());
    }
    assert_tile_at(&engine, 111, output, 7);
    assert_eq!(
        (
            engine.float_homes[&floating].output,
            engine.float_homes[&floating].workspace
        ),
        (output, 7)
    );
    assert_eq!(assert_trailing_empty(&engine, output), trailing);
    let snapshot = engine.ipc_snapshot();
    assert_eq!(
        snapshot
            .windows
            .iter()
            .map(|window| (window.id, window.workspace_id))
            .collect::<Vec<_>>(),
        windows_before
    );
    for (position, key) in monitor.workspace_ids().iter().enumerate() {
        let workspace = &monitor.workspaces[key];
        let ipc = snapshot
            .workspaces
            .iter()
            .find(|ipc| ipc.id == workspace.ipc_id)
            .unwrap();
        assert_eq!(ipc.idx, position as u32 + 1);
        assert_eq!(ipc.name, workspace.name);
    }
}

#[test]
fn reorder_to_last_slot_keeps_an_empty_workspace_after_named_workspaces() {
    let (mut engine, backend, output) = ordered_fixture();
    let before = cursor(&engine, output);
    let monitor = engine.monitors.get(&output).unwrap();
    let identity = monitor.workspaces[&7].ipc_id;
    let last_slot = monitor.workspace_ids().len() as u32;

    engine.move_workspace_to_index(
        last_slot,
        Some(WorkspaceRef::Name("middle".to_string())),
        &backend,
    );

    let trailing = assert_trailing_empty(&engine, output);
    let monitor = engine.monitors.get(&output).unwrap();
    assert_eq!(monitor.workspace_ids(), vec![40, 25, 7, trailing]);
    assert_eq!(monitor.workspaces[&7].ipc_id, identity);
    assert_eq!(monitor.workspaces[&7].name.as_deref(), Some("middle"));
    assert_eq!(
        engine.workspace_name_on(output, 7).as_deref(),
        Some("middle")
    );
    assert_tile_at(&engine, 110, output, 7);
    assert_tile_at(&engine, 111, output, 7);
    assert_eq!(cursor(&engine, output), before);
}

#[test]
fn workspace_names_are_unique_across_outputs() {
    let (mut engine, backend, left, right) = two_output_fixture();
    let left_before = cursor(&engine, left);
    let right_before = cursor(&engine, right);
    let left_id = engine.monitors.get(&left).unwrap().workspaces[&40].ipc_id;
    let right_id = engine.monitors.get(&right).unwrap().workspaces[&7].ipc_id;

    engine.set_workspace_name("destination", None, &backend);
    assert_eq!(engine.workspace_name_on(left, 40).as_deref(), Some("left"));
    assert_eq!(
        engine.workspace_name_on(right, 7).as_deref(),
        Some("destination")
    );
    assert!(engine.rename_workspace(40, "destination").is_err());
    // Reapplying the name to its owner is harmless, unlike assigning it elsewhere.
    engine.set_workspace_name(
        "destination",
        Some(WorkspaceRef::Name("destination".to_string())),
        &backend,
    );
    assert_eq!(
        engine.monitors.get(&left).unwrap().workspaces[&40].ipc_id,
        left_id
    );
    assert_eq!(
        engine.monitors.get(&right).unwrap().workspaces[&7].ipc_id,
        right_id
    );
    assert_eq!(
        engine.monitors.get(&left).unwrap().workspaces[&40]
            .name
            .as_deref(),
        Some("left")
    );
    let owners = engine
        .monitors
        .values()
        .flat_map(|monitor| monitor.workspaces.values())
        .filter(|workspace| workspace.name.as_deref() == Some("destination"))
        .count();
    assert_eq!(owners, 1);
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(cursor(&engine, left), left_before);
    assert_eq!(cursor(&engine, right), right_before);
}

#[test]
fn set_and_unset_names_are_scoped_to_workspace_objects() {
    let (mut engine, backend, left, right) = two_output_fixture();
    let left_before = cursor(&engine, left);
    let right_before = cursor(&engine, right);
    let right_id = engine.monitors.get(&right).unwrap().workspaces[&7].ipc_id;

    engine.rename_workspace(40, "renamed-left").unwrap();
    assert_eq!(
        engine.monitors.get(&left).unwrap().workspaces[&40]
            .name
            .as_deref(),
        Some("renamed-left")
    );
    assert_eq!(
        engine.workspace_name_on(right, 40).as_deref(),
        Some("right"),
        "same key on another output is unrelated"
    );
    engine.set_workspace_name(
        "remote-name",
        Some(WorkspaceRef::Name("destination".to_string())),
        &backend,
    );
    assert_eq!(
        engine.workspace_name_on(right, 7).as_deref(),
        Some("remote-name")
    );
    assert_eq!(
        engine.workspace_name_on(left, 7).as_deref(),
        Some("left-spare")
    );
    engine.unset_workspace_name(
        Some(WorkspaceRef::Name("remote-name".to_string())),
        &backend,
    );
    assert_eq!(
        engine.monitors.get(&right).unwrap().workspaces[&7].name,
        None
    );
    assert_eq!(engine.workspace_name_on(right, 7), None);
    assert_eq!(
        engine.monitors.get(&right).unwrap().workspaces[&7].ipc_id,
        right_id
    );
    assert_tile_at(&engine, 210, right, 7);

    engine.set_workspace_name("local-name", Some(WorkspaceRef::Index(2)), &backend);
    assert_eq!(
        engine.workspace_name_on(left, 7).as_deref(),
        Some("local-name")
    );
    engine.unset_workspace_name(Some(WorkspaceRef::Index(2)), &backend);
    assert_eq!(
        engine.monitors.get(&left).unwrap().workspaces[&7].name,
        None
    );
    engine.unset_workspace_name(None, &backend);
    assert_eq!(
        engine.monitors.get(&left).unwrap().workspaces[&40].name,
        None
    );
    assert_eq!(
        engine.workspace_name_on(right, 40).as_deref(),
        Some("right")
    );
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(cursor(&engine, left), left_before);
    assert_eq!(cursor(&engine, right), right_before);
}

#[test]
fn runtime_names_do_not_mutate_config_or_rebind_on_reload() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let backend = BackendHandle::default_for_test();
    let output = OutputId::from_name("lane-f-config");
    let bounds = Rect::new(0, 0, 1200, 800);
    engine.register_monitor(output, bounds, bounds);
    let mut config = crate::config::Config::default();
    config.workspace = vec![crate::config::WorkspaceConfig {
        name: "configured".to_string(),
        ..crate::config::WorkspaceConfig::default_config()
    }];
    engine.set_full_config(config.clone());
    let key = *engine
        .monitors
        .get(&output)
        .unwrap()
        .workspaces
        .iter()
        .find(|(_, workspace)| workspace.name.as_deref() == Some("configured"))
        .expect("config seeds a model-owned workspace name")
        .0;
    let identity = engine.monitors.get(&output).unwrap().workspaces[&key].ipc_id;
    // Keep the object occupied after unset; pruning an unnamed empty object is legitimate.
    engine
        .tiled_windows
        .insert(WindowId::new(100), window_info(100));
    {
        let monitor = engine.monitors.get_mut(&output).unwrap();
        monitor
            .workspaces
            .get_mut(&key)
            .unwrap()
            .columns
            .push(column(&[100]));
        monitor.active_workspace = key;
        monitor.set_focus(WindowId::new(100));
    }

    engine.rename_workspace(key, "runtime").unwrap();
    assert_eq!(
        engine.full_config.as_ref().unwrap().workspace[0].name,
        "configured"
    );
    engine.set_workspace_name(
        "renamed",
        Some(WorkspaceRef::Name("runtime".to_string())),
        &backend,
    );
    assert_eq!(
        engine.full_config.as_ref().unwrap().workspace[0].name,
        "configured"
    );
    engine.move_workspace_to_index(2, Some(WorkspaceRef::Name("renamed".to_string())), &backend);
    engine.set_full_config(config.clone());
    assert_eq!(
        engine.monitors.get(&output).unwrap().workspaces[&key].ipc_id,
        identity
    );
    assert_eq!(
        engine.workspace_name_on(output, key).as_deref(),
        Some("renamed")
    );
    assert_tile_at(&engine, 100, output, key);

    engine.unset_workspace_name(Some(WorkspaceRef::Name("renamed".to_string())), &backend);
    assert_eq!(
        engine.full_config.as_ref().unwrap().workspace[0].name,
        "configured"
    );
    engine.set_full_config(config);
    let workspace = &engine.monitors.get(&output).unwrap().workspaces[&key];
    assert_eq!(workspace.ipc_id, identity);
    assert_eq!(
        workspace.name, None,
        "reload must not attach a positional config name to this object"
    );
    assert_eq!(engine.workspace_name_on(output, key), None);
    assert_tile_at(&engine, 100, output, key);
    let snapshot = engine.ipc_snapshot();
    let ipc = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.id == identity)
        .unwrap();
    assert_eq!(ipc.name, None);
    assert_eq!(
        snapshot
            .windows
            .iter()
            .find(|window| window.id == 100)
            .unwrap()
            .workspace_id,
        Some(identity)
    );
}

#[test]
fn minimized_only_unnamed_inactive_workspace_survives_maintenance_and_reorder() {
    let (mut engine, backend, output) = ordered_fixture();
    let before = cursor(&engine, output);
    let trailing = assert_trailing_empty(&engine, output);
    let identity = engine.monitors.get(&output).unwrap().workspaces[&7].ipc_id;
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&7)
        .unwrap()
        .name = None;

    engine.minimize_window(WindowId::new(110), &backend);
    engine.minimize_window(WindowId::new(111), &backend);
    engine.maintain_empty_workspace(output);
    {
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.workspace_ids(), vec![40, 7, 25, trailing]);
        assert_eq!(monitor.workspaces[&7].ipc_id, identity);
        assert!(monitor.workspaces[&7].columns.is_empty());
        assert_eq!(monitor.workspaces[&7].name, None);
        assert_ne!(monitor.active_workspace, 7);
    }

    engine.move_workspace_to_index(3, Some(WorkspaceRef::Index(2)), &backend);
    engine.maintain_empty_workspace(output);
    engine.maintain_empty_workspace(output);
    let monitor = engine.monitors.get(&output).unwrap();
    assert_eq!(monitor.workspace_ids(), vec![40, 25, 7, trailing]);
    assert_eq!(monitor.workspaces[&7].ipc_id, identity);
    assert!(monitor.workspaces[&7].columns.is_empty());
    assert_eq!(monitor.workspaces[&7].name, None);
    assert_eq!(cursor(&engine, output), before);
    for id in [110, 111] {
        let window = WindowId::new(id);
        let home = &engine.minimized[&window];
        assert_eq!((home.output, home.workspace), (output, 7));
        assert!(home.slot.is_some());
        assert_eq!(engine.locate_window(window), None);
        assert!(!engine.floating_windows.contains(&window));
    }
    // The column-empty minimized home is occupied; only the final workspace is vacant.
    let vacant: Vec<_> = monitor
        .workspace_ids()
        .into_iter()
        .filter(|key| {
            let workspace = &monitor.workspaces[key];
            workspace.columns.is_empty()
                && workspace.name.is_none()
                && !engine
                    .minimized
                    .values()
                    .any(|home| home.output == output && home.workspace == *key)
                && !engine
                    .float_homes
                    .values()
                    .any(|home| home.output == output && home.workspace == *key)
        })
        .collect();
    assert_eq!(vacant, vec![trailing]);
    let snapshot = engine.ipc_snapshot();
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.id == identity)
        .unwrap();
    assert_eq!(workspace.idx, 3);
    assert_eq!(workspace.name, None);
    assert!(!workspace.is_active);
    for id in [110, 111] {
        let window = snapshot
            .windows
            .iter()
            .find(|window| window.id == id)
            .unwrap();
        assert!(window.is_minimized);
        assert_eq!(window.workspace_id, Some(identity));
    }
}

#[test]
fn empty_middle_active_workspace_survives_until_left() {
    let (mut engine, backend, output) = ordered_fixture();
    let trailing = assert_trailing_empty(&engine, output);
    let identity = engine.monitors.get(&output).unwrap().workspaces[&7].ipc_id;
    engine.switch_workspace(7, &backend);
    engine.unset_workspace_name(None, &backend);
    engine.move_column_to_workspace_ref(WorkspaceRef::Name("last".to_string()), false, &backend);

    for _ in 0..2 {
        engine.maintain_empty_workspace(output);
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.workspace_ids(), vec![40, 7, 25, trailing]);
        assert_eq!(monitor.workspaces[&7].ipc_id, identity);
        assert!(monitor.workspaces[&7].columns.is_empty());
        assert_eq!(monitor.workspaces[&7].name, None);
        assert_eq!(cursor(&engine, output), (7, None, None));
        let empty: Vec<_> = monitor
            .workspace_ids()
            .into_iter()
            .filter(|key| {
                let workspace = &monitor.workspaces[key];
                workspace.columns.is_empty() && workspace.name.is_none()
            })
            .collect();
        assert_eq!(empty, vec![7, trailing]);
    }
    let snapshot = engine.ipc_snapshot();
    let active = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.id == identity)
        .unwrap();
    assert!(active.is_active && active.is_focused);
    assert_eq!(active.active_window_id, None);

    engine.switch_workspace(40, &backend);
    engine.maintain_empty_workspace(output);
    let monitor = engine.monitors.get(&output).unwrap();
    assert!(!monitor.workspaces.contains_key(&7));
    assert_eq!(monitor.workspace_ids(), vec![40, 25, trailing]);
    assert_eq!(assert_trailing_empty(&engine, output), trailing);
    assert_tile_at(&engine, 110, output, 25);
    assert_tile_at(&engine, 111, output, 25);
    assert!(!engine
        .ipc_snapshot()
        .workspaces
        .iter()
        .any(|workspace| workspace.id == identity));
}

#[test]
fn configured_names_follow_declaration_order_monitor_placement_and_layout() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let first_name = "lane-f-config-order-first";
    let second_name = "lane-f-config-order-second";
    let first = OutputId::from_name(first_name);
    let second = OutputId::from_name(second_name);
    let bounds = Rect::new(0, 0, 1200, 800);
    engine.register_monitor(first, bounds, bounds);
    let bounds = Rect::new(1200, 0, 1200, 800);
    engine.register_monitor(second, bounds, bounds);
    engine.monitors.set_focused(second);
    let mut config = crate::config::Config::default();
    config.workspace = vec![
        crate::config::WorkspaceConfig {
            name: "zeta-local".to_string(),
            monitor: String::new(),
            layout: "spiral".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
        crate::config::WorkspaceConfig {
            name: "zeta-remote".to_string(),
            monitor: second_name.to_string(),
            layout: "bstack".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
        crate::config::WorkspaceConfig {
            name: "alpha-local".to_string(),
            monitor: first_name.to_string(),
            layout: "scrolling".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
        crate::config::WorkspaceConfig {
            name: "alpha-remote".to_string(),
            monitor: second_name.to_string(),
            layout: "spiral".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
    ];
    engine.set_full_config(config);

    for (output, expected) in [
        (
            first,
            vec![
                ("zeta-local", WorkspaceLayout::Spiral),
                ("alpha-local", WorkspaceLayout::Scrolling),
            ],
        ),
        (
            second,
            vec![
                ("zeta-remote", WorkspaceLayout::BStack),
                ("alpha-remote", WorkspaceLayout::Spiral),
            ],
        ),
    ] {
        let monitor = engine.monitors.get(&output).unwrap();
        let actual: Vec<_> = monitor
            .workspace_ids()
            .into_iter()
            .filter_map(|key| {
                let workspace = &monitor.workspaces[&key];
                workspace
                    .name
                    .as_deref()
                    .map(|name| (name, workspace.layout_mode))
            })
            .collect();
        assert_eq!(
            actual, expected,
            "declaration order is per output, not name-sorted"
        );
        assert_eq!(
            monitor.workspaces.len(),
            3,
            "two named workspaces and the trailing empty"
        );
        assert_trailing_empty(&engine, output);
    }
    assert_eq!(
        engine.monitors.focused_id(),
        Some(second),
        "unspecified placement does not use focus"
    );
}

#[test]
fn pending_configured_names_are_realized_on_the_requested_monitor_connect() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let host = OutputId::from_name("lane-f-pending-host");
    let pending_name = "lane-f-pending-output";
    let pending = OutputId::from_name(pending_name);
    let bounds = Rect::new(0, 0, 1200, 800);
    engine.register_monitor(host, bounds, bounds);
    let mut config = crate::config::Config::default();
    config.workspace = vec![
        crate::config::WorkspaceConfig {
            name: "zeta-pending".to_string(),
            monitor: pending_name.to_string(),
            layout: "spiral".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
        crate::config::WorkspaceConfig {
            name: "host-only".to_string(),
            monitor: String::new(),
            layout: "scrolling".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
        crate::config::WorkspaceConfig {
            name: "alpha-pending".to_string(),
            monitor: pending_name.to_string(),
            layout: "bstack".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
    ];
    for _ in 0..2 {
        engine.set_full_config(config.clone());
        let names: Vec<_> = engine
            .monitors
            .values()
            .flat_map(|monitor| monitor.workspaces.values())
            .filter_map(|workspace| workspace.name.as_deref())
            .collect();
        assert_eq!(
            names,
            vec!["host-only"],
            "missing outputs stay pending, without a fallback host"
        );
        assert!(!engine.monitors.contains_key(&pending));
    }
    let host_before = cursor(&engine, host);
    let bounds = Rect::new(1200, 0, 1200, 800);
    engine.register_monitor(pending, bounds, bounds);
    let monitor = engine.monitors.get(&pending).unwrap();
    let named: Vec<_> = monitor
        .workspace_ids()
        .into_iter()
        .filter_map(|key| {
            let workspace = &monitor.workspaces[&key];
            workspace
                .name
                .as_deref()
                .map(|name| (name, workspace.layout_mode))
        })
        .collect();
    assert_eq!(
        named,
        vec![
            ("zeta-pending", WorkspaceLayout::Spiral),
            ("alpha-pending", WorkspaceLayout::BStack)
        ]
    );
    let identities: Vec<_> = monitor
        .workspace_ids()
        .into_iter()
        .map(|key| {
            (
                key,
                monitor.workspaces[&key].ipc_id,
                monitor.workspaces[&key].name.clone(),
            )
        })
        .collect();
    assert_trailing_empty(&engine, pending);
    assert_eq!(engine.monitors.focused_id(), Some(host));
    assert_eq!(cursor(&engine, host), host_before);

    engine.set_full_config(config);
    let monitor = engine.monitors.get(&pending).unwrap();
    assert_eq!(monitor.workspaces.len(), identities.len());
    for (key, identity, name) in identities {
        assert_eq!(monitor.workspaces[&key].ipc_id, identity);
        assert_eq!(monitor.workspaces[&key].name, name);
    }
    let names: Vec<_> = engine
        .monitors
        .get(&host)
        .unwrap()
        .workspaces
        .values()
        .filter_map(|workspace| workspace.name.as_deref())
        .collect();
    assert_eq!(names, vec!["host-only"]);
}

#[test]
fn configured_names_are_not_rebound_after_runtime_rename_unset_and_reload() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let backend = BackendHandle::default_for_test();
    let home_name = "lane-f-config-identity-home";
    let other_name = "lane-f-config-identity-other";
    let home = OutputId::from_name(home_name);
    let other = OutputId::from_name(other_name);
    let bounds = Rect::new(0, 0, 1200, 800);
    engine.register_monitor(home, bounds, bounds);
    let bounds = Rect::new(1200, 0, 1200, 800);
    engine.register_monitor(other, bounds, bounds);
    let mut config = crate::config::Config::default();
    config.workspace = vec![
        crate::config::WorkspaceConfig {
            name: "configured-rename".to_string(),
            monitor: home_name.to_string(),
            layout: "spiral".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
        crate::config::WorkspaceConfig {
            name: "configured-unset".to_string(),
            monitor: home_name.to_string(),
            layout: "bstack".to_string(),
            ..crate::config::WorkspaceConfig::default_config()
        },
    ];
    engine.set_full_config(config.clone());
    let monitor = engine.monitors.get(&home).unwrap();
    let rename_key = *monitor
        .workspaces
        .iter()
        .find(|(_, workspace)| workspace.name.as_deref() == Some("configured-rename"))
        .unwrap()
        .0;
    let unset_key = *monitor
        .workspaces
        .iter()
        .find(|(_, workspace)| workspace.name.as_deref() == Some("configured-unset"))
        .unwrap()
        .0;
    let rename_id = monitor.workspaces[&rename_key].ipc_id;
    let unset_id = monitor.workspaces[&unset_key].ipc_id;
    // Occupancy keeps the unset object alive independently of any name declaration.
    for (key, id) in [(rename_key, 100), (unset_key, 101)] {
        engine
            .tiled_windows
            .insert(WindowId::new(id), window_info(id));
        engine
            .monitors
            .get_mut(&home)
            .unwrap()
            .workspaces
            .get_mut(&key)
            .unwrap()
            .columns
            .push(column(&[id]));
    }
    engine.monitors.set_focused(home);
    engine.monitors.get_mut(&home).unwrap().active_workspace = rename_key;
    engine
        .monitors
        .get_mut(&home)
        .unwrap()
        .set_focus(WindowId::new(100));
    engine
        .rename_workspace(rename_key, "runtime-renamed")
        .unwrap();
    engine.unset_workspace_name(
        Some(WorkspaceRef::Name("configured-unset".to_string())),
        &backend,
    );
    let stored: Vec<_> = engine
        .full_config
        .as_ref()
        .unwrap()
        .workspace
        .iter()
        .map(|workspace| {
            (
                workspace.name.as_str(),
                workspace.monitor.as_str(),
                workspace.layout.as_str(),
            )
        })
        .collect();
    assert_eq!(
        stored,
        vec![
            ("configured-rename", home_name, "spiral"),
            ("configured-unset", home_name, "bstack")
        ]
    );

    let mut reloaded = config.clone();
    reloaded.workspace.reverse();
    for declaration in &mut reloaded.workspace {
        declaration.monitor = other_name.to_string();
        declaration.layout = "scrolling".to_string();
    }
    // A changed declaration order, placement and layout must not rebind realized names.
    for reloaded in [reloaded, config] {
        engine.set_full_config(reloaded);
        let monitor = engine.monitors.get(&home).unwrap();
        assert_eq!(monitor.workspaces[&rename_key].ipc_id, rename_id);
        assert_eq!(
            monitor.workspaces[&rename_key].name.as_deref(),
            Some("runtime-renamed")
        );
        assert_eq!(
            monitor.workspaces[&rename_key].layout_mode,
            WorkspaceLayout::Spiral
        );
        assert_eq!(monitor.workspaces[&unset_key].ipc_id, unset_id);
        assert_eq!(monitor.workspaces[&unset_key].name, None);
        assert_eq!(
            monitor.workspaces[&unset_key].layout_mode,
            WorkspaceLayout::BStack
        );
        assert_tile_at(&engine, 100, home, rename_key);
        assert_tile_at(&engine, 101, home, unset_key);
        assert!(engine
            .monitors
            .get(&other)
            .unwrap()
            .workspaces
            .values()
            .all(|workspace| { workspace.name.is_none() && workspace.columns.is_empty() }));
        assert!(!engine
            .monitors
            .values()
            .flat_map(|monitor| monitor.workspaces.values())
            .any(|workspace| {
                matches!(
                    workspace.name.as_deref(),
                    Some("configured-rename" | "configured-unset")
                )
            }));
        assert_trailing_empty(&engine, home);
        let snapshot = engine.ipc_snapshot();
        for (id, identity) in [(100, rename_id), (101, unset_id)] {
            let window = snapshot
                .windows
                .iter()
                .find(|window| window.id == id)
                .unwrap();
            assert_eq!(window.workspace_id, Some(identity));
        }
    }
}

#[test]
fn configured_name_waits_until_the_connector_name_is_registered() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let output = OutputId::from_name("physical-late-output");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1200, 800),
        Rect::new(0, 0, 1200, 800),
    );
    let mut config = crate::config::Config::default();
    config.workspace = vec![crate::config::WorkspaceConfig {
        name: "late-name".to_string(),
        open_on_outputs: vec!["DISPLAY9".to_string()],
        ..crate::config::WorkspaceConfig::default_config()
    }];
    engine.set_full_config(config);
    assert!(
        !engine
            .monitors
            .get(&output)
            .unwrap()
            .workspaces
            .values()
            .any(|workspace| workspace.name.as_deref() == Some("late-name")),
        "the declaration is pending while the connector has no name yet"
    );
    engine.set_output_name(output, "DISPLAY9");
    engine.ensure_named_workspaces();
    assert!(
        engine
            .monitors
            .get(&output)
            .unwrap()
            .workspaces
            .values()
            .any(|workspace| workspace.name.as_deref() == Some("late-name")),
        "the final display sync realizes it once names are known"
    );
}

#[test]
fn configured_output_preferences_and_reload_top_insertion_preserve_prior_workspace_order() {
    let (mut engine, backend, left, right) = two_output_fixture();
    engine.set_output_name(left, r"\\.\DISPLAY1");
    engine.set_output_name(right, r"\\.\DISPLAY2");
    let declaration = |name: &str, layout: &str| crate::config::WorkspaceConfig {
        name: name.to_string(),
        layout: layout.to_string(),
        // The second preference is connected and matches its cached DISPLAY name
        // case-insensitively; the connected legacy monitor is only a final fallback.
        open_on_outputs: vec!["DISPLAY404".to_string(), "dIsPlAy2".to_string()],
        monitor: "DISPLAY1".to_string(),
        ..crate::config::WorkspaceConfig::default_config()
    };
    let mut config = crate::config::Config::default();
    config.workspace = vec![
        declaration("prior-zeta", "spiral"),
        declaration("prior-alpha", "bstack"),
    ];
    engine.set_full_config(config.clone());
    let monitor = engine.monitors.get(&right).unwrap();
    let names: Vec<_> = monitor
        .workspace_ids()
        .into_iter()
        .filter_map(|key| monitor.workspaces[&key].name.as_deref())
        .collect();
    assert_eq!(
        names,
        vec!["prior-zeta", "prior-alpha", "right", "destination"]
    );
    assert!(!engine
        .monitors
        .get(&left)
        .unwrap()
        .workspaces
        .values()
        .any(|workspace| {
            matches!(
                workspace.name.as_deref(),
                Some("prior-zeta" | "prior-alpha")
            )
        }));

    // Realized declarations must not restore their original order on reload.
    engine.move_workspace_to_index(
        1,
        Some(WorkspaceRef::Name("prior-alpha".to_string())),
        &backend,
    );
    let monitor = engine.monitors.get(&right).unwrap();
    let prior: Vec<_> = monitor
        .workspace_ids()
        .into_iter()
        .filter_map(|key| {
            let workspace = &monitor.workspaces[&key];
            workspace
                .name
                .clone()
                .map(|name| (key, workspace.ipc_id, name, workspace.layout_mode))
        })
        .collect();
    assert_eq!(
        prior
            .iter()
            .map(|(_, _, name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["prior-alpha", "prior-zeta", "right", "destination"]
    );
    let left_before = cursor(&engine, left);
    let right_before = cursor(&engine, right);
    // Interleave consumed declarations with new ones. Even changed placement/layout
    // for consumed names must neither rebind them nor reserve a top insertion slot.
    let mut old_zeta = config.workspace[0].clone();
    let mut old_alpha = config.workspace[1].clone();
    for old in [&mut old_zeta, &mut old_alpha] {
        old.open_on_outputs = vec!["display1".to_string()];
        old.layout = "scrolling".to_string();
    }
    config.workspace = vec![
        old_zeta,
        declaration("reload-zeta", "bstack"),
        old_alpha,
        declaration("reload-alpha", "spiral"),
    ];
    engine.set_full_config(config.clone());
    for _ in 0..2 {
        let monitor = engine.monitors.get(&right).unwrap();
        let names: Vec<_> = monitor
            .workspace_ids()
            .into_iter()
            .filter_map(|key| monitor.workspaces[&key].name.as_deref())
            .collect();
        assert_eq!(
            names,
            vec![
                "reload-zeta",
                "reload-alpha",
                "prior-alpha",
                "prior-zeta",
                "right",
                "destination",
            ]
        );
        for (key, identity, name, layout) in &prior {
            let workspace = &monitor.workspaces[key];
            assert_eq!(workspace.ipc_id, *identity);
            assert_eq!(workspace.name.as_deref(), Some(name.as_str()));
            assert_eq!(workspace.layout_mode, *layout);
        }
        let left_names: Vec<_> = engine
            .monitors
            .get(&left)
            .unwrap()
            .workspace_ids()
            .into_iter()
            .filter_map(|key| {
                engine.monitors.get(&left).unwrap().workspaces[&key]
                    .name
                    .as_deref()
            })
            .collect();
        assert_eq!(left_names, vec!["left", "left-spare"]);
        assert_eq!(engine.monitors.focused_id(), Some(left));
        assert_eq!(cursor(&engine, left), left_before);
        assert_eq!(cursor(&engine, right), right_before);
        assert_trailing_empty(&engine, right);
        engine.set_full_config(config.clone());
    }
}
