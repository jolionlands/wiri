//! The four non-appearance window-policy keys. This module does not depend on the
//! parent adding fields to `WindowRule`: pass its sparse `RulePolicy` explicitly.
//!
//! niri `e19df12cf8`, `niri-config/src/window_rule.rs:158-191`, requires numeric x/y,
//! defaults relative-to to top-left, and accepts eight anchors. Offsets are signed
//! `-65535..=65535`; scroll factor is a finite number in `0..=100` (lines 77-78).
//! See the pinned window-rule documentation at the same revision, sections
//! `default-column-display`, `default-floating-position`, `on-xdg-activate`, and
//! `scroll-factor`. Unknown values, properties, duplicates and wrong node shapes
//! are errors, not unsupported-feature warnings. A failed node never changes policy.
//!
//! Parent hook: declare `mod rule_policy;`, then call
//! `self.rule_policy_key(child, &mut rule.policy)` in `decode_window_rule` before
//! its unsupported/unknown fallback; remove these four keys from that fallback.

use super::{norm, Decoder};
use crate::config::kdl::{Node, Value};
use crate::window::rule_policy::{
    DefaultColumnDisplay, FloatingPosition, OnXdgActivate, RelativeTo, RulePolicy,
    FLOATING_POSITION_LIMIT, MAX_SCROLL_FACTOR,
};

impl Decoder {
    /// True means this is one of our keys, even if it produced validation errors.
    /// False leaves both diagnostics and the destination untouched.
    pub(super) fn rule_policy_key(&mut self, node: &Node, policy: &mut RulePolicy) -> bool {
        let issue_count = self.issues.len();
        let mut updated = *policy;
        match norm(&node.name).as_str() {
            "default-column-display" => {
                if let Some(text) = self.arg_text(node) {
                    match DefaultColumnDisplay::parse(&text) {
                        Some(display) => updated.default_column_display = Some(display),
                        None => self.error(
                            node.args[0].pos,
                            "'default-column-display' expects \"normal\" or \"tabbed\"",
                        ),
                    }
                }
            }
            "default-floating-position" => {
                updated.default_floating_position = self.rule_floating_position(node);
            }
            "on-xdg-activate" => {
                if let Some(text) = self.arg_text(node) {
                    match OnXdgActivate::parse(&text) {
                        Some(action) => updated.on_xdg_activate = Some(action),
                        None => self.error(
                            node.args[0].pos,
                            "'on-xdg-activate' expects \"ignore\", \"set-urgent\", or \"focus\"",
                        ),
                    }
                }
            }
            "scroll-factor" => {
                if let Some(value) = self.one_arg(node) {
                    updated.scroll_factor = self.policy_number(node, value, 0.0, MAX_SCROLL_FACTOR);
                }
            }
            _ => return false,
        }
        if self.issues.len() == issue_count {
            *policy = updated;
        }
        true
    }

    /// Property-only syntax, not a positional pair or an x/y child block.
    fn rule_floating_position(&mut self, node: &Node) -> Option<FloatingPosition> {
        let issue_count = self.issues.len();
        self.no_args(node);
        self.no_children(node);
        let mut x = None;
        let mut y = None;
        let mut relative_to = RelativeTo::default();
        let mut seen = [false; 3];
        for property in &node.props {
            let key = norm(&property.key);
            let index = match key.as_str() {
                "x" => 0,
                "y" => 1,
                "relative-to" => 2,
                _ => {
                    self.error(
                        property.key_pos,
                        format!(
                            "unknown property '{}' in 'default-floating-position'; expected 'x', 'y', or 'relative-to'",
                            property.key,
                        ),
                    );
                    continue;
                }
            };
            if seen[index] {
                self.error(
                    property.key_pos,
                    format!(
                        "duplicate '{}' property in 'default-floating-position'",
                        property.key
                    ),
                );
                continue;
            }
            seen[index] = true;
            match index {
                0 => {
                    x = self.policy_number(
                        node,
                        &property.value,
                        -FLOATING_POSITION_LIMIT,
                        FLOATING_POSITION_LIMIT,
                    )
                }
                1 => {
                    y = self.policy_number(
                        node,
                        &property.value,
                        -FLOATING_POSITION_LIMIT,
                        FLOATING_POSITION_LIMIT,
                    )
                }
                _ => {
                    if let Some(text) = self.value_text(node, &property.value) {
                        match RelativeTo::parse(&text) {
                            Some(anchor) => relative_to = anchor,
                            None => self.error(
                                property.value.pos,
                                "'relative-to' expects top-left, top-right, bottom-left, bottom-right, top, bottom, left, or right",
                            ),
                        }
                    }
                }
            }
        }
        for (index, name) in ["x", "y"].iter().enumerate() {
            if !seen[index] {
                self.error(
                    node.pos,
                    format!("'default-floating-position' needs the '{}' property", name),
                );
            }
        }
        if self.issues.len() != issue_count {
            return None;
        }
        Some(FloatingPosition {
            x: x?,
            y: y?,
            relative_to,
        })
    }

    /// Unlike the legacy number helper, niri's FloatOrInt does not accept quoted
    /// numbers, booleans or null. Keep fractions and reject non-finite values.
    fn policy_number(&mut self, node: &Node, value: &Value, min: f64, max: f64) -> Option<f64> {
        let Some(number) = value.as_f64() else {
            self.error(
                value.pos,
                format!("'{}' expects a number, found a {}", node.name, value.kind()),
            );
            return None;
        };
        if !number.is_finite() || number < min || number > max {
            self.error(
                value.pos,
                format!("'{}' must be between {} and {}", node.name, min, max),
            );
            return None;
        }
        Some(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::decode::{ValidationIssue, ValidationSeverity};
    use crate::config::kdl::{self, Scalar};

    fn decoder() -> Decoder {
        Decoder {
            issues: Vec::new(),
            includes: Vec::new(),
            file: "rule-policy-test.kdl".to_string(),
            base_dir: None,
            stack: Vec::new(),
            depth: 0,
        }
    }

    fn apply(text: &str, policy: &mut RulePolicy) -> Vec<ValidationIssue> {
        let document = kdl::parse(text).expect("test fixture must be syntactically valid KDL");
        let mut decoder = decoder();
        for node in &document.nodes {
            assert!(
                decoder.rule_policy_key(node, policy),
                "unhandled {}",
                node.name
            );
        }
        decoder.issues
    }

    fn nondefault_policy() -> RulePolicy {
        RulePolicy {
            default_column_display: Some(DefaultColumnDisplay::Tabbed),
            default_floating_position: Some(FloatingPosition {
                x: 10.0,
                y: 20.0,
                relative_to: RelativeTo::BottomRight,
            }),
            on_xdg_activate: Some(OnXdgActivate::Focus),
            scroll_factor: Some(2.0),
        }
    }

    #[test]
    fn canonical_nodes_decode_without_warnings() {
        let mut policy = RulePolicy::default();
        let issues = apply(
            r#"
            default-column-display "tabbed"
            default-floating-position x=-32.5 y=48 relative-to="bottom-left"
            on-xdg-activate "set-urgent"
            scroll-factor 0.75
        "#,
            &mut policy,
        );
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(
            policy.default_column_display,
            Some(DefaultColumnDisplay::Tabbed)
        );
        assert_eq!(
            policy.default_floating_position,
            Some(FloatingPosition {
                x: -32.5,
                y: 48.0,
                relative_to: RelativeTo::BottomLeft,
            })
        );
        assert_eq!(policy.on_xdg_activate, Some(OnXdgActivate::SetUrgent));
        assert_eq!(policy.scroll_factor, Some(0.75));
    }

    #[test]
    fn x_and_y_are_required_but_relative_to_defaults_to_top_left() {
        let mut policy = RulePolicy::default();
        assert!(apply("default-floating-position x=-65535 y=65535", &mut policy).is_empty());
        assert_eq!(
            policy.default_floating_position,
            Some(FloatingPosition {
                x: -65_535.0,
                y: 65_535.0,
                relative_to: RelativeTo::TopLeft,
            })
        );
        assert!(apply("default-floating-position x=0 y=-0.25", &mut policy).is_empty());
        assert_eq!(policy.default_floating_position.unwrap().y, -0.25);
    }

    #[test]
    fn each_anchor_and_activation_value_decodes() {
        let mut policy = RulePolicy::default();
        for anchor in [
            "top-left",
            "top-right",
            "bottom-left",
            "bottom-right",
            "top",
            "bottom",
            "left",
            "right",
        ] {
            let text = format!("default-floating-position x=0 y=0 relative-to=\"{anchor}\"");
            assert!(apply(&text, &mut policy).is_empty());
            assert_eq!(
                policy.default_floating_position.unwrap().relative_to,
                RelativeTo::parse(anchor).unwrap()
            );
        }
        for action in ["ignore", "set-urgent", "focus"] {
            assert!(apply(&format!("on-xdg-activate \"{action}\""), &mut policy).is_empty());
            assert_eq!(policy.on_xdg_activate, OnXdgActivate::parse(action));
        }
    }

    #[test]
    fn inclusive_scroll_range_and_explicit_normal_zero_ignore_are_preserved() {
        let mut policy = nondefault_policy();
        assert!(apply("scroll-factor 100", &mut policy).is_empty());
        assert_eq!(policy.scroll_factor, Some(100.0));
        assert!(apply(
            r#"
            default-column-display "normal"
            on-xdg-activate "ignore"
            scroll-factor 0
        "#,
            &mut policy
        )
        .is_empty());
        assert_eq!(
            policy.default_column_display,
            Some(DefaultColumnDisplay::Normal)
        );
        assert_eq!(policy.on_xdg_activate, Some(OnXdgActivate::Ignore));
        assert_eq!(policy.scroll_factor, Some(0.0));
    }

    #[test]
    fn bad_values_shapes_and_unknown_properties_are_errors_and_atomic() {
        let bad_nodes = [
            "default-column-display",
            "default-column-display true",
            "default-column-display \"stacked\"",
            "default-column-display \"Tabbed\"",
            "default-column-display \"normal\" \"tabbed\"",
            "default-column-display \"normal\" extra=1",
            "default-column-display \"normal\" { bogus; }",
            "on-xdg-activate",
            "on-xdg-activate false",
            "on-xdg-activate \"urgent\"",
            "on-xdg-activate \"ignore\" extra=1",
            "scroll-factor",
            "scroll-factor -0.01",
            "scroll-factor 100.01",
            "scroll-factor \"0.75\"",
            "scroll-factor true",
            "scroll-factor null",
            "scroll-factor 1 2",
            "scroll-factor 1 extra=1",
            "scroll-factor 1 { bogus; }",
            "default-floating-position",
            "default-floating-position x=1",
            "default-floating-position y=1",
            "default-floating-position 1 2",
            "default-floating-position x=1 y=2 { x 3; }",
            "default-floating-position x=65535.01 y=0",
            "default-floating-position x=0 y=-65535.01",
            "default-floating-position x=\"1\" y=2",
            "default-floating-position x=true y=2",
            "default-floating-position x=0 y=null",
            "default-floating-position x=1 y=2 relative-to=\"center\"",
            "default-floating-position x=1 y=2 relative-to=false",
            "default-floating-position x=1 y=2 z=3",
        ];
        for text in bad_nodes {
            let mut policy = nondefault_policy();
            let before = policy;
            let issues = apply(text, &mut policy);
            assert!(!issues.is_empty(), "no diagnostic for {text}");
            assert!(
                issues
                    .iter()
                    .all(|issue| issue.severity == ValidationSeverity::Error),
                "{text}: {issues:?}"
            );
            assert_eq!(policy, before, "invalid node changed policy: {text}");
        }
    }

    #[test]
    fn duplicate_properties_are_rejected_even_if_values_are_identical() {
        for index in 0..3 {
            let mut node = kdl::parse("default-floating-position x=1 y=2 relative-to=\"top\"")
                .unwrap()
                .nodes
                .remove(0);
            node.props.push(node.props[index].clone());
            let mut decoder = decoder();
            let mut policy = nondefault_policy();
            let before = policy;
            assert!(decoder.rule_policy_key(&node, &mut policy));
            assert!(decoder
                .issues
                .iter()
                .any(|issue| issue.message.contains("duplicate")));
            assert_eq!(policy, before);
        }
    }

    #[test]
    fn nonfinite_numeric_values_are_rejected_without_losing_prior_values() {
        for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for floating in [false, true] {
                let text = if floating {
                    "default-floating-position x=1 y=2"
                } else {
                    "scroll-factor 1"
                };
                let mut node = kdl::parse(text).unwrap().nodes.remove(0);
                if floating {
                    node.props[0].value.scalar = Scalar::Float(number);
                } else {
                    node.args[0].scalar = Scalar::Float(number);
                }
                let mut decoder = decoder();
                let mut policy = nondefault_policy();
                let before = policy;
                assert!(decoder.rule_policy_key(&node, &mut policy));
                assert!(!decoder.issues.is_empty());
                assert_eq!(policy, before);
            }
        }
    }

    #[test]
    fn diagnostics_keep_source_file_and_value_position() {
        let issues = apply(
            "// comment\n\nscroll-factor 101",
            &mut RulePolicy::default(),
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].file, "rule-policy-test.kdl");
        assert_eq!((issues[0].line, issues[0].column), (3, 15));
        assert_eq!(issues[0].severity, ValidationSeverity::Error);
    }

    #[test]
    fn unrelated_key_is_not_consumed_or_reported() {
        let node = kdl::parse("opacity 0.5").unwrap().nodes.remove(0);
        let mut decoder = decoder();
        let mut policy = nondefault_policy();
        let before = policy;
        assert!(!decoder.rule_policy_key(&node, &mut policy));
        assert!(decoder.issues.is_empty());
        assert_eq!(policy, before);
    }

    #[test]
    fn later_valid_nodes_replace_only_the_fields_they_set() {
        let mut policy = nondefault_policy();
        let issues = apply(
            r#"
            default-column-display "normal"
            scroll-factor 0
            scroll-factor 0.5
            default-floating-position x=0 y=0
        "#,
            &mut policy,
        );
        assert!(issues.is_empty());
        assert_eq!(
            policy.default_column_display,
            Some(DefaultColumnDisplay::Normal)
        );
        assert_eq!(policy.scroll_factor, Some(0.5));
        assert_eq!(policy.on_xdg_activate, Some(OnXdgActivate::Focus));
        assert_eq!(
            policy.default_floating_position,
            Some(FloatingPosition {
                x: 0.0,
                y: 0.0,
                relative_to: RelativeTo::TopLeft,
            })
        );
    }
}
