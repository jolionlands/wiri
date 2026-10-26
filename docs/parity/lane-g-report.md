# Lane G — strip, widths, floating dialogs and native move/resize (2026-10-25)

Scope: layout matrix §1/§2, the existing floating invariants in §5, and native
caption/edge reconciliation in §15. No focus-layer/workspace/monitor actions,
input hooks, decoration pipeline, animation clock, IPC schema or new dependency.

## Comparison and boundaries

The comparison uses the pinned niri `e19df12cf8` references and behavior recorded
in the two gap matrices and the Phase 1/2 reports. Attempts to fetch the pinned
`scrolling.rs`, `workspace.rs`, `floating.rs` and layout wiki from
raw.githubusercontent.com were refused by the runner auditor; access was requested,
not granted, and not bypassed. Consequently this is **not a fresh source audit**.
Function names, not the original stale line numbers, identify the wiri evidence.

### Strip, width intent, presets and remembered activity

- `Workspace::reveal_with_policy` supports `center-focused-column "never"`,
  `"always"`, `"on-overflow"`, and the independent `always-center-single-column`
  flag. Leading negative offsets let even the first/only column center. Never
  reveals only as far as needed; on-overflow leaves a fully visible target still.
- Columns retain requested proportion or fixed logical window width, separately
  from their resolved tile-pixel cache. Resolution uses the effective output view
  and scale. Opening/closing another column cannot divide that request away;
  minimize restoration and expelling a tile preserve it. The explicitly configured
  legacy equal-sharing mode remains compatible (including the shipped config).
- `layout { default-column-width { proportion 0.5; } }`, fixed and empty forms,
  and mixed `preset-column-widths { fixed 300; proportion 0.5; fixed 800; }`
  are decoded and acted on. Rules override layout defaults. Forward/back cycles
  wrap in configured order; nonpreset widths step to a wider/narrower preset.
- Signed struts compose with gaps using bounded signed arithmetic, avoiding the
  negative-to-u32 empty-view bug. Fractional/logical gap and strut scaling at
  non-100% DPI remains incomplete; these rows stay partial.
- Ordinary admission, admission to an inactive workspace and unfloat insert right
  of that workspace's active column. Column/workspace active-window ids remember
  stacked-tile activity independently of the bounded MRU ring. Removal has a safe
  surviving-tile fallback. No new focus/workspace/monitor action is introduced.
- Animation completion/timing is outside this lane: H40-H44 turn animations off.
  Multi-monitor placement/clipping and actual scale/mode changes were not live-tested.

### Floating dialogs

`should_auto_float` distinguishes owned top-levels and native `#32770` dialogs
from normal top-level applications, child/tool/no-activate/system helpers.
Explicit `open-floating false` suppresses the automatic default. Unless placement
rules override, a dialog follows its owner's workspace/output and centers on its
owner. Non-dialog fixed-size heuristics are still incomplete (row stays partial).
Existing per-workspace float storage, above-tile raising, non-scrolling placement
and rectangle memory were tested rather than rewritten. Dedicated focus-layer
actions remain another lane's work.

### Native caption/edge loops — explicit Windows approximation

`EVENT_SYSTEM_MOVESIZESTART` records the live frame and cancels any queued HWND
animation. Until `MOVESIZEEND`, both tile and float positioning skip that HWND;
application sizing classification is also withheld. No engine frame may take a
held caption/border grab back from the application's native modal loop.

At end, read the live visible frame unconditionally (not the throttled location
cache), invalidate applied-state/animation state, adopt changed width as a fixed
logical column request, and adopt changed stacked-tile height. Siblings retain
the remaining vertical allocation. Floats remember their final rectangle. Destroy
clears an abandoned native loop. **A caption-only tiled move snaps back to the
same slot**: native Windows caption drags do not reorder/insert as niri's compositor
grabs do. This deliberate, documented Windows approximation closes the native
reconciliation row, not the separate interactive-move/drop-target parity row.

## Verification

Required release commands, each with `RUSTFLAGS=-D warnings`:

- `cargo test --release -j 2`: 1,008 library + 7 ctl tests passed; one doc test ignored.
- `cargo check --release --all-targets -j 2`: passed.
- `cargo build --release -j 2`: passed.
- `C:\wiri-dev\fmt.cmd`: used before each commit (CI stable rustfmt).

Pure regressions cover centering policies and empty/single strips, mixed preset
cycling and wrap, right-of-active middle/end/empty insertion, focus memory beyond MRU
capacity, minimize/expel width intent, dialog classification, and native loop/cache/
stack/float cleanup behavior. Review follow-ups add: width intent through
another-workspace minimize/restore and workspace/monitor transfer (source focus and
destination scale included); a 5% request staying 5% after a pixel minimum rather than
being re-derived from the clamped cache; `center-focused-column` on the effective
output even for the first/last column under `never`; inactive admission and
`open-focused false` restoring remembered column/tile focus; unsorted float presets
cycling in configured order with manual width changes discarding that state; and legacy
`default-width` rules outranking the layout default. DPI/multiple-output behavior is
synthetic-unit-tested only; **the laptop's display settings are never changed**.

The dot-sourced `scripts/live-phases/lane-g.ps1` contributes 23 boolean H40-H44
gates under the harness's global desktop lock. Expectations derive from `$swa`
and current DPI, and window-list reads use `wiri-ctl --json window-list` (not
niri-shaped `windows`). It refuses a foreign daemon, retains owned process
handles, verifies the connected server PID before Quit, releases its injected
mouse button in finally, and closes only its own probes. Evidence:

- `.live/summary.json`, `.live/lane-g/values.json`, per-step `g-*.txt`;
- `h40-*.png`, `h43-floats.png`;
- `h44-caption-held.png`, `h44-edge-held.png`, `h44-edge-drop.png`.

H44 uses **real caption and bottom-right border mouse drags**, observes actual
`WM_ENTERSIZEMOVE`/`WM_EXITSIZEMOVE` and moving/sizing messages in the probe, requests
an idempotent `set-workspace-layout scrolling` relayout while the button is still
held, then checks width/height and sibling geometry after release and another
relayout. This is an existing IPC command, not a new refresh protocol. It does not merely inject
WinEvents or call the reconciliation methods directly. Because Windows may show only a
drag outline until release, the held-loop gates read the probe's proposed
`WM_MOVING`/`WM_SIZING` frame (converted with measured visible-frame insets), require
no new `WM_WINDOWPOSCHANGING` after the relayout, and compare the post-refresh frame
against that proposal rather than a visible frame that never advanced. Live
`full-window-dragging` is recorded for interpretation; the gates do not assume one
display mode.

Live result: the final locked run (2026-10-25 21:12–21:21, `DONE` written) passed
all 347 boolean gates with no failures; `hidden_json_present_after_restart` was the
only intentionally false value and no other gate failed. All 23 H40–H44 gates are
true, including real held caption/border drags, the no-placement-after-relayout
check, post-drop width/height/sibling reconciliation, and the idempotent second
relayout. The probe measured full-window dragging disabled (`full-window-dragging 0`
in `lane-g/values.json`), which is why the outline-aware proposal check was needed.
The same run covered the existing regression suite, H4/H9 included. Evidence in
`.live/summary.json`, `.live/lane-g/values.json`, `g-*.txt`, `h40-*.png`,
`h43-floats.png`, `h44-caption-held.png`, `h44-edge-held.png`, `h44-edge-drop.png`.
Idle resource endpoints stayed informational (`p_*`); no new timer, thread or resting
window was added by this lane.

## Cost and remaining work

No crate, idle timer, polling loop, always-on thread or resting HWND was added.
Native-loop state exists only for active grabs; strip math runs on layout/focus
events, not on a new frame clock. The existing optional animation pipeline is not
replaced. The unchanged perf phase records idle and resource endpoints as `p_*`
information, not load/stress/soak testing.

Remaining: focus-layer actions, compositor-style native drop reorder, fixed-size
non-dialog heuristics, fractional logical gaps/struts, per-named-workspace layout
overrides, and a fresh upstream-source audit once access is granted. None is
silently claimed by a done row.
