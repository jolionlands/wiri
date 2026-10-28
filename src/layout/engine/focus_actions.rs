//! Focus decisions use completed native visits, ordered by occurrence rather than
//! delivery. Explicit previous commits qualify its two endpoints without inventing
//! foreground dwell. Unconfirmed previous requests resume on a native observation.
use super::*;
use crate::backend::ForegroundStamp;
use crate::layout::ColumnDisplay;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub(super) enum FocusStep {
    Left,
    Right,
    Up,
    Down,
    FirstColumn,
    LastColumn,
    Column(u32),
    Window(u32),
    RightOrFirst,
    LeftOrLast,
    DownOrLeft,
    DownOrRight,
    UpOrLeft,
    UpOrRight,
    Top,
    Bottom,
    DownOrTop,
    UpOrBottom,
    DownOrWorkspace,
    UpOrWorkspace,
    LeftOrMonitor,
    RightOrMonitor,
    UpOrMonitor,
    DownOrMonitor,
}

// At the native clock's millisecond boundary, an explicit command wins over
// queued occurrences from that same tick. Native sequence orders ordinary ties;
// the local command serial orders repeated previous commands within one tick.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FocusOrder(u64, u8, u64);

struct FocusVisit {
    window: WindowId,
    dwell: Duration,
    order: FocusOrder,
}

#[derive(Default)]
pub(super) struct FocusHistory {
    committed: Vec<FocusVisit>,
    command_serial: u64,
}

impl FocusHistory {
    fn record(&mut self, window: WindowId, dwell: Duration, order: FocusOrder) {
        // Keep the time/dwell Pareto frontier. A delayed older visit must not
        // remove a newer visit, and a short revisit must not erase settled dwell.
        if self
            .committed
            .iter()
            .any(|old| old.window == window && old.dwell >= dwell && old.order >= order)
        {
            return;
        }
        self.committed
            .retain(|old| old.window != window || old.dwell > dwell || old.order > order);
        self.committed.push(FocusVisit {
            window,
            dwell,
            order,
        });
        self.committed
            .sort_unstable_by_key(|visit| std::cmp::Reverse(visit.order));
    }

    pub(super) fn record_visit(
        &mut self,
        window: WindowId,
        dwell: Duration,
        stamp: ForegroundStamp,
    ) {
        self.record(
            window,
            dwell,
            FocusOrder(stamp.uptime_ms, 0, stamp.sequence),
        );
    }

    fn commit_pair(&mut self, current: WindowId, target: WindowId, uptime_ms: u64) {
        for window in [current, target] {
            self.command_serial = self.command_serial.saturating_add(1);
            self.record(
                window,
                Duration::MAX,
                FocusOrder(uptime_ms, 1, self.command_serial),
            );
        }
    }

    fn eligible(&self, debounce: Duration) -> impl Iterator<Item = WindowId> + '_ {
        self.committed
            .iter()
            .filter(move |visit| visit.dwell >= debounce)
            .map(|visit| visit.window)
    }

    pub(super) fn remove(&mut self, window: WindowId) {
        self.committed.retain(|visit| visit.window != window);
    }
}

/// Select a tile, remembering the active tile of every column (also when there are more
/// windows than fit in the monitor's legacy 32-entry MRU ring).
fn remembered_window(monitor: &Monitor, column: usize) -> Option<WindowId> {
    let col = monitor.workspace()?.columns.get(column)?;
    col.active_window()
        .filter(|w| col.index_of(*w).is_some())
        .or_else(|| match col.display {
            ColumnDisplay::Tabbed { active_tab } => col.tiles.get(active_tab).map(|t| t.window_id),
            ColumnDisplay::Stacked => None,
        })
        .or_else(|| {
            monitor
                .focus_ring
                .iter_mru()
                .find(|w| col.index_of(*w).is_some())
        })
        .or_else(|| col.tiles.first().map(|t| t.window_id))
}

/// None at an edge, not the currently focused tile. Composite actions can then fall back
/// without accidentally executing both halves.
fn tile_target(monitor: &Monitor, step: FocusStep) -> Option<WindowId> {
    use FocusStep::*;
    let ws = monitor.workspace()?;
    let last = ws.columns.len().checked_sub(1)?;
    let column = monitor
        .focus_window
        .and_then(|w| ws.find_window_column(w))
        .or(monitor.focus_column)
        .unwrap_or(0)
        .min(last);
    let col = &ws.columns[column];
    let current = monitor
        .focus_window
        .and_then(|w| col.index_of(w))
        .unwrap_or(0);
    let bottom = col.tiles.len().checked_sub(1)?;
    let tile_at = |index: usize| col.tiles.get(index).map(|t| t.window_id);
    let up = || current.checked_sub(1).and_then(tile_at);
    let down = || (current < bottom).then(|| current + 1).and_then(tile_at);
    let left = || {
        column
            .checked_sub(1)
            .and_then(|i| remembered_window(monitor, i))
    };
    let right = || {
        (column < last)
            .then(|| column + 1)
            .and_then(|i| remembered_window(monitor, i))
    };
    match step {
        Left | LeftOrMonitor => left(),
        Right | RightOrMonitor => right(),
        Up | UpOrWorkspace | UpOrMonitor => up(),
        Down | DownOrWorkspace | DownOrMonitor => down(),
        FirstColumn => remembered_window(monitor, 0),
        LastColumn => remembered_window(monitor, last),
        Column(index) => remembered_window(monitor, (index.saturating_sub(1) as usize).min(last)),
        Window(index) => tile_at((index.saturating_sub(1) as usize).min(bottom)),
        RightOrFirst => right().or_else(|| remembered_window(monitor, 0)),
        LeftOrLast => left().or_else(|| remembered_window(monitor, last)),
        DownOrLeft => down().or_else(left),
        DownOrRight => down().or_else(right),
        UpOrLeft => up().or_else(left),
        UpOrRight => up().or_else(right),
        Top => tile_at(0),
        Bottom => tile_at(bottom),
        DownOrTop => down().or_else(|| tile_at(0)),
        UpOrBottom => up().or_else(|| tile_at(bottom)),
    }
}

impl TilingEngine {
    pub fn focus_layout_action(
        &mut self,
        action: &crate::input::FocusAction,
        backend: &BackendHandle,
    ) {
        use crate::input::FocusAction as A;
        use FocusStep as S;
        let step = match *action {
            A::ColumnRightOrFirst => S::RightOrFirst,
            A::ColumnLeftOrLast => S::LeftOrLast,
            A::Column(index) => S::Column(index),
            A::WindowInColumn(index) => S::Window(index),
            A::DownOrColumnLeft => S::DownOrLeft,
            A::DownOrColumnRight => S::DownOrRight,
            A::UpOrColumnLeft => S::UpOrLeft,
            A::UpOrColumnRight => S::UpOrRight,
            A::WindowTop => S::Top,
            A::WindowBottom => S::Bottom,
            A::DownOrTop => S::DownOrTop,
            A::UpOrBottom => S::UpOrBottom,
            A::WindowOrWorkspaceDown => S::DownOrWorkspace,
            A::WindowOrWorkspaceUp => S::UpOrWorkspace,
            A::ColumnOrMonitorLeft => S::LeftOrMonitor,
            A::ColumnOrMonitorRight => S::RightOrMonitor,
            A::WindowOrMonitorUp => S::UpOrMonitor,
            A::WindowOrMonitorDown => S::DownOrMonitor,
            A::Previous { debounce_ms } => {
                self.focus_previous_with_debounce(debounce_ms, backend);
                return;
            }
        };
        self.focus_step(step, backend);
    }

    pub(super) fn remember_action_focus(&mut self, window: WindowId) {
        if !self.tiled_windows.contains_key(&window) {
            return;
        }
        // A different selection supersedes an unfinished previous command, but
        // selection/activation itself never starts an observed dwell interval.
        if self
            .pending_previous
            .is_some_and(|(expected, _)| expected != window)
        {
            self.pending_previous = None;
        }
        if let Some((output, workspace, _column)) = self.locate_window(window) {
            if let Some(ws) = self
                .monitors
                .get_mut(&output)
                .and_then(|m| m.workspaces.get_mut(&workspace))
            {
                ws.remember_focus(window);
                ws.active_float = None;
            }
        } else if let Some((output, workspace)) = self.float_location(window) {
            if let Some(monitor) = self.monitors.get_mut(&output) {
                // A float has its own stack and home; `Workspace::active_window`
                // deliberately stays the last tile so switching back restores the
                // column even after the bounded MRU ring forgot it.
                monitor.focus_ring.push(window);
                if let Some(ws) = monitor.workspaces.get_mut(&workspace) {
                    ws.active_float = Some(window);
                }
            }
        }
    }

    /// Focus a managed window across workspaces/outputs. This is also the MRU path: never
    /// activate a hidden tile without first selecting its workspace.
    pub(super) fn focus_action_window(
        &mut self,
        window: WindowId,
        backend: &BackendHandle,
    ) -> bool {
        if self.floating_windows.contains(&window) {
            self.focus_layer = Some(FocusLayer::Floating);
            return self.focus_float(window, backend);
        }
        self.focus_layer = Some(FocusLayer::Tiling);
        if self.minimized.contains_key(&window) {
            self.restore_window(window, backend);
        }
        let Some((output, workspace, column)) = self.locate_window(window) else {
            return false;
        };
        self.monitors.set_focused(output);
        if self.monitors.get(&output).map(|m| m.active_workspace) != Some(workspace) {
            self.switch_workspace(workspace, backend);
        }
        if let Some(monitor) = self.monitors.get_mut(&output) {
            monitor.focus_column = Some(column);
            monitor.focus_window = Some(window);
            monitor.focus_ring.push(window);
        }
        self.remember_action_focus(window);
        self.scroll_to_focused_column(output, backend);
        true
    }

    pub(super) fn focus_step(
        &mut self,
        step: FocusStep,
        backend: &BackendHandle,
    ) -> Option<WindowId> {
        let output = self.monitors.focused_id()?;
        let floating = self
            .os_foreground
            .filter(|w| self.floating_windows.contains(w))
            .filter(|w| {
                self.float_location(*w).map(|(o, ws)| (o, Some(ws)))
                    == self
                        .monitors
                        .get(&output)
                        .map(|m| (output, Some(m.active_workspace)))
            });
        let target = if let Some(current) = floating {
            self.float_focus_target(current, step)
        } else {
            tile_target(self.monitors.get(&output)?, step)
        };
        if let Some(target) = target {
            self.focus_action_window(target, backend);
            return Some(target);
        }
        use FocusStep::*;
        match step {
            DownOrWorkspace | UpOrWorkspace => {
                let down = matches!(step, DownOrWorkspace);
                self.focus_workspace_relative(
                    if down {
                        WorkspaceDirection::Next
                    } else {
                        WorkspaceDirection::Previous
                    },
                    backend,
                );
            }
            LeftOrMonitor | RightOrMonitor | UpOrMonitor | DownOrMonitor => {
                let target = match step {
                    LeftOrMonitor => super::monitor_actions::MonitorTarget::Left,
                    RightOrMonitor => super::monitor_actions::MonitorTarget::Right,
                    UpOrMonitor => super::monitor_actions::MonitorTarget::Up,
                    _ => super::monitor_actions::MonitorTarget::Down,
                };
                self.focus_monitor_action(&target, backend);
            }
            _ => {}
        }
        self.monitors.focused().and_then(|m| m.focus_window)
    }

    /// Floating directional focus stays within its own workspace. Index/edge/wrap tile
    /// actions have no column to operate on and leave the float alone.
    fn float_focus_target(&self, current: WindowId, step: FocusStep) -> Option<WindowId> {
        use FocusStep::*;
        let home = self.float_homes.get(&current)?;
        let center = |r: Rect| {
            (
                i64::from(r.loc.x) * 2 + i64::from(r.size.w),
                i64::from(r.loc.y) * 2 + i64::from(r.size.h),
            )
        };
        let (cx, cy) = center(self.float_rect_of(current)?);
        let toward = |direction: (i64, i64)| {
            self.float_homes
                .iter()
                .filter(|(w, h)| {
                    **w != current
                        && h.output == home.output
                        && h.workspace == home.workspace
                        && self.floating_windows.contains(w)
                        && !self.minimized.contains_key(w)
                })
                .filter_map(|(w, _h)| {
                    let (x, y) = center(self.float_rect_of(*w)?);
                    let (dx, dy) = (x - cx, y - cy);
                    let ahead = dx * direction.0 + dy * direction.1;
                    // i128 avoids overflow on synthetic extreme monitor coordinates.
                    let distance =
                        i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy);
                    (ahead > 0).then_some((distance, w.as_isize(), *w))
                })
                .min_by_key(|(distance, id, _)| (*distance, *id))
                .map(|(_, _, w)| w)
        };
        match step {
            Left | LeftOrMonitor | LeftOrLast => toward((-1, 0)),
            Right | RightOrMonitor | RightOrFirst => toward((1, 0)),
            Up | UpOrMonitor | UpOrWorkspace => toward((0, -1)),
            Down | DownOrMonitor | DownOrWorkspace => toward((0, 1)),
            DownOrLeft => toward((0, 1)).or_else(|| toward((-1, 0))),
            DownOrRight => toward((0, 1)).or_else(|| toward((1, 0))),
            UpOrLeft => toward((0, -1)).or_else(|| toward((-1, 0))),
            UpOrRight => toward((0, -1)).or_else(|| toward((1, 0))),
            _ => None,
        }
    }

    pub fn focus_previous_with_debounce(&mut self, debounce_ms: u64, backend: &BackendHandle) {
        // Resolve under this same engine transaction as selection, including
        // legacy direct IPC callers. This is a live sample, never native dwell.
        self.resolve_action_target(backend);
        let current = self
            .os_foreground
            .filter(|w| self.tiled_windows.contains_key(w))
            .or_else(|| self.monitors.focused().and_then(|m| m.focus_window));
        let Some(current) = current else {
            self.pending_previous = None;
            return;
        };
        // SAFETY: GetTickCount64 reads uptime without pointers or handles.
        let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
        self.focus_previous_for_at(current, debounce_ms, now, backend);
    }

    pub(super) fn focus_previous_for_at(
        &mut self,
        current: WindowId,
        debounce_ms: u64,
        uptime_ms: u64,
        backend: &BackendHandle,
    ) -> bool {
        self.pending_previous = None;
        if !self.tiled_windows.contains_key(&current) {
            return false;
        }
        if self.foreground_observations.current() != Some(current) {
            // A live action snapshot got ahead of the native stream. Do not
            // stretch the old pending visit to now or guess from the MRU ring.
            self.pending_previous = Some((current, debounce_ms));
            return false;
        }
        let target = self
            .focus_history
            .eligible(Duration::from_millis(debounce_ms))
            .find(|w| *w != current && self.tiled_windows.contains_key(w));
        if let Some(target) = target {
            if self.focus_action_window(target, backend) {
                let native_ms = self
                    .foreground_observations
                    .last_stamp()
                    .map_or(uptime_ms, |stamp| stamp.uptime_ms);
                // Explicit previous is back-and-forth. Its qualification is
                // command intent, not a fabricated native dwell interval.
                self.focus_history
                    .commit_pair(current, target, uptime_ms.max(native_ms));
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WindowState;
    fn fixture() -> (TilingEngine, BackendHandle) {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let output = OutputId::from_name("focus-tests");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 800),
        );
        for id in 100..104 {
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
        let monitor = engine.monitors.get_mut(&output).unwrap();
        let ws = monitor.workspace_mut().unwrap();
        let mut second = ws.columns.remove(1);
        ws.columns[0].tiles.append(&mut second.tiles);
        let mut fourth = ws.columns.remove(2);
        ws.columns[1].tiles.append(&mut fourth.tiles);
        // The fixture joins columns directly, so establish the remembered tiles
        // that normal engine focus events would have recorded before returning.
        ws.columns[0].active_window = Some(WindowId::new(101));
        ws.columns[1].active_window = Some(WindowId::new(103));
        ws.active_window = Some(WindowId::new(100));
        monitor.focus_column = Some(0);
        monitor.focus_window = Some(WindowId::new(100));
        engine.seed_native_foreground(100, 100, 0, &backend);
        (engine, backend)
    }
    fn observe(
        engine: &mut TilingEngine,
        backend: &BackendHandle,
        hwnd: isize,
        uptime_ms: u64,
        sequence: u64,
    ) {
        engine.observe_native_foreground_at(
            hwnd,
            hwnd,
            ForegroundStamp {
                uptime_ms,
                sequence,
            },
            true,
            uptime_ms,
            backend,
        );
    }
    fn stamp(uptime_ms: u64, sequence: u64) -> ForegroundStamp {
        ForegroundStamp {
            uptime_ms,
            sequence,
        }
    }
    fn focused(engine: &TilingEngine) -> Option<isize> {
        engine
            .monitors
            .focused()
            .and_then(|m| m.focus_window)
            .map(WindowId::as_isize)
    }
    #[test]
    fn floating_focus_keeps_the_tiled_workspace_memory() {
        let (mut engine, backend) = fixture();
        let output = engine.monitors.focused_id().unwrap();
        let workspace = engine.monitors.get(&output).unwrap().active_workspace;
        let floating = WindowId::new(101);
        // Make 101 a real float while it remains a tracked window, as unfloat can.
        engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .remove_window(floating);
        engine.register_float(floating, output, workspace, Rect::new(50, 50, 300, 200));
        engine.remember_action_focus(floating);
        assert_eq!(
            engine.monitors.get(&output).unwrap().workspaces[&workspace].active_window,
            Some(WindowId::new(100)),
            "a float must not replace the remembered tile"
        );

        // Floating the active tile repairs the workspace's tile memory too.
        engine.float_window(WindowId::new(100), &backend);
        assert_eq!(
            engine.monitors.get(&output).unwrap().workspaces[&workspace].active_window,
            Some(WindowId::new(103)),
            "removing the remembered tile selects the remaining tile"
        );
    }

    #[test]
    fn workspace_focus_memory_outlives_the_monitors_bounded_mru_ring() {
        let (mut engine, backend) = fixture();
        let output = engine.monitors.focused_id().unwrap();
        engine.focus_step(FocusStep::Column(2), &backend);
        engine.focus_step(FocusStep::Window(1), &backend);
        let remembered = focused(&engine);
        engine.switch_workspace(1, &backend);
        for id in 200..240 {
            let monitor = engine.monitors.get_mut(&output).unwrap();
            monitor.add_window(WindowId::new(id));
            monitor.set_focus(WindowId::new(id));
        }
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .unwrap()
                .focus_ring
                .iter_mru()
                .count(),
            32
        );
        let snapshot = engine.ipc_snapshot();
        let source = engine.monitors.get(&output).unwrap().workspaces[&0].ipc_id;
        assert_eq!(
            snapshot
                .workspaces
                .iter()
                .find(|ws| ws.id == source)
                .unwrap()
                .active_window_id,
            remembered.map(|id| id as u64)
        );
        engine.switch_workspace(0, &backend);
        assert_eq!(focused(&engine), remembered);
    }

    #[test]
    fn column_focus_restores_its_active_tile() {
        let (mut e, b) = fixture();
        e.focus_step(FocusStep::Down, &b);
        e.focus_step(FocusStep::Right, &b);
        e.focus_step(FocusStep::Down, &b);
        e.focus_step(FocusStep::Left, &b);
        assert_eq!(focused(&e), Some(101));
        e.focus_step(FocusStep::Right, &b);
        assert_eq!(focused(&e), Some(103));
    }
    #[test]
    fn column_wrapping_and_absolute_indices_do_not_cross_workspace() {
        let (mut e, b) = fixture();
        e.focus_step(FocusStep::LeftOrLast, &b);
        assert_eq!(focused(&e), Some(103));
        e.focus_step(FocusStep::RightOrFirst, &b);
        // Startup's real foreground was 100, so it supersedes older column memory.
        assert_eq!(focused(&e), Some(100));
        e.focus_step(FocusStep::Column(2), &b);
        assert_eq!(focused(&e), Some(103));
        e.focus_step(FocusStep::Column(99), &b);
        assert_eq!(focused(&e), Some(103));
        e.focus_step(FocusStep::Window(1), &b);
        assert_eq!(focused(&e), Some(102));
        e.focus_step(FocusStep::Window(99), &b);
        assert_eq!(focused(&e), Some(103));
    }
    #[test]
    fn vertical_edges_and_wraps() {
        let (mut e, b) = fixture();
        for (step, expected) in [
            (FocusStep::Bottom, 101),
            (FocusStep::DownOrTop, 100),
            (FocusStep::UpOrBottom, 101),
            (FocusStep::Top, 100),
        ] {
            e.focus_step(step, &b);
            assert_eq!(focused(&e), Some(expected));
        }
    }
    #[test]
    fn composite_tile_focus_exhausts_vertical_before_column() {
        let (mut e, b) = fixture();
        e.focus_step(FocusStep::DownOrRight, &b);
        assert_eq!(focused(&e), Some(101));
        e.focus_step(FocusStep::DownOrRight, &b);
        assert_eq!(focused(&e), Some(103));
        e.focus_step(FocusStep::UpOrLeft, &b);
        assert_eq!(focused(&e), Some(102));
        e.focus_step(FocusStep::UpOrLeft, &b);
        assert_eq!(focused(&e), Some(101));
    }
    #[test]
    fn empty_workspace_focus_is_safe_and_workspace_fallback_still_works() {
        let (mut e, b) = fixture();
        e.switch_workspace(1, &b);
        for step in [
            FocusStep::Column(1),
            FocusStep::Window(1),
            FocusStep::Top,
            FocusStep::Bottom,
            FocusStep::LeftOrLast,
            FocusStep::RightOrFirst,
            FocusStep::DownOrRight,
            FocusStep::UpOrLeft,
            FocusStep::UpOrBottom,
        ] {
            e.focus_step(step, &b);
            assert_eq!(focused(&e), None);
        }
        e.focus_step(FocusStep::UpOrWorkspace, &b);
        assert!(focused(&e).is_some());
    }
    #[test]
    fn workspace_fallback_only_runs_at_vertical_edge() {
        let (mut e, b) = fixture();
        let out = e.monitors.focused_id().unwrap();
        e.focus_step(FocusStep::DownOrWorkspace, &b);
        assert_eq!(focused(&e), Some(101));
        assert_eq!(e.monitors.get(&out).unwrap().active_workspace, 0);
        e.focus_step(FocusStep::DownOrWorkspace, &b);
        assert_eq!(e.monitors.get(&out).unwrap().active_workspace, 1);
    }
    #[test]
    fn remembered_tab_is_revealed_when_focused() {
        let (mut e, b) = fixture();
        let out = e.monitors.focused_id().unwrap();
        e.monitors
            .get_mut(&out)
            .unwrap()
            .workspace_mut()
            .unwrap()
            .columns[0]
            .switch_to_tabbed();
        e.focus_step(FocusStep::Bottom, &b);
        assert_eq!(
            e.monitors.get(&out).unwrap().workspace().unwrap().columns[0].visible_tile_indices(),
            vec![1]
        );
    }
    #[test]
    fn previous_switches_workspace_and_toggles_back() {
        let (mut e, b) = fixture();
        e.focus_action_window(WindowId::new(100), &b);
        e.switch_workspace(1, &b);
        e.add_window(
            WindowInfo {
                hwnd: 200,
                title: "other".into(),
                class_name: "fixture".into(),
                process_id: 1,
                bounds: Rect::new(0, 0, 400, 400),
                state: WindowState::Normal,
                is_visible: true,
            },
            &b,
        );
        e.focus_action_window(WindowId::new(200), &b);
        observe(&mut e, &b, 200, 1000, 1);
        e.focus_previous_for_at(WindowId::new(200), 750, 1100, &b);
        assert_eq!(focused(&e), Some(100));
        observe(&mut e, &b, 100, 1120, 2);
        e.focus_previous_for_at(WindowId::new(100), 750, 1150, &b);
        assert_eq!(focused(&e), Some(200));
    }
    #[test]
    fn previous_can_leave_and_return_to_floating_layer() {
        let (mut e, b) = fixture();
        e.focus_action_window(WindowId::new(100), &b);
        e.toggle_floating_for(WindowId::new(103), &b);
        e.focus_float(WindowId::new(103), &b);
        observe(&mut e, &b, 103, 1000, 1);
        e.focus_previous_for_at(WindowId::new(103), 750, 1100, &b);
        assert_eq!(focused(&e), Some(100));
        // Native confirmation, not a synthetic action dwell, permits the toggle.
        observe(&mut e, &b, 100, 1120, 2);
        e.focus_previous_for_at(WindowId::new(100), 750, 1150, &b);
        assert_eq!(e.os_foreground, Some(WindowId::new(103)));
    }
    #[test]
    fn tile_indices_do_not_steal_focus_from_a_float() {
        let (mut e, b) = fixture();
        e.toggle_floating_for(WindowId::new(103), &b);
        e.focus_float(WindowId::new(103), &b);
        for step in [
            FocusStep::FirstColumn,
            FocusStep::LastColumn,
            FocusStep::Column(1),
            FocusStep::Window(1),
            FocusStep::Top,
            FocusStep::Bottom,
            FocusStep::DownOrTop,
            FocusStep::UpOrBottom,
        ] {
            e.focus_step(step, &b);
            assert_eq!(e.os_foreground, Some(WindowId::new(103)));
        }
    }
    #[test]
    fn previous_can_cross_outputs_and_skips_closed_windows() {
        let (mut e, b) = fixture();
        e.focus_action_window(WindowId::new(100), &b);
        let other = OutputId::from_name("other-focus-output");
        e.register_monitor(
            other,
            Rect::new(1200, 0, 1200, 800),
            Rect::new(1200, 0, 1200, 800),
        );
        e.monitors.set_focused(other);
        e.add_window(
            WindowInfo {
                hwnd: 200,
                title: "other".into(),
                class_name: "fixture".into(),
                process_id: 1,
                bounds: Rect::new(1200, 0, 400, 400),
                state: WindowState::Normal,
                is_visible: true,
            },
            &b,
        );
        e.focus_action_window(WindowId::new(200), &b);
        observe(&mut e, &b, 200, 1000, 1);
        e.focus_previous_for_at(WindowId::new(200), 750, 1100, &b);
        assert_eq!(focused(&e), Some(100));
        assert_ne!(e.monitors.focused_id(), Some(other));
        observe(&mut e, &b, 100, 1120, 2);
        e.remove_window(WindowId::new(200), &b);
        e.focus_previous_for_at(WindowId::new(100), 750, 1150, &b);
        assert_ne!(focused(&e), Some(200));
    }
    #[test]
    fn previous_does_not_bypass_debounce_with_the_legacy_ring() {
        let (mut e, b) = fixture();
        e.focus_action_window(WindowId::new(100), &b);
        e.focus_action_window(WindowId::new(101), &b);
        observe(&mut e, &b, 101, 10, 1);
        e.focus_action_window(WindowId::new(102), &b);
        observe(&mut e, &b, 102, 20, 2);
        e.focus_previous_for_at(WindowId::new(102), 60_000, 30, &b);
        assert_eq!(focused(&e), Some(102));
        e.focus_previous_for_at(WindowId::new(102), 0, 30, &b);
        assert_eq!(focused(&e), Some(101));
    }
    #[test]
    fn short_revisit_does_not_reorder_settled_history() {
        let mut history = FocusHistory::default();
        history.record_visit(WindowId::new(1), Duration::from_secs(2), stamp(2000, 1));
        history.record_visit(WindowId::new(2), Duration::from_secs(1), stamp(3000, 2));
        history.record_visit(WindowId::new(1), Duration::from_millis(10), stamp(3010, 3));
        let settled = history
            .eligible(Duration::from_millis(750))
            .collect::<Vec<_>>();
        assert_eq!(settled, vec![WindowId::new(2), WindowId::new(1)]);
        assert_eq!(
            history.eligible(Duration::ZERO).next(),
            Some(WindowId::new(1))
        );
    }
    #[test]
    fn focus_history_skips_transient_windows_without_a_timer() {
        let mut history = FocusHistory::default();
        let delay = Duration::from_millis(750);
        history.record_visit(WindowId::new(1), delay, stamp(750, 1));
        history.record_visit(WindowId::new(2), Duration::from_millis(10), stamp(760, 2));
        assert_eq!(
            history.eligible(delay).collect::<Vec<_>>(),
            vec![WindowId::new(1)]
        );
        // Only an actual ending observation completes the next native visit.
        history.record_visit(WindowId::new(3), delay, stamp(1510, 3));
        assert_eq!(
            history.eligible(delay).collect::<Vec<_>>(),
            vec![WindowId::new(3), WindowId::new(1)]
        );
        assert_eq!(history.eligible(Duration::from_secs(5)).count(), 0);
        assert_eq!(history.eligible(Duration::ZERO).count(), 3);
        history.remove(WindowId::new(3));
        assert_eq!(
            history.eligible(delay).collect::<Vec<_>>(),
            vec![WindowId::new(1)]
        );
    }
    #[test]
    fn late_native_visits_cannot_undo_explicit_previous_endpoints() {
        let mut history = FocusHistory::default();
        let a = WindowId::new(1);
        let d = WindowId::new(4);
        history.record_visit(a, Duration::from_secs(1), stamp(1000, 1));
        history.commit_pair(d, a, 2000);
        // Delivered after the command, but occurred before it. Even a settled
        // third endpoint cannot jump ahead of the explicitly committed pair.
        history.record_visit(WindowId::new(3), Duration::from_secs(1), stamp(1900, 2));
        history.record_visit(d, Duration::from_millis(40), stamp(1940, 3));
        let previous = |history: &FocusHistory, current| {
            history
                .eligible(Duration::from_millis(750))
                .find(|w| *w != current)
        };
        assert_eq!(previous(&history, a), Some(d));
        history.commit_pair(a, d, 2100);
        history.record_visit(a, Duration::from_secs(1), stamp(1950, 4));
        assert_eq!(previous(&history, d), Some(a));
    }
    #[test]
    fn late_older_long_visit_does_not_erase_newer_short_visit() {
        let mut history = FocusHistory::default();
        let a = WindowId::new(1);
        history.record_visit(a, Duration::from_millis(10), stamp(3000, 3));
        history.record_visit(WindowId::new(2), Duration::from_secs(1), stamp(2500, 2));
        history.record_visit(a, Duration::from_secs(2), stamp(2000, 1));
        assert_eq!(history.eligible(Duration::ZERO).next(), Some(a));
        assert_eq!(
            history
                .eligible(Duration::from_millis(750))
                .collect::<Vec<_>>(),
            vec![WindowId::new(2), a],
        );
    }
    #[test]
    fn explicit_ties_and_repeated_commands_keep_exact_endpoint_order() {
        let mut history = FocusHistory::default();
        let a = WindowId::new(1);
        let d = WindowId::new(4);
        history.commit_pair(d, a, 2000);
        history.record_visit(WindowId::new(3), Duration::from_secs(1), stamp(2000, 999));
        assert_eq!(history.eligible(Duration::from_millis(750)).next(), Some(a));
        history.commit_pair(a, d, 2000);
        assert_eq!(history.eligible(Duration::from_millis(750)).next(), Some(d));
    }
}
