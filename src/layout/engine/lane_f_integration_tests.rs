//! Lane F integration regressions against the shared G selection and H snapshot model.
use super::*;
use crate::backend::WindowState;
use crate::overlay::thumbnail_overview::OverviewCommand;

fn fixture() -> (TilingEngine, BackendHandle, OutputId) {
    let backend = BackendHandle::default_for_test();
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let output = OutputId::from_name("lane-f-integration");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1200, 800),
        Rect::new(0, 0, 1200, 760),
    );
    engine.rename_workspace(0, "first").unwrap();
    for id in 100..102 {
        engine.add_window(
            WindowInfo {
                hwnd: id,
                title: id.to_string(),
                class_name: "fixture".into(),
                process_id: 1,
                bounds: Rect::new(0, 0, 400, 300),
                state: WindowState::Normal,
                is_visible: true,
            },
            &backend,
        );
    }
    engine.switch_workspace(1, &backend);
    engine.rename_workspace(1, "second").unwrap();
    engine.switch_workspace(0, &backend);
    (engine, backend, output)
}

#[test]
fn floating_only_workspace_restores_its_last_focused_float() {
    let (mut engine, backend, output) = fixture();
    for id in 100..102 {
        engine.float_window(WindowId::new(id), &backend);
    }
    engine.focus_float(WindowId::new(100), &backend);
    engine.switch_workspace(1, &backend);
    engine.switch_workspace(0, &backend);
    let monitor = engine.monitors.get(&output).unwrap();
    assert_eq!(monitor.focus_window, Some(WindowId::new(100)));
    assert_eq!(monitor.focus_column, None);
    assert_eq!(
        monitor.workspace().unwrap().active_float,
        Some(WindowId::new(100))
    );
}

#[test]
fn remembered_float_is_a_float_action_target_without_native_foreground() {
    let (mut engine, backend, output) = fixture();
    let window = WindowId::new(101);
    engine.float_window(window, &backend);
    engine.focus_float(window, &backend);
    engine.switch_workspace(1, &backend);
    engine.switch_workspace(0, &backend);
    assert_eq!(
        engine.monitors.get(&output).unwrap().focus_window,
        Some(window)
    );

    // Desktop/taskbar focus clears the live foreground; the restored workspace
    // selection still identifies a float, not a tile in its remembered column.
    engine.os_foreground = None;
    let target = engine
        .resolve_action_target_for(None, false, &backend)
        .unwrap();
    assert_eq!(target.window, window);
    assert!(target.floating);

    let tile = WindowId::new(100);
    engine.focus_action_window(tile, &backend);
    engine.os_foreground = None;
    let target = engine
        .resolve_action_target_for(None, false, &backend)
        .unwrap();
    assert_eq!(target.window, tile);
    assert!(!target.floating);
}

#[test]
fn floating_the_native_foreground_remembers_focus_without_another_notice() {
    let (mut engine, backend, output) = fixture();
    let window = WindowId::new(101);
    engine.sync_os_foreground(window, &backend);
    engine.float_window(window, &backend);
    assert_eq!(
        engine
            .monitors
            .get(&output)
            .unwrap()
            .workspace()
            .unwrap()
            .active_float,
        Some(window)
    );
    engine.switch_workspace(1, &backend);
    engine.switch_workspace(0, &backend);
    assert_eq!(
        engine.monitors.get(&output).unwrap().focus_window,
        Some(window)
    );
}

#[test]
fn floating_a_background_tile_does_not_replace_the_native_selection() {
    let (mut engine, backend, output) = fixture();
    engine.sync_os_foreground(WindowId::new(100), &backend);
    engine.float_window(WindowId::new(101), &backend);
    assert_eq!(
        engine
            .monitors
            .get(&output)
            .unwrap()
            .workspace()
            .unwrap()
            .active_float,
        None
    );
    engine.switch_workspace(1, &backend);
    engine.switch_workspace(0, &backend);
    assert_eq!(
        engine.monitors.get(&output).unwrap().focus_window,
        Some(WindowId::new(100))
    );
}

#[test]
fn newer_tiled_focus_supersedes_the_floating_layer_marker() {
    let (mut engine, backend, output) = fixture();
    engine.float_window(WindowId::new(101), &backend);
    engine.focus_float(WindowId::new(101), &backend);
    engine.focus_action_window(WindowId::new(100), &backend);
    assert_eq!(
        engine
            .monitors
            .get(&output)
            .unwrap()
            .workspace()
            .unwrap()
            .active_float,
        None
    );
    engine.switch_workspace(1, &backend);
    engine.switch_workspace(0, &backend);
    assert_eq!(
        engine.monitors.get(&output).unwrap().focus_window,
        Some(WindowId::new(100))
    );
}

#[test]
fn minimized_float_is_not_restored_as_workspace_focus() {
    let (mut engine, backend, output) = fixture();
    engine.float_window(WindowId::new(101), &backend);
    engine.focus_float(WindowId::new(101), &backend);
    engine.minimize_window(WindowId::new(101), &backend);
    engine.switch_workspace(1, &backend);
    engine.switch_workspace(0, &backend);
    assert_eq!(
        engine.monitors.get(&output).unwrap().focus_window,
        Some(WindowId::new(100))
    );
}

#[test]
fn reordered_overview_navigation_uses_visible_order_not_local_keys() {
    let (mut engine, backend, output) = fixture();
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .move_workspace_to_position(1, 0);
    engine.overview = Some(OverviewState {
        zoom: 0.5,
        workspace_offsets: Vec::new(),
    });
    engine.overview_command(OverviewCommand::PreviousWorkspace, &backend);
    assert_eq!(engine.monitors.get(&output).unwrap().active_workspace, 1);
    engine.overview_command(OverviewCommand::NextWorkspace, &backend);
    assert_eq!(engine.monitors.get(&output).unwrap().active_workspace, 0);
}

#[test]
fn reordered_workspace_slide_uses_visible_direction() {
    let (mut engine, backend, output) = fixture();
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .move_workspace_to_position(1, 0);
    let mut config = crate::config::Config::default();
    config.animations.enabled = true;
    config.animations.duration = 200;
    config.animations.workspace_transition = true;
    engine.set_full_config(config);
    engine.update_animation_settings(true, 200, crate::layout::Easing::Linear);
    engine.switch_workspace(1, &backend);
    assert!(engine.animation.workspace_slide_offset(output) < 0.0);
    engine.switch_workspace(0, &backend);
    assert!(engine.animation.workspace_slide_offset(output) > 0.0);
}

#[test]
fn net_negative_struts_share_one_bounded_view_with_focus_and_transfers() {
    let (_, _, output) = fixture();
    let monitor = Monitor::new(
        output,
        Rect::new(0, 0, 1200, 800),
        Rect::new(0, 0, 1200, 760),
    );
    let config = LayoutConfig {
        reserve_left: -20,
        outer_gaps: (0, 0, 0, 0),
        ..LayoutConfig::default()
    };
    assert_eq!(
        TilingEngine::tiling_work_rect(&monitor, &config),
        Rect::new(-20, 0, 1220, 760)
    );
    assert_eq!(
        TilingEngine::view_width_for_monitor(&monitor, &config),
        1220
    );
}
