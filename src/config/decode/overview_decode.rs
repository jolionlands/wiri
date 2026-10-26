//! Overview-only decoding, kept out of the shared layout/config decoder.
use super::{norm, Decoder};
use crate::config::{kdl::Node, overview_config::OverviewConfig, parse_css_color};

impl Decoder {
    pub(super) fn decode_overview(&mut self, node: &Node, cfg: &mut OverviewConfig) {
        self.section(node);
        for child in node.kids() {
            match norm(&child.name).as_str() {
                "zoom" => {
                    if let Some(value) = self.arg_number(child, 0.0, 1.0) {
                        cfg.zoom = value;
                    }
                }
                "backdrop-color" => {
                    if let Some(value) = self.paint_color(child).and_then(|s| parse_css_color(&s)) {
                        cfg.backdrop_color = value;
                    }
                }
                "workspace-shadow" => {
                    self.section(child);
                    for c in child.kids() {
                        let shadow = &mut cfg.workspace_shadow;
                        match norm(&c.name).as_str() {
                            "off" => {
                                if let Some(value) = self.flag(c) {
                                    shadow.off = value;
                                }
                            }
                            "on" => {
                                if let Some(value) = self.flag(c) {
                                    shadow.off = !value;
                                }
                            }
                            "softness" => {
                                if let Some(value) = self.arg_number(c, 0.0, 1024.0) {
                                    shadow.softness = value;
                                }
                            }
                            "spread" => {
                                if let Some(value) = self.arg_number(c, -1024.0, 1024.0) {
                                    shadow.spread = value;
                                }
                            }
                            "color" => {
                                if let Some(value) =
                                    self.paint_color(c).and_then(|s| parse_css_color(&s))
                                {
                                    shadow.color = value;
                                }
                            }
                            "offset" => {
                                self.no_args(c);
                                self.no_children(c);
                                for prop in &c.props {
                                    match prop.key.as_str() {
                                        "x" | "y" => {
                                            if let Some(value) =
                                                self.value_number(c, &prop.value, -65535.0, 65535.0)
                                            {
                                                if prop.key == "x" {
                                                    shadow.offset_x = value;
                                                } else {
                                                    shadow.offset_y = value;
                                                }
                                            }
                                        }
                                        _ => self.error(
                                            prop.key_pos,
                                            "workspace shadow offset accepts only x= and y=",
                                        ),
                                    }
                                }
                            }
                            _ => self.unknown(
                                c,
                                "'workspace-shadow'",
                                &["off", "on", "softness", "spread", "offset", "color"],
                            ),
                        }
                    }
                }
                _ => self.unknown(
                    child,
                    "'overview'",
                    &["zoom", "backdrop-color", "workspace-shadow"],
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::decode::decode_text;
    #[test]
    fn overview_defaults_and_all_niri_options_are_applied() {
        let d = decode_text(
            r##"overview {
            zoom 0.25
            backdrop-color "#12345680"
            workspace-shadow { softness 21.5; spread -2; offset x=-4 y=9; color 10 20 30 40; }
        }"##,
            "",
            None,
        );
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        let c = d.config.overview;
        assert_eq!(c.zoom, 0.25);
        assert_eq!(c.backdrop_color, [0x12, 0x34, 0x56, 0x80]);
        assert_eq!(c.workspace_shadow.softness, 21.5);
        assert_eq!(c.workspace_shadow.spread, -2.0);
        assert_eq!(c.workspace_shadow.offset_x, -4.0);
        assert_eq!(c.workspace_shadow.offset_y, 9.0);
        assert_eq!(c.workspace_shadow.color, [10, 20, 30, 40]);
        assert!(!c.workspace_shadow.off);
        let d = decode_text("overview { workspace-shadow { off; } }", "", None);
        assert!(d.issues.is_empty());
        assert!(d.config.overview.workspace_shadow.off);
    }
    #[test]
    fn overview_rejects_bad_ranges_and_unknown_shadow_fields() {
        for body in [
            "zoom -0.1",
            "zoom 1.1",
            "zoom nan",
            "backdrop-color nonsense",
            "workspace-shadow { softness 1025; }",
            "workspace-shadow { spread -1025; }",
            "workspace-shadow { offset z=2; }",
            "workspace-shadow { draw-behind-window true; }",
        ] {
            let d = decode_text(&format!("overview {{ {body}; }}"), "", None);
            assert!(d.has_errors(), "accepted {body}");
        }
    }
}
