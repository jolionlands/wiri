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

fn layout_geometry(rect: Rect, output: Rect, scale: f64, inset: i32) -> WindowLayoutIpc {
    WindowLayoutIpc {
        pos_in_scrolling_layout: None,
        tile_size: (rect.size.w as f64 / scale, rect.size.h as f64 / scale),
        window_size: (
            (rect.size.w.saturating_sub(inset as u32 * 2) as f64 / scale).round() as i32,
            (rect.size.h.saturating_sub(inset as u32 * 2) as f64 / scale).round() as i32,
        ),
        tile_pos_in_workspace_view: Some((
            (rect.loc.x - output.loc.x) as f64 / scale,
            (rect.loc.y - output.loc.y) as f64 / scale,
        )),
        window_offset_in_tile: (inset as f64 / scale, inset as f64 / scale),
    }
}

impl TilingEngine {
    /// Duplicate foreground reports do not reorder MRU history. Unmanaged windows
    /// still break a focus interval, but never acquire managed-window history.
    pub(super) fn record_ipc_foreground(&mut self, window: WindowId) {
        if self.os_foreground != Some(window) {
            // Any other live focus change ends the converged window's claim that its
            // broadcast was already made.
            self.converged_broadcast = None;
        }
        if self.os_foreground != Some(window) && self.tiled_windows.contains_key(&window) {
            let stamp = self.focus_epoch.elapsed().max(
                self.last_focus_stamp
                    .saturating_add(std::time::Duration::from_nanos(1)),
            );
            self.last_focus_stamp = stamp;
            self.focus_timestamps.insert(window, stamp);
        }
        self.os_foreground = Some(window);
    }

    /// Describe logical geometry before the offscreen-parking optimization. The
    /// pure calculators are shared with layout application; no Win32 calls or
    /// layout mutations occur, including for inactive workspaces.
    fn ipc_layouts(&self) -> HashMap<WindowId, WindowLayoutIpc> {
        let mut layouts = HashMap::new();
        for (output, monitor) in self.monitors.iter() {
            let scale = monitor.scale_factor.max(0.01);
            for (workspace_id, workspace) in &monitor.workspaces {
                let cfg = self.effective_config_for_workspace(*output, *workspace_id);
                // Inactive workspaces may have a different layout from the active one.
                let work = Self::tiling_work_rect(monitor, cfg);
                use crate::layout::workspace::WorkspaceLayout;
                let positions = match workspace.layout_mode {
                    WorkspaceLayout::Scrolling => self.calculate_positions(
                        workspace,
                        Self::precise_tiling_area(monitor, cfg),
                        cfg,
                    ),
                    WorkspaceLayout::BStack => {
                        self.calculate_positions_bstack(workspace, work, cfg)
                    }
                    WorkspaceLayout::Spiral => self.calculate_positions_spiral(workspace, work),
                };
                let ids = positions.iter().map(|(id, _)| *id).collect();
                let modes = self.sizing_draw_modes(&ids);
                let positions = Self::apply_sizing_states(
                    positions,
                    &modes,
                    work,
                    monitor.work_area,
                    monitor.bounds,
                );
                for (window, rect) in positions {
                    let normal =
                        modes.get(&window).copied().unwrap_or(DrawMode::Inset) == DrawMode::Inset;
                    let inset = if normal {
                        self.rule_border_inset(window, cfg)
                    } else {
                        0
                    };
                    let resolved = self.resolved_window_geometry(window, rect, cfg, inset, normal);
                    let mut geometry = layout_geometry(resolved.tile, monitor.bounds, scale, inset);
                    if let Some(width) = resolved.logical_window_size.0 {
                        geometry.window_size.0 = width;
                    }
                    if let Some(height) = resolved.logical_window_size.1 {
                        geometry.window_size.1 = height;
                    }
                    layouts.insert(window, geometry);
                }
            }
        }
        layouts
    }

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
    /// its remembered active window, then the most recently focused window or its first tile.
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
        if let Some(remembered) = monitor
            .workspaces
            .get(&workspace)
            .and_then(|ws| {
                ws.active_float
                    .filter(|w| !self.minimized.contains_key(w) && on_workspace(w))
                    .or(ws.active_window)
            })
            .filter(|w| on_workspace(w))
        {
            return Some(remembered);
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
        let names: HashMap<OutputId, &String> = outputs.iter().map(|(n, o)| (*o, n)).collect();

        let places = self.ipc_places();
        let layouts = self.ipc_layouts();
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
            for workspace in monitor.workspace_ids() {
                let is_active = monitor.active_workspace == workspace;
                workspaces.push(WorkspaceIpc {
                    id: monitor.workspaces[&workspace].ipc_id,
                    idx: monitor.workspace_number_of(workspace).unwrap_or(1).max(1) as u32,
                    name: self.workspace_name_on(*output, workspace),
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
            let mut layout = layouts.get(window).cloned().unwrap_or_else(|| {
                let rect = self
                    .float_homes
                    .get(window)
                    .map_or(info.bounds, |home| home.rect);
                let monitor = place.and_then(|p| self.monitors.get(&p.output));
                let mut layout = layout_geometry(
                    rect,
                    monitor.map_or(Rect::default(), |m| m.bounds),
                    monitor.map_or(1.0, |m| m.scale_factor.max(0.01)),
                    0,
                );
                if self.minimized.contains_key(window) || monitor.is_none() {
                    layout.tile_pos_in_workspace_view = None;
                }
                layout
            });
            layout.pos_in_scrolling_layout = place
                .and_then(|p| p.slot)
                .map(|(column, tile)| (column + 1, tile + 1));
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
                    self.monitors
                        .get(&p.output)?
                        .workspaces
                        .get(&p.workspace)
                        .map(|ws| ws.ipc_id)
                }),
                is_focused: focused_window == Some(*window),
                is_floating: self.floating_windows.contains(window),
                is_urgent: self.urgent_windows.contains(window),
                layout,
                focus_timestamp: self.focus_timestamps.get(window).copied(),
                class_name: info.class_name.clone(),
                output: place.and_then(|p| names.get(&p.output).map(|name| (*name).clone())),
                is_fullscreen: self.is_fullscreen(*window),
                is_maximized: self.is_maximized_to_edges(*window),
                is_minimized: self.minimized.contains_key(window),
            });
        }
        windows.sort_by_key(|w| w.id);

        IpcSnapshot {
            windows,
            workspaces,
            // niri reports the overview closed as soon as it starts closing, while the camera
            // is still animating; the host and layout stay up until the last frame.
            overview_open: self.overview.is_some() && !self.overview_camera.closing,
            keyboard_layouts: None,
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
            !w.is_floating
                && !w.is_urgent
                && !w.is_fullscreen
                && !w.is_maximized
                && !w.is_minimized
        );
        // One column each, counted from 1.
        assert_eq!(w.layout.pos_in_scrolling_layout, Some((2, 1)));
        assert_eq!(
            window_of(&snapshot, 100).layout.pos_in_scrolling_layout,
            Some((1, 1))
        );
        assert_eq!(
            window_of(&snapshot, 102).layout.pos_in_scrolling_layout,
            Some((3, 1))
        );
        // All on the same workspace, which exists in the list.
        let workspace = window_of(&snapshot, 100).workspace_id.expect("a workspace");
        assert_eq!(w.workspace_id, Some(workspace));
        assert!(snapshot.workspaces.iter().any(|ws| ws.id == workspace));
    }

    #[test]
    fn the_keyboard_focus_is_the_os_foreground_window_when_wiri_manages_it() {
        let (mut engine, backend) = engine_with(&[100, 101]);
        assert_eq!(
            engine.ipc_snapshot().focused_window(),
            None,
            "no report yet"
        );
        engine.sync_os_foreground(WindowId::new(100), &backend);
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
        // A window wiri does not manage (the Start menu) has the focus: no managed window has.
        engine.sync_os_foreground(WindowId::new(999), &backend);
        assert_eq!(engine.ipc_snapshot().focused_window(), None);
        engine.sync_os_foreground(WindowId::new(101), &backend);
        engine.remove_window(WindowId::new(101), &backend);
        assert_eq!(
            engine.ipc_snapshot().focused_window(),
            None,
            "its window is gone"
        );
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
        assert!(
            count >= 3,
            "two monitors, one with two workspaces: {:?}",
            ids
        );
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            count,
            "no two workspaces share an id: {:?}",
            snapshot.workspaces
        );
        assert!(ids.iter().all(|id| *id < (1u64 << 53)), "{:?}", ids);
        // Only the workspace that is active on the focused monitor is focused; the other
        // monitor's active workspace is active but not focused.
        assert_eq!(
            snapshot.workspaces.iter().filter(|w| w.is_focused).count(),
            1
        );
        assert!(snapshot.workspaces.iter().filter(|w| w.is_active).count() >= 2);
    }

    #[test]
    fn workspace_identity_is_allocated_once_and_survives_reparenting() {
        use crate::layout::Workspace;

        let mut workspace = Workspace::new();
        let id = workspace.ipc_id;
        assert!(id > 0 && id < (1 << 53));
        assert_ne!(id, Workspace::new().ipc_id);
        workspace.scroll_offset.x = 40;
        let copy = workspace.clone();
        assert_eq!(copy.ipc_id, id);
        let mut first = Monitor::new(
            OutputId::from_name("identity-a"),
            Rect::new(0, 0, 800, 600),
            Rect::new(0, 0, 800, 600),
        );
        let mut second = Monitor::new(
            OutputId::from_name("identity-b"),
            Rect::new(800, 0, 800, 600),
            Rect::new(800, 0, 800, 600),
        );
        first.insert_workspace(17, workspace);
        second.insert_workspace(31, first.remove_workspace(17).unwrap());
        assert_eq!(second.workspaces[&31].ipc_id, id);
    }

    #[test]
    fn snapshot_uses_source_workspace_identity_and_output_qualified_names() {
        let (mut engine, _backend) = engine_with(&[100]);
        let first = OutputId::from_name("TestMonitor");
        let second = OutputId::from_name("identity-destination");
        engine.register_monitor(
            second,
            Rect::new(1920, 0, 1920, 1080),
            Rect::new(1920, 0, 1920, 1040),
        );
        engine.set_output_name(first, "DISPLAY1");
        engine.set_output_name(second, "DISPLAY2");
        let source_id = {
            let workspace = engine
                .monitors
                .get_mut(&first)
                .unwrap()
                .workspaces
                .get_mut(&0)
                .unwrap();
            workspace.name = Some("source".to_string());
            workspace.ipc_id
        };
        let other_id = {
            let workspace = engine
                .monitors
                .get_mut(&second)
                .unwrap()
                .workspaces
                .get_mut(&0)
                .unwrap();
            workspace.name = Some("other".to_string());
            workspace.ipc_id
        };
        assert_ne!(
            source_id, other_id,
            "local key 0 does not identify a workspace globally"
        );
        engine.maintain_empty_workspace(second);
        let before = engine.ipc_snapshot();
        assert_eq!(window_of(&before, 100).workspace_id, Some(source_id));
        assert_eq!(
            before
                .workspaces
                .iter()
                .find(|ws| ws.id == other_id)
                .unwrap()
                .name
                .as_deref(),
            Some("other")
        );

        let moved = {
            let monitor = engine.monitors.get_mut(&first).unwrap();
            let moved = monitor.remove_workspace(0).unwrap();
            monitor.active_workspace = monitor.workspace_ids()[0];
            monitor.focus_window = None;
            monitor.focus_column = None;
            moved
        };
        engine
            .monitors
            .get_mut(&second)
            .unwrap()
            .insert_workspace(31, moved);
        engine.maintain_empty_workspace(second);
        let after = engine.ipc_snapshot();
        assert_eq!(window_of(&after, 100).workspace_id, Some(source_id));
        let workspace = after
            .workspaces
            .iter()
            .find(|ws| ws.id == source_id)
            .unwrap();
        assert_eq!(workspace.name.as_deref(), Some("source"));
        assert_eq!(workspace.output.as_deref(), Some("DISPLAY2"));
        assert_eq!(
            workspace.idx, 2,
            "the position changed, not the source identity"
        );
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
        assert!(
            snapshot.workspaces[0].is_urgent,
            "a window of the workspace is urgent"
        );
        assert!(!snapshot.overview_open);
        // The fullscreen window keeps its column.
        assert_eq!(
            window_of(&snapshot, 100).layout.pos_in_scrolling_layout,
            Some((1, 1))
        );
        engine.enter_overview(&backend);
        assert!(engine.ipc_snapshot().overview_open);
        engine.exit_overview(&backend);
        assert!(!engine.ipc_snapshot().overview_open);
    }

    #[test]
    fn a_closing_overview_is_told_closed_at_once_and_open_again_when_reversed() {
        use crate::overlay::MockThumbnailOverview;
        let (mut engine, backend) = engine_with(&[100, 101]);
        engine.install_thumbnail_overview(MockThumbnailOverview::new());
        engine.set_animation_notifier(std::sync::Arc::new(tokio::sync::Notify::new()));
        engine.enter_overview(&backend);
        assert!(engine.ipc_snapshot().overview_open);
        engine.tick_animations_with_backend(60_000, &backend);
        engine.exit_overview(&backend);
        // The camera is still closing: the layout and host stay up, IPC says closed.
        assert!(engine.is_overview());
        assert!(!engine.ipc_snapshot().overview_open);
        engine.enter_overview(&backend);
        assert!(engine.ipc_snapshot().overview_open);
        engine.exit_overview(&backend);
        engine.tick_animations_with_backend(60_000, &backend);
        assert!(!engine.is_overview());
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
        assert_ne!(
            window_of(&after, 101).workspace_id.unwrap(),
            first_workspace
        );
        assert_eq!(window_of(&after, 100).workspace_id, Some(first_workspace));
        assert!(
            after.workspaces.len() > before.workspaces.len(),
            "the workspace it went to is listed"
        );
    }

    #[test]
    fn focus_stamps_are_monotonic_deduplicated_and_removed_with_the_window() {
        let (mut engine, backend) = engine_with(&[100, 101]);
        let one = WindowId::new(100);
        let two = WindowId::new(101);
        assert_eq!(window_of(&engine.ipc_snapshot(), 100).focus_timestamp, None);
        engine.sync_os_foreground(one, &backend);
        let first = engine.focus_timestamps[&one];
        engine.sync_os_foreground(one, &backend);
        assert_eq!(
            engine.focus_timestamps[&one], first,
            "duplicate notification"
        );
        engine.sync_os_foreground(two, &backend);
        let second = engine.focus_timestamps[&two];
        assert!(second > first);
        engine.sync_os_foreground(WindowId::new(999), &backend);
        assert!(!engine.focus_timestamps.contains_key(&WindowId::new(999)));
        engine.sync_os_foreground(one, &backend);
        assert!(engine.focus_timestamps[&one] > second);
        assert_eq!(engine.focus_timestamps[&two], second);
        engine.remove_window(one, &backend);
        assert!(!engine.focus_timestamps.contains_key(&one));
        engine.add_window(make_window(100), &backend);
        assert_eq!(window_of(&engine.ipc_snapshot(), 100).focus_timestamp, None);
    }

    #[test]
    fn geometry_uses_logical_output_local_coordinates_and_border_offsets() {
        let layout = layout_geometry(
            Rect::new(-1400, 150, 900, 600),
            Rect::new(-1600, 0, 1600, 900),
            1.5,
            3,
        );
        assert_eq!(layout.tile_size, (600.0, 400.0));
        assert_eq!(layout.window_size, (596, 396));
        assert_eq!(layout.window_offset_in_tile, (2.0, 2.0));
        assert_eq!(
            layout.tile_pos_in_workspace_view,
            Some((200.0 / 1.5, 100.0))
        );
    }

    fn geometry_fixture(
        mut config: crate::config::Config,
        inactive_layout: Option<crate::config::LayoutConfigPartial>,
    ) -> (TilingEngine, OutputId, i32, i32) {
        let mut active = crate::config::WorkspaceConfig::default_config();
        active.name = "GeometryActive".to_string();
        let mut inactive = crate::config::WorkspaceConfig::default_config();
        inactive.name = "GeometryInactive".to_string();
        inactive.layout_override = inactive_layout;
        config.workspace = vec![active, inactive];
        let mut engine = TilingEngine::new(LayoutConfig::from_config(&config));
        engine.set_full_config(config);
        let output = OutputId::from_name("ipc-geometry-fractional-scale");
        engine.register_monitor_with_scale(
            output,
            Rect::new(-1920, 120, 1920, 1080),
            Rect::new(-1920, 120, 1920, 1040),
            1.5,
        );
        let key_of = |name: &str| {
            engine
                .monitors
                .get(&output)
                .unwrap()
                .workspaces
                .iter()
                .find_map(|(key, workspace)| {
                    (workspace.name.as_deref() == Some(name)).then_some(*key)
                })
                .unwrap()
        };
        let active = key_of("GeometryActive");
        let inactive = key_of("GeometryInactive");
        for (workspace, id) in [(active, 100), (inactive, 101)] {
            let window = WindowId::new(id);
            engine
                .monitors
                .get_mut(&output)
                .unwrap()
                .workspaces
                .get_mut(&workspace)
                .unwrap()
                .add_window_to_new_column(window);
            engine.tiled_windows.insert(window, make_window(id));
        }
        let monitor = engine.monitors.get_mut(&output).unwrap();
        monitor.active_workspace = active;
        monitor.focus_column = Some(0);
        monitor.focus_window = Some(WindowId::new(100));
        (engine, output, active, inactive)
    }

    fn assert_snapshot_matches_calculated_native_allocation(
        engine: &TilingEngine,
        output: OutputId,
        workspace: i32,
        expected_inset: i32,
    ) {
        let monitor = engine.monitors.get(&output).unwrap();
        assert_eq!(monitor.scale_factor, 1.5);
        let cfg = engine.effective_config_for_workspace(output, workspace);
        let strip = &monitor.workspaces[&workspace];
        let positions =
            engine.calculate_positions(strip, TilingEngine::tiling_work_rect(monitor, cfg), cfg);
        assert!(!positions.is_empty());
        let snapshot = engine.ipc_snapshot();
        for (window, tile) in positions {
            // Ordinary tiles use this inset when apply_layout_for_monitor
            // constructs the native visible allocation. Focus-ring thickness
            // does not reserve window space; a disabled/zero-width border does
            // not reserve an artificial pixel either.
            let inset = engine.rule_border_inset(window, cfg);
            assert_eq!(inset, expected_inset, "fixture's actual native inset");
            let native = Rect::new(
                tile.loc.x + inset,
                tile.loc.y + inset,
                tile.size.w.saturating_sub(2 * inset as u32),
                tile.size.h.saturating_sub(2 * inset as u32),
            );
            let layout = &window_of(&snapshot, window.as_isize() as u64).layout;
            let scale = monitor.scale_factor;
            assert_eq!(
                layout.tile_size,
                (tile.size.w as f64 / scale, tile.size.h as f64 / scale),
                "tile allocation for workspace {workspace} / {window:?}"
            );
            assert_eq!(
                layout.window_size,
                (
                    (native.size.w as f64 / scale).round() as i32,
                    (native.size.h as f64 / scale).round() as i32,
                ),
                "IPC must describe the native visible allocation, workspace {workspace} / {window:?}"
            );
            assert_eq!(
                layout.window_offset_in_tile,
                (
                    (native.loc.x - tile.loc.x) as f64 / scale,
                    (native.loc.y - tile.loc.y) as f64 / scale,
                ),
                "IPC inset must match native placement, workspace {workspace} / {window:?}"
            );
        }
    }

    #[test]
    fn parity_ipc_layout_border_off_matches_native_allocation_at_fractional_scale() {
        for border_width in [0.0, 7.0] {
            for ring_width in [0.0, 9.0] {
                for ring_off in [false, true] {
                    let mut config = crate::config::Config::default();
                    config.layout.effects.border.off = true;
                    config.layout.effects.border.width = border_width;
                    config.layout.effects.focus_ring.off = ring_off;
                    config.layout.effects.focus_ring.width = ring_width;
                    let (engine, output, active, inactive) = geometry_fixture(config, None);
                    assert_eq!(engine.active_workspace_config(output).border_inset(), 0);
                    assert_snapshot_matches_calculated_native_allocation(
                        &engine, output, active, 0,
                    );
                    assert_snapshot_matches_calculated_native_allocation(
                        &engine, output, inactive, 0,
                    );
                }
            }
        }
    }

    #[test]
    fn parity_ipc_layout_enabled_border_uses_each_workspaces_own_inset() {
        let mut config = crate::config::Config::default();
        config.layout.effects.border.off = false;
        config.layout.effects.border.width = 3.0;
        config.layout.effects.focus_ring.width = 0.0;
        let mut override_layout = crate::config::LayoutConfigPartial::default();
        let mut border = crate::config::effects::RingStyle::border_default();
        border.width = 6.0;
        override_layout.effects.border = Some(border);
        let mut ring = crate::config::effects::RingStyle::focus_ring_default();
        ring.width = 11.0;
        override_layout.effects.focus_ring = Some(ring);
        override_layout.outer_gaps = Some(23.0);
        let (engine, output, active, inactive) = geometry_fixture(config, Some(override_layout));
        assert_ne!(active, inactive);
        assert_eq!(
            engine.monitors.get(&output).unwrap().active_workspace,
            active
        );
        // The widths are logical: niri's round_logical_in_physical_max1 at the fixture's
        // 1.5 scale turns 3 into 4.5 -> 5 and 6 into 9 physical pixels.
        assert_snapshot_matches_calculated_native_allocation(&engine, output, active, 5);
        assert_snapshot_matches_calculated_native_allocation(&engine, output, inactive, 9);
    }

    #[test]
    fn parity_ipc_layout_inactive_border_off_does_not_inherit_active_inset() {
        let mut config = crate::config::Config::default();
        config.layout.effects.border.off = false;
        config.layout.effects.border.width = 5.0;
        let mut override_layout = crate::config::LayoutConfigPartial::default();
        let mut border = crate::config::effects::RingStyle::border_default();
        border.off = true;
        border.width = 8.0;
        override_layout.effects.border = Some(border);
        let (engine, output, active, inactive) = geometry_fixture(config, Some(override_layout));
        assert_eq!(
            engine.monitors.get(&output).unwrap().active_workspace,
            active
        );
        // Logical 5 at the fixture's 1.5 scale is 7.5 -> 8 physical pixels.
        assert_snapshot_matches_calculated_native_allocation(&engine, output, active, 8);
        assert_snapshot_matches_calculated_native_allocation(&engine, output, inactive, 0);
    }

    #[test]
    fn offscreen_and_inactive_tiles_keep_their_logical_geometry_not_the_parking_rect() {
        let (mut engine, backend) = engine_with(&[100, 101, 102, 103, 104]);
        let snapshot = engine.ipc_snapshot();
        for window in &snapshot.windows {
            assert!(window.layout.tile_size.0 > 0.0 && window.layout.tile_size.1 > 0.0);
            assert!(window.layout.window_size.0 > 0 && window.layout.window_size.1 > 0);
            let (x, y) = window.layout.tile_pos_in_workspace_view.unwrap();
            assert!(
                x.abs() < 10_000.0 && y.abs() < 10_000.0,
                "not parked: {window:?}"
            );
        }
        let before = window_of(&snapshot, 100).layout.clone();
        engine.switch_workspace(1, &backend);
        assert_eq!(window_of(&engine.ipc_snapshot(), 100).layout, before);
        engine.toggle_floating_for(WindowId::new(101), &backend);
        let floating = window_of(&engine.ipc_snapshot(), 101).clone();
        assert_eq!(floating.layout.window_offset_in_tile, (0.0, 0.0));
        assert!(floating.layout.tile_size.0 > 0.0);
    }

    #[test]
    fn fullscreen_geometry_is_exact_and_multimonitor_scale_is_unit_tested() {
        let (mut engine, backend) = engine_with(&[100]);
        let output = OutputId::from_name("TestMonitor");
        engine.sync_monitor_with_scale(
            output,
            Rect::new(-1920, 0, 1920, 1080),
            Rect::new(-1920, 0, 1920, 1040),
            1.5,
        );
        engine.toggle_fullscreen_for(WindowId::new(100), &backend);
        let layout = window_of(&engine.ipc_snapshot(), 100).layout.clone();
        assert_eq!(layout.tile_size, (1280.0, 720.0));
        assert_eq!(layout.window_size, (1280, 720));
        assert_eq!(layout.window_offset_in_tile, (0.0, 0.0));
        assert_eq!(layout.tile_pos_in_workspace_view, Some((0.0, 0.0)));
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
