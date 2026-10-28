//! niri's `block-out-from` rule property, applied to wiri's own overlay windows.
//!
//! niri: `block-out-from "screencast"` keeps a window (or a layer surface) out of
//! screencasts while normal screenshots still contain it, and
//! `block-out-from "screen-capture"` keeps it out of every kind of screen capture
//! (wiki:Screencasting — "Block out windows"; wiki:Configuration:-Window-Rules).
//!
//! Windows equivalent: `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` (Windows
//! 10 2004+).  The capture APIs — `Windows.Graphics.Capture`, DXGI Desktop Duplication and
//! the GDI screen copy wiri's own screenshot code uses — leave such a window out.  Windows
//! offers one affinity for both niri values, so `"screencast"` and `"screen-capture"` are
//! both `WDA_EXCLUDEFROMCAPTURE` here (an approximation: a screencast and a screenshot are
//! the same thing to that API).
//!
//! `SetWindowDisplayAffinity` only works for a window that belongs to the calling process,
//! so wiri applies the rule to **its own** overlay windows — the workspace indicator, the
//! overview host and banner, the status bar, the hotkey list, the recent-windows switcher,
//! the screenshot UI and the decoration layers.  Foreign application windows cannot be
//! excluded from capture without injecting a DLL into every one of them, which wiri does
//! not do (so that half of the niri feature stays `partial`, and says so in the parity
//! matrix).
//!
//! A rule reaches wiri's overlays through their synthetic identity: the window class
//! `wiri-overlay` and a title naming the overlay (`workspace-indicator`,
//! `overview-banner`, `overview`, `status-bar`, `hotkeys`, `recent-windows`,
//! `screenshot-ui`, `decor`, `animation`).  So
//!
//! ```kdl
//! // niri layer rules apply to layer surfaces, which Windows has none of; wiri's own
//! // overlays are the closest thing, and a layer rule's namespace is their name.
//! layer-rule {
//!     match namespace="workspace-indicator"
//!     block-out-from "screencast"
//! }
//! window-rule {
//!     match title="overview"
//!     block-out-from "screen-capture"
//! }
//! ```
//!
//! Layer rules are applied first and window rules after them, so a window rule wins when
//! both match one overlay.  Nothing is polled: the affinity is set when an overlay creates
//! its window, when a config is applied, and at the one startup-rule expiry.

use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, SetWindowDisplayAffinity,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};

use crate::config::types::MatcherContext;
use crate::config::WindowRule;

/// niri's two `block-out-from` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockOutFrom {
    /// `block-out-from "screencast"`: out of screencasts, still in screenshots.
    Screencast,
    /// `block-out-from "screen-capture"`: out of every capture.
    ScreenCapture,
}

impl BlockOutFrom {
    /// niri's spelling of the value; `None` for anything else (a config error).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "screencast" => Some(Self::Screencast),
            "screen-capture" | "screen_capture" | "screencapture" => Some(Self::ScreenCapture),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Screencast => "screencast",
            Self::ScreenCapture => "screen-capture",
        }
    }

    /// Windows has a single "this window is not in any capture" affinity, so both niri
    /// values map to it (the row says the values collapse).
    pub fn excluded(self) -> bool {
        true
    }
}

/// One of wiri's own overlay windows, as a rule can match it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Overlay {
    WorkspaceIndicator,
    OverviewBanner,
    OverviewHost,
    StatusBar,
    HotkeyList,
    RecentWindows,
    ScreenshotUi,
    Decor,
    Animation,
}

/// The class name every overlay identity answers to in a rule matcher.
pub const OVERLAY_CLASS: &str = "wiri-overlay";

impl Overlay {
    pub const ALL: [Overlay; 9] = [
        Overlay::WorkspaceIndicator,
        Overlay::OverviewBanner,
        Overlay::OverviewHost,
        Overlay::StatusBar,
        Overlay::HotkeyList,
        Overlay::RecentWindows,
        Overlay::ScreenshotUi,
        Overlay::Decor,
        Overlay::Animation,
    ];

    /// The rule-visible name (`match title="…"`, layer-rule `namespace`).
    pub fn name(self) -> &'static str {
        match self {
            Self::WorkspaceIndicator => "workspace-indicator",
            Self::OverviewBanner => "overview-banner",
            Self::OverviewHost => "overview",
            Self::StatusBar => "status-bar",
            Self::HotkeyList => "hotkeys",
            Self::RecentWindows => "recent-windows",
            Self::ScreenshotUi => "screenshot-ui",
            Self::Decor => "decor",
            Self::Animation => "animation",
        }
    }

    /// The window class the overlay's HWND is created with.
    pub fn class_name(self) -> &'static str {
        match self {
            Self::WorkspaceIndicator => "WiriWorkspaceIndicator",
            Self::OverviewBanner => "WiriOverviewBanner",
            Self::OverviewHost => "WiriThumbnailOverviewHost",
            Self::StatusBar => "WiriStatusBar",
            Self::HotkeyList => "WiriBindingsCheatsheet",
            Self::RecentWindows => "WiriRecentWindowsHost",
            Self::ScreenshotUi => "WiriScreenshotUI",
            Self::Decor => "WiriDecor",
            Self::Animation => "WiriAnimationLayer",
        }
    }

    /// The overlay that owns `class_name`, when the class is one of wiri's overlays.
    pub fn from_class_name(class_name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|overlay| overlay.class_name().eq_ignore_ascii_case(class_name))
    }

    /// The matcher context a rule sees for this overlay.
    pub fn matcher_context(self) -> MatcherContext<'static> {
        MatcherContext::from_legacy(OVERLAY_CLASS, self.name(), None, None)
    }
}

/// The affinity the configured rules ask for on one of wiri's own overlays.
///
/// Every matching layer rule is applied first and every matching window rule after it, so
/// a later window rule wins, like every other window-rule property (last match wins).
pub fn resolve(
    layer_rules: &[WindowRule],
    window_rules: &[WindowRule],
    overlay: Overlay,
) -> Option<BlockOutFrom> {
    resolve_in_phase(layer_rules, window_rules, overlay, false)
}

fn resolve_in_phase(
    layer_rules: &[WindowRule],
    window_rules: &[WindowRule],
    overlay: Overlay,
    at_startup: bool,
) -> Option<BlockOutFrom> {
    let mut ctx = overlay.matcher_context();
    ctx.at_startup = at_startup;
    let mut resolved = None;
    for rule in layer_rules.iter().chain(window_rules) {
        if let (true, Some(value)) = (rule.matches(&ctx), rule.block_out_from) {
            resolved = Some(value);
        }
    }
    resolved
}

/// Apply one rule decision to a window of this process.  Returns whether the call was
/// made; a failed call (an older Windows, a window with a different owner) is logged and
/// leaves the window as it was.
pub fn set_affinity(hwnd: isize, blocked: Option<BlockOutFrom>) -> bool {
    if hwnd == 0 {
        return false;
    }
    let affinity = match blocked {
        Some(_) => WDA_EXCLUDEFROMCAPTURE,
        None => WDA_NONE,
    };
    unsafe {
        match SetWindowDisplayAffinity(HWND(hwnd as *mut _), affinity) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    %error,
                    hwnd,
                    blocked = blocked.map(BlockOutFrom::as_str),
                    "could not set the window's capture affinity"
                );
                false
            }
        }
    }
}

/// The affinity currently set on a window (`WDA_NONE` when there is none).
pub fn affinity_of(hwnd: isize) -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowDisplayAffinity;
    if hwnd == 0 {
        return None;
    }
    let mut affinity = 0u32;
    unsafe { GetWindowDisplayAffinity(HWND(hwnd as *mut _), &mut affinity) }
        .ok()
        .map(|()| affinity)
}

// ---------------------------------------------------------------------------------------
// The policy installed from the config
// ---------------------------------------------------------------------------------------

use parking_lot::RwLock;
use std::sync::LazyLock;

struct Policy {
    layer_rules: Vec<WindowRule>,
    window_rules: Vec<WindowRule>,
    at_startup: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            layer_rules: Vec::new(),
            window_rules: Vec::new(),
            at_startup: true,
        }
    }
}

impl Policy {
    fn resolve(&self, overlay: Overlay) -> Option<BlockOutFrom> {
        resolve_in_phase(
            &self.layer_rules,
            &self.window_rules,
            overlay,
            self.at_startup,
        )
    }

    fn install(&mut self, layer_rules: &[WindowRule], window_rules: &[WindowRule]) {
        self.layer_rules = layer_rules.to_vec();
        self.window_rules = window_rules.to_vec();
    }

    fn finish_startup(&mut self) -> bool {
        std::mem::replace(&mut self.at_startup, false)
    }
}

static POLICY: LazyLock<RwLock<Policy>> = LazyLock::new(|| RwLock::new(Policy::default()));

/// Install the rules a config load decided (startup or reload).  Overlays that already
/// have their window are updated here; later ones read the policy when they are created.
pub fn install(layer_rules: &[WindowRule], window_rules: &[WindowRule]) {
    POLICY.write().install(layer_rules, window_rules);
    if layer_rules
        .iter()
        .chain(window_rules)
        .any(|rule| rule.block_out_from.is_some())
    {
        // Said once per config load: the same affinity cannot be set on another process's
        // window, so this rule reaches wiri's overlays and nothing else.
        tracing::info!(
            "block-out-from applies to wiri's own overlay windows; \
             another process's windows cannot be excluded from screen capture"
        );
    }
    refresh_own_windows();
}

/// Follow the engine's one startup timeout, including overlays already created.
/// Rule reloads preserve this phase, and later overlays resolve the expired policy.
pub fn finish_startup() {
    let changed = POLICY.write().finish_startup();
    if changed {
        refresh_own_windows();
    }
}

/// The affinity the installed policy asks for on `overlay`.
pub fn blocked_from(overlay: Overlay) -> Option<BlockOutFrom> {
    POLICY.read().resolve(overlay)
}

/// Apply the installed policy to one overlay window that has just been created.
pub fn apply_to(overlay: Overlay, hwnd: isize) -> Option<BlockOutFrom> {
    let blocked = blocked_from(overlay);
    if blocked.is_some() {
        set_affinity(hwnd, blocked);
    }
    blocked
}

/// Apply the installed policy to every overlay window this process already owns.
///
/// Called per config load and at startup expiry: no polling, and nothing happens for overlays
/// that do not exist yet — they read the policy when their window is created.
pub fn refresh_own_windows() -> usize {
    let mut windows: Vec<(isize, Overlay)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect_own), LPARAM(&mut windows as *mut _ as isize));
    }
    let mut applied = 0;
    for (hwnd, overlay) in windows {
        let blocked = blocked_from(overlay);
        // A reload can remove a rule: the window then goes back to WDA_NONE.  Compare with
        // what Windows reports first so an unchanged window is not touched at all.
        let wanted = if blocked.is_some() {
            WDA_EXCLUDEFROMCAPTURE.0
        } else {
            WDA_NONE.0
        };
        if affinity_of(hwnd) == Some(wanted) {
            continue;
        }
        if set_affinity(hwnd, blocked) {
            applied += 1;
        }
    }
    applied
}

unsafe extern "system" fn collect_own(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let windows = &mut *(lparam.0 as *mut Vec<(isize, Overlay)>);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid != GetCurrentProcessId() {
        return BOOL(1);
    }
    let mut class = [0u16; 128];
    let len = GetClassNameW(hwnd, &mut class);
    if len <= 0 {
        return BOOL(1);
    }
    let class = String::from_utf16_lossy(&class[..len as usize]);
    if let Some(overlay) = Overlay::from_class_name(&class) {
        windows.push((hwnd.0 as isize, overlay));
    }
    BOOL(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{MatchGroup, Matcher};

    fn rule(matchers: Vec<Matcher>, blocked: Option<BlockOutFrom>) -> WindowRule {
        WindowRule {
            match_groups: vec![MatchGroup { conds: matchers }],
            block_out_from: blocked,
            ..Default::default()
        }
    }

    fn by_name(name: &str) -> Matcher {
        Matcher::TitleSearch(crate::config::types::RegexPat::new(name).unwrap())
    }

    #[test]
    fn startup_rule_phase_applies_to_owned_overlay_policy() {
        let policy = Policy {
            layer_rules: vec![rule(
                vec![Matcher::AtStartup],
                Some(BlockOutFrom::Screencast),
            )],
            ..Policy::default()
        };

        assert_eq!(
            policy.resolve(Overlay::WorkspaceIndicator),
            Some(BlockOutFrom::Screencast)
        );
    }

    #[test]
    fn startup_rule_phase_overlay_expiry_resolves_existing_and_later_overlays() {
        let mut policy = Policy::default();
        policy.install(
            &[rule(
                vec![Matcher::AtStartup],
                Some(BlockOutFrom::Screencast),
            )],
            &[rule(
                vec![Matcher::Not(Box::new(Matcher::AtStartup))],
                Some(BlockOutFrom::ScreenCapture),
            )],
        );
        for overlay in Overlay::ALL {
            assert_eq!(policy.resolve(overlay), Some(BlockOutFrom::Screencast));
        }

        assert!(policy.finish_startup());

        for overlay in Overlay::ALL {
            assert_eq!(policy.resolve(overlay), Some(BlockOutFrom::ScreenCapture));
        }
        assert!(!policy.finish_startup(), "the timeout is consumed once");
    }

    #[test]
    fn startup_rule_phase_overlay_reload_preserves_the_current_phase() {
        let mut policy = Policy::default();
        let startup = rule(vec![Matcher::AtStartup], Some(BlockOutFrom::Screencast));
        policy.install(&[startup.clone()], &[]);
        assert_eq!(
            policy.resolve(Overlay::Decor),
            Some(BlockOutFrom::Screencast)
        );
        policy.install(&[], &[startup.clone()]);
        assert_eq!(
            policy.resolve(Overlay::Decor),
            Some(BlockOutFrom::Screencast)
        );

        assert!(policy.finish_startup());
        assert_eq!(policy.resolve(Overlay::Decor), None);
        policy.install(&[startup.clone()], &[startup]);
        assert_eq!(policy.resolve(Overlay::Decor), None);
        assert!(!policy.finish_startup());
    }

    #[test]
    fn startup_rule_phase_overlay_expiry_rechecks_exclusions() {
        let mut policy = Policy::default();
        policy.install(
            &[WindowRule {
                exclude_groups: vec![MatchGroup {
                    conds: vec![Matcher::AtStartup],
                }],
                ..rule(Vec::new(), Some(BlockOutFrom::ScreenCapture))
            }],
            &[],
        );
        assert_eq!(policy.resolve(Overlay::ScreenshotUi), None);

        assert!(policy.finish_startup());

        assert_eq!(
            policy.resolve(Overlay::ScreenshotUi),
            Some(BlockOutFrom::ScreenCapture)
        );
    }

    #[test]
    fn niri_values_are_understood_and_anything_else_is_not() {
        assert_eq!(
            BlockOutFrom::parse("screencast"),
            Some(BlockOutFrom::Screencast)
        );
        assert_eq!(
            BlockOutFrom::parse(" screen-capture "),
            Some(BlockOutFrom::ScreenCapture)
        );
        assert_eq!(
            BlockOutFrom::parse("SCREEN-CAPTURE"),
            Some(BlockOutFrom::ScreenCapture)
        );
        for bad in ["", "screen", "none", "screencasts"] {
            assert_eq!(BlockOutFrom::parse(bad), None, "{bad}");
        }
        assert_eq!(BlockOutFrom::Screencast.as_str(), "screencast");
        assert_eq!(BlockOutFrom::ScreenCapture.as_str(), "screen-capture");
    }

    #[test]
    fn every_overlay_answers_to_its_own_class_and_name() {
        for overlay in Overlay::ALL {
            assert_eq!(
                Overlay::from_class_name(overlay.class_name()),
                Some(overlay)
            );
            assert!(!overlay.name().is_empty());
            // The class names really are the ones the overlays create (see the modules
            // that own them); a mismatch would silently never apply the rule.
            assert!(overlay.class_name().starts_with("Wiri"));
        }
        assert_eq!(Overlay::from_class_name("Notepad"), None);
        assert_eq!(Overlay::from_class_name(""), None);
        let ctx = Overlay::WorkspaceIndicator.matcher_context();
        assert_eq!(ctx.class_name, OVERLAY_CLASS);
        assert_eq!(ctx.title, "workspace-indicator");
    }

    #[test]
    fn a_rule_matching_the_overlay_identity_blocks_it_out() {
        let layer_rules = vec![rule(
            vec![by_name("workspace-indicator")],
            Some(BlockOutFrom::Screencast),
        )];
        assert_eq!(
            resolve(&layer_rules, &[], Overlay::WorkspaceIndicator),
            Some(BlockOutFrom::Screencast)
        );
        // The same rule leaves every other overlay alone.
        assert_eq!(resolve(&layer_rules, &[], Overlay::StatusBar), None);
        // A rule on the synthetic class covers all of them.
        let all = vec![rule(
            vec![Matcher::ClassSearch(
                crate::config::types::RegexPat::new("wiri-overlay").unwrap(),
            )],
            Some(BlockOutFrom::ScreenCapture),
        )];
        assert_eq!(
            resolve(&all, &[], Overlay::ScreenshotUi),
            Some(BlockOutFrom::ScreenCapture)
        );
        assert_eq!(
            resolve(&all, &[], Overlay::Animation),
            Some(BlockOutFrom::ScreenCapture)
        );
        // An exclusion beats a match, as for every window rule.
        let excluded = WindowRule {
            exclude_groups: vec![MatchGroup {
                conds: vec![by_name("decor")],
            }],
            ..rule(Vec::new(), Some(BlockOutFrom::ScreenCapture))
        };
        assert_eq!(resolve(&[excluded], &[], Overlay::Decor), None);
        assert_eq!(
            resolve(
                &[rule(Vec::new(), Some(BlockOutFrom::ScreenCapture))],
                &[],
                Overlay::Decor
            ),
            Some(BlockOutFrom::ScreenCapture)
        );
    }

    #[test]
    fn a_window_rule_wins_over_a_layer_rule() {
        let layer_rules = vec![rule(Vec::new(), Some(BlockOutFrom::Screencast))];
        let window_rules = vec![rule(
            vec![by_name("overview")],
            Some(BlockOutFrom::ScreenCapture),
        )];
        assert_eq!(
            resolve(&layer_rules, &window_rules, Overlay::OverviewHost),
            Some(BlockOutFrom::ScreenCapture)
        );
        assert_eq!(
            resolve(&layer_rules, &window_rules, Overlay::StatusBar),
            Some(BlockOutFrom::Screencast)
        );
    }

    #[test]
    fn a_rule_that_does_not_block_leaves_the_window_alone() {
        let rules = vec![rule(
            vec![by_name("workspace-indicator")],
            None, // e.g. `opacity 0.5` only
        )];
        assert_eq!(resolve(&rules, &[], Overlay::WorkspaceIndicator), None);
    }
}
