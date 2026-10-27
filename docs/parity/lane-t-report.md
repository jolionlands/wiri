# Lane T — gestures and interactive pointer operations

Branch `meylin/sa-a9a93-6b957`, base `fb34b900`. Reference: pinned niri
`e19df12cf8` Gestures, Configuration:-Gestures, Configuration:-Input,
`spatial_movement_grab.rs`, `swipe_tracker.rs`, `rubber_band.rs`, and layout
scrolling gesture code (freshly read through the source reader).

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
one ignored doc example. Final commands and desktop results will be recorded
below; references to H58-H61 in the matrix identify the acceptance scripts,
not a claim of a completed run before that evidence is recorded.

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

Remaining approximation: tiled drops join an existing column; arbitrary new
column insertion, cross-output drops and niri's insertion compositor are not
implemented here. The move row stays partial. H59 covers actual edge actions
and resize persistence; final live evidence is recorded below.

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
was exercised by automation; H61 verifies configured-only registration, HWND
creation and removal, not hardware recognition. Devices that do not expose
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
performed** by this automated lane: unit-only evidence is explicit in H60.
All DnD rows remain partial, not mislabelled as live-verified OLE parity.
