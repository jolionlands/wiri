# Phase 2B Lane A: look and feel

## A2 integrated-branch validation — complete

Branch `meylin/sa-f9cbe-6b716`, base `df229cd`; final tested source/binary **`bd71c96`**.
The runner refused cherry-pick, so the six requested source changes were ported as
explicit worktree edits in source order. Each part builds. Existing LF in the shared engine
is retained; no source-branch EOL churn. No dependencies or Cargo.lock changes.

| Source change | Integrated commit | Scope |
| --- | --- | --- |
| `1158e31` | `81ff35c` | decoration foundation (item 1) |
| `6b52f47` | `a21c603` | session-local overview (item 4), rustc 1.95 explicit autoref |
| `696c62e` | `976c04a` | recent-window service (item 5) |
| `5daa01d` | `5e21ebf` | recent-window diagnostics |
| `16f332f` | `488c279` | shared integration, motion/tabs/hotkeys (items 1/2/3/6) |
| `922bd30` | `826ba68` | 102 H16–H21 acceptance checks |

Integration corrections: `a804d4c` resolves the startup probe through `pipe_path()`;
`f45c441` fixes H16's gesture configuration and the perf observer's obsolete endpoint;
`bd71c96` fixes the older H5 fixture's foreground precondition. None changes a threshold
or weakens an existing assertion. The IPC/CLI feature lane is otherwise untouched.

**Lane B architecture retained:** recent input uses the existing ordered `Work` queue on
its hotkey message thread, not the source lane's action channel/relay or dedicated keyboard
thread. Primary keyboard binding/fallback/repeat state, pointer grabs, focus dwell/warp and
Lane D output/screenshot setup remain. Modal input advances physical state and retires
captured releases without firing normal/release actions; tagged activation input is bypassed.

### Final required validation

All three commands passed with `RUSTFLAGS=-D warnings` and `-j 2`: release build,
release tests (**1114 library + 7 CLI**, zero failed; one ignored doctest), and release
all-target check. Logs: `.scratch/final-{build,test,check}.log`. CI-toolchain formatting via
`C:\wiri-dev\fmt.cmd` and the collector-only PowerShell/native self-test also passed.

The exact full live command with `-LockWaitMinutes 120` completed in **1123 seconds**:
**440 pass / 0 fail / 77 information**, using the unchanged `scripts/live-report.py` rules.
`.live/summary.json` has 426 true booleans; its sole false boolean is the expected-negative
`hidden_json_present_after_restart`. No IPC timeout, settled placement or ordinary-form
overlap; `h9_overlapping_terminal_pairs=0`. H15 typed JSON actions still pass. The H19
preflight quit returns 1 because its specified precondition is no daemon; it is not a failure.

| Integrated checks | Passed / total |
| --- | ---: |
| Lane A H16 decoration | 12 / 12 |
| H17 config and motion | 45 / 45 (37 offline config checks, not visible-motion evidence) |
| H18 tabs | 7 / 7 |
| H19 overview | 7 / 7 |
| H20 recent windows | 14 / 14 |
| H21 Important Hotkeys | 17 / 17 |
| Retained Lane B H22–H25 | 47 / 47 |
| Retained Lane D H26–H29 | 13 / 13 |

Evidence paths and live-versus-unit limits for each item are listed below; they now also
refer to the integrated `.live` run. H5 additionally records foreground HWND **791226**
(the requested float) immediately before input in `h5-move-input.json`. TRIDARIS is x64,
rustc 1.95, one 1366×768 display / 1366×728 work area at 96 DPI. The collector reports
**Windows 11 Pro 10.0.22631**, not the Home/build 26200 description in the historical
source report. No physical display setting changed; multi-output, non-96-DPI and physical
mode-change behaviour remain unit-tested only. No load/stress/soak tests were run.

### Findings from integrated runs (not pooled with the final result)

- First complete integration run: 438 pass / 1 fail / 73 info. H16's unchanged Ctrl+Alt
  injection was plain Mod under its Ctrl+Alt configuration: lane B correctly performed a
  move, not Ctrl+Mod column reorder. Configure Alt as Mod; the insert-hint assertion stays.
- Second complete run (`f45c441`): 435 pass / 4 fail / 77 info; all 102 Lane A checks passed.
  H5's float stayed on workspace 1. `wiri.stderr5.log` shows float 7934162 focused at
  00:26:20.982Z, Terminal 5378534 taking foreground at 00:26:22.403Z, then the move hotkey
  at 00:26:22.661Z. The terminal, not the float, correctly moved. This caused
  `h5_move_to_workspace_hides_the_float_here`, `h5_ipc_reports_float_on_workspace_2`,
  `h5_float_shown_on_its_new_workspace_at_the_same_place` and
  `h5_float_hidden_again_on_workspace_1` to fail. Preserve evidence in
  `.scratch/second-live/{summary.json,wiri.stderr5.log,h5-windows-moved.txt}`. The fixture
  now settles before focusing and adds a foreground precondition gate; no product targeting
  change. That run also recorded one informational terminal-overlap pair, versus zero in
  the first and final runs: minimum-width readiness still warrants follow-up, not a guarantee
  that the older intermittent observation is impossible.
- An invocation hit the tool's 30-minute ceiling after lock queueing and H15; it had no
  final summary and is not counted as a completed run. A mutex-only wait followed by the
  unchanged full command allowed the final run above to complete.
- The perf observer used the obsolete global pipe. It now assigns its owned child a unique
  explicit `WIRI_PIPE`, uses that endpoint and retains server-PID validation. Final collector
  errors **0**; `p_*` remain information only. Passive decoration classification requires
  both `WiriDecor` and every required extended style, with no APPWINDOW; H16 independently
  checks those styles, shell exclusion and cleanup.

### Resting cost and remaining work

No added always-on thread or periodic idle timer. Motion has a frame sleep only while it
animates; MRU focus/preview and pointer dwell are cancellable one-shots. Decorations reuse
Lane B's message pump: one lazy frame HWND per visible decorated tile, plus eligible tab
indicators; no insert hint at rest. Overview/recent/cheatsheet hosts and their threads are
session-local. Existing reconciler/main-loop/indicator activity is not claimed removed.

The final perf checkpoint (`.live/perf/{measurement,idle}.json`, 85 managed windows) has
**0 ms CPU / 30.0084 s**, unchanged input ticks, **0 positioning lines**, and **251.66/s**
endpoint-counted context switches (not timer wakeups). Startup/after endpoints: **21/23
threads**, **8,097,792/9,117,696 private bytes**, **23,838,720/27,742,208 working-set bytes**;
three passive decoration HWNDs were observed at startup. First IPC/layout: **249.97/186.73
ms**. Ten confirmed hotkeys: **21.76 ms median / 25.69 ms max**, with a **25.69 ms** maximum
observer gap. These are a single noisy checkpoint, not proof of zero idle work, a leak slope
or sub-frame latency. Phase 4's proposed thread, after-action memory, latency and binary
budgets are not yet met; its historical baseline is unchanged. See [PERF.md](../PERF.md).

Remaining compositor/style/input approximations and unit-only scenarios below stay open
in the matrices. ARM64, a second display, scaled desktop and full niri compositor parity
are not claimed. Everything below is the **source lane's historical report**, not a second
claim that its original desktop run occurred on this branch.

## Source-lane report (historical)

Branch `meylin/sa-c6e38-6b646`, based on the unfinished decoration work in `1158e31`.
This is the look-and-feel lane, not a claim that every Phase 2/niri compositor feature is done.
The two matrices record remaining partial rows. See README for recounted integrated totals;
counts cited in this historical source section are not new integrated results.
Reference: niri `main` e19df12cf8 (v26.04-era docs).

## Commit organization

- Overview implementation: `6b52f47` (item 4).
- Recent-window implementation: `696c62e`; diagnostic live-harness corrections: `5daa01d` (item 5).
- `16f332f`: the remaining decoration, animation, tab and hotkey implementations (items 1/2/3/6),
  plus shared config/input/layout lifecycle wiring for all six items. These shared integrations
  could not be cleanly isolated by whole-file staging. The requested item order was not preserved
  by the implementation workers; this is recorded rather than implying six independent deliveries.
- The final live-harness commit adds the single Lane A entry point and desktop evidence checks.

## Environment and verification

TRIDARIS: shared Windows 11 Home build 26200, x64, rustc 1.95; one **1366x768** output,
work area **1366x728**, actual **96 DPI**. No resolution, scale or orientation changes.
Live checks use the parent harness's machine-wide `Global\wiri-live-test` mutex, IPC, real
keyboard/mouse injection, WinForms fixtures, observed HWND/DWM properties and desktop PNGs.
Only fixture windows and processes started by the harness are closed. No stress/soak/load tests.

Required commands (all with `RUSTFLAGS=-D warnings`):

```text
cargo build --release -j 2
cargo test --release -j 2
cargo check --release --all-targets -j 2
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\live-test.ps1 -Bin target\release -Out .live -LockWaitMinutes 60
```

Final command results and gate counts are recorded below after the final run.
The parent harness dot-sources `live-phases\lane-a.ps1` before serialization of the existing summary.
That entry point adds H16–H21 without replacing older phases. A narrowly scoped measurement fix
excludes a `WiriDecor` HWND from the accidental-application-window count only if it has all layered,
click-through, NOACTIVATE and TOOLWINDOW flags and no APPWINDOW flag. A bad-style host still fails;
H16 independently verifies styles, shell exclusion and cleanup.

## Items and evidence

### 1. Unified decorations

`overlay/decor/{paint,spec,layer}.rs`, `layout/engine/decor.rs` and the existing message pump.
A tile's border, active focus ring and shadow share **one** lazy premultiplied-BGRA layered HWND.
Tab indicators and the transient insertion hint use that same layer, not separate rendering systems.
The layer is click-through (`HTTRANSPARENT`, `WS_EX_TRANSPARENT`), `NOACTIVATE`, `TOOLWINDOW`,
stacked below its source, excluded from managed tiles and shell switchers, and destroyed when unneeded.
No decoration thread, timer or polling. WinEvent moves—including self-applied animation/grab moves—
keep the decoration aligned before the engine's layout-feedback guard.

**Live H16:** frame extents around the target, real focus, styles/Alt+Tab exclusion, move/resize following,
real column grab/drop hint lifecycle, DWM round-corner preference, shadow-on reload expanding the
same layer, and quit cleanup. Screenshots: `h16-decorated.png`, `h16-insert-hint.png`, `h16-shadow.png`.
**Pure tests:** rounded coverage, premultiplication/alpha, colour parsing, gradient coordinates/angle,
Gaussian shadow/spread/offset, frame extents, urgency/inactive paints, DWM radius mapping,
visible-tab-only column destination bounds and suppression for fullscreen/overview.
**Limits:** interpolation spaces other than sRGB are accepted but rendered in sRGB; DWM offers square,
4px and 8px corner choices, not arbitrary compositor clipping. No per-window style/radius overrides.
The insertion hint is for the existing column-reorder gesture, not every possible insertion gesture.

### 2. Configured analytic motion

`config/animation_config.rs`, `decode/animations_decode.rs`, `layout/animation/timeline.rs`,
`layout/engine/motion.rs` and a notification-driven task on the existing Tokio runtime.
Global off/on/slowdown and per-kind spring/easing/off reach workspace, view, movement and resize.
The oscillator is analytic for under/critical/overdamping; retargeting preserves displayed position
and spring momentum. Resize and position have independent kinds. Reload cancels previous timelines.
The frame task has **no pending sleep while idle**; while moving it uses actual deltas and a 16ms sleep.

**Live H17 motion:** intermediate/final real HWND width, column swap and incoming vertical workspace
position; stable rest and immediate resize after `animations { off; }` reload. `h17-motion.json` records
sampled rectangles; screenshots include resize, workspace slide and disabled settled geometry.
**H17 config-only gates:** offline `validate-config` fixtures, exact exit codes and located diagnostic
fragments; these do not prove visible motion. **Pure tests:** timestep independence, all damping modes,
slowdown/instant axes, retarget position/velocity, finite termination and backend-driver composition.
**Limits:** disabled when the section is absent (wiri compatibility; niri differs); an empty section opts
into springs. No foreign-window content cross-fade, outgoing-workspace compositor slide, open/close
snapshots, shader support, gesture velocity hand-off or hardware-vsync pacing. Legacy `cubic-bezier`
is a cubic-out alias, not a control-point solver. Undamped springs have a 60-second animation-time
safety cap. Numeric damping bounds are wider than niri's.

### 3. Tabbed columns

`layout/engine/tabs.rs`, `workspace.rs`, `Monitor::set_focus` and the unified indicator specs.
Global/output `default-column-display "normal"|"tabbed"` selects new columns. The active tab alone is
positioned/shown; inactive tabs go through the normal crash-recoverable hide path. Layout shows the
selected HWND before activation. Focus and selection remain one state through consume/expel/removal.

**Live H18:** default tabbed single column, one visible tab, toggle normal/tabbed, focused tab selection,
consume, next/previous cycle and close. Screenshots: `h18-normal.png`, `h18-tabbed-selected-second.png`.
**Pure tests:** removal ordering, edge navigation, horizontal return to active tab, indicator reservation,
geometry, paints and options. **Limits:** indicator is click-through (no click-to-select); urgency and
alternate sides are unit-only. Per-window default-display and decoration rules remain gaps.

### 4. Workspace overview

`overlay/thumbnail_overview.rs` and `layout/engine/overview.rs` replace the inert host with a
session-local DWM/GDI message thread. It shows half-scale normal workspace views stacked around the
active workspace, workspace labels, focus outline and live foreign-window thumbnails. DWM work is
not done on the engine/main thread. Closing is asynchronous and never joins under the engine lock.

**Live H19:** actual DWM registrations and changing preview pixels, unchanged source rectangles,
responsive IPC while open, keyboard selection, real mouse selection, Escape/toggle exit and cleanup.
`h19-*` screenshots and the evidence JSON distinguish real DWM registrations from candidate count.
**Pure tests:** negative-origin scaling/cropping, hit tests and focus geometry, mock lifecycle/cleanup.
**Limits:** no drag/drop between workspaces or right-drag scrolling; full normal-bind and gesture
parity is not claimed. Only the focused output is hosted; multiple-output behaviour is unit-only.

### 5. Recent windows

`input/recent.rs`, `layout/engine/recent.rs`, `overlay/recent_windows{,_win}.rs` and the typed decoder.
The hook dispatches through the existing keyboard drainer. It tracks physical modifiers before
release dispatch, snapshots a frozen MRU list, and waits for the last held modifier to commit.
Focus debounce and preview delay are cancellable one-shots, never idle polling. DWM preview work
and host resources are session-local. Ordinary binds win a collision with dedicated recent binds.
Wiri's own tagged F24 activation nudge cannot falsely count as real application input.

**Live H20:** successful DWM preview registrations, repeated forward/back cycling, no source geometry
or foreground mutation while previewing, pairwise Alt→Shift→Win→Ctrl handoffs and final release,
Escape cancellation, A/O/W/S scope navigation, a binding 96px preview-height cap, a sub-750ms
transient-focus race, application-key early MRU commit, title-independent same-executable filtering,
and quit with an active host. `h20-evidence.json` includes timing preconditions and first mismatch/
quit-component diagnostics. A missed timing precondition is a failed gate, not a pass.
**Unit-only:** distinct app exclusion, distinct-workspace/output membership, off/reload cancellation,
precise 150ms show-delay semantics, collisions/repeats/modifier sides, close/screenshot mappings,
highlight painting and extreme geometry. H20's scope keys do not prove cross-workspace filtering,
and fixtures share one executable identity. **Limit:** no IPC focus timestamps.

### 6. Important Hotkeys

`overlay/hotkey_rows.rs`, `bindings_cheatsheet.rs` and the existing input hooks. Rows use the actual
registered bindings, readable curated essentials, configured title overrides, null exclusion and
alias/chord deduplication. The focused monitor's work area and DPI bound a lazy NOACTIVATE sheet;
columns and wheel pages retain overflow instead of silently dropping rows. Any key/click dismisses;
the dismissal key, repeats and release are consumed before other action dispatch.

**Live H21:** no pre-show HWND, IPC action and real Mod+F1, caption, bounded work-area geometry,
letter/held-Shift/Alt/Escape dismissal, no letter delivered to the application, recreation, IPC and quit
cleanup. Stable owner-drawn screenshot controls prove title strings/null change pixels, not OCR or
the semantic correctness of text. Pure row tests separately prove title override and null removal.
**Unit-only:** 125/150/200% scale, negative origins, overflow-page coverage, click dismissal and
curation edge cases. No real scaled-DPI run was made. **Limits:** plain GDI text, no Pango markup;
exact niri not-bound placeholders and curation ordering are not implemented.

## What the desktop run found

The first full run preserved the earlier phases but exposed new-lane failures: the insertion hint
used the window's stale open rectangle for hit-testing; resize timelines were cancelled by concurrent
camera frames; the motion fixture used an unsupported workspace action and an unsupported width
configuration; the recent test pressed the shell's four-modifier Office chord and lacked first-mismatch
evidence; a disposed/uncached process handle could leave an empty quit exit code. Product fixes and
harness fixes are kept distinct. No previous gate was relaxed.

`hidden_json_present_after_restart=false` is an expected negative flag: it asks whether hidden entries
remain after recovery, not whether the originals-only journal file exists (see Phase 1 report).
The H19 preflight `quit` may have rc=1 because its specified precondition is no daemon; it is not an
IPC responsiveness failure. Offline invalid-config fixtures intentionally return nonzero.

## Final results

The second full locked run completed in 1321 seconds: **377 pass / 2 fail / 31 information** by
`scripts/live-report.py`, including **101/102** new Lane A boolean checks. The failures were the
legacy accidental-window count including three intentional passive decoration HWNDs and the
concurrent-camera resize intermediate-frame check. The legacy terminal-overlap observation also
recorded **2 pairs** (information in that reporter, but not acceptable as a clean sign-off).
The final required validation completed successfully after fixing passive-decoration classification
(class AND all required extended styles) and preserving resize timelines across camera frames:
**1001 library + 4 CLI passed**, zero failed, one ignored doctest. Release build, release tests and
release all-target check all returned zero with `RUSTFLAGS=-D warnings`; the complete locked desktop
harness returned zero as well. Required-command chain duration was 1487 seconds.

Final `.live/summary.json`: **379 acceptance gates pass / 0 fail / 31 information** under the existing
`scripts/live-report.py` rules; **102/102 new Lane A gates pass** (H16 12, H17 45 including 37 offline
configuration checks, H18 7, H19 7, H20 14, H21 17). Of 366 boolean entries, 365 are true and the sole
false is the expected-negative hidden-record recovery flag. The preflight H19 quit is the only
nonzero IPC step (no daemon was its precondition). **Important unresolved observation:** the older
`h9_overlapping_terminal_pairs` is still **2**, which the current reporter treats as information.
Therefore a blanket claim that every older desktop condition is clean would be false; terminal
minimum-width behaviour needs follow-up. No existing gate was set to true or threshold relaxed.

Direct targeted changes and CRLF-only normalization of `src/layout/engine.rs` were refused by the
runner's write scope. Access requests remain pending; no bypass was attempted. Explicit camera
scroll integration and broader workspace/rectangle composition therefore remain incomplete. The
large EOL-only engine diff must be normalized by an authorized reviewer (use
`git diff --ignore-space-at-eol` to review substantive changes).

## Cost and remaining work

No dependency/Cargo.lock changes. No added always-on thread or idle animation/decoration timer.
Decorations reuse the existing hotkey pump; while enabled and visible there is one frame HWND per
decorated tile plus a tab-indicator HWND per eligible column, zero transient-hint HWNDs at rest.
Overview/recent/hotkey host threads and HWNDs are created on use and end on dismissal. One-shot MRU
commit tasks may remain pending only until the configured debounce expires after a focus event.
Existing main-loop/reconciler/focus-follows-mouse behaviour is not advertised as removed; Phase 4
still owns total idle CPU/memory/thread budgets and hardware-vsync/latency measurements.

No second monitor, hot unplug, ARM64, non-96-DPI desktop, alternative Win11 build or genuine urgency
request was exercised live. Unit coverage is not a substitute for those desktop checks. Remaining
compositor/style/input gaps are listed per item above and as partial rows in the two matrices.
