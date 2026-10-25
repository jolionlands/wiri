//! The engine described for event-stream clients: niri's `Window` and `Workspace` objects
//! (`crate::ipc::state` turns two such descriptions into events).
//!
//! Built on demand and only while somebody listens, from the state the engine keeps anyway:
//! no layout pass, no Win32 call, no write lock.

use super::*;
use crate::ipc::messages::{WindowIpc, WindowLayoutIpc, WorkspaceIpc};
use crate::ipc::state::IpcSnapshot;
use std::collections::HashSet;

/// Where a window is: its monitor, its workspace and, when it sits in a strip, its column and
/// tile (0-based).
#[derive(Clone, Copy)]
struct Place {
    output: OutputId,
    workspace: i32,
    slot: Option<(usize, usize)>,
}

/// The workspace ids event-stream clients see are unique among all monitors and small enough
/// for a JavaScript number (below 2^53): a tag of the monitor in the upper bits and the
/// workspace id of the monitor in the lower 32 bits.
fn workspace_ipc_id(tag: u64, workspace: i32) -> u64 {
    (tag << 32) | u64::from(workspace as u32)
}

/// A tag from 1 to 0xFFFFF for each id, in the order given.  An id that would get a tag
/// that is taken gets the next free one.
fn tags_for(ids: &[u64]) -> Vec<u64> {
    const TAGS: u64 = 0xF_FFFF;
    let mut used = HashSet::new();
    ids.iter()
        .map(|id| {
            let mut tag = id % TAGS + 1;
            while !used.insert(tag) {
                tag = tag % TAGS + 1;
            }
            tag
        })
        .collect()
}

/// The tag of each monitor, taken from its id.  The monitors are given in the order of
/// their names, so that it is always the same one that moves on when two have the same tag.
fn output_tags(outputs: &[OutputId]) -> HashMap<OutputId, u64> {
    let ids: Vec<u64> = outputs.iter().map(|o| o.as_u64()).collect();
    outputs.iter().copied().zip(tags_for(&ids)).collect()
}

impl TilingEngine {
    /// The device name of a monitor as event-stream clients see it (`\\.\DISPLAY1`); the id
    /// when the engine was never told.
    fn ipc_output_name(&self, output: OutputId) -> String {
        match self.output_names.get(&output) {
            Some(name) if !name.is_empty() => name.clone(),
            _ => format!("output-{:016x}", output.as_u64()),
        }
    }

    /// Where every managed window is: the strips, then the floating windows, then the minimized
    /// ones.
    fn ipc_places(&self) -> HashMap<WindowId, Place> {
        let mut places = HashMap::new();
        for (output, monitor) in self.monitors.iter() {
            for (&workspace, strip) in monitor.workspaces.iter() {
                for (column_idx, column) in strip.columns.iter().enumerate() {
                    for (tile_idx, tile) in column.tiles.iter().enumerate() {
                        places.insert(
                            tile.window_id,
                            Place {
                                output: *output,
                                workspace,
                                slot: Some((column_idx, tile_idx)),
                            },
                        );
                    }
                }
            }
        }
        for window in self.tiled_windows.keys() {
            if places.contains_key(window) {
                continue;
            }
            if self.floating_windows.contains(window) {
                if let Some((output, workspace)) = self.float_location(*window) {
                    places.insert(
                        *window,
                        Place {
                            output,
                            workspace,
                            slot: None,
                        },
                    );
                    continue;
                }
            }
            if let Some(minimized) = self.minimized.get(window) {
                places.insert(
                    *window,
                    Place {
                        output: minimized.output,
                        workspace: minimized.workspace,
                        slot: None,
                    },
                );
            }
        }
        places
    }

    /// The window that is the active one of a workspace (niri's `active_window_id`): the
    /// monitor's focused window when the workspace is the active one and has it, otherwise
    /// the window that was focused most recently on the workspace, otherwise its first tile.
    fn ipc_active_window(
        &self,
        monitor: &Monitor,
        workspace: i32,
        places: &HashMap<WindowId, Place>,
    ) -> Option<WindowId> {
        let on_workspace = |window: &WindowId| {
            places.get(window).map_or(false, |place| {
                place.output == monitor.output_id && place.workspace == workspace
            })
        };
        if monitor.active_workspace == workspace {
            if let Some(focused) = monitor.focus_window.filter(|w| on_workspace(w)) {
                return Some(focused);
            }
        }
        if let Some(recent) = monitor
            .focus_ring
            .iter_mru()
            .find(|w| self.tiled_windows.contains_key(w) && on_workspace(w))
        {
            return Some(recent);
        }
        monitor
            .workspaces
            .get(&workspace)?
            .columns
            .iter()
            .find_map(|column| column.tiles.first().map(|tile| tile.window_id))
    }

    /// The windows and workspaces as event-stream clients see them (niri's `Window` and
    /// `Workspace`).  `app_id` is filled in from what the window rules already know; the
    /// caller asks the system for the windows it is still missing for.
    pub fn ipc_snapshot(&self) -> IpcSnapshot {
        // Monitors in the order of their device names, so that the lists are the same from
        // one snapshot to the next.
        let mut outputs: Vec<(String, OutputId)> = self
            .monitors
            .iter()
            .map(|(output, _)| (self.ipc_output_name(*output), *output))
            .collect();
        outputs.sort_by(|a, b| (&a.0, a.1.as_u64()).cmp(&(&b.0, b.1.as_u64())));
        let tags = output_tags(&outputs.iter().map(|(_, o)| *o).collect::<Vec<_>>());
        let names: HashMap<OutputId, &String> = outputs.iter().map(|(n, o)| (*o, n)).collect();

        let places = self.ipc_places();
        let focused_output = self.monitors.focused_id();
        // The keyboard focus is the OS foreground window, when wiri manages it.
        let focused_window = self
            .os_foreground
            .filter(|window| self.tiled_windows.contains_key(window));

        // ---- workspaces ----
        let mut urgent_workspaces: HashSet<(OutputId, i32)> = HashSet::new();
        for window in &self.urgent_windows {
            if let Some(place) = places.get(window) {
                urgent_workspaces.insert((place.output, place.workspace));
            }
        }
        let mut workspaces = Vec::new();
        for (_, output) in &outputs {
            let Some(monitor) = self.monitors.get(output) else {
                continue;
            };
            let tag = tags[output];
            for workspace in monitor.workspace_ids() {
                let is_active = monitor.active_workspace == workspace;
                workspaces.push(WorkspaceIpc {
                    id: workspace_ipc_id(tag, workspace),
                    idx: monitor.workspace_number_of(workspace).unwrap_or(1).max(1) as u32,
                    name: self.workspace_name(workspace),
                    output: Some(names[output].clone()),
                    is_urgent: urgent_workspaces.contains(&(*output, workspace)),
                    is_active,
                    is_focused: is_active && focused_output == Some(*output),
                    active_window_id: self
                        .ipc_active_window(monitor, workspace, &places)
                        .map(|window| window.as_isize() as u64),
                });
            }
        }

        // ---- windows ----
        let mut windows: Vec<WindowIpc> = Vec::with_capacity(self.tiled_windows.len());
        for (window, info) in &self.tiled_windows {
            let place = places.get(window);
            windows.push(WindowIpc {
                id: window.as_isize() as u64,
                title: (!info.title.is_empty()).then(|| info.title.clone()),
                app_id: self
                    .rule_state
                    .get(window)
                    .and_then(|state| state.process_name.clone()),
                pid: (info.process_id != 0).then_some(info.process_id),
                // A floating window of a monitor that is away has no workspace to name.
                workspace_id: place.and_then(|p| {
                    tags.get(&p.output)
                        .map(|tag| workspace_ipc_id(*tag, p.workspace))
                }),
                is_focused: focused_window == Some(*window),
                is_floating: self.floating_windows.contains(window),
                is_urgent: self.urgent_windows.contains(window),
                layout: WindowLayoutIpc {
                    pos_in_scrolling_layout: place
                        .and_then(|p| p.slot)
                        .map(|(column, tile)| (column + 1, tile + 1)),
                },
                class_name: info.class_name.clone(),
                output: place.and_then(|p| names.get(&p.output).map(|name| (*name).clone())),
                is_fullscreen: self.fullscreen_windows.contains(window),
                is_maximized: self.maximized_edges.contains(window),
                is_minimized: self.minimized.contains_key(window),
            });
        }
        windows.sort_by_key(|w| w.id);

        IpcSnapshot {
            windows,
            workspaces,
            overview_open: self.overview.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WindowState;

    fn make_engine() -> TilingEngine {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        engine.register_monitor(
            OutputId::from_name("TestMonitor"),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        engine
    }

    fn make_window(hwnd: isize) -> WindowInfo {
        WindowInfo {
            hwnd,
            title: format!("Window {}", hwnd),
            class_name: "TestClass".to_string(),
            process_id: 1000 + hwnd as u32,
            bounds: Rect::new(0, 0, 800, 600),
            state: WindowState::Normal,
            is_visible: true,
        }
    }

    fn engine_with(ids: &[isize]) -> (TilingEngine, BackendHandle) {
        let backend = BackendHandle::default_for_test();
        let mut engine = make_engine();
        for id in ids {
            engine.add_window(make_window(*id), &backend);
        }
        (engine, backend)
    }

    fn window_of(snapshot: &IpcSnapshot, id: u64) -> &WindowIpc {
        snapshot
            .windows
            .iter()
            .find(|w| w.id == id)
            .unwrap_or_else(|| panic!("window {} is not in the snapshot", id))
    }

    #[test]
    fn a_window_is_described_the_way_niri_describes_one() {
        let (mut engine, backend) = engine_with(&[100, 101, 102]);
        engine.set_output_name(OutputId::from_name("TestMonitor"), "\\\\.\\DISPLAY1");
        engine.sync_os_foreground(WindowId::new(101), &backend);
        let snapshot = engine.ipc_snapshot();
        assert_eq!(
            snapshot.windows.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![100, 101, 102],
            "ordered by id"
        );
        let w = window_of(&snapshot, 101);
        assert_eq!(w.title.as_deref(), Some("Window 101"));
        assert_eq!(w.pid, Some(1101));
        assert_eq!(w.class_name, "TestClass");
        assert_eq!(w.output.as_deref(), Some("\\\\.\\DISPLAY1"));
        assert!(w.is_focused);
        assert!(!window_of(&snapshot, 100).is_focused);
        assert!(
            !w.is_floating && !w.is_urgent && !w.is_fullscreen && !w.is_maximized && !w.is_minimized
        );
        // One column each, counted from 1.
        assert_eq!(w.layout.pos_in_scrolling_layout, Some((2, 1)));
        assert_eq!(window_of(&snapshot, 100).layout.pos_in_scrolling_layout, Some((1, 1)));
        assert_eq!(window_of(&snapshot, 102).layout.pos_in_scrolling_layout, Some((3, 1)));
        // All on the same workspace, which exists in the list.
        let workspace = window_of(&snapshot, 100).workspace_id.expect("a workspace");
        assert_eq!(w.workspace_id, Some(workspace));
        assert!(snapshot.workspaces.iter().any(|ws| ws.id == workspace));
    }

    #[test]
    fn the_keyboard_focus_is_the_os_foreground_window_when_wiri_manages_it() {
        let (mut engine, backend) = engine_with(&[100, 101]);
        assert_eq!(engine.ipc_snapshot().focused_window(), None, "no report yet");
        engine.sync_os_foreground(WindowId::new(100), &backend);
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
        // A window wiri does not manage (the Start menu) has the focus: no managed window has.
        engine.sync_os_foreground(WindowId::new(999), &backend);
        assert_eq!(engine.ipc_snapshot().focused_window(), None);
        engine.sync_os_foreground(WindowId::new(101), &backend);
        engine.remove_window(WindowId::new(101), &backend);
        assert_eq!(engine.ipc_snapshot().focused_window(), None, "its window is gone");
    }

    #[test]
    fn workspaces_have_a_number_a_name_and_the_flags_of_niri() {
        let (mut engine, backend) = engine_with(&[100]);
        engine.set_output_name(OutputId::from_name("TestMonitor"), "\\\\.\\DISPLAY1");
        engine.switch_workspace(1, &backend);
        engine.add_window(make_window(101), &backend);
        engine.rename_workspace(1, "web").unwrap();
        engine.sync_os_foreground(WindowId::new(101), &backend);
        let snapshot = engine.ipc_snapshot();
        // Two with windows, and the empty one below them that niri (and wiri) always keep.
        assert_eq!(snapshot.workspaces.len(), 3);
        let trailing = &snapshot.workspaces[2];
        assert_eq!(trailing.idx, 3);
        assert!(!trailing.is_active && !trailing.is_focused && trailing.active_window_id.is_none());
        let (first, second) = (&snapshot.workspaces[0], &snapshot.workspaces[1]);
        assert_eq!((first.idx, second.idx), (1, 2));
        assert_eq!(first.name, None);
        assert_eq!(second.name.as_deref(), Some("web"));
        assert_eq!(second.output.as_deref(), Some("\\\\.\\DISPLAY1"));
        assert!(!first.is_active && !first.is_focused);
        assert!(second.is_active && second.is_focused);
        assert_eq!(second.active_window_id, Some(101));
        assert_eq!(first.active_window_id, Some(100), "the window it had");
        assert_ne!(first.id, second.id);
        // The window names the workspace it is on.
        assert_eq!(window_of(&snapshot, 101).workspace_id, Some(second.id));
        assert_eq!(window_of(&snapshot, 100).workspace_id, Some(first.id));
    }

    #[test]
    fn workspace_ids_are_unique_among_monitors_and_fit_a_javascript_number() {
        let (mut engine, backend) = engine_with(&[100]);
        engine.register_monitor(
            OutputId::from_name("SecondMonitor"),
            Rect::new(1920, 0, 1920, 1080),
            Rect::new(1920, 0, 1920, 1040),
        );
        engine.switch_workspace(3, &backend);
        let snapshot = engine.ipc_snapshot();
        let mut ids: Vec<u64> = snapshot.workspaces.iter().map(|w| w.id).collect();
        let count = ids.len();
        assert!(count >= 3, "two monitors, one with two workspaces: {:?}", ids);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "no two workspaces share an id: {:?}", snapshot.workspaces);
        assert!(ids.iter().all(|id| *id < (1u64 << 53)), "{:?}", ids);
        // Only the workspace that is active on the focused monitor is focused; the other
        // monitor's active workspace is active but not focused.
        assert_eq!(snapshot.workspaces.iter().filter(|w| w.is_focused).count(), 1);
        assert!(snapshot.workspaces.iter().filter(|w| w.is_active).count() >= 2);
    }

    #[test]
    fn tags_are_small_distinct_and_never_zero() {
        const TAGS: u64 = 0xF_FFFF;
        assert_eq!(tags_for(&[0, 1, 2]), vec![1, 2, 3]);
        // The same tag asked twice: the next free one.
        assert_eq!(tags_for(&[5, 5 + TAGS]), vec![6, 7]);
        // The last tag wraps to the first.
        assert_eq!(tags_for(&[TAGS - 1, TAGS - 1 + TAGS]), vec![TAGS, 1]);
        for tag in tags_for(&[u64::MAX, 0, 12345, TAGS]) {
            assert!((1..=TAGS).contains(&tag), "{}", tag);
        }
        // A workspace id is below 2^53 whatever the workspace.
        assert!(workspace_ipc_id(TAGS, i32::MAX) < (1u64 << 53));
        assert_ne!(workspace_ipc_id(1, 0), workspace_ipc_id(2, 0));
        assert_ne!(workspace_ipc_id(1, 0), workspace_ipc_id(1, 1));
    }

    #[test]
    fn floating_and_minimized_windows_belong_to_a_workspace_but_to_no_column() {
        let (mut engine, backend) = engine_with(&[100, 101, 102]);
        engine.toggle_floating_for(WindowId::new(101), &backend);
        engine.minimize_window(WindowId::new(102), &backend);
        let snapshot = engine.ipc_snapshot();
        let tiled = window_of(&snapshot, 100);
        let floating = window_of(&snapshot, 101);
        let minimized = window_of(&snapshot, 102);
        assert!(floating.is_floating && !floating.is_minimized);
        assert_eq!(floating.layout.pos_in_scrolling_layout, None);
        assert_eq!(floating.workspace_id, tiled.workspace_id);
        assert!(minimized.is_minimized && !minimized.is_floating);
        assert_eq!(minimized.layout.pos_in_scrolling_layout, None);
        assert_eq!(minimized.workspace_id, tiled.workspace_id);
        assert_eq!(tiled.layout.pos_in_scrolling_layout, Some((1, 1)));
    }

    #[test]
    fn fullscreen_maximized_urgent_and_the_overview_are_told() {
        let (mut engine, backend) = engine_with(&[100, 101, 102]);
        engine.toggle_fullscreen_for(WindowId::new(100), &backend);
        engine.set_maximized_to_edges(WindowId::new(101), true, &backend);
        engine.mark_urgent(WindowId::new(102));
        let snapshot = engine.ipc_snapshot();
        assert!(window_of(&snapshot, 100).is_fullscreen);
        assert!(window_of(&snapshot, 101).is_maximized);
        assert!(window_of(&snapshot, 102).is_urgent);
        assert!(snapshot.workspaces[0].is_urgent, "a window of the workspace is urgent");
        assert!(!snapshot.overview_open);
        // The fullscreen window keeps its column.
        assert_eq!(window_of(&snapshot, 100).layout.pos_in_scrolling_layout, Some((1, 1)));
        engine.enter_overview(&backend);
        assert!(engine.ipc_snapshot().overview_open);
        engine.exit_overview(&backend);
        assert!(!engine.ipc_snapshot().overview_open);
    }

    #[test]
    fn a_window_that_went_to_another_workspace_names_that_one() {
        let (mut engine, backend) = engine_with(&[100, 101]);
        engine.sync_os_foreground(WindowId::new(101), &backend);
        let before = engine.ipc_snapshot();
        let first_workspace = window_of(&before, 101).workspace_id.unwrap();
        engine.move_window_to_workspace(1, &backend);
        let after = engine.ipc_snapshot();
        assert_ne!(window_of(&after, 101).workspace_id.unwrap(), first_workspace);
        assert_eq!(window_of(&after, 100).workspace_id, Some(first_workspace));
        assert!(
            after.workspaces.len() > before.workspaces.len(),
            "the workspace it went to is listed"
        );
    }

    #[test]
    fn describing_the_engine_twice_gives_the_same_description() {
        // `diff` relies on it: nothing may change between two looks at an unchanged engine.
        let (mut engine, backend) = engine_with(&[100, 101, 102]);
        engine.sync_os_foreground(WindowId::new(100), &backend);
        engine.toggle_floating_for(WindowId::new(102), &backend);
        assert_eq!(engine.ipc_snapshot(), engine.ipc_snapshot());
    }
}
