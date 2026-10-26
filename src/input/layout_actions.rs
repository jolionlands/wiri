//! Shared, backend-independent transport and grammar for layout actions.
use super::{Action, WorkspaceRef};
use crate::utils::WindowId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FocusAction {
    ColumnRightOrFirst,
    ColumnLeftOrLast,
    Column(u32),
    WindowInColumn(u32),
    DownOrColumnLeft,
    DownOrColumnRight,
    UpOrColumnLeft,
    UpOrColumnRight,
    WindowTop,
    WindowBottom,
    DownOrTop,
    UpOrBottom,
    WindowOrWorkspaceDown,
    WindowOrWorkspaceUp,
    ColumnOrMonitorLeft,
    ColumnOrMonitorRight,
    WindowOrMonitorUp,
    WindowOrMonitorDown,
    /// Explicit debounce override; the legacy FocusPrevious uses the default.
    Previous {
        debounce_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkspaceAction {
    MoveWindow {
        window: Option<WindowId>,
        reference: WorkspaceRef,
        focus: bool,
    },
    MoveWindowRelative {
        window: Option<WindowId>,
        down: bool,
        focus: bool,
    },
    MoveColumn {
        reference: WorkspaceRef,
        focus: bool,
    },
    MoveColumnRelative {
        down: bool,
        focus: bool,
    },
    MoveToIndex {
        index: u32,
        workspace: Option<WorkspaceRef>,
    },
    SetName {
        name: String,
        workspace: Option<WorkspaceRef>,
    },
    UnsetName {
        workspace: Option<WorkspaceRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonitorTarget {
    Left,
    Right,
    Up,
    Down,
    Previous,
    Next,
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonitorAction {
    Focus(MonitorTarget),
    MoveWindow {
        window: Option<WindowId>,
        target: MonitorTarget,
    },
    MoveColumn(MonitorTarget),
    MoveWorkspace {
        workspace: Option<WorkspaceRef>,
        target: MonitorTarget,
    },
}

impl Action {
    /// Attach an IPC window target without first changing focus. Returns false for
    /// actions which still need the legacy focus-before-action handling.
    pub fn set_layout_window_target(&mut self, window: WindowId) -> bool {
        match self {
            Action::WorkspaceAction(WorkspaceAction::MoveWindow { window: id, .. })
            | Action::WorkspaceAction(WorkspaceAction::MoveWindowRelative { window: id, .. })
            | Action::MonitorAction(MonitorAction::MoveWindow { window: id, .. }) => {
                *id = Some(window)
            }
            Action::MoveToMonitorLeft | Action::MoveToMonitorRight => {
                let target = if matches!(self, Action::MoveToMonitorLeft) {
                    MonitorTarget::Left
                } else {
                    MonitorTarget::Right
                };
                *self = Action::MonitorAction(MonitorAction::MoveWindow {
                    window: Some(window),
                    target,
                });
            }
            Action::MoveToWorkspace(index) if *index >= 0 => {
                *self = Action::WorkspaceAction(WorkspaceAction::MoveWindow {
                    window: Some(window),
                    reference: WorkspaceRef::Index((*index as u32).max(1)),
                    focus: false,
                });
            }
            _ => return false,
        }
        true
    }
}

/// Unlike the legacy parser, a recognized layout action with invalid arguments
/// must not fall through to a permissive alias. Outer None means not our action.
pub(super) fn parse(name: &str, args: &[String], quoted: &[bool]) -> Option<Option<Action>> {
    let recognized = matches!(
        name,
        "focus-column-right-or-first"
            | "focus-column-left-or-last"
            | "focus-column"
            | "focus-window-in-column"
            | "focus-window-down-or-column-left"
            | "focus-window-down-or-column-right"
            | "focus-window-up-or-column-left"
            | "focus-window-up-or-column-right"
            | "focus-window-top"
            | "focus-window-bottom"
            | "focus-window-down-or-top"
            | "focus-window-up-or-bottom"
            | "focus-window-or-workspace-down"
            | "focus-window-or-workspace-up"
            | "focus-column-or-monitor-left"
            | "focus-column-or-monitor-right"
            | "focus-window-or-monitor-up"
            | "focus-window-or-monitor-down"
            | "focus-column-left"
            | "focus-left"
            | "focus-column-right"
            | "focus-right"
            | "focus-up"
            | "focus-window-up"
            | "focus-down"
            | "focus-window-down"
            | "focus-column-first"
            | "focus-column-last"
            | "focus-window-previous"
            | "focus-previous"
            | "alt-tab"
            | "focus-history-back"
            | "focus-workspace"
            | "focus-workspace-named"
            | "switch-workspace"
            | "workspace"
            | "focus-workspace-next"
            | "workspace-next"
            | "focus-workspace-down"
            | "focus-workspace-prev"
            | "workspace-prev"
            | "focus-workspace-up"
            | "focus-workspace-previous"
            | "focus-workspace-back"
            | "move-to-workspace"
            | "move-window-to-workspace"
            | "move-window-to-workspace-down"
            | "move-window-to-workspace-up"
            | "move-column-to-workspace"
            | "move-column-to-workspace-down"
            | "move-column-to-workspace-up"
            | "move-column-up"
            | "move-column-down"
            | "move-column-to-workspace-above"
            | "move-column-to-workspace-below"
            | "move-workspace"
            | "move-workspace-up"
            | "swap-workspace-up"
            | "move-workspace-down"
            | "swap-workspace-down"
            | "move-workspace-to-index"
            | "set-workspace-name"
            | "rename-workspace"
            | "rename-ws"
            | "unset-workspace-name"
            | "focus-monitor"
            | "focus-monitor-left"
            | "focus-monitor-right"
            | "focus-monitor-up"
            | "focus-monitor-down"
            | "focus-monitor-previous"
            | "focus-monitor-next"
            | "move-window-to-monitor"
            | "move-window-to-monitor-left"
            | "move-window-to-monitor-right"
            | "move-window-to-monitor-up"
            | "move-window-to-monitor-down"
            | "move-window-to-monitor-previous"
            | "move-window-to-monitor-next"
            | "move-to-monitor-left"
            | "move-to-monitor-right"
            | "move-column-to-monitor"
            | "move-column-to-monitor-left"
            | "move-column-to-monitor-right"
            | "move-column-to-monitor-up"
            | "move-column-to-monitor-down"
            | "move-column-to-monitor-previous"
            | "move-column-to-monitor-next"
            | "move-workspace-to-monitor"
            | "move-workspace-to-monitor-left"
            | "move-workspace-to-monitor-right"
            | "move-workspace-to-monitor-up"
            | "move-workspace-to-monitor-down"
            | "move-workspace-to-monitor-previous"
            | "move-workspace-to-monitor-next"
    );
    if !recognized {
        return None;
    }
    Some(parse_known(name, args, quoted))
}

struct Words<'a> {
    positional: Vec<(&'a str, bool)>,
    focus: Option<bool>,
    workspace: Option<WorkspaceRef>,
    window: Option<WindowId>,
    debounce_ms: Option<u64>,
}

impl<'a> Words<'a> {
    fn read(args: &'a [String], quoted: &[bool], allowed: &[&str]) -> Option<Self> {
        let mut words = Self {
            positional: Vec::new(),
            focus: None,
            workspace: None,
            window: None,
            debounce_ms: None,
        };
        let mut i = 0;
        while i < args.len() {
            let word = args[i].as_str();
            let is_quoted = quoted.get(i).copied().unwrap_or(false);
            if !is_quoted && word.starts_with("--") {
                // Support both CLI spellings; a flag's value may itself be quoted.
                let (flag, inline) = match word.split_once('=') {
                    Some((flag, value)) => (flag, Some(value)),
                    None => (word, None),
                };
                if !allowed.contains(&flag) {
                    return None;
                }
                let (value, value_quoted) = if let Some(value) = inline {
                    (value, false)
                } else {
                    i += 1;
                    let value = args.get(i)?.as_str();
                    let value_quoted = quoted.get(i).copied().unwrap_or(false);
                    // An unquoted flag starts another option, not this option's value.
                    if !value_quoted && value.starts_with("--") {
                        return None;
                    }
                    (value, value_quoted)
                };
                match flag {
                    "--focus" if words.focus.is_none() => {
                        words.focus = Some(match value {
                            "true" => true,
                            "false" => false,
                            _ => return None,
                        })
                    }
                    "--workspace" if words.workspace.is_none() => {
                        words.workspace = Some(WorkspaceRef::from_word(value, value_quoted)?)
                    }
                    "--id" if words.window.is_none() => {
                        let id = value.parse::<isize>().ok()?;
                        if id <= 0 {
                            return None;
                        }
                        words.window = Some(WindowId::new(id));
                    }
                    "--debounce-ms" if words.debounce_ms.is_none() => {
                        words.debounce_ms = Some(value.parse().ok()?)
                    }
                    _ => return None,
                }
            } else {
                words.positional.push((word, is_quoted));
            }
            i += 1;
        }
        Some(words)
    }

    fn one(&self) -> Option<&'a str> {
        if self.positional.len() == 1 {
            Some(self.positional[0].0)
        } else {
            None
        }
    }

    fn reference(&self) -> Option<WorkspaceRef> {
        self.one()?;
        WorkspaceRef::from_word(self.positional[0].0, self.positional[0].1)
    }
}

fn parse_known(name: &str, args: &[String], quoted: &[bool]) -> Option<Action> {
    use FocusAction as F;
    use WorkspaceAction as W;
    let allowed: &[&str] = match name {
        "move-window-to-workspace"
        | "move-window-to-workspace-down"
        | "move-window-to-workspace-up" => &["--focus", "--id"],
        "move-column-to-workspace"
        | "move-column-to-workspace-down"
        | "move-column-to-workspace-up"
        | "move-column-up"
        | "move-column-down"
        | "move-column-to-workspace-above"
        | "move-column-to-workspace-below" => &["--focus"],
        "move-workspace-to-index"
        | "set-workspace-name"
        | "rename-workspace"
        | "rename-ws"
        | "unset-workspace-name" => &["--workspace"],
        "focus-window-previous" | "focus-previous" | "alt-tab" | "focus-history-back" => {
            &["--debounce-ms"]
        }
        n if n.starts_with("move-workspace-to-monitor") => &["--workspace"],
        n if n.starts_with("move-window-to-monitor")
            || n == "move-to-monitor-left"
            || n == "move-to-monitor-right" =>
        {
            &["--id"]
        }
        _ => &[],
    };
    let w = Words::read(args, quoted, allowed)?;
    let empty = w.positional.is_empty();
    let focus = |action| {
        if empty {
            Some(Action::LayoutFocus(action))
        } else {
            None
        }
    };
    let simple = |action| if empty { Some(action) } else { None };
    match name {
        "focus-column-right-or-first" => focus(F::ColumnRightOrFirst),
        "focus-column-left-or-last" => focus(F::ColumnLeftOrLast),
        "focus-column" => Some(Action::LayoutFocus(F::Column(
            w.one()?.parse::<u32>().ok()?.max(1),
        ))),
        "focus-window-in-column" => Some(Action::LayoutFocus(F::WindowInColumn(
            w.one()?.parse::<u32>().ok()?.max(1),
        ))),
        "focus-window-down-or-column-left" => focus(F::DownOrColumnLeft),
        "focus-window-down-or-column-right" => focus(F::DownOrColumnRight),
        "focus-window-up-or-column-left" => focus(F::UpOrColumnLeft),
        "focus-window-up-or-column-right" => focus(F::UpOrColumnRight),
        "focus-window-top" => focus(F::WindowTop),
        "focus-window-bottom" => focus(F::WindowBottom),
        "focus-window-down-or-top" => focus(F::DownOrTop),
        "focus-window-up-or-bottom" => focus(F::UpOrBottom),
        "focus-window-or-workspace-down" => focus(F::WindowOrWorkspaceDown),
        "focus-window-or-workspace-up" => focus(F::WindowOrWorkspaceUp),
        "focus-column-or-monitor-left" => focus(F::ColumnOrMonitorLeft),
        "focus-column-or-monitor-right" => focus(F::ColumnOrMonitorRight),
        "focus-window-or-monitor-up" => focus(F::WindowOrMonitorUp),
        "focus-window-or-monitor-down" => focus(F::WindowOrMonitorDown),
        "focus-column-left" | "focus-left" => simple(Action::FocusColumnLeft),
        "focus-column-right" | "focus-right" => simple(Action::FocusColumnRight),
        "focus-up" | "focus-window-up" => simple(Action::FocusUp),
        "focus-down" | "focus-window-down" => simple(Action::FocusDown),
        "focus-column-first" => simple(Action::FocusColumnFirst),
        "focus-column-last" => simple(Action::FocusColumnLast),
        "focus-window-previous" | "focus-previous" | "alt-tab" | "focus-history-back" => {
            simple(match w.debounce_ms {
                Some(debounce_ms) => Action::LayoutFocus(F::Previous { debounce_ms }),
                None => Action::FocusPrevious,
            })
        }
        "focus-workspace" => match w.reference()? {
            WorkspaceRef::Index(index) => Some(Action::FocusWorkspace(i32::try_from(index).ok()?)),
            WorkspaceRef::Name(name) => Some(Action::FocusWorkspaceNamed(name)),
        },
        "focus-workspace-named" => Some(Action::FocusWorkspaceNamed(w.one()?.to_string())),
        "switch-workspace" | "workspace" => Some(Action::FocusWorkspace(w.one()?.parse().ok()?)),
        "focus-workspace-next" | "workspace-next" | "focus-workspace-down" => {
            simple(Action::FocusWorkspaceNext)
        }
        "focus-workspace-prev" | "workspace-prev" | "focus-workspace-up" => {
            simple(Action::FocusWorkspacePrevious)
        }
        "focus-workspace-previous" | "focus-workspace-back" => simple(Action::FocusWorkspaceLast),
        "move-to-workspace" => Some(Action::MoveToWorkspace(w.one()?.parse().ok()?)),
        "move-window-to-workspace" => Some(Action::WorkspaceAction(W::MoveWindow {
            window: w.window,
            reference: w.reference()?,
            focus: w.focus.unwrap_or(true),
        })),
        "move-window-to-workspace-down" | "move-window-to-workspace-up" if empty => {
            Some(Action::WorkspaceAction(W::MoveWindowRelative {
                window: w.window,
                down: name.ends_with("down"),
                focus: w.focus.unwrap_or(true),
            }))
        }
        "move-column-to-workspace" => Some(Action::WorkspaceAction(W::MoveColumn {
            reference: w.reference()?,
            focus: w.focus.unwrap_or(true),
        })),
        "move-column-to-workspace-down" | "move-column-down" | "move-column-to-workspace-below"
            if empty =>
        {
            Some(match w.focus {
                None => Action::MoveColumnToWorkspaceDown,
                Some(focus) => Action::WorkspaceAction(W::MoveColumnRelative { down: true, focus }),
            })
        }
        "move-column-to-workspace-up" | "move-column-up" | "move-column-to-workspace-above"
            if empty =>
        {
            Some(match w.focus {
                None => Action::MoveColumnToWorkspaceUp,
                Some(focus) => {
                    Action::WorkspaceAction(W::MoveColumnRelative { down: false, focus })
                }
            })
        }
        "move-workspace" => Some(Action::MoveWorkspace(w.one()?.parse().ok()?)),
        "move-workspace-down" | "swap-workspace-down" => simple(Action::MoveWorkspaceDown),
        "move-workspace-up" | "swap-workspace-up" => simple(Action::MoveWorkspaceUp),
        "move-workspace-to-index" => Some(Action::WorkspaceAction(W::MoveToIndex {
            index: w.one()?.parse::<u32>().ok()?.max(1),
            workspace: w.workspace.clone(),
        })),
        "set-workspace-name" | "rename-workspace" | "rename-ws" => {
            let name = w.one()?.to_string();
            Some(match w.workspace {
                None => Action::RenameWorkspace(name),
                workspace => Action::WorkspaceAction(W::SetName { name, workspace }),
            })
        }
        "unset-workspace-name" if empty => Some(Action::WorkspaceAction(W::UnsetName {
            workspace: w.workspace,
        })),
        _ => {
            let (kind, suffix) = if let Some(s) = name.strip_prefix("focus-monitor") {
                (0, s)
            } else if let Some(s) = name.strip_prefix("move-window-to-monitor") {
                (1, s)
            } else if let Some(s) = name.strip_prefix("move-to-monitor") {
                (1, s)
            } else if let Some(s) = name.strip_prefix("move-column-to-monitor") {
                (2, s)
            } else if let Some(s) = name.strip_prefix("move-workspace-to-monitor") {
                (3, s)
            } else {
                return None;
            };
            let target = if suffix.is_empty() {
                let name = w.one()?;
                if name.is_empty() {
                    return None;
                }
                MonitorTarget::Name(name.to_string())
            } else {
                if !empty {
                    return None;
                }
                match suffix {
                    "-left" => MonitorTarget::Left,
                    "-right" => MonitorTarget::Right,
                    "-up" => MonitorTarget::Up,
                    "-down" => MonitorTarget::Down,
                    "-previous" => MonitorTarget::Previous,
                    "-next" => MonitorTarget::Next,
                    _ => return None,
                }
            };
            // Keep existing parse equality for the old no-property spellings.
            Some(match (kind, &target, w.window) {
                (0, MonitorTarget::Left, _) => Action::FocusMonitorLeft,
                (0, MonitorTarget::Right, _) => Action::FocusMonitorRight,
                (1, MonitorTarget::Left, None) => Action::MoveToMonitorLeft,
                (1, MonitorTarget::Right, None) => Action::MoveToMonitorRight,
                (2, MonitorTarget::Left, _) => Action::MoveColumnToMonitorLeft,
                (2, MonitorTarget::Right, _) => Action::MoveColumnToMonitorRight,
                (0, _, _) => Action::MonitorAction(MonitorAction::Focus(target)),
                (1, _, _) => Action::MonitorAction(MonitorAction::MoveWindow {
                    window: w.window,
                    target,
                }),
                (2, _, _) => Action::MonitorAction(MonitorAction::MoveColumn(target)),
                _ => Action::MonitorAction(MonitorAction::MoveWorkspace {
                    workspace: w.workspace,
                    target,
                }),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{parse_action_flagged, parse_action_name};

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn composite_focus_actions_are_shared_by_config_and_ipc() {
        for (name, expected) in [
            (
                "focus-column-right-or-first",
                FocusAction::ColumnRightOrFirst,
            ),
            ("focus-column-left-or-last", FocusAction::ColumnLeftOrLast),
            (
                "focus-window-down-or-column-left",
                FocusAction::DownOrColumnLeft,
            ),
            (
                "focus-window-down-or-column-right",
                FocusAction::DownOrColumnRight,
            ),
            (
                "focus-window-up-or-column-left",
                FocusAction::UpOrColumnLeft,
            ),
            (
                "focus-window-up-or-column-right",
                FocusAction::UpOrColumnRight,
            ),
            ("focus-window-top", FocusAction::WindowTop),
            ("focus-window-bottom", FocusAction::WindowBottom),
            ("focus-window-down-or-top", FocusAction::DownOrTop),
            ("focus-window-up-or-bottom", FocusAction::UpOrBottom),
            (
                "focus-window-or-workspace-down",
                FocusAction::WindowOrWorkspaceDown,
            ),
            (
                "focus-window-or-workspace-up",
                FocusAction::WindowOrWorkspaceUp,
            ),
            (
                "focus-column-or-monitor-left",
                FocusAction::ColumnOrMonitorLeft,
            ),
            (
                "focus-column-or-monitor-right",
                FocusAction::ColumnOrMonitorRight,
            ),
            ("focus-window-or-monitor-up", FocusAction::WindowOrMonitorUp),
            (
                "focus-window-or-monitor-down",
                FocusAction::WindowOrMonitorDown,
            ),
        ] {
            assert_eq!(
                parse_action_name(name, &[]),
                Some(Action::LayoutFocus(expected))
            );
            assert_eq!(parse_action_name(name, &words(&["extra"])), None, "{name}");
        }
        assert_eq!(
            parse_action_name("focus-column", &words(&["3"])),
            Some(Action::LayoutFocus(FocusAction::Column(3)))
        );
        assert_eq!(
            parse_action_name("focus-window-in-column", &words(&["2"])),
            Some(Action::LayoutFocus(FocusAction::WindowInColumn(2)))
        );
    }

    #[test]
    fn workspace_references_and_focus_are_not_lost() {
        let args = words(&["2", "--focus", "false"]);
        assert_eq!(
            parse_action_flagged("move-window-to-workspace", &args, &[true]),
            Some(Action::WorkspaceAction(WorkspaceAction::MoveWindow {
                window: None,
                reference: WorkspaceRef::Name("2".into()),
                focus: false,
            }))
        );
        assert_eq!(
            parse_action_name("move-window-to-workspace", &words(&["2"])),
            Some(Action::WorkspaceAction(WorkspaceAction::MoveWindow {
                window: None,
                reference: WorkspaceRef::Index(2),
                focus: true,
            }))
        );
        assert_eq!(
            parse_action_name("move-to-workspace", &words(&["2"])),
            Some(Action::MoveToWorkspace(2))
        );
        assert_eq!(
            parse_action_name("move-column-to-workspace-up", &[]),
            Some(Action::MoveColumnToWorkspaceUp)
        );
        assert_eq!(
            parse_action_flagged(
                "move-workspace-to-index",
                &words(&["3", "--workspace", "2"]),
                &[false, false, true]
            ),
            Some(Action::WorkspaceAction(WorkspaceAction::MoveToIndex {
                index: 3,
                workspace: Some(WorkspaceRef::Name("2".into())),
            }))
        );
    }

    #[test]
    fn legacy_aliases_preserve_transport() {
        for (name, args, expected) in [
            ("focus-left", vec![], Action::FocusColumnLeft),
            ("alt-tab", vec![], Action::FocusPrevious),
            ("focus-workspace-down", vec![], Action::FocusWorkspaceNext),
            ("move-to-monitor-left", vec![], Action::MoveToMonitorLeft),
            (
                "move-window-to-monitor-right",
                vec![],
                Action::MoveToMonitorRight,
            ),
            (
                "rename-ws",
                words(&["web"]),
                Action::RenameWorkspace("web".into()),
            ),
        ] {
            assert_eq!(parse_action_name(name, &args), Some(expected), "{name}");
        }
    }

    #[test]
    fn bad_flags_and_surplus_arguments_are_rejected() {
        for (name, args) in [
            ("focus-column", vec!["1", "2"]),
            ("focus-column", vec!["-1"]),
            ("focus-monitor-left", vec!["extra"]),
            ("focus-workspace", vec!["1", "2"]),
            ("move-window-to-workspace", vec!["2", "--focus", "yes"]),
            ("move-window-to-workspace", vec!["2", "--focus"]),
            (
                "move-window-to-workspace",
                vec!["2", "--focus", "true", "--focus", "false"],
            ),
            ("move-window-to-workspace", vec!["2", "--workspace", "web"]),
            ("move-column-to-workspace", vec!["2", "--id", "42"]),
            ("move-window-to-monitor-up", vec!["--id", "abc"]),
            ("move-workspace-to-monitor", vec!["screen", "extra"]),
            ("unset-workspace-name", vec!["web"]),
            ("focus-window-previous", vec!["--debounce-ms", "-1"]),
        ] {
            assert_eq!(
                parse_action_name(name, &words(&args)),
                None,
                "{name} {args:?}"
            );
        }
    }

    #[test]
    fn workspace_option_requires_a_value_not_an_unquoted_flag() {
        for args in [
            vec!["--workspace"],
            vec!["--workspace", "--bogus"],
            vec!["--workspace", "--workspace=2"],
        ] {
            assert_eq!(
                parse_action_name("unset-workspace-name", &words(&args)),
                None,
                "{args:?}"
            );
        }
        let expected = Some(Action::WorkspaceAction(WorkspaceAction::UnsetName {
            workspace: Some(WorkspaceRef::Name("--bogus".into())),
        }));
        assert_eq!(
            parse_action_flagged(
                "unset-workspace-name",
                &words(&["--workspace", "--bogus"]),
                &[false, true]
            ),
            expected
        );
        assert_eq!(
            parse_action_name("unset-workspace-name", &words(&["--workspace=--bogus"])),
            expected
        );
    }

    #[test]
    fn zero_layout_indices_mean_first_but_quoted_workspaces_remain_names() {
        for (name, expected) in [
            ("focus-column", Action::LayoutFocus(FocusAction::Column(1))),
            (
                "focus-window-in-column",
                Action::LayoutFocus(FocusAction::WindowInColumn(1)),
            ),
            (
                "move-workspace-to-index",
                Action::WorkspaceAction(WorkspaceAction::MoveToIndex {
                    index: 1,
                    workspace: None,
                }),
            ),
            ("focus-workspace", Action::FocusWorkspace(1)),
            (
                "move-window-to-workspace",
                Action::WorkspaceAction(WorkspaceAction::MoveWindow {
                    window: None,
                    reference: WorkspaceRef::Index(1),
                    focus: true,
                }),
            ),
            (
                "move-column-to-workspace",
                Action::WorkspaceAction(WorkspaceAction::MoveColumn {
                    reference: WorkspaceRef::Index(1),
                    focus: true,
                }),
            ),
        ] {
            assert_eq!(
                parse_action_name(name, &words(&["0"])),
                Some(expected),
                "{name}"
            );
        }
        assert_eq!(
            parse_action_flagged("focus-workspace", &words(&["0"]), &[true]),
            Some(Action::FocusWorkspaceNamed("0".into()))
        );
        for (quoted, reference) in [
            (false, WorkspaceRef::Index(1)),
            (true, WorkspaceRef::Name("0".into())),
        ] {
            assert_eq!(
                parse_action_flagged(
                    "unset-workspace-name",
                    &words(&["--workspace", "0"]),
                    &[false, quoted]
                ),
                Some(Action::WorkspaceAction(WorkspaceAction::UnsetName {
                    workspace: Some(reference)
                }))
            );
        }
        // Legacy transports retain their existing zero value.
        assert_eq!(
            parse_action_name("switch-workspace", &words(&["0"])),
            Some(Action::FocusWorkspace(0))
        );
        assert_eq!(
            parse_action_name("move-to-workspace", &words(&["0"])),
            Some(Action::MoveToWorkspace(0))
        );
    }

    #[test]
    fn window_targets_do_not_require_a_focus_operation() {
        let mut action = parse_action_name("move-window-to-monitor-left", &[]).unwrap();
        assert!(action.set_layout_window_target(WindowId::new(42)));
        assert_eq!(
            action,
            Action::MonitorAction(MonitorAction::MoveWindow {
                window: Some(WindowId::new(42)),
                target: MonitorTarget::Left
            })
        );
        assert!(!Action::FocusColumnLeft.set_layout_window_target(WindowId::new(42)));
    }
}
