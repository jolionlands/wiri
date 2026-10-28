# Lane K: part-time DWM compositor — provisional desktop evidence

Branch `meylin/sa-a4871-6b955`, implementation/config commits `7691bc8`, `7de592d`,
`4de1ca1`; live gates `267a612`. This is lane-local evidence, **not integrated
sign-off**. The completed 2026-10-26 run was moved from `.live` to
`.scratch/lane-k-first-live` (called `evidence/` below). The later `.live` attempt
has no summary and is not counted. No harness was run for this documentation follow-up;
the lead will perform the official integrated cycle.

## Implementation and limits

`overlay/compositor{.rs,/win.rs,/snapshot.rs}` and `layout/engine/transitions.rs`
reuse the existing input message pump and active-only animation driver. A lazy
transparent, click-through, non-activating topmost `WiriAnimationLayer` hosts DWM
thumbnails while real sources are temporarily cloaked. Open scales 0.5→1 and fades
in; close uses copied pixels after HWND destruction; workspace switches represent
both window sets; resize blends old pixels with the live resized thumbnail.
Final layout/visibility reconciliation precedes thumbnail/layer removal. Input,
actions, reload and quit cancel/snap. Source inspection finds no additional idle
thread, timer, frame task or resting HWND; this is not an ETW wakeup measurement.

`window-open` and `window-close` accept independent spring/easing/off blocks and
global slowdown/off. Their defaults are 150 ms ease-out-expo / ease-out-quad;
workspace/view/movement/resize retain Phase 2B springs. Absent `animations` remains
disabled for wiri compatibility. Config/timeline, centered scale/alpha, cropping,
overshoot bounds, close-wait expiry and cleanup bookkeeping have pure tests.

These remain Windows approximations, so the four effect rows remain **partial**:
pre-show interception cannot guarantee no initial application flash; close snapshots
are best-effort and only prepared for wiri-requested closes, not arbitrary external
destruction. Capture is a bounded `GetWindowDC`/`BitBlt` copy, not an application
message or WGC wait. Protected/empty/unavailable sources take the fallback; those
failure paths were not exercised live. Workspace window sets slide, not shell
wallpaper surfaces. H57 observes resize lifecycle/final width, not pixel-perfect
blend fidelity. Custom shaders, gesture velocity hand-off, hardware-vsync pacing,
ARM64, multiple outputs and non-96-DPI desktops remain unverified or unsupported.

## Saved validation

The warning-denying release build, release tests and all-target check with `-j 2`
passed; logs are `.scratch/lane-k-{build,test,check}.log`. The saved test log reports
**1216 library + 14 CLI tests**, zero failed, one ignored doctest (correcting the
previous handoff's 1215 library count). No runtime code changed in this follow-up.

TRIDARIS x64, rustc 1.95, one 1366×768 display / 1366×728 work area at 96 DPI.
The existing reporter records **497 pass / 6 fail**
(`.scratch/lane-k-live-report/index.html:32`). All **10 H54–H57 checks pass**
(`evidence/summary.json:3002–3011`):

| Checks | Saved evidence |
| --- | --- |
| H54: passive layer, real source cloak, final geometry/uncloak | `lane-k/measurement.json` open fields; `h54-open-mid.png` |
| H55: original HWND already destroyed during copied-snapshot effect; layer cleanup | close fields (`source_exists=false`); `h55-close-mid.png` |
| H56: both workspaces move in two slowdown-10 PNGs; final visibility | workspace fields; `h56-workspaces-mid-{1,2}.png` |
| H57: resize lifecycle/final width, input cancellation, no idle layer, global-off observations | summary; `lane-k/measurement.json`; `lane-k/wiri.stderr.log` |

In H56 the outgoing red pixel region's bottom moves **512→290**, and incoming
blue top **600→378**, between the two PNGs. Both regions are 394 px wide. These are
visible-content observations, not just moving source rectangles. Animation CPU and
before/after idle/resource endpoints are recorded in [PERF.md](../PERF.md#lane-k-transient-compositor-checkpoint-2026-10-26).

### H22: foreground interference, not a demonstrated Lane K bug

In `evidence/lane-b/h22.stderr.log:235–236`, probe HWND **15864680** is activated
and foreground. Lines **286–287** show the cooldown marker dispatch at
**21:52:23.893753Z**; lines **288–289** then show **21:52:27.751369Z
`foreground: hwnd=2299090 managed=false`**, followed by **`+ Terminal
(CASCADIA_HOSTING_WINDOW_CLASS) [via foreground retry]`** at **21:52:27.877684Z**.
Thus another Terminal took foreground before the next injection; the log does not
identify who opened/activated it. `lane-b/results.json:126–130` records the last
successful cooldown-suppression check followed by **`Input refused: foreground is
not a Lane B probe`**. The guard correctly refused input, aborting the phase:
`summary.json:2888–2908` has 15 H22 successes and six default-false, unexecuted
cooldown-expiry/release/inhibition checks, not six observed semantic regressions.
H22's config has no animations section and its log has no compositor transition.
The evidence supports desktop foreground interference, not a Lane K compositor
fault; no code or gate was changed. These six gates still need the lead's official run.

The separate owned-daemon shutdown timeout is recorded in
`lane-b/results.json:131–135`. The existing Lane B helper still connects to literal
`wiri_control` (`scripts/live-phases/lane-b.ps1:283`), whereas this daemon advertises
`wiri_control_ark_1` (`h22.stderr.log:195`). This stale endpoint explains the cleanup
connection timeout, not the preceding foreground failure; no Lane B helper change
is included here.
