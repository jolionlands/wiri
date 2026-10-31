//! The animation frame clock: presentation-paced waits with real frame timestamps.
//!
//! The pure decisions (which wait, which interval, which delta) live in `pacing.rs`; this
//! module supplies the three native waits and the async driver.
//!
//! * `DCompositionWaitForCompositorClock` (`dcomp.dll`, exported by Windows 11) blocks
//!   until the next DWM compositor clock tick. It is loaded with `GetProcAddress` on first
//!   use, so wiri still starts (and animates) on a machine without the export.
//! * `DwmFlush` (`dwmapi.dll`) blocks until the next composition pass.
//! * Otherwise a sleep of one refresh interval, derived from DWM's composition timing or
//!   the display mode instead of the old hard-coded 16 ms.
//!
//! Both blocking waits run on tokio's blocking pool, never on a runtime worker holding the
//! engine lock and never on a hook thread. Nothing at all is armed while no timeline runs:
//! the first animation request sets the baseline, the last finished timeline disarms it and
//! drops the summary line (`animation clock: ...`, one per burst).
//!
//! niri renders on vblank while something animates and advances its animation clock with
//! the presented frame time (niri `e19df12cf81ffe805609fee497da1a097066ad2f`:
//! `src/animation/clock.rs` and the frame-callback path in `src/niri.rs`). Windows offers no
//! foreign-window page-flip callback, so the compositor clock above is the closest source.

use super::pacing::{
    fallback_interval, interval_from_hz, interval_from_ratio, Backend, Frame, Pacer, Wait,
};
use std::time::{Duration, Instant};
use tracing::debug;

/// `DCompositionWaitForCompositorClock` returns these `WAIT_*` codes.
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;

/// `DWORD DCompositionWaitForCompositorClock(UINT count, const HANDLE *handles, DWORD timeout)`.
type CompositorClockWait = unsafe extern "system" fn(u32, *const isize, u32) -> u32;

/// Resolved once per process. `dcomp.dll` stays loaded: the returned address has to remain
/// valid for the process lifetime, and only an animating session ever loads it.
fn compositor_clock_wait_fn() -> Option<CompositorClockWait> {
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    static ADDRESS: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let address = *ADDRESS.get_or_init(|| unsafe {
        // System32 only: never let a DLL in the current directory answer this name.
        let module = LoadLibraryExW(w!("dcomp.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let address = GetProcAddress(module, s!("DCompositionWaitForCompositorClock"))?;
        Some(address as usize)
    });
    // SAFETY: the address is the documented export with exactly this signature.
    address.map(|address| unsafe { std::mem::transmute::<usize, CompositorClockWait>(address) })
}

/// One compositor-clock wait. `timeout_ms` bounds a compositor that stops presenting (an
/// occluded desktop, a locked session), so a timeline still advances instead of parking.
fn compositor_clock_wait(timeout_ms: u32) -> Wait {
    let Some(wait) = compositor_clock_wait_fn() else {
        return Wait::Failed;
    };
    // SAFETY: no handles are waited on, the timeout is a plain millisecond count.
    let code = unsafe { wait(0, std::ptr::null(), timeout_ms) };
    match code {
        WAIT_OBJECT_0 => Wait::Presented,
        WAIT_TIMEOUT => Wait::TimedOut,
        _ => Wait::Failed,
    }
}

fn dwm_flush() -> Wait {
    match unsafe { windows::Win32::Graphics::Dwm::DwmFlush() } {
        Ok(()) => Wait::Presented,
        Err(_) => Wait::Failed,
    }
}

/// Which backend this machine offers; called once, on the first animation request.
fn probe() -> Backend {
    // DwmFlush is exported by every supported Windows, so the choice is between the
    // compositor clock and it.
    Backend::select(compositor_clock_wait_fn().is_some(), true)
}

/// The current output refresh interval, and where the number came from. DWM's composition
/// timing is what the desktop actually runs at; the display mode is the fallback.
fn current_interval() -> (Duration, &'static str) {
    use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
    use windows::Win32::Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS};
    let mut info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    if unsafe { DwmGetCompositionTimingInfo(None, &mut info) }.is_ok() {
        if let Some(interval) =
            interval_from_ratio(info.rateRefresh.uiNumerator, info.rateRefresh.uiDenominator)
        {
            return (interval, "dwm-timing");
        }
    }
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // A null device name asks for the current display settings of the calling thread's
    // display; that is the monitor wiri is animating on.
    if unsafe { EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool()
        && mode.dmDisplayFrequency > 1
    {
        return (
            interval_from_hz(f64::from(mode.dmDisplayFrequency)),
            "display-mode",
        );
    }
    (fallback_interval(), "fallback")
}

/// Process CPU time (kernel + user) for the measurement test, like the compositor's own.
#[cfg(test)]
fn cpu_ms() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .is_err()
        {
            return 0.0;
        }
        let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        (ticks(kernel) + ticks(user)) as f64 / 10_000.0
    }
}

fn millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

/// The daemon's frame clock. Cheap to construct: every native resource (the `dcomp.dll`
/// probe, the refresh query) is resolved on the first frame of the first animation.
pub struct FrameClock {
    origin: Instant,
    pacer: Pacer,
    resolved: bool,
    interval_source: &'static str,
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameClock {
    /// No Win32 call, no thread and no timer: this runs at startup, when nothing animates.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            pacer: Pacer::new(Backend::Sleep, fallback_interval()),
            resolved: false,
            interval_source: "unresolved",
        }
    }

    pub fn backend(&self) -> Backend {
        self.pacer.backend()
    }

    pub fn interval(&self) -> Duration {
        self.pacer.interval()
    }

    /// True between the first animation request and the last finished timeline.
    pub fn is_running(&self) -> bool {
        self.pacer.running()
    }

    /// Frame timestamps are `Instant`s: QueryPerformanceCounter on Windows, the same domain
    /// the compositor clock ticks in, and monotonic.
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    /// First frame of a burst. The backend is chosen once per process (a backend that
    /// failed stays replaced); the refresh interval is re-read because a mode switch or a
    /// window moved to another output changes it. Arms no wait.
    fn arm(&mut self) {
        if !self.resolved {
            let interval = self.pacer.interval();
            self.pacer = Pacer::new(probe(), interval);
            self.resolved = true;
        }
        let (interval, source) = current_interval();
        self.pacer.set_interval(interval);
        self.interval_source = source;
        self.pacer.start(self.now());
    }

    /// Wait for the next presented frame and report the animation delta for it.
    pub async fn next_frame(&mut self) -> Frame {
        if !self.pacer.running() {
            self.arm();
        }
        let timeout = self
            .pacer
            .next_wait()
            .unwrap_or_else(|| self.pacer.interval());
        let backend = self.pacer.backend();
        let outcome = Self::wait(backend, timeout).await;
        self.pacer.on_wait(outcome);
        self.pacer.frame(self.now())
    }

    async fn wait(backend: Backend, timeout: Duration) -> Wait {
        match backend {
            // No compositor clock on this machine: the interval itself is the frame clock.
            // tokio's timer is coarser than the compositor clock; that is stated in the
            // matrix row and in the perf notes.
            Backend::Sleep => {
                tokio::time::sleep(timeout).await;
                Wait::Slept
            }
            Backend::CompositorClock => {
                let timeout_ms = millis(timeout);
                let task = tokio::task::spawn_blocking(move || compositor_clock_wait(timeout_ms));
                // The wait itself gives up after `timeout`, but a thread has to be free
                // first: the blocking pool is capped at four threads (src/runtime.rs) and
                // IPC work runs on it too, so a busy pool would otherwise hold
                // `next_frame()` and make the animation jump. Allow one `timeout` of queue
                // and then fall back exactly like the DwmFlush arm.
                match tokio::time::timeout(timeout.saturating_mul(2), task).await {
                    Ok(Ok(outcome)) => outcome,
                    // The blocking task panicked or was cancelled by shutdown, or the pool
                    // stayed busy long enough that this frame is late anyway.
                    Ok(Err(_)) | Err(_) => Wait::Failed,
                }
            }
            Backend::DwmFlush => {
                // DwmFlush has no timeout argument, so the bound is enforced from here. A
                // wait that overruns means this compositor is not presenting; the backend
                // is replaced instead of leaving one blocked thread per frame behind.
                let task = tokio::task::spawn_blocking(dwm_flush);
                match tokio::time::timeout(timeout, task).await {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(_)) => Wait::Failed,
                    Err(_) => Wait::Failed,
                }
            }
        }
    }

    /// The last timeline finished. Nothing waits after this, and one line records what the
    /// burst saw (the live gate reads it; `-v` enables it).
    pub fn disarm(&mut self) {
        if let Some(burst) = self.pacer.stop() {
            debug!(
                "animation clock: backend={} interval_us={} refresh={} frames={} missed={} timeouts={} mean_us={} max_us={} span_ms={}",
                burst.backend.name(),
                burst.interval.as_micros(),
                self.interval_source,
                burst.frames,
                burst.missed,
                burst.timeouts,
                burst.mean_gap.as_micros(),
                burst.max_gap.as_micros(),
                burst.span.as_millis(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_clock_touches_nothing_and_arms_no_wait() {
        // Construction happens at startup: no dcomp.dll load, no refresh query, no timer.
        let clock = FrameClock::new();
        assert!(!clock.is_running());
        assert_eq!(clock.pacer.next_wait(), None);
        // Until the first animation resolves them, the interval is the 60 Hz fallback:
        // 16.667 ms, not the old hard-coded 16 ms.
        assert_eq!(clock.interval(), Duration::from_micros(16_667));
        assert!(clock.interval() > Duration::from_millis(16));
    }

    /// The blocking pool is capped and shared with IPC work (src/runtime.rs), so the
    /// compositor-clock join is bounded like the DwmFlush one: a busy pool makes this frame
    /// late, it does not park `next_frame()` until a thread frees.
    #[test]
    fn a_busy_blocking_pool_cannot_stall_the_compositor_clock_wait() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_time()
            .build()
            .expect("a current-thread runtime with timers");
        // Hold the pool's only thread; the bound keeps a failing run from hanging.
        let (release, held) = std::sync::mpsc::channel::<()>();
        let _held = runtime.spawn_blocking(move || {
            let _ = held.recv_timeout(Duration::from_millis(600));
        });
        std::thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        let outcome = runtime.block_on(FrameClock::wait(
            Backend::CompositorClock,
            Duration::from_millis(50),
        ));
        let elapsed = started.elapsed();
        let _ = release.send(());

        assert_eq!(outcome, Wait::Failed, "the wait gave up, as DwmFlush does");
        assert!(
            elapsed < Duration::from_millis(300),
            "the frame clock waited {elapsed:?} for a free blocking thread"
        );
    }

    /// One short measurement, not a stress test: 120 presented frames through the backend
    /// this machine selects.
    ///
    /// `cargo test --release -j 2 -- --ignored --nocapture compositor_frame_cadence`
    #[test]
    #[ignore = "measurement: waits for 120 real compositor frames (about two seconds)"]
    fn compositor_frame_cadence_measurement() {
        let runtime = crate::runtime::build().expect("runtime");
        let cpu_before = cpu_ms();
        let started = Instant::now();
        let mut clock = FrameClock::new();
        let mut intervals = Vec::with_capacity(120);
        runtime.block_on(async {
            // The first frame of a burst only sets the baseline.
            clock.next_frame().await;
            let mut last = Instant::now();
            for _ in 0..120 {
                clock.next_frame().await;
                let now = Instant::now();
                intervals.push(now.duration_since(last));
                last = now;
            }
            clock.disarm();
        });
        let elapsed = started.elapsed();
        let cpu = cpu_ms() - cpu_before;
        let mut sorted = intervals.clone();
        sorted.sort();
        let at = |q: f64| -> f64 {
            let index = ((sorted.len() as f64 - 1.0) * q).round() as usize;
            sorted[index].as_secs_f64() * 1000.0
        };
        let mean = intervals.iter().map(|d| d.as_secs_f64()).sum::<f64>() / intervals.len() as f64;
        println!(
            "animation clock measurement: backend={} interval_us={} refresh={} frames={} \
             mean_ms={:.3} median_ms={:.3} p95_ms={:.3} max_ms={:.3} \
             wall_ms={:.1} process_cpu_ms={:.1} cpu_per_frame_ms={:.3}",
            clock.backend().name(),
            clock.interval().as_micros(),
            clock.interval_source,
            intervals.len(),
            mean * 1000.0,
            at(0.5),
            at(0.95),
            at(1.0),
            elapsed.as_secs_f64() * 1000.0,
            cpu,
            cpu / intervals.len() as f64,
        );
        assert_eq!(intervals.len(), 120);
        // A presented frame cannot be instantaneous, and a cadence that never advances
        // would be a stalled clock the test has to report rather than pass silently.
        assert!(at(0.5) > 0.0);
    }
}
