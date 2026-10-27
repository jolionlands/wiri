//! The daemon's async runtime, built with explicit bounds.
//!
//! `#[tokio::main]` starts one worker thread per logical CPU (8 on the reference laptop) and
//! lets the blocking pool grow to 512 threads that then linger for ten seconds.  wiri's async
//! work is small: IPC accept/read/write glue, a handful of event tasks and the animation frame
//! driver.  Everything that can wait on the engine lock or on Win32 goes through
//! `spawn_blocking` (IPC requests, the event-stream state description), and the Win32 message
//! pumps (hotkeys, hooks, tray, overlays) are plain std threads that never touch the runtime.
//!
//! Two workers keep a slow task (a `SendMessageTimeout` to a busy window while a new window is
//! admitted, an `EnumWindows` pass) from stalling the animation frames or the IPC glue; more
//! only cost a thread stack, a run queue and a parking slot each.

use std::time::Duration;

/// Worker threads that poll async tasks.
pub const WORKER_THREADS: usize = 2;

/// Upper bound on the threads `spawn_blocking` may use (IPC requests, the state description for
/// event-stream subscribers).  A request beyond the bound waits in the pool's queue; none of
/// them waits on another blocking task, so the bound cannot deadlock.
pub const MAX_BLOCKING_THREADS: usize = 4;

/// An idle blocking-pool thread exits after this long, so that the thread count falls back to
/// its settled baseline soon after an IPC burst (tokio's default is ten seconds).
pub const BLOCKING_KEEP_ALIVE: Duration = Duration::from_secs(2);

/// Build the runtime the daemon runs on.
pub fn build() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .thread_keep_alive(BLOCKING_KEEP_ALIVE)
        .thread_name("wiri-rt")
        .enable_all()
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn the_runtime_has_exactly_the_configured_workers() {
        let runtime = build().expect("runtime");
        assert_eq!(runtime.metrics().num_workers(), WORKER_THREADS);
    }

    #[test]
    fn the_blocking_pool_never_runs_more_tasks_than_its_bound() {
        let runtime = build().expect("runtime");
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        runtime.block_on(async {
            let mut tasks = Vec::new();
            for _ in 0..MAX_BLOCKING_THREADS * 4 {
                let (running, peak) = (running.clone(), peak.clone());
                tasks.push(tokio::task::spawn_blocking(move || {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(15));
                    running.fetch_sub(1, Ordering::SeqCst);
                }));
            }
            for task in tasks {
                task.await.expect("blocking task");
            }
        });
        let peak = peak.load(Ordering::SeqCst);
        assert!(
            peak <= MAX_BLOCKING_THREADS,
            "{peak} blocking tasks ran at once, the bound is {MAX_BLOCKING_THREADS}"
        );
        assert!(peak >= 1);
    }

    #[test]
    fn every_queued_blocking_task_still_completes_under_the_bound() {
        let runtime = build().expect("runtime");
        let done = Arc::new(AtomicUsize::new(0));
        runtime.block_on(async {
            let mut tasks = Vec::new();
            for _ in 0..MAX_BLOCKING_THREADS * 3 {
                let done = done.clone();
                tasks.push(tokio::task::spawn_blocking(move || {
                    done.fetch_add(1, Ordering::SeqCst);
                }));
            }
            for task in tasks {
                task.await.expect("blocking task");
            }
        });
        assert_eq!(done.load(Ordering::SeqCst), MAX_BLOCKING_THREADS * 3);
    }
}
