# Lane W — window rules and effects

Pinned upstream: niri `e19df12cf8`, Window-Rules and Layout documentation and
`niri-config/src/{window_rule,appearance}.rs`.

## Item 1 — sparse per-window appearance

Focus-ring/border/shadow/tab paints merge only specified properties; later
matching rules replace only those keys. Ring `on` beats `off`, shadow `off`
beats `on`, and a gradient in the same block beats a colour independent of
text order, matching niri. Tab rules change paints, not column geometry.
Border widths affect admission size, tile inset and native/rule minima.

Geometry accepts one or four CSS-order radii. Decorations use independently
DPI-scaled corners with CSS overlap fitting. Geometry alone never clips the
application. Opt-in clipping uses a pixel-centre HWND region relative to the
observed visible frame, preserving/intersecting the application's original
region and restoring it on rule removal, unmanage and graceful exit. DWM's own
rounding is disabled during clipping. This is not an antialiased compositor
mask: non-client content is clipped, DWM shadows can disappear, separate owned
popups are not clipped, and protected/RTL HWNDs can reject it. An application
can replace its region; native resize or a changed mask reapplies it, not a
polling loop. Failure is logged and not cached as success.

Urgency is driven by shell `HSHELL_FLASH` and WinEvent ALERT; generic
accessibility STATECHANGE is no longer an attention request. Foreground clears
urgency. Sparse effects re-resolve on existing title/focus/floating/urgency/
reload events. Idle cost: no timer, polling loop, thread or new dependency;
only an opt-in per-HWND saved region and changed decoration data.

No compositor blur is claimed. Legacy `blur true` and opt-in DWM backdrop are
compatibility hints; Mica/Acrylic only show through applications which already
provide a transparent client. Arbitrary foreign-HWND xray/noise/saturation and
niri blur passes are N/A without owning or injecting into the compositor.

Validation and desktop gate results will be recorded after the integrated
warnings-denied release suite and locked harness run. Pure tests cover sparse
merge precedence, located decoder errors, corners, region ownership/restoration
and failures. `scripts/live-phases/lane-w.ps1` provides lane-specific evidence;
unexecuted checks are not desktop verification.
