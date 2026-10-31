//! Sparse window-rule appearance blocks; layout decoding remains unchanged.
use super::{norm, Decoder};
use crate::config::kdl::Node;
use crate::config::{PaintStyle, RingStyle, ShadowStyle};
use crate::window::rule_effects::{RingOverride, RuleEffects, ShadowOverride, TabOverride};

impl Decoder {
    pub(super) fn rule_effect_key(&mut self, node: &Node, rule: &mut RuleEffects) -> bool {
        match norm(&node.name).as_str() {
            "focus-ring" => self.rule_ring(node, &mut rule.focus_ring),
            "border" => self.rule_ring(node, &mut rule.border),
            "shadow" => self.rule_shadow(node, &mut rule.shadow),
            "tab-indicator" => self.rule_tab(node, &mut rule.tab_indicator),
            "geometry-corner-radius" => {
                self.no_props(node);
                self.no_children(node);
                if node.args.len() != 1 && node.args.len() != 4 {
                    self.error(node.pos, "'geometry-corner-radius' needs 1 or 4 radii");
                } else {
                    let mut values = [0.0f32; 4];
                    let mut valid = true;
                    for (i, value) in node.args.iter().enumerate() {
                        // niri: `CornerRadius` is four plain f32s with no parse-time bound
                        // (niri-config/src/appearance.rs:168-177).
                        if let Some(radius) =
                            self.value_number(node, value, f32::MIN as f64, f32::MAX as f64)
                        {
                            values[i] = radius as f32;
                        } else {
                            valid = false;
                        }
                    }
                    if node.args.len() == 1 {
                        values = [values[0]; 4];
                    }
                    if valid {
                        rule.geometry_corner_radius = Some(values);
                    }
                }
            }
            "clip-to-geometry" => self.set_flag(node, &mut rule.clip_to_geometry),
            _ => return false,
        }
        true
    }

    fn rule_ring(&mut self, node: &Node, target: &mut RingOverride) {
        self.section(node);
        let mut style = RingStyle::border_default();
        let (mut on, mut off) = (false, false);
        // Gradient wins over colour when both are present in the same block, independent
        // of source order (niri's merge_color_gradient).
        let mut kids: Vec<_> = node.kids().iter().collect();
        kids.sort_by_key(|c| norm(&c.name).ends_with("gradient"));
        for child in kids {
            let key = norm(&child.name);
            if key == "on" || key == "off" {
                if let Some(value) = self.flag(child) {
                    if key == "on" {
                        on |= value;
                    } else {
                        off |= value;
                    }
                }
                continue;
            }
            if !self.ring_key(child, &mut style, false) {
                self.unknown(
                    child,
                    "window-rule ring",
                    &[
                        "on",
                        "off",
                        "width",
                        "active-color",
                        "inactive-color",
                        "urgent-color",
                        "active-gradient",
                        "inactive-gradient",
                        "urgent-gradient",
                    ],
                );
                continue;
            }
            match key.as_str() {
                "width" => target.width = Some(style.width),
                "active-color" | "active-gradient" | "color" => {
                    target.active = Some(style.active.clone())
                }
                "inactive-color" | "inactive-gradient" => {
                    target.inactive = Some(style.inactive.clone())
                }
                "urgent-color" | "urgent-gradient" => target.urgent = Some(style.urgent.clone()),
                _ => {}
            }
        }
        if on {
            target.off = Some(false);
        } else if off {
            target.off = Some(true);
        }
    }

    fn rule_shadow(&mut self, node: &Node, target: &mut ShadowOverride) {
        let mut style = ShadowStyle::default();
        self.decode_shadow_style(node, &mut style, &mut Default::default());
        let (mut on, mut off) = (false, false);
        for child in node.kids() {
            match norm(&child.name).as_str() {
                "on" | "enable" => on |= self.flag(child).unwrap_or(false),
                "off" => off |= self.flag(child).unwrap_or(false),
                "softness" | "blur" => target.softness = Some(style.softness),
                "spread" => target.spread = Some(style.spread),
                "offset" => {
                    // Offset is one niri property: omitted x/y default to zero.
                    target.offset_x = Some(
                        child
                            .props
                            .iter()
                            .find(|p| p.key == "x")
                            .and_then(|p| p.value.as_f64())
                            .unwrap_or(0.0),
                    );
                    target.offset_y = Some(
                        child
                            .props
                            .iter()
                            .find(|p| p.key == "y")
                            .and_then(|p| p.value.as_f64())
                            .unwrap_or(0.0),
                    );
                }
                "offset-x" => target.offset_x = Some(style.offset_x),
                "offset-y" => target.offset_y = Some(style.offset_y),
                "draw-behind-window" => target.draw_behind_window = Some(style.draw_behind_window),
                "color" | "opacity" => target.color = Some(style.color.clone()),
                "inactive-color" => target.inactive_color = style.inactive_color.clone(),
                _ => {}
            }
        }
        // niri e19df12 appearance::Shadow::merge_with: off wins over on.
        if off {
            target.on = Some(false);
        } else if on {
            target.on = Some(true);
        }
    }

    fn rule_tab(&mut self, node: &Node, target: &mut TabOverride) {
        self.section(node);
        let mut kids: Vec<_> = node.kids().iter().collect();
        kids.sort_by_key(|c| norm(&c.name).ends_with("gradient"));
        for child in kids {
            let key = norm(&child.name);
            let paint = match key.as_str() {
                "active-color" | "inactive-color" | "urgent-color" => {
                    self.paint_color(child).map(PaintStyle::Color)
                }
                "active-gradient" | "inactive-gradient" | "urgent-gradient" => {
                    self.gradient_of(child).map(PaintStyle::Gradient)
                }
                _ => {
                    self.unknown(
                        child,
                        "window-rule tab-indicator",
                        &[
                            "active-color",
                            "inactive-color",
                            "urgent-color",
                            "active-gradient",
                            "inactive-gradient",
                            "urgent-gradient",
                        ],
                    );
                    None
                }
            };
            if let Some(paint) = paint {
                if key.starts_with("active-") {
                    target.active = Some(paint);
                } else if key.starts_with("inactive-") {
                    target.inactive = Some(paint);
                } else {
                    target.urgent = Some(paint);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sparse_blocks_load_without_resetting_and_flags_have_upstream_precedence() {
        let cfg = crate::config::parse::parse_kdl_config(
            r##"
layout { border { width 7; inactive-color "blue"; } }
window-rule {
    border { on; off; active-color "red"; }
    shadow { on; off; }
}
window-rule {
    border { width 11; }
    geometry-corner-radius 12 8 4 0
    clip-to-geometry true
}
"##,
        )
        .unwrap();
        let ctx = crate::config::types::MatcherContext::from_legacy("", "", None, None);
        let result = crate::window::resolve_window_rules(&cfg.window_rules, &ctx);
        let fx = result.effects.apply_to(&cfg.layout.effects);
        assert!(!fx.border.off);
        assert_eq!(fx.border.width, 11);
        assert_eq!(
            fx.border.inactive,
            crate::config::PaintStyle::color("#0000ff")
        );
        assert!(!fx.shadow.on);
        assert_eq!(
            result.effects.geometry_corner_radius,
            Some([12.0, 8.0, 4.0, 0.0])
        );
        assert_eq!(result.effects.clip_to_geometry, Some(true));
    }
}
