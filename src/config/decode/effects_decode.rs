//! Decoding of the appearance blocks of niri's `layout {}`: `focus-ring`, `border`, `shadow`,
//! `insert-hint` and `tab-indicator`, with niri's colours and gradients.
//!
//! The older flat spellings (`border-width`, `focus-ring-color`, `shadow-blur`, ...) are still
//! read by `decode.rs`; the blocks here fill [`Effects`](crate::config::effects::Effects) and
//! the arms in `decode.rs` that know a flat key fill both.

use super::{norm, Decoder};
use crate::config::effects::{
    color_text, GradientSpace, GradientStyle, InsertHintStyle, PaintStyle, RingStyle, ShadowStyle,
    TabIndicatorStyle, TabPosition,
};
use crate::config::kdl::{Node, Pos};
use crate::config::types::LayoutConfigPartial;

impl Decoder {
    /// A colour node: `active-color "#7fc8ff"`, a CSS name, `rgb(...)`, or niri's deprecated
    /// four numbers (`active-color 127 200 255 255`).  Returns the text wiri stores.
    pub(super) fn paint_color(&mut self, node: &Node) -> Option<String> {
        if node.args.len() == 4 && node.args.iter().all(|a| a.as_f64().is_some()) {
            self.no_props(node);
            self.no_children(node);
            let mut c = [0u8; 4];
            for (i, a) in node.args.iter().enumerate() {
                let n = a.as_f64().unwrap_or(0.0);
                if !(0.0..=255.0).contains(&n) {
                    self.error(
                        a.pos,
                        format!("'{}': a colour channel is 0 to 255", node.name),
                    );
                    return None;
                }
                c[i] = n.round() as u8;
            }
            return Some(color_text(c));
        }
        self.arg_color(node)
    }

    /// `active-gradient from="#80c8ff" to="#bbddff" angle=45 relative-to="workspace-view"
    /// in="srgb-linear"`.
    pub(super) fn gradient_of(&mut self, node: &Node) -> Option<GradientStyle> {
        self.no_args(node);
        self.no_children(node);
        let mut from: Option<String> = None;
        let mut to: Option<String> = None;
        let mut angle = 180.0;
        let mut relative_to_view = false;
        let mut space = GradientSpace::Srgb;
        let mut ok = true;
        for prop in &node.props {
            match prop.key.as_str() {
                "from" => match self.value_color(node, &prop.value) {
                    Some(c) => from = Some(c),
                    None => ok = false,
                },
                "to" => match self.value_color(node, &prop.value) {
                    Some(c) => to = Some(c),
                    None => ok = false,
                },
                "angle" => match self.value_number(node, &prop.value, -360_000.0, 360_000.0) {
                    Some(a) => angle = a,
                    None => ok = false,
                },
                "relative-to" => match prop.value.as_str() {
                    Some("window") => relative_to_view = false,
                    Some("workspace-view") => relative_to_view = true,
                    _ => {
                        self.error(
                            prop.value.pos,
                            "'relative-to' is \"window\" or \"workspace-view\"",
                        );
                        ok = false;
                    }
                },
                "in" => match prop.value.as_str() {
                    Some(text) => {
                        let first = text.split_whitespace().next().unwrap_or("");
                        match first {
                            "srgb" => space = GradientSpace::Srgb,
                            "srgb-linear" => space = GradientSpace::SrgbLinear,
                            "oklab" => space = GradientSpace::Oklab,
                            "oklch" => space = GradientSpace::Oklch,
                            _ => {
                                self.error(
                                    prop.value.pos,
                                    "'in' is \"srgb\", \"srgb-linear\", \"oklab\" or \"oklch\"",
                                );
                                ok = false;
                            }
                        }
                        if space != GradientSpace::Srgb && ok {
                            self.warn(
                                prop.value.pos,
                                format!(
                                    "gradient colour space \"{}\" is drawn as srgb by wiri",
                                    text
                                ),
                            );
                        }
                    }
                    None => {
                        self.error(prop.value.pos, "'in' expects text");
                        ok = false;
                    }
                },
                other => {
                    let msg = format!("unexpected property '{}' on '{}'", other, node.name);
                    self.error(prop.key_pos, msg);
                    ok = false;
                }
            }
        }
        let (from, to) = match (from, to) {
            (Some(f), Some(t)) => (f, t),
            _ => {
                if ok {
                    self.error(node.pos, format!("'{}' needs from= and to=", node.name));
                }
                return None;
            }
        };
        if !ok {
            return None;
        }
        Some(GradientStyle {
            from,
            to,
            angle,
            relative_to_view,
            space,
        })
    }

    /// The colour and gradient keys of a ring-like block (`focus-ring`, `border`, the tab
    /// indicator and the insert hint).  Returns the paint a key sets, if `key` is one.
    fn paint_key(&mut self, c: &Node, key: &str) -> Option<Option<PaintStyle>> {
        match key {
            "active-color" | "inactive-color" | "urgent-color" | "color" => {
                Some(self.paint_color(c).map(PaintStyle::Color))
            }
            "active-gradient" | "inactive-gradient" | "urgent-gradient" | "gradient" => {
                Some(self.gradient_of(c).map(PaintStyle::Gradient))
            }
            _ => None,
        }
    }

    /// The keys `focus-ring` and `border` share.  Returns `true` when `c` was one of them.
    ///
    /// `legacy_colors` is wiri's older meaning of a bare `color` / `color-focused` / `color-urgent`
    /// (`border` only); the `focus-ring` takes a bare `color` for its active colour.
    pub(super) fn ring_key(&mut self, c: &Node, style: &mut RingStyle, is_border: bool) -> bool {
        let key = norm(&c.name);
        match key.as_str() {
            "off" => {
                if let Some(v) = self.flag(c) {
                    style.off = v;
                }
            }
            "on" => {
                if let Some(v) = self.flag(c) {
                    style.off = !v;
                }
            }
            "width" => {
                if let Some(w) = self.arg_number(c, 0.0, 4096.0) {
                    style.width = w.round() as u32;
                }
            }
            "active-color" | "active-gradient" => {
                if let Some(Some(p)) = self.paint_key(c, &key) {
                    style.active = p;
                }
            }
            "inactive-color" | "inactive-gradient" => {
                if let Some(Some(p)) = self.paint_key(c, &key) {
                    style.inactive = p;
                }
            }
            "urgent-color" | "urgent-gradient" => {
                if let Some(Some(p)) = self.paint_key(c, &key) {
                    style.urgent = p;
                }
            }
            // wiri's older names: `color` is the active colour of the ring and the inactive
            // colour of the border; `color-focused` and `color-urgent` are the border's.
            "color" => {
                if let Some(Some(p)) = self.paint_key(c, "color") {
                    if is_border {
                        style.inactive = p;
                    } else {
                        style.active = p;
                    }
                }
            }
            "color-focused" if is_border => {
                if let Some(Some(p)) = self.paint_key(c, "color") {
                    style.active = p;
                }
            }
            "color-urgent" if is_border => {
                if let Some(Some(p)) = self.paint_key(c, "color") {
                    style.urgent = p;
                }
            }
            _ => return false,
        }
        true
    }

    /// `shadow { on; softness 30; spread 5; offset x=0 y=5; draw-behind-window true;
    /// color "#00000070"; inactive-color "#00000054" }` and wiri's older flat spelling
    /// (`enable`, `opacity`, `offset-x`, `offset-y`, `blur`).
    pub(super) fn decode_shadow_style(
        &mut self,
        node: &Node,
        style: &mut ShadowStyle,
        p: &mut LayoutConfigPartial,
    ) {
        self.section(node);
        let mut legacy_opacity: Option<f64> = None;
        for c in node.kids() {
            let key = norm(&c.name);
            match key.as_str() {
                "on" => {
                    if let Some(v) = self.flag(c) {
                        style.on = v;
                        p.shadow_enable = Some(v);
                    }
                }
                "off" => {
                    if let Some(v) = self.flag(c) {
                        style.on = !v;
                        p.shadow_enable = Some(!v);
                    }
                }
                "enable" => {
                    if let Some(v) = self.flag(c) {
                        style.on = v;
                        p.shadow_enable = Some(v);
                    }
                }
                "softness" | "blur" => {
                    if let Some(v) = self.arg_number(c, 0.0, 4096.0) {
                        style.softness = v;
                        p.shadow_blur = Some(v.round() as u32);
                    }
                }
                "spread" => {
                    if let Some(v) = self.arg_number(c, -4096.0, 4096.0) {
                        style.spread = v;
                        p.shadow_spread = Some(v.max(0.0).round() as u32);
                    }
                }
                "opacity" => {
                    if let Some(v) = self.arg_number(c, 0.0, 1.0) {
                        legacy_opacity = Some(v);
                        p.shadow_opacity = Some(v);
                    }
                }
                "offset-x" => {
                    if let Some(v) = self.arg_number(c, -65535.0, 65535.0) {
                        style.offset_x = v;
                        p.shadow_offset_x = Some(v.round() as i32);
                    }
                }
                "offset-y" => {
                    if let Some(v) = self.arg_number(c, -65535.0, 65535.0) {
                        style.offset_y = v;
                        p.shadow_offset_y = Some(v.round() as i32);
                    }
                }
                "offset" => {
                    // niri: offset x=0 y=5
                    self.no_args(c);
                    self.no_children(c);
                    for prop in &c.props {
                        let n = self.value_number(c, &prop.value, -65535.0, 65535.0);
                        match (prop.key.as_str(), n) {
                            ("x", Some(v)) => {
                                style.offset_x = v;
                                p.shadow_offset_x = Some(v.round() as i32);
                            }
                            ("y", Some(v)) => {
                                style.offset_y = v;
                                p.shadow_offset_y = Some(v.round() as i32);
                            }
                            ("x", None) | ("y", None) => {}
                            (other, _) => {
                                let msg = format!("unexpected property '{}' on 'offset'", other);
                                self.error(prop.key_pos, msg);
                            }
                        }
                    }
                }
                "draw-behind-window" => {
                    if let Some(v) = self.flag(c) {
                        style.draw_behind_window = v;
                    }
                }
                "color" => {
                    if let Some(v) = self.paint_color(c) {
                        style.color = v.clone();
                        p.shadow_color = Some(v);
                    }
                }
                "inactive-color" => {
                    if let Some(v) = self.paint_color(c) {
                        style.inactive_color = Some(v);
                    }
                }
                _ => self.unknown(
                    c,
                    "'shadow'",
                    &[
                        "on",
                        "off",
                        "offset",
                        "softness",
                        "spread",
                        "color",
                        "inactive-color",
                        "draw-behind-window",
                    ],
                ),
            }
        }
        if let Some(opacity) = legacy_opacity {
            if let Some(mut rgba) = crate::config::effects::parse_css_color(&style.color) {
                rgba[3] = ((rgba[3] as f64) * opacity).round() as u8;
                style.color = color_text(rgba);
            }
        }
    }

    /// `insert-hint { off; color "#ffc87f80"; gradient from=... to=... }`.
    pub(super) fn decode_insert_hint(&mut self, node: &Node, style: &mut InsertHintStyle) {
        self.section(node);
        for c in node.kids() {
            let key = norm(&c.name);
            match key.as_str() {
                "off" => {
                    if let Some(v) = self.flag(c) {
                        style.off = v;
                    }
                }
                "on" => {
                    if let Some(v) = self.flag(c) {
                        style.off = !v;
                    }
                }
                "color" | "gradient" => {
                    if let Some(Some(p)) = self.paint_key(c, &key) {
                        style.paint = p;
                    }
                }
                _ => self.unknown(c, "'insert-hint'", &["off", "on", "color", "gradient"]),
            }
        }
    }

    /// `tab-indicator { on; hide-when-single-tab; place-within-column; gap 5; width 4;
    /// length total-proportion=1.0; position "right"; gaps-between-tabs 2; corner-radius 8;
    /// active-color "red"; ... }`.
    pub(super) fn decode_tab_indicator(&mut self, node: &Node, style: &mut TabIndicatorStyle) {
        self.section(node);
        for c in node.kids() {
            let key = norm(&c.name);
            match key.as_str() {
                "off" => {
                    if let Some(v) = self.flag(c) {
                        style.off = v;
                    }
                }
                "on" => {
                    if let Some(v) = self.flag(c) {
                        style.off = !v;
                    }
                }
                "hide-when-single-tab" => {
                    if let Some(v) = self.flag(c) {
                        style.hide_when_single_tab = v;
                    }
                }
                "place-within-column" => {
                    if let Some(v) = self.flag(c) {
                        style.place_within_column = v;
                    }
                }
                "gap" => {
                    if let Some(v) = self.arg_number(c, -65535.0, 65535.0) {
                        style.gap = v;
                    }
                }
                "width" => {
                    if let Some(v) = self.arg_number(c, 0.0, 65535.0) {
                        style.width = v;
                    }
                }
                "gaps-between-tabs" => {
                    if let Some(v) = self.arg_number(c, 0.0, 65535.0) {
                        style.gaps_between_tabs = v;
                    }
                }
                "corner-radius" => {
                    if let Some(v) = self.arg_number(c, 0.0, 65535.0) {
                        style.corner_radius = v;
                    }
                }
                "length" => {
                    self.no_args(c);
                    self.no_children(c);
                    for prop in &c.props {
                        if prop.key == "total-proportion" {
                            // The pinned niri schema accepts an optional f64 here; the
                            // indicator clamps it to 0..2 when producing geometry.
                            if let Some(v) = self.value_number(c, &prop.value, f64::MIN, f64::MAX) {
                                style.length = v;
                            }
                        } else {
                            let msg = format!("unexpected property '{}' on 'length'", prop.key);
                            self.error(prop.key_pos, msg);
                        }
                    }
                }
                "position" => {
                    if let Some(text) = self.arg_text(c) {
                        match text.as_str() {
                            "left" => style.position = TabPosition::Left,
                            "right" => style.position = TabPosition::Right,
                            "top" => style.position = TabPosition::Top,
                            "bottom" => style.position = TabPosition::Bottom,
                            _ => {
                                let at: Pos = c.args.first().map_or(c.pos, |a| a.pos);
                                self.error(
                                    at,
                                    "'position' is \"left\", \"right\", \"top\" or \"bottom\"",
                                );
                            }
                        }
                    }
                }
                "active-color" | "active-gradient" => {
                    if let Some(Some(p)) = self.paint_key(c, &key) {
                        style.active = Some(p);
                    }
                }
                "inactive-color" | "inactive-gradient" => {
                    if let Some(Some(p)) = self.paint_key(c, &key) {
                        style.inactive = Some(p);
                    }
                }
                "urgent-color" | "urgent-gradient" => {
                    if let Some(Some(p)) = self.paint_key(c, &key) {
                        style.urgent = Some(p);
                    }
                }
                _ => self.unknown(
                    c,
                    "'tab-indicator'",
                    &[
                        "off",
                        "on",
                        "hide-when-single-tab",
                        "place-within-column",
                        "gap",
                        "width",
                        "length",
                        "position",
                        "gaps-between-tabs",
                        "corner-radius",
                        "active-color",
                        "inactive-color",
                        "urgent-color",
                        "active-gradient",
                        "inactive-gradient",
                        "urgent-gradient",
                    ],
                ),
            }
        }
    }
}

#[cfg(test)]
mod tab_tests {
    use super::*;
    use crate::config::decode::decode_text;

    fn style(body: &str) -> TabIndicatorStyle {
        let decoded = decode_text(
            &format!("layout {{ tab-indicator {{ {body} }} }}"),
            "",
            None,
        );
        assert!(!decoded.has_errors(), "{body}: {:?}", decoded.issues);
        decoded.config.layout.effects.tab_indicator
    }

    #[test]
    fn pinned_niri_tab_indicator_defaults_and_all_options() {
        let defaults = style("");
        assert_eq!(defaults, TabIndicatorStyle::default());
        assert!(!defaults.off && !defaults.hide_when_single_tab && !defaults.place_within_column);
        assert_eq!(
            (defaults.width, defaults.gap, defaults.length),
            (4.0, 5.0, 0.5)
        );
        assert_eq!(
            (defaults.gaps_between_tabs, defaults.corner_radius),
            (0.0, 0.0)
        );
        assert_eq!(defaults.position, TabPosition::Left);
        assert!(
            defaults.active.is_none() && defaults.inactive.is_none() && defaults.urgent.is_none()
        );

        let parsed = style(
            r##"
            off; on; hide-when-single-tab; place-within-column;
            width 12.5; gap -6.25; gaps-between-tabs 3.5; corner-radius 8.75;
            length total-proportion=0.75; position "bottom";
            active-color "#ff0000";
            inactive-color "#00ff00";
            urgent-color "#0000ff";
        "##,
        );
        assert!(!parsed.off && parsed.hide_when_single_tab && parsed.place_within_column);
        assert_eq!(
            (parsed.width, parsed.gap, parsed.length),
            (12.5, -6.25, 0.75)
        );
        assert_eq!(
            (parsed.gaps_between_tabs, parsed.corner_radius),
            (3.5, 8.75)
        );
        assert_eq!(parsed.position, TabPosition::Bottom);
        for paint in [parsed.active, parsed.inactive, parsed.urgent] {
            assert!(matches!(paint, Some(PaintStyle::Color(_))));
        }
        assert!(style("off;").off);
    }

    #[test]
    fn every_indicator_side_and_gradient_spelling_is_decoded() {
        for (text, expected) in [
            ("left", TabPosition::Left),
            ("right", TabPosition::Right),
            ("top", TabPosition::Top),
            ("bottom", TabPosition::Bottom),
        ] {
            assert_eq!(style(&format!("position \"{text}\";")).position, expected);
        }
        for key in ["active-gradient", "inactive-gradient", "urgent-gradient"] {
            let parsed = style(&format!(
                "{key} from=\"red\" to=\"blue\" angle=45 relative-to=\"workspace-view\" in=\"srgb\";"
            ));
            let paint = match key {
                "active-gradient" => parsed.active,
                "inactive-gradient" => parsed.inactive,
                _ => parsed.urgent,
            };
            assert!(matches!(paint, Some(PaintStyle::Gradient(_))));
        }
    }

    #[test]
    fn indicator_dimensions_accept_full_pinned_ranges_and_decimals() {
        for (key, min) in [
            ("gap", -65535.0),
            ("width", 0.0),
            ("gaps-between-tabs", 0.0),
            ("corner-radius", 0.0),
        ] {
            for value in [min, 4096.5, 65535.0] {
                let parsed = style(&format!("{key} {value};"));
                let actual = match key {
                    "gap" => parsed.gap,
                    "width" => parsed.width,
                    "gaps-between-tabs" => parsed.gaps_between_tabs,
                    _ => parsed.corner_radius,
                };
                assert_eq!(actual, value);
            }
        }
        // Schema permits all finite proportions; rendering applies the 0..2 clamp.
        assert_eq!(style("length total-proportion=-0.5;").length, -0.5);
        assert_eq!(style("length total-proportion=3.5;").length, 3.5);
    }

    #[test]
    fn malformed_indicator_options_report_errors_instead_of_silently_defaulting() {
        for body in [
            "gap -65535.01;",
            "gap 65535.01;",
            "width -0.01;",
            "width 65535.01;",
            "gaps-between-tabs -1;",
            "gaps-between-tabs 65536;",
            "corner-radius -1;",
            "corner-radius 65536;",
            "position \"middle\";",
            "position 2;",
            "length 0.5;",
            "length proportion=0.5;",
            "length total-proportion=\"half\";",
            "width 1 2;",
            "gap size=2;",
            "unknown-option;",
            "active-color \"not-a-color\";",
            "active-gradient from=\"red\";",
            "inactive-gradient from=\"red\" to=\"blue\" foo=1;",
            "urgent-gradient from=\"red\" to=\"blue\" relative-to=\"column\";",
        ] {
            let decoded = decode_text(
                &format!("layout {{ tab-indicator {{ {body} }} }}"),
                "",
                None,
            );
            assert!(
                decoded.has_errors(),
                "accepted malformed tab-indicator: {body}"
            );
        }
    }
}
