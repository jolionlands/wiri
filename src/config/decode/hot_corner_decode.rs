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
    fn an_output_named_in_kdl_escapes_or_literally_matches_the_windows_connector() {
        use crate::backend::{outputs::OutputMatcher, MonitorInfo};
        use crate::utils::{OutputId, Rect};
        // `\\.\DISPLAY1` is what Windows calls the connector. The live H64 gates write it with
        // KDL's escapes (the form CONFIG.md and the IPC JSON show); a literal spelling has
        // always worked. Both must reach the same monitor, or `output { hot-corners }` is
        // silently ignored (it was: the escaped spelling kept four leading backslashes).
        let monitor = MonitorInfo {
            id: OutputId::from_name(r"\\.\DISPLAY1"),
            name: r"\\.\DISPLAY1".to_string(),
            bounds: Rect::new(0, 0, 1366, 768),
            work_area: Rect::new(0, 0, 1366, 728),
            is_primary: true,
            scale_factor: 1.0,
        };
        for output in [r#""\\\\.\\DISPLAY1""#, r#""\\.\DISPLAY1""#] {
            let off = decode_text(
                &format!("gestures {{ hot-corners {{ top-right; }}; }}\noutput {output} {{ hot-corners {{ off; }}; }}"),
                "",
                None,
            );
            assert!(off.issues.is_empty(), "{output}: {:?}", off.issues);
            let matched = OutputMatcher::default()
                .matching_config(&off.config, &monitor)
                .unwrap_or_else(|| panic!("{output} named no monitor: {:?}", off.config.output));
            let policy = off
                .config
                .gestures
                .hot_corners
                .for_output(matched.hot_corners);
            assert_eq!(policy.enabled(), [false; 4], "{output}: per-output off");

            let on = decode_text(
                &format!("gestures {{ hot-corners {{ off; }}; }}\noutput {output} {{ hot-corners {{}}; }}"),
                "",
                None,
            );
            assert!(on.issues.is_empty(), "{output}: {:?}", on.issues);
            let matched = OutputMatcher::default()
                .matching_config(&on.config, &monitor)
                .unwrap_or_else(|| panic!("{output} named no monitor: {:?}", on.config.output));
            let policy = on
                .config
                .gestures
                .hot_corners
                .for_output(matched.hot_corners);
            assert_eq!(
                policy.enabled(),
                [true, false, false, false],
                "{output}: per-output override re-enables the default corner"
            );
        }
    }

    #[test]
    fn default_top_left_unless_a_block_replaces_it_and_a_repeated_section_is_an_error() {
        // No block, or a gestures section without `hot-corners`, keeps niri's top-left.
        for text in ["", "gestures {}", "gestures { hot-corners {}; }"] {
            let decoded = decode_text(text, "", None);
            assert!(decoded.issues.is_empty(), "{text}: {:?}", decoded.issues);
            assert_eq!(decoded.config.gestures.hot_corners, HotCorners::default());
            assert!(decoded.config.gestures.hot_corners.top_left_enabled());
        }
        // Lane C2: one `gestures` section per file. A second one is a located error rather
        // than a silent replacement, so a stale block can never override the intended one.
        let repeated = decode_text(
            "gestures { hot-corners { off; }; }\ngestures { hot-corners {}; }",
            "",
            None,
        );
        assert!(repeated.has_errors());
        assert!(
            repeated
                .issues
                .iter()
                .any(|i| i.message.contains("duplicate")),
            "{:?}",
            repeated.issues
        );
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
