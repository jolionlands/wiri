//! What a config reload re-applies, in niri's order.
//!
//! niri's `reload_config` (niri e19df12 `src/niri.rs:1683-1967`) re-applies only the sections
//! whose value changed, in a fixed order: named workspaces are unnamed/created around the
//! layout update, then the animation clock and the child environment, then the cursor and the
//! input devices, the keyboard repeat, the outputs, the binds and the hotkey overlay, the
//! window rules, and finally a redraw.  `spawn-at-startup` is never re-run.
//!
//! wiri publishes the whole new [`Config`] to the engine on every successful reload (the
//! engine reads binds, effects and recent-window options from it), so the plan below gates the
//! *side effects*: OS pointer settings, hotkey registration, output geometry, layout
//! re-placement and rule re-evaluation.  A reload that changes only a bind therefore does not
//! re-place every window.
//!
//! The comparison is [`std::fmt::Debug`]-based because [`Config`] and its sub-structs do not
//! implement `PartialEq` (adding it would touch every config type, which other lanes own).
//! Reloads are user actions, not a hot path, and the fingerprints are only taken on reload.

use super::types::Config;

/// One re-application step of a reload, in niri's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReloadStep {
    /// `workspace "x" {}` entries: remove the names that disappeared, create the new ones
    /// (niri `src/niri.rs:1701-1710`, 1717-1720).
    NamedWorkspaces,
    /// `layout {}` and the per-output `layout {}` override, plus the view rebuild
    /// (niri `src/niri.rs:1712-1715`).
    Layout,
    /// `animations {}`: slowdown, off and the per-effect curves
    /// (niri `src/niri.rs:1722-1726`).
    Animations,
    /// `environment {}`: child processes only (niri `src/niri.rs:1728`).
    Environment,
    /// `input {}` and `cursor {}`: keyboard repeat/xkb and the pointer settings
    /// (niri `src/niri.rs:1742-1764`, 1893-1902).
    Input,
    /// `output {}` entries: scale, position, layout override
    /// (niri `src/niri.rs:1779-1787`, 1908-1910).
    Outputs,
    /// `binds {}` and `gestures {}` (hotkeys, hot corners, drag-and-drop edges)
    /// (niri `src/niri.rs:1789-1809`, 1930-1932).
    Binds,
    /// `window-rule {}` and `layer-rule {}`: re-evaluated on the open windows
    /// (niri `src/niri.rs:1912-1918`).
    WindowRules,
    /// `switch-events {}` subscriptions.
    SwitchEvents,
    /// `screenshot-path`.
    Screenshot,
}

/// The ordered list of steps a given reload has to perform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReloadPlan {
    steps: Vec<ReloadStep>,
}

impl ReloadPlan {
    /// The steps, in niri's reload order.
    pub fn steps(&self) -> &[ReloadStep] {
        &self.steps
    }

    /// Whether this reload re-applies `step`.
    pub fn contains(&self, step: ReloadStep) -> bool {
        self.steps.contains(&step)
    }

    /// Whether anything the engine has to publish changed: the layout views, the motion
    /// configuration, the window rules or the named workspaces.  A reload that only changes
    /// binds still has to publish the new config to the engine (that is where `binds` is read
    /// from), but it needs neither a re-placement nor a rule re-evaluation.
    pub fn touches_engine(&self) -> bool {
        self.steps.iter().any(|step| {
            matches!(
                step,
                ReloadStep::NamedWorkspaces
                    | ReloadStep::Layout
                    | ReloadStep::Animations
                    | ReloadStep::WindowRules
            )
        })
    }

    /// Whether open windows must follow new window/layer rules.
    pub fn reevaluates_rules(&self) -> bool {
        self.contains(ReloadStep::WindowRules)
    }
}

/// A value fingerprint.  See the module comment for why this is not `PartialEq`.
fn same<T: std::fmt::Debug>(a: &T, b: &T) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

/// Plan a reload from the config the engine holds (`old`, `None` before the first load) to the
/// config that was just decoded.  An absent `old` plans every step, which is what a first
/// application needs.
pub fn plan(old: Option<&Config>, new: &Config) -> ReloadPlan {
    let old = match old {
        Some(old) => old,
        None => {
            return ReloadPlan {
                steps: ReloadStep::ALL.to_vec(),
            }
        }
    };

    let mut steps = Vec::new();
    let mut push = |step: ReloadStep| steps.push(step);

    // niri's order.  `NamedWorkspaces` covers the unname/new pair around `Layout`.
    if !same(&old.workspace, &new.workspace) {
        push(ReloadStep::NamedWorkspaces);
    }
    if !same(&old.layout, &new.layout) || !same(&old.output, &new.output) {
        push(ReloadStep::Layout);
    }
    if !same(&old.animations, &new.animations) {
        push(ReloadStep::Animations);
    }
    if !same(&old.environment, &new.environment) {
        push(ReloadStep::Environment);
    }
    if !same(&old.input, &new.input) || !same(&old.cursor, &new.cursor) {
        push(ReloadStep::Input);
    }
    if !same(&old.output, &new.output) {
        push(ReloadStep::Outputs);
    }
    if !same(&old.binds, &new.binds) || !same(&old.gestures, &new.gestures) {
        push(ReloadStep::Binds);
    }
    if !same(&old.window_rules, &new.window_rules) || !same(&old.layer_rules, &new.layer_rules) {
        push(ReloadStep::WindowRules);
    }
    if !same(&old.switch_events, &new.switch_events) {
        push(ReloadStep::SwitchEvents);
    }
    if !same(&old.screenshot_path, &new.screenshot_path) {
        push(ReloadStep::Screenshot);
    }
    ReloadPlan { steps }
}

impl ReloadStep {
    /// Every step, in reload order (a first application).
    pub const ALL: &'static [ReloadStep] = &[
        ReloadStep::NamedWorkspaces,
        ReloadStep::Layout,
        ReloadStep::Animations,
        ReloadStep::Environment,
        ReloadStep::Input,
        ReloadStep::Outputs,
        ReloadStep::Binds,
        ReloadStep::WindowRules,
        ReloadStep::SwitchEvents,
        ReloadStep::Screenshot,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::decode;

    fn config(text: &str) -> Config {
        let decoded = decode::decode_text(text, "test.kdl", None);
        assert!(
            !decoded.has_errors(),
            "{text}: {:?}",
            decoded
                .issues
                .iter()
                .filter(|i| i.severity == decode::ValidationSeverity::Error)
                .map(|i| i.message.clone())
                .collect::<Vec<_>>()
        );
        decoded.config
    }

    fn steps(old: &Config, new: &Config) -> Vec<ReloadStep> {
        plan(Some(old), new).steps().to_vec()
    }

    fn only(old: &Config, new: &Config) -> Vec<ReloadStep> {
        let steps = steps(old, new);
        assert_eq!(steps.len(), 1, "expected exactly one step: {steps:?}");
        steps
    }

    /// The base config every other test changes one section of.
    const BASE: &str = r#"
        layout { gaps 16; }
        animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
        environment { FOO "1"; }
        input { keyboard { repeat-delay 600; repeat-rate 25; } }
        output "DP-1" { scale 1; }
        binds { Ctrl+Alt+F8 { screenshot; } }
        window-rule { match app-id="a"; opacity 0.9; }
        switch-events { lid-close { spawn "x"; } }
        screenshot-path "~/Pictures/a.png"
        workspace "chat" {}
    "#;

    /// Re-decoding the same text is not a reload of anything.
    #[test]
    fn an_unchanged_config_replans_nothing() {
        let old = config(BASE);
        let new = config(BASE);
        assert_eq!(plan(Some(&old), &new).steps(), &[] as &[ReloadStep]);
        assert!(!plan(Some(&old), &new).touches_engine());
        assert!(!plan(Some(&old), &new).reevaluates_rules());
    }

    /// Before the first application every section has to be applied.
    #[test]
    fn a_first_application_plans_every_step_in_niri_order() {
        let new = config(BASE);
        let plan = plan(None, &new);
        assert_eq!(plan.steps(), ReloadStep::ALL);
        assert!(plan.touches_engine());
        // niri applies the layout before the outputs, the binds before the rules and the
        // rules before the redraw (niri e19df12 src/niri.rs:1712-1936).
        let index = |step: ReloadStep| plan.steps().iter().position(|s| *s == step).unwrap();
        assert!(index(ReloadStep::NamedWorkspaces) < index(ReloadStep::Layout));
        assert!(index(ReloadStep::Layout) < index(ReloadStep::Animations));
        assert!(index(ReloadStep::Animations) < index(ReloadStep::Environment));
        assert!(index(ReloadStep::Environment) < index(ReloadStep::Input));
        assert!(index(ReloadStep::Input) < index(ReloadStep::Outputs));
        assert!(index(ReloadStep::Outputs) < index(ReloadStep::Binds));
        assert!(index(ReloadStep::Binds) < index(ReloadStep::WindowRules));
    }

    /// `layout { gaps 8; }`: the change niri re-applies as the layout update, and it must not
    /// churn binds, pointer settings or output modes.
    #[test]
    fn a_gap_change_replans_only_the_layout() {
        let old = config(BASE);
        let new = config(
            r#"
            layout { gaps 8; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(only(&old, &new), vec![ReloadStep::Layout]);
        let plan = plan(Some(&old), &new);
        assert!(plan.touches_engine());
        assert!(!plan.reevaluates_rules());
    }

    /// A new gap inside an output's `layout {}` override is both a layout and an output step.
    #[test]
    fn an_output_override_replans_layout_and_outputs() {
        let old = config(BASE);
        let new = config(
            r#"
            layout { gaps 16; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            output "DP-1" { scale 1; layout { gaps 4; } }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(
            steps(&old, &new),
            vec![ReloadStep::Layout, ReloadStep::Outputs]
        );
    }

    /// `window-rule`: the rules are re-evaluated on the windows that are already open.
    #[test]
    fn a_window_rule_change_reevaluates_the_rules() {
        let old = config(BASE);
        let new = config(
            r#"
            layout { gaps 16; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.5; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(only(&old, &new), vec![ReloadStep::WindowRules]);
        assert!(plan(Some(&old), &new).reevaluates_rules());
    }

    /// A bind change re-registers hotkeys but neither re-places windows nor touches the OS
    /// pointer settings.
    #[test]
    fn a_bind_change_replans_only_the_binds() {
        let old = config(BASE);
        let new = config(
            r#"
            layout { gaps 16; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F9 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(only(&old, &new), vec![ReloadStep::Binds]);
        assert!(!plan(Some(&old), &new).touches_engine());
    }

    /// The animation curve is one of niri's re-applied sections (its clock and curves).
    #[test]
    fn an_animation_curve_change_replans_the_animations() {
        let old = config(BASE);
        let new = config(
            r#"
            layout { gaps 16; }
            animations { window-open { duration-ms 300; curve "linear"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(only(&old, &new), vec![ReloadStep::Animations]);
        assert!(plan(Some(&old), &new).touches_engine());
    }

    /// Removing a named workspace is niri's `unname_workspace` step; keeping every other
    /// section unchanged must not add another step.
    #[test]
    fn a_removed_workspace_declaration_replans_the_names() {
        let old = config(BASE);
        let new = config(
            r#"
            layout { gaps 16; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
        "#,
        );
        assert_eq!(only(&old, &new), vec![ReloadStep::NamedWorkspaces]);
        assert!(plan(Some(&old), &new).touches_engine());
    }

    /// `input {}` and `cursor {}` are one step: the OS settings are derived from the whole
    /// block, like niri's libinput re-application.
    #[test]
    fn input_and_cursor_changes_share_one_step() {
        let old = config(BASE);
        let keyboard = config(
            r#"
            layout { gaps 16; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 300; repeat-rate 25; } }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(only(&old, &keyboard), vec![ReloadStep::Input]);
        let cursor = config(
            r#"
            layout { gaps 16; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            cursor { hide-after-inactive-ms 1000; }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(only(&old, &cursor), vec![ReloadStep::Input]);
    }

    /// `environment {}` reaches later children only (niri `src/niri.rs:1728`).
    #[test]
    fn an_environment_change_is_its_own_step() {
        let old = config(BASE);
        let new = config(
            r#"
            layout { gaps 16; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "2"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(only(&old, &new), vec![ReloadStep::Environment]);
        assert!(!plan(Some(&old), &new).touches_engine());
    }

    /// `spawn-at-startup` is not a reload section: niri takes it out before the runtime and
    /// never re-runs it (niri e19df12 src/main.rs:168-169).
    #[test]
    fn spawn_at_startup_is_not_a_reload_step() {
        let old = config(BASE);
        let new = config(
            r#"
            spawn-at-startup "a.exe"
            layout { gaps 16; }
            animations { window-open { duration-ms 150; curve "ease-out-expo"; } }
            environment { FOO "1"; }
            input { keyboard { repeat-delay 600; repeat-rate 25; } }
            output "DP-1" { scale 1; }
            binds { Ctrl+Alt+F8 { screenshot; } }
            window-rule { match app-id="a"; opacity 0.9; }
            switch-events { lid-close { spawn "x"; } }
            screenshot-path "~/Pictures/a.png"
            workspace "chat" {}
        "#,
        );
        assert_eq!(plan(Some(&old), &new).steps(), &[] as &[ReloadStep]);
    }
}
