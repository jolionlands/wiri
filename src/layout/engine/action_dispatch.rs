//! Shared action adapters: hotkeys and typed RunAction IPC enter the same engine paths.
use super::*;
use crate::input::{MonitorAction, MonitorTarget, WorkspaceAction};

impl TilingEngine {
    pub fn workspace_layout_action(&mut self, action: &WorkspaceAction, backend: &BackendHandle) {
        match action {
            WorkspaceAction::Focus { reference } => self.focus_workspace_ref(reference, backend),
            WorkspaceAction::MoveWindow {
                window,
                reference,
                focus,
            } => self.move_window_to_workspace_ref(*window, reference.clone(), *focus, backend),
            WorkspaceAction::MoveWindowRelative {
                window,
                down,
                focus,
            } => self.move_window_to_workspace_relative(*window, *down, *focus, backend),
            WorkspaceAction::MoveColumn { reference, focus } => {
                self.move_column_to_workspace_ref(reference.clone(), *focus, backend)
            }
            WorkspaceAction::MoveColumnRelative { down, focus } => {
                self.move_column_to_workspace_relative(*down, *focus, backend)
            }
            WorkspaceAction::MoveToIndex { index, workspace } => {
                self.move_workspace_to_index(*index, workspace.clone(), backend)
            }
            WorkspaceAction::SetName { name, workspace } => {
                self.set_workspace_name(name, workspace.clone(), backend)
            }
            WorkspaceAction::UnsetName { workspace } => {
                self.unset_workspace_name(workspace.clone(), backend)
            }
        }
    }

    pub fn monitor_layout_action(&mut self, action: &MonitorAction, backend: &BackendHandle) {
        let target = |target: &MonitorTarget| match target {
            MonitorTarget::Left => monitor_actions::MonitorTarget::Left,
            MonitorTarget::Right => monitor_actions::MonitorTarget::Right,
            MonitorTarget::Up => monitor_actions::MonitorTarget::Up,
            MonitorTarget::Down => monitor_actions::MonitorTarget::Down,
            MonitorTarget::Previous => monitor_actions::MonitorTarget::Previous,
            MonitorTarget::Next => monitor_actions::MonitorTarget::Next,
            MonitorTarget::Name(name) => monitor_actions::MonitorTarget::Name(name.clone()),
        };
        match action {
            MonitorAction::Focus(to) => self.focus_monitor_action(&target(to), backend),
            MonitorAction::MoveWindow { window, target: to } => {
                self.move_window_to_monitor_action(*window, &target(to), backend)
            }
            MonitorAction::MoveColumn(to) => {
                self.move_column_to_monitor_action(&target(to), backend)
            }
            MonitorAction::MoveWorkspace {
                workspace,
                target: to,
            } => self.move_workspace_to_monitor_action(workspace.clone(), &target(to), backend),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WindowState;
    use crate::input::{parse_action_name, Action};
    use crate::ipc::{messages::IpcEvent, state};

    fn fixture() -> (TilingEngine, BackendHandle) {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let output = OutputId::from_name("dispatch-main");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 800),
        );
        engine.set_output_name(output, "dispatch-main");
        engine.rename_workspace(0, "source").unwrap();
        for id in 100..103 {
            engine.add_window(
                WindowInfo {
                    hwnd: id,
                    title: id.to_string(),
                    class_name: "fixture".into(),
                    process_id: 1,
                    bounds: Rect::new(0, 0, 400, 400),
                    state: WindowState::Normal,
                    is_visible: true,
                },
                &backend,
            );
        }
        engine.sync_os_foreground(WindowId::new(100), &backend);
        (engine, backend)
    }

    fn run(engine: &mut TilingEngine, backend: &BackendHandle, name: &str, words: &[&str]) {
        let args = words
            .iter()
            .map(|word| word.to_string())
            .collect::<Vec<_>>();
        match parse_action_name(name, &args).unwrap_or_else(|| panic!("cannot parse {name}")) {
            Action::LayoutFocus(action) => engine.focus_layout_action(&action, backend),
            Action::WorkspaceAction(action) => engine.workspace_layout_action(&action, backend),
            Action::MonitorAction(action) => engine.monitor_layout_action(&action, backend),
            other => panic!("expected structured action, got {other:?}"),
        }
    }

    #[test]
    fn parsed_focus_uses_existing_niri_focus_events() {
        let (mut engine, backend) = fixture();
        let before = engine.ipc_snapshot();
        run(&mut engine, &backend, "focus-column", &["2"]);
        // WinEvent confirmation, not an invented OS foreground change.
        engine.sync_os_foreground(WindowId::new(101), &backend);
        let events = state::diff(&before, &engine.ipc_snapshot());
        assert!(events
            .iter()
            .any(|e| matches!(e, IpcEvent::WindowFocusChanged { id: Some(101) })));
        assert!(events.iter().any(|e| matches!(
            e,
            IpcEvent::WorkspaceActiveWindowChanged {
                active_window_id: Some(101),
                ..
            }
        )));
    }

    #[test]
    fn parsed_workspace_transfer_keeps_ids_and_emits_existing_change_events() {
        let (mut engine, backend) = fixture();
        engine.switch_workspace(1, &backend);
        engine.rename_workspace(1, "target").unwrap();
        engine.switch_workspace(0, &backend);
        let before = engine.ipc_snapshot();
        let source_id = before
            .workspaces
            .iter()
            .find(|w| w.name.as_deref() == Some("source"))
            .unwrap()
            .id;
        let target_id = before
            .workspaces
            .iter()
            .find(|w| w.name.as_deref() == Some("target"))
            .unwrap()
            .id;
        run(
            &mut engine,
            &backend,
            "move-window-to-workspace",
            &["target", "--id", "101", "--focus", "false"],
        );
        let after = engine.ipc_snapshot();
        assert_eq!(
            after
                .windows
                .iter()
                .find(|w| w.id == 101)
                .unwrap()
                .workspace_id,
            Some(target_id)
        );
        assert_eq!(
            after
                .windows
                .iter()
                .find(|w| w.id == 100)
                .unwrap()
                .workspace_id,
            Some(source_id)
        );
        assert_eq!(
            after.workspaces.iter().find(|w| w.is_focused).unwrap().id,
            source_id
        );
        let events = state::diff(&before, &after);
        assert!(events.iter().any(|e| matches!(e,IpcEvent::WindowOpenedOrChanged { window } if window.id==101 && window.workspace_id==Some(target_id))));
        assert!(!events
            .iter()
            .any(|e| matches!(e, IpcEvent::WindowClosed { .. })));
    }

    #[test]
    fn parsed_workspace_reorder_preserves_identity_and_only_restructures_workspace_list() {
        let (mut engine, backend) = fixture();
        engine.switch_workspace(1, &backend);
        engine.rename_workspace(1, "target").unwrap();
        engine.switch_workspace(0, &backend);
        let before = engine.ipc_snapshot();
        let source = before
            .workspaces
            .iter()
            .find(|w| w.name.as_deref() == Some("source"))
            .unwrap();
        run(&mut engine, &backend, "move-workspace-to-index", &["2"]);
        let after = engine.ipc_snapshot();
        let moved = after.workspaces.iter().find(|w| w.id == source.id).unwrap();
        assert_eq!(moved.name, source.name);
        assert_eq!(moved.idx, 2);
        assert_eq!(after.windows, before.windows);
        assert!(state::diff(&before, &after)
            .iter()
            .any(|e| matches!(e, IpcEvent::WorkspacesChanged { .. })));
    }

    #[test]
    fn monitor_actions_match_cached_friendly_and_edid_names_without_queries() {
        let (mut engine, backend) = fixture();
        let second = OutputId::from_name("dispatch-second");
        engine.register_monitor(
            second,
            Rect::new(1200, 0, 1200, 800),
            Rect::new(1200, 0, 1200, 800),
        );
        engine.set_output_name(second, "dispatch-second");
        engine.set_output_identities(vec![crate::backend::outputs::OutputIdentity {
            connector: "dispatch-second".into(),
            make: "DEL".into(),
            model: "TestPanel".into(),
            serial: "S123".into(),
            friendly_name: "Study Screen".into(),
            device_path: String::new(),
        }]);
        for name in ["Dell TestPanel S123", "del testpanel", "study screen"] {
            run(&mut engine, &backend, "focus-monitor", &[name]);
            assert_eq!(engine.monitors.focused_id(), Some(second));
        }
        run(
            &mut engine,
            &backend,
            "focus-monitor",
            &["a nonexistent output"],
        );
        assert_eq!(engine.monitors.focused_id(), Some(second));
    }

    #[test]
    fn parsed_monitor_workspace_transfer_reparents_identity_without_closing_windows() {
        let (mut engine, backend) = fixture();
        let second = OutputId::from_name("dispatch-second");
        engine.register_monitor(
            second,
            Rect::new(1200, 0, 1200, 800),
            Rect::new(1200, 0, 1200, 800),
        );
        engine.set_output_name(second, "dispatch-second");
        let before = engine.ipc_snapshot();
        let source_id = before
            .workspaces
            .iter()
            .find(|w| w.name.as_deref() == Some("source"))
            .unwrap()
            .id;
        run(
            &mut engine,
            &backend,
            "move-workspace-to-monitor",
            &["dispatch-second"],
        );
        let after = engine.ipc_snapshot();
        let moved = after.workspaces.iter().find(|w| w.id == source_id).unwrap();
        assert_eq!(moved.output.as_deref(), Some("dispatch-second"));
        assert!(moved.is_focused);
        assert!(after
            .windows
            .iter()
            .all(|w| w.workspace_id == Some(source_id)));
        let events = state::diff(&before, &after);
        assert!(events
            .iter()
            .any(|e| matches!(e, IpcEvent::WorkspacesChanged { .. })));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, IpcEvent::WindowOpenedOrChanged { .. }))
                .count(),
            3
        );
        assert!(!events
            .iter()
            .any(|e| matches!(e, IpcEvent::WindowClosed { .. })));
        let mut ids = after.workspaces.iter().map(|w| w.id).collect::<Vec<_>>();
        let len = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), len);
    }
}
