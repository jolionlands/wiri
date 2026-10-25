# Phase 4: measured baseline, not performance gates

Lane E is **measurement only**: no `src/`, dependency, runtime, or release-profile changes.
All `p_*` properties in `summary.json` are numeric **information**, never pass/fail.
`-1` means unavailable, not zero cost or a passing sample. Existing live gates are untouched.
The proposed budgets below are future acceptance criteria, not enforced by this collector.

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
`.live/perf`; it does not rewrite the shared config. It refuses an already-running
wiri, checks every connected pipe's server PID before writing (including Quit), and
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

Completed required live invocation on **2026-10-25**, desktop steps
10:38:56–10:51:41 PDT. Measured child PID **10696**, process creation
**2026-10-25T17:50:48.4393252Z**, binary/source commit
**`af3eb304b002d9f6608c969a341ce77d84db0881`** (`wiri-ctl 0.1.0 (commit af3eb30)`).
Rust source is unchanged from `d84f04b`; intervening commits only add/fix observers.
This document is written after the live-tested collector commit, not evidence of a
subsequent runtime optimization.

- **Machine:** TRIDARIS, Intel Core i5-8350U @ 1.70 GHz, 4 cores / 8 logical processors;
  native x64 collector, rustc 1.95. One 1366×768 display, work area `(0,0)-(1366,728)`.
- **OS as measured by CIM:** Windows 11 **Pro 10.0.22631** (build 22631).
  Earlier parity reports call this laptop Home/build 26200; that is **not the version
  reported by this run**. Do not silently carry their OS description into this baseline.
- **Desktop:** 49 managed windows including the two sentinels; other tenants' terminals,
  browser and tools were left running, not created as a benchmark workload or stopped.
  One sentinel was parked at visible-frame `(4,836)-(435,1540)`; the other tiled at
  `(923,12)-(1354,716)`. No expectation that both fit the current strip viewport.
- **Sampling:** one valid fresh process start, one 30 s idle interval, one resource
  observation per endpoint, ten confirmed alternating maximize keys, two actions each
  for width/fullscreen/column swap. No load, stress or soak run; no cache flushing.
- **Release validation:** build, test and all-target check passed with `RUSTFLAGS=-D warnings`
  and `-j 2`; **872 library + 4 CLI tests passed**, one ignored documentation example.
  Collector self-check passed, including reading a log while its writer remains open.
- **Existing live-report verdicts:** **277 pass, 0 fail, 31 information entries**, using
  the repository's unchanged `scripts/live-report.py` rules. No step had a nonzero
  numeric return code. The expected-False crash-record check was False; IPC timeouts,
  settled positioning and ordinary-form overlap were zero. All **46 `p_*` fields are
  numeric and classify as information**; collector errors = 0.
- **Do not hide the terminal anomaly:** `h9_overlapping_terminal_pairs = 1`, an
  informational numeric entry in the existing report (not an enforced pass/fail rule).
  Its saved rectangles show an **11 px visible-frame overlap** between `(-10,12)-(456,716)`
  and `(445,12)-(911,716)`, both 466 px wide. The preceding completed run also recorded
  one pair. H9 occurs **before** Lane E starts; neither the gate nor runtime was changed.
  This is follow-up product/harness work, not an all-clear claim about terminal geometry.

### Resource endpoints

| Resource | Startup, after 5 s settle | After actions + 5 s settle |
| --- | ---: | ---: |
| Private bytes | 3,862,528 (3.684 MiB) | 4,521,984 (4.313 MiB) |
| Working set bytes | 14,716,928 (14.035 MiB) | 18,432,000 (17.578 MiB) |
| Threads | 24 | 26 |
| Handles | 235 | 278 |
| GDI objects | 3 | 3 |
| USER objects | 38 | 42 |

These endpoints include startup probes and normal IPC; the idle window starts after
startup IPC and a screenshot. The after-minus-startup increases are **not evidence
of a leak**: this single short sequence exercises lazy resources and Tokio blocking
workers, without proving they return to a long-term plateau.

### Idle (no harness input or IPC)

| Measurement | Result |
| --- | ---: |
| Elapsed | 30.015335 s |
| Process CPU, kernel + user | 437.500 ms |
| CPU as percent of one core | **1.457588%** |
| Thread context switches | 9,663 counted; **321.935/s lower bound** |
| Endpoint-visible new / exited threads | 2 / 3 |
| Last-input tick before / after | 70,923,579 / 70,923,579 (unchanged) |
| Engine `Positioning window` lines | **0** |

The unchanged input tick makes this an input-free observation, **not an isolated
machine**. External window events, other tenants' builds, scheduling, power policy,
thermal state, timer coalescing and debug logging are uncontrolled. One interval
cannot establish a distribution or causally attribute the process CPU to individual
pollers. CPU counters here are visibly quantized in 15.625 ms increments.

Window-owner mapping gives one useful, limited attribution:

| Dedicated owner thread | CPU in 30.015335 s | Context switches |
| --- | ---: | ---: |
| Hidden workspace indicator, TID 30472 | **46.875 ms** (0.156170% of one core) | **2,534** (84.424/s) |
| Hidden overview banner, TID 18344 | 0 ms | 0 |
| Hidden cheatsheet, TID 3288 | 0 ms | 0 |
| Tray owner, TID 36256 | 0 ms | 0 |
| Hotkey owner, TID 23632 | 0 ms | 0 |

The indicator thread's total includes any IME/system message activity; this is not
proof that every switch was `WM_TIMER`. It alone accounts for ~10.7% of the process
CPU observed here. Do not divide the remaining CPU between the reconciler/main pump/
mouse/animation tasks by their nominal frequencies; there is no such instrumentation.

### Startup and key-to-rectangle observation

| Measurement | Samples | Result |
| --- | ---: | ---: |
| OS process creation → complete first IPC reply | 1 | **182.8974 ms** |
| OS process creation → first observed sentinel tiling | 1 | **147.4223 ms** |
| `Process.Start` call duration | 1 | 9.1495 ms |
| IPC / layout observer began after process creation | 1 each | 5.6561 / 10.1010 ms |
| SendInput → first changed target rectangle | **10 confirmed** | **median 23.68405 ms; max 24.9869 ms** |
| Invalid injections / observation timeouts | 10 attempted | 0 / 0 |
| Largest gap between rectangle observations | 10 keys | **17.2182 ms** |

Raw key latencies (ms, in order):
`24.9869, 17.3199, 23.4133, 23.9276, 24.5764, 24.0607, 21.7286, 23.4405, 23.9343, 23.4350`.
Every key accepted all eight input events and kept the target foreground. Width
alternated between native-frame 445 px and 1,356 px, with >=600 ms settling per key.
The wide observer gaps mean these **upper bounds cannot decide whether the native
placement happened within 16 ms**. No timer-resolution change was made. Use a
WinEvent/ETW acknowledgement observer, still external to wiri, before enforcing a
sub-frame latency gate; neither DWM presentation nor exact call-entry timing is known.

### Logged positioning work per representative action

| Action | Confirmed samples | Successful `SetWindowPos [sync]` lines, per action | Median | Engine placement-intention lines |
| --- | ---: | --- | ---: | --- |
| `maximize-column` (real chord, alternating) | 10 | 3 on every key (max 3) | **3** | 3 each |
| `set-column-width 600`, then `400` (IPC, explicit target) | 2 | 3, 3 | **3** | 3, 3 |
| `fullscreen-window` on/off (IPC, explicit target) | 2 | 4, 3 | **3.5** | 3, 3 |
| `move-column-left`, then right (IPC, from a non-boundary column) | 2 | 2, 2 | **2** | 2, 2 |

Capture intervals were 611.97–649.39 ms, including the 600 ms post-action settle.
All recorded fallback/failure-line deltas were zero. The counts concern **all 49
managed windows**, not just the sentinel; moving neighbour tiles can be necessary.
They do not demonstrate redundant layout passes or three calls on the same target.
The isolated final debug log was 51,527 bytes, so no log rotation occurred.

### Executables

| Release executable | Bytes | MiB | SHA-256 |
| --- | ---: | ---: | --- |
| `wiri.exe` | **6,698,496** | 6.388 | `9AD042E896249F4EF6A7311B842416075C968EF6885C7A0AAA9EA627CF5257DE` |
| `wiri-ctl.exe` | **3,220,480** | 3.071 | `C2BCA4BFF5083138DC48372CEF325DF2F43348772E77EA61C6F5B695348BCC18` |

Earlier attempts: one harness invocation was killed by the command runtime ceiling
after queue wait + H14; a completed run of `83e4901` kept summary serialization working
but its perf phase stopped at an open-log sharing error and its same-row layout
predicate timed out on a legitimately parked sentinel. Those incomplete perf
measurements are **not pooled** with this baseline. `af3eb30` fixes the collector,
not wiri's runtime. Independent repeated normal runs and an ARM64 measurement remain
necessary before claiming stable budgets.

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
