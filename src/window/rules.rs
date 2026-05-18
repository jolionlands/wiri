use crate::config::types::MatcherContext;
use crate::config::WindowRule;
use crate::window::ResolvedWindowRules;

/// Canonical rule resolver. Takes a `MatcherContext` that can carry both string
/// fields and boolean flags (`is_active`, `is_floating`, `is_urgent`, `at_startup`).
///
/// Rules are evaluated in order; later rules override earlier ones (last-wins).
pub fn resolve_window_rules(rules: &[WindowRule], ctx: &MatcherContext<'_>) -> ResolvedWindowRules {
    let mut resolved = ResolvedWindowRules::default();

    for rule in rules {
        if rule.matches(ctx) {
            // Apply this rule's properties to the resolved rules
            if rule.floating {
                resolved.float = true;
            }
            if let Some(ref ws) = rule.workspace {
                resolved.workspace = Some(ws.parse().ok()).flatten();
            }
            // `rule.opacity` is `Option<f64>`: `Some(x)` is an explicit
            // override (including `Some(0.0)` for "fully transparent"), `None`
            // means "field unset; fall through" so earlier matching rules
            // (or the default `None`) win.
            if let Some(o) = rule.opacity {
                resolved.opacity = Some(o.clamp(0.0, 1.0) as f32);
            }
            resolved.border = !rule.blur; // blur is used as "borderless" proxy
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

    fn ctx_with_process<'a>(class: &'a str, title: &'a str, process: &'a str) -> MatcherContext<'a> {
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
        let got = resolved.opacity.expect("opacity should be Some after matching rule");
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
        let got = resolved.opacity.expect("opacity should be Some after matching rule");
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
        assert!(!resolved_no.float, "ClassName matcher should not match different class");
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
        assert!(!resolved_no.float, "ProcessName matcher should not match different process");

        // No process_name in context → no match.
        let resolved_none = resolve_window_rules(&rules, &ctx("", ""));
        assert!(!resolved_none.float, "ProcessName matcher should not match when process_name is None");
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
        assert!(resolved_active.float, "IsActive matcher should match when is_active=true");

        let resolved_inactive = resolve_window_rules(&rules, &ctx_active("Any", "", false));
        assert!(!resolved_inactive.float, "IsActive matcher should not match when is_active=false");
    }

    /// Multi-matcher AND semantics: all matchers must pass.
    #[test]
    fn test_matcher_and_semantics() {
        let rule = WindowRule {
            matchers: vec![
                Matcher::ClassName("Firefox".to_string()),
                Matcher::IsActive,
            ],
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
        };
        assert!(resolve_window_rules(&rules, &ctx_both).float, "AND: both matchers met");

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
        };
        assert!(!resolve_window_rules(&rules, &ctx_class_only).float, "AND: only class matches → fail");

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
        };
        assert!(!resolve_window_rules(&rules, &ctx_active_only).float, "AND: only active matches → fail");
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
        assert!(resolved_yes.float, "Flat fallback: class match should float");

        let resolved_no = resolve_window_rules(&rules, &ctx("Vim", ""));
        assert!(!resolved_no.float, "Flat fallback: class mismatch should not float");
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
        assert!(!no_proc.float, "ProcessNameRegex should not match when process_name is None");
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
        assert!(!result.float, "invalid regex pattern must return false, not panic");
    }
}
