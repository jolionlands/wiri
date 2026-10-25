# Phase 2C — Lane D: screenshots, outputs and startup (2026-10-25)

Branch: `meylin/sa-d00aa-6b648`, based on `d84f04b`.
Reference: niri main `e19df12cf8` (v26.04 release plus main-only changes), as recorded
in the verified gap matrices. External source/docs fetches were denied in this task;
they were not retried or bypassed. No changes to another worker's worktree.

## Verification

On TRIDARIS (Windows 11 x64, 1366x768, rustc 1.95), all required commands passed with
`RUSTFLAGS=-D warnings` and `-j 2`:

- `cargo check --release --all-targets -j 2`
- `cargo test --release -j 2`: **918 unit tests** (914 library, 4 CLI), no failures;
  one pre-existing ignored doctest.
- `cargo build --release -j 2`
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\live-test.ps1 -Bin target\release -Out .live -LockWaitMinutes 60`

The final combined Lane D working tree completed the full live suite in **731.3 seconds**:
**277 boolean checks at their expected values**, including all **13 new H26-H29 checks**,
**zero unexpected boolean or numeric gate failures**. The pre-existing
`hidden_json_present_after_restart` metric is expected **false** (not a positive gate); all
other boolean checks are true. Numeric legacy gates: **0 overlapping terminal pairs**, 0 form
overlaps, 0 IPC timeouts, maximum ctl **320 ms**, 0 settled positioning calls/s, 0 unknown-key
warnings, 0 visible wiri-owned windows. All 203 completed command steps returned 0 (the initial
start marker is labelled `running`).

An earlier run passed the new checks but exposed a real H9 terminal overlap; it is not counted
as a successful full run. Original evidence remains in `.scratch/before-readiness`; a subsequent
passing retry-only candidate is in `.scratch/after-title-readiness`. The final candidate also
fixes the native-minimum border units. No existing product gate was weakened. This is a
functional live suite, not a load/stress/soak test. Final evidence remains local/ignored under
`.live` (summary, per-step JSON/logs, PNGs).

## Screenshot item

New implementation lives in `src/screenshot/`: scoped WGC/GDI capture, WIC PNG encoder,
eager registered PNG + CF_DIB clipboard transfers, strftime path handling, notification
and completion event. Hotkey, tray and existing window-capture IPC requests share it.
No new crate or lockfile dependency; only windows 0.58 API feature flags.

Live verified with real Ctrl+Alt+F8 / F9:

- H26: focused-output PNG decodes at **1366x768**, expanded date filename; both clipboard
  formats and sequence change.
- H27: focused-window WGC PNG decodes at **431x704**, contains the test form's green content,
  matches clipboard dimensions, excludes the rest of the desktop.
- H28: reload to `screenshot-path null`; both captures still copy, create no files and
  stream exactly four `screenshot_captured` events overall (two paths, two null).
- Tray notification API succeeded in the log; Windows notification policy decides visibility.

The first exploratory live run exposed process-static WinRT activation factories surviving
request-local COM/WinRT teardown: first WGC capture worked, second terminated the daemon.
Request-local uncached statics fixed this; the completed follow-up live runs captured twice.
Pure tests cover path/default filename/Unicode/percent/home/null/ISO-week boundaries,
invalid CRT conversions, DIB layout, event JSON and WIC PNG dimensions. The Pictures known
folder itself, forced PrintWindow fallback, GPU-specific/protected surfaces and multiple
physical outputs were **not** independently exercised live. WGC waits once for at most
650 ms (setup deducted); PrintWindow is the ordinary synchronous fallback.

Still partial: interactive region UI, per-action `write-to-disk`/`show-pointer`, and niri's
screenshot action argument surface. The existing hwnd/path window-capture IPC is supported.

## Live harness safety corrections

The pre-existing harness killed every process named wiri and wrote the shared user's AppData.
It now refuses a foreign existing daemon, redirects AppData/temp into this worktree's evidence,
and preserves its interrupted-run recovery journal when replacing evidence. Crash recovery
checks the current daemon PID and that **the test forms** are no longer hidden: unrelated
windows outside the scroll strip may legitimately be hidden again. These fix real ownership /
false-failure bugs; the product gates and existing phases are otherwise unchanged. Lane D is
dot-sourced once near the end of the harness.

## Added idle cost

No application idle timers, polling loops, resident worker threads, cached capture devices
or extra resident windows. The clipboard owner window and all COM/WIC/WGC/GDI objects are
created on demand and released per request; Windows may retain its own runtime thread pools.
The existing tray icon is reused, not duplicated. Baseline wiri timers/threads are unchanged.
