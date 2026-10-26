# Phase 2C — Lane F2: integrated focus, workspaces and monitor actions

Branch `meylin/sa-f82e9-6b8a6`, integrated base `a499d20` (B/D, Phase 2B, G and H).
This ports source lane `meylin/sa-ff818-6b717`'s `29630c1`, `d6e8af4`,
`d8becf7`, `12c1ce9` and `ee7f5b0` without replacing the integrated architecture.
The endpoint/PID-safe performance collector already contains the equivalent of
`12c1ce9`; its `PipeName` / `PipeNameFor` logic is retained unchanged.

Reference: the recorded audits of niri main `e19df12cf8` (v26.04 plus main-only
features). Upstream source/wiki fetches were denied and were not bypassed. The
checked-in matrices, source-lane changes and pure regressions are the reference;
this is not a claim of a fresh upstream-source audit.

## Implementation and integration

- `input/layout_actions.rs` supplies typed focus/workspace/monitor actions and a
  grammar shared by KDL, CLI and H's typed `run_action`. Indices are 1-based and
  clamped; quoted numeric workspace names remain names. KDL uses `focus=`,
  `workspace=` and `debounce-ms=`; CLI/IPC use the corresponding `--` options.
  Malformed/duplicate options and extra arguments are errors. Explicit HWND
  transfers do not activate the addressed window before moving it.
- `engine/action_dispatch.rs` connects the new executors through existing B/2B
  dispatch and H transport, retaining G width intent, selected columns/tiles,
  tab visibility, struts and effective output view. No input relay or second event
  publisher is restored. Revealing/layout submission precedes activation.
- `engine/focus_actions.rs` handles indices, edges, wraps, vertical-then-column
  and tile-then-workspace/monitor fallbacks. Floats use live rectangle distances
  within their workspace/layer; tile indices do not select a float. Workspaces
  remember the last valid non-minimized float independently of G's tiled focus;
  a later genuine tiled selection clears that float marker.
- Previous-window focus uses global native visit history, with a default 750 ms
  debounce and explicit back-and-forth endpoint commits. The existing WinEvent
  callback stamps each foreground occurrence (32-bit timestamp extended against
  uptime plus FIFO sequence). Every occurrence, including stale and unmanaged
  notices, terminates history visits; requested selection never fabricates dwell.
  Native sequence orders same-tick events; explicit commands win same-tick ties.
  A command ahead of the native stream resumes on confirmation, without sleeping
  or retrying on a clock. Stale notices cannot undo live focus, cancel the pending
  command or restamp H's focus timestamp. The handler compares the raw HWND with
  a fresh native foreground query under the lock before live state/broadcasts;
  stale notices also skip foreground admission/minimum-width retries.
- `engine/workspace_actions.rs` separates monitor-local keys, visible order and
  session-stable IPC identity. Names belong to objects, not positions. Transfers
  retain column/tile/float state and width intent. Dynamic unnamed empties are
  maintained without deleting named workspaces. Overview navigation and slide
  direction use visible order rather than local keys.
- `engine/workspace_names.rs` reconciles globally unique configured names after
  output identities are known. Repeated `open-on-output` preferences use the
  existing cached connector/EDID matcher; new names on reload appear at the top.
- `engine/monitor_actions.rs` shares directional/cycle/name targeting across
  focus and window/column/workspace moves. Single-output/missing-target operations
  are harmless no-ops. Workspace moves retain identity and re-home floats and
  minimized windows. Chained unplug migrations follow the live identity instead
  of recreating duplicate objects when an earlier monitor returns.
- H's existing snapshots/events describe these mutations. There is no idle
  snapshot allocation, parallel focus timestamp path or new event stream.

## Verification

With `RUSTFLAGS=-D warnings`, the final source passes **1298 library + 14 CLI
release tests** (one documentation example ignored), release all-target check and
release build. This includes the stale-admission guard and the floating-memory
and action-target fixes described below. Commands use `-j 2`; `C:\wiri-dev\fmt.cmd` supplies
CI's stable rustfmt. Local build/test/check logs are
`.scratch/f2-{build,test,check}.log`. Both edited PowerShell scripts pass syntax
parsing; their latest evidence barriers have not been desktop-revalidated.

The required harness uses the existing machine-wide lock and isolated child
configuration/pipe state. `scripts/live-phases/lane-f.ps1` adds H30–H34 with four
owned forms, checked CLI and typed IPC, native foreground confirmation, geometry,
workspace/window snapshots, event replay, and timing preconditions. Failure or
missing evidence fails closed. H31 dispatches immediately after its short visits;
there is no event-drain sleep masking a queued-native-event race. Existing gates
and their thresholds are retained. Detailed Lane F evidence is in `.live/lane-f`;
`window-list` is wiri's geometry shape, while `windows` is H's niri-compatible shape.

### Saved desktop result — not an acceptance pass

The saved `.live/summary.json` contains **519 pass / 15 fail / 79 informational**
entries. The harness process exited 0, which does **not** override failed gates.
Lane F contributes **28 pass / 14 fail**:

| Phase | Pass | Fail |
| --- | ---: | ---: |
| H30 focus (including owned-process cleanup) | 14 | 0 |
| H31 previous-window history | 5 | 0 |
| H32 workspace transfers | 0 | 8 |
| H33 names/order/floating visibility | 6 | 1 |
| H34 single-display monitor actions | 3 | 5 |

- **H32:** all 32 recorded per-action checks across 16 transfers passed, but the
  fixture contained a foreign terminal (HWND `29692760`, PID `29936`) between the
  owned A/B and C/D pairs. Its admission preceded fixture preparation. That
  invalidated the required two-column fixture and all eight aggregate gates;
  these remain failures, not evidence of accepted transfers. No transfer-product
  change was justified by these artifacts. Fixture evidence and managed/native
  focus preconditions are now recorded more explicitly, still fail-closed.
- **H34:** a foreign window disappeared between actions, invalidating the shared
  baseline. Saved target-family checks record **1 pass / 55 fail**, and harmless
  state checks **9 pass / 55 fail**; topology comparisons passed 57/57 and
  composites 16/16. Retrospective per-action comparisons were diagnostic only,
  not a replacement for the saved failures or acceptance. The five failed
  aggregate gates remain failed. The harness now compares foreign state before
  and after each action and uses cached output IDs/edge overlap for its oracle;
  those harness corrections have not been rerun.
- **H33:** returning to the floating workspace left a tile in the foreground
  instead of float HWND `15731892`. Converting an already-foreground tile into a
  float now records its floating selection immediately, since no new native
  foreground notice need follow. The action-target fallback also recognizes a
  remembered float when native foreground is cleared, rather than classifying
  it as a tile. Three pure regressions pass, but these product fixes are
  **not live-revalidated**.
- The remaining failure is existing **H15 explicit-ID fullscreen**: its requested
  target became fullscreen, but setup had not produced the expected k1 focus
  delta (the stream still named k3), so the exact-one focus assertion failed.
  Root cause remains unresolved; H's deduplication was not weakened. The harness
  now requires native/managed/stream agreement on k1 before the action and saves
  `h15-fullscreen-id.evidence.json`; that new barrier is also unexecuted.

A fresh desktop recheck was prepared but refused by the tool auditor. It was not
retried through another route; the saved `.live` evidence was preserved. Thus
neither the latest harness nor the final floating-memory/target fixes have desktop
acceptance.
The laptop has one display: multi-output adjacency/cycling, transfers, EDID
preference, unplug/reconnect and non-100% DPI behaviour are **synthetic unit-tested
only**. No display settings are changed on the machine.

## Cost, counts and remaining work

No crate, idle timer, polling loop, always-on thread or resting HWND is added.
History/order/name state changes only on existing foreground/action/output events;
workspace identities are allocated at creation. Existing B pointer handling, 2B
motion/UI and G strip policy are not replaced. SW_HIDE remains default; cloaking
stays opt-in. Live resource/idle measurements are informational `p_*` entries, not
new performance acceptance thresholds.

Recounted feature rows after integration (not additional work outside this lane):
layout **124 done / 43 partial / 44 missing / 15 N/A** (226); config/input/IPC
**90 done / 56 partial / 43 missing / 70 N/A** (259). The config summary header is
not a feature row. Historical audit findings/counts remain labelled as historical.

Still open: named declarations with every preferred output absent remain pending,
not realized on a fallback output; niri's nested per-workspace `layout {}` overrides;
`original-output` config; preserving a returning monitor's former enumeration
position; physical multi-monitor verification. Other open matrix rows (including
output/compositor approximations and interactive screenshot flags) are not claimed
by this action lane.
