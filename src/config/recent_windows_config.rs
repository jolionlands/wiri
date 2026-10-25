//! Configuration for the niri-style recent-windows switcher.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RecentScope {
    #[default]
    All,
    Output,
    Workspace,
}

impl RecentScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value { "all" => Some(Self::All), "output" => Some(Self::Output),
            "workspace" => Some(Self::Workspace), _ => None }
    }
    pub fn next(self) -> Self {
        match self { Self::All => Self::Output, Self::Output => Self::Workspace, Self::Workspace => Self::All }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecentCycle {
    pub backwards: bool,
    pub filter_app_id: bool,
    /// Only applied when opening, not on repeated cycling.
    pub scope: RecentScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentBind {
    pub chord: String,
    pub action: RecentCycle,
}

/// Reject bindings that cannot keep the switcher open. Mod is resolved by the
/// input layer; modifier-key triggers (Ctrl+Alt) are deliberately permitted.
pub fn valid_recent_chord(chord: &str) -> bool {
    fn modifier(s: &str) -> Option<&'static str> {
        match s.to_ascii_lowercase().as_str() {
            "alt" | "alt_l" | "alt_r" | "lalt" | "ralt" => Some("alt"),
            "ctrl" | "control" | "control_l" | "control_r" | "lctrl" | "rctrl" => Some("ctrl"),
            "shift" | "shift_l" | "shift_r" | "lshift" | "rshift" => Some("shift"),
            "win" | "super" | "super_l" | "super_r" | "lwin" | "rwin" => Some("win"),
            "mod" => Some("mod"),
            _ => None,
        }
    }
    let parts: Vec<_> = chord.split('+').collect();
    if parts.len() < 2 || parts.iter().any(|s| s.is_empty()) { return false; }
    let trigger = modifier(parts[parts.len() - 1]);
    let mods = &parts[..parts.len() - 1];
    mods.iter().all(|s| modifier(s).is_some())
        && mods.iter().any(|s| modifier(s) != trigger)
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecentHighlight {
    pub active_color: String,
    pub urgent_color: String,
    pub padding: f64,
    pub corner_radius: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecentPreviews {
    pub max_height: f64,
    pub max_scale: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecentWindowsConfig {
    pub enabled: bool,
    pub debounce_ms: u32,
    pub open_delay_ms: u32,
    pub highlight: RecentHighlight,
    pub previews: RecentPreviews,
    /// An explicit binds block replaces this entire list, even when empty.
    /// Normal binds take precedence when these are compiled by the input layer.
    pub binds: Vec<RecentBind>,
}

impl Default for RecentWindowsConfig {
    fn default() -> Self {
        let mut binds = Vec::new();
        for modifier in ["Alt", "Mod"] {
            for (key, filter_app_id) in [("Tab", false), ("grave", true)] {
                for backwards in [false, true] {
                    binds.push(RecentBind {
                        chord: format!("{modifier}+{}{key}", if backwards { "Shift+" } else { "" }),
                        action: RecentCycle { backwards, filter_app_id, scope: RecentScope::All },
                    });
                }
            }
        }
        Self {
            enabled: true, debounce_ms: 750, open_delay_ms: 150,
            highlight: RecentHighlight { active_color: "#999999ff".into(),
                urgent_color: "#ff9999ff".into(), padding: 30.0, corner_radius: 0.0 },
            previews: RecentPreviews { max_height: 480.0, max_scale: 0.5 }, binds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_modifier_trigger_validation() {
        let c = RecentWindowsConfig::default();
        assert_eq!((c.debounce_ms, c.open_delay_ms, c.binds.len()), (750, 150, 8));
        for b in c.binds { assert!(valid_recent_chord(&b.chord)); }
        assert!(valid_recent_chord("Ctrl+Alt"));
        for s in ["Alt", "Tab", "Alt+Alt_L", "Shift+Shift_R", "Foo+Tab", "Alt++Tab"] {
            assert!(!valid_recent_chord(s), "{s}");
        }
        assert_eq!(RecentScope::All.next().next().next(), RecentScope::All);
    }
}
