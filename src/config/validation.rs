//! Structural checks kept separate from decoding. Includes are independent
//! parts (niri ConfigPart); duplicates within each physical file are errors.
use super::decode::{ValidationIssue, ValidationSeverity};
use super::diagnostics::{DiagnosticContext, SourceLabel};
use super::kdl::{Node, Scalar, Value};
use std::collections::HashMap;

fn norm(s: &str) -> String {
    s.replace('_', "-").to_ascii_lowercase()
}
fn issue(file: &str, source: &str, value: &Value, message: String) -> ValidationIssue {
    ValidationIssue {
        severity: ValidationSeverity::Error,
        file: file.into(),
        line: value.pos.line as usize,
        column: value.pos.col as usize,
        message,
        context: DiagnosticContext {
            source_line: source
                .lines()
                .nth(value.pos.line.saturating_sub(1) as usize)
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
        let bounds = match (parent, key.as_str()) {
            ("mouse" | "touchpad" | "trackpoint" | "trackball", "accel-speed") => Some((-1.0, 1.0)),
            ("mouse" | "touchpad" | "trackpoint" | "trackball", "scroll-factor") => {
                Some((0.0, 100.0))
            }
            ("touchpad", "pinch-sensitivity") => Some((0.0, 100.0)),
            // Even unsupported device options must not accept nonsensical values.
            _ => None,
        };
        if let Some((min, max)) = bounds {
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
}
