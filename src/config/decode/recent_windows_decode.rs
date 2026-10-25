//! Kept out of the shared decoder to avoid collisions with other parity work.
use super::{norm, Decoder};
use crate::config::kdl::Node;
use crate::config::recent_windows_config::{valid_recent_chord, RecentBind, RecentCycle, RecentScope, RecentWindowsConfig};

impl Decoder {
    pub(super) fn decode_recent_windows(&mut self, node: &Node, config: &mut RecentWindowsConfig) {
        self.section(node);
        for child in node.kids() {
            match norm(&child.name).as_str() {
                "off" => { if let Some(v) = self.flag(child) { config.enabled = !v; } }
                "debounce-ms" => { if let Some(v) = self.recent_milliseconds(child) { config.debounce_ms = v; } }
                "open-delay-ms" => { if let Some(v) = self.recent_milliseconds(child) { config.open_delay_ms = v; } }
                "highlight" => {
                    self.section(child);
                    for option in child.kids() {
                        match norm(&option.name).as_str() {
                            "active-color" | "urgent-color" => {
                                if let Some(text) = self.arg_text(option) {
                                    if crate::config::effects::parse_css_color(&text).is_none() {
                                        self.error(option.pos, "invalid recent-windows highlight color");
                                    } else if norm(&option.name) == "active-color" { config.highlight.active_color = text; }
                                    else { config.highlight.urgent_color = text; }
                                }
                            }
                            "padding" => { if let Some(v) = self.arg_number(option, 0.0, 100_000.0) { config.highlight.padding = v; } }
                            "corner-radius" => { if let Some(v) = self.arg_number(option, 0.0, 100_000.0) { config.highlight.corner_radius = v; } }
                            _ => self.unknown(option, "recent-windows highlight", &["active-color", "urgent-color", "padding", "corner-radius"]),
                        }
                    }
                }
                "previews" => {
                    self.section(child);
                    for option in child.kids() {
                        match norm(&option.name).as_str() {
                            "max-height" => { if let Some(v) = self.arg_number(option, 1.0, 100_000.0) { config.previews.max_height = v; } }
                            "max-scale" => { if let Some(v) = self.arg_number(option, 0.0, 1.0) { config.previews.max_scale = v; } }
                            _ => self.unknown(option, "recent-windows previews", &["max-height", "max-scale"]),
                        }
                    }
                }
                "binds" => {
                    self.section(child);
                    config.binds.clear(); // even binds {} disables the defaults
                    let mut seen = std::collections::HashSet::new();
                    for bind in child.kids() {
                        if let Some(parsed) = self.decode_recent_bind(bind) {
                            if !seen.insert(parsed.chord.to_ascii_lowercase()) {
                                self.error(bind.pos, "duplicate recent-windows chord");
                            } else { config.binds.push(parsed); }
                        }
                    }
                }
                _ => self.unknown(child, "recent-windows", &["off", "debounce-ms", "open-delay-ms", "highlight", "previews", "binds"]),
            }
        }
    }

    fn recent_milliseconds(&mut self, node: &Node) -> Option<u32> {
        let value = self.one_arg(node)?;
        match value.as_i64() {
            Some(n) if (0..=u32::MAX as i64).contains(&n) => Some(n as u32),
            _ => { self.error(value.pos, "expected an integer millisecond delay between 0 and 4294967295"); None }
        }
    }

    fn decode_recent_bind(&mut self, node: &Node) -> Option<RecentBind> {
        self.no_args(node); self.no_props(node);
        if !valid_recent_chord(&node.name) {
            self.error(node.pos, "recent-windows bind needs a modifier distinct from its trigger key");
            return None;
        }
        if node.kids().len() != 1 {
            self.error(node.pos, "recent-windows bind needs exactly one next-window or previous-window action");
            return None;
        }
        let action = &node.kids()[0];
        self.no_args(action); self.no_children(action);
        let mut cycle = RecentCycle::default();
        match norm(&action.name).as_str() {
            "next-window" => {}, "previous-window" => cycle.backwards = true,
            _ => { self.error(action.pos, "expected next-window or previous-window"); return None; }
        }
        let mut valid = true;
        let mut seen = std::collections::HashSet::new();
        for prop in &action.props {
            let key = norm(&prop.key);
            if !seen.insert(key.clone()) { self.error(prop.key_pos, "duplicate recent-windows action property"); valid = false; }
            match key.as_str() {
                "scope" => match prop.value.as_str().and_then(RecentScope::parse) {
                    Some(scope) => cycle.scope = scope,
                    None => { self.error(prop.value.pos, "scope must be all, output, or workspace"); valid = false; }
                }
                "filter" if prop.value.as_str() == Some("app-id") => cycle.filter_app_id = true,
                "filter" => { self.error(prop.value.pos, "filter must be app-id"); valid = false; }
                _ => { self.error(prop.key_pos, "unknown recent-windows action property"); valid = false; }
            }
        }
        valid.then(|| RecentBind { chord: node.name.clone(), action: cycle })
    }
}

#[cfg(test)]
mod tests {
    use crate::config::decode::{decode_text, ValidationSeverity};
    use crate::config::recent_windows_config::RecentScope;
    #[test]
    fn recent_settings_decode_and_explicit_empty_binds_remove_defaults() {
        let d = decode_text(r##"recent-windows {
            debounce-ms 120; open-delay-ms 0
            highlight { active-color "#10203080"; urgent-color "#ff0000"; padding 12; corner-radius 9; }
            previews { max-height 320; max-scale 0.25; }
            binds { Ctrl+Alt { previous-window scope="workspace" filter="app-id"; }; }
        }"##, "", None);
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        let c = d.config.recent_windows;
        assert_eq!((c.debounce_ms, c.open_delay_ms), (120, 0));
        assert_eq!((c.highlight.padding, c.previews.max_scale), (12.0, 0.25));
        assert_eq!(c.binds.len(), 1); assert!(c.binds[0].action.backwards && c.binds[0].action.filter_app_id);
        assert_eq!(c.binds[0].action.scope, RecentScope::Workspace);
        let d = decode_text("recent-windows { off; binds {}; }", "", None);
        assert!(!d.config.recent_windows.enabled); assert!(d.config.recent_windows.binds.is_empty());
    }
    #[test]
    fn invalid_recent_configuration_is_rejected() {
        for body in ["debounce-ms -1", "open-delay-ms 0.5", "previews { max-scale -0.5; }",
            "highlight { padding -1; }", "binds { Alt { next-window; }; }",
            "binds { Alt+Alt_L { next-window; }; }", "binds { Alt+Tab { bogus; }; }",
            "binds { Alt+Tab { next-window filter=\"title\"; }; }",
            "binds { Alt+Tab { next-window scope=\"monitor\"; }; }"] {
            let d = decode_text(&format!("recent-windows {{ {body}; }}"), "", None);
            assert!(d.issues.iter().any(|i| i.severity == ValidationSeverity::Error), "accepted {body}");
        }
    }
}
