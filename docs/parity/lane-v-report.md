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
