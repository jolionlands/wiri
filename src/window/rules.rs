use crate::config::types::MatcherContext;
use crate::config::WindowRule;
use crate::window::ResolvedWindowRules;

/// The workspace a rule sends a window to, as `(position, name)`: niri's
/// `open-on-workspace "web"` is a name, wiri's `open-in-workspace 2` a position, and the
/// older `workspace "2"` / `workspace "web"` is either (a number is a position).  `None`
/// when the rule does not say.
fn rule_workspace(rule: &WindowRule) -> Option<(Option<i32>, Option<String>)> {
    if let Some(name) = rule.open_on_workspace.as_deref() {
        return Some((None, Some(name.to_string())));
    }
    if let Some(number) = rule.open_in_workspace {
        return Some((Some(number), None));
    }
    let text = rule.workspace.as_deref()?.trim();
    if text.is_empty() {
        return None;
    }
    Some(match text.parse::<i32>() {
        Ok(number) => (Some(number), None),
        Err(_) => (None, Some(text.to_string())),
    })
}

/// Canonical rule resolver. Takes a `MatcherContext` that can carry both string
/// fields and boolean flags (`is_active`, `is_floating`, `is_urgent`, `at_startup`).
///
/// A window matches a rule when it matches any of the rule's `match` lines (every line is
/// the AND of its conditions) and none of its `exclude` lines (see `WindowRule::matches`).
/// Rules are evaluated in order; later rules override earlier ones (last-wins) for every
/// property that they set.
pub fn resolve_window_rules(rules: &[WindowRule], ctx: &MatcherContext<'_>) -> ResolvedWindowRules {
    let mut resolved = ResolvedWindowRules::default();

    for rule in rules {
        if rule.matches(ctx) {
            // Apply only specified appearance keys; never reset a block to defaults.
            resolved.effects.merge(&rule.effects);
            // Apply this rule's properties to the resolved rules
            if rule.floating {
                resolved.float = true;
            }
            if let Some((number, name)) = rule_workspace(rule) {
                resolved.open_in_workspace = number;
                resolved.open_on_workspace = name;
            }
            // wiri's older pixel sizes first: a niri `default-*` block of the same rule wins
            // over them, and a later rule over an earlier one whichever kind it uses.
            if let Some(px) = rule.default_width {
                resolved.default_width_px = Some(px);
                resolved.default_column_width = None;
            }
            if let Some(px) = rule.default_height {
                resolved.default_height_px = Some(px);
                resolved.default_window_height = None;
            }
            if let Some(size) = rule.default_column_width {
                resolved.default_column_width = Some(size);
                resolved.default_width_px = None;
            }
            if let Some(size) = rule.default_window_height {
                resolved.default_window_height = Some(size);
                resolved.default_height_px = None;
            }
            if rule.min_width.is_some() {
                resolved.min_width = rule.min_width;
            }
            if rule.max_width.is_some() {
                resolved.max_width = rule.max_width;
            }
            if rule.min_height.is_some() {
                resolved.min_height = rule.min_height;
            }
            if rule.max_height.is_some() {
                resolved.max_height = rule.max_height;
            }
            if rule.open_maximized.is_some() {
                resolved.open_maximized = rule.open_maximized;
            }
            if rule.open_maximized_to_edges.is_some() {
                resolved.open_maximized_to_edges = rule.open_maximized_to_edges;
            }
            if rule.open_focused.is_some() {
                resolved.open_focused = rule.open_focused;
            }
            // `rule.opacity` is `Option<f64>`: `Some(x)` is an explicit
            // override (including `Some(0.0)` for "fully transparent"), `None`
            // means "field unset; fall through" so earlier matching rules
            // (or the default `None`) win.
            if let Some(o) = rule.opacity {
                resolved.opacity = Some(o.clamp(0.0, 1.0) as f32);
            }
            // niri-parity blur backdrop: surface the rule's `blur` field on
            // the resolved struct so the engine can call
            // `DwmEnableBlurBehindWindow` once per HWND.  Last-rule-wins: any
            // matching rule with `blur true` flips it on; matching rules
            // that don't set blur leave the prior decision intact.
            if rule.blur {
                resolved.blur = true;
            }

            // ── niri-parity `open-*` placement hints — last-rule-wins ──────
            // Only overwrite on Some(_) so a later rule that omits a field
            // doesn't clobber an earlier rule's explicit value.
            if let Some(ref out) = rule.open_on_output {
                resolved.open_on_output = Some(out.clone());
            }
            if let Some(fs) = rule.open_fullscreen {
                resolved.open_fullscreen = Some(fs);
            }
            if let Some(fl) = rule.open_floating {
                resolved.open_floating = Some(fl);
                // An explicit `open-floating` says it either way: `true` also sets `float`
                // (engines that read only `.float` honour it), and `false` takes back a
                // `float true` of an earlier rule.
                resolved.float = fl;
            }
            if let Some(bounds) = rule.open_max_bounds {
                resolved.open_max_bounds = Some(bounds);
            }
        }
    }

    resolved
}

/// Backwards-compatible shim for callers outside the owned files that use the
/// old `(rules, class, title, instance, pid)` signature.
///
/// Constructs a `MatcherContext` with all boolean flags defaulting to `false`,
/// then delegates to `resolve_window_rules`.
///
/// `layout/engine.rs` already builds a full `MatcherContext` with real
/// `is_active`/`is_floating`/`is_urgent`/`at_startup` flags — this shim is
/// retained for IPC handlers and other callers that don't track them.
pub fn resolve_window_rules_legacy(
    rules: &[WindowRule],
    class_name: Option<&str>,
    title: Option<&str>,
    instance: Option<&str>,
    _pid: u32,
) -> ResolvedWindowRules {
    let ctx = MatcherContext::from_legacy(
        class_name.unwrap_or(""),
        title.unwrap_or(""),
        instance,
        None,
    );
    resolve_window_rules(rules, &ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Matcher, MatcherContext};
    use crate::config::WindowRule;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn ctx<'a>(class: &'a str, title: &'a str) -> MatcherContext<'a> {
        MatcherContext::from_legacy(class, title, None, None)
    }

    fn ctx_with_process<'a>(
        class: &'a str,
        title: &'a str,
        process: &'a str,
    ) -> MatcherContext<'a> {
        MatcherContext::from_legacy(class, title, None, Some(process))
    }

    fn ctx_active<'a>(class: &'a str, title: &'a str, is_active: bool) -> MatcherContext<'a> {
        MatcherContext {
            class_name: class,
            title,
            instance: None,
            process_name: None,
            is_active,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid: None,
            is_focused: is_active,
            is_active_in_column: false,
        }
    }

    // ── pre-existing tests (updated to new API) ───────────────────────────────

    #[test]
    fn test_no_rules() {
        let rules = vec![];
        let resolved = resolve_window_rules(&rules, &ctx("Chrome", "Google"));
        assert!(!resolved.float);
    }

    #[test]
    fn test_matching_class() {
        let mut rule = WindowRule::default();
        rule.class = Some("Chrome".to_string());
        rule.floating = true;

        let rules = vec![rule];
        let resolved = resolve_window_rules(&rules, &ctx("Chrome", "Google"));
        assert!(resolved.float);
    }

    #[test]
    fn test_non_matching_class() {
        let mut rule = WindowRule::default();
        rule.class = Some("Firefox".to_string());
        rule.floating = true;

        let rules = vec![rule];
        let resolved = resolve_window_rules(&rules, &ctx("Chrome", "Google"));
        assert!(!resolved.float);
    }

    #[test]
    fn test_opacity_rule() {
        let mut rule = WindowRule::default();
        rule.class = Some("Terminal".to_string());
        rule.opacity = Some(0.9);

        let rules = vec![rule];
        let resolved = resolve_window_rules(&rules, &ctx("Terminal", ""));
        let got = resolved
            .opacity
            .expect("opacity should be Some after matching rule");
        assert!((got - 0.9).abs() < 0.01);
    }

    /// Item 1: explicit `Some(0.0)` produces a fully-transparent window.
    /// Previously the `> 0.0` filter dropped this case silently.
    #[test]
    fn test_opacity_zero_is_applied() {
        let mut rule = WindowRule::default();
        rule.class = Some("Ghost".to_string());
        rule.opacity = Some(0.0);

        let rules = vec![rule];
        let resolved = resolve_window_rules(&rules, &ctx("Ghost", ""));
        assert_eq!(
            resolved.opacity,
            Some(0.0),
            "opacity:0 must be honoured as fully transparent"
        );
    }

    /// `None` opacity means "field unset; leave default".
    #[test]
    fn test_opacity_none_falls_through() {
        let mut rule = WindowRule::default();
        rule.class = Some("Other".to_string());
        // opacity left as None
        let rules = vec![rule];
        let resolved = resolve_window_rules(&rules, &ctx("Other", ""));
        assert!(resolved.opacity.is_none(), "unset opacity must stay None");
    }

    /// Out-of-range opacity is clamped to [0.0, 1.0] rather than rejected.
    #[test]
    fn test_opacity_clamped_to_range() {
        let mut rule = WindowRule::default();
        rule.class = Some("Bright".to_string());
        rule.opacity = Some(2.5);
        let rules = vec![rule];
        let resolved = resolve_window_rules(&rules, &ctx("Bright", ""));
        assert_eq!(resolved.opacity, Some(1.0), "opacity>1 must clamp to 1.0");

        let mut rule = WindowRule::default();
        rule.class = Some("Dark".to_string());
        rule.opacity = Some(-0.5);
        let rules = vec![rule];
        let resolved = resolve_window_rules(&rules, &ctx("Dark", ""));
        assert_eq!(resolved.opacity, Some(0.0), "opacity<0 must clamp to 0.0");
    }

    #[test]
    fn test_multiple_rules_override() {
        let mut rule1 = WindowRule::default();
        rule1.class = Some("Chrome".to_string());
        rule1.floating = true;
        rule1.opacity = Some(0.8);

        let mut rule2 = WindowRule::default();
        rule2.class = Some("Chrome".to_string());
        rule2.opacity = Some(0.95);

        let rules = vec![rule1, rule2];
        let resolved = resolve_window_rules(&rules, &ctx("Chrome", ""));
        assert!(resolved.float); // from rule1
        let got = resolved
            .opacity
            .expect("opacity should be Some after matching rule");
        assert!((got - 0.95).abs() < 0.01); // overridden by rule2
    }

    // ── new tests ─────────────────────────────────────────────────────────────

    /// Match by `Matcher::ClassName`.
    #[test]
    fn test_matcher_class_name() {
        let rule = WindowRule {
            matchers: vec![Matcher::ClassName("Alacritty".to_string())],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let resolved_yes = resolve_window_rules(&rules, &ctx("Alacritty", ""));
        assert!(resolved_yes.float, "ClassName matcher should match");

        let resolved_no = resolve_window_rules(&rules, &ctx("Notepad", ""));
        assert!(
            !resolved_no.float,
            "ClassName matcher should not match different class"
        );
    }

    /// Match by `Matcher::ProcessName`.
    #[test]
    fn test_matcher_process_name() {
        let rule = WindowRule {
            matchers: vec![Matcher::ProcessName("notepad.exe".to_string())],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let resolved_yes = resolve_window_rules(&rules, &ctx_with_process("", "", "notepad.exe"));
        assert!(resolved_yes.float, "ProcessName matcher should match");

        let resolved_no = resolve_window_rules(&rules, &ctx_with_process("", "", "explorer.exe"));
        assert!(
            !resolved_no.float,
            "ProcessName matcher should not match different process"
        );

        // No process_name in context → no match.
        let resolved_none = resolve_window_rules(&rules, &ctx("", ""));
        assert!(
            !resolved_none.float,
            "ProcessName matcher should not match when process_name is None"
        );
    }

    /// Match by `Matcher::IsActive` — true and false cases.
    #[test]
    fn test_matcher_is_active_true_false() {
        let rule = WindowRule {
            matchers: vec![Matcher::IsActive],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let resolved_active = resolve_window_rules(&rules, &ctx_active("Any", "", true));
        assert!(
            resolved_active.float,
            "IsActive matcher should match when is_active=true"
        );

        let resolved_inactive = resolve_window_rules(&rules, &ctx_active("Any", "", false));
        assert!(
            !resolved_inactive.float,
            "IsActive matcher should not match when is_active=false"
        );
    }

    /// Multi-matcher AND semantics: all matchers must pass.
    #[test]
    fn test_matcher_and_semantics() {
        let rule = WindowRule {
            matchers: vec![Matcher::ClassName("Firefox".to_string()), Matcher::IsActive],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        // Both conditions met → match.
        let ctx_both = MatcherContext {
            class_name: "Firefox",
            title: "",
            instance: None,
            process_name: None,
            is_active: true,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid: None,
            is_focused: true,
            is_active_in_column: false,
        };
        assert!(
            resolve_window_rules(&rules, &ctx_both).float,
            "AND: both matchers met"
        );

        // Only class matches, not active → no match.
        let ctx_class_only = MatcherContext {
            class_name: "Firefox",
            title: "",
            instance: None,
            process_name: None,
            is_active: false,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid: None,
            is_focused: false,
            is_active_in_column: false,
        };
        assert!(
            !resolve_window_rules(&rules, &ctx_class_only).float,
            "AND: only class matches → fail"
        );

        // Only active, wrong class → no match.
        let ctx_active_only = MatcherContext {
            class_name: "Chrome",
            title: "",
            instance: None,
            process_name: None,
            is_active: true,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid: None,
            is_focused: true,
            is_active_in_column: false,
        };
        assert!(
            !resolve_window_rules(&rules, &ctx_active_only).float,
            "AND: only active matches → fail"
        );
    }

    /// Empty `matchers` vec falls back to flat-field matching.
    #[test]
    fn test_matcher_empty_falls_back_to_flat() {
        // Rule using flat field (no matchers vec).
        let rule = WindowRule {
            matchers: vec![],
            class: Some("Emacs".to_string()),
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let resolved_yes = resolve_window_rules(&rules, &ctx("Emacs", ""));
        assert!(
            resolved_yes.float,
            "Flat fallback: class match should float"
        );

        let resolved_no = resolve_window_rules(&rules, &ctx("Vim", ""));
        assert!(
            !resolved_no.float,
            "Flat fallback: class mismatch should not float"
        );
    }

    // ── regex matcher tests ───────────────────────────────────────────────────

    /// `Matcher::ClassNameRegex` — pattern `^Term.*` matches "Terminal" and "TermX"
    /// but not "MyTerm".
    #[test]
    fn test_matcher_class_name_regex() {
        let rule = WindowRule {
            matchers: vec![Matcher::ClassNameRegex("^Term.*".to_string())],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let yes_terminal = resolve_window_rules(&rules, &ctx("Terminal", ""));
        assert!(yes_terminal.float, "^Term.* should match \"Terminal\"");

        let yes_termx = resolve_window_rules(&rules, &ctx("TermX", ""));
        assert!(yes_termx.float, "^Term.* should match \"TermX\"");

        let no_myterm = resolve_window_rules(&rules, &ctx("MyTerm", ""));
        assert!(!no_myterm.float, "^Term.* should NOT match \"MyTerm\"");
    }

    /// `Matcher::ProcessNameRegex` — case-insensitive pattern `(?i)firefox` matches
    /// "Firefox.exe".
    #[test]
    fn test_matcher_process_name_regex() {
        let rule = WindowRule {
            matchers: vec![Matcher::ProcessNameRegex("(?i)firefox".to_string())],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let yes = resolve_window_rules(&rules, &ctx_with_process("", "", "Firefox.exe"));
        assert!(yes.float, "(?i)firefox should match \"Firefox.exe\"");

        let no = resolve_window_rules(&rules, &ctx_with_process("", "", "chrome.exe"));
        assert!(!no.float, "(?i)firefox should NOT match \"chrome.exe\"");

        // No process_name in context — unwrap_or("") produces empty string, no match.
        let no_proc = resolve_window_rules(&rules, &ctx("", ""));
        assert!(
            !no_proc.float,
            "ProcessNameRegex should not match when process_name is None"
        );
    }

    // ── live-state matcher tests ──────────────────────────────────────────────
    //
    // These prove the engine-populated boolean flags (`is_active`, `is_floating`,
    // `is_urgent`, `at_startup`) drive `Matcher` evaluation correctly when the
    // caller passes real state instead of the placeholder default.

    /// Helper that returns a `MatcherContext` with explicit live-state flags.
    fn ctx_flags<'a>(
        class: &'a str,
        title: &'a str,
        is_active: bool,
        is_floating: bool,
        is_urgent: bool,
        at_startup: bool,
    ) -> MatcherContext<'a> {
        MatcherContext {
            class_name: class,
            title,
            instance: None,
            process_name: None,
            is_active,
            is_floating,
            is_urgent,
            at_startup,
            aumid: None,
            is_focused: is_active,
            is_active_in_column: false,
        }
    }

    /// `Matcher::IsFloating` fires only when `ctx.is_floating == true`.
    #[test]
    fn test_matcher_is_floating_fires_only_when_floating() {
        let rule = WindowRule {
            matchers: vec![Matcher::IsFloating],
            opacity: Some(0.7),
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let floating = ctx_flags("Any", "", false, true, false, false);
        let tiled = ctx_flags("Any", "", false, false, false, false);

        assert_eq!(
            resolve_window_rules(&rules, &floating).opacity,
            Some(0.7),
            "IsFloating rule must apply when window is floating"
        );
        assert!(
            resolve_window_rules(&rules, &tiled).opacity.is_none(),
            "IsFloating rule must not apply when window is tiled"
        );
    }

    /// `Matcher::IsUrgent` fires only when `ctx.is_urgent == true`.
    #[test]
    fn test_matcher_is_urgent_fires_only_when_urgent() {
        let rule = WindowRule {
            matchers: vec![Matcher::IsUrgent],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let urgent = ctx_flags("Any", "", false, false, true, false);
        let calm = ctx_flags("Any", "", false, false, false, false);

        assert!(
            resolve_window_rules(&rules, &urgent).float,
            "IsUrgent rule must fire when window is flashing"
        );
        assert!(
            !resolve_window_rules(&rules, &calm).float,
            "IsUrgent rule must not fire on calm windows"
        );
    }

    /// `Matcher::AtStartup` fires only on the initial startup scan.
    #[test]
    fn test_matcher_at_startup_distinguishes_runtime_from_startup() {
        let rule = WindowRule {
            matchers: vec![Matcher::AtStartup],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let startup = ctx_flags("Any", "", false, false, false, true);
        let runtime = ctx_flags("Any", "", false, false, false, false);

        assert!(
            resolve_window_rules(&rules, &startup).float,
            "AtStartup rule must fire during initial scan"
        );
        assert!(
            !resolve_window_rules(&rules, &runtime).float,
            "AtStartup rule must NOT fire for windows opened after startup"
        );
    }

    /// Composite matcher: class + is_active. Both must hold.
    #[test]
    fn test_matcher_composite_class_and_is_floating() {
        let rule = WindowRule {
            matchers: vec![
                Matcher::ClassName("Notepad".to_string()),
                Matcher::IsFloating,
            ],
            opacity: Some(0.5),
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let class_and_floating = ctx_flags("Notepad", "", false, true, false, false);
        let class_only = ctx_flags("Notepad", "", false, false, false, false);
        let floating_only = ctx_flags("Chrome", "", false, true, false, false);

        assert_eq!(
            resolve_window_rules(&rules, &class_and_floating).opacity,
            Some(0.5),
            "Composite: both class and floating must hold"
        );
        assert!(
            resolve_window_rules(&rules, &class_only).opacity.is_none(),
            "Composite: class without floating must NOT match"
        );
        assert!(
            resolve_window_rules(&rules, &floating_only)
                .opacity
                .is_none(),
            "Composite: floating without class must NOT match"
        );
    }

    // ── niri-parity `open-*` placement rule tests ────────────────────────────

    /// `open-on-output` is folded into the resolved rules when the matcher
    /// fires; left None otherwise.
    #[test]
    fn test_resolved_open_on_output() {
        let rule = WindowRule {
            class: Some("Firefox".to_string()),
            open_on_output: Some("HDMI-1".to_string()),
            ..WindowRule::default()
        };
        let rules = vec![rule];

        let yes = resolve_window_rules(&rules, &ctx("Firefox", ""));
        assert_eq!(yes.open_on_output.as_deref(), Some("HDMI-1"));

        let no = resolve_window_rules(&rules, &ctx("Notepad", ""));
        assert!(no.open_on_output.is_none());
    }

    /// `open-in-workspace` and `open-fullscreen` round-trip.
    #[test]
    fn test_resolved_open_in_workspace_and_fullscreen() {
        let rule = WindowRule {
            class: Some("Steam".to_string()),
            open_in_workspace: Some(3),
            open_fullscreen: Some(true),
            ..WindowRule::default()
        };
        let rules = vec![rule];
        let r = resolve_window_rules(&rules, &ctx("Steam", ""));
        assert_eq!(r.open_in_workspace, Some(3));
        assert_eq!(r.open_fullscreen, Some(true));
    }

    /// `open-floating true` implies `float = true` (so engines that look only
    /// at `.float` honour the user's intent without re-reading `open_floating`).
    #[test]
    fn test_resolved_open_floating_implies_float() {
        let rule = WindowRule {
            class: Some("Calculator".to_string()),
            open_floating: Some(true),
            ..WindowRule::default()
        };
        let rules = vec![rule];
        let r = resolve_window_rules(&rules, &ctx("Calculator", ""));
        assert_eq!(r.open_floating, Some(true));
        assert!(
            r.float,
            "open-floating true must also set the legacy .float flag"
        );
    }

    /// `open-max-bounds` is preserved on the resolved struct (engine wiring
    /// in `add_window` will cap the initial bounds — owned by Agent E).
    #[test]
    fn test_resolved_open_max_bounds() {
        let rule = WindowRule {
            class: Some("BigApp".to_string()),
            open_max_bounds: Some((1600, 900)),
            ..WindowRule::default()
        };
        let rules = vec![rule];
        let r = resolve_window_rules(&rules, &ctx("BigApp", ""));
        assert_eq!(r.open_max_bounds, Some((1600, 900)));
    }

    /// Later rules override earlier ones for the open-* fields (last-wins),
    /// but rules that don't set a field do not clobber earlier explicit values.
    #[test]
    fn test_resolved_open_fields_last_wins_but_unset_preserves() {
        let rule1 = WindowRule {
            class: Some("App".to_string()),
            open_on_output: Some("DP-1".to_string()),
            open_in_workspace: Some(2),
            ..WindowRule::default()
        };
        let rule2 = WindowRule {
            class: Some("App".to_string()),
            open_on_output: Some("HDMI-1".to_string()),
            // open_in_workspace deliberately unset
            ..WindowRule::default()
        };
        let rules = vec![rule1, rule2];
        let r = resolve_window_rules(&rules, &ctx("App", ""));
        assert_eq!(
            r.open_on_output.as_deref(),
            Some("HDMI-1"),
            "last-rule-wins"
        );
        assert_eq!(
            r.open_in_workspace,
            Some(2),
            "unset field on later rule preserves earlier"
        );
    }

    /// An invalid regex pattern must not panic; the matcher returns `false`.
    #[test]
    fn test_matcher_invalid_regex_does_not_panic() {
        let rule = WindowRule {
            matchers: vec![Matcher::ClassNameRegex("[invalid".to_string())],
            floating: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];

        // Must return without panicking and must NOT match.
        let result = resolve_window_rules(&rules, &ctx("anything", ""));
        assert!(
            !result.float,
            "invalid regex pattern must return false, not panic"
        );
    }

    // ── niri window rules: match lines, exclude lines, properties ────────────

    use crate::config::types::{MatchGroup, RegexPat, SizeSpec};

    fn pat(source: &str) -> RegexPat {
        RegexPat::new(source).expect("a valid regex")
    }

    fn group(conds: Vec<Matcher>) -> MatchGroup {
        MatchGroup { conds }
    }

    /// A window with the facts rules can look at.
    fn facts<'a>(
        title: &'a str,
        process: Option<&'a str>,
        aumid: Option<&'a str>,
        focused: bool,
    ) -> MatcherContext<'a> {
        MatcherContext {
            class_name: "TestClass",
            title,
            instance: None,
            process_name: process,
            is_active: focused,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid,
            is_focused: focused,
            is_active_in_column: false,
        }
    }

    #[test]
    fn several_match_lines_are_ored_and_an_exclude_line_vetoes() {
        let rule = WindowRule {
            match_groups: vec![
                group(vec![Matcher::AppId(pat("^firefox$"))]),
                group(vec![
                    Matcher::AppId(pat("^chrome$")),
                    Matcher::TitleSearch(pat("Picture")),
                ]),
            ],
            exclude_groups: vec![group(vec![Matcher::TitleSearch(pat("Private"))])],
            open_floating: Some(true),
            ..WindowRule::default()
        };
        let rules = vec![rule];
        let floats = |title: &str, process: &str| {
            resolve_window_rules(&rules, &facts(title, Some(process), None, false)).float
        };
        assert!(floats("Mozilla", "firefox.exe"), "the first match line");
        assert!(
            floats("Picture-in-Picture", "chrome.exe"),
            "the second one: both its conditions"
        );
        assert!(
            !floats("Docs", "chrome.exe"),
            "the second line needs its title as well"
        );
        assert!(
            !floats("Private Browsing", "firefox.exe"),
            "an exclude line vetoes"
        );
        assert!(!floats("Mozilla", "notepad.exe"), "no line matches");
    }

    #[test]
    fn a_rule_without_match_lines_matches_every_window_unless_excluded() {
        let rule = WindowRule {
            exclude_groups: vec![group(vec![Matcher::AppId(pat("^code$"))])],
            opacity: Some(0.8),
            ..WindowRule::default()
        };
        let rules = vec![rule];
        let opacity = |process: &str| {
            resolve_window_rules(&rules, &facts("t", Some(process), None, false)).opacity
        };
        assert_eq!(opacity("notepad.exe"), Some(0.8));
        assert_eq!(
            opacity("code.exe"),
            None,
            "the name without .exe is excluded"
        );
        assert_eq!(
            opacity("Code.exe"),
            Some(0.8),
            "a regex is case sensitive, like niri's"
        );
    }

    #[test]
    fn app_id_is_searched_in_the_process_file_name_its_stem_and_the_aumid() {
        let rules = vec![WindowRule {
            match_groups: vec![group(vec![Matcher::AppId(pat("Terminal"))])],
            floating: true,
            ..WindowRule::default()
        }];
        let floats = |process: Option<&str>, aumid: Option<&str>| {
            resolve_window_rules(&rules, &facts("t", process, aumid, false)).float
        };
        assert!(
            floats(Some("WindowsTerminal.exe"), None),
            "anywhere in the file name"
        );
        assert!(
            floats(
                Some("explorer.exe"),
                Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App")
            ),
            "the AppUserModelID of a packaged app"
        );
        assert!(!floats(Some("explorer.exe"), None));
        assert!(
            !floats(None, None),
            "a window whose process is not known has no app-id"
        );

        // The anchors see the name without `.exe` as well as the file name.
        let anchored = vec![WindowRule {
            match_groups: vec![group(vec![Matcher::AppId(pat("^firefox$"))])],
            floating: true,
            ..WindowRule::default()
        }];
        assert!(
            resolve_window_rules(&anchored, &facts("t", Some("firefox.exe"), None, false)).float
        );
        let by_file_name = vec![WindowRule {
            match_groups: vec![group(vec![Matcher::AppId(pat(r"^firefox\.exe$"))])],
            floating: true,
            ..WindowRule::default()
        }];
        assert!(
            resolve_window_rules(&by_file_name, &facts("t", Some("firefox.exe"), None, false))
                .float
        );
    }

    #[test]
    fn the_older_process_name_matchers_know_the_file_name_and_the_stem() {
        let by = |m: Matcher| {
            let rules = vec![WindowRule {
                matchers: vec![m],
                floating: true,
                ..WindowRule::default()
            }];
            resolve_window_rules(&rules, &ctx_with_process("c", "t", "Notepad.exe")).float
        };
        assert!(by(Matcher::ProcessName("notepad.exe".to_string())));
        assert!(
            by(Matcher::ProcessName("NOTEPAD".to_string())),
            "a stem, any case"
        );
        assert!(!by(Matcher::ProcessName("note".to_string())));
        assert!(by(Matcher::ProcessNameRegex("^Notepad$".to_string())));
        assert!(by(Matcher::ProcessSearch(pat(r"pad\.exe$"))));
    }

    #[test]
    fn a_title_regex_reads_the_current_title() {
        let rules = vec![WindowRule {
            match_groups: vec![group(vec![Matcher::TitleSearch(pat(r"^\d+ unread"))])],
            opacity: Some(0.5),
            ..WindowRule::default()
        }];
        let opacity = |title: &str| {
            resolve_window_rules(&rules, &facts(title, Some("mail.exe"), None, false)).opacity
        };
        assert_eq!(opacity("3 unread - Mail"), Some(0.5));
        assert_eq!(opacity("Mail"), None, "the same window under another title");
        assert!(rules[0].depends_on_title());
        assert!(!rules[0].depends_on_focus());
    }

    #[test]
    fn is_focused_false_is_a_condition_of_its_own() {
        // The usual "dim what has not the focus" rule.
        let rules = vec![WindowRule {
            match_groups: vec![group(vec![Matcher::Not(Box::new(Matcher::IsFocused))])],
            opacity: Some(0.7),
            ..WindowRule::default()
        }];
        let at = |focused: bool| {
            resolve_window_rules(&rules, &facts("t", Some("a.exe"), None, focused)).opacity
        };
        assert_eq!(at(false), Some(0.7));
        assert_eq!(at(true), None);
        assert!(rules[0].depends_on_focus());
        assert!(rules[0].has_dynamic_conditions());
    }

    #[test]
    fn a_later_rule_wins_for_a_property_only_when_it_sets_it() {
        let first = WindowRule {
            match_groups: vec![group(vec![Matcher::AppId(pat("^a$"))])],
            opacity: Some(0.9),
            min_width: Some(300),
            max_width: Some(900),
            open_focused: Some(false),
            ..WindowRule::default()
        };
        let second = WindowRule {
            match_groups: vec![group(vec![Matcher::AppId(pat("^a$"))])],
            max_width: Some(700),
            min_height: Some(200),
            open_focused: Some(true),
            ..WindowRule::default()
        };
        let r = resolve_window_rules(&[first, second], &facts("t", Some("a.exe"), None, false));
        assert_eq!(
            r.opacity,
            Some(0.9),
            "the first rule's, the second sets none"
        );
        assert_eq!(r.min_width, Some(300));
        assert_eq!(r.max_width, Some(700), "the second rule's");
        assert_eq!(r.min_height, Some(200));
        assert_eq!(r.max_height, None);
        assert_eq!(r.open_focused, Some(true));
    }

    #[test]
    fn open_floating_false_takes_back_a_float_of_an_earlier_rule() {
        let rules = vec![
            WindowRule {
                floating: true,
                ..WindowRule::default()
            },
            WindowRule {
                open_floating: Some(false),
                ..WindowRule::default()
            },
        ];
        let r = resolve_window_rules(&rules, &ctx("c", "t"));
        assert!(!r.float);
        assert_eq!(r.open_floating, Some(false));
    }

    #[test]
    fn a_workspace_is_a_name_a_position_or_the_older_key() {
        let one = |rule: WindowRule| {
            let r = resolve_window_rules(&[rule], &ctx("c", "t"));
            (r.open_in_workspace, r.open_on_workspace)
        };
        assert_eq!(
            one(WindowRule {
                open_on_workspace: Some("web".into()),
                ..WindowRule::default()
            }),
            (None, Some("web".to_string()))
        );
        assert_eq!(
            one(WindowRule {
                open_in_workspace: Some(3),
                ..WindowRule::default()
            }),
            (Some(3), None)
        );
        assert_eq!(
            one(WindowRule {
                workspace: Some("2".into()),
                ..WindowRule::default()
            }),
            (Some(2), None),
            "a number is a position"
        );
        assert_eq!(
            one(WindowRule {
                workspace: Some("chat".into()),
                ..WindowRule::default()
            }),
            (None, Some("chat".to_string())),
            "anything else is a name"
        );
        // A later rule replaces the workspace of an earlier one, whichever kind each names.
        let r = resolve_window_rules(
            &[
                WindowRule {
                    open_in_workspace: Some(2),
                    ..WindowRule::default()
                },
                WindowRule {
                    open_on_workspace: Some("web".into()),
                    ..WindowRule::default()
                },
            ],
            &ctx("c", "t"),
        );
        assert_eq!(
            (r.open_in_workspace, r.open_on_workspace.as_deref()),
            (None, Some("web"))
        );
    }

    #[test]
    fn the_two_kinds_of_default_size_replace_each_other() {
        let niri = WindowRule {
            default_column_width: Some(Some(SizeSpec::Proportion(0.5))),
            default_window_height: Some(None),
            ..WindowRule::default()
        };
        let older = WindowRule {
            default_width: Some(640),
            default_height: Some(480),
            ..WindowRule::default()
        };
        let r = resolve_window_rules(&[niri.clone(), older.clone()], &ctx("c", "t"));
        assert_eq!(
            (r.default_width_px, r.default_column_width),
            (Some(640), None)
        );
        assert_eq!(
            (r.default_height_px, r.default_window_height),
            (Some(480), None)
        );
        let r = resolve_window_rules(&[older, niri], &ctx("c", "t"));
        assert_eq!(r.default_width_px, None);
        assert_eq!(
            r.default_column_width,
            Some(Some(SizeSpec::Proportion(0.5)))
        );
        assert_eq!(
            r.default_window_height,
            Some(None),
            "the empty block: the window's own height"
        );
    }
}
