//! Appearance metrics are logical pixels: the border, the focus ring, the shadow and the
//! window-rule overrides of all three are projected through the scale of the output the
//! window is on.
//!
//! Source oracle: niri e19df12cf81ffe805609fee497da1a097066ad2f.
//! - layout/tile.rs:244,252-257 (and 395-408): the merged layout + rule `border` and
//!   `focus-ring` widths become `round_logical_in_physical_max1(scale, width)`.
//! - utils/mod.rs:185-191: that function (zero stays zero, otherwise at least one physical
//!   pixel, rounded to the nearest, halves away from zero).
//! - layout/tile.rs:818-840: the tile is the window plus twice the rounded border width.
//! - layout/focus_ring.rs:69-70,115: the ring extends the rounded width outside the tile.
//! - layout/shadow.rs:40-61: offsets and the spread magnitude are ceiled to physical
//!   pixels, the blur sigma (softness / 2) is not rounded.
//!
//! The fake monitors never touch Win32: the engine is fed `register_monitor_with_scale` at
//! 100%, 125%, 150%, 175% and 200%. The 100% case keeps wiri's historic pixels.

use super::decor::{frame_specs, FrameInputs};
use super::rules::RuleState;
use super::*;
use crate::config::Config;
use crate::layout::metrics::{project_shadow, ring_width_px};
use crate::overlay::decor::{DecorContent, FrameSpec, RecordingSink};
use crate::window::ResolvedWindowRules;

const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

/// niri utils/mod.rs:185-191, in logical pixels.
fn niri_round_max1(scale: f64, logical: f64) -> f64 {
    if logical == 0. {
        return 0.;
    }
    (logical * scale).max(1.).round() / scale
}

/// The `ceil` closure of niri shadow.rs:40, in logical pixels.
fn niri_ceil(scale: f64, logical: f64) -> f64 {
    (logical * scale).ceil() / scale
}

fn wid(n: isize) -> WindowId {
    WindowId::new(n)
}

/// A layout with fractional logical widths and the shadow on.
fn fractional_config() -> Config {
    let mut config = Config::default();
    config.layout.effects.border.width = 2.5;
    config.layout.effects.focus_ring.width = 3.5;
    config.layout.effects.shadow.on = true;
    config
}

fn engine_with(config: Config, outputs: &[(&str, f64)]) -> (TilingEngine, Vec<OutputId>) {
    let mut engine = TilingEngine::new(LayoutConfig::from_config(&config));
    engine.set_full_config(config);
    let mut ids = Vec::new();
    for (index, (name, scale)) in outputs.iter().enumerate() {
        let id = OutputId::from_name(name);
        let x = 2000 * index as i32;
        engine.register_monitor_with_scale(
            id,
            Rect::new(x, 0, 1920, 1080),
            Rect::new(x, 0, 1920, 1040),
            *scale,
        );
        ids.push(id);
    }
    (engine, ids)
}

/// What the overlay layer is asked to draw around `window` after a layout pass on `output`.
fn decorate(engine: &mut TilingEngine, output: OutputId, window: WindowId) -> FrameSpec {
    let sink = RecordingSink::new();
    engine.install_decor_sink(sink.clone());
    engine.monitors.set_focused(output);
    let monitor = engine.monitors.get_mut(&output).unwrap();
    monitor.focus_window = Some(window);
    let work = monitor.work_area;
    let tile = Rect::new(work.loc.x + 100, work.loc.y + 100, 600, 400);
    let positions = HashMap::from([(window, tile)]);
    let modes: HashMap<WindowId, DrawMode> = HashMap::new();
    engine.refresh_decorations(output, &positions, &modes, work, false);
    sink.of_output(output.as_u64())
        .iter()
        .find_map(|spec| match &spec.content {
            DecorContent::Frame(frame) if spec.id.key == window.as_isize() => Some(frame.clone()),
            _ => None,
        })
        .expect("the active window has a frame")
}

fn shadow_of(frame: &FrameSpec) -> (f32, f32, f32, f32) {
    let shadow = frame.shadow.expect("the shadow is on");
    (
        shadow.softness,
        shadow.spread,
        shadow.offset_x,
        shadow.offset_y,
    )
}

#[test]
fn parity_projected_widths_match_niri_rounding_at_every_scale() {
    for scale in SCALES {
        for logical in [0.0, 0.2, 0.5, 1.0, 1.5, 2.5, 3.0, 3.5, 4.0, 6.5, 12.25] {
            let px = ring_width_px(logical, scale);
            // niri keeps the width logical; its physical pixels are width * scale.
            let reference = (niri_round_max1(scale, logical) * scale).round() as i32;
            assert_eq!(px, reference, "width {logical} at scale {scale}");
            assert_eq!(px == 0, logical == 0.0, "only zero is off: {logical}");
        }
    }
    // Hand-checked pins (niri rounds half away from zero).
    assert_eq!(ring_width_px(2.5, 1.0), 3);
    assert_eq!(ring_width_px(3.0, 1.5), 5);
    assert_eq!(ring_width_px(2.5, 1.25), 3);
    assert_eq!(ring_width_px(2.5, 1.75), 4);
    assert_eq!(ring_width_px(0.2, 2.0), 1, "a non-zero width is one pixel at least");
}

#[test]
fn parity_projected_shadow_matches_niri_ceiling_at_every_scale() {
    for scale in SCALES {
        for (softness, spread, ox, oy) in [
            (30.0, 5.0, 0.0, 5.0),
            (10.0, -2.0, 1.2, -1.2),
            (0.0, 0.0, 0.0, 0.0),
            (7.5, 0.4, -2.5, 2.5),
        ] {
            let got = project_shadow(softness, spread, ox, oy, scale);
            let spread_ref = niri_ceil(scale, f64::abs(spread)).copysign(spread);
            assert_eq!(got.softness_px, softness * scale);
            assert_eq!(got.spread_px, (spread_ref * scale).round());
            assert_eq!(got.offset_x_px, (niri_ceil(scale, ox) * scale).round());
            assert_eq!(got.offset_y_px, (niri_ceil(scale, oy) * scale).round());
        }
    }
    // 100% leaves whole logical pixels unchanged.
    let at_96 = project_shadow(30.0, 5.0, 0.0, 5.0, 1.0);
    assert_eq!(
        (at_96.softness_px, at_96.spread_px, at_96.offset_x_px, at_96.offset_y_px),
        (30.0, 5.0, 0.0, 5.0)
    );
    // Invalid scales and non-finite inputs fall back to 96 DPI and zero.
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(project_shadow(30.0, 5.0, 0.0, 5.0, scale), at_96);
        assert_eq!(ring_width_px(4.0, scale), 4);
    }
    let broken = project_shadow(f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 1.5);
    assert_eq!(broken, crate::layout::metrics::PhysicalShadow::default());
}

#[test]
fn parity_layout_decorations_scale_per_output() {
    // (scale, border, ring, softness, spread, offset_y) for border 2.5, ring 3.5 and
    // niri's default shadow (softness 30, spread 5, offset 0 5).
    let table = [
        (1.0, 3, 4, 30.0, 5.0, 5.0),
        (1.25, 3, 4, 37.5, 7.0, 7.0),
        (1.5, 4, 5, 45.0, 8.0, 8.0),
        (1.75, 4, 6, 52.5, 9.0, 9.0),
        (2.0, 5, 7, 60.0, 10.0, 10.0),
    ];
    for (scale, border, ring, softness, spread, offset_y) in table {
        let (mut engine, outputs) = engine_with(fractional_config(), &[("scaled", scale)]);
        let output = outputs[0];
        assert_eq!(
            engine.effective_config(output).border_inset(),
            border,
            "the layout band at scale {scale}"
        );
        assert_eq!(engine.effective_config(output).output_scale(), scale);
        let frame = decorate(&mut engine, output, wid(11));
        assert_eq!(frame.band, border, "tile shrink at scale {scale}");
        assert_eq!(frame.ring_width, ring, "ring at scale {scale}");
        assert_eq!(
            shadow_of(&frame),
            (softness, spread, 0.0, offset_y),
            "shadow at scale {scale}"
        );
        // The tile is the window and the band; the window shrinks by exactly that band.
        let tile = frame.tile;
        let cfg = engine.effective_config(output).clone();
        let inset = engine.rule_border_inset(wid(11), &cfg);
        let geometry = engine.resolved_window_geometry(wid(11), tile, &cfg, inset, false);
        assert_eq!(
            geometry.window,
            Rect::new(
                tile.loc.x + border,
                tile.loc.y + border,
                tile.size.w - 2 * border as u32,
                tile.size.h - 2 * border as u32,
            ),
            "native allocation at scale {scale}"
        );
    }
}

#[test]
fn parity_the_decoration_extent_grows_with_the_projected_shadow() {
    let reach = |scale: f64| {
        let (mut engine, outputs) = engine_with(fractional_config(), &[("extent", scale)]);
        decorate(&mut engine, outputs[0], wid(12)).extent()
    };
    // sigma 15 px, spread 5, offset 5: reach 55 (+1 margin, +2 slack).
    assert_eq!(reach(1.0), 58);
    // sigma 30 px, spread 10, offset 10: reach 110 (+1, +2).
    assert_eq!(reach(2.0), 113);
}

#[test]
fn parity_window_rule_overrides_use_the_same_projection() {
    // Rule: border 2.5, ring 1.2, shadow softness 10, spread -2, offset 1.2 -1.2.
    let expected = [
        // scale, border, ring, softness, spread, offset_x, offset_y
        (1.0, 3, 1, 10.0, -2.0, 2.0, -1.0),
        (1.5, 4, 2, 15.0, -3.0, 2.0, -1.0),
        (2.0, 5, 2, 20.0, -4.0, 3.0, -2.0),
    ];
    for (scale, border, ring, softness, spread, offset_x, offset_y) in expected {
        let (mut engine, outputs) = engine_with(fractional_config(), &[("rules", scale)]);
        let output = outputs[0];
        let window = wid(21);
        let mut resolved = ResolvedWindowRules::default();
        resolved.effects.border.width = Some(2.5);
        resolved.effects.focus_ring.width = Some(1.2);
        resolved.effects.shadow.softness = Some(10.0);
        resolved.effects.shadow.spread = Some(-2.0);
        resolved.effects.shadow.offset_x = Some(1.2);
        resolved.effects.shadow.offset_y = Some(-1.2);
        engine.rule_state.insert(
            window,
            RuleState {
                resolved,
                process_name: None,
                aumid: None,
            },
        );
        let cfg = engine.effective_config(output).clone();
        assert_eq!(engine.rule_border_inset(window, &cfg), border);
        let frame = decorate(&mut engine, output, window);
        assert_eq!(frame.band, border, "rule border at scale {scale}");
        assert_eq!(frame.ring_width, ring, "rule ring at scale {scale}");
        assert_eq!(
            shadow_of(&frame),
            (softness, spread, offset_x, offset_y),
            "rule shadow at scale {scale}"
        );
    }
}

#[test]
fn parity_an_off_layout_border_keeps_its_logical_width_for_a_rule_that_enables_it() {
    let mut config = fractional_config();
    config.layout.effects.border.off = true;
    let (mut engine, outputs) = engine_with(config, &[("off-border", 1.5)]);
    let output = outputs[0];
    let window = wid(31);
    let cfg = engine.effective_config(output).clone();
    assert_eq!(cfg.border_inset(), 0, "the layout border is off");
    assert_eq!(engine.rule_border_inset(window, &cfg), 0);
    let mut resolved = ResolvedWindowRules::default();
    resolved.effects.border.off = Some(false);
    engine.rule_state.insert(
        window,
        RuleState {
            resolved,
            process_name: None,
            aumid: None,
        },
    );
    // The saved logical 2.5 is projected for this output: 3.75 -> 4.
    assert_eq!(engine.rule_border_inset(window, &cfg), 4);
}

#[test]
fn parity_a_window_on_another_scale_is_decorated_with_that_outputs_pixels() {
    let (mut engine, outputs) = engine_with(
        fractional_config(),
        &[("scale-one", 1.0), ("scale-one-and-three-quarters", 1.75)],
    );
    let (low, high) = (outputs[0], outputs[1]);
    let window = wid(41);
    let before = decorate(&mut engine, low, window);
    let after = decorate(&mut engine, high, window);
    assert_eq!((before.band, before.ring_width), (3, 4));
    assert_eq!((after.band, after.ring_width), (4, 6));
    assert_eq!(shadow_of(&before), (30.0, 5.0, 0.0, 5.0));
    assert_eq!(shadow_of(&after), (52.5, 9.0, 0.0, 9.0));
    // The layout reads each output's own view, so a window that lands on the other output
    // is laid out with that output's band.
    assert_eq!(engine.effective_config(low).border_inset(), 3);
    assert_eq!(engine.effective_config(high).border_inset(), 4);
}

#[test]
fn parity_a_scale_change_projects_the_decorations_again() {
    let (mut engine, outputs) = engine_with(fractional_config(), &[("rescaled", 1.0)]);
    let output = outputs[0];
    assert_eq!(engine.effective_config(output).border_inset(), 3);
    let (bounds, work) = {
        let monitor = engine.monitors.get(&output).unwrap();
        (monitor.bounds, monitor.work_area)
    };
    engine.sync_monitor_with_scale(output, bounds, work, 2.0);
    assert_eq!(engine.effective_config(output).border_inset(), 5);
    let frame = decorate(&mut engine, output, wid(51));
    assert_eq!((frame.band, frame.ring_width), (5, 7));
    // Back to 100%: the historic pixels.
    engine.sync_monitor_with_scale(output, bounds, work, 1.0);
    let frame = decorate(&mut engine, output, wid(51));
    assert_eq!((frame.band, frame.ring_width), (3, 4));
}

#[test]
fn parity_scale_one_keeps_integer_widths_and_shadow_bit_identical() {
    let mut config = Config::default();
    config.layout.effects.shadow.on = true;
    let cfg = LayoutConfig::from_config_at_scale(&config, 1.0);
    assert_eq!(cfg.border_inset(), 4);
    let urgent = HashSet::new();
    let inputs = FrameInputs {
        cfg: &cfg,
        focus_monitor: true,
        active: Some(wid(2)),
        urgent: &urgent,
        monitor_bounds: Rect::new(0, 0, 1366, 768),
        view: Rect::new(8, 8, 1350, 752),
        accent: None,
        radius: 8.0,
        lone_tile: false,
    };
    let tile = (wid(2), Rect::new(8, 8, 400, 752), DrawMode::Inset);
    let specs = frame_specs(&inputs, &[tile]);
    let DecorContent::Frame(frame) = &specs[0].content else {
        panic!("expected a frame");
    };
    assert_eq!((frame.band, frame.ring_width), (4, 3));
    assert_eq!(shadow_of(frame), (30.0, 5.0, 0.0, 5.0));
}

#[test]
fn parity_fractional_widths_survive_the_decoder_and_project_per_output() {
    let config = crate::config::parse::parse_kdl_config(
        r##"
layout { border { width 2.5; } focus-ring { width 3.25; } }
window-rule {
    border { width 1.5; }
    focus-ring { width 0.4; }
}
"##,
    )
    .unwrap();
    assert_eq!(config.layout.effects.border.width, 2.5);
    assert_eq!(config.layout.effects.focus_ring.width, 3.25);
    let ctx = crate::config::types::MatcherContext::from_legacy("", "", None, None);
    let rule = crate::window::resolve_window_rules(&config.window_rules, &ctx);
    assert_eq!(rule.effects.border.width, Some(1.5));
    assert_eq!(rule.effects.focus_ring.width, Some(0.4));
    // 2.5 -> 3 at 100%; the rule's 1.5 -> 2 and its 0.4 -> 1 (never invisible).
    let cfg = LayoutConfig::from_config_at_scale(&config, 1.0);
    assert_eq!(cfg.border_inset(), 3);
    assert_eq!(cfg.focus_ring_width, 3);
    assert_eq!(cfg.border_inset_with(&rule.effects.border), 2);
    assert_eq!(ring_width_px(rule.effects.focus_ring.width.unwrap(), 1.0), 1);
    // At 150%: 3.75 -> 4 and 4.875 -> 5; the rule's 2.25 -> 2 and 0.6 -> 1.
    let cfg = LayoutConfig::from_config_at_scale(&config, 1.5);
    assert_eq!(cfg.border_inset(), 4);
    assert_eq!(cfg.focus_ring_width, 5);
    assert_eq!(cfg.border_inset_with(&rule.effects.border), 2);
    assert_eq!(ring_width_px(rule.effects.focus_ring.width.unwrap(), 1.5), 1);
}
