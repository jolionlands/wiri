//! Curated Important Hotkeys, using only bindings Windows actually registered.
use crate::backend::message_loop::RegisteredBinding;
use crate::input::{Action, WorkspaceAction};
use std::collections::HashSet;

#[derive(Clone, PartialEq, Eq, Hash)]
enum OverlayAction {
    Typed(Action),
    Legacy(String),
}

fn normalized(action: Action) -> Action {
    match action {
        Action::MoveColumnToWorkspaceDown => {
            Action::WorkspaceAction(WorkspaceAction::MoveColumnRelative {
                down: true,
                focus: true,
            })
        }
        Action::MoveColumnToWorkspaceUp => {
            Action::WorkspaceAction(WorkspaceAction::MoveColumnRelative {
                down: false,
                focus: true,
            })
        }
        Action::System(crate::input::system_actions::SystemAction::Quit {
            skip_confirmation: false,
        }) => Action::Quit,
        action => action,
    }
}

// Compatibility for older snapshot producers with only a simple action name.
// Parameterized actions and command heads always use typed runtime metadata.
fn named_action(name: &str) -> Option<Action> {
    Some(match name {
        "ShowKeyBindings" | "Hotkeys" => Action::ShowKeyBindings,
        "Quit" => Action::Quit,
        "CloseWindow" => Action::CloseWindow,
        "FocusLeft" | "FocusColumnLeft" => Action::FocusColumnLeft,
        "FocusRight" | "FocusColumnRight" => Action::FocusColumnRight,
        "MoveColumnLeft" => Action::MoveColumnLeft,
        "MoveColumnRight" => Action::MoveColumnRight,
        "FocusWorkspaceDown" | "FocusWorkspaceNext" | "WorkspaceNext" => Action::FocusWorkspaceNext,
        "FocusWorkspaceUp" | "FocusWorkspacePrevious" | "WorkspacePrev" => {
            Action::FocusWorkspacePrevious
        }
        "MoveColumnToWorkspaceDown" => Action::MoveColumnToWorkspaceDown,
        "MoveColumnToWorkspaceUp" => Action::MoveColumnToWorkspaceUp,
        "ColumnWidthPresetCycle" => Action::ColumnWidthPresetCycle,
        "MaximizeColumn" => Action::MaximizeColumn,
        "ConsumeOrExpelWindowLeft" => Action::ConsumeOrExpelWindowLeft,
        "ConsumeOrExpelWindowRight" => Action::ConsumeOrExpelWindowRight,
        "ToggleFloating" => Action::ToggleFloating,
        "SwitchFocusBetweenFloatingAndTiling" => Action::SwitchFocusBetweenFloatingAndTiling,
        "Overview" | "ToggleOverview" | "OverviewToggle" => Action::OverviewToggle,
        _ => return None,
    })
}

fn typed(action: Action) -> OverlayAction {
    OverlayAction::Typed(normalized(action))
}

fn binding_action(binding: &RegisteredBinding) -> OverlayAction {
    binding
        .overlay_action
        .clone()
        .or_else(|| named_action(&binding.action))
        .map(typed)
        .unwrap_or_else(|| OverlayAction::Legacy(binding.action.clone()))
}

fn description(action: &OverlayAction) -> String {
    match action {
        OverlayAction::Typed(
            Action::Quit | Action::System(crate::input::system_actions::SystemAction::Quit { .. }),
        ) => "Exit wiri".into(),
        OverlayAction::Typed(Action::WorkspaceAction(WorkspaceAction::MoveColumnRelative {
            down,
            ..
        })) => {
            format!(
                "Move Column to Workspace {}",
                if *down { "Down" } else { "Up" }
            )
        }
        OverlayAction::Typed(Action::WorkspaceAction(WorkspaceAction::MoveWindowRelative {
            down,
            ..
        })) => {
            format!(
                "Move Window to Workspace {}",
                if *down { "Down" } else { "Up" }
            )
        }
        OverlayAction::Typed(Action::Screenshot | Action::ScreenshotUi(_)) => {
            "Take a Screenshot".into()
        }
        OverlayAction::Typed(Action::Spawn(argv)) => {
            format!("Spawn {}", argv.first().map(String::as_str).unwrap_or(""))
        }
        OverlayAction::Typed(Action::SpawnCmd(command)) => format!(
            "Spawn {}",
            command.split_ascii_whitespace().next().unwrap_or("")
        ),
        _ => ESSENTIALS
            .iter()
            .find_map(|(name, label)| {
                (typed(named_action(name).unwrap()) == *action).then(|| (*label).to_string())
            })
            .unwrap_or_else(|| "FIXME: Unknown".into()),
    }
}

/// Pinned niri order: essentials, one bound screenshot action, custom-titled
/// actions, then Mod/Super keyboard spawns. Resolve titles across the whole
/// action: first custom title wins, otherwise any null hides it, otherwise the
/// first ordinary bind wins. Placeholders retain their essential positions.
pub fn curated_rows(
    bindings: &[RegisteredBinding],
    _mod_key: u32,
    hide_not_bound: bool,
) -> Vec<(String, String)> {
    let bound: Vec<_> = bindings.iter().map(binding_action).collect();
    let mut actions: Vec<_> = ESSENTIALS
        .iter()
        .map(|(name, _)| typed(named_action(name).unwrap()))
        .collect();
    if !bound.contains(&typed(Action::Quit)) {
        if let Some(action) = bound.iter().find(|action| {
            matches!(
                action,
                OverlayAction::Typed(Action::System(
                    crate::input::system_actions::SystemAction::Quit {
                        skip_confirmation: true,
                    }
                ))
            )
        }) {
            actions[1] = action.clone();
        }
    }
    for (index, down) in [(9, true), (10, false)] {
        if let Some(action) = bound.iter().find(|action| matches!(action,
            OverlayAction::Typed(Action::WorkspaceAction(WorkspaceAction::MoveColumnRelative { down: d, .. })) if *d == down)) {
            // Preserve the selected bound column transfer's focus flag.
            actions[index] = action.clone();
        } else if bound.iter().any(|action| matches!(action,
            OverlayAction::Typed(Action::WorkspaceAction(WorkspaceAction::MoveWindowRelative { down: d, .. })) if *d == down)) {
            // The pin's window fallback specifically selects focus=true.
            actions[index] = typed(Action::WorkspaceAction(WorkspaceAction::MoveWindowRelative {
                window: None, down, focus: true,
            }));
        }
    }
    if let Some(action) = bound.iter().find(|action| {
        matches!(
            action,
            OverlayAction::Typed(Action::Screenshot | Action::ScreenshotUi(_))
        )
    }) {
        actions.push(action.clone());
    }
    for (binding, action) in bindings.iter().zip(&bound) {
        if matches!(binding.title, Some(Some(_))) && !actions.contains(action) {
            actions.push(action.clone());
        }
    }
    for (binding, action) in bindings.iter().zip(&bound) {
        if matches!(
            action,
            OverlayAction::Typed(Action::Spawn(_) | Action::SpawnCmd(_))
        ) && (binding.overlay_uses_compositor_mod || binding.modifiers & 8 != 0)
            && !actions.contains(action)
        {
            actions.push(action.clone());
        }
    }
    let mut chords = HashSet::new();
    let mut rows = Vec::new();
    for action in actions {
        let mut custom = None;
        let mut ordinary = None;
        let mut hidden = false;
        for (binding, candidate) in bindings.iter().zip(&bound) {
            if *candidate != action {
                continue;
            }
            match &binding.title {
                Some(Some(_)) => {
                    custom.get_or_insert(binding);
                }
                Some(None) => hidden = true,
                None => {
                    ordinary.get_or_insert(binding);
                }
            }
        }
        if custom.is_none() && hidden {
            continue;
        }
        let selected = custom.or(ordinary);
        if selected.is_none() && hide_not_bound {
            continue;
        }
        let label = custom
            .and_then(|binding| binding.title.as_ref().and_then(|title| title.clone()))
            .unwrap_or_else(|| description(&action));
        let chord = if let Some(binding) = selected {
            if !chords.insert((binding.modifiers, binding.vk_code)) {
                continue;
            }
            crate::ipc::server::format_chord(binding.modifiers, binding.vk_code)
        } else {
            NOT_BOUND.to_string()
        };
        rows.push((chord, label));
    }
    rows
}

/// What an unbound row shows in the chord column (niri prints "(not bound)").
pub const NOT_BOUND: &str = "(not bound)";

/// The actions every wiri install can do, in the order niri lists them, with the wording the
/// sheet uses.  A bind of one of these replaces the placeholder row.
const ESSENTIALS: &[(&str, &str)] = &[
    ("ShowKeyBindings", "Show Important Hotkeys"),
    ("Quit", "Exit wiri"),
    ("CloseWindow", "Close Focused Window"),
    ("FocusColumnLeft", "Focus Column to the Left"),
    ("FocusColumnRight", "Focus Column to the Right"),
    ("MoveColumnLeft", "Move Column Left"),
    ("MoveColumnRight", "Move Column Right"),
    ("FocusWorkspaceDown", "Switch Workspace Down"),
    ("FocusWorkspaceUp", "Switch Workspace Up"),
    ("MoveColumnToWorkspaceDown", "Move Column to Workspace Down"),
    ("MoveColumnToWorkspaceUp", "Move Column to Workspace Up"),
    ("ColumnWidthPresetCycle", "Switch Preset Column Widths"),
    ("MaximizeColumn", "Maximize Column"),
    ("ConsumeOrExpelWindowLeft", "Consume or Expel Window Left"),
    ("ConsumeOrExpelWindowRight", "Consume or Expel Window Right"),
    ("ToggleFloating", "Move Window Between Floating and Tiling"),
    (
        "SwitchFocusBetweenFloatingAndTiling",
        "Switch Focus Between Floating and Tiling",
    ),
    ("Overview", "Open the Overview"),
];

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
            overlay_action: None,
            overlay_uses_compositor_mod: false,
            title: title.map(|t| t.map(str::to_owned)),
        }
    }

    fn action_bind(
        id: i32,
        mods: u32,
        key: u32,
        action: crate::input::Action,
        title: Option<Option<&str>>,
    ) -> RegisteredBinding {
        let mut binding = bind(id, mods, key, &format!("{action:?}"), title);
        binding.overlay_action = Some(action);
        binding
    }

    fn mod_action_bind(
        id: i32,
        key: u32,
        action: crate::input::Action,
        title: Option<Option<&str>>,
    ) -> RegisteredBinding {
        let mut binding = action_bind(id, 3, key, action, title);
        // This helper represents a literal Mod chord, independently of its mask.
        binding.overlay_uses_compositor_mod = true;
        binding
    }

    fn parsed_bind(id: i32, key: u32, command: &str, args: &[&str]) -> RegisteredBinding {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        let action = crate::input::parse_action_flagged(command, &args, &[])
            .unwrap_or_else(|| panic!("fixture action must parse: {command} {args:?}"));
        mod_action_bind(id, key, action, None)
    }

    // Pinned niri e19df12: ui/hotkey_overlay.rs collect_actions/format_bind.
    // These expectations are independent of this module's ESSENTIALS table.
    #[test]
    fn pinned_hotkey_default_essentials_have_exact_order() {
        let rows = curated_rows(&[], 3, false);
        let expected = [
            "Show Important Hotkeys",
            "Exit wiri",
            "Close Focused Window",
            "Focus Column to the Left",
            "Focus Column to the Right",
            "Move Column Left",
            "Move Column Right",
            "Switch Workspace Down",
            "Switch Workspace Up",
            "Move Column to Workspace Down",
            "Move Column to Workspace Up",
            "Switch Preset Column Widths",
            "Maximize Column",
            "Consume or Expel Window Left",
            "Consume or Expel Window Right",
            "Move Window Between Floating and Tiling",
            "Switch Focus Between Floating and Tiling",
            "Open the Overview",
        ];
        assert_eq!(rows.len(), 18);
        assert_eq!(
            rows.iter().map(|row| row.1.as_str()).collect::<Vec<_>>(),
            expected.to_vec()
        );
        assert!(rows.iter().all(|row| row.0 == NOT_BOUND));
        assert!(curated_rows(&[], 3, true).is_empty());
    }

    #[test]
    fn pinned_hotkey_essentials_precede_custom_actions_and_spawns() {
        let bindings = [
            mod_action_bind(
                0,
                0x54,
                crate::input::Action::Spawn(vec!["wt.exe".into()]),
                None,
            ),
            action_bind(1, 3, 0x51, crate::input::Action::CloseWindow, None),
            action_bind(
                2,
                3,
                0x44,
                crate::input::Action::ToggleFullscreen,
                Some(Some("Custom action")),
            ),
        ];
        let rows = curated_rows(&bindings, 3, true);
        assert_eq!(
            rows.iter().map(|row| row.0.clone()).collect::<Vec<_>>(),
            [0x51, 0x44, 0x54]
                .iter()
                .map(|key| crate::ipc::server::format_chord(3, *key))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pinned_hotkey_null_title_suppresses_ordinary_bind_for_the_action() {
        let bindings = [
            action_bind(0, 3, 0x51, crate::input::Action::CloseWindow, None),
            action_bind(1, 3, 0x57, crate::input::Action::CloseWindow, Some(None)),
        ];
        assert!(curated_rows(&bindings, 3, true).is_empty());
    }

    #[test]
    fn pinned_hotkey_null_title_does_not_resurrect_an_unbound_essential() {
        let bindings = [action_bind(
            0,
            3,
            0x51,
            crate::input::Action::CloseWindow,
            Some(None),
        )];
        let rows = curated_rows(&bindings, 3, false);
        assert!(!rows
            .iter()
            .any(|row| { row.1 == "Close window" || row.1 == "Close Focused Window" }));
        assert!(!rows
            .iter()
            .any(|row| row.0 == crate::ipc::server::format_chord(3, 0x51)));
    }

    #[test]
    fn pinned_hotkey_custom_title_survives_null_title_control() {
        let bindings = [
            action_bind(0, 3, 0x51, crate::input::Action::CloseWindow, Some(None)),
            action_bind(
                1,
                3,
                0x57,
                crate::input::Action::CloseWindow,
                Some(Some("Close safely")),
            ),
        ];
        assert_eq!(
            curated_rows(&bindings, 3, true),
            vec![(
                crate::ipc::server::format_chord(3, 0x57),
                "Close safely".into()
            )]
        );
    }

    #[test]
    fn pinned_hotkey_first_ordinary_bind_wins_over_later_mod_chord() {
        let bindings = [
            action_bind(0, 1, 0x48, crate::input::Action::FocusColumnLeft, None),
            action_bind(1, 3, 0x25, crate::input::Action::FocusColumnLeft, None),
        ];
        let rows = curated_rows(&bindings, 3, true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, crate::ipc::server::format_chord(1, 0x48));
    }

    #[test]
    fn pinned_hotkey_untitled_spawns_exclude_bare_and_unrelated_ctrl_chords() {
        let bindings: Vec<_> = [
            (0, 0x41, false),
            (2, 0x42, false),
            (3, 0x43, true),
            (8, 0x44, false),
        ]
        .iter()
        .enumerate()
        .map(|(id, (mods, key, literal_mod))| {
            let mut binding = action_bind(
                id as i32,
                *mods,
                *key,
                crate::input::Action::Spawn(vec![format!("launcher-{id}.exe")]),
                None,
            );
            binding.overlay_uses_compositor_mod = *literal_mod;
            binding
        })
        .collect();
        let rows = curated_rows(&bindings, 3, true);
        assert_eq!(
            rows.iter().map(|row| row.0.clone()).collect::<Vec<_>>(),
            vec![
                crate::ipc::server::format_chord(3, 0x43),
                crate::ipc::server::format_chord(8, 0x44),
            ]
        );
        // The same-mask literal Ctrl/Mod case is covered through the real
        // runtime builder and registered snapshot in message_loop's regression.
    }

    #[test]
    fn pinned_hotkey_custom_titled_bare_spawn_remains_visible_control() {
        let bindings = [action_bind(
            0,
            0,
            0x54,
            crate::input::Action::Spawn(vec!["wt.exe".into()]),
            Some(Some("Open terminal")),
        )];
        assert_eq!(
            curated_rows(&bindings, 3, true),
            vec![(
                crate::ipc::server::format_chord(0, 0x54),
                "Open terminal".into()
            )]
        );
    }

    #[test]
    fn pinned_hotkey_real_spawn_description_identifies_only_the_program() {
        let bindings = [mod_action_bind(
            0,
            0x54,
            crate::input::Action::Spawn(vec![
                "wt.exe".into(),
                "-d".into(),
                r"C:\My Projects".into(),
            ]),
            None,
        )];
        let rows = curated_rows(&bindings, 3, true);
        assert_eq!(rows.len(), 1);
        let description = &rows[0].1;
        assert!(description.starts_with("Spawn "), "{description}");
        assert!(description.contains("wt.exe"), "{description}");
        assert!(
            !description.contains("-d") && !description.contains("My Projects"),
            "{description}"
        );
        assert!(!description.contains("Spawn(["), "{description}");
    }

    #[test]
    fn pinned_hotkey_real_shell_spawn_description_identifies_only_command_head() {
        let bindings = [mod_action_bind(
            0,
            0x50,
            crate::input::Action::SpawnCmd("  pwsh.exe\t-NoProfile -Command exit".into()),
            None,
        )];
        let rows = curated_rows(&bindings, 3, true);
        assert_eq!(rows.len(), 1);
        let description = &rows[0].1;
        assert!(description.starts_with("Spawn "), "{description}");
        assert!(description.contains("pwsh.exe"), "{description}");
        assert!(
            !description.contains("-NoProfile") && !description.contains("-Command"),
            "{description}"
        );
        assert!(!description.contains("SpawnCmd("), "{description}");
    }

    #[test]
    fn pinned_hotkey_essential_variants_use_actual_parsed_actions() {
        for (command, args) in [
            ("quit", vec!["--skip-confirmation", "true"]),
            ("switch-preset-column-width", vec![]),
            ("switch-focus-between-floating-and-tiling", vec![]),
            ("move-column-to-workspace-down", vec!["--focus", "false"]),
            ("move-window-to-workspace-up", vec![]),
        ] {
            let binding = parsed_bind(0, 0x46, command, &args);
            let rows = curated_rows(&[binding], 3, true);
            assert_eq!(rows.len(), 1, "essential action was omitted: {command}");
            assert_eq!(
                rows[0].0,
                crate::ipc::server::format_chord(3, 0x46),
                "{command}"
            );
        }
    }

    #[test]
    fn pinned_hotkey_workspace_transfer_prefers_column_then_falls_back_to_window() {
        let window = parsed_bind(0, 0x57, "move-window-to-workspace-down", &[]);
        let column = parsed_bind(1, 0x43, "move-column-to-workspace-down", &[]);
        for (bindings, expected_key) in [(vec![window.clone(), column], 0x43), (vec![window], 0x57)]
        {
            let rows = curated_rows(&bindings, 3, true);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0, crate::ipc::server::format_chord(3, expected_key));
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
            true,
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.1 == "My special action"));
        assert!(rows.iter().any(|r| r.1 == "Look left"));
        assert!(!rows.iter().any(|r| r.1 == "Focus Column to the Left"));
    }

    #[test]
    fn first_ordinary_alias_is_selected_without_duplicate_action_rows() {
        let rows = curated_rows(
            &[
                bind(0, 1, 0x48, "FocusLeft", None),
                bind(1, 3, 0x25, "FocusColumnLeft", None),
                bind(2, 3, 0x25, "FocusColumnRight", None),
                bind(3, 3, 0x52, "SetColumnWidth(Fixed(100))", None),
                bind(4, 3, 0x54, "DebugLayout", None),
            ],
            3,
            true,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, crate::ipc::server::format_chord(1, 0x48));
        assert_eq!(rows[1].0, crate::ipc::server::format_chord(3, 0x25));
    }

    #[test]
    fn shipped_shell_launcher_overview_and_quit_are_important() {
        let rows = curated_rows(
            &[
                mod_action_bind(0, 0x0d, Action::SpawnCmd("start wt.exe".into()), None),
                bind(1, 3, 0x20, "OverviewToggle", None),
                bind(2, 7, 0x51, "Quit", None),
            ],
            3,
            true,
        );
        assert_eq!(
            rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
            vec!["Exit wiri", "Open the Overview", "Spawn start"]
        );
    }

    #[test]
    fn untitled_numbered_workspace_is_not_added_to_essential_actions() {
        let rows = curated_rows(
            &[
                mod_action_bind(0, 0x0d, Action::Spawn(vec!["wt.exe".into()]), None),
                mod_action_bind(1, 0x32, Action::FocusWorkspace(2), None),
            ],
            3,
            true,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "Spawn wt.exe");
    }

    #[test]
    fn registered_workspace_navigation_replaces_essential_placeholders() {
        // Registered bindings use the action enum's Debug names, not KDL spellings.
        let previous = format!("{:?}", crate::input::Action::FocusWorkspacePrevious);
        let next = format!("{:?}", crate::input::Action::FocusWorkspaceNext);
        let bindings = [
            bind(0, 3, 0x21, &previous, None),
            bind(1, 3, 0x22, &next, None),
        ];
        let bound = curated_rows(&bindings, 3, true);
        assert_eq!(
            bound,
            vec![
                (
                    crate::ipc::server::format_chord(3, 0x22),
                    "Switch Workspace Down".to_string(),
                ),
                (
                    crate::ipc::server::format_chord(3, 0x21),
                    "Switch Workspace Up".to_string(),
                ),
            ]
        );
        let shown = curated_rows(&bindings, 3, false);
        assert_eq!(shown.len(), ESSENTIALS.len());
        for expected in &bound {
            assert_eq!(shown.iter().filter(|row| row.1 == expected.1).count(), 1);
            assert!(shown.contains(expected));
        }
    }

    #[test]
    fn unbound_essentials_are_placeholders_unless_hide_not_bound() {
        // Only the close window bind exists: everything else is a "(not bound)" row.
        let bindings = [bind(0, 3, 0x51, "CloseWindow", None)];
        let shown = curated_rows(&bindings, 3, false);
        assert_eq!(shown[2].1, "Close Focused Window");
        assert_eq!(shown[2].0, crate::ipc::server::format_chord(3, 0x51));
        assert_eq!(shown.len(), ESSENTIALS.len());
        assert!(shown
            .iter()
            .filter(|row| row.1 != "Close Focused Window")
            .all(|row| row.0 == NOT_BOUND));
        assert!(shown.iter().any(|row| row.1 == "Open the Overview"));
        assert_eq!(
            shown
                .iter()
                .filter(|row| row.0 == NOT_BOUND)
                .map(|row| row.1.as_str())
                .collect::<Vec<_>>(),
            ESSENTIALS
                .iter()
                .map(|(_, text)| *text)
                .filter(|text| *text != "Close Focused Window")
                .collect::<Vec<_>>()
        );
        // `hotkey-overlay { hide-not-bound }` lists only what can actually be pressed.
        let hidden = curated_rows(&bindings, 3, true);
        assert_eq!(
            hidden,
            vec![(
                crate::ipc::server::format_chord(3, 0x51),
                "Close Focused Window".to_string()
            )]
        );
        assert!(!hidden.iter().any(|row| row.0 == NOT_BOUND));
        // An explicit title always wins, including over a placeholder.
        let titled = curated_rows(
            &[bind(
                0,
                3,
                0x46,
                "DebugLayout",
                Some(Some("My special action")),
            )],
            3,
            true,
        );
        assert_eq!(titled.len(), 1);
        assert_eq!(titled[0].1, "My special action");
    }

    // The live h80_curated_hotkeys_render gate (lane-p.ps1) binds each KDL action on a
    // Ctrl+Alt+Shift chord with `hotkey-overlay { hide-not-bound }`. These are the same
    // binds, parsed by the real action parser, and the sheet it painted.
    fn live_chord_bind(id: i32, key: u32, command: &str, args: &[&str]) -> RegisteredBinding {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        let action = crate::input::parse_action_flagged(command, &args, &[])
            .unwrap_or_else(|| panic!("live gate action must parse: {command} {args:?}"));
        // Ctrl+Alt+Shift is 7, and not a literal Mod/Super chord.
        action_bind(id, 7, key, action, None)
    }

    fn live_gate_rows(commands: &[(&str, &[&str])]) -> Vec<(String, String)> {
        let bindings: Vec<_> = commands
            .iter()
            .enumerate()
            .map(|(index, (command, args))| {
                live_chord_bind(index as i32, 0x41 + index as u32, command, args)
            })
            .collect();
        curated_rows(&bindings, 3, true)
    }

    // Pinned niri e19df12 ui/hotkey_overlay.rs:197-304 collect_actions (the 18 candidates, in
    // this order) and :458-494 action_name (their titles; only "niri" reads "wiri" here).
    #[test]
    fn pinned_hotkey_live_gate_binding_all_eighteen_curated_actions_lists_all_eighteen() {
        let curated: [(&str, &str); 18] = [
            ("show-hotkey-overlay", "Show Important Hotkeys"),
            ("quit", "Exit wiri"),
            ("close-window", "Close Focused Window"),
            ("focus-column-left", "Focus Column to the Left"),
            ("focus-column-right", "Focus Column to the Right"),
            ("move-column-left", "Move Column Left"),
            ("move-column-right", "Move Column Right"),
            ("focus-workspace-down", "Switch Workspace Down"),
            ("focus-workspace-up", "Switch Workspace Up"),
            (
                "move-column-to-workspace-down",
                "Move Column to Workspace Down",
            ),
            ("move-column-to-workspace-up", "Move Column to Workspace Up"),
            ("switch-preset-column-width", "Switch Preset Column Widths"),
            ("maximize-column", "Maximize Column"),
            (
                "consume-or-expel-window-left",
                "Consume or Expel Window Left",
            ),
            (
                "consume-or-expel-window-right",
                "Consume or Expel Window Right",
            ),
            (
                "toggle-window-floating",
                "Move Window Between Floating and Tiling",
            ),
            (
                "switch-focus-between-floating-and-tiling",
                "Switch Focus Between Floating and Tiling",
            ),
            ("toggle-overview", "Open the Overview"),
        ];
        // Registered as well, but not in niri's list: no row. A spawn would need a
        // Mod/Super chord (:281-296); focus-window-up/down and fullscreen-window never.
        let not_curated: [(&str, &[&str]); 4] = [
            ("spawn", &["notepad.exe"]),
            ("focus-window-up", &[]),
            ("focus-window-down", &[]),
            ("fullscreen-window", &[]),
        ];
        let mut commands: Vec<(&str, &[&str])> = Vec::new();
        for (command, _) in curated {
            commands.push((command, &[]));
        }
        commands.extend(not_curated);
        assert_eq!(commands.len(), 22);
        let rows = live_gate_rows(&commands);
        assert_eq!(
            rows.iter().map(|row| row.1.as_str()).collect::<Vec<_>>(),
            curated.iter().map(|(_, title)| *title).collect::<Vec<_>>()
        );
        // Each row names the chord of its own bind (A..R, in the order bound).
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(
                row.0,
                crate::ipc::server::format_chord(7, 0x41 + index as u32),
                "{}",
                row.1
            );
        }
    }

    // The sheet the aa6c884 live run measured: 18 binds of the pre-niri list ("Launch
    // application", focus-window-up/down, fullscreen-window and 14 curated ones). niri lists
    // only the 14 that are in its curated list, so 14 rows is the correct answer.
    #[test]
    fn pinned_hotkey_live_gate_old_pre_niri_bind_set_gives_fourteen_rows() {
        let binds: [(&str, &[&str]); 18] = [
            ("spawn", &["notepad.exe"]),
            ("close-window", &[]),
            ("focus-column-left", &[]),
            ("focus-column-right", &[]),
            ("focus-window-up", &[]),
            ("focus-window-down", &[]),
            ("move-column-left", &[]),
            ("move-column-right", &[]),
            ("consume-or-expel-window-left", &[]),
            ("consume-or-expel-window-right", &[]),
            ("maximize-column", &[]),
            ("fullscreen-window", &[]),
            ("toggle-window-floating", &[]),
            ("toggle-overview", &[]),
            ("focus-workspace-up", &[]),
            ("focus-workspace-down", &[]),
            ("show-hotkey-overlay", &[]),
            ("quit", &[]),
        ];
        let rows = live_gate_rows(&binds);
        assert_eq!(rows.len(), 14);
        for absent in ["Launch application", "Fullscreen window"] {
            assert!(!rows.iter().any(|row| row.1 == absent), "{absent}");
        }
        assert!(!rows.iter().any(|row| row.1.starts_with("Spawn")));
        assert!(!rows.iter().any(|row| row.1.contains("Focus window")));
    }
}
