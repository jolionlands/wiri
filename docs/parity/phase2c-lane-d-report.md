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

## Output item

`backend/outputs.rs` separates read-only CCD/EDID identity discovery, a pure planner/preparer,
and a conservative Windows executor. Exact case-insensitive connector matches precede friendly
or make/model/serial matches; a named mismatch never falls back by enumeration index. Default
scale 0 and empty transform mean unset, and position needs an explicit config node. Refresh
comparison retains millihertz precision and unchanged timings retain their original rational.
Only explicit physical differences reach SDC_VALIDATE/SDC_APPLY; no DPI setter, primary-display
setter, ALLOW_CHANGES fallback or SAVE_TO_DATABASE is used. Invalid/unsupported requests are
logged rather than silently substituted; last-display-off and cloned-source edits fail closed.

`backend/output_runtime.rs` refreshes the actual monitor snapshot after reload/display events,
syncs surviving engine monitors without replacing their workspaces, and chooses initial focus
once in config order. `primary` is a legacy initial-focus alias, never a Windows primary setter.
Shared-file hooks are the backend exports, OutputConfig/decoder, startup/reload/display-change
calls in main and three test literals in engine; other implementation is in the new modules.

Live H29 used an unmatched off block before a lowercase connector block, explicit **current
1366x768** dimensions, and wiri-only scale **1.25**. It verified matching, initial output focus,
reload to the real Windows DPI, and identical before/after desktop state:
`[1366,768,60,0,0,0,32]` (size, integer-Hz observation, position, rotation, bit depth).
No real resolution, DPI, position, orientation, primary or off change was requested or executed.
Synthetic unit tests cover mode parsing/refresh, no-op/default/error plans, CCW rotation/axis
conversion, negative positions, EDID validation, preserved unconfigured paths/timings, cloned
sources, last-display refusal and config-order focus with two invented outputs. Test executables
cannot call the mutating display executor. A different actual mode/rotation/position or physical
disconnect/reconnect was **not live-verified**; driver acceptance remains unproven.

Still partial: inactive outputs are not auto-enabled, flipped transforms/VRR are unsupported,
scale is stored layout metadata rather than full logical-pixel sizing, per-output layout/rule
aliases still use raw names, and physical output settings apply at startup/reload (not replayed
on hotplug). Windows keeps its own arrangement and unsupported requests fail closed rather
than using niri's automatic fallback mode selection.

## Legacy terminal overlap found by verification

The first completed run's H9 measured one real **11px** terminal overlap, despite the new
screenshot/output checks passing. One terminal was admitted through its initial foreground
event, timed out asking WM_GETMINMAXINFO, and never received the already-managed foreground
retry until refocus. Main now also retries on a managed window's title-readiness event, outside
the engine lock, using the **existing three-query lifetime cap**. No timer or location polling
was added. Separately, `sync_min_widths` now converts max(native, rule) **visible-window**
minimum to tile units by adding both engine borders, just as rule limits already did.

Two new pure regressions replay three 466px minima with 4px borders (474px tiles, visible
request 466px, scrolling/no overlap), duplicate-answer suppression and rule/native precedence;
existing lifecycle/budget/movement tests were updated for the correct units. Shared-file edits
are the bounded title-event hook in main and the one-line conversion plus tests in engine.
The final unchanged H9 gate is **0 overlapping pairs**: visible x ranges are
`[-88,378]`, `[402,868]`, `[892,1358]`, all 466px wide with **24px gaps**. Dynamic changes after a
window has already answered, native minimum heights and maximum sizes remain outside this fix.

## Small Phase 3 follow-through

`hooks/startup.rs` always quotes the Run executable, even without spaces, preventing ambiguous
command parsing. Four pure tests cover spaced/unspaced, Unicode and UNC paths; included in the
918 passing unit tests. No actual Run registry value, logon, task or elevation was changed or
tested live. `docs/ELEVATION.md` compares the normal-integrity default, an explicit same-user
interactive highest-privileges scheduled task and signed uiAccess in a protected installation.
It documents UIPI/secure-desktop limits and IPC/child-token risks; neither option is enabled.
Per-user/session IPC names, elevation implementation and ARM64 release/live work remain.

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
