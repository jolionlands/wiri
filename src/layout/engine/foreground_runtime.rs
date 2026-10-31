//! Bridge native foreground facts into dwell history and the existing live-focus
//! path. No action snapshot enters the native reducer, and no retry clock exists.
use super::*;
use crate::backend::ForegroundStamp;
use crate::ipc::messages::IpcEvent;

/// A live-focus change `converge_live_focus` made that subscribers must still be told
/// (the stream's `window_focus_changed` comes from the state diff; this is the legacy
/// `window_focused` / `column_focused` pair the native notice would have produced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LiveFocusNotice {
    pub(super) window: WindowId,
    pub(super) location: Option<(usize, i32)>,
}

impl TilingEngine {
    /// After wiri's own activation request succeeded for `window`, make the live engine
    /// and IPC focus follow when a fresh native query says the window now is the
    /// foreground window, instead of waiting for an EVENT_SYSTEM_FOREGROUND that Windows
    /// does not always raise (it raised none for a successful `SetForegroundWindow` in
    /// the aa6c884 h15 run, and a request for the window that already is foreground
    /// never raises one). Call under the engine lock, right after the request returned.
    ///
    /// This is a native observation, not a synthetic one: it runs only when
    /// `GetForegroundWindow` itself names the requested window, mirrors that fact through
    /// the same `sync_os_foreground` path a native notice uses, and queues the same
    /// broadcast. It never touches `foreground_observations` or the dwell history, so the
    /// timeline still gets entries only from the hook FIFO; a late real notice for the
    /// window ends the previous visit at its own native time. A refused request, or one
    /// Windows accepted but did not carry out yet, moves nothing.
    pub(super) fn converge_live_focus(&mut self, window: WindowId, backend: &BackendHandle) {
        if self.os_foreground == Some(window) || !self.tiled_windows.contains_key(&window) {
            return;
        }
        if backend.foreground_window() != window.as_isize() {
            return;
        }
        let location = self.sync_os_foreground(window, backend);
        if self.os_foreground != Some(window) {
            // The open-focused policy took the focus back for another window.
            return;
        }
        self.converged_broadcast = Some(window);
        *self.live_focus_notice.lock() = Some(LiveFocusNotice { window, location });
    }

    /// The legacy broadcast for a live-focus change: `window_focused`, plus
    /// `column_focused` when the window is tiled. `managed` is the managed window the
    /// native notice resolved to (it names the workspace), which differs from `hwnd` for
    /// an owned dialog.
    pub fn live_focus_events(
        &self,
        hwnd: isize,
        managed: Option<WindowId>,
        location: Option<(usize, i32)>,
    ) -> Vec<IpcEvent> {
        let mut events = vec![IpcEvent::WindowFocused { window_hwnd: hwnd }];
        if let Some((col_idx, ws_id)) = location {
            let number = match managed.and_then(|w| self.locate_window(w)) {
                Some((oid, _, _)) => self.workspace_number_on(oid, ws_id),
                None => crate::layout::workspace_number(ws_id),
            };
            events.push(IpcEvent::ColumnFocused {
                window_hwnd: hwnd,
                column_index: col_idx,
                workspace_id: number,
            });
        }
        events
    }

    /// The broadcast `converge_live_focus` queued, for the IPC server to send once.
    pub fn take_live_focus_events(&self) -> Vec<IpcEvent> {
        match self.live_focus_notice.lock().take() {
            Some(notice) => self.live_focus_events(
                notice.window.as_isize(),
                Some(notice.window),
                notice.location,
            ),
            None => Vec::new(),
        }
    }

    /// Call once from the real startup foreground query, after initial management.
    /// `owner` is GA_ROOTOWNER (or `hwnd` when there is no different owner).
    pub fn seed_native_foreground(
        &mut self,
        hwnd: isize,
        owner: isize,
        uptime_ms: u64,
        backend: &BackendHandle,
    ) -> Option<(usize, i32)> {
        let managed = self.managed_window_for(hwnd, owner);
        self.foreground_observations.seed(uptime_ms, managed);
        self.sync_os_foreground(managed.unwrap_or_else(|| WindowId::new(hwnd)), backend)
    }

    /// Feed every native occurrence, even when its raw HWND is no longer current.
    /// Compute `is_current` from a fresh raw GetForegroundWindow comparison after
    /// classification work, immediately before this call under the engine lock.
    ///
    /// The bool permits broadcasting THIS notice as live; the location is separate
    /// because a current float/unmanaged window need not have a tiled location.
    /// A resumed previous command suppresses the old notice's live broadcast.
    pub fn observe_native_foreground(
        &mut self,
        hwnd: isize,
        owner: isize,
        stamp: ForegroundStamp,
        is_current: bool,
        backend: &BackendHandle,
    ) -> (bool, Option<(usize, i32)>) {
        // SAFETY: GetTickCount64 reads the process-independent uptime clock and
        // takes no pointers or handles.
        let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
        self.observe_native_foreground_at(hwnd, owner, stamp, is_current, now, backend)
    }

    // Injecting only the observation/command clock keeps regressions deterministic;
    // production and tests otherwise use exactly the same state machine.
    pub(super) fn observe_native_foreground_at(
        &mut self,
        hwnd: isize,
        owner: isize,
        stamp: ForegroundStamp,
        is_current: bool,
        uptime_ms: u64,
        backend: &BackendHandle,
    ) -> (bool, Option<(usize, i32)>) {
        let managed = self.managed_window_for(hwnd, owner);
        let update = self
            .foreground_observations
            .observe(stamp, managed, is_current);
        if let Some(visit) = update.completed {
            if self.tiled_windows.contains_key(&visit.window) {
                self.focus_history
                    .record_visit(visit.window, visit.dwell, stamp);
            }
        }

        // A stale B report while D is native foreground must not cancel D's
        // deferred command. A genuinely current different window does cancel it,
        // including an unmanaged/null foreground and even a duplicate notice.
        if is_current
            && self
                .pending_previous
                .is_some_and(|(expected, _)| managed != Some(expected))
        {
            self.pending_previous = None;
        }
        if !update.accepted || !update.apply_current {
            return (false, None);
        }

        let window = managed.unwrap_or_else(|| WindowId::new(hwnd));
        // The window's live-focus broadcast was already made when wiri's own request
        // converged on it: this notice still updates the state, but is not told twice.
        let already_broadcast = self.converged_broadcast.take() == Some(window);
        let location = self.sync_os_foreground(window, backend);
        if self.os_foreground != Some(window) {
            // Existing open-focused/rule policy refused this foreground.
            self.pending_previous = None;
            return (false, None);
        }
        if let Some((expected, debounce_ms)) = self.pending_previous.take() {
            if managed == Some(expected)
                && self.focus_previous_for_at(expected, debounce_ms, uptime_ms, backend)
            {
                // Unlike ordinary dispatch, this continuation has no action-tail
                // apply_all. Submit activation/layout once here, never by polling.
                self.apply_all(backend);
                return (false, None);
            }
        }
        if already_broadcast {
            return (false, None);
        }
        (true, location)
    }

    /// Forget a removed window's visits and any deferred previous command.
    pub(super) fn forget_native_foreground(&mut self, window: WindowId) {
        self.focus_history.remove(window);
        self.foreground_observations.remove(window);
        if self
            .pending_previous
            .is_some_and(|(expected, _)| expected == window)
        {
            self.pending_previous = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WindowState;

    fn stamp(uptime_ms: u64, sequence: u64) -> ForegroundStamp {
        ForegroundStamp {
            uptime_ms,
            sequence,
        }
    }

    fn fixture() -> (TilingEngine, BackendHandle) {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let output = OutputId::from_name("native-history-tests");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 800),
        );
        for hwnd in [100, 101, 102] {
            engine.add_window(
                WindowInfo {
                    hwnd,
                    title: hwnd.to_string(),
                    class_name: "fixture".into(),
                    process_id: 1,
                    bounds: Rect::new(0, 0, 400, 400),
                    state: WindowState::Normal,
                    is_visible: true,
                },
                &backend,
            );
        }
        engine.switch_workspace(1, &backend);
        engine.add_window(
            WindowInfo {
                hwnd: 200,
                title: "other workspace".into(),
                class_name: "fixture".into(),
                process_id: 1,
                bounds: Rect::new(0, 0, 400, 400),
                state: WindowState::Normal,
                is_visible: true,
            },
            &backend,
        );
        engine.focus_action_window(WindowId::new(100), &backend);
        engine.seed_native_foreground(100, 100, 0, &backend);
        (engine, backend)
    }

    fn observe(
        engine: &mut TilingEngine,
        backend: &BackendHandle,
        hwnd: isize,
        ms: u64,
        seq: u64,
        current: bool,
    ) -> bool {
        engine
            .observe_native_foreground_at(hwnd, hwnd, stamp(ms, seq), current, ms + 100, backend)
            .0
    }

    #[test]
    fn queued_b_d_resume_previous_then_toggle_exact_cross_workspace_endpoints() {
        let (mut engine, backend) = fixture();
        let a = WindowId::new(100);
        let d = WindowId::new(200);
        engine.focus_action_window(d, &backend);
        // Dispatch's real live snapshot is ahead of the native event consumer.
        engine.sync_os_foreground(d, &backend);
        assert!(!engine.focus_previous_for_at(d, 750, 1080, &backend));
        assert_eq!(engine.pending_previous, Some((d, 750)));
        let a_stamp = engine.focus_timestamps[&a];
        assert!(!observe(&mut engine, &backend, 101, 1000, 1, false));
        assert_eq!(engine.os_foreground, Some(d));
        assert_eq!(engine.pending_previous, Some((d, 750)));
        // D confirmation ends B's 40 ms visit and resumes the waiting previous.
        assert!(!observe(&mut engine, &backend, 200, 1040, 2, true));
        assert_eq!(engine.pending_previous, None);
        assert_eq!(engine.monitors.focused().unwrap().focus_window, Some(a));
        // No stale B notification or duplicate D confirmation restamps A.
        assert_eq!(engine.focus_timestamps[&a], a_stamp);
        assert!(!observe(&mut engine, &backend, 101, 1000, 1, false));
        assert!(!observe(&mut engine, &backend, 200, 1040, 2, false));
        assert!(observe(&mut engine, &backend, 100, 1200, 3, true));
        let confirmed_a = engine.focus_timestamps[&a];
        assert!(observe(&mut engine, &backend, 100, 1201, 4, true));
        assert_eq!(engine.focus_timestamps[&a], confirmed_a);
        assert!(engine.focus_previous_for_at(a, 750, 1250, &backend));
        assert_eq!(engine.monitors.focused().unwrap().focus_window, Some(d));
        assert!(observe(&mut engine, &backend, 200, 1260, 5, true));
        assert!(engine.focus_previous_for_at(d, 750, 1300, &backend));
        assert_eq!(engine.monitors.focused().unwrap().focus_window, Some(a));
    }

    #[test]
    fn genuinely_different_current_foreground_cancels_but_stale_reports_do_not() {
        let (mut engine, backend) = fixture();
        let d = WindowId::new(200);
        engine.focus_previous_for_at(d, 750, 1080, &backend);
        assert!(!observe(&mut engine, &backend, 101, 1000, 1, false));
        assert_eq!(engine.pending_previous, Some((d, 750)));
        assert!(observe(&mut engine, &backend, 102, 1020, 2, true));
        assert_eq!(engine.pending_previous, None);
        assert_eq!(engine.os_foreground, Some(WindowId::new(102)));
        assert!(!observe(&mut engine, &backend, 200, 1040, 3, false));
        assert_eq!(engine.os_foreground, Some(WindowId::new(102)));
    }

    #[test]
    fn action_selection_does_not_create_native_history() {
        let (mut engine, backend) = fixture();
        let a = WindowId::new(100);
        let b = WindowId::new(101);
        engine.focus_action_window(b, &backend);
        assert_eq!(engine.foreground_observations.current(), Some(a));
        // If selection had fabricated a B visit, zero-debounce previous on
        // confirmed A would find it. There have been no completed native visits.
        assert!(!engine.focus_previous_for_at(a, 0, 5000, &backend));
        assert_eq!(engine.pending_previous, None);
        assert!(!engine.focus_previous_for_at(b, 0, 5000, &backend));
        assert_eq!(engine.pending_previous, Some((b, 0)));
    }

    #[test]
    fn unmanaged_current_cancels_deferred_previous_and_ends_short_visit() {
        let (mut engine, backend) = fixture();
        engine.focus_previous_for_at(WindowId::new(200), 750, 50, &backend);
        assert!(observe(&mut engine, &backend, 999, 100, 1, true));
        assert_eq!(engine.pending_previous, None);
        assert!(observe(&mut engine, &backend, 101, 5000, 2, true));
        assert!(!engine.focus_previous_for_at(WindowId::new(101), 750, 5100, &backend));
    }

    #[test]
    fn second_previous_waits_for_target_confirmation_despite_intermediate_notice() {
        let (mut engine, backend) = fixture();
        let a = WindowId::new(100);
        let d = WindowId::new(200);
        engine.focus_action_window(d, &backend);
        assert!(observe(&mut engine, &backend, 200, 1000, 1, true));
        assert!(engine.focus_previous_for_at(d, 750, 1010, &backend));
        // Windows now shows A, but its notification is still behind an
        // intermediate occurrence. A second previous must await that fact.
        engine.sync_os_foreground(a, &backend);
        assert!(!engine.focus_previous_for_at(a, 750, 1020, &backend));
        assert_eq!(engine.pending_previous, Some((a, 750)));
        assert!(!observe(&mut engine, &backend, 101, 1015, 2, false));
        assert_eq!(engine.pending_previous, Some((a, 750)));
        assert!(!observe(&mut engine, &backend, 100, 1030, 3, true));
        assert_eq!(engine.pending_previous, None);
        assert_eq!(engine.monitors.focused().unwrap().focus_window, Some(d));
    }

    #[test]
    fn owned_dialog_confirmation_deduplicates_owner_and_preserves_dwell() {
        let (mut engine, backend) = fixture();
        let a = WindowId::new(100);
        let before = engine.focus_timestamps[&a];
        let (accepted, location) =
            engine.observe_native_foreground_at(999, 100, stamp(600, 1), true, 600, &backend);
        assert!(accepted);
        assert!(location.is_some());
        assert_eq!(engine.foreground_observations.current(), Some(a));
        assert_eq!(engine.focus_timestamps[&a], before);
        assert!(observe(&mut engine, &backend, 101, 750, 2, true));
        assert!(engine.focus_previous_for_at(WindowId::new(101), 750, 800, &backend));
        assert_eq!(engine.monitors.focused().unwrap().focus_window, Some(a));
    }

    // ---- explicit focus converges on the native foreground (aa6c884 h15 evidence) ----------

    use crate::backend::activation_attempt_test_support::{with_native_foreground, Activation};

    #[test]
    fn explicit_focus_of_the_window_that_already_is_native_foreground_converges() {
        let (mut engine, backend) = fixture();
        let (a, b) = (WindowId::new(100), WindowId::new(101));
        assert_eq!(engine.os_foreground, Some(a));
        let seen = (
            engine.foreground_observations.current(),
            engine.foreground_observations.last_stamp(),
        );
        // Windows already shows B (it needs no request and raises no notice), while the
        // live focus still names A.
        let (focused, attempts) = with_native_foreground(101, Activation::Refuse, || {
            engine.focus_action_window(b, &backend)
        });
        assert!(focused);
        assert!(attempts.is_empty(), "nothing to ask Windows for");
        assert_eq!(engine.os_foreground, Some(b));
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(101));
        assert_eq!(engine.monitors.focused().unwrap().focus_window, Some(b));
        // The native timeline and the dwell history did not move.
        assert_eq!(
            (
                engine.foreground_observations.current(),
                engine.foreground_observations.last_stamp()
            ),
            seen
        );
        assert_eq!(seen.0, Some(a));
        assert!(!engine.focus_previous_for_at(a, 0, 5000, &backend));
        assert_eq!(engine.pending_previous, None);
    }

    #[test]
    fn accepted_activation_without_a_native_notice_converges() {
        let (mut engine, backend) = fixture();
        let b = WindowId::new(101);
        let (_, attempts) = with_native_foreground(100, Activation::SwitchWithoutNotice, || {
            engine.focus_action_window(b, &backend)
        });
        assert_eq!(
            attempts,
            vec![101],
            "one request, then the query settles it"
        );
        assert_eq!(engine.os_foreground, Some(b));
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(101));
        assert_eq!(
            engine.foreground_observations.current(),
            Some(WindowId::new(100))
        );
    }

    #[test]
    fn refused_activation_does_not_move_live_focus() {
        let (mut engine, backend) = fixture();
        let (a, b) = (WindowId::new(100), WindowId::new(101));
        let (focused, attempts) = with_native_foreground(100, Activation::Refuse, || {
            engine.focus_action_window(b, &backend)
        });
        assert!(focused, "the selection itself still moves");
        assert!(!attempts.is_empty() && attempts.iter().all(|hwnd| *hwnd == 101));
        assert_eq!(engine.os_foreground, Some(a));
        assert_eq!(engine.ipc_snapshot().focused_window(), Some(100));
        assert!(engine.take_live_focus_events().is_empty());
    }

    #[test]
    fn deferred_acceptance_waits_for_the_native_notice() {
        let (mut engine, backend) = fixture();
        let (a, b) = (WindowId::new(100), WindowId::new(101));
        let (_, attempts) = with_native_foreground(100, Activation::AcceptDeferred, || {
            engine.focus_action_window(b, &backend)
        });
        assert!(!attempts.is_empty());
        assert_eq!(
            engine.os_foreground,
            Some(a),
            "the query does not name B yet"
        );
        // The notice that follows is the ordinary path.
        assert!(observe(&mut engine, &backend, 101, 1000, 1, true));
        assert_eq!(engine.os_foreground, Some(b));
    }

    #[test]
    fn a_native_notice_after_convergence_updates_state_and_history_but_is_not_told_twice() {
        let (mut engine, backend) = fixture();
        let (a, b) = (WindowId::new(100), WindowId::new(101));
        with_native_foreground(100, Activation::SwitchWithoutNotice, || {
            engine.focus_action_window(b, &backend)
        });
        let events = engine.take_live_focus_events();
        assert!(matches!(
            events.first(),
            Some(IpcEvent::WindowFocused { window_hwnd: 101 })
        ));
        assert!(engine.take_live_focus_events().is_empty(), "delivered once");
        // The real notice, whenever it comes, ends A's visit at its native time and is
        // the same fact the engine already follows: not broadcast again.
        assert!(!observe(&mut engine, &backend, 101, 1000, 1, true));
        assert_eq!(engine.foreground_observations.current(), Some(b));
        assert!(engine.focus_previous_for_at(b, 750, 1100, &backend));
        assert_eq!(engine.monitors.focused().unwrap().focus_window, Some(a));
        // A later current notice for another window and back is an ordinary change.
        assert!(observe(&mut engine, &backend, 102, 2000, 2, true));
        assert!(observe(&mut engine, &backend, 101, 3000, 3, true));
    }

    #[test]
    fn removal_cancels_a_deferred_command_without_recreating_history() {
        let (mut engine, backend) = fixture();
        let d = WindowId::new(200);
        engine.focus_previous_for_at(d, 750, 50, &backend);
        engine.forget_native_foreground(d);
        assert_eq!(engine.pending_previous, None);
        assert!(observe(&mut engine, &backend, 101, 1000, 1, true));
        assert_eq!(engine.pending_previous, None);
    }
}
