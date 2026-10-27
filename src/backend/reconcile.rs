//! When the safety-net reconcile pass runs.
//!
//! The pass (`reconcile_visible_windows` in `main.rs`) catches windows that never produced a
//! usable CREATE/SHOW event (common with launchers, and a UWP window that is uncloaked
//! without a SHOW) and re-hides an inactive-workspace window the shell showed again.  It used
//! to run on a fixed 750 ms interval for the whole life of the process: an `EnumWindows` over
//! every top-level window, a window-info read for each visible one and an `apply_all`, 1.3
//! times a second on an idle desktop.
//!
//! The pass is only useful soon after something happened to the windows, so it is armed by
//! window events instead: an event schedules a pass one interval later, each pass that finds
//! nothing to admit schedules the next one until [`QUIET_PASSES_TO_REST`] of them in a row
//! did, and then the schedule rests with no deadline at all.  A window that needs a pass
//! produces events (the shell raises SHOW/NAMECHANGE/FOREGROUND for it), and a late
//! uncloak/title is covered by the follow-up passes of the preceding events.

use std::time::{Duration, Instant};

use super::BackendEvent;

/// The distance between two passes while the schedule is armed (the old fixed interval).
pub const PASS_INTERVAL: Duration = Duration::from_millis(750);

/// Passes in a row that admitted no window before the schedule goes to rest (4.5 s).
pub const QUIET_PASSES_TO_REST: u32 = 6;

/// The deadline of the next reconcile pass, or none when the schedule rests.
#[derive(Debug, Clone)]
pub struct ReconcileSchedule {
    next: Option<Instant>,
    quiet_passes: u32,
}

impl ReconcileSchedule {
    /// A schedule whose first pass (the startup pass) is due at `now`.
    pub fn starting(now: Instant) -> Self {
        Self {
            next: Some(now),
            quiet_passes: 0,
        }
    }

    /// When the next pass is due.  `None`: at rest, no timer is needed.
    pub fn deadline(&self) -> Option<Instant> {
        self.next
    }

    /// A window event arrived at `now`: a pass follows within one interval, and the run of
    /// quiet passes starts over.  A pass that is already scheduled keeps its time, so a
    /// steady stream of events cannot push the pass back forever.
    pub fn note_activity(&mut self, now: Instant) {
        self.quiet_passes = 0;
        if self.next.is_none() {
            self.next = Some(now + PASS_INTERVAL);
        }
    }

    /// The pass that was due ran at `now`; `admitted` says it added a window to the layout.
    pub fn pass_done(&mut self, now: Instant, admitted: bool) {
        self.quiet_passes = if admitted { 0 } else { self.quiet_passes + 1 };
        self.next = (self.quiet_passes < QUIET_PASSES_TO_REST).then(|| now + PASS_INTERVAL);
    }
}

/// Whether `event` can mean that a window needs admitting or re-hiding, and so arms the
/// schedule.  The high-rate and purely informational events do not: a window that moves
/// while it is dragged or animated, a z-order change, a title change that is not also a
/// SHOW (NAMECHANGE sends both), urgency and the CBT notices.
pub fn event_arms_reconcile(event: &BackendEvent) -> bool {
    matches!(
        event,
        BackendEvent::WindowCreated { .. }
            | BackendEvent::WindowDestroyed { .. }
            | BackendEvent::WindowShown { .. }
            | BackendEvent::WindowHidden { .. }
            | BackendEvent::WindowMinimized { .. }
            | BackendEvent::WindowRestored { .. }
            | BackendEvent::ForegroundChanged { .. }
            | BackendEvent::MonitorConnected { .. }
            | BackendEvent::MonitorDisconnected { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ForegroundStamp;
    use crate::utils::Rect;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn the_startup_pass_is_due_at_once() {
        let t0 = Instant::now();
        assert_eq!(ReconcileSchedule::starting(t0).deadline(), Some(t0));
    }

    #[test]
    fn quiet_passes_run_every_interval_and_then_the_schedule_rests() {
        let t0 = Instant::now();
        let mut schedule = ReconcileSchedule::starting(t0);
        let mut now = t0;
        for pass in 1..=QUIET_PASSES_TO_REST {
            assert!(schedule.deadline().is_some(), "pass {pass} must be due");
            schedule.pass_done(now, false);
            if pass < QUIET_PASSES_TO_REST {
                assert_eq!(schedule.deadline(), Some(now + PASS_INTERVAL));
                now += PASS_INTERVAL;
            }
        }
        assert_eq!(
            schedule.deadline(),
            None,
            "after {QUIET_PASSES_TO_REST} quiet passes there is no timer"
        );
    }

    #[test]
    fn a_pass_that_admits_a_window_restarts_the_quiet_run() {
        let t0 = Instant::now();
        let mut schedule = ReconcileSchedule::starting(t0);
        for _ in 0..QUIET_PASSES_TO_REST - 1 {
            schedule.pass_done(t0, false);
        }
        assert!(schedule.deadline().is_some());
        schedule.pass_done(t0, true);
        // Another full run of quiet passes is needed before it rests.
        for _ in 0..QUIET_PASSES_TO_REST - 1 {
            assert!(schedule.deadline().is_some());
            schedule.pass_done(t0, false);
        }
        assert!(schedule.deadline().is_some());
        schedule.pass_done(t0, false);
        assert_eq!(schedule.deadline(), None);
    }

    #[test]
    fn activity_wakes_a_resting_schedule_one_interval_later() {
        let t0 = Instant::now();
        let mut schedule = ReconcileSchedule::starting(t0);
        for _ in 0..QUIET_PASSES_TO_REST {
            schedule.pass_done(t0, false);
        }
        assert_eq!(schedule.deadline(), None);
        let event = at(t0, 10_000);
        schedule.note_activity(event);
        assert_eq!(schedule.deadline(), Some(event + PASS_INTERVAL));
        // ... and the full run of quiet passes follows it.
        let mut now = event + PASS_INTERVAL;
        let mut passes = 0;
        while schedule.deadline().is_some() {
            schedule.pass_done(now, false);
            now += PASS_INTERVAL;
            passes += 1;
            assert!(passes <= QUIET_PASSES_TO_REST, "the schedule must rest");
        }
        assert_eq!(passes, QUIET_PASSES_TO_REST);
    }

    #[test]
    fn a_steady_stream_of_events_cannot_postpone_a_scheduled_pass() {
        let t0 = Instant::now();
        let mut schedule = ReconcileSchedule::starting(t0);
        schedule.pass_done(t0, false);
        let due = schedule.deadline().expect("armed");
        for ms in (0..700).step_by(50) {
            schedule.note_activity(at(t0, ms));
            assert_eq!(schedule.deadline(), Some(due), "event at {ms} ms");
        }
    }

    #[test]
    fn activity_restarts_the_quiet_run_of_an_armed_schedule() {
        let t0 = Instant::now();
        let mut schedule = ReconcileSchedule::starting(t0);
        for _ in 0..QUIET_PASSES_TO_REST - 1 {
            schedule.pass_done(t0, false);
        }
        schedule.note_activity(t0);
        // Without the event the next quiet pass would have rested; now it does not.
        schedule.pass_done(t0, false);
        assert!(schedule.deadline().is_some());
    }

    #[test]
    fn only_window_lifecycle_events_arm_the_schedule() {
        let rect = Rect::new(0, 0, 10, 10);
        let stamp = ForegroundStamp {
            uptime_ms: 1,
            sequence: 1,
        };
        for event in [
            BackendEvent::WindowCreated { hwnd: 1 },
            BackendEvent::WindowDestroyed { hwnd: 1 },
            BackendEvent::WindowShown { hwnd: 1 },
            BackendEvent::WindowHidden { hwnd: 1 },
            BackendEvent::WindowMinimized { hwnd: 1 },
            BackendEvent::WindowRestored { hwnd: 1 },
            BackendEvent::ForegroundChanged { hwnd: 1, stamp },
            BackendEvent::MonitorConnected { id: 1 },
            BackendEvent::MonitorDisconnected { id: 1 },
        ] {
            assert!(event_arms_reconcile(&event), "{event:?}");
        }
        for event in [
            BackendEvent::WindowMoved { hwnd: 1, rect },
            BackendEvent::WindowResized { hwnd: 1, rect },
            BackendEvent::WindowTitleChanged { hwnd: 1 },
            BackendEvent::ZOrderChanged,
            BackendEvent::WindowMoveResizeStart { hwnd: 1 },
            BackendEvent::WindowMoveResizeEnd { hwnd: 1 },
            BackendEvent::CbtCreateWindow { hwnd: 1 },
            BackendEvent::CbtSetFocus { hwnd: 1 },
        ] {
            assert!(!event_arms_reconcile(&event), "{event:?}");
        }
    }
}
