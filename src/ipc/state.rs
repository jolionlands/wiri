// What event-stream clients are told about wiri's windows and workspaces, in niri's
// vocabulary (niri-ipc `Window`, `Workspace` and the events built from them).
//
// This module is pure (no Win32, no engine): the engine describes itself as an
// `IpcSnapshot` (`TilingEngine::ipc_snapshot`), `diff` turns the snapshot the clients
// were last told about and the next one into the events they need, and `replay` turns one
// snapshot into the burst a new subscriber gets first, so that it never has to ask for
// the state.
use std::collections::HashMap;

use super::messages::{IpcEvent, WindowIpc, WindowLayoutIpc, WorkspaceIpc};

/// Windows and workspaces as of one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IpcSnapshot {
    /// Every window wiri manages, by ascending id.
    pub windows: Vec<WindowIpc>,
    /// Every workspace of every monitor: monitors in the order of their device names, the
    /// workspaces of each from the top.
    pub workspaces: Vec<WorkspaceIpc>,
    pub overview_open: bool,
}

impl IpcSnapshot {
    /// The window that has the keyboard focus, if wiri manages it.
    pub fn focused_window(&self) -> Option<u64> {
        self.windows.iter().find(|w| w.is_focused).map(|w| w.id)
    }
}

/// What a new subscriber is told first: the workspaces, the windows and whether the
/// overview is open, as niri does.  The last config load follows (it is not part of the
/// snapshot).
pub fn replay(snapshot: &IpcSnapshot) -> Vec<IpcEvent> {
    vec![
        IpcEvent::WorkspacesChanged {
            workspaces: snapshot.workspaces.clone(),
        },
        IpcEvent::WindowsChanged {
            windows: snapshot.windows.clone(),
        },
        IpcEvent::OverviewOpenedOrClosed {
            is_open: snapshot.overview_open,
        },
    ]
}

/// The workspaces came or went, or changed their name, monitor or place: what makes niri
/// send the whole list again.
fn workspaces_restructured(prev: &[WorkspaceIpc], next: &[WorkspaceIpc]) -> bool {
    prev.len() != next.len()
        || prev.iter().zip(next).any(|(old, new)| {
            old.id != new.id
                || old.idx != new.idx
                || old.name != new.name
                || old.output != new.output
        })
}

/// Two windows that differ in nothing a client is told with `window_opened_or_changed`
/// (focus, urgency and the place in the strip have events of their own).
fn same_window(old: &WindowIpc, new: &WindowIpc) -> bool {
    old.title == new.title
        && old.app_id == new.app_id
        && old.pid == new.pid
        && old.workspace_id == new.workspace_id
        && old.is_floating == new.is_floating
        && old.class_name == new.class_name
        && old.output == new.output
        && old.is_fullscreen == new.is_fullscreen
        && old.is_maximized == new.is_maximized
        && old.is_minimized == new.is_minimized
}

/// The events that tell a client that knows `prev` what `next` looks like, in the order
/// niri sends them: workspaces, windows (opened or changed, closed, urgency, layouts,
/// focus), the overview.
pub fn diff(prev: &IpcSnapshot, next: &IpcSnapshot) -> Vec<IpcEvent> {
    let mut events = Vec::new();
    if prev == next {
        return events;
    }

    // ---- workspaces ----
    // As niri does: when workspaces came or went or changed in a way the other events cannot
    // say, the whole list is sent and that is all (it carries the flags too); otherwise one
    // event for each flag that changed.
    if workspaces_restructured(&prev.workspaces, &next.workspaces) {
        events.push(IpcEvent::WorkspacesChanged {
            workspaces: next.workspaces.clone(),
        });
    } else {
        for (old, new) in prev.workspaces.iter().zip(&next.workspaces) {
            if old.active_window_id != new.active_window_id {
                events.push(IpcEvent::WorkspaceActiveWindowChanged {
                    workspace_id: new.id,
                    active_window_id: new.active_window_id,
                });
            }
            if old.is_urgent != new.is_urgent {
                events.push(IpcEvent::WorkspaceUrgencyChanged {
                    id: new.id,
                    urgent: new.is_urgent,
                });
            }
            // The focus came to the workspace (it was activated, or the focus moved to its
            // monitor), or it became the active one of its monitor without the focus.
            if new.is_focused && !old.is_focused {
                events.push(IpcEvent::WorkspaceActivated {
                    id: new.id,
                    focused: true,
                });
            } else if new.is_active && !old.is_active {
                events.push(IpcEvent::WorkspaceActivated {
                    id: new.id,
                    focused: false,
                });
            }
        }
    }

    // ---- windows ----
    let old_windows: HashMap<u64, &WindowIpc> = prev.windows.iter().map(|w| (w.id, w)).collect();
    let new_ids: std::collections::HashSet<u64> = next.windows.iter().map(|w| w.id).collect();
    let mut urgency = Vec::new();
    let mut layouts: Vec<(u64, WindowLayoutIpc)> = Vec::new();
    for new in &next.windows {
        match old_windows.get(&new.id) {
            None => events.push(IpcEvent::WindowOpenedOrChanged {
                window: new.clone(),
            }),
            // As niri does: a window that is sent whole carries its urgency and its place in the
            // strip, so they are not told again for it.
            Some(old) if !same_window(old, new) => events.push(IpcEvent::WindowOpenedOrChanged {
                window: new.clone(),
            }),
            Some(old) => {
                if old.is_urgent != new.is_urgent {
                    urgency.push((new.id, new.is_urgent));
                }
                if old.layout != new.layout {
                    layouts.push((new.id, new.layout.clone()));
                }
            }
        }
    }
    for old in &prev.windows {
        if !new_ids.contains(&old.id) {
            events.push(IpcEvent::WindowClosed {
                window_hwnd: old.id as isize,
                id: old.id,
            });
        }
    }
    for (id, urgent) in urgency {
        events.push(IpcEvent::WindowUrgencyChanged { id, urgent });
    }
    if !layouts.is_empty() {
        events.push(IpcEvent::WindowLayoutsChanged { changes: layouts });
    }
    if prev.focused_window() != next.focused_window() {
        events.push(IpcEvent::WindowFocusChanged {
            id: next.focused_window(),
        });
    }

    // ---- the overview ----
    if prev.overview_open != next.overview_open {
        events.push(IpcEvent::OverviewOpenedOrClosed {
            is_open: next.overview_open,
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64) -> WindowIpc {
        WindowIpc {
            id,
            title: Some(format!("window {}", id)),
            app_id: Some("app.exe".into()),
            pid: Some(100 + id as u32),
            workspace_id: Some(1),
            is_focused: false,
            is_floating: false,
            is_urgent: false,
            layout: WindowLayoutIpc {
                pos_in_scrolling_layout: Some((id as usize, 1)),
            },
            class_name: "Class".into(),
            output: Some("\\\\.\\DISPLAY1".into()),
            is_fullscreen: false,
            is_maximized: false,
            is_minimized: false,
        }
    }

    fn workspace(id: u64, idx: u32, active: bool) -> WorkspaceIpc {
        WorkspaceIpc {
            id,
            idx,
            name: None,
            output: Some("\\\\.\\DISPLAY1".into()),
            is_urgent: false,
            is_active: active,
            is_focused: active,
            active_window_id: None,
        }
    }

    fn snapshot(windows: Vec<WindowIpc>, workspaces: Vec<WorkspaceIpc>) -> IpcSnapshot {
        IpcSnapshot {
            windows,
            workspaces,
            overview_open: false,
        }
    }

    fn types(events: &[IpcEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn nothing_changed_nothing_is_sent() {
        let s = snapshot(vec![window(1)], vec![workspace(10, 1, true)]);
        assert!(diff(&s, &s.clone()).is_empty());
    }

    #[test]
    fn a_new_subscriber_gets_the_workspaces_the_windows_and_the_overview_first() {
        let mut s = snapshot(vec![window(1), window(2)], vec![workspace(10, 1, true)]);
        s.overview_open = true;
        let events = replay(&s);
        assert_eq!(
            types(&events),
            [
                "workspaces_changed",
                "windows_changed",
                "overview_opened_or_closed"
            ]
        );
        match &events[1] {
            IpcEvent::WindowsChanged { windows } => assert_eq!(windows.len(), 2),
            other => panic!("unexpected {:?}", other),
        }
        match &events[2] {
            IpcEvent::OverviewOpenedOrClosed { is_open } => assert!(*is_open),
            other => panic!("unexpected {:?}", other),
        }
        // An empty desktop is replayed as empty lists, not as nothing.
        let empty = replay(&IpcSnapshot::default());
        assert_eq!(empty.len(), 3);
    }

    #[test]
    fn a_window_that_opens_is_sent_whole_and_then_the_focus_moves_to_it() {
        let prev = snapshot(vec![window(1)], vec![workspace(10, 1, true)]);
        let mut opened = window(2);
        opened.is_focused = true;
        let next = snapshot(
            vec![window(1), opened.clone()],
            vec![workspace(10, 1, true)],
        );
        let events = diff(&prev, &next);
        assert_eq!(
            types(&events),
            ["window_opened_or_changed", "window_focus_changed"]
        );
        match &events[0] {
            IpcEvent::WindowOpenedOrChanged { window } => assert_eq!(*window, opened),
            other => panic!("unexpected {:?}", other),
        }
        match &events[1] {
            IpcEvent::WindowFocusChanged { id } => assert_eq!(*id, Some(2)),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_window_that_closes_is_reported_by_id_and_by_handle() {
        let prev = snapshot(vec![window(1), window(2)], vec![]);
        let next = snapshot(vec![window(1)], vec![]);
        let events = diff(&prev, &next);
        assert_eq!(types(&events), ["window_closed"]);
        match &events[0] {
            IpcEvent::WindowClosed { window_hwnd, id } => {
                assert_eq!((*window_hwnd, *id), (2, 2));
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn the_focus_going_away_is_a_focus_change_to_nothing() {
        let mut focused = window(1);
        focused.is_focused = true;
        let prev = snapshot(vec![focused.clone()], vec![]);
        let mut next_window = focused;
        next_window.is_focused = false;
        let next = snapshot(vec![next_window], vec![]);
        let events = diff(&prev, &next);
        assert_eq!(types(&events), ["window_focus_changed"]);
        match &events[0] {
            IpcEvent::WindowFocusChanged { id } => assert_eq!(*id, None),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_title_change_is_a_changed_window_and_nothing_else() {
        let prev = snapshot(vec![window(1)], vec![]);
        let mut changed = window(1);
        changed.title = Some("a new title".into());
        let next = snapshot(vec![changed.clone()], vec![]);
        let events = diff(&prev, &next);
        assert_eq!(types(&events), ["window_opened_or_changed"]);
        match &events[0] {
            IpcEvent::WindowOpenedOrChanged { window } => {
                assert_eq!(window.title.as_deref(), Some("a new title"));
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn floating_fullscreen_maximized_and_workspace_changes_are_changed_windows() {
        let prev = snapshot(vec![window(1)], vec![]);
        for edit in [
            (|w: &mut WindowIpc| w.is_floating = true) as fn(&mut WindowIpc),
            |w| w.is_fullscreen = true,
            |w| w.is_maximized = true,
            |w| w.is_minimized = true,
            |w| w.workspace_id = Some(2),
            |w| w.app_id = None,
        ] {
            let mut changed = window(1);
            edit(&mut changed);
            let events = diff(&prev, &snapshot(vec![changed], vec![]));
            assert_eq!(types(&events), ["window_opened_or_changed"]);
        }
    }

    #[test]
    fn urgency_and_the_place_in_the_strip_have_events_of_their_own() {
        let prev = snapshot(vec![window(1), window(2)], vec![]);
        let mut urgent = window(1);
        urgent.is_urgent = true;
        let mut moved = window(2);
        moved.layout.pos_in_scrolling_layout = Some((1, 2));
        let events = diff(&prev, &snapshot(vec![urgent, moved.clone()], vec![]));
        assert_eq!(
            types(&events),
            ["window_urgency_changed", "window_layouts_changed"]
        );
        match &events[0] {
            IpcEvent::WindowUrgencyChanged { id, urgent } => assert_eq!((*id, *urgent), (1, true)),
            other => panic!("unexpected {:?}", other),
        }
        match &events[1] {
            IpcEvent::WindowLayoutsChanged { changes } => {
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].0, 2);
                assert_eq!(changes[0].1, moved.layout);
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_window_that_is_sent_whole_is_not_told_again_about_its_layout_or_urgency() {
        let prev = snapshot(vec![window(1), window(2)], vec![]);
        let mut changed = window(1);
        changed.is_floating = true;
        changed.is_urgent = true;
        changed.layout.pos_in_scrolling_layout = None;
        let mut moved_up = window(2);
        moved_up.layout.pos_in_scrolling_layout = Some((1, 1));
        let events = diff(&prev, &snapshot(vec![changed, moved_up], vec![]));
        // The window that changed carries its urgency and its place; the other one moved.
        assert_eq!(
            types(&events),
            ["window_opened_or_changed", "window_layouts_changed"]
        );
        match &events[1] {
            IpcEvent::WindowLayoutsChanged { changes } => {
                assert_eq!(changes.iter().map(|c| c.0).collect::<Vec<_>>(), vec![2]);
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_new_workspace_sends_the_whole_list_once() {
        let prev = snapshot(vec![], vec![workspace(10, 1, true)]);
        let next = snapshot(
            vec![],
            vec![workspace(10, 1, true), workspace(11, 2, false)],
        );
        let events = diff(&prev, &next);
        assert_eq!(types(&events), ["workspaces_changed"]);
        match &events[0] {
            IpcEvent::WorkspacesChanged { workspaces } => assert_eq!(workspaces.len(), 2),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_renamed_or_renumbered_workspace_sends_the_whole_list() {
        let prev = snapshot(
            vec![],
            vec![workspace(10, 1, true), workspace(11, 2, false)],
        );
        let mut renamed = prev.clone();
        renamed.workspaces[1].name = Some("web".into());
        assert_eq!(types(&diff(&prev, &renamed)), ["workspaces_changed"]);
        let mut renumbered = prev.clone();
        renumbered.workspaces[1].idx = 3;
        assert_eq!(types(&diff(&prev, &renumbered)), ["workspaces_changed"]);
        let mut moved = prev.clone();
        moved.workspaces[1].output = Some("\\\\.\\DISPLAY2".into());
        assert_eq!(types(&diff(&prev, &moved)), ["workspaces_changed"]);
    }

    #[test]
    fn switching_workspace_activates_the_new_one_as_the_focused() {
        let prev = snapshot(
            vec![],
            vec![workspace(10, 1, true), workspace(11, 2, false)],
        );
        let next = snapshot(
            vec![],
            vec![workspace(10, 1, false), workspace(11, 2, true)],
        );
        let events = diff(&prev, &next);
        assert_eq!(types(&events), ["workspace_activated"]);
        match &events[0] {
            IpcEvent::WorkspaceActivated { id, focused } => assert_eq!((*id, *focused), (11, true)),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn the_focus_moving_to_another_monitor_activates_its_workspace_as_focused() {
        // Two monitors, each with an active workspace; the focus moves from the first to the
        // second.  Nothing was activated, but the second workspace is the focused one now.
        let mut a = workspace(10, 1, true);
        let mut b = workspace(20, 1, true);
        b.output = Some("\\\\.\\DISPLAY2".into());
        b.is_focused = false;
        let prev = snapshot(vec![], vec![a.clone(), b.clone()]);
        a.is_focused = false;
        b.is_focused = true;
        let next = snapshot(vec![], vec![a, b]);
        let events = diff(&prev, &next);
        assert_eq!(types(&events), ["workspace_activated"]);
        match &events[0] {
            IpcEvent::WorkspaceActivated { id, focused } => assert_eq!((*id, *focused), (20, true)),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_workspace_that_is_activated_on_another_monitor_is_not_the_focused_one() {
        // The first monitor has the focus; the second monitor's second workspace is
        // activated.
        let a = workspace(10, 1, true);
        let mut b = workspace(20, 1, true);
        b.output = Some("\\\\.\\DISPLAY2".into());
        b.is_focused = false;
        let mut b2 = workspace(21, 2, false);
        b2.output = b.output.clone();
        let prev = snapshot(vec![], vec![a.clone(), b.clone(), b2.clone()]);
        b.is_active = false;
        b2.is_active = true;
        let next = snapshot(vec![], vec![a, b, b2]);
        let events = diff(&prev, &next);
        assert_eq!(types(&events), ["workspace_activated"]);
        match &events[0] {
            IpcEvent::WorkspaceActivated { id, focused } => {
                assert_eq!((*id, *focused), (21, false))
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_workspace_tells_when_its_active_window_or_its_urgency_changes() {
        let prev = snapshot(vec![], vec![workspace(10, 1, true)]);
        let mut next = prev.clone();
        next.workspaces[0].active_window_id = Some(7);
        next.workspaces[0].is_urgent = true;
        let events = diff(&prev, &next);
        assert_eq!(
            types(&events),
            [
                "workspace_active_window_changed",
                "workspace_urgency_changed"
            ]
        );
        match &events[0] {
            IpcEvent::WorkspaceActiveWindowChanged {
                workspace_id,
                active_window_id,
            } => assert_eq!((*workspace_id, *active_window_id), (10, Some(7))),
            other => panic!("unexpected {:?}", other),
        }
        match &events[1] {
            IpcEvent::WorkspaceUrgencyChanged { id, urgent } => {
                assert_eq!((*id, *urgent), (10, true))
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn the_overview_opening_and_closing_is_told_once_each() {
        let closed = snapshot(vec![window(1)], vec![]);
        let mut open = closed.clone();
        open.overview_open = true;
        let events = diff(&closed, &open);
        assert_eq!(types(&events), ["overview_opened_or_closed"]);
        match &events[0] {
            IpcEvent::OverviewOpenedOrClosed { is_open } => assert!(*is_open),
            other => panic!("unexpected {:?}", other),
        }
        let events = diff(&open, &closed);
        match &events[0] {
            IpcEvent::OverviewOpenedOrClosed { is_open } => assert!(!*is_open),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_new_window_on_a_new_workspace_is_told_after_the_workspaces() {
        let prev = snapshot(vec![], vec![workspace(10, 1, true)]);
        let mut w = window(5);
        w.is_focused = true;
        w.workspace_id = Some(11);
        let next = snapshot(
            vec![w],
            vec![workspace(10, 1, false), workspace(11, 2, true)],
        );
        let events = diff(&prev, &next);
        // The list carries the new workspace and who is active: no event of its own for it.
        assert_eq!(
            types(&events),
            [
                "workspaces_changed",
                "window_opened_or_changed",
                "window_focus_changed"
            ],
            "a window names a workspace that the client must know by then"
        );
    }

    #[test]
    fn when_the_whole_list_is_sent_the_flag_events_are_not() {
        // niri: a restructured list is the whole story, flags included.
        let mut a = workspace(10, 1, true);
        let prev = snapshot(vec![], vec![a.clone()]);
        a.active_window_id = Some(3);
        a.is_urgent = true;
        let next = snapshot(vec![], vec![a, workspace(11, 2, false)]);
        assert_eq!(types(&diff(&prev, &next)), ["workspaces_changed"]);
    }

    #[test]
    fn replaying_the_state_and_applying_the_deltas_gives_the_same_state_as_the_snapshot() {
        // What a client does: start from the replay, apply every event.  Here only the
        // window list and the focus are tracked, which is what the deltas are about.
        let a = snapshot(vec![window(1), window(2)], vec![workspace(10, 1, true)]);
        let mut focused = window(2);
        focused.is_focused = true;
        let b = snapshot(
            vec![window(1), focused, window(3)],
            vec![workspace(10, 1, true)],
        );
        let mut known: Vec<WindowIpc> = Vec::new();
        for event in replay(&a).into_iter().chain(diff(&a, &b)) {
            match event {
                IpcEvent::WindowsChanged { windows } => known = windows,
                IpcEvent::WindowOpenedOrChanged { window } => {
                    known.retain(|w| w.id != window.id);
                    if window.is_focused {
                        for w in known.iter_mut() {
                            w.is_focused = false;
                        }
                    }
                    known.push(window);
                }
                IpcEvent::WindowClosed { id, .. } => known.retain(|w| w.id != id),
                IpcEvent::WindowFocusChanged { id } => {
                    for w in known.iter_mut() {
                        w.is_focused = Some(w.id) == id;
                    }
                }
                _ => {}
            }
        }
        known.sort_by_key(|w| w.id);
        assert_eq!(known, b.windows);
    }
}
