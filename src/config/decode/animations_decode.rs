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
                | "window-movement"
                | "window-open"
                | "window-close" => Some(name.as_str()),
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
                    }
                }
                "off" => {
                    if let Some(v) = self.flag(child) {
                        cfg.enabled = !v;
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
                | "window-open"
                | "window-close" => {
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
                "config-notification-open-close"
                | "exit-confirmation-open-close"
                | "screenshot-ui-open"
                | "overview-open-close"
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
            let default = match norm(&child.name).as_str() {
                "window-open" => AnimationsConfig {
                    enabled: true,
                    ..cfg.clone()
                }
                .open_kind(),
                "window-close" => AnimationsConfig {
                    enabled: true,
                    ..cfg.clone()
                }
                .close_kind(),
                _ => AnimationKind::Spring(Spring::default()),
            };
            let kind = Some(self.animation_spec(child, cfg.duration, default));
            match norm(&child.name).as_str() {
                "workspace-switch" => cfg.workspace_switch = kind,
                "horizontal-view-movement" => cfg.horizontal_view_movement = kind,
                "window-movement" => cfg.window_movement = kind,
                "window-resize" => cfg.resize = kind,
                "window-open" => cfg.window_open = kind,
                "window-close" => cfg.window_close = kind,
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

    fn animation_spec(
        &mut self,
        node: &Node,
        default_duration: u32,
        default: AnimationKind,
    ) -> AnimationKind {
        self.section(node);
        let mut seen = std::collections::HashSet::new();
        let mut spring = None;
        let (mut duration, mut curve) = match default {
            AnimationKind::Easing { duration_ms, curve } => (duration_ms, curve),
            _ => (default_duration, Easing::CubicOut),
        };
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
            default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::decode::{decode_text, ValidationSeverity};

    fn easing(duration_ms: u32, curve: Easing) -> AnimationKind {
        AnimationKind::Easing { duration_ms, curve }
    }

    fn decode_animations(body: &str) -> AnimationsConfig {
        let d = decode_text(&format!("animations {{ {body} }}"), "", None);
        assert!(d.issues.is_empty(), "{body}: {:?}", d.issues);
        d.config.animations
    }

    #[test]
    fn open_close_defaults_match_niri_only_when_animations_are_enabled() {
        let d = decode_text("", "", None);
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        assert!(!d.config.animations.enabled);
        assert_eq!(d.config.animations.open_kind(), AnimationKind::Off);
        assert_eq!(d.config.animations.close_kind(), AnimationKind::Off);

        // Defaults pinned to niri e19df12cf8, Configuration: Animations.
        for body in ["", "window-open {}; window-close {};"] {
            let cfg = decode_animations(body);
            assert!(cfg.enabled);
            assert_eq!(cfg.open_kind(), easing(150, Easing::ExpoOut));
            assert_eq!(cfg.close_kind(), easing(150, Easing::QuadOut));
            let explicit = !body.is_empty();
            assert_eq!(cfg.window_open.is_some(), explicit);
            assert_eq!(cfg.window_close.is_some(), explicit);
        }
    }

    #[test]
    fn open_close_specs_are_typed_independent_overrides() {
        for (effect, curve, other_curve) in [
            ("window-open", Easing::ExpoOut, Easing::QuadOut),
            ("window-close", Easing::QuadOut, Easing::ExpoOut),
        ] {
            for (settings, expected) in [
                ("off;", AnimationKind::Off),
                ("off false;", easing(150, curve)),
                ("duration-ms 240;", easing(240, curve)),
                ("curve linear;", easing(150, Easing::Linear)),
                (
                    "duration 320; easing ease-out-quad;",
                    easing(320, Easing::QuadOut),
                ),
                (
                    "spring damping-ratio=0.7 stiffness=900 epsilon=0.001;",
                    AnimationKind::Spring(Spring {
                        damping_ratio: 0.7,
                        stiffness: 900.0,
                        epsilon: 0.001,
                    }),
                ),
            ] {
                let cfg = decode_animations(&format!("{effect} {{ {settings} }};"));
                let (specific, kind, other, other_kind) = if effect == "window-open" {
                    (
                        cfg.window_open,
                        cfg.open_kind(),
                        cfg.window_close,
                        cfg.close_kind(),
                    )
                } else {
                    (
                        cfg.window_close,
                        cfg.close_kind(),
                        cfg.window_open,
                        cfg.open_kind(),
                    )
                };
                assert_eq!(specific, Some(expected), "{effect}: {settings}");
                assert_eq!(kind, expected, "{effect}: {settings}");
                assert_eq!(other, None);
                assert_eq!(other_kind, easing(150, other_curve));
            }
        }
    }

    #[test]
    fn open_close_inherit_global_defaults_in_either_order() {
        for (global, expected) in [
            ("duration 450; easing linear;", easing(450, Easing::Linear)),
            (
                "duration-ms 450; curve linear;",
                easing(450, Easing::Linear),
            ),
            (
                "spring damping-ratio=0.7 stiffness=900 epsilon=0.001;",
                AnimationKind::Spring(Spring {
                    damping_ratio: 0.7,
                    stiffness: 900.0,
                    epsilon: 0.001,
                }),
            ),
        ] {
            for blocks in ["", "window-open {}; window-close {};"] {
                for body in [format!("{global} {blocks}"), format!("{blocks} {global}")] {
                    let cfg = decode_animations(&body);
                    assert_eq!(cfg.open_kind(), expected, "{body}");
                    assert_eq!(cfg.close_kind(), expected, "{body}");
                }
            }
        }
    }

    #[test]
    fn open_close_global_off_preserves_specs_in_either_order() {
        let blocks = "window-open { duration-ms 240; curve linear; }; window-close {};";
        for off in ["off;", "enabled false;"] {
            for body in [format!("{off} {blocks}"), format!("{blocks} {off}")] {
                let cfg = decode_animations(&body);
                assert!(!cfg.enabled);
                assert_eq!(cfg.open_kind(), AnimationKind::Off);
                assert_eq!(cfg.close_kind(), AnimationKind::Off);
                assert_eq!(cfg.window_open, Some(easing(240, Easing::Linear)));
                assert_eq!(cfg.window_close, Some(easing(150, Easing::QuadOut)));
            }
        }
    }

    #[test]
    fn open_close_slowdown_is_stored_without_rewriting_kinds() {
        for slowdown in [0.0, 0.25, 3.0] {
            let cfg = decode_animations(&format!("slowdown {slowdown};"));
            assert_eq!(cfg.slowdown, slowdown);
            assert_eq!(cfg.open_kind(), easing(150, Easing::ExpoOut));
            assert_eq!(cfg.close_kind(), easing(150, Easing::QuadOut));

            let cfg = decode_animations(&format!(
                "slowdown {slowdown}; window-open {{ off; }}; \
                 window-close {{ duration-ms 240; curve linear; }};"
            ));
            assert_eq!(cfg.slowdown, slowdown);
            assert_eq!(cfg.open_kind(), AnimationKind::Off);
            assert_eq!(cfg.close_kind(), easing(240, Easing::Linear));
        }
    }

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
            "window-open { off; }; WINDOW_OPEN { off false; };",
            "window-close { off; }; WINDOW_CLOSE { off false; };",
            "workspace-switch { duration 100; duration_ms 200; };",
            "horizontal-view-movement { curve linear; easing ease-out-expo; };",
            "window-movement { spring; spring stiffness=900; };",
            "window-resize { off; off false; };",
            "window-open { duration 100; duration_ms 200; };",
            "window-close { duration 100; duration_ms 200; };",
            "window-open { curve linear; easing ease-out-expo; };",
            "window-close { curve linear; easing ease-out-expo; };",
            "window-open { spring; SPRING stiffness=900; };",
            "window-close { spring; SPRING stiffness=900; };",
            "window-open { off; off false; };",
            "window-close { off; off false; };",
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
                format!("window-open {{ {settings} }}"),
                format!("window-close {{ {settings} }}"),
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
            "window-open true",
            "window-close mystery=1",
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

    #[test]
    fn invalid_open_close_settings_are_errors() {
        for effect in ["window-open", "window-close"] {
            for settings in [
                "off \"typo\";",
                "duration-ms -2;",
                "duration-ms 60001;",
                "curve \"typo\";",
                "spring damping-ratio=-1;",
                "spring stiffness=0;",
                "spring epsilon=0;",
                "spring mystery=1;",
            ] {
                let body = format!("animations {{ {effect} {{ {settings} }} }}");
                let d = decode_text(&body, "", None);
                assert!(
                    d.issues
                        .iter()
                        .any(|i| i.severity == ValidationSeverity::Error),
                    "accepted {body}: {:?}",
                    d.issues
                );
            }
        }
    }
}
