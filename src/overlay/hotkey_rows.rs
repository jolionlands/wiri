//! Curated Important Hotkeys, using only bindings Windows actually registered.
use crate::backend::message_loop::RegisteredBinding;
use std::collections::HashSet;

fn canonical(action: &str) -> &str {
    match action {
        "FocusLeft" => "FocusColumnLeft",
        "FocusRight" => "FocusColumnRight",
        "ToggleOverview" | "OverviewToggle" => "Overview",
        "ShowKeyBindings" => "Hotkeys",
        _ => action,
    }
}

fn important(action: &str) -> Option<(u8, String)> {
    let name = match action {
        a if a.starts_with("Spawn(")
            || a.starts_with("SpawnArgv(")
            || a.starts_with("SpawnCmd(") =>
        {
            (0, "Launch application")
        }
        "CloseWindow" => (1, "Close window"),
        "FocusColumnLeft" => (2, "Focus column left"),
        "FocusColumnRight" => (2, "Focus column right"),
        "FocusUp" => (2, "Focus window above"),
        "FocusDown" => (2, "Focus window below"),
        "MoveColumnLeft" => (3, "Move column left"),
        "MoveColumnRight" => (3, "Move column right"),
        "ConsumeOrExpelWindowLeft" => (4, "Consume or expel window left"),
        "ConsumeOrExpelWindowRight" => (4, "Consume or expel window right"),
        "MaximizeColumn" => (5, "Maximize column"),
        "ToggleFullscreen" => (5, "Fullscreen window"),
        "ToggleFloating" => (5, "Toggle floating"),
        "Overview" => (6, "Overview"),
        "FocusWorkspaceUp" | "WorkspacePrev" => (7, "Previous workspace"),
        "FocusWorkspaceDown" | "WorkspaceNext" => (7, "Next workspace"),
        "Hotkeys" => (8, "Important Hotkeys"),
        "Quit" => (9, "Exit wiri"),
        a if a.starts_with("FocusWorkspace(") => {
            return Some((
                7,
                format!(
                    "Workspace {}",
                    a.trim_start_matches("FocusWorkspace(")
                        .trim_end_matches(')')
                ),
            ));
        }
        _ => return None,
    };
    Some((name.0, name.1.into()))
}

/// Explicit titles take precedence, null hides a row, untitled nonessential variants are
/// omitted. Equivalent actions prefer the configured Mod chord over secondary aliases.
pub fn curated_rows(bindings: &[RegisteredBinding], mod_key: u32) -> Vec<(String, String)> {
    let mut choices: Vec<_> = bindings
        .iter()
        .filter_map(|b| {
            if matches!(b.title, Some(None)) {
                return None;
            }
            let action = canonical(&b.action);
            let (rank, description) = match &b.title {
                Some(Some(text)) => (0, text.clone()),
                _ => important(action)?,
            };
            let explicit = matches!(b.title, Some(Some(_)));
            let secondary = b.modifiers & mod_key != mod_key;
            Some((b, action, rank, description, explicit, secondary))
        })
        .collect();
    choices
        .sort_by_key(|(b, _, rank, _, explicit, secondary)| (!*explicit, *rank, *secondary, b.id));
    let mut actions = HashSet::new();
    let mut chords = HashSet::new();
    choices
        .into_iter()
        .filter_map(|(b, action, _, description, _, _)| {
            if chords.contains(&(b.modifiers, b.vk_code)) || actions.contains(action) {
                return None;
            }
            chords.insert((b.modifiers, b.vk_code));
            actions.insert(action);
            Some((
                crate::ipc::server::format_chord(b.modifiers, b.vk_code),
                description,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bind(
        id: i32,
        mods: u32,
        key: u32,
        action: &str,
        title: Option<Option<&str>>,
    ) -> RegisteredBinding {
        RegisteredBinding {
            id,
            modifiers: mods,
            vk_code: key,
            action: action.into(),
            title: title.map(|t| t.map(str::to_owned)),
        }
    }
    #[test]
    fn titles_override_essentials_and_null_always_hides() {
        let rows = curated_rows(
            &[
                bind(0, 3, 0x25, "FocusColumnLeft", None),
                bind(1, 3, 0x51, "CloseWindow", Some(None)),
                bind(2, 3, 0x46, "DebugLayout", Some(Some("My special action"))),
                bind(3, 3, 0x48, "FocusLeft", Some(Some("Look left"))),
            ],
            3,
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.1 == "My special action"));
        assert!(rows.iter().any(|r| r.1 == "Look left"));
        assert!(!rows.iter().any(|r| r.1 == "Focus column left"));
    }
    #[test]
    fn default_variants_and_duplicate_chords_do_not_flood_the_sheet() {
        let rows = curated_rows(
            &[
                bind(0, 1, 0x48, "FocusLeft", None),
                bind(1, 3, 0x25, "FocusColumnLeft", None),
                bind(2, 3, 0x25, "FocusColumnRight", None),
                bind(3, 3, 0x52, "SetColumnWidth(Fixed(100))", None),
                bind(4, 3, 0x54, "DebugLayout", None),
            ],
            3,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, crate::ipc::server::format_chord(3, 0x25));
    }
    #[test]
    fn shipped_shell_launcher_overview_and_quit_are_important() {
        let rows = curated_rows(
            &[
                bind(0, 3, 0x0d, "SpawnCmd(\"start wt.exe\")", None),
                bind(1, 3, 0x20, "OverviewToggle", None),
                bind(2, 7, 0x51, "Quit", None),
            ],
            3,
        );
        assert_eq!(
            rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
            vec!["Launch application", "Overview", "Exit wiri"]
        );
    }
    #[test]
    fn workspace_and_launch_are_readable_not_debug_strings() {
        let rows = curated_rows(
            &[
                bind(0, 3, 0x0d, "Spawn(\"wt.exe\")", None),
                bind(1, 3, 0x32, "FocusWorkspace(2)", None),
            ],
            3,
        );
        assert_eq!(rows[0].1, "Launch application");
        assert_eq!(rows[1].1, "Workspace 2");
    }
}
