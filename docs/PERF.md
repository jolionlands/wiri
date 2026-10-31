# Phase 4: measured baseline, not performance gates

Lane E is **measurement only**: no `src/`, dependency, runtime, or release-profile changes.
All `p_*` properties in `summary.json` are numeric **information**, never pass/fail.
`-1` means unavailable, not zero cost or a passing sample. Existing live gates are untouched.
The proposed budgets below are future acceptance criteria, not enforced by this collector.

## Tridaris R3 observation, 2026-10-29 UTC (`E8C1274F` source archive)

The frozen full run `C:\wiri-dev\evidence\parity-full-r3-20261028-234058`
finished with 735 passing and 111 failing desktop gates, 94 information entries
and one cursor-restoration phase error. Builds remained stopped throughout this
run. All 64 new H81-H86 checks passed, but the whole run is not qualified.

| Observation, 28 managed windows / two sentinels | Result |
| --- | ---: |
| First complete IPC reply / first observed layout | 102.69 / 87.01 ms, one cold start |
| Idle CPU / interval | Below counter resolution / 30.006 s |
| Idle input-tick changes / positioning lines | 0 / 0 |
| Idle context switches, endpoint lower bound | 14.73 per second |
| Private memory, startup / after sampled actions | 13.77 / 13.61 MiB |
| Working set, startup / after sampled actions | 35.51 / 36.82 MiB |
| Threads, startup / after sampled actions | 14 / 15 |
| Hotkey samples, valid / timeout / invalid preflight | 0 / 0 / 1 |
| Hotkey latency | Unavailable |

The collector requested focus of its exact owned sentinel, HWND 62067746, but
Windows refused native activation; foreground remained HWND 86250240. The
preflight correctly stopped before sending the hotkey batch. That foreground HWND was
later identified as a visible elevated Windows Terminal window that predated
this run. The evidence establishes activation refusal, not why Windows refused.
It does not establish that all desktop failures have the same cause.

A subsequent owned-window diagnostic on the same frozen binary completed four
comparisons with clean cleanup. All 16 observations through 1,500 ms showed the
requested owned window in the foreground. The initial foreground already belonged
to the collector, so this did not reproduce the earlier Terminal-to-sentinel
refusal or qualify hotkey latency. Collector, daemon and Terminal had the same
integrity level in that diagnostic; this does not establish their earlier state.
The original failed performance observation remains retained.

A separate diagnostic retained the original daemon-first fixture startup order.
Its first owned focus request succeeded from a medium-integrity PowerShell
foreground window to the initially hidden, elevated owned target. That target
remained foreground through 1,500 ms. This does not reproduce the original
elevated-Terminal refusal. The passive state snapshots also add observation
overhead; this diagnostic supplies no hotkey latency measurement.

The idle counters and resource endpoints describe this workload. The action
sequence omitted all ten hotkeys, so its final memory snapshot is not a matched
comparison with f217. Two endpoints prove neither a leak nor a plateau. The
single cold sample also has clock-mapping uncertainty (observer offsets were
negative); its difference from the earlier run cannot establish a regression.

Next qualify owned-target native foreground acknowledgement, then repeat the
perf-only phase on the frozen binary. Retain the pre-injection refusal and record
target/foreground ownership, focus reply and activation log. A later target-side
geometry acknowledgement observer can separate collector wake delay; displayed
frame timing requires separate evidence.

## Historical Tridaris full-baseline checkpoint, 2026-10-28 (`f217e15`)

The frozen full run at `C:\wiri-dev\evidence\parity-full-f217e15-20261028`
recorded the following with 26 managed windows and two existing sentinels:

| Observation | Result |
| --- | ---: |
| First complete IPC reply / first observed layout | 82.30 / 65.13 ms, one cold start |
| Idle process CPU / interval | Below the counter increment / 30.005 s |
| Idle placement lines / context switches | 0 / 17.20 per second |
| Hotkey to first observed rectangle change | 27.52 ms median, 33.47 ms maximum, ten keys |
| Largest rectangle-observer gap | 33.47 ms |
| Private bytes, startup / after actions | 8.11 / 8.24 MiB |
| Working set, startup / after actions | 24.02 / 26.02 MiB |
| Threads, startup / after actions | 14 / 15 |
| Collector errors | 0 |

The run's verdict records concurrent compilation. It is not a quiet performance
qualification, and the observer gap cannot resolve a 16 ms movement target.
Two memory endpoints establish neither a leak nor a stable long-term plateau.
These measurements precede the current startup, native sizing and activation fixes;
they are not a matched comparison with current source. Command round-trip timings from
focused gates are a separate measurement from hotkey-to-rectangle latency.

Next: repeat the normal-desktop collector on the final frozen binary after builds
stop, improve the external acknowledgement observer before enforcing sub-frame
limits, and separately observe focus/cursor completion and animation cadence.
The older poller inventory below is historical: the current main loop, reconciliation
and animation wakeups are demand-driven, and the runtime already bounds its workers.
Remaining source-backed candidates include the synchronous 40 ms activation fallback
under the engine lock; measure that path before changing it and retain final-geometry
delivery guarantees. Active animation pacing is no longer a candidate: the fixed 16 ms
sleep is gone (Lane PACE, 2026-10-30, measured below), replaced by the desktop's
presentation clock.

## Animation clock measurement, 2026-10-30 (Lane PACE)

The animation frame worker used to `sleep(16 ms)` between ticks inside its Tokio task. While
timelines run it now waits for the desktop's presentation clock — `DCompositionWaitForCompositorClock`
(`dcomp.dll`, resolved with `GetProcAddress` on the first animation so wiri also starts without
it) on a blocking-pool task, then `DwmFlush`, then a sleep of one interval derived from the
output refresh rate (`DwmGetCompositionTimingInfo().rateRefresh`, else
`EnumDisplaySettingsW().dmDisplayFrequency`, else 60 Hz). Frame timestamps are `Instant`s
(QueryPerformanceCounter, the compositor clock's domain), so a spring or easing advances by the
measured presentation interval. The wait is armed by the first animation request and disarmed
when the last timeline finishes (`src/animation/`); nothing at all is armed at rest.

Method: one `#[ignore]` measurement test, one run, no other workload of ours, on TRIDARIS
(Windows 11 22631, one 1366x768 display at 96 DPI) built from `2bce5dd9` plus this lane:

```powershell
cargo test --release -j 2 --lib -- --ignored --nocapture compositor_frame_cadence_measurement
```

It waits for 120 frames through the same async path the daemon uses, and measures each interval
from the end of one wait to the end of the next. CPU is this process's kernel+user time
(`GetProcessTimes`), so it includes the test's own work, not only the waits.

| Observation, 121 waits / 120 measured intervals | Result |
| --- | ---: |
| Chosen backend / refresh source | `DCompositionWaitForCompositorClock` / `dwm-timing` |
| Interval used | 16.665 ms |
| Mean / median interval | 16.664 / 16.663 ms |
| p95 / max interval | 16.703 / 16.735 ms |
| Wall time | 2 018 ms |
| Process CPU / per interval | 15.6 ms / 0.130 ms |

This is one short sample, not a stress or soak test, and other lanes were compiling on the same
laptop during it. It proves the compositor clock is reachable here, that the chosen backend paces
at the reported refresh (median 2 µs below the interval, p95 38 µs above it) and what one frame
costs this process. It does not prove displayed-frame cadence, a vsync-locked flip (DWM still
composites), idle CPU under the official collector, or any other refresh rate, output count or
RDP session. While nothing animates the frame worker parks on its notifier: no timer, no blocking
wait and no thread of its own. During a burst one of the runtime's four blocking threads is
occupied by the compositor wait, so the bound in `src/runtime.rs` is unchanged and still
applies. Both facts, and the remaining cadence/CPU budget claims, are for the lead's live cycle
to measure: the `h90_*` gates in `scripts/live-phases/lane-pace.ps1` read the one
`animation clock: backend=... frames=... mean_us=... max_us=...` line a burst logs at debug
level (`-v`) and the process CPU time of a settled daemon.

## Reproduce

From an interactive Windows x64 worktree, with `RUSTFLAGS=-D warnings`:

```powershell
$env:RUSTFLAGS = '-D warnings'
cargo build --release -j 2
cargo test --release -j 2
cargo check --release --all-targets -j 2
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\live-test.ps1 -Bin target\release -Out .live -LockWaitMinutes 60
```

Optional collector-only checks (no wiri process, input, or workload):
`powershell -NoProfile -File scripts\perf\self-test.ps1`.
Do not use this as a load/stress/soak test. It takes one idle window and ten spaced keys.

The one-line dot-source immediately before summary serialization runs
`scripts/live-phases/lane-e-perf.ps1` **after** the existing scenarios have quit their
instances. That phase creates two ordinary WinForms windows **before** starting one
fresh release `wiri.exe`. It copies the shipped default config, keeps the tray on and
animations/status bar/focus-follows-mouse off, and uses `-v --no-autostart`.
Only its child's `APPDATA`, `LOCALAPPDATA`, and `WIRI_CONFIG` are redirected into
`.live/perf`; it does not rewrite the shared config. Since A2 `f45c441`, the child also gets a
unique explicit `WIRI_PIPE` shared with the PID-validating observer (the old global pipe no
longer names the default server). An unconfigured observer fails closed. It refuses an
already-running wiri, checks every connected pipe's server PID before writing (including Quit), and
only terminates its own child on failed graceful shutdown. The parent harness holds
the machine-wide live mutex throughout. Its pre-existing scenarios still use their
original shared-desktop setup.

### Evidence and definitions

- `.live/summary.json`: flat numeric `p_*` measurements alongside unchanged existing gates.
- `.live/perf/measurement.json`: commit/version, SHA-256 binary identities, machine,
  configuration, window/thread ownership, raw key/action rows, and collection errors.
- `.live/perf/idle.json`: process CPU and per-thread context-switch/CPU deltas.
- `.live/perf/first-ipc.json`, `startup-state.json`, `local/wiri/wiri.log`:
  first reply, state, and isolated, unrotated-in-this-run debug trace.
- `.live/p-startup.png`, `p-after.png`: before/after perf scenario screenshots.
  Raw evidence is local/git-ignored; the baseline below is the durable digest.

Lane H fixes a collector bug: `Native.Request` now resolves `WIRI_PIPE` or the
same sanitized user/session pipe as the daemon, rather than the retired global
`wiri_control` pipe. Server-PID validation before every write remains unchanged.
The collector self-test covers discovery/overrides without starting wiri or
connecting to a pipe. Earlier Lane H collector timeouts are unavailable samples,
not a performance regression or a zero-cost result.

Resource endpoints are five seconds after startup observations and five seconds after
the perf actions, **in the same process**, not across the main harness's many restarts.
Private/working-set bytes, OS thread/handle counts and `GetGuiResources` GDI/USER
counts are one observation at each endpoint. They are not leak-test slopes.

The idle observer takes two `NtQuerySystemInformation(SystemProcessInformation)`
snapshots around a **single 30 s sleep**, with no harness input, IPC, subscriber,
screenshot, or repeated sampling in between. CPU is process kernel+user time;
`one_core_percent = CPU_ms / (elapsed_seconds * 10)` (not divided by logical CPUs).
Per-thread cumulative `ContextSwitches` is the locale-independent equivalent of
integrating `\\Thread(wiri*)\\Context Switches/sec`, filtered to the owned PID and
matched by TID **and creation time**. The native x64 header/offsets and thread PIDs are
validated. **The rate is an endpoint-based lower bound:** final counters of exited
threads, and threads both born and exited between endpoints, are not recoverable.
Endpoint churn counts cannot exclude such transient threads. Process creation identity
is checked too. Last-input ticks reveal input contamination, not background application
or scheduler activity. Context switches are **not** hardware wakeups or timer firings.

Cold measurements start at the OS process-creation FILETIME, mapped onto QPC.
Two observers independently await a complete successful `get_state` reply (including
connection and EOF) and observe both pre-existing sentinel windows reaching changed,
disjoint visible-frame rectangles at least `work_area_height - 64` tall, with at
least one intersecting its monitor's work area. Both must remain visible and
non-minimized. Existing desktop apps can separate the sentinel columns: wiri deliberately
**parks scrolled-out tiles below the monitors**, so they need not share a Y row. The
observed rectangles and on-screen count are retained. This proves the first
**observed sentinel tiling**, not that every third-party window has settled or that DWM
presented a frame. Observer startup delay is recorded;
these are upper bounds with clock-mapping, thread-scheduling and JIT noise. A freshly
created process is "cold" here; file-system/standby/Defender caches are **not flushed**.

Hotkey timing is from just before one `SendInput` batch for `Ctrl+Alt+Shift+F`
(`maximize-column`) to the first changed `GetWindowRect`, using QPC and 1 ms sleeps,
then at least 600 ms before another injection. Focus/key-down preflight prevents
knowingly sending to the wrong foreground app or releasing already-held modifiers.
Injection/focus failures and timeouts are separate; width expansion/contraction is
confirmed after settling, outside the timed region. A bad transition stops further
injections. The median/max use only confirmed samples, never a timeout sentinel.
The maximum observer gap is reported: Windows scheduling/sleep granularity can
significantly overestimate delivery time. This is neither key-to-photon nor an exact
hotkey-to-`SetWindowPos` timestamp; rect acknowledgement includes the target's UI thread.

For each representative action, the trace interval starts just before injection/IPC
and ends 600 ms after observation/reply. Its duration and raw counts are retained.
The `p_*_setwindowpos_lines_*` numbers count `SetWindowPos [sync] SUCCESS:` lines in
`BackendHandle::place_window_frame`; ordinary batched placement currently loops through
that path. `Positioning window` lines separately count engine placement intentions.
These are **successful logged placement calls, not an exhaustive native API trace**:
unlogged style/z-order calls, failed first attempts and fallback `MoveWindow` calls
are not equivalent; fallback-result lines are retained separately. Counts include
all managed windows and incidental work in the interval. Only confirmed transitions
enter aggregates. Width/fullscreen actions explicitly address the sentinel; fullscreen
also checks the state flag. Swaps first establish a non-boundary column and verify
changed column indices outside capture intervals (the viewport can follow a swap,
leaving a target rectangle unchanged). Placement-call counts **do not count layout
passes**: the applied-state cache can suppress calls even if an extra pass happened.

## Live baseline

### Identity, conditions and verification

Completed required live invocation on **2026-10-27**, local date verified with `Get-Date`
on TRIDARIS immediately before this record was written. (One request for this update named
2026-10-28; TRIDARIS reported `2026-10-27T18:10:29-07:00`, so that request was wrong and is
not carried into the record.) Binary/source commit
**`cc3920dffd295a4d494516fc97a8e241b3da7d62`** (`style(fmt)`: rustfmt drift in a layout
engine test assertion), one commit above **`0ebdebf`**.

**The event-driven perf pass (`0ebdebf`: bounded tokio runtime, event-driven main loop,
event-woken backend handler, event-armed reconcile, lazy indicator pump) is what this
baseline now measures** — not the `af3eb30` runtime the previous record measured. The
`af3eb30` numbers survive only as the delta column below; their raw per-thread and
per-action tables are replaced by these deltas and remain in git history. This document is
written after the live-tested commit, not evidence of a later runtime optimization.

- **Machine:** TRIDARIS, Intel Core i5-8350U @ 1.70 GHz, 4 cores / 8 logical processors,
  23.84 GiB visible memory; native x64 collector, rustc 1.95.0. One 1366×768 display,
  work area `(0,0)-(1366,728)` at 96 DPI.
- **OS as measured by CIM:** Windows 11 **Pro 10.0.22631** (build 22631) — the same reading
  as the `af3eb30` record. Earlier parity reports call this laptop Home/build 26200; that
  is **not** the version reported by this run either. Do not silently carry their OS
  description into this baseline.
- **Desktop and sampling:** one valid fresh process start, one 30 s idle interval, one
  resource observation per endpoint, ten confirmed alternating maximize keys,
  `p_collector_errors = 0`. No load, stress or soak run and no cache flushing. The run
  record handed to this documentation lane does **not** state the managed-window count or
  which other tenants were running (the `af3eb30` record had 49 managed windows). The
  resource endpoints below are therefore a same-machine comparison, **not a
  population-matched one**.
- **Release validation:** `cargo build --release -j 2` clean with 0 warnings,
  `cargo test --release -j 2` = **1792 passed / 0 failed / 1 ignored doctest**, and
  `cargo check --release --all-targets` clean. The test population has grown well past the
  `af3eb30` record's 872 library + 4 CLI tests (later lanes added suites), so test counts
  are not like-for-like either.
- **Live-report verdicts:** **761 of 761 enforced gates passed, 0 failed**, 88 further
  information entries, 849 report cells, using the repository's unchanged
  `scripts/live-report.py` rules. `hidden_json_present_after_restart = False` is
  **expected-pass** semantics (the crash record is deleted once the windows are back), not
  a failure. IPC timeouts, observation timeouts and placement failures were zero; collector
  errors zero. The `af3eb30` record had 277 enforced gates and 31 information entries, so
  **compare the pass/fail ratio, not the totals**: a large number of gates has been added
  since.
- **Previously failing gates now pass:** all six `h78_*` cursor-hiding gates
  (`hide_when_typing_dry_run`, `hide_when_typing_real_value_unchanged`,
  `cursor_hides_after_inactive_ms`, `cursor_shows_on_pointer_movement`,
  `cursor_restored_on_quit`, `tablet_stylus_bind_decode_and_registration`), all five `h31`
  focus-history gates including `h31_history_crosses_workspaces`, and all Lane V tab gates
  (`h65`). These are the evidence the parity rows cite; the Windows approximations those
  rows record are unchanged.
- **Not recorded by this run:** binary SHA-256 identities, the managed-window population,
  the per-thread idle attribution table, the largest key-observation gap, the
  window-owner thread table and the `h9_overlapping_terminal_pairs` numeric entry. The
  `af3eb30` record carried all of them. Their absence is a gap in **this** record, not
  evidence that those measurements improved.

### Deltas against the `af3eb30` baseline

| Measurement | `af3eb30` | `cc3920d` | Change |
| --- | ---: | ---: | ---: |
| OS process creation → complete first IPC reply | 182.8974 ms | **64.11 ms** | −118.7874 ms (−64.95%) |
| OS process creation → first observed sentinel tiling | 147.4223 ms | **64.26 ms** | −83.1623 ms (−56.41%) |
| `Process.Start` call duration | 9.1495 ms | **4.95 ms** | −4.1995 ms (−45.90%) |
| Idle process CPU, kernel + user | 437.500 ms | **0 ms** | −437.5 ms (below one 15.625 ms counter increment) |
| Idle CPU as percent of one core | 1.457588% | **0%** | −1.457588 points |
| Idle thread context switches | 321.935/s (endpoint lower bound) | **9.16/s** | −312.775/s (−97.15%) |
| Idle endpoint new / exited threads | 2 / 3 | **2 / 3** | unchanged |
| Idle engine `Positioning window` lines | 0 | **0** | unchanged |
| SendInput → first changed target rectangle, median | 23.68405 ms | **20.17 ms** | −3.51405 ms (−14.84%) |
| SendInput → first changed target rectangle, max | 24.9869 ms | **24.10 ms** | −0.8869 ms (−3.55%) |
| Confirmed hotkey samples / timeouts | 10 / 0 | **10 / 0** | unchanged |
| Threads, startup endpoint | 24 | **14** | −10 (−41.67%) |
| Threads, after actions | 26 | **15** | −11 (−42.31%) |
| Handles, startup endpoint | 235 | **220** | −15 (−6.38%) |
| Handles, after actions | 278 | **239** | −39 (−14.03%) |
| Private bytes, startup endpoint | 3,862,528 (3.684 MiB) | **8,511,488 (8.117 MiB)** | +4,648,960 (+120.36%) |
| Private bytes, after actions | 4,521,984 (4.312 MiB) | **8,523,776 (8.129 MiB)** | +4,001,792 (+88.50%) |
| Working set, startup endpoint | 14,716,928 (14.035 MiB) | **25,145,344 (23.980 MiB)** | +10,428,416 (+70.86%) |
| Working set, after actions | 18,432,000 (17.578 MiB) | **27,037,696 (25.785 MiB)** | +8,605,696 (+46.69%) |
| `wiri.exe` | 6,698,496 (6.388 MiB) | **8,965,120 (8.550 MiB)** | +2,266,624 (+33.84%) |
| `wiri-ctl.exe` | 3,220,480 (3.071 MiB) | **4,022,784 (3.836 MiB)** | +802,304 (+24.91%) |
| `maximize-column` successful `SetWindowPos [sync]` lines, median | 3 | **3** | unchanged |
| Enforced gates passed / failed | 277 / 0 | **761 / 0** | gate population grew |
| Information entries beside the enforced gates | 31 | **88** | — |

Three of those deltas need plain words rather than a headline:

1. **Idle is the measured win, and it is large.** In the same 30 s window the process used
   less CPU than the collector's 15.625 ms counter increment can resolve, and the
   endpoint context-switch lower bound fell from 321.935/s to 9.16/s. `0 ms` is a
   **quantization floor, not proof of zero work**; the definitions above still apply — one
   uncontrolled interval, an endpoint lower bound that cannot see threads born and exited
   between snapshots, and context switches that are not timer wakeups.
2. **Resource breadth fell too.** Threads dropped from 24/26 to 14/15 and handles from
   235/278 to 220/239 at the same two endpoints, which is what a bounded runtime replacing
   the default worker set looks like from outside.
3. **Memory endpoints are higher and this record does not explain why.** Private bytes rose
   120.36% / 88.50% and working set 70.86% / 46.69%, and both release binaries also grew
   (+33.84% `wiri.exe`, +24.91% `wiri-ctl.exe`). Without the managed-window count and the
   per-thread attribution the `af3eb30` record carried, this cannot be attributed to the
   runtime change, to a different window population, or to ordinary code growth. It is
   recorded as an unmeasured difference and a follow-up — **not a leak claim in either
   direction**, and not a claim that the memory work of Phase 4 is done.

### Resource endpoints

| Resource | Startup, after 5 s settle | After actions + 5 s settle |
| --- | ---: | ---: |
| Private bytes | 8,511,488 (8.117 MiB) | 8,523,776 (8.129 MiB) |
| Working set bytes | 25,145,344 (23.980 MiB) | 27,037,696 (25.785 MiB) |
| Threads | 14 | 15 |
| Handles | 220 | 239 |
| GDI objects | not recorded this run | not recorded this run |
| USER objects | not recorded this run | not recorded this run |

These endpoints include startup probes and normal IPC. The after-minus-startup increases are
**not evidence of a leak**: this single short sequence exercises lazy resources, without
proving they return to a long-term plateau.

### Idle (no harness input or IPC)

| Measurement | Result |
| --- | ---: |
| Elapsed (`p_idle_seconds`) | 30.0 s |
| Process CPU, kernel + user (`p_idle_cpu_ms`) | 0 ms — below the 15.625 ms counter increment |
| CPU as percent of one core (`p_idle_one_core_percent`) | 0% |
| Thread context switches (`p_idle_context_switches_per_s`) | 9.16/s, endpoint lower bound |
| Endpoint-visible new / exited threads | 2 / 3 |
| Engine `Positioning window` lines (`p_idle_positioning_lines`) | **0** |
| Per-thread attribution | not recorded this run |

The absent per-thread table is the important gap: the `af3eb30` record could attribute
84.424/s of its 321.935/s to the hidden workspace indicator alone. **Do not transfer that
attribution to this run**, and do not read the 9.16/s as "the indicator is event-driven
now" — no per-thread counter was collected here. External window events, other tenants'
builds, scheduling, power policy, thermal state, timer coalescing and debug logging remain
uncontrolled; one interval cannot establish a distribution.

### Startup and key-to-rectangle observation

| Measurement | Samples | Result |
| --- | ---: | ---: |
| OS process creation → complete first IPC reply (`p_cold_first_ipc_ms`) | 1 | **64.11 ms** |
| OS process creation → first observed sentinel tiling (`p_cold_first_layout_ms`) | 1 | **64.26 ms** |
| `Process.Start` call duration (`p_cold_start_call_ms`) | 1 | 4.95 ms |
| SendInput → first changed target rectangle | **10 confirmed** (`p_hotkey_samples`) | **median 20.17 ms; max 24.10 ms** |
| Invalid injections / observation timeouts (`p_hotkey_timeouts`) | 10 attempted | 0 / 0 |
| Largest gap between rectangle observations | 10 keys | not recorded this run |

The 20.17 ms median is inside the previous record's 23.68405 ms but still above the
**unenforced** one-frame budget discussed under "Proposed Phase 4 budgets" below
(`p_hotkey_move_max_ms = 24.10` is above it as well). Nothing in this record changes that:
a WinEvent/ETW acknowledgement observer, still external to wiri, remains the prerequisite
before any sub-frame latency gate can be enforced.

### Logged positioning work per representative action

Only the maximize transition was recorded per action in this run
(`p_maximize_setwindowpos_lines_median = 3`): ten confirmed keys, **3 successful
`SetWindowPos [sync]` lines on every key**. Width/fullscreen/column-swap counts and the
engine `Positioning window` per-action lines were not recorded this run. The `af3eb30`
record kept them; treat the three-per-maximize figure as the only like-for-like placement
number here, and the definitions above as unchanged: these are successful logged placement
calls over all managed windows, not an exhaustive native API trace and not a layout-pass
count.

### Executables

| Release executable | Bytes | MiB | SHA-256 |
| --- | ---: | ---: | --- |
| `wiri.exe` | **8,965,120** | 8.550 | not recorded this run |
| `wiri-ctl.exe` | **4,022,784** | 3.836 | not recorded this run |

Both binaries are **larger** than the `af3eb30` record (+2,266,624 B and +802,304 B). This
baseline does not claim a binary-size win and does not explain the growth.

### Earlier attempts and history

Unchanged from the previous record: one harness invocation was killed by the command runtime
ceiling after queue wait + H14; a completed run of `83e4901` kept summary serialization
working but its perf phase stopped at an open-log sharing error and its same-row layout
predicate timed out on a legitimately parked sentinel. Those incomplete perf measurements
are **not pooled** with this baseline, and neither are they pooled with the `af3eb30` one.
`af3eb30` fixed the collector, not wiri's runtime. Independent repeated normal runs and an
ARM64 measurement remain necessary before claiming stable budgets, and the endpoints above
still lack the population control that would make the memory comparison conclusive.

## Lane K transient compositor checkpoint (2026-10-26)

This is a separate lane-local observation at `267a612`, **not a replacement for
the historical baseline or integrated sign-off**. The completed run is preserved
at `.scratch/lane-k-first-live`; raw values and binary SHA-256 are in
`lane-k/measurement.json`, with per-transition lines in `lane-k/wiri.stderr.log`.
The binary hash is `EEEF13856581E245474AADD26EE3DFD28DFBD74FF8681FCFD5F5E3173632AFE1`.
TRIDARIS x64, one 1366×768 display at 96 DPI, release build with warnings denied.
The saved report has **497 pass / 6 fail**, including all ten H54–H57 passes;
[the Lane K report](parity/lane-k-report.md) explains the H22 foreground abort.
No additional live run was made for this documentation update.

### Animation observations

| Transition | Renderer frame updates | Layer lifetime (ms) | Whole-process CPU delta (ms) | Live thumbnails |
| --- | ---: | ---: | ---: | ---: |
| Default open | 4 | 151.664 | 0 | 1 |
| Default resize | 13 | 432.923 | 15.625 | 1 |
| Default close (copied snapshot) | 5 | 162.423 | 0 | 0 |
| Default workspace return | 12 | 392.401 | 0 | 1 |
| Slowdown-10 workspace, two window sets | 49 | 1555.345 | 15.625 | 2 |

These are individual log samples, not averages, renderer-only CPU, GPU time,
key-to-photon latency or DWM presentation timestamps. `cpu_ms` is the process
kernel+user delta over the layer's lifetime, quantized here in 15.625 ms units;
**0 ms does not mean zero work**. Frame updates are not monitor presents and do
not establish 60 Hz or hardware-vsync pacing. The two slowed-workspace PNGs
show outgoing red and incoming blue content moving together (report H56).

### Idle and resources before/after the transition sequence

Both endpoints use the **same enabled process/configuration**, not different code
versions. Each idle sample is one 30 s sleep without harness input or IPC. Input
ticks were unchanged within each sample. The fixture population is not identical:
no Lane K fixture at the first endpoint, one retained fixture at the second.

| Measurement | Before | After |
| --- | ---: | ---: |
| Idle elapsed seconds | 30.0133894 | 30.0124899 |
| Process CPU ms | 93.75 | 62.5 |
| Percent of one core | 0.312361% | 0.208247% |
| Endpoint-counted context switches/s (lower bound) | 270.645874 | 271.387013 |
| Endpoint-visible new / exited threads during idle | 2 / 3 | 0 / 2 |
| Private bytes | 3,723,264 | 4,734,976 |
| Working-set bytes | 16,375,808 | 20,303,872 |
| Threads at resource endpoint | 21 | 22 |
| Handles | 234 | 277 |
| GDI / USER objects | 2 / 34 | 2 / 35 |

H57 observed no animation layer at rest and stable fixture geometry. Source
inspection finds the compositor reuses the existing input pump and active-only
frame driver, destroying its layer/thumbnails/snapshots at completion; no added
idle thread, timer or clock is present. This does **not** prove unchanged overall
resource usage, zero idle wakeups, a leak plateau or a causal CPU improvement.
Existing desktop events, pollers, lazy runtime workers and scheduling remain
uncontrolled. Both CPU samples exceed the proposed 0.10%-of-one-core budget;
context switches are not timer wakeups. No load/stress/soak test was run. ARM64,
multiple displays, non-96-DPI and isolated GPU/frame-pacing costs remain unmeasured.

## Polling/timer inventory at source baseline `d84f04b`

The Rust tree is unchanged from `d84f04b4b56107f86c4af6eb1016288b583016f5`.
Locations below refer to that tree, not other lanes' in-flight changes. The starting
list was [Phase 2A's report](parity/phase2a-report.md#timers-and-threads-phase-4-should-remove-noticed-none-added-in-phase-2a).
Two corrections: the main loop also arms a 1 ms Ctrl-C timeout **each iteration**, and
the workspace indicator's 16 ms timer continues **while hidden**, not just during fades.

"Nominal/s" below is a source setting, **not measured wakeups**. Tokio/Windows timer
resolution, work, the extra main-loop timeout, and coalescing change actual cadence.
CPU costs cannot be attributed to each async poller from a process total: Tokio moves
them between workers. The table states the actual idle work and its frequency, not
invented per-poller percentages. Dedicated window-owner thread measurements, when
available below, are stronger evidence for the indicator alone.

| Poller: file and function | Idle cost / when active | Concrete Phase 4 change |
| --- | --- | --- |
| `src/main.rs::main`, main pump (~740–958); `hooks/mod.rs::poll_tray_action`, `hooks/tray.rs::try_recv_action` | Always. `sleep(8 ms)` plus `timeout(1 ms, signal::ctrl_c())` per iteration; polls reload/shutdown/hotkey/tray channels. The often-quoted 125/s is only `1 / 8 ms`, not actual iterations or total deadlines. | One persistent Ctrl-C future and `select!` over async receivers; bridge synchronous producers; disable disconnected branches. No idle deadline. |
| `main.rs::main`, `LAST_WS` (~773–829) | Same loop, **not another timer**: engine read plus mutex/workspace comparison each iteration; overlay/legacy IPC only on change. | Push `(output, workspace)` transitions at actual workspace/output-focus mutations. |
| `main.rs::main`, animation probe (~723–770) | Same loop, elapsed-time probe at >=33 ms; engine read even with animations **off**. Tick/write/apply only while active. | Notify on animation start; frame deadlines only while active, disarm on completion. Preserve final dirty geometry delivery. |
| `main.rs::main`, backend-event task (~554–560) | Always: empty `try_recv` drain plus 50 ms sleep (nominal 20/s), even after disconnection. Adds up to one period to backend-event handling. | Await `event_rx.recv()`, drain bounded bursts, exit on closure; notify IPC once per burst. |
| `main.rs::main`, reconciler (~569–574) → `reconcile_visible_windows` (~1496–1522) → `backend/mod.rs::BackendHandle::refresh_visible_windows` | Always: 750 ms interval (~1.33/s, immediate first tick). `EnumWindows`, visible-window info/cache updates and validity/admission checks, then write-lock attempt and `apply_all`. Cost grows with desktop windows even when placement cache emits no calls. | One startup enumeration, complete CREATE/SHOW/NAME/STATE admission, bounded per-HWND readiness retries and unexpected-SHOW visibility repair. Explicit dirty/resync events, not a permanent rescan. |
| `main.rs::main`, mouse task (~588–636); `input/mouse.rs::MouseTracker::update_cursor_pos` | Always: tracker write lock and `GetCursorPos` every 500 ms (nominal 2/s), **even when focus-follows-mouse is off**. Enabled path also makes a monitor map/scans tiles. | Coalesce existing LL mouse motion onto a channel only when configured; cancellable one-shot focus-delay deadline. No engine work in the LL callback. |
| `overlay/workspace_indicator.rs::run_window_loop` (~203–285), `wnd_proc` (~386–447) | Default, eagerly created: `SetTimer(16)` (~62.5/s requested). Hidden handler still locks two timestamps and calls `ShowWindow(SW_HIDE)` repeatedly. Visible handler updates alpha/invalidates. `KillTimer` is only on destroy. | Lazy creation, posted show/update, frame clock only during fades, one hold deadline; kill timer immediately on hide/completion. **Zero hidden callbacks.** |
| `main.rs::main`, status updater (~665–717) | Only startup `status-bar true` (default **off**): 1 s sleep; engine read, copies label/title, `GetLocalTime`, unconditional bar update. | Event-driven label/title/focus updates; one owner produces the clock. |
| `overlay/status_bar.rs::run_window_loop` (~155–225), `wnd_proc` (~253–382) | Same opt-in bar: another persistent 1 s `SetTimer`, invalidates/paints **HH:MM** every second with text/GDI allocations. Zero default baseline work because not created. | One aligned minute deadline plus time/timezone-change messages; invalidate changed content only; no hidden clock. |
| `backend/message_loop.rs::MessageLoop::run` (~830–837), wrapper `backend/mod.rs::Backend::run` (~500–506) | **Dormant in daemon startup**: exposed library API polls mutex `running` with 100 ms sleeps (10/s if used). Zero measured default cost. | Completion `oneshot`/`Notify`, signalled by pump stop/exit. |

### Recovery deadlines / bounded waits (not healthy-idle pollers)

These exist today too; deleting all occurrences of `sleep`/`timeout` would be wrong.
Their healthy steady-idle contribution is zero, except the main pump's separately
listed recurring Ctrl-C deadline.

| File:function | Trigger / bound | Proposed treatment |
| --- | --- | --- |
| `ipc/server.rs::IpcServer::run` (~224–298) | Pipe-creation failures retry after 1 s, stop after five failure rounds; connect failures retry after 100 ms without that cap. Healthy pipe connect is awaited. | Keep cancellation and bounded/escalating error backoff; no healthy-idle timer. |
| `main.rs::main`, pending config-error balloon (~839–847) | Up to 500 requeues at main-loop cadence only while startup balloon awaits tray readiness. | Tray-ready event and bounded delivery/reporting. |
| `backend/hidden.rs::restore_from_file` (~447–473) | Startup crash/uncloak recovery checks every 10 ms, at most 600 ms. | Observe targeted restore/SHOW completion with a deadline; keep safety fallback. |
| `backend/mod.rs::BackendHandle::activate_window` (~846–906) | One **blocking 40 ms** sleep on final activation escalation. | Foreground acknowledgement/deadline outside engine locks; important latency tail, not idle CPU. |
| `backend/mod.rs::query_min_track_width`; `layout/engine.rs::min_width_query_wanted` | Event-triggered `SendMessageTimeoutW` up to 120 ms, at most three recorded unanswered queries/window. | Preserve bound/cap; perform blocking work outside engine locks on bounded blocking executor. Avoid serial startup critical path. |
| `ipc/server.rs::IpcServer::respond` (~431–453) | One 1,500 ms request deadline; timeout does not cancel the blocking action. | Keep reply bound; explicit cancellation/queued-action semantics if redesigned. |
| `backend/hooks.rs::WinEventHook::new`, `input/low_level_hook.rs::start_mouse_hook` / `start_keyboard_hook`, `overlay/thumbnail_overview.rs::spawn_host_window` | One readiness receive, at most 2 s per thread creation. | Keep handshakes, async oneshot where appropriate. |
| `main.rs::main` | One 1 ms singleton `WaitNamedPipeW` startup probe; one blocking 100 ms shutdown restore grace. | Keep probe; await targeted restore completion with bounded shutdown deadline. |
| `config/loader.rs::tests::wait_for`, watcher test; `layout/engine.rs` tests | Test-only 20 ms polling (<=5 s), one 15 ms sleep, five individual 25 ms throttle sleeps. | No daemon cost. Fake clock/channel waits for test determinism, not an idle optimization. |

### Event-driven or passive timing: keep it event-driven

- `config/loader.rs::spawn_reload_handler` blocks in `recv`; `settle` uses a 200 ms
  trailing-edge debounce and an additional 400 ms error reread **after file events**.
  The native `notify` watcher is not an mtime polling loop. No recurring idle deadline.
- `ipc/server.rs::run_state_task` awaits `Notify`; snapshots/diffs only while subscribed
  after actual changes. The layout-request drainer in `main` awaits `recv`; IPC listener,
  client readers, subscribers and the keyboard-action receiver await I/O/channels.
  `GetMessageW` pumps block. Do not replace them with periodic sampling.
- `backend/hooks.rs::winevent_proc` / `detect_size_class_change`: every external,
  non-self-applied LOCATIONCHANGE does `IsZoomed`, DWM bounds and monitor lookup **before**
  the 200 ms throttle. Zero without events, but costly during application movement.
  Coalesce latest per-HWND geometry off the hook, preserving immediate state transitions
  and final delivery; this is event cost, not an additional timer.
- `layout/mod.rs::ConfigureThrottle::intent` / `TilingEngine::apply_layout_for_monitor`
  have a passive 16 ms configure gate. Removing periodic layout passes must not lose a
  throttled **final** rectangle: keep dirty targets and one next-eligible deadline.
- Self-applied suppression (250 ms), stale-entry pruning (1 s / hook location-map
  60 s), action debounce (125 ms), double-Alt (250 ms), bind cooldown, mouse focus delay
  (main currently hardcodes 200 ms), touch release timeout (500 ms), hidden-foreground
  grace (700 ms), open-unfocused grace (2,500 ms), and fullscreen settling (1,500 ms)
  compare timestamps **when events arrive**, not independently ticking timers.
  Mouse delay is currently serviced by the separately listed 500 ms poller.
- `backend/accent.rs::AccentCache::get` has a **30 s on-demand TTL**, not a 30 s wakeup.
  Prefer color/settings notifications for invalidation, but do not claim timer savings.
- Animation `tick_animations` / `tick_animations_with_backend` are caller-driven; the
  backend-aware variant has no daemon call site. Touch action draining follows received
  Win32 messages, not a periodic tick. Wheel handling can spawn short-lived action
  threads on input; none is a healthy-idle timer.

### Threads and memory implications

`#[tokio::main]` selects the multithread runtime; no worker count is set by wiri.
It normally follows logical CPUs (`TOKIO_WORKER_THREADS` can override it).
`tokio = { features = ["full"] }` does **not** imply one thread per async task.
There are six startup async tasks (layout drain, listener, IPC-state, backend drain,
reconciler, mouse) and an optional seventh for status; requests/subscribers add work.
`spawn_blocking` request/state work uses Tokio's dynamic blocking pool, not a dedicated
permanent thread for each call. Threads may remain after an IPC burst.

Normal successful startup explicitly spawns **10 long-lived std threads**, excluding
main, Tokio and dependency-owned threads: config reload, WinEvent, LL keyboard, LL
mouse, hotkey pump, keyboard-action receiver, tray, workspace indicator, overview
banner, cheatsheet. The last three overlays are eagerly created; banner/cheatsheet
have **no** timers while hidden, unlike the indicator. Optional status adds one;
snap guide and thumbnail-overview host are lazy additional threads. Native notify
also owns dependency worker(s); COM/Windows can add others. Actual totals must be read
from the process, not inferred by counting `spawn` calls. Many blocked stacks mainly
reserve virtual space; do not equate reservation to private committed bytes.

## Proposed Phase 4 budgets (not enforced here)

Apply first on this x64 laptop with the default config, two ordinary sentinel windows
and a quiet, responsive existing desktop; repeat on the owner's ARM64 laptop before
claiming portability. Keep debug-log mode comparable initially, then separately report
normal INFO mode. No load generation. Repeat a few independent cold starts and idle
windows on separate normal live runs to estimate noise; ten keys support median/max,
not a credible p99 claim.

| Budget | Proposed acceptance | Changes that could meet it |
| --- | --- | --- |
| Idle CPU | <=0.10% of **one core** over 30 s (<=30 ms CPU); aim indistinguishable from zero | Remove all always-on polls and hidden indicator callbacks; no per-second default UI refresh. Report contaminated/external-event samples separately. |
| Idle wakeups | **Zero periodic application timer wakeups** when settled/hidden; <=10 thread context switches/s on a genuinely quiet desktop as a diagnostic, not a wakeup equivalence | Await channels/Win32 messages; demand-driven frame/focus clocks; no reconciler except explicit resync. Attribute real external events before blaming the scheduler. |
| Threads | <=8 settled default; <=10 after ordinary IPC, with no unbounded retained blocking pool | Current-thread or tightly bounded runtime, one overlay/tray/hotkey UI owner where safe, lazy overlays, shared watcher delivery. Preserve independent LL hook responsiveness. Feature trimming alone does not reduce worker count. |
| Memory | <=8 MiB private, <=24 MiB working set at rest (baseline already below both); no upward endpoint trend claimed from one run | Preserve the current small footprint as features grow: lazy overlays, shared rendering resources/owner, bounded workers/snapshots. Measure committed bytes, not stack reservation; profile before promising allocator savings. |
| Cold start | First IPC answer <=500 ms, first observed sentinel layout <=250 ms on responsive desktop | Start IPC early, batch initial admission/layout, avoid serial slow-window metadata/minimum queries on the critical path; retain bounded background readiness retries. Hung/unresponsive windows need separately stated bounds. |
| Hotkey movement | Median <=8 ms; maximum <=16 ms for ten confirmed keys, collector uncertainty explicitly shown | Keep direct hotkey dispatch, shorten engine lock scopes, await backend events instead of 50 ms drain, coalesce one layout per mutation/burst. Use a finer event-driven observer before enforcing sub-frame thresholds if sleep gaps dominate. |
| Placement work | Zero placement lines in settled idle; at most one successful placement per distinct dirty target/action, no redundant pass | Persist/deduplicate dirty geometry and final throttled intent; no timer-driven `apply_all`; add exhaustive native instrumentation before calling the proxy a complete API count. |
| Binary (secondary) | wiri <=5 MiB, ctl <=2 MiB, with responsiveness unchanged | Audit actually used Tokio/windows features, release LTO/codegen/size tradeoffs and CLI/shared dependency surface. Linker elimination means feature removal alone may save nothing; measure each build. |

No optimization in this document has been implemented by Lane E. Remaining work is
removing the enumerated pollers without losing admission/crash-recovery/final-layout
correctness, validating budgets with less noisy repeated normal runs, a finer external
latency observer, checking normal non-verbose mode, and measuring ARM64. Separately,
investigate the H9 terminal visible-frame overlap without weakening its recorded result.
No stress/soak testing is proposed.

## Thread and memory budget work, 2026-10-31 (Lane PERF)

This section is the source-backed inventory the "Proposed Phase 4 budgets" table asked for, the
changes that shrink it, and the before/after measurement of exactly those changes. Every thread
below is named with `SetThreadDescription` when it starts (Rust's `std::thread::Builder::name`
and Tokio's `thread_name` both do that), so a collector can attribute each one by description
instead of by guesswork. Only the `notify` crate's own watcher thread is out of our naming reach.

### Inventory: every thread wiri creates

| Wall-clock point | Description | Owner (source) | Count | Lifetime |
| --- | --- | --- | ---: | --- |
| process start | `wiri.exe` (the unnamed main thread) | `main.rs::main` | 1 | process |
| process start | `wiri-rt` | Tokio workers, `runtime.rs::build` (`WORKER_THREADS = 2`) | 2 | process |
| on demand | `wiri-rt` | Tokio blocking pool, `MAX_BLOCKING_THREADS = 4`; created per `spawn_blocking` task, exits after `BLOCKING_KEEP_ALIVE` idle | 0..4 | transient |
| process start | `wiri-ui` | `backend/message_loop.rs::run_message_loop`: hotkey window (`WiriHotkeyWnd`), `WH_KEYBOARD_LL`, the compositor and decoration windows and the tray icon pump here | 1 | process |
| process start | `wiri-winevent-hook` | `backend/hooks.rs::WinEventHook` — `SetWinEventHook(WINEVENT_OUTOFCONTEXT)` needs its own pumping thread | 1 | process |
| process start | `wiri-mouse-hook` | `input/low_level_hook.rs::start_mouse_hook` — `WH_MOUSE_LL` plus the one-shot cursor-hide timer | 1 | process |
| process start | `wiri-config-reload` | `config/loader.rs::spawn_reload_handler` | 1 | process |
| process start | *(unnamed, third party)* | the `notify` crate's `ReadDirectoryChangesW` watcher thread | 1 | process |
| process start (default config) | — | nothing else: the four overlays below are all lazy | 0 | — |
| first announcement | `wiri-a11y` | `a11y.rs::Provider::start` — the UIA provider window; created by the first `announce` | 0..1 | process |
| first `overview-toggle` | `wiri-overview-banner` | `overlay/overview_banner.rs::ensure_thread` (was eager at startup) | 0..1 | process |
| first workspace change | `wiri-ws-indicator` | `overlay/workspace_indicator.rs::ensure_thread` | 0..1 | process |
| `show-key-bindings` / `hotkey-overlay` | `wiri-hotkeys` | `overlay/bindings_cheatsheet.rs` | 0..1 | sheet |
| `status-bar true` | `wiri-status-bar` | `overlay/status_bar.rs::spawn` | 0..1 | process |
| first snap-guide show | `wiri-snap-guide` | `input/snap_guide.rs::spawn` | 0..1 | process |
| first `overview-select`/thumbnail use | `wiri-overview-host` | `overlay/thumbnail_overview.rs` | 0..1 | overview |
| first window close fade | `wiri-recent-preview`, `wiri-recent-debounce` | `overlay/recent_windows*.rs` | 0..2 | fade |
| first screenshot | `wiri-screenshot-ui` | `overlay/screenshot_ui.rs` | 0..1 | capture |
| `quit skip-confirmation=false` | `wiri-quit-confirmation` | `input/system_actions.rs` | 0..1 | dialog |
| a power action | `wiri-power-action` | `input/system_actions.rs` | 0..1 | request |
| any animation burst | *(the Tokio blocking threads above)* | `animation/clock.rs` occupies one with `DCompositionWaitForCompositorClock`; it is released when the last timeline ends | 0..1 | transient |

The hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`, the WinEvent hooks) are on threads that pump
messages, as Win32 requires, and none of them is shared with a thread that can block on a
modal operation: the tray context menu is the only modal call in the process and it now runs on
`wiri-ui`, so an open menu delays hotkey delivery, exactly as it would in any other Win32
application that shows a menu on its own UI thread.

### What changed

1. **One session UI thread.** `wiri-ui` now hosts the tray icon as well as the hotkey window,
   the keyboard hook and the overlay windows. `hooks/tray.rs::create_icon` is called on that
   pump (posted to it with `WM_TRAY_HOST`, answered through a `SyncSender`, 5 s bound) instead
   of starting a second Win32 message loop; when the pump is not running yet the old dedicated
   `wiri-tray` thread is used, so the icon can never be lost. This is the thread the task called
   "one UI thread for overlays/tray/hotkeys". One thread fewer at rest.
2. **Lazy overview banner.** The overview banner used to create its window and pump thread at
   startup although only `overview-toggle` ever shows it. `OverviewBanner::ensure_thread` starts
   it on the first `show`/`hide` (posting one `WM_BANNER_UPDATE` at creation so a `show` that
   raced the thread is not lost). No timer, no poll. One thread fewer at rest.
3. **Shorter blocking-pool keep-alive.** `runtime::BLOCKING_KEEP_ALIVE` fell from 2 s to 250 ms,
   so an IPC burst (one blocking task per request) no longer leaves pool threads behind for
   seconds after the last reply. A request that finds no idle thread starts one, which costs a
   fraction of a millisecond.
4. **Every remaining thread is named**, including the previously unnamed hotkey pump and the
   status bar / snap guide / overview banner pumps, so `p_rest_threads_by_*` can attribute them.

Not changed, deliberately: the WinEvent hook, the low-level mouse hook and the hotkey pump stay
on three separate threads. Merging any two of them would put a blocking native call (a hook
callback that can wait on a hung window, or a modal tray menu) in front of the input path that
the "Hotkey movement" budget measures; the budget itself says "preserve independent LL hook
responsiveness".

