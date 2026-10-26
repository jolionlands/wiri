//! Niri-style spring/easing blocks; kept separate from the shared layout decoder.
use super::{norm, Decoder};
use crate::config::kdl::Node;
use crate::config::types::AnimationsConfig;
use crate::layout::animation::{AnimationKind, Easing, Spring};

impl Decoder {
    pub(super) fn decode_animation_options(&mut self, node: &Node, cfg: &mut AnimationsConfig) {
        self.section(node);
        cfg.enabled = true;
        let mut seen = std::collections::HashSet::new();
        let mut blocks = Vec::new();
        for child in node.kids() {
            let name = norm(&child.name);
            // Definition aliases share a key; legacy flags still parse independently.
            let definition = match name.as_str() {
                "duration" | "duration-ms" => Some("duration-ms"),
                "easing" | "curve" => Some("curve"),
                "spring"
                | "slowdown"
                | "workspace-switch"
                | "horizontal-view-movement"
                | "overview-open-close"
                | "window-movement" => Some(name.as_str()),
                "window-resize" if child.children.is_some() => Some("window-resize"),
                _ => None,
            };
            if let Some(key) = definition {
                if !seen.insert(key.to_owned()) {
                    self.error(
                        child.pos,
                        format!("duplicate animation option '{}'", child.name),
                    );
                }
            }
            match name.as_str() {
                "enabled" | "enable" | "on" => {
                    if let Some(v) = self.flag(child) {
                        cfg.enabled = v;
                        cfg.overview_enabled = v;
                    }
                }
                "off" => {
                    if let Some(v) = self.flag(child) {
                        cfg.enabled = !v;
                        cfg.overview_enabled = !v;
                    }
                }
                "slowdown" => {
                    if let Some(v) = self.arg_number(child, 0.0, 1000.0) {
                        cfg.slowdown = v;
                    }
                }
                "duration" | "duration-ms" => {
                    if let Some(v) = self.arg_u32(child, 60000) {
                        cfg.duration = v;
                        cfg.legacy_easing = true;
                    }
                }
                "easing" | "curve" => {
                    if let Some(v) = self.animation_curve(child) {
                        cfg.easing = v;
                        cfg.legacy_easing = true;
                    }
                }
                "spring" => {
                    cfg.default_kind = self.animation_spring(child).map(AnimationKind::Spring);
                }
                "workspace-switch"
                | "horizontal-view-movement"
                | "window-movement"
                | "overview-open-close" => {
                    blocks.push(child);
                }
                "window-resize" if child.children.is_some() => {
                    blocks.push(child);
                }
                "workspace-transition" => {
                    if let Some(v) = self.flag(child) {
                        cfg.workspace_transition = v;
                    }
                }
                "focus-transition" => {
                    if let Some(v) = self.flag(child) {
                        cfg.focus_transition = v;
                    }
                }
                "window-move" => {
                    if let Some(v) = self.flag(child) {
                        cfg.window_move = v;
                    }
                }
                "window-resize" => {
                    if let Some(v) = self.flag(child) {
                        cfg.window_resize = v;
                    }
                }
                "popup" => {
                    if let Some(v) = self.flag(child) {
                        cfg.popup = v;
                    }
                }
                "shadow" => {
                    if let Some(v) = self.flag(child) {
                        cfg.shadow = v;
                    }
                }
                "window-open"
                | "window-close"
                | "config-notification-open-close"
                | "exit-confirmation-open-close"
                | "screenshot-ui-open"
                | "recent-windows-close" => {
                    self.unsupported(child, "composited open/close animations (only geometry transitions are implemented)");
                }
                _ => self.unknown(
                    child,
                    "'animations'",
                    &[
                        "off",
                        "slowdown",
                        "workspace-switch",
                        "horizontal-view-movement",
                        "window-movement",
                        "window-resize",
                        "duration",
                        "easing",
                    ],
                ),
            }
        }
        if seen.contains("spring") && (seen.contains("duration-ms") || seen.contains("curve")) {
            self.error(
                node.pos,
                "animations cannot combine spring with duration-ms/curve",
            );
        }
        // Curve-only overrides inherit the final global duration, not a partial one.
        for child in blocks {
            let kind = Some(self.animation_spec(child, cfg.duration));
            match norm(&child.name).as_str() {
                "workspace-switch" => cfg.workspace_switch = kind,
                "horizontal-view-movement" => cfg.horizontal_view_movement = kind,
                "window-movement" => cfg.window_movement = kind,
                "window-resize" => cfg.resize = kind,
                "overview-open-close" => cfg.overview_open_close = kind,
                _ => unreachable!(),
            }
        }
    }

    fn animation_curve(&mut self, node: &Node) -> Option<String> {
        let name = self.arg_text(node)?;
        if Easing::parse(&name).is_none() {
            self.error(node.pos, format!("unknown animation curve '{name}'; expected linear, ease-out-quad, ease-out-cubic, or ease-out-expo"));
            None
        } else {
            Some(name)
        }
    }

    fn animation_spring(&mut self, node: &Node) -> Option<Spring> {
        self.no_args(node);
        self.no_children(node);
        let mut spring = Spring::default();
        let mut valid = true;
        let mut seen = std::collections::HashSet::new();
        for prop in &node.props {
            let key = norm(&prop.key);
            if !seen.insert(key.clone()) {
                self.error(
                    prop.key_pos,
                    format!("duplicate spring property '{}'", prop.key),
                );
                valid = false;
            }
            let range = match key.as_str() {
                "damping-ratio" => (0.0, 100.0),
                "stiffness" => (0.000001, 1_000_000_000.0),
                "epsilon" => (0.000000000001, 1.0),
                _ => {
                    self.error(
                        prop.key_pos,
                        format!("unknown spring property '{}'", prop.key),
                    );
                    valid = false;
                    continue;
                }
            };
            match self.value_number(node, &prop.value, range.0, range.1) {
                Some(v) => match key.as_str() {
                    "damping-ratio" => spring.damping_ratio = v,
                    "stiffness" => spring.stiffness = v,
                    "epsilon" => spring.epsilon = v,
                    _ => unreachable!(),
                },
                None => valid = false,
            }
        }
        valid.then_some(spring)
    }

    fn animation_spec(&mut self, node: &Node, default_duration: u32) -> AnimationKind {
        self.section(node);
        let mut seen = std::collections::HashSet::new();
        let mut spring = None;
        let mut duration = default_duration;
        let mut curve = Easing::CubicOut;
        let mut easing = false;
        let mut off = false;
        for child in node.kids() {
            let name = norm(&child.name);
            let key = match name.as_str() {
                "duration" => "duration-ms",
                "easing" => "curve",
                _ => name.as_str(),
            };
            if !seen.insert(key.to_owned()) {
                self.error(
                    child.pos,
                    format!("duplicate animation option '{}'", child.name),
                );
            }
            match name.as_str() {
                "off" => {
                    off = self.flag(child).unwrap_or(false);
                }
                "spring" => {
                    spring = self.animation_spring(child);
                }
                "duration" | "duration-ms" => {
                    if let Some(v) = self.arg_u32(child, 60000) {
                        duration = v;
                    }
                    easing = true;
                }
                "curve" | "easing" => {
                    if let Some(v) = self.animation_curve(child) {
                        curve = Easing::parse(&v).unwrap();
                    }
                    easing = true;
                }
                _ => self.unknown(
                    child,
                    "animation",
                    &["off", "spring", "duration-ms", "curve"],
                ),
            }
        }
        if seen.contains("spring") && easing {
            self.error(
                node.pos,
                "an animation cannot combine spring with duration-ms/curve",
            );
        }
        if off {
            AnimationKind::Off
        } else if let Some(spring) = spring {
            AnimationKind::Spring(spring)
        } else if easing {
            AnimationKind::Easing {
                duration_ms: duration,
                curve,
            }
        } else {
            AnimationKind::Spring(Spring::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::decode::{decode_text, ValidationSeverity};

    #[test]
    fn niri_animation_blocks_are_typed_not_ignored() {
        let d = decode_text(
            r#"animations {
            slowdown 3.0
            workspace-switch { spring damping-ratio=0.7 stiffness=1000 epsilon=0.0001; }
            horizontal-view-movement { duration-ms 240; curve "ease-out-expo"; }
            window-movement { off; }
            window-resize { duration-ms 0; curve "linear"; }
        }"#,
            "",
            None,
        );
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        let cfg = d.config.animations;
        assert!(cfg.enabled);
        assert_eq!(cfg.slowdown, 3.0);
        assert!(matches!(
            cfg.workspace_kind(),
            AnimationKind::Spring(Spring {
                damping_ratio: 0.7,
                ..
            })
        ));
        assert!(matches!(
            cfg.view_kind(),
            AnimationKind::Easing {
                duration_ms: 240,
                ..
            }
        ));
        assert!(matches!(cfg.movement_kind(), AnimationKind::Off));
        assert!(matches!(
            cfg.resize_kind(),
            AnimationKind::Easing { duration_ms: 0, .. }
        ));
    }

    #[test]
    fn legacy_flags_and_global_off_still_work() {
        let d = decode_text(
            "animations { enabled true; duration 400; easing linear; window-resize false; off; }",
            "",
            None,
        );
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        assert!(!d.config.animations.enabled);
        assert!(!d.config.animations.window_resize);
        assert!(matches!(
            d.config.animations.view_kind(),
            AnimationKind::Off
        ));
        let d = decode_text("animations { on; duration 400; easing linear; }", "", None);
        assert!(matches!(
            d.config.animations.view_kind(),
            AnimationKind::Easing {
                duration_ms: 400,
                curve: Easing::Linear
            }
        ));
    }

    #[test]
    fn curve_only_blocks_inherit_global_duration_in_either_order() {
        let blocks = r#"
            workspace-switch { curve "linear"; }
            horizontal-view-movement { easing "linear"; }
            window-movement { curve "linear"; }
            window-resize { curve "linear"; }
        "#;
        for duration in ["duration 450;", "duration-ms 450;"] {
            for body in [
                format!("{duration}\n{blocks}"),
                format!("{blocks}\n{duration}"),
            ] {
                let d = decode_text(&format!("animations {{\n{body}\n}}"), "", None);
                assert!(d.issues.is_empty(), "{:?}", d.issues);
                let cfg = d.config.animations;
                for kind in [
                    cfg.workspace_kind(),
                    cfg.view_kind(),
                    cfg.movement_kind(),
                    cfg.resize_kind(),
                ] {
                    assert!(matches!(
                        kind,
                        AnimationKind::Easing {
                            duration_ms: 450,
                            curve: Easing::Linear
                        }
                    ));
                }
            }
        }
    }

    #[test]
    fn resize_flag_and_block_keep_separate_meanings() {
        let block = "window-resize { curve linear; };";
        for body in [
            format!("window-resize false; {block} duration-ms 450;"),
            format!("{block} duration-ms 450; window-resize false;"),
        ] {
            let d = decode_text(&format!("animations {{ {body} }}"), "", None);
            assert!(d.issues.is_empty(), "{:?}", d.issues);
            let cfg = d.config.animations;
            assert!(!cfg.window_resize);
            assert!(matches!(
                cfg.resize,
                Some(AnimationKind::Easing {
                    duration_ms: 450,
                    curve: Easing::Linear
                })
            ));
            assert!(matches!(cfg.resize_kind(), AnimationKind::Off));
        }
    }

    #[test]
    fn global_spring_allows_per_animation_easing_overrides() {
        let spring = "spring damping-ratio=0.7 stiffness=900;";
        let block = "workspace-switch { duration-ms 240; curve linear; };";
        for body in [format!("{spring} {block}"), format!("{block} {spring}")] {
            let d = decode_text(&format!("animations {{ {body} }}"), "", None);
            assert!(d.issues.is_empty(), "{:?}", d.issues);
            let cfg = d.config.animations;
            assert!(matches!(
                cfg.view_kind(),
                AnimationKind::Spring(Spring {
                    damping_ratio: 0.7,
                    stiffness: 900.0,
                    ..
                })
            ));
            assert!(matches!(
                cfg.workspace_kind(),
                AnimationKind::Easing {
                    duration_ms: 240,
                    curve: Easing::Linear
                }
            ));
        }
    }

    #[test]
    fn duplicate_animation_definitions_are_errors() {
        for body in [
            "duration 100; duration-ms 200;",
            "curve linear; easing ease-out-expo;",
            "spring; SPRING stiffness=900;",
            "slowdown 1; slowdown 2;",
            "workspace-switch { off; }; workspace-switch { off; };",
            "horizontal-view-movement { off; }; horizontal-view-movement { off; };",
            "window-movement { off; }; window-movement { off; };",
            "window-resize { off; }; window-resize { off; };",
            "workspace-switch { duration 100; duration_ms 200; };",
            "horizontal-view-movement { curve linear; easing ease-out-expo; };",
            "window-movement { spring; spring stiffness=900; };",
            "window-resize { off; off false; };",
            "spring damping-ratio=0.7 damping_ratio=0.8;",
        ] {
            let d = decode_text(&format!("animations {{ {body} }}"), "", None);
            assert!(
                d.issues
                    .iter()
                    .any(|i| i.severity == ValidationSeverity::Error
                        && i.message.contains("duplicate")),
                "accepted {body}: {:?}",
                d.issues
            );
        }
    }

    #[test]
    fn spring_and_easing_definitions_cannot_be_combined() {
        for settings in [
            "spring; duration 100;",
            "duration-ms 100; spring;",
            "spring; easing linear;",
            "curve linear; spring;",
        ] {
            for body in [
                settings.to_owned(),
                format!("workspace-switch {{ {settings} }}"),
            ] {
                let d = decode_text(&format!("animations {{ {body} }}"), "", None);
                assert!(
                    d.issues
                        .iter()
                        .any(|i| i.severity == ValidationSeverity::Error
                            && i.message.contains("cannot combine spring")),
                    "accepted {body}: {:?}",
                    d.issues
                );
            }
        }
    }

    #[test]
    fn invalid_animation_settings_are_errors() {
        for body in [
            "slowdown -1",
            "workspace-switch { spring stiffness=0; }",
            "window-resize { curve \"typo\"; }",
            "window-movement { duration-ms -2; }",
            "workspace-switch { spring epsilon=0; }",
            "workspace-switch { spring mystery=1; }",
            "workspace-switch { spring; duration-ms 20; }",
        ] {
            let d = decode_text(&format!("animations {{ {body}; }}"), "", None);
            assert!(
                d.issues
                    .iter()
                    .any(|i| i.severity == ValidationSeverity::Error),
                "accepted {body}"
            );
        }
    }
}
