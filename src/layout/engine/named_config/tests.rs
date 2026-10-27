use super::*;
use crate::config::effects::{PaintStyle, RingStyle};
use crate::config::{Config, LayoutConfigPartial, OutputConfig, WorkspaceConfig};
use crate::layout::Workspace;

fn workspace_config(
    name: &str,
    preferences: &[&str],
    layout_override: Option<LayoutConfigPartial>,
) -> WorkspaceConfig {
    let mut workspace = WorkspaceConfig::default_config();
    workspace.name = name.to_string();
    workspace.open_on_outputs = preferences.iter().map(|name| (*name).to_string()).collect();
    workspace.layout_override = layout_override;
    workspace
}

fn output_config(name: &str, layout_override: LayoutConfigPartial) -> OutputConfig {
    let mut output = Config::default().output.remove(0);
    output.name = name.to_string();
    output.layout_override = Some(layout_override);
    output
}

fn engine_with_config(config: Config) -> TilingEngine {
    let mut engine = TilingEngine::new(LayoutConfig::from_config(&config));
    engine.set_full_config(config);
    engine
}

fn connect(engine: &mut TilingEngine, identity: &str, name: &str) -> OutputId {
    // Equal work areas keep rect fitting out of these ownership/configuration tests.
    // IDs deliberately differ from names: matching must use set_output_name, too.
    let output = OutputId::from_name(identity);
    let area = Rect::new(0, 0, 1920, 1080);
    engine.register_monitor(output, area, area);
    engine.set_output_name(output, name);
    output
}

fn seed_contents(engine: &mut TilingEngine, output: OutputId, workspace: i32) -> [WindowId; 3] {
    let tiled = WindowId::new(101);
    let floating = WindowId::new(102);
    let minimized = WindowId::new(103);
    let strip = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .entry(workspace)
        .or_insert_with(Workspace::new);
    let column = strip.add_window_to_new_column(tiled);
    strip.columns[column].width = Some(641);

    engine.floating_windows.insert(floating);
    engine.float_homes.insert(
        floating,
        FloatHome {
            output,
            workspace,
            rect: Rect::new(40, 60, 320, 240),
            seq: 7,
            width_preset_idx: Some(1),
        },
    );

    // Obtain genuine restore metadata without minimizing an OS window.
    let mut before_minimize = Workspace::new();
    let column = before_minimize.add_window_to_new_column(minimized);
    before_minimize.columns[column].width = Some(389);
    engine.minimized.insert(
        minimized,
        MinimizedWindow {
            output,
            workspace,
            slot: before_minimize.slot_of(minimized),
            was_fullscreen: true,
        },
    );
    [tiled, floating, minimized]
}

fn assert_contents_on(
    engine: &TilingEngine,
    output: OutputId,
    workspace: i32,
    [tiled, floating, minimized]: [WindowId; 3],
) {
    let (actual_output, actual_workspace, column) = engine.locate_window(tiled).unwrap();
    assert_eq!((actual_output, actual_workspace), (output, workspace));
    let strip = &engine.monitors.get(&output).unwrap().workspaces[&workspace];
    assert_eq!(strip.columns[column].width, Some(641));
    let tiled_copies = engine
        .monitors
        .iter()
        .flat_map(|(_, monitor)| monitor.workspaces.values())
        .filter(|strip| strip.find_window_column(tiled).is_some())
        .count();
    assert_eq!(tiled_copies, 1);

    assert!(engine.floating_windows.contains(&floating));
    let home = &engine.float_homes[&floating];
    assert_eq!((home.output, home.workspace), (output, workspace));
    assert_eq!(home.rect, Rect::new(40, 60, 320, 240));
    assert_eq!(home.seq, 7);
    assert_eq!(home.width_preset_idx, Some(1));
    assert_eq!(engine.float_homes.len(), 1);

    let home = &engine.minimized[&minimized];
    assert_eq!((home.output, home.workspace), (output, workspace));
    assert!(home.was_fullscreen);
    let slot = home.slot.as_ref().unwrap();
    assert_eq!(slot.window_id, minimized);
    assert_eq!(slot.column_width, Some(389));
    assert_eq!(engine.minimized.len(), 1);
}

#[test]
fn inactive_workspace_width_intent_and_refresh_use_its_own_layout() {
    use crate::config::types::SizeSpec;
    let mut config = Config::default();
    config.layout.default_column_width = Some(Some(SizeSpec::Fixed(600)));
    config.workspace = vec![
        workspace_config("Active", &[], None),
        workspace_config(
            "Inactive",
            &[],
            Some(LayoutConfigPartial {
                default_column_width: Some(Some(SizeSpec::Fixed(200))),
                outer_gaps: Some(30),
                inner_gaps: Some(40),
                reserve_left: Some(100),
                reserve_right: Some(20),
                border_width: Some(9),
                ..Default::default()
            }),
        ),
    ];
    let mut engine = engine_with_config(config);
    let output = connect(&mut engine, "physical-one", "DISPLAY1");
    let fixed = WindowId::new(901);
    let proportional = WindowId::new(902);
    let ws = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&1)
        .unwrap();
    ws.add_window_to_new_column(fixed);
    ws.add_window_to_new_column(proportional);
    engine.remember_column_width_spec(fixed, None, None);
    let cfg = engine.effective_config_for_workspace(output, 1).clone();
    let border = 2 * cfg.border_width.max(1);
    let view = TilingEngine::view_width_for_monitor(engine.monitors.get(&output).unwrap(), &cfg);
    let ws = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&1)
        .unwrap();
    assert_eq!(ws.columns[0].width_fixed_logical, Some(200.0));
    assert_eq!(ws.columns[0].width, Some((200 + border) as u32));
    ws.columns[1].set_proportion(0.5, 100, 0);
    engine.monitors.get_mut(&output).unwrap().scale_factor = 1.5;
    engine.refresh_strip_widths(output);
    let monitor = engine.monitors.get(&output).unwrap();
    assert_eq!(monitor.active_workspace, 0);
    assert_eq!(
        monitor.workspaces[&1].columns[0].width,
        Some((300 + border) as u32)
    );
    assert_eq!(
        monitor.workspaces[&1].columns[1].width,
        Some(proportion_px(view, cfg.column_gap, 0.5) as u32)
    );
}

#[test]
fn minimized_only_named_workspace_preserves_empty_strip_metadata_on_migration() {
    let mut config = Config::default();
    config.workspace = vec![workspace_config("Code", &["DISPLAY2", "DISPLAY1"], None)];
    let mut engine = engine_with_config(config);
    let first = connect(&mut engine, "physical-one", "DISPLAY1");
    let [tiled, floating, minimized] = seed_contents(&mut engine, first, 0);
    engine.float_homes.remove(&floating);
    engine.floating_windows.remove(&floating);
    let strip = engine
        .monitors
        .get_mut(&first)
        .unwrap()
        .workspaces
        .get_mut(&0)
        .unwrap();
    strip.remove_window(tiled);
    strip.layout_mode = crate::layout::WorkspaceLayout::Spiral;
    strip.scroll_offset.x = 83;
    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    let strip = &engine.monitors.get(&second).unwrap().workspaces[&0];
    assert!(strip.columns.is_empty());
    assert_eq!(strip.layout_mode, crate::layout::WorkspaceLayout::Spiral);
    assert_eq!(strip.scroll_offset.x, 83);
    assert_eq!(engine.minimized[&minimized].output, second);
}

#[test]
fn workspace_layout_folds_global_then_output_then_workspace() {
    let mut config = Config::default();
    config.layout.column_width = 240;
    config.layout.inner_gaps = 3;
    config.layout.reserve_left = 17;
    config.layout.strip_frame = true;
    config.layout.always_center_single_column = true;
    config.layout.effects.focus_ring.width = 41;
    config.layout.effects.focus_ring.active = PaintStyle::color("#112233");
    let global_shadow = config.layout.effects.shadow.clone();

    let mut output_ring = RingStyle::focus_ring_default();
    output_ring.off = true;
    output_ring.width = 19;
    output_ring.active = PaintStyle::color("#445566");
    let mut output_layout = LayoutConfigPartial {
        column_width: Some(480),
        inner_gaps: Some(22),
        strip_frame: Some(false),
        always_center_single_column: Some(true),
        ..Default::default()
    };
    output_layout.effects.focus_ring = Some(output_ring.clone());
    config.output = vec![output_config("DISPLAY1", output_layout)];

    // A named nested block replaces the whole output block, not just its width.
    let mut workspace_ring = RingStyle::focus_ring_default();
    workspace_ring.width = 7;
    let mut workspace_layout = LayoutConfigPartial {
        column_width: Some(720),
        always_center_single_column: Some(false),
        ..Default::default()
    };
    workspace_layout.effects.focus_ring = Some(workspace_ring.clone());
    config.workspace = vec![workspace_config("Code", &[], Some(workspace_layout))];

    let mut engine = engine_with_config(config);
    let output = connect(&mut engine, "physical-one", r"\\.\DISPLAY1");
    let other = connect(&mut engine, "physical-other", "OTHER");
    let global = engine.effective_config_for_workspace(other, 99);
    assert_eq!(global.column_width, 240);
    assert_eq!(global.column_gap, 3);
    assert!(global.strip_frame);

    let per_output = engine.effective_config(output);
    assert_eq!(per_output.column_width, 480);
    assert_eq!(per_output.column_gap, 22);
    assert!(
        !per_output.strip_frame,
        "Some(false) must override global true"
    );
    assert!(per_output.always_center_single_column);
    assert_eq!(per_output.effects.focus_ring, output_ring);

    let per_workspace = engine.effective_config_for_workspace(output, 0);
    assert_eq!(per_workspace.column_width, 720);
    assert_eq!(per_workspace.column_gap, 22);
    assert_eq!(per_workspace.reserve_left, 17);
    assert!(!per_workspace.strip_frame);
    assert!(!per_workspace.always_center_single_column);
    assert_eq!(per_workspace.effects.focus_ring, workspace_ring);
    assert_eq!(per_workspace.effects.shadow, global_shadow);
    assert_eq!(engine.active_workspace_config(output).column_width, 720);
}

#[test]
fn repeated_output_preferences_choose_first_connected_then_stable_fallback() {
    let mut config = Config::default();
    config.workspace = vec![workspace_config(
        "Code",
        &["DISPLAY2", "DISPLAY1", "DISPLAY2"],
        None,
    )];
    let mut engine = engine_with_config(config);
    assert_eq!(engine.named_workspace_output(0), None);

    let first = connect(&mut engine, "physical-one", r"\\.\DISPLAY1");
    assert_eq!(engine.named_workspace_output(0), Some(first));
    let second = connect(&mut engine, "physical-two", r"\\.\display2");
    assert_eq!(engine.named_workspace_output(0), Some(second));
    let fallback = connect(&mut engine, "physical-three", "OTHER");
    engine.monitors.set_focused(fallback);

    engine.set_output_name(second, "RENAMED2");
    assert_eq!(engine.named_workspace_output(0), Some(first));
    engine.set_output_name(first, "RENAMED1");
    assert_eq!(
        engine.named_workspace_output(0),
        Some(first),
        "no matching preference keeps the still-connected owner, not the focused output"
    );
    engine.unregister_monitor(&first);
    assert_eq!(engine.named_workspace_output(0), Some(fallback));
}

#[test]
fn hotplug_rename_and_reload_migrate_all_named_workspace_contents() {
    let mut config = Config::default();
    config.output = vec![
        output_config(
            "DISPLAY1",
            LayoutConfigPartial {
                inner_gaps: Some(11),
                ..Default::default()
            },
        ),
        output_config(
            "DISPLAY2",
            LayoutConfigPartial {
                inner_gaps: Some(22),
                ..Default::default()
            },
        ),
    ];
    config.workspace = vec![workspace_config(
        "Code",
        &["DISPLAY2", "DISPLAY1"],
        Some(LayoutConfigPartial {
            column_width: Some(777),
            ..Default::default()
        }),
    )];
    let mut engine = engine_with_config(config.clone());
    let first = connect(&mut engine, "physical-one", "DISPLAY1");
    let windows = seed_contents(&mut engine, first, 0);
    assert_contents_on(&engine, first, 0, windows);

    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    assert_eq!(engine.named_workspace_output(0), Some(second));
    assert_contents_on(&engine, second, 0, windows);
    assert_eq!(engine.config_for_window(windows[0], second).column_gap, 22);

    engine.unregister_monitor(&second);
    assert_eq!(engine.named_workspace_output(0), Some(first));
    assert_contents_on(&engine, first, 0, windows);
    let reconnected = connect(&mut engine, "physical-two", "DISPLAY2");
    assert_eq!(reconnected, second);
    assert_eq!(engine.named_workspace_output(0), Some(second));
    assert_contents_on(&engine, second, 0, windows);

    engine.set_output_name(second, "RENAMED2");
    assert_eq!(engine.named_workspace_output(0), Some(first));
    assert_contents_on(&engine, first, 0, windows);

    config.workspace[0].open_on_outputs = vec!["RENAMED2".into(), "DISPLAY1".into()];
    config.workspace[0]
        .layout_override
        .as_mut()
        .unwrap()
        .column_width = Some(888);
    engine.set_full_config(config);
    assert_eq!(engine.named_workspace_output(0), Some(second));
    assert_contents_on(&engine, second, 0, windows);
    assert_eq!(
        engine.config_for_window(windows[0], second).column_width,
        888
    );
}

#[test]
fn no_preferences_is_unpinned_and_monitor_is_only_a_legacy_fallback() {
    let mut config = Config::default();
    let mut legacy = workspace_config("Legacy", &[], None);
    legacy.monitor = "DISPLAY2".into();
    let mut modern = workspace_config("Modern", &["DISPLAY1"], None);
    modern.monitor = "DISPLAY2".into();
    let mut offline = workspace_config("Offline", &["DISCONNECTED"], None);
    offline.monitor = "DISPLAY2".into();
    config.workspace = vec![
        workspace_config("Unpinned", &[], None),
        legacy,
        modern,
        offline,
    ];
    let mut engine = engine_with_config(config.clone());
    let first = connect(&mut engine, "physical-one", "DISPLAY1");
    let windows = seed_contents(&mut engine, first, 0);
    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    assert_eq!(engine.named_workspace_output(0), None);
    assert_contents_on(&engine, first, 0, windows);
    assert_eq!(engine.named_workspace_output(1), Some(second));
    assert_eq!(engine.named_workspace_output(2), Some(first));
    assert_eq!(
        engine.named_workspace_output(3),
        Some(first),
        "a nonempty preference list must not fall through to legacy monitor"
    );

    config.workspace[1].monitor.clear();
    config.workspace[2].open_on_outputs.clear();
    engine.set_full_config(config);
    assert_eq!(engine.named_workspace_output(0), None);
    assert_eq!(engine.named_workspace_output(1), None);
    assert_eq!(engine.named_workspace_output(2), Some(second));
    assert_contents_on(&engine, first, 0, windows);
}

#[test]
fn window_config_uses_actual_inactive_tiled_floating_and_minimized_homes() {
    let mut config = Config::default();
    config.output = vec![
        output_config(
            "DISPLAY1",
            LayoutConfigPartial {
                inner_gaps: Some(11),
                ..Default::default()
            },
        ),
        output_config(
            "DISPLAY2",
            LayoutConfigPartial {
                inner_gaps: Some(22),
                ..Default::default()
            },
        ),
    ];
    config.workspace = ["Active", "Tiled", "Floating", "Minimized"]
        .iter()
        .zip([101, 202, 303, 404])
        .map(|(name, width)| {
            workspace_config(
                name,
                &[],
                Some(LayoutConfigPartial {
                    column_width: Some(width),
                    ..Default::default()
                }),
            )
        })
        .collect();
    let mut engine = engine_with_config(config);
    let hint = connect(&mut engine, "physical-one", "DISPLAY1");
    let actual = connect(&mut engine, "physical-two", "DISPLAY2");
    let [tiled, floating, minimized] = seed_contents(&mut engine, actual, 1);
    engine.float_homes.get_mut(&floating).unwrap().workspace = 2;
    engine.minimized.get_mut(&minimized).unwrap().workspace = 3;
    engine.monitors.get_mut(&hint).unwrap().active_workspace = 0;
    engine.monitors.get_mut(&actual).unwrap().active_workspace = 0;
    engine.monitors.set_focused(hint);

    for (window, width) in [(tiled, 202), (floating, 303), (minimized, 404)] {
        // The supplied output is only a fallback, not the window's actual home.
        let effective = engine.config_for_window(window, hint);
        assert_eq!(effective.column_width, width);
        assert_eq!(effective.column_gap, 22);
    }
    assert_eq!(engine.active_workspace_config(actual).column_width, 101);
    let unknown = engine.config_for_window(WindowId::new(999), hint);
    assert_eq!(unknown.column_width, 101);
    assert_eq!(unknown.column_gap, 11);
}

#[test]
fn single_output_workspace_name_swap_moves_layout_overrides_by_name() {
    let mut config = Config::default();
    config.workspace = vec![
        workspace_config(
            "Code",
            &[],
            Some(LayoutConfigPartial {
                column_width: Some(640),
                strip_frame: Some(false),
                ..Default::default()
            }),
        ),
        workspace_config(
            "Web",
            &[],
            Some(LayoutConfigPartial {
                column_width: Some(960),
                strip_frame: Some(true),
                ..Default::default()
            }),
        ),
    ];
    let mut engine = engine_with_config(config);
    let output = connect(&mut engine, "physical-one", "DISPLAY1");
    assert_eq!(
        engine
            .effective_config_for_workspace(output, 0)
            .column_width,
        640
    );
    assert_eq!(
        engine
            .effective_config_for_workspace(output, 1)
            .column_width,
        960
    );

    // This is the pure name/home bookkeeping used by workspace reordering.
    engine.swap_workspace_ties(output, 0, 1);
    assert_eq!(engine.workspace_name(0).as_deref(), Some("Web"));
    assert_eq!(engine.workspace_name(1).as_deref(), Some("Code"));
    assert_eq!(engine.workspace_id_by_name("Code"), Some(1));
    assert_eq!(engine.workspace_id_by_name("Web"), Some(0));
    let web = engine.effective_config_for_workspace(output, 0);
    assert_eq!(web.column_width, 960);
    assert!(web.strip_frame);
    let code = engine.effective_config_for_workspace(output, 1);
    assert_eq!(code.column_width, 640);
    assert!(!code.strip_frame);
    assert_eq!(engine.active_workspace_config(output).column_width, 960);
}

#[test]
fn update_config_refreshes_cached_reservations_but_keeps_explicit_overrides() {
    let mut config = Config::default();
    config.layout.reserve_top = 11;
    config.layout.column_width = 240;
    config.output = vec![
        output_config(
            "DISPLAY1",
            LayoutConfigPartial {
                column_width: Some(480),
                ..Default::default()
            },
        ),
        output_config(
            "DISPLAY2",
            LayoutConfigPartial {
                column_width: Some(600),
                reserve_top: Some(31),
                ..Default::default()
            },
        ),
    ];
    config.workspace = vec![
        workspace_config(
            "Inherited",
            &[],
            Some(LayoutConfigPartial {
                inner_gaps: Some(13),
                ..Default::default()
            }),
        ),
        workspace_config(
            "Explicit",
            &[],
            Some(LayoutConfigPartial {
                column_width: Some(720),
                reserve_top: Some(51),
                ..Default::default()
            }),
        ),
    ];
    let mut engine = engine_with_config(config);
    let first = connect(&mut engine, "physical-one", "DISPLAY1");
    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    assert_eq!(engine.effective_config(first).reserve_top, 11);
    assert_eq!(
        engine.effective_config_for_workspace(first, 0).reserve_top,
        11
    );
    assert_eq!(engine.effective_config(second).reserve_top, 31);
    assert_eq!(
        engine.effective_config_for_workspace(first, 1).reserve_top,
        51
    );

    let mut updated = engine.config.clone();
    updated.reserve_top = 99;
    engine.update_config(updated);

    assert_eq!(
        engine
            .effective_config(OutputId::from_name("unconfigured"))
            .reserve_top,
        99
    );
    assert_eq!(engine.effective_config(first).reserve_top, 99);
    assert_eq!(engine.effective_config(first).column_width, 480);
    let inherited = engine.effective_config_for_workspace(first, 0);
    assert_eq!(inherited.reserve_top, 99);
    assert_eq!(inherited.column_width, 480);
    assert_eq!(inherited.column_gap, 13);
    assert_eq!(engine.active_workspace_config(first).reserve_top, 99);
    assert_eq!(engine.effective_config(second).reserve_top, 31);
    let output_override = engine.effective_config_for_workspace(second, 0);
    assert_eq!(output_override.reserve_top, 31);
    assert_eq!(output_override.column_width, 600);
    for output in [first, second] {
        let explicit = engine.effective_config_for_workspace(output, 1);
        assert_eq!(explicit.reserve_top, 51);
        assert_eq!(explicit.column_width, 720);
    }
}
