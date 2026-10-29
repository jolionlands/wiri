//! Proposed layout::engine::tiled_height_parity_tests child; not registered or executed.
//! Pin e19df12: scrolling.rs4666-4726 preserves fixed/preset sole height and
//! gives all tabs the only non-auto height; :5154-5168 converts the prior owner
//! to auto when cycling an auto tile; :5139-5151 resets all tab heights.
//! Entire setup/action sequence is activation-refused. Invalid negative HWNDs
//! are not pseudo handles; no real windows, input pumps, hooks or sinks exist.
//! Geometry assertions call the real pure scrolling layout with zero gaps,
//! borders and tab indicator so source height units have an unambiguous oracle.
use super::*;
use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;
use crate::config::types::SizeSpec;
use crate::layout::{Column, ColumnDisplay, Tile};

fn tiled_fixture(
    count: usize,
    tabbed: bool,
    presets: Vec<SizeSpec>,
) -> (TilingEngine, BackendHandle, OutputId) {
    let mut cfg = LayoutConfig {
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
    };
    cfg.effects.border.off = true;
    cfg.effects.focus_ring.off = true;
    cfg.effects.tab_indicator.off = true;
    let mut engine = TilingEngine::new(cfg);
    let output = OutputId::from_name("tiled-height-model");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1200, 900),
        Rect::new(0, 0, 1200, 900),
    );
    let mut column = Column::new();
    for index in 0..count {
        column.add_tile(Tile::new(WindowId::new(-9401 - index as isize)));
    }
    if tabbed {
        column.switch_to_tabbed();
    }
    let monitor = engine.monitors.get_mut(&output).unwrap();
    monitor.workspace_mut().unwrap().columns.push(column);
    monitor.focus_column = Some(0);
    monitor.focus_window = Some(WindowId::new(-9401));
    (engine, BackendHandle::default_for_test(), output)
}

fn select_tab(engine: &mut TilingEngine, output: OutputId, tab: usize) {
    let monitor = engine.monitors.get_mut(&output).unwrap();
    monitor.workspace_mut().unwrap().columns[0].display = ColumnDisplay::Tabbed { active_tab: tab };
    monitor.focus_window = Some(WindowId::new(-9401 - tab as isize));
}

fn height(engine: &TilingEngine, output: OutputId) -> u32 {
    let monitor = engine.monitors.get(&output).unwrap();
    let cfg = engine.active_workspace_config(output);
    let positions = engine.calculate_positions(
        monitor.workspace().unwrap(),
        Rect::new(0, 0, 1200, 900),
        cfg,
    );
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].1.loc.y, 0);
    positions[0].1.size.h
}

fn preset_geometry(spec: SizeSpec, tabbed: bool, expected: u32) {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) =
            tiled_fixture(if tabbed { 2 } else { 1 }, tabbed, vec![spec]);
        engine.switch_preset_window_height(true, &backend);
        assert_eq!(height(&engine, output), expected);
        if tabbed {
            // The other tab is auto; column height still comes from the owner.
            select_tab(&mut engine, output, 1);
            assert_eq!(height(&engine, output), expected);
        }
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_height_sole_fixed_preset_leaves_blank_space() {
    preset_geometry(SizeSpec::Fixed(300), false, 300);
}

#[test]
fn parity_tiled_height_sole_proportion_preset_leaves_blank_space() {
    preset_geometry(SizeSpec::Proportion(0.5), false, 450);
}

#[test]
fn parity_tiled_height_tabbed_fixed_preset_survives_auto_tab_selection() {
    preset_geometry(SizeSpec::Fixed(300), true, 300);
}

#[test]
fn parity_tiled_height_tabbed_proportion_preset_survives_auto_tab_selection() {
    preset_geometry(SizeSpec::Proportion(0.5), true, 450);
}

#[test]
fn parity_tiled_height_sole_auto_still_fills_view() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (engine, _, output) = tiled_fixture(1, false, vec![]);
        assert_eq!(height(&engine, output), 900);
    });
    assert!(attempts.is_empty());
}

#[test]
fn parity_tiled_height_tabbed_auto_still_fills_view() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, _, output) = tiled_fixture(2, true, vec![]);
        for tab in 0..2 {
            select_tab(&mut engine, output, tab);
            assert_eq!(height(&engine, output), 900);
        }
    });
    assert!(attempts.is_empty());
}

#[test]
fn parity_tiled_height_cycling_auto_tab_replaces_previous_nonauto_owner() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(2, true, vec![SizeSpec::Fixed(300)]);
        engine.switch_preset_window_height(true, &backend);
        select_tab(&mut engine, output, 1);
        engine.switch_preset_window_height(true, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        let tiles = &monitor.workspace().unwrap().columns[0].tiles;
        assert_eq!(tiles[0].preferred_height, None);
        assert_eq!(tiles[0].height_proportion, None);
        assert_eq!(tiles[0].height_preset_idx, None);
        assert_eq!(tiles[1].preferred_height, Some(300));
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_height_reset_from_auto_tab_clears_column_owner() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(2, true, vec![SizeSpec::Fixed(300)]);
        engine.switch_preset_window_height(true, &backend);
        select_tab(&mut engine, output, 1);
        engine.reset_window_height(&backend);
        let monitor = engine.monitors.get(&output).unwrap();
        for tile in &monitor.workspace().unwrap().columns[0].tiles {
            assert_eq!(tile.preferred_height, None);
            assert_eq!(tile.height_proportion, None);
            assert_eq!(tile.height_preset_idx, None);
        }
        assert_eq!(height(&engine, output), 900);
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

// Append-only TEMP fragment for tiled-height-parity-tests.rs; no registration.
// Pin e19df12 scrolling.rs5063-5138: direct height requests become a Fixed
// owner, including sole/tabbed columns; an auto target first converts the old
// owner to auto. All tabs receive that owner's height at4666-4726.
fn direct_set_sole_geometry(change: SizeChange, expected: u32) {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(1, false, vec![]);
        engine.set_window_height_by(change, &backend);
        assert_eq!(height(&engine, output), expected);
        let monitor = engine.monitors.get(&output).unwrap();
        let tile = &monitor.workspace().unwrap().columns[0].tiles[0];
        assert_eq!(tile.height_preset_idx, None);
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

fn direct_set_tabbed_geometry(change: SizeChange, expected: u32) {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(2, true, vec![SizeSpec::Fixed(200)]);
        engine.switch_preset_window_height(true, &backend);
        let target = WindowId::new(-9402);
        let before_focus = engine.monitors.get(&output).unwrap().focus_window;
        // The target is an inactive auto tab. A direct explicit setter replaces
        // the old non-auto owner without selecting the requested target.
        engine.set_window_height_for(target, change, &backend);
        assert_eq!(
            engine.monitors.get(&output).unwrap().focus_window,
            before_focus
        );
        assert_eq!(height(&engine, output), expected);
        select_tab(&mut engine, output, 1);
        assert_eq!(height(&engine, output), expected);
        let monitor = engine.monitors.get(&output).unwrap();
        let tiles = &monitor.workspace().unwrap().columns[0].tiles;
        assert_eq!(tiles[0].preferred_height, None);
        assert_eq!(tiles[0].height_proportion, None);
        assert_eq!(tiles[0].height_preset_idx, None);
        assert_eq!(tiles[1].preferred_height, Some(expected));
        assert_eq!(tiles[1].height_proportion, None);
        assert_eq!(tiles[1].height_preset_idx, None);
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_height_direct_sole_fixed_leaves_blank_space() {
    direct_set_sole_geometry(SizeChange::SetFixed(300), 300);
}

#[test]
fn parity_tiled_height_direct_sole_proportion_leaves_blank_space() {
    direct_set_sole_geometry(SizeChange::SetProportion(50_000), 450);
}

#[test]
fn parity_tiled_height_direct_tabbed_fixed_replaces_owner_without_focus() {
    direct_set_tabbed_geometry(SizeChange::SetFixed(300), 300);
}

#[test]
fn parity_tiled_height_direct_tabbed_proportion_replaces_owner_without_focus() {
    direct_set_tabbed_geometry(SizeChange::SetProportion(50_000), 450);
}

// Append-only TEMP fragment for frozen combined12; no source registration.
// Pin e19df12 scrolling.rs5219-5233 preserves apparent-height ratios during
// auto conversion; 5157-5214 uses the actual common tab size for next selection.
// scrolling.rs5703-5715 and tab_indicator.rs311-335 give the gap/extra-size math.

fn tiled_extra_positions(engine: &TilingEngine, output: OutputId) -> Vec<(WindowId, Rect)> {
    let monitor = engine.monitors.get(&output).unwrap();
    let cfg = engine.active_workspace_config(output);
    engine.calculate_positions(
        monitor.workspace().unwrap(),
        TilingEngine::precise_tiling_area(monitor, cfg),
        cfg,
    )
}

#[test]
fn parity_tiled_height_stacked_auto_takeover_preserves_apparent_ratios() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(3, false, vec![SizeSpec::Fixed(450)]);
        engine.switch_preset_window_height(true, &backend);
        let before: Vec<_> = tiled_extra_positions(&engine, output)
            .iter()
            .map(|(_, rect)| rect.size.h)
            .collect();
        assert_eq!(before, vec![450, 225, 225]);
        let before_focus = engine.monitors.get(&output).unwrap().focus_window;
        engine.set_window_height_for(WindowId::new(-9402), SizeChange::SetFixed(300), &backend);
        let after: Vec<_> = tiled_extra_positions(&engine, output)
            .iter()
            .map(|(_, rect)| rect.size.h)
            .collect();
        // Convert [450,225,225] to auto weights [2,1,1], then give tile1 300.
        // The remaining 600 splits 2:1 between tile0 and tile2.
        assert_eq!(after, vec![400, 300, 200]);
        assert_eq!(
            engine.monitors.get(&output).unwrap().focus_window,
            before_focus
        );
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_height_auto_tab_selects_next_from_common_owner_height() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(
            2,
            true,
            vec![
                SizeSpec::Fixed(500),
                SizeSpec::Fixed(350),
                SizeSpec::Fixed(700),
            ],
        );
        engine.switch_preset_window_height(true, &backend);
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .tiles[0]
                .preferred_height,
            Some(500),
        );
        select_tab(&mut engine, output, 1);
        engine.switch_preset_window_height(true, &backend);
        let monitor = engine.monitors.get(&output).unwrap();
        let tiles = &monitor.workspace().unwrap().columns[0].tiles;
        // The auto tab is actually 500px high, not the synthetic half-view450.
        assert_eq!(tiles[1].preferred_height, Some(700));
        assert_eq!(tiles[1].height_preset_idx, Some(2));
        assert_eq!(tiles[0].preferred_height, None);
        assert_eq!(height(&engine, output), 700);
        select_tab(&mut engine, output, 0);
        assert_eq!(height(&engine, output), 700);
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_height_tabbed_proportion_reserves_gaps_and_top_indicator_once() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(2, true, vec![SizeSpec::Proportion(0.5)]);
        let mut cfg = engine.active_workspace_config(output).clone();
        cfg.outer_gaps = (12, 12, 12, 12);
        cfg.column_gap = 12;
        cfg.window_gap = 12;
        cfg.effects.tab_indicator.off = false;
        cfg.effects.tab_indicator.place_within_column = true;
        cfg.effects.tab_indicator.hide_when_single_tab = false;
        cfg.effects.tab_indicator.position = crate::config::TabPosition::Top;
        cfg.effects.tab_indicator.width = 8.0;
        cfg.effects.tab_indicator.gap = 6.0;
        engine.update_config(cfg);
        let auto = tiled_extra_positions(&engine, output);
        assert_eq!(auto.len(), 1);
        assert_eq!(auto[0].1.loc.y, 26); // top gap12 + indicator8 + indicator gap6
        assert_eq!(auto[0].1.size.h, 862); // 900 - outer24 - indicator14
        engine.switch_preset_window_height(true, &backend);
        for tab in 0..2 {
            select_tab(&mut engine, output, tab);
            let positions = tiled_extra_positions(&engine, output);
            assert_eq!(positions.len(), 1);
            assert_eq!(positions[0].1.loc.y, 26);
            // (view900 - gap12)*.5 - gap12 - indicator(8+6) = 418.
            assert_eq!(positions[0].1.size.h, 418);
        }
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_height_fixed_preset_scales_client_before_physical_border() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(2, true, vec![SizeSpec::Fixed(200)]);
        let mut cfg = engine.active_workspace_config(output).clone();
        cfg.border_width = 4;
        cfg.effects.border.off = false;
        cfg.effects.border.width = 4;
        engine.update_config(cfg);
        engine.sync_monitor_with_scale(
            output,
            Rect::new(0, 0, 1200, 900),
            Rect::new(0, 0, 1200, 900),
            1.5,
        );
        engine.switch_preset_window_height(true, &backend);
        for tab in 0..2 {
            select_tab(&mut engine, output, tab);
            let positions = tiled_extra_positions(&engine, output);
            assert_eq!(positions.len(), 1);
            // Fixed client200 logical ->300 physical, plus two already-physical4px borders.
            assert_eq!(positions[0].1.size.h, 308);
        }
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

// Append-only TEMP fragment for frozen combined16; no source registration.
// Seed the engine's real rule-height source and let sync_tile_limits(false)
// reconcile it into the tile. Reconcile again after the action to distinguish
// permanent auto conversion from clearing only the derived preferred-height.
// Pin e19df12 scrolling.rs5063-5075,5142-5167,5219-5233: user changes and tabbed
// reset replace prior non-auto ownership with auto, retaining apparent ratios.

#[test]
fn parity_tiled_height_rule_origin_takeover_stays_auto_after_limit_sync() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(3, false, vec![]);
        let previous_owner = WindowId::new(-9401);
        let target = WindowId::new(-9402);
        engine.rule_heights.insert(previous_owner, 450);
        engine.sync_tile_limits(false);
        let before: Vec<_> = tiled_extra_positions(&engine, output)
            .iter()
            .map(|(_, rect)| rect.size.h)
            .collect();
        assert_eq!(before, vec![450, 225, 225]);
        let before_focus = engine.monitors.get(&output).unwrap().focus_window;
        engine.set_window_height_for(target, SizeChange::SetFixed(300), &backend);
        engine.sync_tile_limits(false);
        let monitor = engine.monitors.get(&output).unwrap();
        let tiles = &monitor.workspace().unwrap().columns[0].tiles;
        assert_eq!(tiles[0].preferred_height, None);
        assert_eq!(tiles[0].height_proportion, None);
        assert_eq!(tiles[0].height_preset_idx, None);
        assert!(!engine.rule_heights.contains_key(&previous_owner));
        let after: Vec<_> = tiled_extra_positions(&engine, output)
            .iter()
            .map(|(_, rect)| rect.size.h)
            .collect();
        assert_eq!(after, vec![400, 300, 200]);
        assert_eq!(
            engine.monitors.get(&output).unwrap().focus_window,
            before_focus
        );
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_height_rule_origin_tabbed_reset_stays_auto_after_limit_sync() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output) = tiled_fixture(2, true, vec![]);
        let previous_owner = WindowId::new(-9401);
        engine.rule_heights.insert(previous_owner, 300);
        engine.sync_tile_limits(false);
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .tiles[0]
                .preferred_height,
            Some(300),
        );
        select_tab(&mut engine, output, 1);
        engine.reset_window_height(&backend);
        engine.sync_tile_limits(false);
        let monitor = engine.monitors.get(&output).unwrap();
        for tile in &monitor.workspace().unwrap().columns[0].tiles {
            assert_eq!(tile.preferred_height, None);
            assert_eq!(tile.height_proportion, None);
            assert_eq!(tile.height_preset_idx, None);
        }
        assert!(!engine.rule_heights.contains_key(&previous_owner));
        for tab in 0..2 {
            select_tab(&mut engine, output, tab);
            assert_eq!(height(&engine, output), 900);
        }
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}
// TEMP append-only fragment for the existing tiled_height_parity_tests child.
// Reuses its guarded invalid-HWND fixture, height, select_tab and pure-position
// helpers. It is neither registered nor executed.
// Pin e19df12 scrolling.rs4072-4131: identical preset/config lists preserve the
// existing WindowHeight::Fixed request; TileData::update3952-3968 updates observed
// geometry without replacing height. Width constraints do not reset that request.
// Reload calls are the engine publication sequence from main.rs apply_loaded_config;
// output/native input enumeration is excluded from this synthetic engine fixture.

fn tiled_durability_config() -> crate::config::Config {
    let mut config = crate::config::Config::default();
    config.output.clear();
    config.workspace.clear();
    config.window_rules.clear();
    config.layout.inner_gaps = 0.0;
    config.layout.outer_gaps = 0.0;
    config.layout.preset_window_heights.clear();
    config.layout.effects.border.off = true;
    config.layout.effects.focus_ring.off = true;
    config.layout.effects.tab_indicator.off = true;
    config
}

fn tiled_durability_reload(
    engine: &mut TilingEngine,
    backend: &BackendHandle,
    output: OutputId,
    config: &crate::config::Config,
) {
    engine.update_config(LayoutConfig::from_config(config));
    // Same observed output context, equivalent to the model-side sync in the
    // real reload path; this performs no native enumeration or DPI change.
    engine.sync_monitor_with_scale(
        output,
        Rect::new(0, 0, 1200, 900),
        Rect::new(0, 0, 1200, 900),
        1.0,
    );
    engine.set_full_config(config.clone());
    engine.apply_reloaded_layout(backend);
}

fn tiled_durability_fixture(
    count: usize,
    tabbed: bool,
) -> (TilingEngine, BackendHandle, OutputId, crate::config::Config) {
    let (mut engine, backend, output) = tiled_fixture(count, tabbed, vec![]);
    // The model already received an OS-foreground report. This is an engine
    // observation, not an activation request or a synthetic input bypass.
    assert_eq!(
        engine.sync_os_foreground(WindowId::new(-9401), &backend),
        Some((0, 0)),
    );
    let config = tiled_durability_config();
    tiled_durability_reload(&mut engine, &backend, output, &config);
    (engine, backend, output, config)
}

#[test]
fn parity_tiled_durability_sole_fixed_survives_same_config_reload() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output, config) = tiled_durability_fixture(1, false);
        engine.set_window_height_by(SizeChange::SetFixed(300), &backend);
        assert_eq!(height(&engine, output), 300);
        let focus = engine.monitors.get(&output).unwrap().focus_window;
        tiled_durability_reload(&mut engine, &backend, output, &config);
        assert_eq!(height(&engine, output), 300);
        assert_eq!(engine.monitors.get(&output).unwrap().focus_window, focus);
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_durability_tabbed_fixed_survives_same_config_reload() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output, config) = tiled_durability_fixture(2, true);
        engine.set_window_height_by(SizeChange::SetFixed(300), &backend);
        assert_eq!(height(&engine, output), 300);
        let focus = engine.monitors.get(&output).unwrap().focus_window;
        tiled_durability_reload(&mut engine, &backend, output, &config);
        assert_eq!(engine.monitors.get(&output).unwrap().focus_window, focus);
        for tab in 0..2 {
            select_tab(&mut engine, output, tab);
            assert_eq!(height(&engine, output), 300);
        }
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_durability_stacked_fixed_survives_same_config_reload() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output, config) = tiled_durability_fixture(2, false);
        engine.set_window_height_by(SizeChange::SetFixed(300), &backend);
        let before: Vec<_> = tiled_extra_positions(&engine, output)
            .iter()
            .map(|(_, rect)| rect.size.h)
            .collect();
        assert_eq!(before, vec![300, 600]);
        let focus = engine.monitors.get(&output).unwrap().focus_window;
        tiled_durability_reload(&mut engine, &backend, output, &config);
        let after: Vec<_> = tiled_extra_positions(&engine, output)
            .iter()
            .map(|(_, rect)| rect.size.h)
            .collect();
        assert_eq!(after, vec![300, 600]);
        assert_eq!(engine.monitors.get(&output).unwrap().focus_window, focus);
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}

#[test]
fn parity_tiled_durability_fixed_survives_native_width_minimum_refresh() {
    let ((), attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output, config) = tiled_durability_fixture(1, false);
        engine.set_window_height_by(SizeChange::SetFixed(300), &backend);
        assert_eq!(height(&engine, output), 300);
        let focus = engine.monitors.get(&output).unwrap().focus_window;
        assert!(engine.note_window_min_width(WindowId::new(-9401), 123));
        // Isolate the actual width-only reconciliation trigger before reload.
        assert_eq!(height(&engine, output), 300);
        assert_eq!(engine.monitors.get(&output).unwrap().focus_window, focus);
        tiled_durability_reload(&mut engine, &backend, output, &config);
        assert_eq!(height(&engine, output), 300);
        assert_eq!(engine.monitors.get(&output).unwrap().focus_window, focus);
    });
    assert!(attempts.is_empty(), "unexpected activation: {attempts:?}");
}
