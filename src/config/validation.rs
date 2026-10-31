//! Structural checks kept separate from decoding. Includes are independent
//! parts (niri ConfigPart); duplicates within each physical file are errors.
use super::decode::{ValidationIssue, ValidationSeverity};
use super::diagnostics::{DiagnosticContext, SourceLabel};
use super::kdl::{Node, Pos, Scalar, Value};
use std::collections::HashMap;

fn norm(s: &str) -> String {
    s.replace('_', "-").to_ascii_lowercase()
}
fn issue(file: &str, source: &str, value: &Value, message: String) -> ValidationIssue {
    issue_at(file, source, value.pos, message)
}
fn issue_at(file: &str, source: &str, pos: Pos, message: String) -> ValidationIssue {
    ValidationIssue {
        severity: ValidationSeverity::Error,
        file: file.into(),
        line: pos.line as usize,
        column: pos.col as usize,
        message,
        context: DiagnosticContext {
            source_line: source
                .lines()
                .nth(pos.line.saturating_sub(1) as usize)
                .unwrap_or("")
                .into(),
            ..Default::default()
        },
    }
}
fn duplicate(file: &str, source: &str, node: &Node, first: &Node, key: &str) -> ValidationIssue {
    let label = SourceLabel::at(file, source, node.pos, "duplicate definition");
    ValidationIssue { severity: ValidationSeverity::Error, file: file.into(), line: label.line,
        column: label.column, message: format!("duplicate node '{key}'; single node expected"),
        context: DiagnosticContext { source_line: label.source_line,
            related: vec![SourceLabel::at(file, source, first.pos, "first definition is here")],
            help: "combine these sections in one block (or use include for a separate configuration part)".into(), ..Default::default() } }
}

pub fn check(nodes: &[Node], file: &str, source: &str) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    check_level(nodes, "root", file, source, &mut issues);
    check_ranges(nodes, "root", file, source, &mut issues);
    issues
}

fn check_level(
    nodes: &[Node],
    parent: &str,
    file: &str,
    source: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut seen: HashMap<String, &Node> = HashMap::new();
    for node in nodes {
        let key = norm(&node.name);
        let repeatable = match parent {
            "root" => matches!(
                key.as_str(),
                "spawn-at-startup"
                    | "spawn-sh-at-startup"
                    | "window-rule"
                    | "layer-rule"
                    | "include"
                    | "output"
            ),
            "workspace" => key == "open-on-output",
            "window-rule" | "layer-rule" => matches!(key.as_str(), "match" | "exclude"),
            _ => false,
        };
        let identity = if parent == "root" && matches!(key.as_str(), "output" | "workspace") {
            format!(
                "{} {}",
                key,
                node.args
                    .first()
                    .map(|value| {
                        let name = value.to_text();
                        if key == "output" {
                            name.to_ascii_lowercase()
                        } else {
                            name
                        }
                    })
                    .unwrap_or_default()
            )
        } else if parent == "animations" && key == "window-resize" {
            // Wiri's legacy boolean and niri's animation block are separate fields.
            format!("{}:{}", key, node.children.is_some())
        } else {
            key.clone()
        };
        if !repeatable {
            if let Some(first) = seen.get(&identity) {
                issues.push(duplicate(file, source, node, first, &identity));
            } else {
                seen.insert(identity, node);
            }
        }
        // Lists (presets, rules and binds) are not singleton maps.
        if matches!(
            key.as_str(),
            "input"
                | "keyboard"
                | "xkb"
                | "mouse"
                | "touchpad"
                | "trackpoint"
                | "trackball"
                | "tablet"
                | "touch"
                | "focus"
                | "output"
                | "workspace"
                | "layout"
                | "border"
                | "borders"
                | "focus-ring"
                | "shadow"
                | "shadows"
                | "struts"
                | "cursor"
                | "config-notification"
                | "switch-events"
                | "animations"
                | "window-rule"
                | "layer-rule"
        ) {
            check_level(node.kids(), &key, file, source, issues);
        }
    }
}

fn number(value: &Value) -> Option<f64> {
    match &value.scalar {
        Scalar::Int(n) => Some(*n as f64),
        Scalar::Float(n) => Some(*n),
        Scalar::Str { text, .. } => text.parse().ok(),
        _ => None,
    }
}

fn check_ranges(
    nodes: &[Node],
    parent: &str,
    file: &str,
    source: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    for node in nodes {
        let key = norm(&node.name);
        // (min, max, the decoder leaves the value unread because Windows has no equivalent:
        // such an option must still get niri's argument grammar and range checks).
        let bounds: Option<(f64, f64, bool)> = match (parent, key.as_str()) {
            ("mouse" | "touchpad" | "trackpoint" | "trackball", "accel-speed") => {
                Some((-1.0, 1.0, true))
            }
            ("mouse" | "touchpad" | "trackpoint" | "trackball", "scroll-factor") => {
                Some((0.0, 100.0, false))
            }
            ("window-rule", "pinch-sensitivity") => Some((0.0, 100.0, true)),
            // Even unsupported device options must not accept nonsensical values.
            _ => None,
        };
        if let Some((min, max, require_argument)) = bounds {
            if require_argument {
                // niri's `#[knuffel(child, unwrap(argument))]` on these options means a bare
                // node is a missing-argument error (niri-config/src/input.rs:143-231), not
                // "leave it at the default".
                match node.args.first() {
                    None => issues.push(issue_at(
                        file,
                        source,
                        node.pos,
                        format!("'{}' needs a value", node.name),
                    )),
                    Some(value) => {
                        if node.args.len() > 1 {
                            issues.push(issue(
                                file,
                                source,
                                &node.args[1],
                                format!("'{}' takes one value", node.name),
                            ));
                        }
                        if !number(value).is_some_and(|n| n.is_finite() && (min..=max).contains(&n))
                        {
                            issues.push(issue(
                                file,
                                source,
                                value,
                                format!("'{}' must be between {} and {}", node.name, min, max),
                            ));
                        }
                    }
                }
                continue;
            }
            if let Some(value) = node.args.first() {
                if !number(value).is_some_and(|n| n.is_finite() && (min..=max).contains(&n)) {
                    issues.push(issue(
                        file,
                        source,
                        value,
                        format!("'{}' must be between {} and {}", node.name, min, max),
                    ));
                }
            }
        }
        if key == "scroll-factor" {
            for prop in &node.props {
                if matches!(prop.key.as_str(), "horizontal" | "vertical")
                    && !number(&prop.value)
                        .is_some_and(|n| n.is_finite() && (-100.0..=100.0).contains(&n))
                {
                    issues.push(issue(
                        file,
                        source,
                        &prop.value,
                        format!("'{}' must be between -100 and 100", prop.key),
                    ));
                }
            }
        }
        check_ranges(node.kids(), &key, file, source, issues);
    }
}

#[cfg(test)]
mod tests {
    use crate::config::decode;
    #[test]
    fn duplicate_and_range_errors_are_both_reported() {
        let d = decode::decode_text(
            "input {}\ninput { mouse { accel-speed 2; } }\noutput \"a\" { scale 11; }",
            "test.kdl",
            None,
        );
        let errors: Vec<_> = d
            .issues
            .iter()
            .filter(|i| i.severity == super::ValidationSeverity::Error)
            .collect();
        assert_eq!(errors.len(), 3, "{errors:?}");
        let dup = errors
            .iter()
            .find(|i| i.message.contains("duplicate"))
            .unwrap();
        assert_eq!(dup.context.related[0].line, 1);
        assert_eq!(dup.line, 2);
    }
    /// Live fixtures used to append a second `binds {}` to a config that already had
    /// one (Lane S, H47/H49); that is a duplicate singleton section, so the reload was
    /// refused and the new chord never bound. One section per physical file is the rule.
    #[test]
    fn a_second_binds_section_in_one_file_is_a_duplicate() {
        let d = decode::decode_text(
            "binds { Ctrl+Alt+F8 { screenshot; } }\nbinds { Ctrl+Alt+F9 { screenshot; } }\n",
            "test.kdl",
            None,
        );
        assert_eq!(d.error_count(), 1, "{:?}", d.issues);
        let issue = &d.issues[0];
        assert!(
            issue.message.contains("duplicate node 'binds'"),
            "{issue:?}"
        );
        assert_eq!(issue.line, 2);
        assert_eq!(issue.context.related[0].line, 1);
        let one = decode::decode_text("binds { Ctrl+Alt+F9 { screenshot; } }\n", "test.kdl", None);
        assert_eq!(one.error_count(), 0, "{:?}", one.issues);
    }
    #[test]
    fn output_parts_and_rule_matches_repeat_but_rule_values_do_not() {
        let d = decode::decode_text("output \"DP-1\" { scale 1; }\noutput \"DP-1\" { enable true; }\nwindow-rule { match title=\"a\"; match title=\"b\"; opacity 0.3; opacity 0.8; }", "", None);
        assert_eq!(d.error_count(), 1, "{:?}", d.issues);
        assert!(d.issues[0].message.contains("opacity"));
    }
    #[test]
    fn explicit_notification_and_empty_binds_have_no_implicit_defaults() {
        let d = decode::decode_text(
            "config-notification { disable-failed; }\nbinds {}",
            "",
            None,
        );
        assert!(!d.has_errors(), "{:?}", d.issues);
        assert!(d.config.disable_failed_config_notification);
        assert!(!d.config.binds.extend_defaults);
        assert!(d.config.binds.hotkeys.is_empty());
        assert!(
            !decode::decode_text("", "", None)
                .config
                .binds
                .extend_defaults
        );
    }
    #[test]
    fn integer_options_reject_fractional_values() {
        let d = decode::decode_text("input { keyboard { repeat-delay 250.5; } }", "", None);
        assert_eq!(d.error_count(), 1, "{:?}", d.issues);
        assert!(d.issues[0].message.contains("integer"));
    }
    #[test]
    fn nested_singletons_but_priority_and_preset_lists_are_repeatable() {
        let d = decode::decode_text("input { keyboard {}\nkeyboard {} }\nworkspace \"chat\" { open-on-output \"a\"; open-on-output \"b\"; }\nlayout { preset-column-widths { proportion 0.5; proportion 0.7; } }", "", None);
        assert_eq!(d.error_count(), 1, "{:?}", d.issues);
    }

    /// Number of errors a config text produces.  Warnings (an option Windows cannot apply)
    /// are not errors, which is the point of the unsupported-option tests below.
    fn errors(text: &str) -> usize {
        let d = decode::decode_text(text, "test.kdl", None);
        assert!(d.error_count() <= 2, "{text}: {:?}", d.issues);
        d.error_count()
    }

    /// Pinned niri: `scale: Option<FloatOrInt<0, 10>>` (niri-config/src/output.rs:56-57) and
    /// `position { x: i32, y: i32 }` (niri-config/src/output.rs:130-146).  A decimal i32 is a
    /// type error in knuffel, so it must not be rounded.
    #[test]
    fn parity_bounds_output_scale_and_position() {
        assert_eq!(errors("output \"a\" { scale 0; }"), 0);
        assert_eq!(errors("output \"a\" { scale 10; }"), 0);
        assert_eq!(errors("output \"a\" { scale 10.5; }"), 1);
        assert_eq!(errors("output \"a\" { scale -0.5; }"), 1);
        assert_eq!(errors("output \"a\" { position x=0 y=-1; }"), 0);
        assert_eq!(
            errors("output \"a\" { position x=2147483647 y=-2147483648; }"),
            0
        );
        assert_eq!(errors("output \"a\" { position x=0.5 y=0; }"), 1);
        assert_eq!(errors("output \"a\" { position x=2147483648 y=0; }"), 1);
        assert_eq!(errors("output \"a\" { position 3 4; }"), 0);
        assert_eq!(errors("output \"a\" { position 3.5 4; }"), 1);
    }

    /// Pinned niri: `gaps: Option<FloatOrInt<0, 65535>>` (niri-config/src/layout.rs:120-146) and
    /// `Struts` +-65535 (niri-config/src/layout.rs:195-203).  Preset sizes keep an unbounded
    /// `proportion f64` and an integer `fixed i32` (niri-config/src/layout.rs:127-152).
    #[test]
    fn parity_bounds_layout_gaps_struts_and_presets() {
        assert_eq!(errors("layout { gaps 65535; }"), 0);
        assert_eq!(errors("layout { gaps 65536; }"), 1);
        assert_eq!(
            errors("layout { struts { left -65535; bottom 65535; } }"),
            0
        );
        assert_eq!(errors("layout { struts { left -65536; } }"), 1);
        assert_eq!(
            errors("layout { preset-column-widths { proportion 250; } }"),
            0,
            "niri keeps proportion unbounded"
        );
        assert_eq!(errors("layout { preset-column-widths { fixed 800; } }"), 0);
        assert_eq!(
            errors("layout { preset-column-widths { fixed 800.5; } }"),
            1,
            "niri i32: no rounding"
        );
        assert_eq!(
            errors("layout { preset-window-heights { fixed -1; } }"),
            1,
            "wiri stores u32; a negative fixed size is reported, not wrapped"
        );
        assert_eq!(
            errors("window-rule { default-column-width { proportion 0.25; } }"),
            0
        );
        assert_eq!(
            errors("window-rule { default-column-width { fixed 800; } }"),
            0
        );
        assert_eq!(
            errors("window-rule { default-column-width { fixed 800.5; } }"),
            1
        );
    }

    /// Pinned niri: `min_width .. max_height: Option<u16>` (niri-config/src/window_rule.rs:43-49)
    /// and `opacity: Option<f32>` with no parse bound (niri-config/src/window_rule.rs:61-62);
    /// the clamp to 0..=1 lives in `src/layout/rules.rs`.
    #[test]
    fn parity_bounds_window_rule_sizes_and_opacity() {
        assert_eq!(errors("window-rule { min-width 65535; max-height 1; }"), 0);
        assert_eq!(errors("window-rule { min-width 65536; }"), 1);
        assert_eq!(errors("window-rule { max-height 65536; }"), 1);
        assert_eq!(errors("window-rule { opacity 1.5; }"), 0);
        assert_eq!(errors("window-rule { opacity -0.5; }"), 0);
        assert_eq!(
            errors("window-rule { geometry-corner-radius -1; }"),
            0,
            "niri CornerRadius is four unbounded f32s"
        );
        assert_eq!(errors("window-rule { scroll-factor 100; }"), 0);
        assert_eq!(errors("window-rule { scroll-factor 101; }"), 1);
        assert_eq!(errors("window-rule { pinch-sensitivity 101; }"), 1);
    }

    /// Pinned niri: `slowdown: Option<FloatOrInt<0, i32::MAX>>` (niri-config/src/animations.rs:56-64),
    /// `duration_ms: u32` (niri-config/src/animations.rs:664-680) and spring parameter checks
    /// 0.1..=10.0, stiffness >= 1 (integer), epsilon 0.00001..=0.1
    /// (niri-config/src/animations.rs:886-893).
    #[test]
    fn parity_bounds_animation_slowdown_duration_and_spring() {
        assert_eq!(errors("animations { slowdown 5000; }"), 0);
        assert_eq!(errors("animations { slowdown -1; }"), 1);
        assert_eq!(
            errors("animations { window-open { duration-ms 70000; } }"),
            0
        );
        assert_eq!(errors("animations { window-open { duration-ms 1.5; } }"), 1);
        assert_eq!(
            errors(
                "animations { window-open { spring damping-ratio=10 stiffness=1 epsilon=0.1; } }"
            ),
            0
        );
        assert_eq!(
            errors("animations { window-open { spring damping-ratio=0.1 stiffness=1 epsilon=0.00001; } }"),
            0
        );
        assert_eq!(
            errors(
                "animations { window-open { spring damping-ratio=10.1 stiffness=1 epsilon=0.1; } }"
            ),
            1
        );
        assert_eq!(
            errors(
                "animations { window-open { spring damping-ratio=0.05 stiffness=1 epsilon=0.1; } }"
            ),
            1
        );
        assert_eq!(
            errors(
                "animations { window-open { spring damping-ratio=1 stiffness=0 epsilon=0.1; } }"
            ),
            1
        );
        assert_eq!(
            errors(
                "animations { window-open { spring damping-ratio=1 stiffness=5.5 epsilon=0.1; } }"
            ),
            1,
            "niri stiffness is a u32: a decimal is a type error"
        );
        assert_eq!(
            errors(
                "animations { window-open { spring damping-ratio=1 stiffness=1 epsilon=0.5; } }"
            ),
            1
        );
    }

    /// Pinned niri: gestures trigger width/height `<0, 65535>`, `delay_ms: u16`,
    /// `max_speed <0, 1_000_000>` (niri-config/src/gestures.rs:45-52, 79-86); recent-windows
    /// `debounce_ms`/`open_delay_ms: u16` (niri-config/src/recent_windows.rs:33-35),
    /// highlight padding/corner-radius `<0, 65535>` and previews max-height `<1, 65535>`,
    /// max-scale `<0, 1>` (niri-config/src/recent_windows.rs:95-100, 127-132).
    #[test]
    fn parity_bounds_gestures_and_recent_windows() {
        assert_eq!(
            errors("gestures { dnd-edge-view-scroll { trigger-width 20000; delay-ms 65535; max-speed 500000; } }"),
            0
        );
        assert_eq!(
            errors("gestures { dnd-edge-workspace-switch { trigger-height 70000; } }"),
            1
        );
        assert_eq!(
            errors("gestures { dnd-edge-view-scroll { delay-ms 65536; } }"),
            1
        );
        assert_eq!(
            errors("gestures { dnd-edge-view-scroll { max-speed 1000001; } }"),
            1
        );
        assert_eq!(
            errors("recent-windows { debounce-ms 65535; open-delay-ms 0; }"),
            0
        );
        assert_eq!(errors("recent-windows { debounce-ms 65536; }"), 1);
        assert_eq!(
            errors("recent-windows { highlight { padding 65535; corner-radius 70000; } }"),
            1
        );
        assert_eq!(
            errors("recent-windows { previews { max-height 65535; max-scale 1; } }"),
            0
        );
        assert_eq!(errors("recent-windows { previews { max-height 0; } }"), 1);
    }

    /// Pinned niri: shadow `softness <0, 1024>`, `spread <-1024, 1024>`
    /// (niri-config/src/appearance.rs:648-660); tab-indicator `gap <-65535, 65535>` and
    /// width/gaps-between-tabs/corner-radius `<0, 65535>` (niri-config/src/appearance.rs:565-590);
    /// gradient `angle: i16` (niri-config/src/appearance.rs:99-102).
    #[test]
    fn parity_bounds_shadow_tab_indicator_and_gradient_angle() {
        assert_eq!(
            errors("layout { shadow { softness 1024; spread -1024; } }"),
            0
        );
        assert_eq!(errors("layout { shadow { softness 1025; } }"), 1);
        assert_eq!(errors("layout { shadow { spread -1025; } }"), 1);
        assert_eq!(
            errors("layout { tab-indicator { width 65535; gap -65535; gaps-between-tabs 0; corner-radius 65535; } }"),
            0
        );
        assert_eq!(errors("layout { tab-indicator { width 65536; } }"), 1);
        assert_eq!(
            errors("layout { border { active-gradient from=\"#000\" to=\"#fff\" angle=40000; } }"),
            1
        );
        assert_eq!(
            errors("layout { border { active-gradient from=\"#000\" to=\"#fff\" angle=45.5; } }"),
            1
        );
    }

    /// niri checks `accel-speed < -1, 1>`, `pinch-sensitivity <0, 100>` and `scroll-button: u32`
    /// (niri-config/src/input.rs:143-231).  The first two have no Windows equivalent, so the
    /// decoder only warns; their values must still be niri's.
    #[test]
    fn parity_unsupported_device_options_are_still_type_and_range_checked() {
        assert_eq!(errors("input { mouse { accel-speed 1; } }"), 0);
        assert_eq!(errors("input { mouse { accel-speed 1.5; } }"), 1);
        assert_eq!(errors("input { mouse { accel-speed; } }"), 1);
        assert_eq!(errors("input { mouse { accel-speed 1 2; } }"), 1);
        assert_eq!(errors("input { touchpad { pinch-sensitivity 100; } }"), 0);
        assert_eq!(errors("input { touchpad { pinch-sensitivity 101; } }"), 1);
        assert_eq!(errors("input { mouse { scroll-button 272; } }"), 0);
        assert_eq!(errors("input { mouse { scroll-button 1.5; } }"), 1);
        assert_eq!(errors("cursor { xcursor-size 24; }"), 0);
        assert_eq!(errors("cursor { xcursor-size 300; }"), 1);
    }
}
