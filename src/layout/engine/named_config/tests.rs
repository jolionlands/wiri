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

/// Mirrors the order of `backend::output_runtime::sync`: register, name, then realize
/// the declarations whose connector names are now known.
fn connect(engine: &mut TilingEngine, identity: &str, name: &str) -> OutputId {
    // Equal work areas keep rect fitting out of these ownership/configuration tests.
    // IDs deliberately differ from names: matching must use set_output_name, too.
    let output = OutputId::from_name(identity);
    let area = Rect::new(0, 0, 1920, 1080);
    engine.register_monitor(output, area, area);
    engine.set_output_name(output, name);
    engine.ensure_named_workspaces();
    output
}

/// Workspace identities are stable objects: find a live named one wherever it is.
fn named_workspace(engine: &TilingEngine, name: &str) -> Option<(OutputId, i32)> {
    let anywhere = engine.monitors.focused_id()?;
    engine.resolve_workspace_reference(anywhere, &WorkspaceRef::Name(name.to_string()))
}

fn named_output(engine: &TilingEngine, name: &str) -> Option<OutputId> {
    named_workspace(engine, name).map(|(output, _)| output)
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

    let width_preset_list = engine.column_presets_for_workspace(output, workspace);
    engine.floating_windows.insert(floating);
    engine.float_homes.insert(
        floating,
        FloatHome {
            output,
            workspace,
            rect: Rect::new(40, 60, 320, 240),
            position_history: crate::layout::floating_position::PositionHistory::new(
                Rect::new(40, 60, 320, 240),
                None,
            ),
            seq: 7,
            width_preset_idx: Some(1),
            width_preset_list,
            height_preset_idx: None,
            height_preset_list: Vec::new(),
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

/// The contents sit, exactly once and unchanged, in workspace `workspace` of `output`.
fn assert_contents_at(
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

/// The contents sit in the live workspace called `name`, which is on `output`.
fn assert_contents_on(
    engine: &TilingEngine,
    output: OutputId,
    name: &str,
    windows: [WindowId; 3],
) -> i32 {
    let (actual_output, workspace) = named_workspace(engine, name).unwrap();
    assert_eq!(
        actual_output, output,
        "workspace {name:?} is on another output"
    );
    assert_contents_at(engine, output, workspace, windows);
    workspace
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
                outer_gaps: Some(30.0),
                inner_gaps: Some(40.0),
                struts: crate::config::Edges {
                    left: Some(crate::config::InsetValue::Physical(100)),
                    right: Some(crate::config::InsetValue::Physical(20)),
                    ..Default::default()
                },
                border_width: Some(9),
                ..Default::default()
            }),
        ),
    ];
    let mut engine = engine_with_config(config);
    let output = connect(&mut engine, "physical-one", "DISPLAY1");
    let (inactive_output, inactive) = named_workspace(&engine, "Inactive").unwrap();
    assert_eq!(inactive_output, output);
    assert_eq!(named_workspace(&engine, "Active"), Some((output, 0)));
    let fixed = WindowId::new(901);
    let proportional = WindowId::new(902);
    let ws = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&inactive)
        .unwrap();
    ws.add_window_to_new_column(fixed);
    ws.add_window_to_new_column(proportional);
    engine.remember_column_width_spec(fixed, None, None);
    let cfg = engine
        .effective_config_for_workspace(output, inactive)
        .clone();
    let border = 2 * cfg.border_width.max(1);
    let view = TilingEngine::view_width_for_monitor(engine.monitors.get(&output).unwrap(), &cfg);
    let ws = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&inactive)
        .unwrap();
    assert_eq!(ws.columns[0].width_fixed_logical, Some(200.0));
    assert_eq!(ws.columns[0].width, Some((200 + border) as u32));
    ws.columns[1].set_proportion(0.5, 100, 0);
    engine.monitors.get_mut(&output).unwrap().scale_factor = 1.5;
    engine.refresh_strip_widths(output);
    let monitor = engine.monitors.get(&output).unwrap();
    assert_eq!(monitor.active_workspace, 0);
    assert_eq!(
        monitor.workspaces[&inactive].columns[0].width,
        Some((300 + border) as u32)
    );
    assert_eq!(
        monitor.workspaces[&inactive].columns[1].width,
        Some(proportion_px(view, cfg.column_gap, 0.5) as u32)
    );
}

#[test]
fn minimized_only_named_workspace_preserves_empty_strip_metadata_on_migration() {
    let mut config = Config::default();
    config.workspace = vec![workspace_config("Code", &["DISPLAY2", "DISPLAY1"], None)];
    let mut engine = engine_with_config(config);
    let first = connect(&mut engine, "physical-one", "DISPLAY1");
    let (home, key) = named_workspace(&engine, "Code").unwrap();
    assert_eq!(
        home, first,
        "DISPLAY2 is absent, so the second preference hosts it"
    );
    let [tiled, floating, minimized] = seed_contents(&mut engine, first, key);
    engine.float_homes.remove(&floating);
    engine.floating_windows.remove(&floating);
    let strip = engine
        .monitors
        .get_mut(&first)
        .unwrap()
        .workspaces
        .get_mut(&key)
        .unwrap();
    strip.remove_window(tiled);
    strip.layout_mode = crate::layout::WorkspaceLayout::Spiral;
    strip.scroll_offset.x = 83;
    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    let (owner, moved) = named_workspace(&engine, "Code").unwrap();
    assert_eq!(owner, second);
    let strip = &engine.monitors.get(&second).unwrap().workspaces[&moved];
    assert!(strip.columns.is_empty());
    assert_eq!(strip.layout_mode, crate::layout::WorkspaceLayout::Spiral);
    assert_eq!(strip.scroll_offset.x, 83);
    assert_eq!(engine.minimized[&minimized].output, second);
    assert_eq!(engine.minimized[&minimized].workspace, moved);
}

#[test]
fn workspace_layout_folds_global_then_output_then_workspace() {
    let mut config = Config::default();
    config.layout.column_width = 240;
    config.layout.inner_gaps = 3.0;
    config.layout.struts.left = crate::config::InsetValue::Physical(17);
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
        inner_gaps: Some(22.0),
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

    let (code_output, code) = named_workspace(&engine, "Code").unwrap();
    assert_eq!(code_output, output);
    let per_workspace = engine.effective_config_for_workspace(output, code);
    assert_eq!(per_workspace.column_width, 720);
    assert_eq!(per_workspace.column_gap, 22);
    assert_eq!(per_workspace.reserve_left, 17);
    assert!(!per_workspace.strip_frame);
    assert!(!per_workspace.always_center_single_column);
    assert_eq!(per_workspace.effects.focus_ring, workspace_ring);
    assert_eq!(per_workspace.effects.shadow, global_shadow);
    assert_eq!(engine.active_workspace_config(output).column_width, 720);
    // The same name viewed from another output folds that output's (absent) block.
    let unnamed = engine.effective_config_for_workspace(other, 0);
    assert_eq!(unnamed.column_width, 240);
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
    assert_eq!(named_output(&engine, "Code"), None);

    let first = connect(&mut engine, "physical-one", r"\\.\DISPLAY1");
    assert_eq!(named_output(&engine, "Code"), Some(first));
    let second = connect(&mut engine, "physical-two", r"\\.\display2");
    assert_eq!(named_output(&engine, "Code"), Some(second));
    let fallback = connect(&mut engine, "physical-three", "OTHER");
    engine.monitors.set_focused(fallback);

    engine.set_output_name(second, "RENAMED2");
    assert_eq!(named_output(&engine, "Code"), Some(first));
    engine.set_output_name(first, "RENAMED1");
    assert_eq!(
        named_output(&engine, "Code"),
        Some(first),
        "no matching preference keeps the still-connected owner, not the focused output"
    );
    engine.unregister_monitor(&first);
    assert_eq!(named_output(&engine, "Code"), Some(fallback));
}

#[test]
fn hotplug_rename_and_reload_migrate_all_named_workspace_contents() {
    let mut config = Config::default();
    config.output = vec![
        output_config(
            "DISPLAY1",
            LayoutConfigPartial {
                inner_gaps: Some(11.0),
                ..Default::default()
            },
        ),
        output_config(
            "DISPLAY2",
            LayoutConfigPartial {
                inner_gaps: Some(22.0),
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
    let (_, key) = named_workspace(&engine, "Code").unwrap();
    let windows = seed_contents(&mut engine, first, key);
    assert_contents_on(&engine, first, "Code", windows);

    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    assert_contents_on(&engine, second, "Code", windows);
    assert_eq!(engine.config_for_window(windows[0], second).column_gap, 22);
    assert_eq!(
        engine.config_for_window(windows[0], second).column_width,
        777
    );

    engine.unregister_monitor(&second);
    assert_contents_on(&engine, first, "Code", windows);
    let reconnected = connect(&mut engine, "physical-two", "DISPLAY2");
    assert_eq!(reconnected, second);
    assert_contents_on(&engine, second, "Code", windows);

    engine.set_output_name(second, "RENAMED2");
    assert_contents_on(&engine, first, "Code", windows);

    config.workspace[0].open_on_outputs = vec!["RENAMED2".into(), "DISPLAY1".into()];
    config.workspace[0]
        .layout_override
        .as_mut()
        .unwrap()
        .column_width = Some(888);
    engine.set_full_config(config);
    // Lane F's contract: a reload never re-places a realized declaration. It does
    // rebuild the layout view, so the new block already applies where the workspace is.
    assert_contents_on(&engine, first, "Code", windows);
    assert_eq!(
        engine.config_for_window(windows[0], first).column_width,
        888
    );
    assert_eq!(engine.config_for_window(windows[0], first).column_gap, 11);
    // The next output event reevaluates the reloaded preference list.
    engine.set_output_name(second, "RENAMED2");
    assert_contents_on(&engine, second, "Code", windows);
    assert_eq!(
        engine.config_for_window(windows[0], second).column_width,
        888
    );
    // The `DISPLAY2` output block no longer matches the renamed output.
    assert_ne!(engine.config_for_window(windows[0], second).column_gap, 22);
}

#[test]
fn no_preferences_is_unpinned_and_monitor_is_the_last_fallback_preference() {
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
    let (_, unpinned) = named_workspace(&engine, "Unpinned").unwrap();
    let windows = seed_contents(&mut engine, first, unpinned);
    // Legacy waits for DISPLAY2; Modern prefers DISPLAY1, which is connected.
    assert_eq!(named_output(&engine, "Legacy"), None);
    assert_eq!(named_output(&engine, "Modern"), Some(first));
    assert_eq!(named_output(&engine, "Offline"), None);

    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    assert_contents_on(&engine, first, "Unpinned", windows);
    assert_eq!(named_output(&engine, "Legacy"), Some(second));
    assert_eq!(named_output(&engine, "Modern"), Some(first));
    assert_eq!(
        named_output(&engine, "Offline"),
        Some(second),
        "the legacy monitor is the last entry of the preference list"
    );

    // Without preferences a workspace is unpinned and stays where it is; with the
    // `open-on-output` list gone, `monitor` alone is Modern's list. A reload alone
    // re-places nothing (Lane F); the next output event reevaluates the new lists.
    config.workspace[1].monitor.clear();
    config.workspace[2].open_on_outputs.clear();
    engine.set_full_config(config);
    assert_eq!(named_output(&engine, "Modern"), Some(first));
    engine.set_output_name(second, "DISPLAY2");
    assert_contents_on(&engine, first, "Unpinned", windows);
    assert_eq!(named_output(&engine, "Legacy"), Some(second));
    assert_eq!(named_output(&engine, "Modern"), Some(second));
}

#[test]
fn window_config_uses_actual_inactive_tiled_floating_and_minimized_homes() {
    let mut config = Config::default();
    config.output = vec![
        output_config(
            "DISPLAY1",
            LayoutConfigPartial {
                inner_gaps: Some(11.0),
                ..Default::default()
            },
        ),
        output_config(
            "DISPLAY2",
            LayoutConfigPartial {
                inner_gaps: Some(22.0),
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
                &["DISPLAY2"],
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
    let key = |engine: &TilingEngine, name: &str| {
        let (output, key) = named_workspace(engine, name).unwrap();
        assert_eq!(output, actual);
        key
    };
    let (active, tiled_ws) = (key(&engine, "Active"), key(&engine, "Tiled"));
    let (floating_ws, minimized_ws) = (key(&engine, "Floating"), key(&engine, "Minimized"));
    let [tiled, floating, minimized] = seed_contents(&mut engine, actual, tiled_ws);
    engine.float_homes.get_mut(&floating).unwrap().workspace = floating_ws;
    engine.minimized.get_mut(&minimized).unwrap().workspace = minimized_ws;
    engine.monitors.get_mut(&hint).unwrap().active_workspace = 0;
    engine.monitors.get_mut(&actual).unwrap().active_workspace = active;
    engine.monitors.set_focused(hint);

    for (window, width) in [(tiled, 202), (floating, 303), (minimized, 404)] {
        // The supplied output is only a fallback, not the window's actual home.
        let effective = engine.config_for_window(window, hint);
        assert_eq!(effective.column_width, width);
        assert_eq!(effective.column_gap, 22);
    }
    assert_eq!(engine.active_workspace_config(actual).column_width, 101);
    // An unknown window falls back to the supplied output's active (unnamed) workspace.
    let unknown = engine.config_for_window(WindowId::new(999), hint);
    assert_eq!(
        unknown.column_width,
        engine.effective_config(hint).column_width
    );
    assert_ne!(unknown.column_width, 101);
    assert_eq!(unknown.column_gap, 11);
}

#[test]
fn layout_overrides_follow_workspace_names_not_positions() {
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
    assert_eq!(named_workspace(&engine, "Code"), Some((output, 0)));
    assert_eq!(named_workspace(&engine, "Web"), Some((output, 1)));
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

    // Trade the two names through a temporary one: the layouts trade places with them.
    engine.rename_workspace(1, "Tmp").unwrap();
    assert_eq!(
        engine
            .effective_config_for_workspace(output, 1)
            .column_width,
        engine.effective_config(output).column_width,
        "a name with no workspace block has no override"
    );
    engine.rename_workspace(0, "Web").unwrap();
    engine.rename_workspace(1, "Code").unwrap();
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
fn layout_overrides_survive_reordering_and_moving_to_another_output() {
    let mut config = Config::default();
    config.output = vec![output_config(
        "DISPLAY2",
        LayoutConfigPartial {
            inner_gaps: Some(22.0),
            ..Default::default()
        },
    )];
    config.workspace = vec![workspace_config(
        "Code",
        &[],
        Some(LayoutConfigPartial {
            column_width: Some(640),
            ..Default::default()
        }),
    )];
    let mut engine = engine_with_config(config);
    let first = connect(&mut engine, "physical-one", "DISPLAY1");
    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    let (output, key) = named_workspace(&engine, "Code").unwrap();
    assert_eq!((output, key), (first, 0));

    // A user move to another output renumbers the local key; the layout follows the name.
    let key = engine.relocate_workspace(first, key, second, None).unwrap();
    assert_eq!(named_workspace(&engine, "Code"), Some((second, key)));
    let moved = engine.effective_config_for_workspace(second, key);
    assert_eq!(moved.column_width, 640);
    assert_eq!(moved.column_gap, 22, "the new output's block is folded in");
    // The vacated key on the old output no longer carries the override.
    assert_ne!(
        engine.effective_config_for_workspace(first, 0).column_width,
        640
    );
}

#[test]
fn runtime_reservations_refresh_cached_views_but_keep_explicit_overrides() {
    let mut config = Config::default();
    config.layout.struts.top = crate::config::InsetValue::Physical(11);
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
                struts: crate::config::Edges {
                    top: Some(crate::config::InsetValue::Physical(31)),
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
    ];
    config.workspace = vec![
        workspace_config(
            "Inherited",
            &["DISPLAY1"],
            Some(LayoutConfigPartial {
                inner_gaps: Some(13.0),
                ..Default::default()
            }),
        ),
        workspace_config(
            "Explicit",
            &["DISPLAY2"],
            Some(LayoutConfigPartial {
                column_width: Some(720),
                struts: crate::config::Edges {
                    top: Some(crate::config::InsetValue::Physical(51)),
                    ..Default::default()
                },
                ..Default::default()
            }),
        ),
    ];
    let mut engine = engine_with_config(config);
    let first = connect(&mut engine, "physical-one", "DISPLAY1");
    let second = connect(&mut engine, "physical-two", "DISPLAY2");
    let (inherited_output, inherited_key) = named_workspace(&engine, "Inherited").unwrap();
    let (explicit_output, explicit_key) = named_workspace(&engine, "Explicit").unwrap();
    assert_eq!((inherited_output, explicit_output), (first, second));
    assert_eq!(engine.effective_config(first).reserve_top, 11);
    assert_eq!(
        engine
            .effective_config_for_workspace(first, inherited_key)
            .reserve_top,
        11
    );
    assert_eq!(engine.effective_config(second).reserve_top, 31);
    assert_eq!(
        engine
            .effective_config_for_workspace(second, explicit_key)
            .reserve_top,
        51
    );

    assert!(engine.set_runtime_reservation("top", 99));

    assert_eq!(
        engine
            .effective_config(OutputId::from_name("unconfigured"))
            .reserve_top,
        99
    );
    assert_eq!(engine.effective_config(first).reserve_top, 99);
    assert_eq!(engine.effective_config(first).column_width, 480);
    let inherited = engine.effective_config_for_workspace(first, inherited_key);
    assert_eq!(inherited.reserve_top, 99);
    assert_eq!(inherited.column_width, 480);
    assert_eq!(inherited.column_gap, 13);
    assert_eq!(engine.effective_config(second).reserve_top, 31);
    // An unnamed workspace of the second output keeps that output's explicit block.
    let trailing = engine
        .monitors
        .get(&second)
        .unwrap()
        .workspace_ids()
        .into_iter()
        .find(|id| *id != explicit_key)
        .unwrap();
    let output_override = engine.effective_config_for_workspace(second, trailing);
    assert_eq!(output_override.reserve_top, 31);
    assert_eq!(output_override.column_width, 600);
    let explicit = engine.effective_config_for_workspace(second, explicit_key);
    assert_eq!(explicit.reserve_top, 51);
    assert_eq!(explicit.column_width, 720);
}

// niri e19df12: Options::adjusted_for_scale rounds gaps on the physical
// pixel grid; scrolling::compute_working_area applies signed logical struts
// before ceiling its origin. These fixtures need no HWNDs or native queries.
fn parity_spacing_engine(config: Config, scale: f64) -> (TilingEngine, OutputId) {
    assert!(
        config.layout.effects.border.off,
        "fixture explicitly disables borders"
    );
    assert_eq!(
        config.layout.border_width, 0,
        "parsed legacy border cache is zero"
    );
    assert!(config.layout.effects.focus_ring.off);
    let mut engine = engine_with_config(config);
    let output = OutputId::from_name("parity-spacing-output");
    let area = Rect::new(-300, 0, 900, 600);
    engine.register_monitor_with_scale(output, area, area, scale);
    engine.set_output_name(output, "DISPLAY1");
    engine.ensure_named_workspaces();
    let monitor = engine.monitors.get(&output).unwrap();
    assert_eq!(
        monitor.work_area, area,
        "fixture keeps physical work bounds"
    );
    assert_eq!(
        monitor.scale_factor, scale,
        "fixture installs the requested scale"
    );
    (engine, output)
}

fn parity_spacing_work_rect(engine: &TilingEngine, output: OutputId, workspace: i32) -> Rect {
    let monitor = engine.monitors.get(&output).unwrap();
    assert!(monitor.workspaces.contains_key(&workspace));
    TilingEngine::tiling_work_rect(
        monitor,
        engine.effective_config_for_workspace(output, workspace),
    )
}

fn parity_spacing_seed_windows(
    engine: &mut TilingEngine,
    output: OutputId,
    workspace: i32,
    first_id: isize,
) -> [WindowId; 3] {
    let windows = [
        WindowId::new(first_id),
        WindowId::new(first_id + 1),
        WindowId::new(first_id + 2),
    ];
    let strip = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&workspace)
        .unwrap();
    assert!(strip.columns.is_empty(), "seed only a fresh workspace");
    let left = strip.add_window_to_new_column(windows[0]);
    assert!(strip.add_window_to_column(left, windows[1]));
    strip.add_window_to_new_column(windows[2]);
    strip.active_window = Some(windows[0]);
    for window in windows {
        // Populate the existing snapshot registry directly. Admission would
        // query native HWND constraints, which these pure fixtures must avoid.
        engine.tiled_windows.insert(
            window,
            WindowInfo {
                hwnd: window.as_isize(),
                title: format!("Spacing fixture {}", window.as_isize()),
                class_name: "SpacingParityFixture".to_string(),
                process_id: 1,
                bounds: Rect::new(20, 30, 111, 77),
                state: crate::backend::WindowState::Normal,
                is_visible: true,
            },
        );
    }
    windows
}

fn parity_spacing_assert_placement_and_ipc(
    engine: &TilingEngine,
    output: OutputId,
    workspace: i32,
    windows: [WindowId; 3],
    expected_gap: i32,
) {
    let monitor = engine.monitors.get(&output).unwrap();
    let layout = engine.effective_config_for_workspace(output, workspace);
    assert_eq!(
        layout.border_inset(),
        0,
        "the placement fixture has no border inset"
    );
    let work = parity_spacing_work_rect(engine, output, workspace);
    // This calculator also supplies the native layout pass. Observe relative
    // gaps without prescribing how available width/height is allocated.
    let positions: HashMap<_, _> = engine
        .calculate_positions(&monitor.workspaces[&workspace], work, layout)
        .into_iter()
        .collect();
    assert_eq!(positions.len(), 3, "all three fake windows are laid out");
    let top = positions[&windows[0]];
    let bottom = positions[&windows[1]];
    let right = positions[&windows[2]];
    assert_eq!(
        top.loc, work.loc,
        "the first window starts at the work origin"
    );
    assert_eq!(bottom.loc.x, top.loc.x);
    assert_eq!(bottom.size.w, top.size.w);
    assert_eq!(right.loc.y, work.loc.y);
    assert_eq!(right.loc.x - top.loc.x - top.size.w as i32, expected_gap);
    assert_eq!(bottom.loc.y - top.loc.y - top.size.h as i32, expected_gap);
    assert_eq!(
        bottom.loc.y + bottom.size.h as i32,
        work.loc.y + work.size.h as i32
    );

    let snapshot = engine.ipc_snapshot();
    let scale = monitor.scale_factor;
    let close = |actual: f64, expected: f64| {
        assert!((actual - expected).abs() < 1e-7, "{actual} != {expected}");
    };
    for (window, slot) in windows.into_iter().zip([(1, 1), (1, 2), (2, 1)]) {
        let rect = positions[&window];
        assert!(rect.size.w > 0 && rect.size.h > 0);
        let reported = &snapshot
            .windows
            .iter()
            .find(|reported| reported.id == window.as_isize() as u64)
            .expect("the seeded window is present in IPC")
            .layout;
        assert_eq!(reported.pos_in_scrolling_layout, Some(slot));
        assert_eq!(reported.window_offset_in_tile, (0.0, 0.0));
        let (x, y) = reported.tile_pos_in_workspace_view.unwrap();
        close(x * scale + monitor.bounds.loc.x as f64, rect.loc.x as f64);
        close(y * scale + monitor.bounds.loc.y as f64, rect.loc.y as f64);
        close(reported.tile_size.0 * scale, rect.size.w as f64);
        close(reported.tile_size.1 * scale, rect.size.h as f64);
        // niri's integer logical window_size can differ by half a logical
        // pixel from its floating-point tile_size; no border separates them.
        assert!((reported.window_size.0 as f64 - rect.size.w as f64 / scale).abs() <= 0.5);
        assert!((reported.window_size.1 as f64 - rect.size.h as f64 / scale).abs() <= 0.5);
    }
}

#[test]
fn parity_fractional_spacing_gap_geometry_uses_output_scale_and_minimum_pixel() {
    use crate::config::parse::parse_kdl_config;

    for (gap, scale, physical_gap) in [
        ("0", 1.0, 0),
        ("0", 1.5, 0),
        ("2.5", 1.0, 3),
        ("2.5", 1.5, 4),
        ("0.1", 1.0, 1),
        ("0.1", 1.5, 1),
    ] {
        let config = parse_kdl_config(&format!(
            "layout {{ gaps {gap}; border {{ off; }}\nfocus-ring {{ off; }}\n}}\n"
        ))
        .expect("the gap fixture parses");
        let (mut engine, output) = parity_spacing_engine(config, scale);
        let workspace = engine.monitors.get(&output).unwrap().active_workspace;
        let windows = parity_spacing_seed_windows(&mut engine, output, workspace, 31_000);
        let expected = Rect::new(
            -300 + physical_gap,
            physical_gap,
            (900 - 2 * physical_gap) as u32,
            (600 - 2 * physical_gap) as u32,
        );
        assert_eq!(
            parity_spacing_work_rect(&engine, output, workspace),
            expected,
            "logical gap {gap} at scale {scale}: outer geometry rounds after scaling"
        );
        parity_spacing_assert_placement_and_ipc(&engine, output, workspace, windows, physical_gap);
    }
}

#[test]
fn parity_fractional_spacing_signed_strut_ceils_negative_origin_after_scaling() {
    use crate::config::parse::parse_kdl_config;

    for (scale, expected) in [
        (1.0, Rect::new(-304, 0, 904, 600)),
        (1.5, Rect::new(-306, 0, 906, 600)),
    ] {
        let config = parse_kdl_config(
            "layout { gaps 0; struts { left -4.5; }\nborder { off; }\nfocus-ring { off; }\n}\n",
        )
        .expect("the signed fractional strut fixture parses");
        let (mut engine, output) = parity_spacing_engine(config, scale);
        let workspace = engine.monitors.get(&output).unwrap().active_workspace;
        let windows = parity_spacing_seed_windows(&mut engine, output, workspace, 32_000);
        assert_eq!(
            parity_spacing_work_rect(&engine, output, workspace),
            expected,
            "scale {scale}: apply the logical strut, then ceil the physical origin"
        );
        parity_spacing_assert_placement_and_ipc(&engine, output, workspace, windows, 0);
    }
}

#[test]
fn parity_fractional_spacing_reload_and_scale_round_trip_keep_override_sources() {
    use crate::config::parse::parse_kdl_config;

    let initial = parse_kdl_config(
        "layout { gaps 2; border { off; }\nfocus-ring { off; }\n}\n\
         workspace \"Code\"\nworkspace \"OutputOnly\"\n",
    )
    .expect("the initial named-workspace fixture parses");
    let (mut engine, output) = parity_spacing_engine(initial, 1.0);
    let (_, workspace) = named_workspace(&engine, "Code").unwrap();
    let (_, output_only) = named_workspace(&engine, "OutputOnly").unwrap();
    let windows = parity_spacing_seed_windows(&mut engine, output, workspace, 33_000);
    let output_windows = parity_spacing_seed_windows(&mut engine, output, output_only, 34_000);
    assert_eq!(
        parity_spacing_work_rect(&engine, output, workspace),
        Rect::new(-298, 2, 896, 596),
        "integer setup control before the reload"
    );
    parity_spacing_assert_placement_and_ipc(&engine, output, workspace, windows, 2);
    parity_spacing_assert_placement_and_ipc(&engine, output, output_only, output_windows, 2);

    let reloaded = parse_kdl_config(
        r#"
layout { gaps 9; border { off; }
focus-ring { off; }
}
output "DISPLAY1" { layout { gaps 2.5; }
}
workspace "Code" { layout { gaps 3.5; }
}
workspace "OutputOnly"
"#,
    )
    .expect("the replacement config parses");
    // Same existing entry points as main::apply_loaded_config; output sync is
    // represented below by monitor sync followed by its output-name event.
    engine.update_config(LayoutConfig::from_config(&reloaded));
    engine.set_full_config(reloaded);
    assert_eq!(named_workspace(&engine, "Code"), Some((output, workspace)));

    for (scale, output_gap, workspace_gap) in [(1.0, 3, 4), (1.5, 4, 5), (1.0, 3, 4)] {
        let area = Rect::new(-300, 0, 900, 600);
        engine.sync_monitor_with_scale(output, area, area, scale);
        engine.set_output_name(output, "DISPLAY1");
        assert_eq!(engine.monitors.get(&output).unwrap().scale_factor, scale);
        let expected = Rect::new(
            -300 + workspace_gap,
            workspace_gap,
            (900 - 2 * workspace_gap) as u32,
            (600 - 2 * workspace_gap) as u32,
        );
        assert_eq!(
            parity_spacing_work_rect(&engine, output, workspace),
            expected,
            "reload and scale {scale} must project the logical workspace override afresh"
        );
        parity_spacing_assert_placement_and_ipc(&engine, output, workspace, windows, workspace_gap);
        let output_expected = Rect::new(
            -300 + output_gap,
            output_gap,
            (900 - 2 * output_gap) as u32,
            (600 - 2 * output_gap) as u32,
        );
        assert_eq!(
            parity_spacing_work_rect(&engine, output, output_only),
            output_expected,
            "the output-only workspace projects its inherited source at scale {scale}"
        );
        parity_spacing_assert_placement_and_ipc(
            &engine,
            output,
            output_only,
            output_windows,
            output_gap,
        );
    }
}

#[test]
fn parity_fractional_spacing_runtime_reservation_stays_physical_across_scale_changes() {
    use crate::config::parse::parse_kdl_config;

    let config = parse_kdl_config("layout { gaps 0; border { off; }\nfocus-ring { off; }\n}\n")
        .expect("the physical-reservation control parses");
    let (mut engine, output) = parity_spacing_engine(config, 1.0);
    let workspace = engine.monitors.get(&output).unwrap().active_workspace;
    let windows = parity_spacing_seed_windows(&mut engine, output, workspace, 35_000);
    assert_eq!(
        parity_spacing_work_rect(&engine, output, workspace),
        Rect::new(-300, 0, 900, 600),
        "the control starts with no spacing or reservations"
    );
    parity_spacing_assert_placement_and_ipc(&engine, output, workspace, windows, 0);

    // The ReserveArea adapter keeps physical intent independent of DPI views.
    assert!(engine.set_runtime_reservation("left", 10));
    for scale in [1.0, 1.5, 1.0] {
        let area = Rect::new(-300, 0, 900, 600);
        engine.sync_monitor_with_scale(output, area, area, scale);
        engine.set_output_name(output, "DISPLAY1");
        assert_eq!(
            parity_spacing_work_rect(&engine, output, workspace),
            Rect::new(-290, 0, 890, 600),
            "runtime reserve_area pixels=10 remains ten physical pixels at scale {scale}"
        );
        parity_spacing_assert_placement_and_ipc(&engine, output, workspace, windows, 0);
    }
}

#[test]
fn parity_fractional_spacing_trailing_fraction_survives_proportional_width_resolution() {
    use crate::config::parse::parse_kdl_config;
    use crate::config::SizeSpec;

    let config = parse_kdl_config(
        r#"
layout {
    gaps 0
    struts { left -4.5; right 0.25; }
    border { off; }
    focus-ring { off; }
    center-focused-column "never"
    always-center-single-column false
    default-column-width { proportion 0.8; }
}
"#,
    )
    .expect("the fractional trailing-strut fixture parses");
    assert_eq!(
        config.layout.default_column_width,
        Some(Some(SizeSpec::Proportion(0.8)))
    );
    let (mut engine, output) = parity_spacing_engine(config, 1.0);
    let workspace = engine.monitors.get(&output).unwrap().active_workspace;
    let window = WindowId::new(36_000);
    let strip = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&workspace)
        .unwrap();
    assert!(strip.columns.is_empty());
    strip.add_window_to_new_column(window);
    strip.active_window = Some(window);
    engine.tiled_windows.insert(
        window,
        WindowInfo {
            hwnd: window.as_isize(),
            title: "Fractional trailing-strut fixture".to_string(),
            class_name: "SpacingParityFixture".to_string(),
            process_id: 1,
            bounds: Rect::new(20, 30, 111, 77),
            state: crate::backend::WindowState::Normal,
            is_visible: true,
        },
    );
    // Use the existing admission-time intent resolver without native admission.
    engine.remember_column_width_spec(window, None, None);
    let monitor = engine.monitors.get(&output).unwrap();
    let strip = &monitor.workspaces[&workspace];
    assert_eq!(strip.columns[0].width_proportion, Some(0.8));
    let layout = engine.effective_config_for_workspace(output, workspace);
    assert_eq!(layout.border_inset(), 0);
    let work = TilingEngine::tiling_work_rect(monitor, layout);
    let positions = engine.calculate_positions(strip, work, layout);
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].0, window);
    let rect = positions[0].1;
    // Pinned working area: x=ceil(-304.5)=-304, width=904.25-0.5=903.75.
    // Resolve 80% before final integer window sizing: 903.75 * 0.8 = 723.
    // Flooring the whole work area first would incorrectly produce 722.
    assert_eq!((rect.loc.x, rect.size.w), (-304, 723));
    let snapshot = engine.ipc_snapshot();
    let reported = &snapshot
        .windows
        .iter()
        .find(|reported| reported.id == window.as_isize() as u64)
        .expect("the seeded proportional window is present in IPC")
        .layout;
    assert_eq!(reported.pos_in_scrolling_layout, Some((1, 1)));
    assert_eq!(reported.window_offset_in_tile, (0.0, 0.0));
    assert_eq!(reported.tile_pos_in_workspace_view, Some((-4.0, 0.0)));
    assert_eq!(reported.tile_size.0, 723.0);
    assert_eq!(reported.window_size.0, 723);
}

fn assert_fractional_spacing_settled_logical_window_request(
    scale: f64,
    right_strut: f64,
    expected_origin: i32,
    expected_work_width: f64,
    expected_native_width: u32,
    expected_logical_width: i32,
    window: WindowId,
) {
    use crate::config::parse::parse_kdl_config;
    use crate::config::SizeSpec;

    let source = r#"
layout {
    gaps 0
    struts { left -4.5; right 0.5; }
    border { off; }
    focus-ring { off; }
    center-focused-column "never"
    always-center-single-column false
    default-column-width { proportion 0.8; }
}
"#
    .replace("right 0.5;", &format!("right {right_strut};"));
    let config = parse_kdl_config(&source).expect("the logical window-request fixture parses");
    assert_eq!(
        config.layout.default_column_width,
        Some(Some(SizeSpec::Proportion(0.8)))
    );
    let (mut engine, output) = parity_spacing_engine(config, scale);
    let workspace = engine.monitors.get(&output).unwrap().active_workspace;
    let strip = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&workspace)
        .unwrap();
    assert!(strip.columns.is_empty());
    strip.add_window_to_new_column(window);
    strip.active_window = Some(window);
    engine.tiled_windows.insert(
        window,
        WindowInfo {
            hwnd: window.as_isize(),
            title: "Logical window-request fixture".to_string(),
            class_name: "SpacingParityFixture".to_string(),
            process_id: 1,
            bounds: Rect::new(20, 30, 111, 77),
            state: crate::backend::WindowState::Normal,
            is_visible: true,
        },
    );
    // Exercise the existing admission resolver and shared placement calculator,
    // without native admission or a test-only production entry point.
    engine.remember_column_width_spec(window, None, None);
    let monitor = engine.monitors.get(&output).unwrap();
    let strip = &monitor.workspaces[&workspace];
    assert_eq!(strip.columns[0].width_proportion, Some(0.8));
    let layout = engine.effective_config_for_workspace(output, workspace);
    assert_eq!(layout.border_inset(), 0);
    assert_eq!((layout.column_gap, layout.window_gap), (0, 0));
    let work = TilingEngine::precise_tiling_area(monitor, layout);
    assert_eq!(
        (work.loc.x, work.width),
        (expected_origin, expected_work_width)
    );
    let positions = engine.calculate_positions(strip, work, layout);
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].0, window);
    assert_eq!(
        (positions[0].1.loc.x, positions[0].1.size.w),
        (expected_origin, expected_native_width),
        "niri floors the logical client request before rasterizing its accepted size"
    );

    // niri e19df12: tile.rs:955-959 floors the logical request. After a compliant
    // client settles, tile_size() follows its actual integer logical size
    // (807-824,845-851), and IPC reports that actual size (903-912).
    // A physically rounded visual tile can differ from the raw logical client
    // size at fractional scales. No unrounded request frame or centering
    // remainder survives as a window_offset_in_tile.
    let snapshot = engine.ipc_snapshot();
    let reported = &snapshot
        .windows
        .iter()
        .find(|reported| reported.id == window.as_isize() as u64)
        .expect("the seeded proportional window is present in IPC")
        .layout;
    assert_eq!(reported.pos_in_scrolling_layout, Some((1, 1)));
    assert_eq!(reported.tile_size.0, expected_native_width as f64 / scale);
    assert_eq!(reported.window_size.0, expected_logical_width);
    assert_eq!(reported.window_offset_in_tile, (0.0, 0.0));
}

#[test]
fn parity_fractional_spacing_window_request_floors_logical_at_1x() {
    // Working width 903.5 * .8 = 722.8 logical pixels. The accepted request is
    // 722 logical/physical pixels, including the settled borderless tile frame.
    assert_fractional_spacing_settled_logical_window_request(
        1.0,
        0.5,
        -304,
        903.5,
        722,
        722,
        WindowId::new(36_100),
    );
}

#[test]
fn parity_fractional_spacing_window_request_floors_before_physical_rounding_at_1_5x() {
    // Working width 905.25 / 1.5 * .8 = 482.8 logical pixels. Floor to 482,
    // then round(482 * 1.5) = 723 physical pixels. Flooring 724.2 physical
    // pixels directly would incorrectly leave a 724-pixel native window.
    assert_fractional_spacing_settled_logical_window_request(
        1.5,
        0.5,
        -306,
        905.25,
        723,
        482,
        WindowId::new(36_200),
    );
}

#[test]
fn parity_fractional_spacing_odd_logical_window_keeps_physical_visual_tile_at_1_5x() {
    // 906 / 1.5 * .8 = 483.2 -> logical client 483 -> physical round(724.5)=725.
    // The visual tile is 725/1.5 logical pixels; raw IPC window_size remains 483.
    assert_fractional_spacing_settled_logical_window_request(
        1.5,
        0.0,
        -306,
        906.0,
        725,
        483,
        WindowId::new(36_300),
    );
}

#[test]
fn parity_fractional_spacing_subunit_scale_retains_logical_request_despite_physical_collision() {
    // niri LayoutElement::size is integer logical and IPC reports it directly.
    // 902 / .5 * .8 = 1443.2 -> logical client 1443 -> physical round(721.5)=722.
    // Both 1443 and 1444 can rasterize to 722: inversion would incorrectly
    // report 1444. The visual tile is 1444 logical pixels, raw window_size 1443.
    assert_fractional_spacing_settled_logical_window_request(
        0.5,
        0.0,
        -302,
        902.0,
        722,
        1443,
        WindowId::new(36_400),
    );
}

fn fractional_constrained_window_fixture(windows: &[WindowId]) -> (TilingEngine, OutputId, i32) {
    let config = crate::config::parse::parse_kdl_config(
        r#"
layout {
    gaps 0
    struts { left -4.5; right 0.5; }
    border { off; }
    focus-ring { off; }
    center-focused-column "never"
    always-center-single-column false
    default-column-width { proportion 0.8; }
}
"#,
    )
    .expect("the constrained logical-request fixture parses");
    let (mut engine, output) = parity_spacing_engine(config, 1.5);
    let workspace = engine.monitors.get(&output).unwrap().active_workspace;
    for &window in windows {
        engine.tiled_windows.insert(
            window,
            WindowInfo {
                hwnd: window.as_isize(),
                title: "Constrained logical-request fixture".to_string(),
                class_name: "SpacingParityFixture".to_string(),
                process_id: 1,
                bounds: Rect::new(20, 30, 111, 77),
                state: crate::backend::WindowState::Normal,
                is_visible: true,
            },
        );
    }
    let strip = engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspaces
        .get_mut(&workspace)
        .unwrap();
    strip.add_window_to_new_column(windows[0]);
    strip.active_window = Some(windows[0]);
    engine.remember_column_width_spec(windows[0], None, None);
    for &window in &windows[1..] {
        assert!(engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspaces
            .get_mut(&workspace)
            .unwrap()
            .add_window_to_column(0, window));
    }
    (engine, output, workspace)
}

fn assert_constrained_window_native_and_ipc(
    engine: &TilingEngine,
    output: OutputId,
    workspace: i32,
    expected: &[(WindowId, u32, u32, i32)], // window, physical client width, physical inset, logical client width
) {
    let monitor = engine.monitors.get(&output).unwrap();
    let strip = &monitor.workspaces[&workspace];
    assert_eq!(
        strip.columns[0].width_proportion,
        Some(0.8),
        "constraints retain proportional intent"
    );
    let cfg = engine.effective_config_for_workspace(output, workspace);
    let positions: HashMap<_, _> = engine
        .calculate_positions(strip, TilingEngine::precise_tiling_area(monitor, cfg), cfg)
        .into_iter()
        .collect();
    let snapshot = engine.ipc_snapshot();
    for &(window, client_px, inset, logical_client) in expected {
        let native_tile = positions[&window];
        // The shared native placement calculator supplies the outer frame;
        // native application subtracts this fixture's physical border band.
        assert_eq!(native_tile.size.w, client_px + inset * 2);
        assert_eq!(native_tile.size.w - inset * 2, client_px);
        let reported = &snapshot
            .windows
            .iter()
            .find(|entry| entry.id == window.as_isize() as u64)
            .unwrap()
            .layout;
        assert_eq!(reported.tile_size.0, native_tile.size.w as f64 / 1.5);
        assert_eq!(reported.window_size.0, logical_client);
        assert_eq!(reported.window_offset_in_tile.0, inset as f64 / 1.5);
        assert_eq!(reported.window_offset_in_tile.1, inset as f64 / 1.5);
    }
}

#[test]
fn parity_fractional_spacing_native_minimum_carries_final_logical_window_request() {
    let window = WindowId::new(36_500);
    let (mut engine, output, workspace) = fractional_constrained_window_fixture(&[window]);
    assert!(engine.note_window_min_width(window, 751));
    engine.refresh_strip_widths(output);
    // The 751-physical-pixel minimum overrides the 724.2-pixel request.
    // Logical 500 rasterizes to 750 and cannot satisfy it; logical 501 -> 752.
    assert_constrained_window_native_and_ipc(&engine, output, workspace, &[(window, 752, 0, 501)]);
}

#[test]
fn parity_fractional_spacing_rule_maximum_carries_final_logical_window_request() {
    let window = WindowId::new(36_600);
    let (mut engine, output, workspace) = fractional_constrained_window_fixture(&[window]);
    engine.rule_limits.insert(
        window,
        rules::RuleLimits {
            max_width: 700,
            ..Default::default()
        },
    );
    engine.sync_min_widths();
    engine.refresh_strip_widths(output);
    // A 700-physical-pixel rule maximum excludes logical 467 -> physical 701.
    // The largest compliant integer logical request is 466 -> physical 699.
    assert_constrained_window_native_and_ipc(&engine, output, workspace, &[(window, 699, 0, 466)]);
}

#[test]
fn parity_fractional_spacing_unrepresentable_limits_prefer_minimum_and_keep_request() {
    let window = WindowId::new(36_700);
    let (mut engine, output, workspace) = fractional_constrained_window_fixture(&[window]);
    engine.rule_limits.insert(
        window,
        rules::RuleLimits {
            max_width: 700,
            ..Default::default()
        },
    );
    assert!(engine.note_window_min_width(window, 700));
    engine.refresh_strip_widths(output);
    // Integer logical widths rasterize to 699 or 701 at this boundary. The
    // existing engine contract gives minimums precedence over maximums.
    assert_constrained_window_native_and_ipc(&engine, output, workspace, &[(window, 701, 0, 467)]);
}

#[test]
fn parity_fractional_spacing_stacked_border_insets_carry_each_window_request() {
    let plain = WindowId::new(36_800);
    let bordered = WindowId::new(36_801);
    let (mut engine, output, workspace) = fractional_constrained_window_fixture(&[plain, bordered]);
    let mut resolved = crate::window::ResolvedWindowRules::default();
    resolved.effects.border.off = Some(false);
    resolved.effects.border.width = Some(2);
    engine.rule_state.insert(
        bordered,
        rules::RuleState {
            resolved,
            process_name: None,
            aumid: None,
        },
    );
    engine.sync_min_widths();
    engine.refresh_strip_widths(output);
    // The same 724.2-physical-pixel proportional request subtracts each
    // window's current physical border before flooring in logical units:
    // plain floor(724.2/1.5)=482 -> 723; bordered floor(720.2/1.5)=480 -> 720.
    // Borders stay in their existing physical units; appearance scaling is separate.
    assert_constrained_window_native_and_ipc(
        &engine,
        output,
        workspace,
        &[(plain, 723, 0, 482), (bordered, 720, 2, 480)],
    );
}
