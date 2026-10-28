# Lane V — overview and tabs

Branch `meylin/sa-45892-6b96a`, base `c2bb1ec`; ported onto the integrated head
`e7ced50` (lanes K, W, F2, T2 and FK) as `wip/sonnet-v` — see "Integration port" at
the end. Compared directly with pinned niri `e19df12cf8` wiki Overview, Miscellaneous,
Gestures and config source. **Live status: the first complete official cycle has run.** The 2026-10-27 cycle at
`cc3920d` passed **761/761 enforced gates, 0 failed** (88 information entries), which is
the cycle this lane was waiting for. Exactly one row moved to `done`
(`set-column-display normal or tabbed`, whose only stated gap was live verification); every
other row keeps `partial` for the Windows approximation or the unit-tested-only surface it
documents. The per-gate H62-H64 results are not individually transcribed here: only the
cycle total and the H65 tab set were handed to this documentation lane.

## Item 1: overview config and camera

`config/overview_config.rs`, `decode/overview_decode.rs`, `engine/overview.rs`
and `overlay/overview_appearance.rs` implement niri's defaults and options.
The DWM camera uses the existing Phase 2B analytic timeline/notifier and frame
worker, not a new timer/thread. Opening and closing preserve source HWND sizes;
close retires the host asynchronously after the last frame. Reversing a spring
preserves position and velocity. Global off/slowdown and `overview-open-close`
blocks apply. The overview spring defaults on independently of wiri's legacy
opt-in HWND geometry motion.

The overview host is necessarily opaque for foreign DWM thumbnails. Alpha
backdrop colours composite against black, not a compositor wallpaper layer.
Workspace shadows reuse the software gaussian renderer, scaled with workspaces.
The desktop shell/appbars do not become layer-shell surfaces.

Unit coverage: typed defaults/ranges/all fields, colour forms, shadow pixels/off,
negative-origin scaling, active-only clock and interrupted close/retirement.
Release unit run after item 1: 1257 library + 14 CLI tests pass; one doctest ignored.

## Item 2: overview pointer and normal actions

The session host captures complete pointer pairs with an 8 px drag threshold.
Drops target an existing workspace or a new one above/between/below; an insertion adds
a fresh stable workspace and moves only the visible order (names, IPC ids, floating
homes and minimized slots stay with their workspaces). A 350 ms
one-shot dwell switches the camera during a drag. Wheel deltas accumulate at 120;
right-drag/horizontal wheel scroll the normal workspace view at the camera scale.
The drag image remains anchored to the pointer when the workspace camera changes.
No idle dwell timer is armed. Pointer-only updates reuse the backdrop raster.

Normal dispatch remains active, with overview-aware workspace switches and source
refresh/retirement after model changes. Floating previews are included. Existing
source HWNDs retain their normal sizes; exiting restores workspace/tab visibility.
Pure tests cover gap targeting, click/drag cancellation, anchored camera changes,
wheel accumulation, scrolling, insertion/order/names and opening/closing sources.
H62/H63 exercise camera and pointer messages on the owned host, source lifecycle
and guarded normal close bindings; no foreign app is clicked or closed.

## Item 3: event-driven hot corners

`input/hot_corner.rs` publishes coalesced coordinates/travel generations from the
existing `WH_MOUSE_LL` callback, then does all policy/engine work on its existing
message thread. Exact physical `rcMonitor` pixels (including negative origins),
not work-area or logical coordinates, select the enabled corners. The default is
top-left; explicit names replace it. Global and per-output blocks inherit/replace
as pinned niri does. A 120 ms one-shot dwell toggles once per entry, is consumed
before action, and cannot reopen after an IPC close while the cursor stays parked.
Reload/hotplug park rather than synthesizing entry. Modal pick/screenshot/recent
and window grabs suppress activation. No idle timer or additional thread exists.

Pure tests cover config, physical corners, negative origins, coalesced leave/return,
cancellation, suppression and parked reload. The isolated item-3 warning-denied
release run passed 1281 library + 14 CLI tests (one doctest ignored).
H64 uses physical cursor movements
under the harness lock, restores the cursor/config and only stops its retained
process. Multi-display isolation is unit-tested only on the one-display laptop.

## Item 4: tab display, input and sizing

`set-column-display "normal"|"tabbed"` uses the shared strict parser for KDL binds,
ctl/direct/msg actions and typed IPC. Typed `Normal`/`Tabbed` matches pinned niri's
serialized `ColumnDisplay` enum. Selecting the current mode is a no-op, including
tab selection/focus. `decor/spec.rs` shares indicator geometry between paint and
hit-test: pinned-niri rounded extents, total-proportion length, signed gap,
per-tab gap, corner radius, positions and colour/gradient states. Transparent
margins/gaps pass through; rounded cutouts remain rectangular niri hit targets.
The existing message thread queues and revalidates a selection, shows the hidden
tab before focusing it and does not activate the `NOACTIVATE` host.

Fullscreen and maximize-to-edges apply to the current tabbed column. Transitions
are recorded for all members before native styling; hidden siblings defer style
changes until selected, with asynchronous frame recalculation and a fresh settle
interval. Restoration preserves current visibility. A hung selected HWND's
`SetWindowLongW` remains the pre-existing Windows limitation, not a claim of fully
nonblocking style calls. Column/slot provenance keeps forced-width restoration
through admission, close, minimize and split, without undoing a manually maximized
destination. Indicators
and their reservation disappear until exit from either tab. Maximize-column is
separate and retains indicators. Stacked/floating windows keep per-window sizing.

Pure regressions cover bind/CLI/wire/dispatch idempotence, hit/paint/config geometry,
sizing inheritance/removal/minimize/reentrancy, suppressed reservation and hidden
visibility. H65 covers owned hidden-tab focus, four positions, reload/colours/
gradients, signed gap, suppression and single-tab settings. Urgent paint remains
unit-tested only; no foreign application's urgency is induced.

## Validation ledger

Warning-denied `cargo build --release -j 2`, `cargo test --release -j 2` and
`cargo check --release --all-targets -j 2` pass after item 4: **1324 library +
15 CLI tests**, one ignored doctest. Required formatter and the four additive
PowerShell parser checks pass. Locked full desktop results will be appended after
all four items. H62-H65 are
Lane V gates; no existing assertion is weakened. Until that run, desktop checks
are planned, not claimed passed. No load/stress/soak test or display/system-wide
settings mutation is part of this lane.

## Idle cost and boundaries

No new idle timer, polling loop, resident thread or resting HWND. Overview host,
DWM registrations and raster buffers exist only for a session; frame work only
while a camera timeline runs. The four-finger swipe and promoted-touch handling
added by the integration port are event-driven too: the swipe is recognized inside the
existing touchpad Raw Input callback, and the touch long press is a one-shot 500 ms
timer that exists only while a touch is held on the host.

## Integration port onto `e7ced50` (lanes K, W, F2, T2, FK)

The five original commits were cherry-picked in order with their subjects, then the
unverified WIP commit was finished. Overlap decisions, all keeping both sides:

- **F2 (stable workspaces, structured actions).** Workspaces are objects with a
  session-stable IPC id and a name; the visible order is separate from the local key.
  Dropping a window on a gap therefore *adds* a fresh workspace and moves only the order
  (`Monitor::{next_workspace_id, insert_workspace, move_workspace_to_position}`); the
  rotation and tie-swapping of the original port are gone, and the stale rotation test
  became an identity test (names, IPC ids and the active key stay with their workspace).
  The overview paints each workspace's name or "Workspace N" by visible position and
  publishes its IPC id (`WiriOverviewWorkspace<n>IpcId`, `…ActiveWorkspaceIpcId`).
  H63's insertion gates compare the id `windows` reports with that property instead of
  the old composite-id low bits.
- **K (transient compositor).** Overview and compositor never share a frame: K's
  effects are disabled while `is_overview()`, `enter_workspace_overview` cancels any
  running transition, and an overview workspace switch is handled before K's composited
  one. `has_active_animations` and the per-frame tick include both; nothing is created
  at rest.
- **W (rule effects, decor).** `Gradient` lives in `decor/gradient.rs` with space/hue,
  `RRect::sdf` is W's general corner-square distance (it covers the unequal-radius case
  Lane V had patched), tab paints still honour per-window `tab-indicator` rule effects,
  and V's `fill_relative` is retained for tab gradients.
- **T2 (gestures).** One `GesturesConfig` now holds the drag-and-drop settings and the
  global `hot_corners`; a block holding only `hot-corners` configures the overview and
  does *not* start the drag-capture adapter (an empty block or a `dnd-edge-*` block
  still does). The LL hook publishes the cursor to `hot_corner` before any gesture
  early return, and hot corners are suppressed by `is_grab_active` (which includes the
  gesture and pen sessions).
- **FK (H17).** The compositor-options fixture no longer expects a warning for
  `overview-open-close`; a separate fixture accepts the camera block.

### Finishing the work

- Native drag-and-drop in the overview hit-tested a hard-coded half-scale stack. It now
  asks the engine for the same stack the host paints (`overview_workspace_stack`), so
  the configured zoom, the 50 px gap scaled by the zoom and the camera animation all
  agree between paint, pointer and DnD. Only the output the camera opened over
  publishes geometry to the single host.
- `close-overview` and Escape no longer reverse a camera that is already closing (they
  took the toggle path); only a toggle reverses a close and `open-overview` reverses it
  explicitly.
- **Four-finger swipe (new).** With the Precision Touchpad opt-in, a completed
  four-finger vertical swipe (centroid travel of at least 45 pad units of 300 and 1.5x
  more vertical than horizontal) opens (up) or closes (down) the overview once per
  touch. It is a discrete trigger that plays the reversible spring; the camera does not
  scrub with the fingers. A fourth finger still cancels the three-finger swipe. The
  operator must set Windows four-finger Swipes to Nothing; no physical touchpad was used.
- **Touch overview (new).** The primary finger Windows promotes to mouse input pans the
  overview (horizontal scroll; a workspace per 150 px vertically) and a tap still
  selects; resting 500 ms on a window picks it up for the existing drop path. Windows
  delivers only the primary contact, so there is no multi-finger or kinetic scrolling,
  and no touchscreen was used.
- H64 moves the physical pointer with absolute, motion-only `SendInput` (the original
  `Cursor.Position` call can warp without a low-level hook notification), restoring the
  cursor and config afterwards.
- `set-column-display` was already complete through binds, ctl, direct/msg actions and
  typed IPC; the integration kept its tests and H65, and the matrix row is `partial`
  only because nothing here was live-verified; the 2026-10-27 official cycle closed that
gap, so the row is now `done`.

### Not done / limits

- Four-finger and touch gestures are unit-tested only; their rows stay `partial`.
- Multi-display overview remains one session host on the focused output (unit-tested).
- Workspace-shadow alpha composites over the opaque host's backdrop colour, not a
  wallpaper layer; layer-shell zooming is N/A.
- Every lane V gate ran in the 2026-10-27 official cycle at `cc3920d`, which passed
  761/761 enforced gates with 0 failures; H65 is named in that record alongside the h78
  cursor-hiding and h31 focus-history sets. The swipe, touch and multi-display limits above
  are still unit-tested only and their rows stay `partial`.

### Integration validation ledger

On `wip/sonnet-v` (tridaris, `RUSTFLAGS=-D warnings`, `-j 2`): `cargo fmt` clean,
`cargo check --release --all-targets` clean, `cargo test --release --no-fail-fast`:
**1565 library + 15 CLI** pass, one doctest ignored. The one library test excluded
is `hooks::spawner::tests::test_capture_screenshot_to_file_bmp_signature`, which fails
with `BitBlt failed` in the non-interactive SSH session on tridaris (it needs a real
desktop; the file is untouched by this lane). All changed PowerShell files parse with
zero errors. No live harness, wiri.exe or desktop test was run.
