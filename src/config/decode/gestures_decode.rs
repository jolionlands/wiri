use super::{norm, Decoder};
use crate::config::gestures_config::GesturesConfig;
use crate::config::kdl::Node;
impl Decoder {
    pub(super) fn decode_gestures(&mut self, node: &Node, cfg: &mut GesturesConfig) {
        self.section(node);
        cfg.enabled = true;
        for child in node.kids() {
            let name = norm(&child.name);
            let (target, trigger) = match name.as_str() {
                "dnd-edge-view-scroll" => (&mut cfg.view, "trigger-width"),
                "dnd-edge-workspace-switch" => (&mut cfg.workspace, "trigger-height"),
                "hot-corners" => {
                    self.unsupported(
                        child,
                        "hot corners are not implemented by this drag adapter",
                    );
                    continue;
                }
                _ => {
                    self.unknown(
                        child,
                        "gestures",
                        &[
                            "dnd-edge-view-scroll",
                            "dnd-edge-workspace-switch",
                            "hot-corners",
                        ],
                    );
                    continue;
                }
            };
            self.section(child);
            for key in child.kids() {
                match norm(&key.name).as_str() {
                    k if k == trigger => {
                        if let Some(v) = self.arg_number(key, 0.0, 10000.0) {
                            target.trigger = v;
                        }
                    }
                    "delay-ms" => {
                        if let Some(v) = self.arg_u32(key, 60000) {
                            target.delay_ms = v;
                        }
                    }
                    "max-speed" => {
                        if let Some(v) = self.arg_number(key, 0.0, 100000.0) {
                            target.max_speed = v;
                        }
                    }
                    _ => self.unknown(key, &child.name, &[trigger, "delay-ms", "max-speed"]),
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn niri_drag_config_is_typed_and_validated() {
        let decode = |s| crate::config::decode::decode_text(s, "", None);
        assert!(!decode("").config.gestures.enabled);
        let d = decode("gestures { dnd-edge-view-scroll { trigger-width 40; delay-ms 200; max-speed 900; }; dnd-edge-workspace-switch { trigger-height 60; }; }");
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        assert!(d.config.gestures.enabled);
        assert_eq!(d.config.gestures.view.trigger, 40.0);
        assert_eq!(d.config.gestures.view.delay_ms, 200);
        assert_eq!(d.config.gestures.view.max_speed, 900.0);
        assert_eq!(d.config.gestures.workspace.trigger, 60.0);
        assert!(decode("gestures { dnd-edge-view-scroll { max-speed -1; }; }").has_errors());
        assert!(
            decode("gestures { dnd-edge-workspace-switch { trigger-width 50; }; }").has_errors()
        );
    }
}
