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

## Item 2 — policy defaults

`default-column-display` is consulted on admission, expel and float-to-tile,
not during ordinary repaint or tab toggling. Explicit normal overrides a global
tabbed default. `default-floating-position x=20 y=30 relative-to="bottom-right"`
uses the work area and logical offsets after size limits; eight anchors and
negative offsets are supported. A remembered floating rectangle wins thereafter.
Startup adoption retains existing geometry; explicit positions beat owner centering.

`scroll-factor` composes with the existing mouse/device factor after pointer
bindings. The existing hook consults a tiny rule-change snapshot, with independent
axis fractions and recipient/factor reset, then tags its SendInput replacement
so it bypasses bindings and scaling. Replay is queued on the existing input pump
only after the hook returns, avoiding original-plus-replacement delivery during
nested SendInput. Zero suppresses application wheel delivery. Queue rejection
passes through the original; total injection failure tries one tagged original
delta, with no retry loop. No engine lock or system setting change is used; UIPI
can reject both injections and Windows wheel recipient routing still applies.
Only physical/ordinary injected wheels are scaled, never Wiri's own replacement.

`on-xdg-activate` is latched at admission (including absence). Actual shell flashes
and ALERT requests implement ignore/set-urgent/focus, with set-urgent as the
platform default. Windows has no Wayland serial provenance and cannot reliably
veto an already-granted foreground. Explicit user/IPC focus is never classified
as an activation request. Thus this is parity for classified Windows attention
requests, not a promise of universal focus-steal prevention. A queued flash
rejected because its window already has foreground also clears the native urgency
registry; otherwise that late flash could swallow the next background request.

The isolated appearance/policy release suite passes 1251 library and 14 CLI
tests, with warnings denied; the integrated suite below supersedes that count.
No new idle resource, thread or timer; snapshots update only on rule/config/window
lifecycle events.

## Item 3 — gradients and insertion hints

Gradient coordinates use CSS angles over either the window or workspace view,
including offset/negative work-area origins. Interpolation follows the pinned
`src/render_helpers/shaders/border.frag`: gamma 2.2 for linear sRGB, premultiplied
rectangular spaces, OKLab conversion and OKLCH's alpha-independent hue with
shorter/longer/increasing/decreasing paths and upstream tie rules. Endpoint
conversions are prepared once per changed paint, not per pixel. Invalid/trailing
`in=` tokens are located configuration errors, not silent sRGB fallbacks.

General Mod+LMB tiled moves now share one pure nearest-column-boundary versus
vertical-tile-slot calculation between the lazy preview and release. The source
is removed only in a planning copy until commit. Calculated logical strip bounds
avoid parked HWNDs and stale hidden-tab caches. Successful drops preserve tile
policy, normalize sizing, apply new-column defaults only to new columns and
reveal both affected outputs. Escape/Mod-up, hook shutdown and window lifecycle
cancellation invalidate queued motion and restore the unchanged source. Competing
grabs cannot replace the active move. The old Ctrl+Mod column-reorder path stays.
Tiled clicks and sub-threshold movement preserve their source: the pinned niri
pull-out threshold is 256 logical pixels of displacement, latched once crossed.
Ordinary floating drags remain floating; right-click float/tile switching during
a move remains a separate missing gesture, not claimed by the insert-hint row.
The pre-pull-out rubber-band animation is not reproduced by a foreign HWND.

## Validation

The integrated release build, tests and all-target check pass with
`RUSTFLAGS=-D warnings` and `-j 2`: **1287 library + 14 CLI tests**, zero failures,
one ignored documentation example. Pure tests cover sparse merge precedence,
located decoder errors, corners, region ownership/restoration, policy defaults,
wheel fractions/replay, gradient coordinates/spaces/hues/alpha and insertion
transactions/cancellation, including cross-output reveal, retained native heights,
new-column-only defaults, pending native-size acknowledgments and DPI-scaled,
latched pull-out thresholds (including return-to-origin and diagonal motion).

The final mandatory locked harness completed with **526 pass / 3 fail / 80
information entries** (529 gates), including **37/37 Lane W gates passing**.
All 582 numeric step return codes were zero; startup additionally records `running`.
The expected-false crash-record check passes. Evidence: `.live/summary.json` and
`.live/lane-w/evidence.json`, with saved screenshots. Live coverage includes sparse
active/inactive/urgent paints, target-only native clipping and restoration, actual
tab visibility, zero/double/unmatched delivered wheel deltas, floating offsets,
all activation policies and explicit focus, gradient endpoints/midpoints/angle and
linear-sRGB distinction, real tiled insertion, release/Escape/Mod-up cleanup,
stationary/sub-threshold preservation and owned input/cursor restoration.

**Existing H9 failures remain and are not waived:**
`h9_fullscreen_covers_exactly_the_monitor`,
`h9_fullscreen_off_gives_the_tile_back`, and
`h9_tile_after_fullscreen_is_aligned_with_its_neighbour`.
Their evidence is `.live/h9-fullscreen-rect.json`,
`.live/h9-tiles-after-fullscreen.json`, `h9-01-fullscreen.png` and
`h9-02-tile-after-fullscreen.png`. The earlier completed run passed every existing
gate but failed 11 Lane W gates (518 pass / 11 fail); it exposed nested wheel replay,
a stale native urgency registry, and two pixel-probe bugs corrected before this run.
The fullscreen rectangle was `(-455,0)-(911,768)`, full-sized but horizontally
offset; afterward its recorded frame was parked at `(0,832)-(1366,1600)` while
the neighbour was `(12,12)-(443,716)`. This is not an all-green integration claim.
The H9 failures need independent follow-up; their cause is not established.

The single input-free 30.0134 s idle observation recorded 46.875 ms CPU
(**0.15618% of one core**), 252.654 context switches/s, no changed input and
**zero positioning lines**. These are shared-desktop observations, not a causal
performance guarantee. No new timer, polling loop, resting thread or dependency
was introduced; gradient preparation runs only on changed paint, and hints exist
only during a grab. Workspace-view/OKLab/OKLCH combinations and cross-output or
invalid-destination insertion permutations remain pure-test-only.

CI formatting ran successfully earlier. Its later required invocation was refused
by the command auditor as a formatter retry and was not retried; final whitespace
checking passed, but final CI-format conformance is not independently confirmed.
An initial queued full invocation exhausted the runner ceiling at H7 without a
summary; the focused diagnostic also first lacked its borrowed key map. Neither
incomplete attempt is counted as a full desktop pass.
