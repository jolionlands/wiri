//! Synthetic output topology only: these tests never change Windows display settings.
use super::*;
use crate::backend::WindowState;
use crate::input::{MonitorTarget, WorkspaceRef};
use crate::layout::ColumnDisplay;

fn add_output(engine: &mut TilingEngine, name: &str, rect: Rect) -> OutputId {
    let output = OutputId::from_name(name);
    engine.register_monitor(output, rect, rect);
    engine.set_output_name(output, name);
    output
}

fn add_window(engine: &mut TilingEngine, backend: &BackendHandle, id: isize) {
    let output = engine.monitors.focused_id().unwrap();
    let workspace_id = engine.monitors.get(&output).unwrap().active_workspace;
    engine.add_window_with_target(
        WindowInfo {
            hwnd: id,
            title: id.to_string(),
            class_name: "MonitorFixture".into(),
            process_id: 1,
            bounds: Rect::new(0, 0, 400, 300),
            state: WindowState::Normal,
            is_visible: true,
        },
        AddWindowTarget::Workspace {
            output,
            workspace_id,
        },
        backend,
        false,
    );
}

fn fixture() -> (TilingEngine, BackendHandle, OutputId, OutputId) {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let backend = BackendHandle::default_for_test();
    let left = add_output(&mut engine, "left", Rect::new(-1200, 0, 1200, 800));
    let right = add_output(&mut engine, "right", Rect::new(0, 0, 1600, 900));
    engine.set_focused_output(left);
    engine.rename_workspace(0, "source").unwrap();
    for id in 100..104 {
        add_window(&mut engine, &backend, id);
    }
    engine.set_focused_output(right);
    engine.rename_workspace(0, "destination").unwrap();
    add_window(&mut engine, &backend, 200);
    engine.focus_action_window(WindowId::new(100), &backend);
    (engine, backend, left, right)
}

#[test]
fn directional_resolution_uses_both_axes_and_does_not_wrap() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let center = add_output(&mut engine, "center", Rect::new(0, 0, 1000, 800));
    // Insert a close diagonal first; the aligned output wins despite a larger gap.
    add_output(&mut engine, "diagonal", Rect::new(1001, 1200, 500, 500));
    let right = add_output(&mut engine, "right", Rect::new(1800, 200, 500, 500));
    let above = add_output(&mut engine, "above", Rect::new(200, -700, 600, 600));
    assert_eq!(
        engine.monitor_target(center, &MonitorTarget::Right),
        Some(right)
    );
    assert_eq!(
        engine.monitor_target(center, &MonitorTarget::Up),
        Some(above)
    );
    assert_eq!(engine.monitor_target(center, &MonitorTarget::Left), None);
    assert_eq!(
        engine.monitor_target(above, &MonitorTarget::Down),
        Some(center)
    );
}

#[test]
fn monitor_cycles_are_cyclic_in_enumeration_order_not_hash_order() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let a = add_output(&mut engine, "z", Rect::new(0, 0, 100, 100));
    let b = add_output(&mut engine, "a", Rect::new(100, 0, 100, 100));
    let c = add_output(&mut engine, "m", Rect::new(200, 0, 100, 100));
    assert_eq!(engine.monitor_target(a, &MonitorTarget::Previous), Some(c));
    assert_eq!(engine.monitor_target(c, &MonitorTarget::Next), Some(a));
    assert_eq!(engine.monitor_target(a, &MonitorTarget::Next), Some(b));
    engine.unregister_monitor(&b);
    assert_eq!(engine.monitor_target(a, &MonitorTarget::Next), Some(c));
}

#[test]
fn extreme_output_coordinates_do_not_overflow_directional_scoring() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let left = add_output(
        &mut engine,
        "far-left",
        Rect::new(i32::MIN, i32::MIN, 100, 100),
    );
    let right = add_output(
        &mut engine,
        "far-right",
        Rect::new(i32::MAX - 100, i32::MAX - 100, 100, 100),
    );
    assert_eq!(
        engine.monitor_target(left, &MonitorTarget::Right),
        Some(right)
    );
    assert_eq!(
        engine.monitor_target(right, &MonitorTarget::Left),
        Some(left)
    );
}

#[test]
fn chained_hotplug_does_not_duplicate_an_earlier_workspace_identity_or_name() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let backend = BackendHandle::default_for_test();
    let first = add_output(&mut engine, "first", Rect::new(0, 0, 1200, 800));
    engine.set_focused_output(first);
    engine.rename_workspace(0, "chained").unwrap();
    add_window(&mut engine, &backend, 100);
    let second = add_output(&mut engine, "second", Rect::new(1200, 0, 1200, 800));
    engine.set_focused_output(second);
    engine.rename_workspace(0, "middle").unwrap();
    add_window(&mut engine, &backend, 200);
    let third = add_output(&mut engine, "third", Rect::new(2400, 0, 1200, 800));
    engine.set_focused_output(third);
    engine.rename_workspace(0, "last").unwrap();
    add_window(&mut engine, &backend, 300);

    let chained_id = engine.monitors.get(&first).unwrap().workspaces[&0].ipc_id;
    engine.set_focused_output(second);
    engine.unregister_monitor(&first);
    assert!(engine
        .monitors
        .get(&second)
        .unwrap()
        .workspaces
        .values()
        .any(|workspace| workspace.name.as_deref() == Some("chained")));
    engine.set_focused_output(third);
    engine.unregister_monitor(&second);
    engine.register_monitor(
        first,
        Rect::new(0, 0, 1200, 800),
        Rect::new(0, 0, 1200, 800),
    );
    engine.set_output_name(first, "first");

    let first_monitor = engine.monitors.get(&first).unwrap();
    let restored = first_monitor
        .workspaces
        .values()
        .find(|workspace| workspace.name.as_deref() == Some("chained"))
        .expect("the original workspace returns to its monitor");
    assert_eq!(restored.ipc_id, chained_id);
    assert_eq!(
        restored
            .columns
            .iter()
            .flat_map(|column| &column.tiles)
            .map(|tile| tile.window_id)
            .collect::<Vec<_>>(),
        vec![WindowId::new(100)]
    );
    let all: Vec<u64> = engine
        .monitors
        .iter()
        .flat_map(|(_, monitor)| monitor.workspaces.values())
        .map(|workspace| workspace.ipc_id)
        .collect();
    let unique: std::collections::HashSet<_> = all.iter().copied().collect();
    assert_eq!(all.len(), unique.len(), "live IPC identities are unique");
    assert!(!engine
        .monitors
        .get(&third)
        .unwrap()
        .workspaces
        .values()
        .any(|workspace| workspace.name.as_deref() == Some("chained")));
}

#[test]
fn explicit_unfocused_window_transfer_does_not_steal_selection() {
    let (mut engine, backend, left, right) = fixture();
    engine.move_window_to_monitor_action(Some(WindowId::new(101)), &MonitorTarget::Next, &backend);
    assert_eq!(
        engine.locate_window(WindowId::new(101)).map(|p| p.0),
        Some(right)
    );
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(
        engine.monitors.get(&left).unwrap().focus_window,
        Some(WindowId::new(100))
    );
    assert_eq!(
        engine.monitors.get(&right).unwrap().focus_window,
        Some(WindowId::new(200))
    );
    engine.move_window_to_monitor_action(Some(WindowId::new(100)), &MonitorTarget::Right, &backend);
    assert_eq!(engine.monitors.focused_id(), Some(right));
    assert_eq!(
        engine.monitors.get(&right).unwrap().focus_window,
        Some(WindowId::new(100))
    );
}

#[test]
fn monitor_column_transfer_preserves_tabs_restore_width_and_tile_weights() {
    let (mut engine, backend, left, right) = fixture();
    {
        let ws = engine
            .monitors
            .get_mut(&left)
            .unwrap()
            .workspace_mut()
            .unwrap();
        let tile = ws.columns.remove(1).tiles.remove(0);
        ws.columns[0].tiles.push(tile);
        let column = &mut ws.columns[0];
        column.set_fixed_logical(592.0, 1.0, 8);
        column.pre_max_width = Some(Some(433));
        column.display = ColumnDisplay::Tabbed { active_tab: 0 };
        column.select_window(WindowId::new(101));
        column.tiles[0].height_weight = 1.5;
    }
    engine.monitors.get_mut(&right).unwrap().scale_factor = 1.5;
    engine.focus_action_window(WindowId::new(101), &backend);
    engine.move_column_to_monitor_action(&MonitorTarget::Right, &backend);
    let (output, _, column) = engine.locate_window(WindowId::new(101)).unwrap();
    assert_eq!(output, right);
    let column = &engine
        .monitors
        .get(&right)
        .unwrap()
        .workspace()
        .unwrap()
        .columns[column];
    assert_eq!(column.tiles.len(), 2);
    assert_eq!(column.width_fixed_logical, Some(592.0));
    assert_eq!(column.width, Some(896));
    assert_eq!(column.active_window(), Some(WindowId::new(101)));
    assert_eq!(column.pre_max_width, Some(Some(433)));
    assert_eq!(column.display, ColumnDisplay::Tabbed { active_tab: 1 });
    assert_eq!(column.tiles[0].height_weight, 1.5);
    assert_eq!(engine.monitors.focused_id(), Some(right));
}

#[test]
fn workspace_monitor_transfer_rehomes_floats_minimized_and_stable_identity() {
    let (mut engine, backend, left, right) = fixture();
    engine.toggle_floating_for(WindowId::new(102), &backend);
    engine.minimize_window(WindowId::new(103), &backend);
    engine.focus_action_window(WindowId::new(100), &backend);
    let identity = engine.monitors.get(&left).unwrap().workspaces[&0].ipc_id;
    engine.move_workspace_to_monitor_action(None, &MonitorTarget::Right, &backend);
    let (output, key, _) = engine.locate_window(WindowId::new(100)).unwrap();
    assert_eq!(output, right);
    let moved = &engine.monitors.get(&right).unwrap().workspaces[&key];
    assert_eq!(moved.ipc_id, identity);
    assert_eq!(moved.name.as_deref(), Some("source"));
    assert_eq!(
        engine.float_location(WindowId::new(102)),
        Some((right, key))
    );
    let minimized = &engine.minimized[&WindowId::new(103)];
    assert_eq!((minimized.output, minimized.workspace), (right, key));
    assert_eq!(engine.monitors.focused_id(), Some(right));
    assert_eq!(engine.monitors.get(&right).unwrap().active_workspace, key);
    let snapshot = engine.ipc_snapshot();
    assert_eq!(
        snapshot
            .workspaces
            .iter()
            .filter(|w| w.id == identity)
            .count(),
        1
    );
    assert!(snapshot
        .windows
        .iter()
        .filter(|w| (100..104).contains(&w.id))
        .all(|w| w.workspace_id == Some(identity)));
}

#[test]
fn explicit_inactive_workspace_transfer_does_not_change_output_focus() {
    let (mut engine, backend, left, right) = fixture();
    engine.move_workspace_to_monitor_action(
        Some(WorkspaceRef::Name("destination".into())),
        &MonitorTarget::Previous,
        &backend,
    );
    assert_eq!(engine.monitors.focused_id(), Some(left));
    assert_eq!(engine.monitors.get(&left).unwrap().active_workspace, 0);
    assert_eq!(
        engine.monitors.get(&left).unwrap().focus_window,
        Some(WindowId::new(100))
    );
    assert_eq!(
        engine.locate_window(WindowId::new(200)).map(|p| p.0),
        Some(left)
    );
    assert!(engine.monitors.get(&right).unwrap().workspace().is_some());
}

#[test]
fn explicit_workspace_rehome_cancels_automatic_return_to_unplugged_output() {
    let (mut engine, backend, left, right) = fixture();
    let third = add_output(&mut engine, "third", Rect::new(1600, 0, 1200, 800));
    engine.unregister_monitor(&right);
    engine.move_workspace_to_monitor_action(
        Some(WorkspaceRef::Name("destination".into())),
        &MonitorTarget::Name("third".into()),
        &backend,
    );
    add_output(&mut engine, "right", Rect::new(0, 0, 1600, 900));
    assert_eq!(
        engine.locate_window(WindowId::new(200)).map(|p| p.0),
        Some(third)
    );
    assert_eq!(engine.monitors.focused_id(), Some(left));
    let snapshot = engine.ipc_snapshot();
    assert_eq!(
        snapshot
            .workspaces
            .iter()
            .filter(|w| w.name.as_deref() == Some("destination"))
            .count(),
        1
    );
}

#[test]
fn single_monitor_and_missing_target_actions_are_safe_noops() {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let backend = BackendHandle::default_for_test();
    let output = add_output(&mut engine, "only", Rect::new(0, 0, 1366, 768));
    add_window(&mut engine, &backend, 100);
    engine.focus_action_window(WindowId::new(100), &backend);
    let before = engine.ipc_snapshot();
    for target in [
        MonitorTarget::Left,
        MonitorTarget::Right,
        MonitorTarget::Up,
        MonitorTarget::Down,
        MonitorTarget::Previous,
        MonitorTarget::Next,
        MonitorTarget::Name("only".into()),
        MonitorTarget::Name("missing".into()),
    ] {
        engine.focus_monitor_action(&target, &backend);
        engine.move_window_to_monitor_action(Some(WindowId::new(100)), &target, &backend);
        engine.move_column_to_monitor_action(&target, &backend);
        engine.move_workspace_to_monitor_action(None, &target, &backend);
    }
    assert_eq!(engine.monitors.focused_id(), Some(output));
    let after = engine.ipc_snapshot();
    assert_eq!(after.workspaces, before.workspaces);
    assert_eq!(after.windows, before.windows);
}
