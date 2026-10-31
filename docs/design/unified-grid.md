# wiri extension: the unified XY grid

Status: implemented (opt-in, default off). This document is the contract; keep it in
sync with the code.

## The model

wiri is a scrollable-tiling manager modelled on niri. niri's desktop is a **stack of
workspaces**, and each workspace row has its **own** horizontal strip with its **own**
horizontal camera (`scroll_offset.x` in `src/layout/workspace.rs`). Moving a row up or
down in niri therefore lands on whatever column that row happened to be showing.

The unified XY grid treats the whole desktop as **one 2D grid**:

* **x** — each workspace's endless strip of columns, one strip per row.
* **y** — the monitor's workspaces, stacked vertically.

In unified mode all rows of a monitor share **one horizontal camera**. Columns keep
their own widths — the grid is *not* a table of equal-width cells. Because both axes
are meaningful at the same time, the grid can be traversed **diagonally**.

The mode is **opt-in** (`layout { unified-grid }`) and **off by default**. With it off,
every code path in this document is bypassed and behaviour is bit-identical to niri
parity, which is what the rest of `docs/parity/` measures.

## Configuration

```kdl
layout {
    // Opt in. `unified-grid true` / `unified-grid false` are the explicit spellings;
    // the bare flag is `true`, like niri's other flag options.
    unified-grid
    // How long two arrow presses count as one diagonal chord, in milliseconds.
    // Integer, 0..=1000. 0 disables merging.
    unified-grid-chord-ms 150
}
```

Both keys are also accepted inside a per-output `layout {}` override
(`output "…" { layout { unified-grid } }`), which is how one monitor can run the grid
while another keeps niri's per-row cameras. `unified-grid-chord-ms` defaults to 150.

Decoding: `src/config/decode.rs` (`decode_layout_into`, the `"unified-grid"` and
`"unified-grid-chord-ms"` arms). The partial carries `Option<bool>` / `Option<u32>` so
the per-output fold (`LayoutConfigPartial::apply_to`) keeps the unset-inherit
behaviour every other layout key has. The projected per-output value ends up in
`LayoutConfig::unified_grid` / `LayoutConfig::unified_grid_chord_ms`
(`src/layout/engine.rs`).

## The shared camera

The camera is still stored where niri stores it — in the active workspace's
`scroll_offset.x` — so every existing consumer (`calculate_positions`,
`is_over_gap`, the gestures, the animations, the IPC geometry) is unchanged.

What unified mode changes is **what happens at a row change**: the offset is carried
across instead of being read from the row you land on. `TilingEngine::carry_unified_camera`
does that inside `switch_workspace`. The camera therefore belongs to the monitor, not
to a row:

* Horizontal focus and column moves inside the focused row are untouched: they reveal
  with the existing `Never` / `OnOverflow` / `Always` policy, animate with the existing
  view animation, and clamp against the focused row's own strip, exactly as today.
* A vertical move keeps x. The workspace slide animates only vertically; the x camera
  animation is not restarted, because x did not change.
* Moving to a row that is shorter than the camera offset does not clamp x away: rule 3
  explicitly allows x to sit past a row's ends, and the x-match then picks the nearest
  column by distance.

`TilingEngine::camera_offset(output)` is the read-only hook the overview uses: it answers
"what horizontal offset does this monitor's camera have right now". The overview
(`src/layout/engine/overview.rs`) draws **every** row of a unified monitor through that one
camera, so the columns of different rows line up in x and the zoomed-out view is the same
grid as the normal view; with the mode off it keeps niri's per-row camera unchanged. A row
change inside the overview goes through `unified_switch_workspace`
(`overview_step_row` / `switch_workspace_in_overview`), so it carries the camera and lands
on the x-matched column exactly like a row change in the normal view.

Because the overview pan is the shared `scroll_offset.x`, `focus-column-left/right`, the
diagonal actions, the two-arrow chord and a two-axis scroll all move the one camera in the
zoomed-out view: x and y animate together and every row follows.

## Rule 3 — vertical moves land on the x-matched column

Applies in unified mode to:

| action | wiri variant |
| --- | --- |
| `focus-workspace-up` / `-down` | `Action::FocusWorkspacePrevious` / `FocusWorkspaceNext` |
| `focus-window-or-workspace-up` / `-down` (only when it crosses rows) | `FocusAction::WindowOrWorkspaceUp` / `WindowOrWorkspaceDown` |
| `move-window-to-workspace-up` / `-down` | `WorkspaceAction::MoveWindowRelative` |
| `move-column-to-workspace-up` / `-down` | `Action::MoveColumnToWorkspaceUp` / `Down` |

The **x anchor** is the horizontal centre of the current focus, in strip-content
coordinates. Because the camera is shared, strip-content x and on-screen x differ by
the same offset in every row, so matching by strip x is the same as matching by screen
x.

The destination is the column of the target row **whose strip x-range contains the
anchor**. If the anchor falls in a gap between columns, in the leading/trailing margin,
or past the row's ends, the **nearest column by horizontal distance** wins. Ties go to
the earlier column. An empty target row simply becomes active and keeps the camera.

A **moved** window or column is inserted into the target row at the column index the
anchor falls into, keeping its own width. `move-window-…` moves the focused tile;
`move-column-…` moves the whole column with all its tiles.

Named and indexed workspace actions (`focus-workspace 3`, `focus-workspace "web"`) keep
their current semantics — they still resolve the workspace by index/name — but they land
on the x-matched column instead of the row's remembered focus.

Nothing here changes when the mode is off.

## Rule 4 — diagonal actions

New wiri actions (not part of niri's vocabulary; see
`docs/parity/action-census.md`):

| action | meaning (unified mode on) |
| --- | --- |
| `focus-up-left` | one row up (rule 3), then one column left from the x-matched column |
| `focus-up-right` | one row up, then one column right |
| `focus-down-left` | one row down, then one column left |
| `focus-down-right` | one row down, then one column right |
| `move-window-up-left` | move the focused window one row up and one column position left |
| `move-window-up-right` | move the focused window one row up and one column position right |
| `move-window-down-left` | move the focused window one row down and one column position left |
| `move-window-down-right` | move the focused window one row down and one column position right |

They are usable from `binds {}`, `wiri-ctl action <name>` and typed IPC
(`{"action": "focus-up-right"}` / `RunAction`).

The diagonal step is **atomic**: the row change and the column change are resolved
before anything is applied, and the camera animation for x is retargeted once to the
final destination (`AnimationManager::animate_with_kind` retargets the existing
`ScrollX` timeline) while the row change runs its normal vertical slide. That is one
camera motion along the diagonal, not two sequential moves.

`move-window-*` keeps focus on the moved window.

**With the mode off** every one of the eight falls back to the sequential
vertical-then-horizontal niri actions, which is what a user of the parity behaviour
expects:

* `focus-up-left` → `focus-window-or-workspace-up` then `focus-column-left`
* `move-window-up-left` → `move-window-to-workspace-up` then `move-column-left`

(and so on for the other six).

## Rule 5 — two arrows together

When a bound vertical arrow fires and, **while that key is still held** and within
`unified-grid-chord-ms` measured **from the input events' own timestamps**, the
perpendicular arrow of the same family fires, the pair becomes the diagonal action:

* focus family: `focus-window-or-workspace-up` / `-down`, `focus-workspace-up` / `-down`
  merge with `focus-column-left` / `focus-column-right`
* move family: `move-column-to-workspace-up` / `-down`, `move-window-to-workspace-up` /
  `-down` merge with `move-column-left` / `move-column-right`

Implementation:

* The **first** arrow press is dispatched immediately — nothing waits, so a single
  arrow costs exactly what it costs today. `KeyboardEngine::event` returns the action on
  the same call that observes the key-down.
* `KeyboardEngine` remembers the last chordable vertical action together with its
  virtual key and the event's own timestamp (`KBDLLHOOKSTRUCT::time`, milliseconds since
  boot). When a later action fires it tests, in this order: same family, perpendicular
  direction, that virtual key still down, and
  `event_ms.wrapping_sub(first_event_ms) <= unified-grid-chord-ms`. Only then is the
  action rewritten into the diagonal one and marked as chorded.
* The layout engine recorded the pre-vertical snapshot (output, row, focused window,
  focused column, camera x) when it applied the vertical action.
  `TilingEngine::apply_grid_action` uses that snapshot for a chorded diagonal, so the
  diagonal target is one row and one column away from where the user started — not one
  row away from where the first arrow already moved them.
* The in-flight camera animation is retargeted, not restarted.

A single arrow press is proven not to wait by
`input::keyboard::tests::a_single_vertical_arrow_dispatches_on_the_first_down`; the merge
and no-merge cases by the `layout::engine::unified_grid` tests.

## Rule 6 — diagonal pan

In normal view, when a scroll input carries **both** axes (precision-touchpad
two-finger scroll, or horizontal tilt together with vertical wheel), unified mode pans
the shared camera along x **and** moves between rows along y as one diagonal motion
instead of two independent ones. The handler is `TilingEngine::unified_grid_pan`; it
runs only when the mode is on and only when the existing view (`ScrollLeft`/`ScrollRight`)
and workspace-scroll handlers would both fire for the two axes.

## The overview

The overview is the same grid seen through `overview { zoom }` (see
`src/layout/overview_geometry.rs`), so with the mode on `workspace_overview_scene` draws
every row of the monitor through `camera_offset` — the rows line up in x, and a row that is
shorter than the camera offset simply has empty space there (rule 3's "past the row's
ends"). With the mode off each row keeps its own camera and nothing changes.

* **Row change**: `switch_workspace_in_overview` and `overview_step_row` (the overview's
  next/previous-workspace camera step, driven by a vertical wheel, a touch drag or an
  action) go through `unified_switch_workspace`, so the camera moves y and keeps x.
* **Diagonal actions**: `apply_grid_action` resolves the row and column before anything
  moves, so `focus-up-left` … `move-window-down-right` pan the zoomed-out camera in x and
  y from one action; the vertical camera (`OverviewCamera::pan`) and the x camera
  (`queue_view`) animate together.
* **Two arrows together**: unchanged — the chord produces the diagonal action above.
* **Two axes of a scroll**: the vertical leg steps the row with the camera carried and the
  horizontal leg (`overview_scroll_view`) pans the same camera, so every row follows
  instead of only the active one.
* A **floating** window is drawn at its own screen rect, because floats are not part of the
  grid.

## Rule 7 — IPC

The mode is reported per monitor as `unified_grid` on the `MonitorInfo` reply
(`monitors` / `outputs` requests). Diagonal moves go through the ordinary focus and
workspace paths, so the usual `workspace-activated`, `windows-changed`,
`window-focus-changed` and `workspace-active-window-changed` events are emitted with no
new event type.

## Cost

No timer, thread, polling loop or idle wake-up is added. The chord is a comparison of
two `u32`s and one `Instant` that only happens when an action actually fires; the layout
engine's snapshot is only written by a vertical arrow. With the mode off, the only
added work is one `bool` read on the workspace-switch and vertical-action paths.

## What is not done here

* Multi-monitor: the camera is per monitor by construction, but this lane's live gates
  run on the single-display test machine, so cross-output grid behaviour is covered by
  unit tests only.
* Floating windows are not part of the grid; the diagonal focus keeps the existing
  floating-layer rules, and the overview draws a float at its own screen rect.
