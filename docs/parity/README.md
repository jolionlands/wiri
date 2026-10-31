# niri parity: where wiri really stands (2026-10-24)

These two matrices replace the claims in `docs/PARITY.md`. Each row was checked in
wiri's code (config key parsed, then stored, then acted on), not taken from the docs,
against niri `main` at e19df12cf8 (2026-10-22; the latest release tag is v26.04).

- [Layout and behaviour](niri-gaps-layout-behaviour.md): layout, sizing and arrangement
  actions, focus, fullscreen and maximize, floating, workspaces, monitors, tabs, overview,
  recent windows, window rules, effects, animations, gestures.
- [Config, input and IPC](niri-gaps-config-input-ipc.md): config system and KDL, keyboard
  and pointer input, key bindings, hotkey overlay, screenshots, IPC requests and events,
  outputs, non-layout actions, misc and security.
- [Action and CLI contract census](action-census.md): every pinned niri action (143) and the
  `niri msg` subcommands against wiri's KDL, `wiri-ctl` and typed-IPC entry points.
- [Partial rows, classified](partial-rows-2026-10-30.md): every `partial` row in both matrices
  with one primary condition (missing software, evidence not reconciled, hardware, Windows
  backend difference) and the exact remaining condition, plus the `h95_*` live gates added for
  the evidence rows.

| Matrix | done | partial | missing | N/A on Windows |
| --- | --- | --- | --- | --- |
| Layout and behaviour (226 rows) | 171 | 38 | 0 | 17 |
| Config, input and IPC (259 rows) | 144 | 44 | 0 | 71 |

The counts are a recount of current feature-row statuses including integrated Phase 2B,
retained Phase 2C Lane B/D and Lane G/H/S/Z/W/K/F2/T/C2/V/L2/P/A11Y updates and
the R4 source/native follow-ups; they are not a full live sign-off. Official desktop
cycles passed W+K (560/560) and F2 (601/602, H34 included);
C2's H66-H70 gates passed in the 8c6c9b0 cycle and lane T's H58-H61 in the f4d2666 cycle.
The 2026-10-27 cycle at `cc3920d` passed **761/761 enforced gates, 0 failed** (88 further
information entries), including every `h78_*` cursor-hiding gate, all five H31
focus-history gates (`h31_history_crosses_workspaces` included) and lane V's H62-H65 set.
That first recount moved exactly one row to `done`: `set-column-display normal or tabbed`,
whose only stated gap was that nothing was live-verified. The two `cursor {}` rows and
the tablet stylus row stay `partial` for their recorded Windows approximations.

**2026-10-28 recount, following the 761/761 `cc3920d` cycle.** 45 rows moved from `partial`
to `done` — the class-(a-1) list surveyed in [next-lanes.md](next-lanes.md), whose live gates
already existed, are dot-sourced into `scripts/live-test.ps1`, and passed in that cycle:
23 in the layout matrix (lanes L2, T, G, F2, V plus lane B's `h23_*` wheel and lane S's
`h48_*` urgency gates) and 22 in the config/input/IPC matrix. Every flip was cross-checked
against the gate names that actually appear in `scripts/live-test.ps1` and
`scripts/live-phases/*.ps1`. The matrices are now **164 done / 45 partial** (layout) and
**139 done / 48 partial** (config, input and IPC) after that recount; `missing` and `N/A` were unchanged. One
surveyed row was left `partial`: `horizontal-view-movement`, because the only evidence the
survey could name for it is the offline H17 config-decode fragment, which its own header says
does not prove visible motion, and no live gate covers camera scroll.

**2026-10-28 gate-hardening pass, `f4157d3`.** The 2026-10-28 cycle at `f4157d3` is the first
fully green official run at the enlarged gate count: **769 enforced gates pass, 0 fail**, and
the hosted x86_64 live-desktop job passed at the same commit. Reaching it took five gate-side
fixes (h79 PowerShell 5.1 JSON unwrap, h60 idle-frame baseline and held-rect comparisons,
h75 overlay find, h25 warp poll, h15 setup poll), each root-caused against daemon logs and
per-gate evidence: the harness was racing the event-driven runtime's deferred effects, not
finding product bugs. The diagnosis and the open event-chain-latency question are recorded
in [live-gate-notes.md](../live-gate-notes.md).
Lane V's H62-H65 rows no longer rest on unit tests alone: the 2026-10-27 cycle passed
every enforced gate, and its record names the h78 cursor-hiding, h31 focus-history and H65
tab sets explicitly. The rows keep `partial` only for the Windows approximations and
unit-tested-only surfaces (four-finger swipe, promoted touch, multi-display) they document.
Blanket "live pending" labels from earlier lane reports are historical: the
integrated cycles above and the focused run below provide named live evidence.
Remaining partial rows retain their specific semantic or hardware limits.

**2026-10-28 PR #2 verification (`f217e15`).** Tridaris passed 1,800 native tests
and all 62 checks in the affected recent-window, config, layout and overlay
phases. Four layout rows now cite that evidence: height modes, recent-window
fade, recent-windows off/timestamps and the repeated fade-kind row. Current
totals at that checkpoint were **307 done / 89 partial / 89 N/A**. Its frozen
full desktop cycle completed with **775 passing / seven failing** enforced gates.

**2026-10-28 full-parity work in progress.** The latest completed source checkpoint
passed **1,973 tests**, including the interactive graphics test. The preceding
native checkpoint passed all **64** new desktop checks H81-H86 on Tridaris.
The affected keyboard/focus/reload checks passed
**65/65**; a separate session run passed **8/8** after repairing a missing helper
in the temporary focused runner. These are focused results, not a new full-cycle
sign-off. Startup and transfer review follow-ups passed their integrated source
tests and scoped reviews. The complete desktop run against frozen source
`E8C1274F` and copied release binaries finished with **735 pass / 111 fail /
94 information entries**, plus one cursor-restoration phase error. All 64 new
H81-H86 checks passed again. Several older groups stopped at native foreground
preconditions; two H15 oracles expected obsolete explicit-target focus stealing.
Their corrected source passed a subsequent focused native run: both original
explicit-ID assertions and owned setup/cleanup passed (**4/4**). The frozen full
run remains recorded unchanged; this focused result is not a full-cycle sign-off.
The single-output machine cannot qualify cross-output relocation or mixed DPI.

The later canonical layout/output action correction at `0f1c4fc` passed
**75 unique scoped tests**, including its 12 new regressions. This is a source
qualification, not a replacement for the failed full native cycle. The combined
spacing, hotkey-selection and axis-scroll checkpoint at `a62d10d` passed
**2,037 Rust tests**, the separate graphics smoke test and **12/12 H87 native
scroll checks**. All 17 source qualification gates passed. The 53 targeted
spacing/scroll/hotkey tests are included in the full-suite count.
The later pointer-origin correction at `5738371` passed **2,041 Rust tests** and
all 11 source gates; it has no new graphics or native result. The exact immutable
R4 binaries also passed **7/7 native spacing** and **7/7 native panel** follow-up
checks at actual 96 DPI. Root verified the PNG hashes and reviewed both panel
images. Task19's later routing checkpoint `69e0eb7` passed **25/25 height tests**
(21 engine and four compositional IPC cases), all 20 backend-message tests and
**2,066 Rust tests**, with all 14 source gates and the release build passing.
Explicit-ID handler pre-focus and workspace-transfer index lifecycle are corrected;
the IPC tests combine two positive-ID wire normalization controls and two shared-handler
negative-model cases, rather than typed end-to-end or native foreground evidence.
The later Task19 R3 owned native run passed **14/14 checks**, with **38 native/IPC
observations** and **12 verified artifacts**, at actual 96 DPI. It covers floating
height order, wrap, proportion, refresh/reload and transfer lifecycle, plus typed
inactive-target foreground preservation. Root separately reviewed two saved
visible-source images. The earlier R2 attempt stopped before height observations
at an admission-grace focus precondition and cleaned up; R3 settles managed
admission for three seconds before its one explicit focus request. This does not
qualify focus during admission grace. This native result does not cover tiled
height or mixed-DPI hardware. The separate graphics run passed its real
BMP test with no display skip; the 245-file base graph and composed native graph
are recorded in the evidence report. Each result retains
its own source manifests; these results are not a
new full desktop cycle. See the
[R4 evidence and remaining work](2026-10-29-tridaris-r4.md).

The later floating-width lifecycle checkpoint `351065a` passed **four width
regressions**, all **25 height controls**, the **2,070-test suite** and all
**15 source gates**, including the release build. Changed preset lists now
discard stale floating width indices on reload and workspace transfer; unchanged
lists retain their indices. This is source qualification; the native-height
result above remains bound to `69e0eb7`. See the
[width evidence](2026-10-29-tridaris-r4.md#floating-width-list-lifecycle).

The tiled-height checkpoint `1cb2a25` passed **18 tiled-height cases**, **three
follow-up cases**, all **50 height-rule controls**, the **2,091-test suite** and
all **18 source gates**, including the release build. Direct/preset sole and
tabbed heights now preserve their requests rather than filling the view; owner
replacement/reset, border/indicator constraints, legacy Grow/Shrink and
nonactivating height layout are source-qualified. All 197 pinned inputs were
verified; the guarded import changed only four formatted files. This adds no
native or performance result. See the
[tiled-height evidence](2026-10-29-tridaris-r4.md#tiled-height-ownership-and-resize).

The later durability checkpoint `3d7c8f6` passed **four retention
cases**, all prior controls and **2,095 Rust tests**, with
**19 source gates** and the release build passing.
Three same-config reload controls already passed in RED; native minimum-width
refresh caused the actual 300-to-900 px request loss. At that checkpoint, only the native minimum-width caller
preserved the height request; generic rule/config synchronization was unchanged.
The two-file guarded import verifies all 197 inputs. See the
[durability evidence](2026-10-29-tridaris-r4.md#tiled-height-request-durability).

The later lifecycle checkpoint `381ce11` passed **five effective-list
cases**, **four rule/admission cases**, all prior controls and **2,104 Rust
tests**, with **21 source gates** and the release build passing. Its actual
nine-case RED had seven semantic failures and two passing controls; all 54
existing controls passed. Changed effective tiled height lists now convert the
old rendered heights to Auto, while unchanged output overrides retain the preset
index. Constraint refresh preserves existing direct requests and still fills
missing opening defaults. The four-file guarded import verifies all 197 inputs
and preserves 193 others. This adds source qualification only; see the
[lifecycle evidence](2026-10-29-tridaris-r4.md#tiled-height-preset-list-and-constraint-lifecycle).

A separate owned run at the same `381ce11` source passed **21/21 native checks**,
with **34 stable native/IPC observations** and **22 referenced artifacts** at
actual 96 DPI, covering selected sole/stacked/tabbed height and reload behavior.
See the [native tiled-height evidence](2026-10-29-tridaris-r4.md#owned-native-tiled-height-lifecycle).

The same `381ce11` source then completed a bounded owned target-acknowledgement
measurement: **10/10 transitions**, with target UI receipt median/max
**5.8467/7.5401 ms** and raw rectangle read completion **5.8476/7.5408 ms**.
Five fresh observer calibration cases passed. Cold start to the first PID-verified
typed reply was **167.6095 ms**; the **30.0297263 s** idle interval showed no
resolvable daemon CPU increase, with 27 total managed windows and two owned
ordinary forms/listeners. This is target-message/bounds acknowledgement, with no
frame, historical speedup or zero-CPU certification. The first readiness failure
retains its zero-input diagnostics; full R3 remains failed. See the
[performance endpoints and context](2026-10-29-tridaris-r4.md#owned-target-acknowledgement-performance).

**Lane CFG config-bounds and reload checkpoint (`109de46`, `b524943`).** wiri's numeric
config bounds are now niri's: every numeric or ranged option was compared with the pinned
decoder and recorded in [config-bounds-census.md](config-bounds-census.md) (type, range and
error with file:line, wiri before/after, regression). Integer options reject decimals instead
of rounding, window-rule sizes are `u16`, `opacity`/`geometry-corner-radius` take niri's
unbounded range, and options Windows cannot apply (`accel-speed`, `pinch-sensitivity`,
`xcursor-size`) are still type- and range-checked. A reload now re-applies exactly the sections
niri re-applies, in niri's order: `src/config/reload.rs` plans from a diff of the config the
engine holds and the new one, `apply_loaded_config` applies only the planned sections (so a
bind-only reload no longer rewrites pointer settings or re-places windows), and a workspace
whose `workspace "x"` declaration disappeared is unnamed while runtime renames survive. The
suite is **2130 tests**; the release build and `cargo check --release --all-targets` are clean
under `-D warnings`. Four `h91_*` live gates are written in
`scripts/live-phases/lane-cfg.ps1` and parse-checked, but they have not run: neither matrix row
leaves `partial` until the lead's cycle passes them. No dependency, timer, polling loop or
thread was added.

The source audit found missing DPI projection on five appearance rows and a
missing floating-height preset consumer affecting two rows previously marked done.
After the qualified R4 source and native follow-ups, `gaps`, `struts` and the
curated hotkey list move to done, the 2026-10-30 action census moves
`binds` action names to done, and Lane FS moves "Fullscreen and maximized windows
cannot be floating" to done on 22 source-pinned engine cases (its `h88_*` live gates
are pending their first cycle; the row returns to partial if they fail). The focus lane
then made the `strict-new-window-focus-policy` debug flag real on Windows, so that row moves
from N/A to partial. The overview lane replaced the fit-everything overview with niri's
fixed-zoom camera, so "Live zoomed-out view" returns to partial until its `h97_*` gates pass
in an official cycle (they passed in the lane's own desktop probes). The current totals are
**315 done / 82 partial / 88 N/A** across 485 rows: **315/397 applicable rows (79.3%)**, after
the official dba4606 cycle passed all 11 `h93_*` gates and five pointer rows moved to done, and
then the official dab9a88 cycle of 2026-10-31 passed the h88-h97, h61 and h17 gate families and
six more rows moved to done. Floating-height resolution/routing and the
sole/tabbed tiled-height correction are source-qualified. Both preset-height rows
remain partial for physical mixed-DPI and remaining lifecycle routes. The lifecycle
checkpoint qualifies its listed source cases; earlier floating native evidence
retains its own source binding.
Appearance scaling, other height lifecycle routes, input/config semantics and
hardware evidence remain open. See the [full-parity plan](../superpowers/plans/2026-10-28-full-parity.md)
for the active work; a gate count alone is not full parity.

**2026-10-30 partial-row reconcile (Lane EVIDENCE).** [partial-rows-2026-10-30.md](partial-rows-2026-10-30.md)
classifies all 92 rows that were partial at that base by one primary condition: **5 missing or incorrect software, 15
implemented but evidence not reconciled, 37 hardware evidence unavailable, 35 Windows backend
differences**. The matrix totals are unchanged — **164 done / 45 partial / 17 N/A** (layout) and
**141 done / 47 partial / 71 N/A** (config, input and IPC), 305 / 92 / 88 of 485 — because no row
was promoted: promotion needs a named gate to have passed in a complete official run, and this
pass could not read `C:\wiri-dev\evidence\<run>\`, so the 15 evidence rows instead name the gate
they need. Eight `h95_*` gates are now written in `scripts/live-phases/lane-evidence.ps1`
(dot-sourced next to the other lanes and parse-checked, not yet run in a cycle) and settle five
of those rows plus the overview half of one backend row; the remaining rows point at gates that
already exist in `lane-a-camera-motion.ps1`, `lane-startup-rules.ps1`, `lane-pango.ps1`,
`lane-pace.ps1`, `lane-cfg.ps1` or `lane-focus.ps1`, or name the one check still to write.

**2026-10-31 official cycle `dab9a88` (Lane PROMOTE).** The official tridaris cycle at
`dab9a88` (run directory `C:\wiri-dev\evidence\dab9a88-live`, 2026-10-31, 911/915 boolean gates
passed) ran the gates that the remaining rows were waiting for: `h88_*` 10/10, `h89_*` 6/6,
`h90_*` 2/2, `h91_*` 4/4, `h92_*` 11/11, `h94_*` 6/6, `h95_*` 8/8, `h96_*` 6/6, `h97_*` 8/8,
`h61_*` 4/4, `h17_*` 48/48 (niri's exact numeric limits accepted and values just past them
rejected), `h25_*` 8/8, `h28_*` 3/3 and every `h15_*`. The only non-passing entries were the three
`h98_*` movement gates (a wiri extension, not a parity row) and
`h30_floating_directional_focus_stays_in_workspace`, one raw-IPC case that raced its fixture
while the identical CLI case passed. Six rows move from `partial` to `done`, each citing the gate
that passed: Mouse move/resize of floats via the Mod key, Tab indicator, the "Live zoomed-out
view" overview row, Gesture kinetics, `hotkey-overlay-title` Pango markup and "What a reload
re-applies". Fourteen more rows had their named gates pass but keep `partial`: their text also
records a hardware, mixed-DPI, multi-output or Windows-backend condition that a gate cannot
clear (self-resize's unread maximum sizes and minimum heights, `center-column` on differing
physical outputs, `FloatingActive::NoButRaised`, the overview's multi-display keyboard path, the
opaque-host alpha of the backdrop and workspace shadow, the new-window token path and strict
cross-workspace detail in both focus-policy rows, other refresh rates and multi-output for the
animation clock, a real Precision Touchpad, non-animation config bounds, the action census's six
executor-evidence partials, `background-color`'s per-output backdrop and Windows' absent
per-client inhibition protocol). Recount: **171 done / 38 partial / 17 N/A** (layout, 226 rows) and
**144 done / 44 partial / 71 N/A** (config, input and IPC, 259 rows), i.e. **315 done / 82
partial / 88 N/A** across 485 rows, **315/397 applicable rows (79.3%)**. The stale "pending"
sentences in the rows this cycle settled are replaced by the gate names and the official run
they passed in.

Earlier checkpoints (layout; config): Phase 2A (`b702890`) was 53 / 58 / 100 / 15; 43 / 74 / 72 / 70, Phase 1 was
30 / 70 / 111 / 15; 15 / 86 / 88 / 70, and the original audit was 2 / 85 / 124 / 15;
11 / 87 / 91 / 70. Unqualified line numbers in both files refer to commit `1532635`;
phase-labelled rows relocate the changed code by function name. Each matrix ends with a list
of the working-tree edits that were in flight during the original audit. The
[Phase 1](phase1-report.md), [Phase 2A](phase2a-report.md) and
[Phase 2C Lane D](phase2c-lane-d-report.md), [Lane H](lane-h-report.md) and
[Lane S](lane-s-report.md) reports distinguish live verification from unit tests.

## Plan

**Phase 0, done (1532635..4b9d834):** wiri runs on a live desktop. No repositioning loop,
IPC never blocks, every window is tiled, focus follows the OS, xkb/niri key names, a
GUI-subsystem build with a log file, crash-safe hidden windows, Escape reaches
applications, the overview no longer freezes, `move-column-left/right` swaps. Verified by
`scripts/live-test.ps1` on a spare Windows 11 laptop.

**Phase 1, done (`02e8aab`..`6ea45e4`, [report](phase1-report.md)), safe to daily-drive (P0):** unknown
modifiers are rejected and `Mod` is tied to `mod-key`; `spawn` and `spawn-at-startup` keep their
arguments and search PATH; every action targets the real foreground window, floating ones
included; minimized windows leave no gap; floating windows belong to their workspace;
niri's keymap semantics (`consume-or-expel`, `maximize-column`, first/last moves) and its
action names and argument grammar (`focus-workspace 2`, `set-column-width "+10%"`);
workspace model (index addressing, previous-workspace memory, real named workspaces); the Phase 0
leftovers (a terminal's minimum width, an exact fullscreen, no `Ctrl+Alt+Tab` default, windows put
back after a crash); monitor unplug keeps the windows (**unit-tested only: the test laptop has one
display**); hiding inactive workspaces by shell cloaking so windows stay in the taskbar and Alt+Tab is
there as the opt-in `hide-method "cloak"` (`SW_HIDE` stays the default).

**Phase 2A, done (`0a0bb75`..`b702890`, [report](phase2a-report.md)):** a real KDL parser with visible
errors (a bad config is never applied half; `cooldown-ms=` and `repeat=false` act); window rules that act
(`open-on-output`, `open-on-workspace`, `open-floating`, `open-fullscreen`, `open-maximized`, `open-focused`,
default sizes, size limits, opacity that follows the window); the fullscreen and maximize model (the window keeps
its column, `maximize-window-to-edges`, an application that goes fullscreen or maximizes itself is followed);
niri's IPC (the event stream: the state first, then every change; `wiri-ctl action` with niri's arguments, quoting
and `--id`).

**Phase 2B Lane A (P1), integrated and validated (`81ff35c`..`bd71c96`):** unified lazy decorations,
configured analytic spring/easing motion, active-only tabbed columns, live DWM workspace/recent previews
and a monitor-fitting Important Hotkeys sheet. The complete locked integrated run passed **440 gates,
0 failed**, including all **102 H16–H21** checks and retained Lane B/D gates; 1114 library + 7 CLI tests
pass. [The Phase 2B report](phase2b-report.md) separates current evidence, earlier-run findings and
remaining compositor approximations. Recent/modal input uses Lane B's existing dispatcher, primary
keyboard engine and pointer/focus architecture, not the older keyboard relay thread. No new dependency,
idle timer or always-on thread; passive decoration HWNDs are lazy and transient UI is session-local.

**Phase 2C Lane B ([report](phase2c-input-report.md)):** primary low-level keyboard bindings,
mouse/wheel bindings and event-driven pointer focus are retained. Other Phase 2 work still includes
full live sign-off of Lane S screenshot/system actions and the explicit partial rows in both matrices.

**Phase 2C, Lane D ([report](phase2c-lane-d-report.md)):** focused-output and focused-window PNG
screenshots, the clipboard, `screenshot-path` and capture events/notifications are implemented.
H26-H28 verify both hotkeys, saved PNG content, clipboard and null-on-reload live. Output
identity matching, startup focus, reload-to-real-DPI and explicit-current-mode no-op pass H29;
physical mode/position/rotation/off changes are synthetic-unit-tested only and never exercised
on this laptop. Screenshot selection UI/action flags and output limitations remain explicit in
the matrices/report. Other Phase 2 lanes are separate work.

**Lane S ([report](lane-s-report.md)), implementation with incomplete desktop sign-off:**
frozen screenshot selection, per-capture options/nonfocusing ids and shared non-layout
system actions are implemented. Warning-denied release build/check and 1241 library +
14 CLI tests pass. The interrupted desktop run persisted 18/21 Lane S gates passing
(including all H46/H47 checks), with three failures corrected but not rerun and four
H49 gates unreached. No full summary or new integrated gate total is claimed. The
formatter and locked rerun still require runner permission; real power mutations are
never part of validation.
**Phase 2C Lane F2 ([report](phase2c-lane-f-report.md)):** structured focus,
workspace and monitor actions are connected to the integrated B/2B dispatcher and H IPC,
retaining G's width intent, strip/strut policy and remembered tile/column selection.
Workspace names, visible order and stable IPC identity are independent; native occurrence
history drives debounced previous-window focus without a retry timer or stale focus replay.
H30–H34 exercise CLI and typed IPC on the single-display desktop; cross-output actions,
EDID preferences and hotplug are unit-tested only. Named fallback placement and nested
workspace layout overrides remain open. The saved desktop run has 519 pass / 15 fail;
the final floating-memory/target fixes and revised harness remain unvalidated live (see report).
**Lane K, provisional ([report](lane-k-report.md)):** on-demand DWM thumbnail open,
close, two-sided workspace and resize effects reuse the existing input pump/active-only
driver. The saved lane-local run at `267a612` has **497 pass / 6 fail**, with all ten
H54–H57 checks passing; the six H22 checks were not reached after another Terminal
took foreground. This is not integrated sign-off. Section 14 records the implemented
Windows approximations and remaining partial rows; matrix status totals are unchanged.
[PERF.md](../PERF.md#lane-k-transient-compositor-checkpoint-2026-10-26) records individual
animation samples and same-process idle endpoints, not a zero-cost or leak-free claim.

**Lane V ([report](lane-v-report.md)):** the configured overview camera, pointer/touch/
touchpad interaction, hot corners, explicit tab display and the clickable tab indicator,
ported onto the stable-workspace head. Every row it touches is `partial (Lane V; ... live
pending lead cycle)` until H62-H65 pass in the lead's locked desktop run; the totals
table above is regenerated by the lead and does not yet count these status changes.

**Phase 3, shine:** elevated windows (uiAccess or a task launch), per-user pipe names,
ARM64 release builds, docs and a demo. Lane D fixed always-quoted Run autostart command
construction (four pure tests; no real registry/logon test) and documented elevation choices
in [ELEVATION.md](../ELEVATION.md); it does not enable elevation.

**Phase 4, super lightweight and fast (owner, 2026-10-24: "full parity and functionality,
super lightweight performant after core functionality is done"):** once the core works,
measure first (added to the live test as info gates), then enforce budgets:
- idle CPU about 0: event-driven only, no polling timers (today a 750 ms reconciler, a
  500 ms focus-follows-mouse poll and overlay timers wake wiri while nothing happens);
- few threads (the first live run showed 23 at idle; `tokio` is built with `full`), small
  private memory and binary, fast cold start to the first tiled layout;
- hotkey-to-`SetWindowPos` latency within one frame (16 ms) and no extra layout passes
  per event (count `SetWindowPos` calls per action);
- measured on the owner's ARM64 laptop as well as on x64.

## How changes are verified

`scripts/live-test.ps1` runs in an interactive desktop session: it starts the freshly
built `wiri.exe`, opens test windows and terminals, drives wiri over `wiri-ctl` and real key
presses, and writes `summary.json` plus screenshots. A change is done when `cargo test
--release` and `cargo check --release --all-targets` with `RUSTFLAGS=-D warnings` pass and
the live gates hold (no repositioning once settled, every test window tiled without
overlap, no IPC timeouts, no unknown-key warnings, every window visible after quit and
after a crash restart).  A live run holds a machine-wide lock, so two runs can never overlap
and spoil each other.

**2026-10-29 explicit admission-focus checkpoint (`75fd0f3`).** Explicitly
selecting a recently admitted floating or tiled window now supersedes its
opening-only focus guard. The expanded baseline had two failing cases and two
passing controls; all four now pass, along with **24 source checks, 2,108 Rust
tests and both release builds**. A separate real BMP test passed without a
display skip from a new frozen copy containing 251 files. Native admission-focus, physical
mixed-DPI and the full desktop cycle remain pending; the failed full R3 stays
recorded. All 485 matrix feature/status pairs remain unchanged: **303/396 done
(76.5%), 93 partial, 89 N/A**. See the [R4 checkpoint](2026-10-29-tridaris-r4.md#explicit-focus-during-recent-admission).

**2026-10-30 action and CLI contract census.** [action-census.md](action-census.md) runs niri's own
KDL node, CLI words and typed JSON for all 143 pinned actions through wiri's parsers, with the
measured gate `ipc::action_census` (119 full, 15 partial, 9 N/A, 0 missing). It fixed a wrong
claim (`move-column-to-index` is 1-based) and nine further contract mismatches, and moves matrix B
row 178 to done: **304/396 applicable rows (76.8%), 92 partial, 89 N/A**. Rows 232 and 257 stay
partial: nine `*ById` forms still activate the named window first, and `--print-request` prints
wiri's dialect.

### Lane SYNTH — touch, pen and drag-and-drop on OS-synthesized input (2026-10-31)

The last five rows that were "implemented and unit-tested only" on the pointer paths now have live
evidence that does not depend on hardware: `scripts/live-phases/lane-synth.ps1` (H93, eleven enforced
gates, dot-sourced next to the other lanes) creates a real Windows pointer device with
`CreateSyntheticPointerDevice(PT_TOUCH/PT_PEN)` and injects contacts with
`InjectSyntheticPointerInput`, and it drives drag-and-drop with a genuine OLE drag
(`DoDragDrop` from a WinForms mouse-down, so the capture belongs to OLE's own window of the source
process). A `WH_MOUSE_LL` recorder proves the promotion wiri's hook consumes: touch is
`MI_WP_SIGNATURE | 0x80 | contact-id`, pen is the same without `0x80`, the pen barrel is promoted to
`WM_RBUTTONDOWN`, and plain `SendInput` input carries no signature at all. The final owned probe run
(`.live/synth-probe-20261031-000911`, this worktree's release binaries) passed **11/11 gates**.
Evidence wording for the rows below is `OS synthetic pointer device (InjectSyntheticPointerInput), no
physical digitizer`.

- DnD edge view scroll, DnD edge workspace switch and DnD hold-to-activate move from `partial` to
  `done`: the strip travelled 217-238 px with every fixture's top edge and height unchanged and the
  frames stopped when the zone was left; `trigger-width 140`/`delay-ms 600` suppressed scrolling
  before the delay and outside the zone; the overview opened under a held drag stepped exactly one
  workspace and did not repeat; the native foreground followed a 1.15 s hover.
- Tablet pen (Mod+drag a window) moves to `done`: the promoted pen events carry exactly the extra info
  wiri classifies, and a Mod+pen drag moved a floating window by -80/+56 px and left it there.
- Tablet stylus triggers `TabletStylusButton1-3` moves to `done`: the `Alt+TabletStylusButton2` bind
  registered once and fired from a promoted barrel contact (tiled -> floating). Buttons 1 and 3 use
  the same mapping but were not exercised live, and which physical button a driver promotes to which
  mouse button remains an approximation.
- Touchscreen Mod+drag and the touch overview stay `partial`: the drag/scroll halves are now live
  (`h93_touch_mod_drag_scrolls_the_view` moved the strip 216 px for a 150 px contact drag;
  `h93_touch_mod_drag_moves_a_floating_window` moved a float +96/+64 px), but the second-finger tap
  was not promoted by Windows for a synthetic second contact in the observation window
  (`second_contact_hook_lines` empty) and the overview's long-press pickup pressed inside the painted
  thumbnail yet never dropped the window to a workspace below (`touch_overview_long_press_observed`,
  `moved=false`). Both stay unit-tested only until a probe explains them.

Recount at that branch point: 310/397 (78.1%); on the integrated branch, where the overview row had
returned to partial, it is **309/397 (77.8%), 88 partial, 88 N/A**. The five new `done` rows were
confirmed by the official dba4606 cycle, which passed all 11 `h93_*` gates. No `src/` file
changed: the probes demonstrated no defect in the touch, pen or DnD paths.
