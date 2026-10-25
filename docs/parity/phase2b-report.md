# Phase 2B Lane A: look and feel

## A2 integrated-branch validation

Integration branch: `meylin/sa-f9cbe-6b716`, starting at `df229cd`. The runner refuses
cherry-pick, so the six requested source changes are ported as explicit worktree edits in
source order. Existing LF in the shared engine is retained; no source-branch EOL churn.
Recent keyboard events now use Lane B's existing ordered `Work` queue on the hotkey
message thread, not the old action channel/relay or dedicated keyboard-hook thread.
Primary keyboard binding/fallback/repeat state, pointer grabs, focus dwell/warp and Lane D
output/screenshot setup are retained. Modal input advances physical keyboard state without
firing ordinary/release actions. No dependency changes or new idle resources.

Release build of the first five integration parts passes with `RUSTFLAGS=-D warnings`.
The source lane's 102 H16–H21 checks are now included before the unchanged Lane B/D/perf
phases. The only older harness change is a real classification bug: an intentional `WiriDecor`
counts as passive only with every required extended style and no APPWINDOW flag. H16
independently tests those properties. No existing assertion or threshold is weakened.
Integrated unit/all-target/live results will be recorded after the complete locked run.
Everything below is the **source lane's historical report**, not a claim that its desktop
run was performed on this integrated branch.

## Source-lane report

Branch `meylin/sa-c6e38-6b646`, based on the unfinished decoration work in `1158e31`.
This is the look-and-feel lane, not a claim that every Phase 2/niri compositor feature is done.
The two matrices record remaining partial rows. Their headline counts remain historical Phase 2A counts.
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
