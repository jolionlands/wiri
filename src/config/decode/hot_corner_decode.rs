//! `hot-corners` block decoding (global under `gestures`, and per `output`), pinned to
//! niri e19df12cf8. The `gestures` section itself is decoded in `gestures_decode.rs`.
use super::{norm, Decoder};
use crate::config::{hot_corner_config::HotCorners, kdl::Node};

impl Decoder {
    pub(super) fn decode_hot_corners(&mut self, node: &Node) -> HotCorners {
        self.section(node);
        let mut corners = HotCorners::default();
        for child in node.kids() {
            let key = norm(&child.name);
            let target = match key.as_str() {
                "off" => &mut corners.off,
                "top-left" => &mut corners.top_left,
                "top-right" => &mut corners.top_right,
                "bottom-left" => &mut corners.bottom_left,
                "bottom-right" => &mut corners.bottom_right,
                _ => {
                    self.unknown(
                        child,
                        "'hot-corners'",
                        &[
                            "off",
                            "top-left",
                            "top-right",
                            "bottom-left",
                            "bottom-right",
                        ],
                    );
                    continue;
                }
            };
            if let Some(value) = self.flag(child) {
                *target = value;
            }
        }
        corners
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{decode::decode_text, hot_corner_config::HotCorners};

    #[test]
    fn global_off_and_named_output_off_are_supported() {
        let decoded = decode_text(
            "gestures { hot-corners { off; }; }\noutput \"DP-1\" { hot-corners { off; }; }",
            "",
            None,
        );
        assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
        assert!(decoded.config.gestures.hot_corners.off);
        let output = decoded
            .config
            .output
            .iter()
            .find(|o| o.name == "DP-1")
            .unwrap();
        assert!(output.hot_corners.unwrap().off);
    }

    #[test]
    fn absent_output_inherits_and_explicit_block_replaces_global() {
        let decoded = decode_text(
            "gestures { hot-corners { off; }; }\noutput \"DP-1\" {}\noutput \"DP-2\" { hot-corners {}; }",
            "",
            None,
        );
        assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
        let global = decoded.config.gestures.hot_corners;
        let outputs = &decoded.config.output;
        assert!(!global
            .for_output(
                outputs
                    .iter()
                    .find(|o| o.name == "DP-1")
                    .unwrap()
                    .hot_corners
            )
            .top_left_enabled());
        assert!(global
            .for_output(
                outputs
                    .iter()
                    .find(|o| o.name == "DP-2")
                    .unwrap()
                    .hot_corners
            )
            .top_left_enabled());
    }

    #[test]
    fn default_top_left_and_repeat_blocks_replace_only_when_present() {
        let decoded = decode_text(
            "gestures { hot-corners { off; }; }\ngestures {}\ngestures { hot-corners {}; }",
            "",
            None,
        );
        assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
        assert_eq!(decoded.config.gestures.hot_corners, HotCorners::default());
        assert!(decoded.config.gestures.hot_corners.top_left_enabled());
    }

    #[test]
    fn named_corners_replace_default_and_are_applied_without_warnings() {
        let decoded = decode_text(
            "gestures { hot-corners { top-right; bottom-right; }; }",
            "",
            None,
        );
        assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
        assert_eq!(
            decoded.config.gestures.hot_corners.enabled(),
            [false, true, false, true]
        );
    }

    #[test]
    fn malformed_corner_flags_are_errors() {
        for text in [
            "gestures { hot-corners { off 1 2; }; }",
            "gestures { hot-corners { off { top-left; }; }; }",
            "gestures { hot-corners { top-left \"nope\"; }; }",
            "output \"DP-1\" { hot-corners unexpected=true {}; }",
            "gestures { hot-corners { top-lefft; }; }",
        ] {
            assert!(decode_text(text, "", None).has_errors(), "{text}");
        }
    }
}
