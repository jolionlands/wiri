//! Native foreground history and live focus are deliberately separate.
//!
//! Feed every foreground notification from the existing hook's FIFO, including
//! notifications which no longer match GetForegroundWindow. Completed visits use
//! native occurrence time, never callback/action/consumer time. Only `apply_current`
//! permits the caller to mirror a report into live engine/IPC focus state.
//!
//! This reducer does not accept activation requests or synthetic action-target
//! snapshots. Mixing those into this timeline would let a late real transition
//! end the wrong visit. It also cannot infer a transition not yet delivered by
//! Windows: an action needing that history must resume on an observation rather
//! than inventing elapsed dwell or waiting on a retry clock.

use crate::backend::ForegroundStamp;
use crate::utils::WindowId;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ForegroundVisit {
    pub(super) window: WindowId,
    pub(super) dwell: Duration,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ForegroundUpdate {
    /// False for a replayed stamp or a notice older than the native watermark.
    pub(super) accepted: bool,
    /// The caller checked that the raw event HWND is still native foreground.
    /// Recheck immediately before applying live state, under the engine lock;
    /// this flag is not a durable promise that Windows will keep that focus.
    pub(super) apply_current: bool,
    /// Record this visit in debounce history even when `apply_current` is false.
    pub(super) completed: Option<ForegroundVisit>,
}

#[derive(Debug, Clone, Copy)]
struct PendingVisit {
    window: WindowId,
    since_ms: u64,
}

#[derive(Debug, Default)]
pub(super) struct ForegroundReducer {
    last: Option<ForegroundStamp>,
    pending: Option<PendingVisit>,
}

impl ForegroundReducer {
    /// Latest managed foreground in the native occurrence stream, not a live
    /// query or requested selection. None also represents unmanaged/null focus.
    pub(super) fn current(&self) -> Option<WindowId> {
        self.pending.map(|visit| visit.window)
    }

    pub(super) fn last_stamp(&self) -> Option<ForegroundStamp> {
        self.last
    }

    /// Seed once from a real startup foreground query. Without this boundary,
    /// a window already foreground when the hook starts has no entry event.
    /// This is not for activation requests or recurring action snapshots. The
    /// zero sequence is reserved for this boundary, not a native notification.
    pub(super) fn seed(&mut self, uptime_ms: u64, window: Option<WindowId>) {
        if self.last.is_some() {
            return;
        }
        self.last = Some(ForegroundStamp {
            uptime_ms,
            sequence: 0,
        });
        self.pending = window.map(|window| PendingVisit {
            window,
            since_ms: uptime_ms,
        });
    }

    /// `window` is the managed window/owner, or None for unmanaged/null focus.
    /// `is_current` compares the raw event HWND, not that normalized owner, with
    /// a fresh native foreground query. Do not discard non-current notices
    /// before calling this method: they terminate real visits in the timeline.
    pub(super) fn observe(
        &mut self,
        stamp: ForegroundStamp,
        window: Option<WindowId>,
        is_current: bool,
    ) -> ForegroundUpdate {
        if self
            .last
            .is_some_and(|last| stamp.sequence <= last.sequence || stamp.uptime_ms < last.uptime_ms)
        {
            return ForegroundUpdate::default();
        }
        self.last = Some(stamp);

        let completed = if self.pending.map(|visit| visit.window) == window {
            // A duplicate/owned-dialog notification does not restart dwell.
            None
        } else {
            let completed = self.pending.take().map(|visit| ForegroundVisit {
                window: visit.window,
                dwell: Duration::from_millis(stamp.uptime_ms - visit.since_ms),
            });
            self.pending = window.map(|window| PendingVisit {
                window,
                since_ms: stamp.uptime_ms,
            });
            completed
        };

        ForegroundUpdate {
            accepted: true,
            apply_current: is_current,
            completed,
        }
    }

    /// Forget an open interval when its managed window is removed. The parent
    /// also removes committed history and validates HWND lifetime when mapping
    /// future notices; numeric HWND equality alone cannot establish that lifetime.
    /// Keep the watermark so a replay cannot reopen the interval.
    pub(super) fn remove(&mut self, window: WindowId) {
        if self.pending.is_some_and(|visit| visit.window == window) {
            self.pending = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(uptime_ms: u64, sequence: u64) -> ForegroundStamp {
        ForegroundStamp {
            uptime_ms,
            sequence,
        }
    }

    fn window(id: isize) -> Option<WindowId> {
        Some(WindowId::new(id))
    }

    fn visit(id: isize, dwell_ms: u64) -> Option<ForegroundVisit> {
        Some(ForegroundVisit {
            window: WindowId::new(id),
            dwell: Duration::from_millis(dwell_ms),
        })
    }

    #[test]
    fn late_short_visit_is_recorded_without_undoing_previous_endpoint() {
        let mut reducer = ForegroundReducer::default();
        reducer.observe(stamp(0, 1), window(1), true);
        // Previous has already selected A. B/D are old queued occurrences,
        // not permission to move the live focus away from A again.
        let mut live = window(1);
        let b = reducer.observe(stamp(1000, 2), window(2), false);
        assert_eq!(b.completed, visit(1, 1000));
        if b.apply_current {
            live = window(2);
        }
        let d = reducer.observe(stamp(1040, 3), window(4), false);
        assert_eq!(d.completed, visit(2, 40));
        if d.apply_current {
            live = window(4);
        }
        assert_eq!(live, window(1));
        let a = reducer.observe(stamp(1080, 4), window(1), true);
        assert!(a.apply_current);
        assert_eq!(a.completed, visit(4, 40));
    }

    #[test]
    fn batch_delivery_does_not_change_native_dwell() {
        fn replay(capture_uptime_ms: u64) -> Vec<ForegroundVisit> {
            let mut reducer = ForegroundReducer::default();
            [(1, 0, 1), (2, 1000, 2), (4, 1040, 3)]
                .into_iter()
                .filter_map(|(id, timestamp, sequence)| {
                    let stamp =
                        ForegroundStamp::from_win_event(timestamp, capture_uptime_ms, sequence);
                    reducer.observe(stamp, window(id), id == 4).completed
                })
                .collect()
        }
        assert_eq!(replay(2000), replay(1_000_000));
        assert_eq!(
            replay(2000),
            vec![visit(1, 1000).unwrap(), visit(2, 40).unwrap()]
        );
    }

    #[test]
    fn duplicate_window_keeps_original_entry_time() {
        let mut reducer = ForegroundReducer::default();
        reducer.observe(stamp(0, 1), window(1), true);
        let duplicate = reducer.observe(stamp(600, 2), window(1), true);
        assert!(duplicate.accepted);
        assert_eq!(duplicate.completed, None);
        assert_eq!(
            reducer.observe(stamp(750, 3), window(2), true).completed,
            visit(1, 750),
        );
    }

    #[test]
    fn unmanaged_and_null_focus_end_managed_dwell() {
        let mut reducer = ForegroundReducer::default();
        reducer.observe(stamp(0, 1), window(1), true);
        assert_eq!(
            reducer.observe(stamp(100, 2), None, false).completed,
            visit(1, 100),
        );
        assert_eq!(
            reducer.observe(stamp(5000, 3), window(2), true).completed,
            None
        );
        assert_eq!(
            reducer.observe(stamp(6000, 4), None, true).completed,
            visit(2, 1000),
        );
    }

    #[test]
    fn same_millisecond_a_b_a_is_ordered_not_coalesced() {
        let mut reducer = ForegroundReducer::default();
        reducer.observe(stamp(100, 1), window(1), false);
        assert_eq!(
            reducer.observe(stamp(100, 2), window(2), false).completed,
            visit(1, 0),
        );
        assert_eq!(
            reducer.observe(stamp(100, 3), window(1), true).completed,
            visit(2, 0),
        );
        assert_eq!(
            reducer.observe(stamp(100, 3), window(1), true),
            ForegroundUpdate::default(),
        );
        assert_eq!(
            reducer.observe(stamp(850, 4), window(2), true).completed,
            visit(1, 750),
        );
    }

    #[test]
    fn replay_or_regressing_occurrence_cannot_replace_pending_visit() {
        let mut reducer = ForegroundReducer::default();
        reducer.observe(stamp(100, 1), window(1), true);
        reducer.observe(stamp(200, 2), window(2), true);
        assert!(!reducer.observe(stamp(199, 3), window(1), true).accepted);
        assert!(!reducer.observe(stamp(201, 1), window(1), true).accepted);
        assert_eq!(
            reducer.observe(stamp(250, 4), window(3), true).completed,
            visit(2, 50),
        );
    }

    #[test]
    fn debounce_boundary_is_measured_at_occurrence() {
        let threshold = Duration::from_millis(750);
        for (dwell_ms, eligible) in [(749, false), (750, true), (751, true)] {
            let mut reducer = ForegroundReducer::default();
            reducer.observe(stamp(100, 1), window(1), true);
            let completed = reducer
                .observe(stamp(100 + dwell_ms, 2), window(2), true)
                .completed
                .unwrap();
            assert_eq!(completed.dwell >= threshold, eligible);
        }
    }

    #[test]
    fn removal_clears_open_visit_without_resetting_replay_watermark() {
        let mut reducer = ForegroundReducer::default();
        reducer.observe(stamp(100, 1), window(1), true);
        reducer.remove(WindowId::new(1));
        assert!(!reducer.observe(stamp(100, 1), window(1), true).accepted);
        assert_eq!(
            reducer.observe(stamp(200, 2), window(2), true).completed,
            None
        );
    }

    #[test]
    fn startup_query_seeds_only_once_and_rejects_pre_start_notices() {
        let mut reducer = ForegroundReducer::default();
        reducer.seed(100, window(1));
        reducer.seed(500, window(2));
        assert!(!reducer.observe(stamp(90, 1), window(3), false).accepted);
        assert_eq!(
            reducer.observe(stamp(1000, 2), window(2), true).completed,
            visit(1, 900),
        );
        // A later startup path must not replace a native observation either.
        reducer.seed(1100, window(3));
        assert_eq!(
            reducer.observe(stamp(1200, 3), None, true).completed,
            visit(2, 200),
        );
    }

    #[test]
    fn native_timestamp_is_extended_across_rollover() {
        let rollover = 1_u64 << 32;
        let old = ForegroundStamp::from_win_event(u32::MAX - 9, rollover + 25, 1);
        let new = ForegroundStamp::from_win_event(20, rollover + 1000, 2);
        assert_eq!(old.uptime_ms, rollover - 10);
        assert_eq!(new.uptime_ms, rollover + 20);
        let mut reducer = ForegroundReducer::default();
        reducer.observe(old, window(1), false);
        assert_eq!(
            reducer.observe(new, window(2), true).completed,
            visit(1, 30)
        );
    }
}
