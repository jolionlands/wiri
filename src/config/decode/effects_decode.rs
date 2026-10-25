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
                    if let Some(v) = self.arg_number(c, -4096.0, 4096.0) {
                        style.gap = v;
                    }
                }
                "width" => {
                    if let Some(v) = self.arg_number(c, 0.0, 4096.0) {
                        style.width = v;
                    }
                }
                "gaps-between-tabs" => {
                    if let Some(v) = self.arg_number(c, 0.0, 4096.0) {
                        style.gaps_between_tabs = v;
                    }
                }
                "corner-radius" => {
                    if let Some(v) = self.arg_number(c, 0.0, 4096.0) {
                        style.corner_radius = v;
                    }
                }
                "length" => {
                    self.no_args(c);
                    self.no_children(c);
                    for prop in &c.props {
                        if prop.key == "total-proportion" {
                            if let Some(v) = self.value_number(c, &prop.value, 0.0, 2.0) {
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
                    ],
                ),
            }
        }
    }
}
