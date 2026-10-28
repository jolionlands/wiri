# Lane T — gestures and interactive pointer operations

Original branch `meylin/sa-a9a93-6b957`, base `fb34b900`; ported in order onto
integrated head `edb460a` on `meylin/sa-be85a-6b9ad`. Reference: pinned niri
`e19df12cf8` Gestures, Configuration:-Gestures, Configuration:-Input,
`spatial_movement_grab.rs`, `swipe_tracker.rs`, `rubber_band.rs`, and layout
scrolling gesture code (freshly read through the source reader).

**Live status: pending lead integration cycle.** Lane T behavior has only
pure, recorded-frame or synthetic unit-test evidence; no H58-H61 gate result
is claimed. The acceptance scripts are PowerShell-parse-checked only. The
lead will run the official integrated desktop cycle; this follow-up did not
run the harness. Cost statements below describe implementation structure,
not measured live idle/performance results.

## 1. Spatial mouse gestures

Configured Mod+middle starts after explicit button binds. An 8 px direction
threshold locks the axis. Horizontal camera displacement follows the cursor
1:1 (the sign is opposite the scroll offset); constrained real column widths
supply left/right or centered snap points. Vertical motion follows the cursor,
projects into the monitor's ordered workspace list, clamps to its endpoints,
and switches on release. Modifier-first release still consumes the paired up.
Wheel input retains Lane B's configurable niri binds, signed fractional tick
accumulation and per-axis suppression: there is no hard-coded wheel shortcut.

The 150 ms tracker includes idle time at release. Projection uses niri's
0.997 deceleration; the selected snap is reached by Phase 2B's existing
analytic spring, seeded with measured gesture velocity (including the exact
snap/nonzero-velocity case). Rational endpoint resistance and its derivative
bound displacement and velocity. Configured animations off/easing/slowdown
remain authoritative. Direct manipulation bypasses HWND easing; no frame
clock runs until release, and that is the existing active-only clock.

**Windows approximation:** while a vertical mouse gesture is held, wiri moves
only the active native HWND scene. The incoming workspace is shown on release,
not composited with the outgoing workspace under a cropped viewport. Section
15's workspace-gesture row remains partial rather than claiming this matches
niri's two-scene compositor. Physical multi-output/DPI behavior is not verified.

Pure regressions cover threshold/axis lock, bounded history, backwards time,
release pauses, projection, rubber-band symmetry/derivative, actual-width and
centered/narrow strip snaps, camera displacement and spring velocity/termination.
Initial required release tests: **1206 library + 14 CLI passed**, 0 failures;
one ignored doc example. Latest aggregate release results are in section 5.
H58-H61 in the matrix identify planned, parse-checked acceptance scripts only;
live status is **pending lead integration cycle**.

## Cost

No dependency, idle timer, polling loop, always-on thread or resting HWND is
added for mouse gestures. Work runs through `crate::input::dispatch` on the
existing hotkey thread. The existing baseline mouse-hook thread is retained,
not misreported as removed. No display/input system settings are changed.

## 2. Edge actions and interactive grabs

Top/bottom edge doubles (native left or configured Mod+right) reset height;
left/right doubles toggle full width. System double-click time and rectangle
are read, never changed. Explicit pointer binds still win. Pure tests cover
edge/window/button/time/distance discrimination. Native-loop exclusion prevents
an unrelated refresh from fighting the held preview. Resize release uses Lane
G's final-rect adapter (logical column width, stacked preferred height, float
memory); move release invalidates the old HWND target and selects the actual
variable-width, scrolled destination column rather than dividing by the source
width. Right-click while moving toggles floating/tiling. Cancellation restores
the original geometry and removes exclusion.

Integrated port: W's `grab_for_cancel` / `cancelled_grab_cleanup` and shared
Move disposition/token remain the single mouse-grab cancellation path. Move
cancellation keeps W's source restoration and restores the initial floating
rectangle; Resize cancellation restores its initial rectangle through native
reconciliation. Resize start owns native-loop exclusion. Right-click floating
toggles validate the same Move token before execution (an executed layer toggle
is an explicit action, not undone by later cancellation).

Mouse tiled drops retain W's transactional `finish_window_insert` destination,
including source-removed geometry, new columns, vertical slots and cross-output
insertion. Promoted touch/pen drops now use that same destination rather than
T's old nearest cached-column heuristic. W alone owns the insert hint; no second
hint/cancel owner was added. T's variable-width/scrolled destination regression
was adapted to W's selector; H59 now aims at the destination's top tile slot
rather than a centre point that W may correctly treat as a new-column request.
Floating drops retain the existing source-output clamping. Niri's insertion
compositor remains an approximation. H59 is a planned probe for edge actions
and resize persistence; live status is **pending lead integration cycle**.

## 3. Precision Touchpad

Opt in with niri syntax `input { touchpad { } }`; absence or
`input { touchpad { off; } }` leaves no Raw Input registration/window. Unlike
niri's libinput default, this Windows opt-in is deliberate. The HID Digitizers
page 0x0D / Touch Pad usage 0x05 registers RIDEV_INPUTSINK (plus DEVNOTIFY) on
`WiriTouchpadRawInput`, a message-only HWND on the existing hotkey thread.
No process-global input settings are written. Descriptor-derived contact IDs,
X/Y ranges, tip/confidence bits and Contact Count/Scan Time support parallel
and serial/hybrid reports. Buffers, descriptor caches and contact counts are
bounded; removal cancels the active device. Three stable IDs produce centroid
deltas; replacement rebases, a fourth finger cancels until all lift. Deltas
feed the shared axis lock, 150ms velocity/projection, snapping and active-only
spring. Pure recorded-frame tests cover these cases, serial assembly, release
pause and opt-in/off decoding.

**Operator prerequisite (not performed by this lane):** Windows Settings →
Bluetooth & devices → Touchpad → Three-finger gestures → Swipes: **Nothing**.
Windows may otherwise also act on the same swipe. Raw Input does not suppress
the shell's gestures. No physical three-finger/swipe or real device descriptor
was exercised by automation. H61 is a planned probe for configured-only
registration, HWND creation and removal, not hardware recognition; its live
status is **pending lead integration cycle**. Devices that do not expose
raw PTP reports cannot use this path. Two-finger pinch/hold remain untouched.

## 4. Drag-and-drop (Windows approximation)

Presence of `gestures { }` enables the capture adapter. niri
`dnd-edge-view-scroll { trigger-width 30; delay-ms 100; max-speed 1500; }`
and `dnd-edge-workspace-switch { trigger-height 50; delay-ms 100; max-speed 1500; }`
are parsed/validated; zero trigger/speed disables an axis. The existing LL hook
observes a held left button, >=8 px drag and capture on the original source
GUI thread in a foreign process. Wiri move/resize/spatial grabs are excluded.
This is a capture heuristic, not a claim that every captured application
selection is OLE, or that every OLE implementation captures on that thread.
No COM injection/global OLE hook is installed.

View speed increases linearly with edge penetration after the configured
delay; it is DPI-scaled and bounded at both strip ends. Overview-only vertical
edge accumulation switches ordered workspaces at one-screen thresholds
(1500 units/s = one workspace/s), not niri's continuous two-scene composition.
Outside edge zones, 500 ms stable hover activates a managed window (raises
floats) or selects a workspace in the overview. One-shot hover/delay timers
are killed before processing; 16 ms frames exist only while an edge actually
scrolls, revalidating held capture, and stop on release/cancel/bounds/reload.
No idle timer/thread. A message-only HWND exists only with the gestures block.

Pure tests cover capture/held/grab/distance discrimination, speed/scale, delay
and direction reset, and config/range validation. **No real OLE drag was
performed** by this automated lane. H60 plans configured-only HWND lifecycle
checks and labels drag behavior unit-only; live status is **pending lead
integration cycle**. All DnD rows remain partial, not live-verified OLE parity.

## 5. Promoted touch and tablet pen (synthetic-only)

`input { touch {} }` and `input { tablet {} }` independently opt in; each accepts
niri's `off`. The existing LL hook recognizes `MI_WP_SIGNATURE` in dwExtraInfo,
the touch bit, and the reported seven-bit contact ID. Ordinary mouse input is
unchanged. Configured Mod claims a managed, non-fullscreen primary down. After
8 px, a tiled horizontal drag scrolls in pixels through the shared kinetics;
vertical drags and floating drags use move/drop with native-loop exclusion.
A distinct reported secondary touch down toggles floating once per tap. Motion
passes so Windows advances the cursor; claimed button releases stay guarded
across modifier-first release and disable/reload. Generation checks reject
obsolete queued work. No new hook, HWND, thread, dependency or timer is added.

**Limits:** Windows commonly promotes only the primary contact to mouse input.
The adapter cannot discover a secondary finger the OS does not report, or take
over application-owned WM_POINTER streams. No transparent capture overlay or
input forwarding is installed. Mapping/calibration/pressure remain Windows-owned.
Five pure tests cover signature/type discrimination, primary/secondary pairing,
release order, pen isolation, axis/mode selection and independent opt-in/off.
There was no physical touchscreen or tablet pen test and no claim of hardware
support. The planned H61 evidence field labels touch/pen synthetic-only; it
does not exercise promoted-event behavior live. Live status is **pending lead
integration cycle**. The two rows stay partial. Item 5 required release tests:
**1222 library + 14 CLI passed**, zero failures, one ignored doc example;
warnings-denied all-target check passed.

## T2 integrated port verification

On `edb460a` plus all seven ordered ports, with `RUSTFLAGS=-D warnings` and
`-j 2`: `cargo build --release`, `cargo check --release --all-targets` and
`cargo test --release` all passed. Tests: **1480 library + 14 CLI**, zero
failures; main has no tests and one doc example remains ignored. Both changed
PowerShell files (`scripts/live-test.ps1`, `scripts/live-phases/lane-t.ps1`)
parsed with **zero errors**. No live harness, physical input-device test or
shared-desktop probe was run. H58-H61 remain for the lead's official cycle.
The matrix's existing aggregate totals are deliberately unchanged.

Reapplication used local text three-way application because runner policy
refused `git cherry-pick`. Overlap resolutions:

- `CHANGELOG.md`: retain integrated entries and all T entries.
- `src/input/low_level_hook.rs`: screenshot pass-through before captured middle
  and promoted touch/pen processing; separate rule-wheel and Mod-middle blocks;
  W cancellation/start/drop ownership retained as described in section 2.
- `src/layout/engine.rs`: retain all integrated modules/fields and add T's
  gestures, interactive and DnD modules/state.
- `scripts/live-test.ps1`: retain every phase and add T immediately after F.

Config, message-loop and action additions applied alongside the integrated
System/LayoutFocus/WorkspaceAction/MonitorAction paths. Nothing from T was
omitted; its old nearest-column drop is intentionally superseded by W, and its
H59 target was adjusted accordingly. Physical hardware, OLE and compositor
limitations above remain; no new live verification is implied.
