//! Presentation pacing for the animation clock — the part with no Win32 calls.
//!
//! niri integrates every timeline with the interval between *presented* frames: while
//! something animates the compositor schedules a redraw on vblank, and the animation clock
//! advances by the real frame time instead of a fixed step (niri
//! `e19df12cf81ffe805609fee497da1a097066ad2f`: `src/animation/clock.rs`, plus the
//! redraw/frame-callback path in `src/niri.rs`, which stops scheduling frames once nothing
//! animates).  A window manager over foreign HWNDs cannot hook a page flip, so this module
//! keeps the same contract — a wait armed only while timelines run, and deltas that are the
//! measured presentation interval — and lets `clock.rs` supply the native wait.
//!
//! Three things are decided here, so that they can be unit-tested without a compositor:
//!
//! * which wait is used (compositor clock, `DwmFlush`, or a refresh-derived sleep),
//! * the interval itself, derived from the output refresh rate rather than 16 ms,
//! * the delta handed to the timeline, from monotonic presentation timestamps, which
//!   catches up a missed frame in one step instead of bursting extra ticks.
//!
//! The pinned niri tree is not on this machine and the runner refuses network fetches, so
//! the references above are file/function level rather than line numbers.

use std::time::Duration;

/// Refresh used when neither DWM nor the display device reports one.
pub const FALLBACK_REFRESH_HZ: f64 = 60.0;
/// Below this a "refresh rate" is a bad reading, not a display.
pub const MIN_REFRESH_HZ: f64 = 10.0;
/// Above 500 Hz the reading is wrong by more than any real panel.
pub const MAX_REFRESH_HZ: f64 = 500.0;
/// One tick may not claim more animation time than this. A machine that was suspended (a
/// QPC jump of minutes) must not resolve every timeline in one invisible step.
pub const MAX_FRAME_DELTA_MS: u32 = 1_000;

/// The interval of a display refresh rate in Hz, clamped to a plausible range.
pub fn interval_from_hz(hz: f64) -> Duration {
    let hz = if hz.is_finite() && hz > 0.0 {
        hz
    } else {
        FALLBACK_REFRESH_HZ
    };
    let hz = hz.clamp(MIN_REFRESH_HZ, MAX_REFRESH_HZ);
    let micros = (1_000_000.0 / hz).round() as u64;
    Duration::from_micros(micros.max(1_000))
}

/// Windows reports refresh rates in millihertz (`59_940` for 59.94 Hz).
pub fn interval_from_millihertz(millihertz: u32) -> Duration {
    if millihertz == 0 {
        return fallback_interval();
    }
    interval_from_hz(f64::from(millihertz) / 1000.0)
}

/// A rational refresh rate (DWM's `rateRefresh`), rejecting the 0/0 "unknown" spelling.
pub fn interval_from_ratio(numerator: u32, denominator: u32) -> Option<Duration> {
    if numerator == 0 || denominator == 0 {
        return None;
    }
    Some(interval_from_hz(
        f64::from(numerator) / f64::from(denominator),
    ))
}

/// The interval a machine without a compositor wait falls back to.
pub fn fallback_interval() -> Duration {
    interval_from_hz(FALLBACK_REFRESH_HZ)
}

/// How the next frame is waited for. Ordered by preference: the compositor clock is the
/// only source that is actually the desktop's vblank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// `DCompositionWaitForCompositorClock` from `dcomp.dll`.
    CompositorClock,
    /// `DwmFlush` from `dwmapi.dll`.
    DwmFlush,
    /// A sleep of one refresh interval (no compositor clock on this machine).
    Sleep,
}

impl Backend {
    /// The first backend this machine can use.
    pub fn select(compositor_clock: bool, dwm_flush: bool) -> Self {
        if compositor_clock {
            Backend::CompositorClock
        } else if dwm_flush {
            Backend::DwmFlush
        } else {
            Backend::Sleep
        }
    }

    /// One step down the chain; idempotent at the end. A backend that failed once is
    /// replaced for the rest of the process rather than retried every frame.
    pub fn downgrade(self) -> Self {
        match self {
            Backend::CompositorClock => Backend::DwmFlush,
            Backend::DwmFlush | Backend::Sleep => Backend::Sleep,
        }
    }

    /// Whether the wait blocks a thread (and must therefore run on the blocking pool).
    pub fn blocks(self) -> bool {
        !matches!(self, Backend::Sleep)
    }

    pub fn name(self) -> &'static str {
        match self {
            Backend::CompositorClock => "DCompositionWaitForCompositorClock",
            Backend::DwmFlush => "DwmFlush",
            Backend::Sleep => "sleep",
        }
    }

    /// The longest one wait may take before it is treated as a frame anyway. A stalled or
    /// occluded compositor must not park the animation loop with no timer at all.
    pub fn wait_timeout(self, interval: Duration) -> Duration {
        match self {
            Backend::CompositorClock | Backend::DwmFlush => interval
                .saturating_mul(4)
                .clamp(Duration::from_millis(20), Duration::from_millis(250)),
            Backend::Sleep => interval,
        }
    }
}

/// How one wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// The compositor clock ticked: a frame was presented.
    Presented,
    /// The sleep fallback elapsed.
    Slept,
    /// Nothing was presented before the timeout: the wait gave up, but a frame is due.
    TimedOut,
    /// The backend call itself failed; the pacer falls back to the next backend.
    Failed,
}

/// One measured frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Elapsed presentation time since the previous frame, monotonic and capped.
    pub delta_ms: u32,
    /// Whole intervals the compositor skipped before this frame (0 for a normal frame).
    pub missed: u32,
    /// True for the first frame of a burst.
    pub first: bool,
}

/// What a finished burst did, for one log line and for the measurement test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Burst {
    pub backend: Backend,
    pub interval: Duration,
    pub frames: u32,
    pub missed: u32,
    pub timeouts: u32,
    pub max_gap: Duration,
    pub mean_gap: Duration,
    pub span: Duration,
}

/// The pacing state machine: idle (nothing armed) or running (one wait armed).
#[derive(Debug, Clone, Copy)]
pub struct Pacer {
    interval: Duration,
    backend: Backend,
    /// Timestamp of the last frame, `None` when nothing runs: no wait may be armed then.
    last: Option<Duration>,
    /// Sub-millisecond remainder, so 59.94 Hz does not lose 0.68 ms per frame.
    remainder: Duration,
    frames: u32,
    missed: u32,
    timeouts: u32,
    max_gap: Duration,
    total_gap: Duration,
    started: bool,
}

impl Pacer {
    pub fn new(backend: Backend, interval: Duration) -> Self {
        Self {
            interval,
            backend,
            last: None,
            remainder: Duration::ZERO,
            frames: 0,
            missed: 0,
            timeouts: 0,
            max_gap: Duration::ZERO,
            total_gap: Duration::ZERO,
            started: false,
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// A refresh change (a mode switch) applies from the next frame on.
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    /// True between the first animation request and the last finished timeline.
    pub fn running(&self) -> bool {
        self.last.is_some()
    }

    /// First animation request: record the baseline. Nothing is armed by this call; the next
    /// frame's delta is measured from here, and a burst therefore never inherits the idle
    /// gap that preceded it.
    pub fn start(&mut self, now: Duration) {
        self.last = Some(now);
        self.remainder = Duration::ZERO;
        self.frames = 0;
        self.missed = 0;
        self.timeouts = 0;
        self.max_gap = Duration::ZERO;
        self.total_gap = Duration::ZERO;
        self.started = true;
    }

    /// Last unfinished timeline finished: after this nothing may wait until `start` again.
    /// Returns what the burst did, or `None` when the pacer was already idle.
    pub fn stop(&mut self) -> Option<Burst> {
        let started = std::mem::replace(&mut self.started, false);
        self.last = None;
        if !started {
            return None;
        }
        let mean_gap = if self.frames == 0 {
            Duration::ZERO
        } else {
            self.total_gap / self.frames
        };
        Some(Burst {
            backend: self.backend,
            interval: self.interval,
            frames: self.frames,
            missed: self.missed,
            timeouts: self.timeouts,
            max_gap: self.max_gap,
            mean_gap,
            span: self.total_gap,
        })
    }

    /// The wait that must be armed for the next frame, or `None` while idle.
    pub fn next_wait(&self) -> Option<Duration> {
        self.last.map(|_| self.backend.wait_timeout(self.interval))
    }

    /// Replace a backend that failed, without stopping the burst.
    pub fn on_wait(&mut self, outcome: Wait) {
        match outcome {
            Wait::Failed => self.backend = self.backend.downgrade(),
            Wait::TimedOut => self.timeouts = self.timeouts.saturating_add(1),
            Wait::Presented | Wait::Slept => {}
        }
    }

    /// A frame arrived at `now` (a monotonic presentation timestamp): the timeline delta.
    ///
    /// A late frame is not repeated: the whole gap is handed over as one delta, so a
    /// timeline catches up in animation time while exactly one frame is drawn for it.
    pub fn frame(&mut self, now: Duration) -> Frame {
        let first = self.frames == 0;
        let last = match self.last {
            Some(last) => last,
            None => {
                self.start(now);
                now
            }
        };
        // Timestamps are monotonic in practice; a caller that hands back an older one must
        // not produce a negative delta or move the baseline backwards.
        let gap = now.saturating_sub(last);
        self.last = Some(if now > last { now } else { last });

        let cap = Duration::from_millis(u64::from(MAX_FRAME_DELTA_MS));
        let counted = (gap + self.remainder).min(cap);
        let delta_ms = u32::try_from(counted.as_millis()).unwrap_or(MAX_FRAME_DELTA_MS);
        self.remainder = counted.saturating_sub(Duration::from_millis(u64::from(delta_ms)));

        let intervals = if self.interval.is_zero() {
            0
        } else {
            u32::try_from(gap.as_micros() / self.interval.as_micros().max(1)).unwrap_or(u32::MAX)
        };
        let missed = intervals.saturating_sub(1);

        self.frames = self.frames.saturating_add(1);
        self.missed = self.missed.saturating_add(missed);
        self.max_gap = self.max_gap.max(gap);
        self.total_gap = self.total_gap.saturating_add(gap);
        Frame {
            delta_ms,
            missed,
            first,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::animation::{Animation, AnimationKind, Easing};

    const I60: Duration = Duration::from_micros(16_667);

    fn pacer() -> Pacer {
        Pacer::new(Backend::CompositorClock, I60)
    }

    #[test]
    fn nothing_is_armed_until_a_timeline_starts_and_after_the_last_one_ends() {
        // niri schedules a frame only while something animates (src/niri.rs redraw path);
        // the Windows equivalent is that no wait exists outside a burst.
        let mut p = pacer();
        assert!(!p.running());
        assert_eq!(p.next_wait(), None);
        assert_eq!(p.stop(), None);

        p.start(Duration::ZERO);
        assert!(p.running());
        assert_eq!(p.next_wait(), Some(I60.saturating_mul(4)));

        p.frame(I60);
        let burst = p.stop().expect("a burst was running");
        assert!(!p.running());
        assert_eq!(p.next_wait(), None);
        assert_eq!(burst.frames, 1);
        assert_eq!(burst.backend, Backend::CompositorClock);
        assert_eq!(burst.interval, I60);

        // A second burst starts from a fresh baseline: no delta spans the idle gap.
        p.start(Duration::from_secs(10));
        let frame = p.frame(Duration::from_secs(10) + I60);
        assert!(frame.first);
        assert_eq!(frame.delta_ms, 16);
    }

    #[test]
    fn backend_preference_falls_back_in_order() {
        // The order is the one Windows offers: compositor clock, then DwmFlush, then a
        // refresh-derived sleep. There is no path that arms nothing while running.
        assert_eq!(Backend::select(true, true), Backend::CompositorClock);
        assert_eq!(Backend::select(true, false), Backend::CompositorClock);
        assert_eq!(Backend::select(false, true), Backend::DwmFlush);
        assert_eq!(Backend::select(false, false), Backend::Sleep);
        assert_eq!(Backend::CompositorClock.downgrade(), Backend::DwmFlush);
        assert_eq!(Backend::DwmFlush.downgrade(), Backend::Sleep);
        assert_eq!(Backend::Sleep.downgrade(), Backend::Sleep);
        assert!(Backend::CompositorClock.blocks());
        assert!(Backend::DwmFlush.blocks());
        assert!(!Backend::Sleep.blocks());
        for backend in [Backend::CompositorClock, Backend::DwmFlush, Backend::Sleep] {
            // A stalled compositor is bounded by these timeouts; the sleep is one interval.
            let timeout = backend.wait_timeout(I60);
            assert!(timeout >= Duration::from_millis(2));
            assert!(timeout <= Duration::from_millis(250));
        }
        assert_eq!(Backend::Sleep.wait_timeout(I60), I60);
    }

    #[test]
    fn a_failed_backend_is_replaced_for_the_rest_of_the_burst() {
        let mut p = pacer();
        p.start(Duration::ZERO);
        p.on_wait(Wait::Failed);
        assert_eq!(p.backend(), Backend::DwmFlush);
        p.on_wait(Wait::Failed);
        assert_eq!(p.backend(), Backend::Sleep);
        p.on_wait(Wait::Failed);
        assert_eq!(p.backend(), Backend::Sleep);
        // A timeout is not a failure: it is a frame the compositor did not present.
        p.on_wait(Wait::TimedOut);
        p.on_wait(Wait::TimedOut);
        assert_eq!(p.backend(), Backend::Sleep);
        let burst = p.stop().expect("burst");
        assert_eq!(burst.timeouts, 2);
    }

    #[test]
    fn the_interval_comes_from_the_refresh_rate_not_a_fixed_sixteen() {
        assert_eq!(interval_from_hz(60.0), I60);
        // 59.94 Hz is the real interval of a "60 Hz" panel: 16.683 ms, not 16.667.
        assert_eq!(
            interval_from_millihertz(59_940),
            Duration::from_micros(16_683)
        );
        assert_eq!(interval_from_millihertz(60_000), I60);
        assert_eq!(
            interval_from_ratio(59_940, 1000),
            Some(Duration::from_micros(16_683))
        );
        assert_eq!(interval_from_ratio(0, 1000), None);
        assert_eq!(interval_from_ratio(60_000, 0), None);
        assert_eq!(interval_from_hz(144.0), Duration::from_micros(6_944));
        assert_eq!(interval_from_hz(165.0), Duration::from_micros(6_061));
        assert_eq!(interval_from_hz(240.0), Duration::from_micros(4_167));
        // Bad readings fall back instead of producing a busy loop or a 0 ms wait.
        assert_eq!(interval_from_hz(0.0), fallback_interval());
        assert_eq!(interval_from_hz(f64::NAN), fallback_interval());
        assert_eq!(interval_from_hz(-5.0), fallback_interval());
        assert_eq!(interval_from_hz(1.0), interval_from_hz(MIN_REFRESH_HZ));
        assert_eq!(
            interval_from_hz(100_000.0),
            interval_from_hz(MAX_REFRESH_HZ)
        );
        assert_eq!(interval_from_millihertz(0), fallback_interval());
        assert!(fallback_interval() >= Duration::from_millis(16));
        assert!(fallback_interval() < Duration::from_millis(17));
    }

    #[test]
    fn deltas_are_monotonic_and_accumulate_the_real_time() {
        let mut p = pacer();
        p.start(Duration::ZERO);
        let mut total = 0u32;
        let mut now = Duration::ZERO;
        for i in 0u64..600 {
            // 59.94 Hz with jitter, never negative in real life.
            now += I60 + Duration::from_micros((i % 7) * 100);
            total += p.frame(now).delta_ms;
        }
        // 600 frames of 16.667 ms are 10 000 ms; the sub-millisecond remainder keeps the
        // sum exact to within one millisecond instead of losing 0.667 ms per frame.
        let expected = (now.as_micros() / 1000) as u32;
        assert!(
            total.abs_diff(expected) <= 1,
            "summed {total} ms for {expected} ms of presentation time"
        );
        // A timestamp that goes backwards cannot produce a negative delta.
        let frame = p.frame(Duration::from_micros(1));
        assert_eq!(frame.delta_ms, 0);
        // A normal interval may round to 16 or 17 ms; the remainder keeps the sum exact.
        let frame = p.frame(now + I60);
        assert!((16..=17).contains(&frame.delta_ms), "{}", frame.delta_ms);
    }

    #[test]
    fn a_missed_frame_catches_up_in_one_step_instead_of_bursting() {
        // niri advances the animation clock by the presented timestamp, so a frame that
        // arrives late carries the whole interval it replaced. Drawing three frames for a
        // 50 ms gap would be a burst the user never sees presented.
        let mut p = pacer();
        p.start(Duration::ZERO);
        assert_eq!(p.frame(I60).delta_ms, 16);
        // Three intervals later: two presentations were missed. The whole 50 ms gap is one
        // frame's delta, not three ticks.
        let late = I60 * 4;
        let frame = p.frame(late);
        assert_eq!(frame.delta_ms, 50);
        assert_eq!(frame.missed, 2);
        let burst = p.stop().expect("burst");
        assert_eq!(burst.frames, 2);
        assert_eq!(burst.missed, 2);

        // The same animation time in one tick equals the ticks it replaced.
        let kind = AnimationKind::Easing {
            duration_ms: 400,
            curve: Easing::Linear,
        };
        let mut once = Animation::with_kind(0.0, 100.0, kind, 1.0);
        let mut stepped = Animation::with_kind(0.0, 100.0, kind, 1.0);
        once.tick(30);
        for _ in 0..3 {
            stepped.tick(10);
        }
        assert_eq!(once.current(), stepped.current());
        assert_eq!(once.is_finished(), stepped.is_finished());

        // A spring is sampled analytically from the elapsed time, so a late frame lands on
        // the same position the frames it replaced would have reached.
        let spring = AnimationKind::Spring(crate::layout::animation::Spring::default());
        let mut once = Animation::with_kind(0.0, 100.0, spring, 1.0);
        let mut stepped = Animation::with_kind(0.0, 100.0, spring, 1.0);
        once.tick(48);
        for _ in 0..3 {
            stepped.tick(16);
        }
        assert!((once.current() - stepped.current()).abs() < 1e-9);
    }

    #[test]
    fn a_suspended_machine_cannot_resolve_a_timeline_in_one_step() {
        let mut p = pacer();
        p.start(Duration::ZERO);
        let frame = p.frame(Duration::from_secs(600));
        assert_eq!(frame.delta_ms, MAX_FRAME_DELTA_MS);
        // 600 s at ~59.94 Hz is 35 999 intervals: every one of them was missed.
        assert_eq!(frame.missed, 35_998);
        // The trapped time is dropped, not repaid: the next frame is a normal interval.
        let frame = p.frame(Duration::from_secs(600) + I60);
        assert_eq!(frame.delta_ms, 16);
        assert_eq!(frame.missed, 0);
    }

    #[test]
    fn a_burst_reports_the_intervals_it_saw() {
        let mut p = pacer();
        p.start(Duration::ZERO);
        p.frame(I60);
        p.frame(I60 * 2);
        p.frame(I60 * 4);
        let burst = p.stop().expect("burst");
        assert_eq!(burst.frames, 3);
        assert_eq!(burst.max_gap, I60 * 2);
        assert_eq!(burst.span, I60 * 4);
        assert_eq!(burst.mean_gap, I60 * 4 / 3);
        assert_eq!(burst.missed, 1);
    }
}
