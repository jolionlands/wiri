# Lane V — overview and tabs

Branch `meylin/sa-45892-6b96a`, base `c2bb1ec`. Compared directly with pinned
niri `e19df12cf8` wiki Overview, Miscellaneous, Gestures and config source.

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
Drops target an existing workspace or a new one above/between/below; insertion
rotates the normal model with names, floating homes and minimized slots. A 350 ms
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

## Validation ledger

Full warning-denied release build/test/check, prescribed formatter, and the locked
full desktop harness results will be appended after all four items. H62-H65 are
Lane V gates; no existing assertion is weakened. Until that run, desktop checks
are planned, not claimed passed. No load/stress/soak test or display/system-wide
settings mutation is part of this lane.

## Idle cost and boundaries

No new idle timer, polling loop, resident thread or resting HWND. Overview host,
DWM registrations and raster buffers exist only for a session; frame work only
while a camera timeline runs. The four-finger overview gesture belongs to the
gestures lane, not this change; the engine camera is its integration hook.
Touchscreen long-press/one-finger input remains outside this mouse/keyboard lane.
