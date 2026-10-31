//! Read-only output/keyboard queries and temporary, target-only output changes.
//! Physical changes use the same conservative CCD planner/executor as config
//! outputs. Nothing here writes a file, reloads config, changes Windows DPI or
//! chooses a Windows primary display.
use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::backend::outputs::{self, ipc::OutputSnapshot, OutputIdentity, OutputMatcher, Rotation};
use crate::backend::{BackendHandle, MonitorInfo};
use crate::config::{Config, OutputConfig, Position};
use crate::layout::TilingEngine;

/// The payload of Outputs, not its dialect-specific response envelope.
pub fn outputs(engine: &TilingEngine, backend: Option<&BackendHandle>) -> Value {
    let monitors = query_monitors(engine, backend);
    // With no backend (including synthetic engine tests), do not inspect the
    // host desktop. A failed CCD query leaves only fields we actually know.
    let snapshots = if backend.is_some() {
        outputs::ipc::snapshots().unwrap_or_else(|error| {
            tracing::debug!(%error, "Output details unavailable");
            Vec::new()
        })
    } else {
        Vec::new()
    };
    render_outputs(engine, &monitors, &snapshots)
}

pub fn focused_output(engine: &TilingEngine, backend: Option<&BackendHandle>) -> Value {
    let Some(id) = engine.focused_output() else {
        return Value::Null;
    };
    let name = engine.output_name(id).map(str::to_owned).or_else(|| {
        backend
            .and_then(|backend| backend.get_monitor(id))
            .map(|m| m.name)
    });
    let Some(name) = name else {
        return Value::Null;
    };
    outputs(engine, backend)
        .get(&name)
        .cloned()
        .unwrap_or(Value::Null)
}

pub fn keyboard_layouts() -> Value {
    super::keyboard::keyboard_layouts()
}

fn query_monitors(engine: &TilingEngine, backend: Option<&BackendHandle>) -> Vec<MonitorInfo> {
    let mut monitors = backend.map(BackendHandle::get_monitors).unwrap_or_default();
    for (id, monitor) in engine.monitors().iter() {
        if monitors.iter().any(|known| known.id == *id) {
            continue;
        }
        let Some(name) = engine.output_name(*id) else {
            continue;
        };
        monitors.push(MonitorInfo {
            id: *id,
            name: name.to_owned(),
            bounds: monitor.bounds,
            work_area: monitor.work_area,
            is_primary: false,
            scale_factor: monitor.scale_factor,
        });
    }
    monitors
}

fn render_outputs(
    engine: &TilingEngine,
    monitors: &[MonitorInfo],
    snapshots: &[OutputSnapshot],
) -> Value {
    let mut result = BTreeMap::new();
    for monitor in monitors {
        let snapshot = snapshots
            .iter()
            .find(|snapshot| connector_eq(&snapshot.identity.connector, &monitor.name));
        let identity = snapshot.map(|snapshot| &snapshot.identity);
        let modes: Vec<Value> = snapshot.map_or_else(Vec::new, |snapshot| {
            snapshot
                .modes
                .iter()
                .map(|mode| {
                    json!({
                        "width": mode.width,
                        "height": mode.height,
                        "refresh_rate": mode.refresh_rate,
                        "is_preferred": mode.is_preferred,
                    })
                })
                .collect()
        });
        let logical = engine.monitors().get(&monitor.id).map(|logical| {
            let scale = valid_scale(logical.scale_factor).unwrap_or(1.0);
            json!({
                // Windows' virtual-desktop origins are the engine's origins;
                // width/height are logical, not physical pixels.
                "x": logical.bounds.loc.x,
                "y": logical.bounds.loc.y,
                "width": (f64::from(logical.bounds.size.w) / scale).round() as u32,
                "height": (f64::from(logical.bounds.size.h) / scale).round() as u32,
                "scale": scale,
                "transform": snapshot.map(|snapshot| rotation_name(snapshot.rotation)),
            })
        });
        result.insert(
            monitor.name.clone(),
            json!({
                "name": monitor.name,
                "make": identity.map_or("", |identity| identity.make.as_str()),
                "model": identity.map_or("", |identity| identity.model.as_str()),
                "serial": identity.map_or("", |identity| identity.serial.as_str()),
                "physical_size": snapshot.and_then(|snapshot| snapshot.physical_size),
                "modes": modes,
                "current_mode": snapshot.and_then(|snapshot| snapshot.current_mode),
                // No reliable per-output Windows capability/state source yet.
                "vrr_supported": Value::Null,
                "vrr_enabled": Value::Null,
                "max_bpc": Value::Null,
                "logical": logical,
            }),
        );
    }
    json!(result)
}

fn rotation_name(rotation: Rotation) -> &'static str {
    match rotation {
        Rotation::Normal => "Normal",
        Rotation::Rotate90 => "90",
        Rotation::Rotate180 => "180",
        Rotation::Rotate270 => "270",
    }
}

fn valid_scale(scale: f64) -> Option<f64> {
    (scale.is_finite() && scale > 0.0).then_some(scale)
}

fn connector_eq(a: &str, b: &str) -> bool {
    let short = |name: &str| {
        name.trim()
            .trim_start_matches("\\\\.\\")
            .to_ascii_lowercase()
    };
    !a.trim().is_empty() && !b.trim().is_empty() && short(a) == short(b)
}

/// Connector names always take precedence over friendly/EDID identities. An
/// ambiguous identity is never resolved by enumeration order or a prefix.
fn resolve_target(
    name: &str,
    monitors: &[MonitorInfo],
    snapshots: &[OutputSnapshot],
) -> Result<MonitorInfo, String> {
    let connectors: Vec<_> = monitors
        .iter()
        .filter(|monitor| connector_eq(name, &monitor.name))
        .collect();
    let matches = if connectors.is_empty() {
        monitors
            .iter()
            .filter(|monitor| {
                snapshots.iter().any(|snapshot| {
                    connector_eq(&snapshot.identity.connector, &monitor.name)
                        && snapshot.identity.matches(name)
                })
            })
            .collect::<Vec<_>>()
    } else {
        connectors
    };
    match matches.as_slice() {
        [monitor] => Ok((*monitor).clone()),
        [] => Err(format!("output '{name}' is not an active connected output; enabling an inactive output is unsupported")),
        _ => Err(format!("output '{name}' is ambiguous; use an exact connector name")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
enum OutputAction {
    On,
    Off,
    Mode { mode: ModeValue },
    Scale { scale: ScaleValue },
    Transform { transform: String },
    Position { position: PositionValue },
    Vrr { vrr: bool },
    MaxBpc { max_bpc: u8 },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ModeValue {
    Text(String),
    Specific(ModeSpec),
    Tagged(TaggedMode),
}

#[derive(Debug, Deserialize)]
enum TaggedMode {
    Specific(ModeSpec),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeSpec {
    width: u32,
    height: u32,
    /// Hz, with at most millihertz precision; absent preserves current refresh.
    refresh: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ScaleValue {
    Number(f64),
    Text(String),
    Tagged(TaggedScale),
}

#[derive(Debug, Deserialize)]
enum TaggedScale {
    Specific(f64),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PositionValue {
    Text(String),
    Specific(Coordinates),
    Tagged(TaggedPosition),
}

#[derive(Debug, Deserialize)]
enum TaggedPosition {
    Specific(Coordinates),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Coordinates {
    x: i32,
    y: i32,
}

#[derive(Debug, PartialEq)]
enum Change {
    On,
    Off,
    Mode(String),
    PreferredMode,
    Scale(Option<f64>),
    Transform(&'static str),
    Position(i32, i32),
}

fn parse_action(value: &Value) -> Result<Change, String> {
    let action: OutputAction = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid output action: {error}"))?;
    Ok(match action {
        OutputAction::On => Change::On,
        OutputAction::Off => Change::Off,
        OutputAction::Mode { mode } => {
            let text = match mode {
                ModeValue::Text(text)
                    if text == "Automatic" || text.eq_ignore_ascii_case("auto") =>
                {
                    return Ok(Change::PreferredMode)
                }
                ModeValue::Text(text) => text,
                ModeValue::Specific(spec) | ModeValue::Tagged(TaggedMode::Specific(spec)) => {
                    let mut text = format!("{}x{}", spec.width, spec.height);
                    if let Some(refresh) = spec.refresh {
                        if !refresh.is_finite() || refresh <= 0.0 {
                            return Err("mode refresh must be finite and positive Hz".into());
                        }
                        text.push_str(&format!("@{refresh}"));
                    }
                    text
                }
            };
            outputs::parse_mode(&text)?;
            Change::Mode(text)
        }
        OutputAction::Scale { scale } => {
            let number = match scale {
                ScaleValue::Text(text)
                    if text == "Automatic" || text.eq_ignore_ascii_case("auto") =>
                {
                    return Ok(Change::Scale(None))
                }
                ScaleValue::Text(text) => text.parse::<f64>().map_err(|_| "scale must be a positive number or Auto")?,
                ScaleValue::Number(number) | ScaleValue::Tagged(TaggedScale::Specific(number)) => number,
            };
            if !number.is_finite() || !(0.1..=10.0).contains(&number) {
                return Err("output scale must be finite and between 0.1 and 10 (or Auto)".into());
            }
            Change::Scale(Some(number))
        }
        OutputAction::Transform { transform } => Change::Transform(match transform.to_ascii_lowercase().as_str() {
            "normal" => "normal",
            "90" => "90",
            "180" => "180",
            "270" => "270",
            name if name.starts_with("flipped") => return Err("flipped output transforms are unsupported on Windows".into()),
            _ => return Err(format!("unknown output transform '{transform}'")),
        }),
        OutputAction::Position { position } => {
            let coordinates = match position {
                PositionValue::Specific(position) | PositionValue::Tagged(TaggedPosition::Specific(position)) => position,
                PositionValue::Text(text) if text.eq_ignore_ascii_case("auto") => return Err("automatic output placement is owned by Windows; no changes made".into()),
                PositionValue::Text(_) => return Err("output position must be {x,y} with integer coordinates".into()),
            };
            // The engine's rectangles use signed right/bottom coordinates.
            if coordinates.x.checked_add(100_000).is_none() || coordinates.y.checked_add(100_000).is_none() {
                return Err("output position would overflow desktop coordinates".into());
            }
            Change::Position(coordinates.x, coordinates.y)
        }
        OutputAction::Vrr { vrr } => return Err(format!("per-output VRR ({vrr}) is controlled by Windows/the GPU driver and is unsupported")),
        OutputAction::MaxBpc { max_bpc } => return Err(format!("per-output max-bpc ({max_bpc}) is controlled by Windows/the GPU driver and is unsupported")),
    })
}

fn neutral_output(name: &str) -> OutputConfig {
    let mut output = Config::default().output.remove(0);
    output.name = name.into();
    output.enable = true;
    output.width = 0;
    output.height = 0;
    output.mode.clear();
    output.position_set = false;
    output.scale = 0.0;
    output.transform.clear();
    output.vrr = false;
    output.primary = false;
    output.focus_at_startup = false;
    output
}

fn patch_output(output: &mut OutputConfig, change: &Change) {
    match change {
        Change::On => output.enable = true,
        Change::Off => output.enable = false,
        Change::Mode(mode) => {
            output.mode = mode.clone();
            output.width = 0;
            output.height = 0;
        }
        Change::Scale(scale) => output.scale = scale.unwrap_or(0.0),
        Change::Transform(transform) => output.transform = (*transform).into(),
        Change::Position(x, y) => {
            output.position_set = true;
            output.position = Position::new(*x, *y);
        }
        Change::PreferredMode => unreachable!("preferred mode must be resolved before patching"),
    }
}

fn temporary_config(
    config: &Config,
    monitor: &MonitorInfo,
    identities: Vec<OutputIdentity>,
    change: &Change,
) -> Config {
    let matcher = OutputMatcher { identities };
    let mut output = config
        .output
        .iter()
        .find(|output| connector_eq(&output.name, &monitor.name))
        .or_else(|| matcher.matching_config(config, monitor))
        .cloned()
        .unwrap_or_else(|| neutral_output(&monitor.name));
    // A connector-specific block shadows a shared model/default block only for
    // this output, retaining layout/mod-key overrides and previous IPC changes.
    output.name = monitor.name.clone();
    patch_output(&mut output, change);
    let mut result = config.clone();
    result
        .output
        .retain(|known| !connector_eq(&known.name, &monitor.name));
    result.output.insert(0, output);
    result
}

/// Validate first; commit the in-memory override only after Windows accepted
/// the exact target's plan. Reloading the user's config naturally discards it.
pub fn change_output(
    name: &str,
    action: &Value,
    engine: &Arc<RwLock<TilingEngine>>,
    backend: Option<&BackendHandle>,
) -> Result<Value, String> {
    let mut change = parse_action(action)?;
    let backend = backend.ok_or("output changes require a live Windows backend")?;
    let monitors = backend.get_monitors();
    let snapshots = outputs::ipc::snapshots().unwrap_or_default();
    let monitor = resolve_target(name, &monitors, &snapshots)?;
    if change == Change::PreferredMode {
        let mode = snapshots
            .iter()
            .find(|snapshot| connector_eq(&snapshot.identity.connector, &monitor.name))
            .and_then(|snapshot| snapshot.modes.iter().find(|mode| mode.is_preferred))
            .ok_or("Windows did not report an exact preferred mode; no changes made")?;
        change = Change::Mode(format!(
            "{}x{}@{}.{:03}",
            mode.width,
            mode.height,
            mode.refresh_rate / 1000,
            mode.refresh_rate % 1000
        ));
    }
    let mut engine = engine.write();
    let config = engine
        .full_config()
        .ok_or("output changes require a loaded configuration")?;
    let config = temporary_config(
        config,
        &monitor,
        snapshots
            .iter()
            .map(|snapshot| snapshot.identity.clone())
            .collect(),
        &change,
    );
    let physical_change = !matches!(change, Change::Scale(_));
    if physical_change {
        // Never send accumulated physical overrides: this request changes one
        // field of one output. In particular, Scale never reaches CCD apply.
        let mut physical = neutral_output(&monitor.name);
        patch_output(&mut physical, &change);
        outputs::ipc::apply_one(&monitor.name, &physical)?;
        crate::backend::output_runtime::refresh(backend);
    }
    engine.set_full_config(config.clone());
    if physical_change {
        // Windows may re-anchor other desktop rectangles when a path disappears.
        // Sync the complete refreshed geometry without replacing workspaces.
        crate::backend::output_runtime::sync(&config, backend, &mut engine);
    } else if let Some(current) = backend.get_monitor(monitor.id) {
        let setting = config.output.first(); // just inserted, exact connector
        if setting.is_some_and(|output| !output.enable) {
            // Changing layout scale must never enable an output disabled in wiri.
            engine.unregister_monitor(&monitor.id);
        } else {
            let scale = outputs::effective_scale(setting, &current);
            engine.sync_monitor_with_scale(current.id, current.bounds, current.work_area, scale);
            engine.set_output_name(current.id, &current.name);
        }
    }
    // Scale-only changes generate no WM_DISPLAYCHANGE. Apply the new layout
    // now rather than waiting for an unrelated action to position the windows.
    engine.apply_all(backend);
    drop(engine);
    super::notify_state_changed();
    Ok(json!({"output": monitor.name, "temporary": true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::{OutputId, Rect};

    fn monitor(name: &str) -> MonitorInfo {
        MonitorInfo {
            id: OutputId::from_name(name),
            name: name.into(),
            bounds: Rect::new(-1280, 0, 1280, 720),
            work_area: Rect::new(-1280, 0, 1280, 680),
            is_primary: false,
            scale_factor: 1.5,
        }
    }

    #[test]
    fn typed_actions_reject_bad_values_before_any_backend_access() {
        for value in [
            json!({"Scale":{"scale":0}}),
            json!({"Scale":{"scale":"NaN"}}),
            json!({"Scale":{"scale":true}}),
            json!({"Mode":{"mode":"0x1080"}}),
            json!({"Mode":{"mode":"1920x1080@59.9401"}}),
            json!({"Transform":{"transform":"Flipped90"}}),
            json!({"Position":{"position":{"x":1.5,"y":0}}}),
            json!({"Vrr":{"vrr":false}}),
            json!({"MaxBpc":{"max_bpc":10}}),
            json!({"Position":{"position":"Auto"}}),
            json!({"On":{}}),
            json!({"Scale":{"scale":2,"unexpected":true}}),
        ] {
            assert!(parse_action(&value).is_err(), "{value}");
        }
        assert_eq!(
            parse_action(&json!({"Scale":{"scale":"Auto"}})).unwrap(),
            Change::Scale(None)
        );
        assert_eq!(
            parse_action(&json!({"Scale":{"scale":1.25}})).unwrap(),
            Change::Scale(Some(1.25))
        );
        assert_eq!(
            parse_action(&json!({"Mode":{"mode":"1920x1080@59.940"}})).unwrap(),
            Change::Mode("1920x1080@59.940".into())
        );
        assert_eq!(
            parse_action(&json!({"Position":{"position":{"Specific":{"x":-1920,"y":0}}}})).unwrap(),
            Change::Position(-1920, 0)
        );
    }

    #[test]
    fn parity_canonical_action_output_mode_automatic_selects_preferred_mode() {
        // niri-ipc ModeToSet::Automatic serializes as "Automatic", not "auto".
        assert_eq!(
            parse_action(&json!({"Mode":{"mode":"Automatic"}})).unwrap(),
            Change::PreferredMode
        );
    }

    #[test]
    fn parity_canonical_action_output_scale_automatic_clears_layout_override() {
        assert_eq!(
            parse_action(&json!({"Scale":{"scale":"Automatic"}})).unwrap(),
            Change::Scale(None)
        );
    }

    #[test]
    fn parity_canonical_action_output_automatic_keeps_legacy_and_specific_controls() {
        for alias in ["auto", "Auto", "AUTO"] {
            assert_eq!(
                parse_action(&json!({"Mode":{"mode":alias}})).unwrap(),
                Change::PreferredMode
            );
            assert_eq!(
                parse_action(&json!({"Scale":{"scale":alias}})).unwrap(),
                Change::Scale(None)
            );
        }
        assert_eq!(
            parse_action(&json!({"Mode":{"mode":{"Specific":{
                "width":1920,"height":1080,"refresh":60.0
            }}}}))
            .unwrap(),
            Change::Mode("1920x1080@60".into())
        );
        assert_eq!(
            parse_action(&json!({"Scale":{"scale":{"Specific":1.5}}})).unwrap(),
            Change::Scale(Some(1.5))
        );
    }

    #[test]
    fn parity_canonical_action_output_rejects_malformed_automatic_and_specific_shapes() {
        for value in [
            json!({"Mode":{}}),
            json!({"Scale":{}}),
            json!({"Mode":{"mode":null}}),
            json!({"Scale":{"scale":null}}),
            json!({"Mode":{"mode":{"Automatic":1}}}),
            json!({"Scale":{"scale":{"Automatic":1}}}),
            json!({"Mode":{"mode":{"Specific":{"width":"1920","height":1080}}}}),
            json!({"Scale":{"scale":{"Specific":"1.5"}}}),
            json!({"Mode":{"mode":{"Specific":{"width":1920}}}}),
            json!({"Scale":{"scale":{"Specific":0}}}),
        ] {
            assert!(parse_action(&value).is_err(), "{value}");
        }
    }

    #[test]
    fn connector_aliases_resolve_exactly_and_duplicates_fail_closed() {
        let a = monitor("\\\\.\\DISPLAY1");
        let b = monitor("\\\\.\\DISPLAY10");
        assert_eq!(
            resolve_target("display1", &[b.clone(), a.clone()], &[])
                .unwrap()
                .id,
            a.id
        );
        assert!(resolve_target("DISPLAY", &[a.clone(), b], &[]).is_err());
        assert!(resolve_target("display1", &[a.clone(), a], &[]).is_err());
    }

    #[test]
    fn overrides_are_target_only_preserve_prior_actions_and_never_touch_source() {
        let a = monitor("\\\\.\\DISPLAY1");
        let b = monitor("\\\\.\\DISPLAY2");
        let mut source = Config::default();
        source.output[0].scale = 1.25;
        source.output[0].mod_key = Some("Alt".into());
        let first = temporary_config(&source, &a, vec![], &Change::Mode("1280x720@59.940".into()));
        let second = temporary_config(&first, &a, vec![], &Change::Scale(Some(2.0)));
        let third = temporary_config(&second, &b, vec![], &Change::Transform("90"));
        let a_setting = third.output.iter().find(|out| out.name == a.name).unwrap();
        assert_eq!(a_setting.mode, "1280x720@59.940");
        assert_eq!(a_setting.scale, 2.0);
        assert_eq!(a_setting.mod_key.as_deref(), Some("Alt"));
        assert_eq!(third.output[0].name, b.name);
        assert_eq!(third.output[0].scale, 1.25);
        assert_eq!(source.output.len(), 1);
        assert!(source.output[0].mode.is_empty());
        assert!(source.output[0].name.is_empty());
    }

    #[test]
    fn temporary_connector_alias_keeps_existing_connector_overrides() {
        let monitor = monitor("\\\\.\\DISPLAY1");
        let mut source = Config::default();
        source.output[0].mod_key = Some("Super".into());
        let mut connector = source.output[0].clone();
        connector.name = "display1".into();
        connector.mod_key = Some("Alt".into());
        connector.scale = 1.75;
        source.output.push(connector);
        let changed = temporary_config(&source, &monitor, vec![], &Change::Position(-1920, 0));
        assert_eq!(changed.output.len(), 2);
        assert_eq!(changed.output[0].name, monitor.name);
        assert_eq!(changed.output[0].mod_key.as_deref(), Some("Alt"));
        assert_eq!(changed.output[0].scale, 1.75);
        assert_eq!(source.output[1].name, "display1");
    }

    #[test]
    fn synthetic_queries_use_engine_scale_without_inventing_monitor_details() {
        let monitor = monitor("synthetic-output");
        let mut engine = TilingEngine::new(crate::layout::LayoutConfig::default());
        engine.register_monitor_with_scale(monitor.id, monitor.bounds, monitor.work_area, 2.0);
        engine.set_output_name(monitor.id, &monitor.name);
        let value = outputs(&engine, None);
        let output = &value[&monitor.name];
        assert_eq!(output["name"], monitor.name);
        assert_eq!(output["logical"]["x"], -1280);
        assert_eq!(output["logical"]["width"], 640);
        assert_eq!(output["logical"]["height"], 360);
        assert_eq!(output["logical"]["scale"], 2.0);
        assert!(output["logical"]["transform"].is_null());
        assert_eq!(output["modes"], json!([]));
        assert_eq!(output["make"], "");
        for key in [
            "physical_size",
            "current_mode",
            "vrr_supported",
            "vrr_enabled",
            "max_bpc",
        ] {
            assert!(output[key].is_null(), "{key}");
        }
        assert_eq!(focused_output(&engine, None), *output);
    }

    #[test]
    fn focused_output_is_null_without_a_named_monitor_and_tracks_engine_focus() {
        let mut engine = TilingEngine::new(crate::layout::LayoutConfig::default());
        assert_eq!(outputs(&engine, None), json!({}));
        assert!(focused_output(&engine, None).is_null());
        let a = monitor("synthetic-a");
        let b = monitor("synthetic-b");
        engine.register_monitor(a.id, a.bounds, a.work_area);
        assert!(focused_output(&engine, None).is_null());
        engine.set_output_name(a.id, &a.name);
        engine.register_monitor(b.id, b.bounds, b.work_area);
        engine.set_output_name(b.id, &b.name);
        engine.set_focused_output(b.id);
        assert_eq!(focused_output(&engine, None)["name"], b.name);
    }

    #[test]
    fn documented_output_example_matches_a_synthetic_snapshot() {
        let mut monitor = monitor("DISPLAY1");
        monitor.bounds = Rect::new(0, 0, 1366, 768);
        monitor.work_area = Rect::new(0, 0, 1366, 728);
        monitor.scale_factor = 1.0;
        let mut engine = TilingEngine::new(crate::layout::LayoutConfig::default());
        engine.register_monitor(monitor.id, monitor.bounds, monitor.work_area);
        engine.set_output_name(monitor.id, &monitor.name);
        let snapshot = OutputSnapshot {
            identity: OutputIdentity {
                connector: monitor.name.clone(),
                ..OutputIdentity::default()
            },
            modes: vec![outputs::ipc::OutputModeSnapshot {
                width: 1366,
                height: 768,
                refresh_rate: 59_940,
                is_preferred: true,
            }],
            current_mode: Some(0),
            physical_size: None,
            rotation: Rotation::Normal,
        };
        let actual = render_outputs(&engine, &[monitor], &[snapshot]);
        let schema: Value =
            serde_json::from_str(include_str!("../../docs/IPC_SCHEMA.json")).unwrap();
        let definition = &schema["$defs"]["OutputIpc"];
        assert_eq!(actual["DISPLAY1"], definition["examples"][0]);
        assert_eq!(
            actual["DISPLAY1"].as_object().unwrap().len(),
            definition["required"].as_array().unwrap().len()
        );
        for key in definition["required"].as_array().unwrap() {
            assert!(actual["DISPLAY1"].get(key.as_str().unwrap()).is_some());
        }
    }

    #[test]
    fn scale_plan_never_contains_a_physical_change() {
        let mut output = neutral_output("connector");
        patch_output(&mut output, &Change::Scale(Some(1.75)));
        let current = outputs::OutputState {
            mode: outputs::OutputMode {
                width: 1920,
                height: 1080,
                refresh_millihz: Some(59_940),
            },
            position: (-1920, 0),
            rotation: Rotation::Rotate90,
        };
        assert!(!outputs::plan_output(&output, &current)
            .unwrap()
            .changes_windows());
    }
}
