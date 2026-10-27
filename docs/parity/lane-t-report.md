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
