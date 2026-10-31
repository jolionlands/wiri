# Wiri: finish Windows-applicable niri parity

Continue the existing parity programme against niri
`e19df12cf81ffe805609fee497da1a097066ad2f`. The user requested continued
implementation toward full parity, primarily on Tridaris. Keep the existing
Rust/Win32 architecture and the existing parity matrices as the scope. A passing
gate is evidence for its named behaviour, not permission to erase a remaining
semantic difference.

## Approach

Implement confirmed missing behaviour in independent slices, add regression
tests, then integrate and run actual desktop checks. Reconcile stale audit rows
against source and recorded evidence before assigning work. A documentation-only
recount cannot deliver the requested behaviour; a wholesale compositor rewrite
would discard working integration without resolving the concrete gaps first.

Keep exact matches, Windows equivalents, missing semantics and hardware evidence
separate. Do not relabel platform approximations as exact parity or count skipped
hardware checks as passes. Preserve the 396 Windows-applicable-row denominator
unless an explicit source-backed applicability correction warrants changing it.

## First implementation batch

1. **Floating-window positioning.** Implement pinned niri's IPC/CLI action with
   optional window ID and independent x/y PositionChange arguments. Absolute
   positions include the working-area origin; proportions use its full size;
   signed changes adjust the existing logical position. Missing axes mean +0.
   Preserve niri's bounded partial offscreen placement and its different fixed
   versus proportional adjustment clamps. For tiled targets, update the stored
   floating position with niri's default-position/adjustment guards. An active
   interactive move excludes only the implicit/currently dragged target, not a
   different explicit target. This revision does not
   expose this action in KDL binds, so do not invent that contract.
2. **Bezier easing.** Decode `curve "cubic-bezier" x1 y1 x2 y2`, preserve the
   control points in a typed curve, invert the x polynomial and sample y. Validate
   x in [0,1], finite/range-checked y, argument count and node structure. Preserve
   existing non-Bezier curves and consciously retain any legacy bare-name alias.
   The recent-window overlay's shared close-fade snapshot must retain all four
   controls coherently, not just an enum code and duration.
3. **Button scrolling.** Implement explicit on-button-down scroll hold/lock in
   the native hook using a pure state reducer. Honor niri's evdev button codes
   (272..276 map to Windows left/right/middle/X1/X2), consume the configured button
   and tag wheel reinjection. Button 0 disables. Preserve existing configuration
   compatibility explicitly; decode accepts integral u32 button codes, while
   unsupported Windows buttons remain explicitly unapplied/diagnosed. Keep the
   mouse method/button/lock as an independent tuple so other device sections
   cannot overwrite it. Unsupported device-specific semantics remain open.

## Constraints and verification

- All builds and native/desktop tests run on Tridaris, with `-j 2` and
  `RUSTFLAGS=-D warnings`. The owner laptop is for source edits and orchestration.
- One controller owns the interactive desktop and the live-test mutex. Workers
  do not launch daemons, drive input, change system settings or kill foreign
  processes. Background helpers launch hidden.
- Keep work isolated from the owner's deliberately dirty main checkout.
- Write and observe a regression failure before each product fix. Review each
  slice for source fidelity, integration and unintended default behaviour.
- Run the full live harness as well as focused new-feature checks. Record the
  exact binary/source revision, counts and remaining failures; do not claim full
  parity while any applicable semantic delta remains.

## Subsequent work

Reaudit native min/max sizing and invalidation, the first-60-second startup rule
context/expiry, focus policy, typed IPC option semantics, middle emulation,
per-client shortcut inhibition, workspace/output handling and the eight excluded
actions. Add missing camera-motion and DnD dwell evidence. Retain separate
multi-display/PTP/pen/touch qualification requirements and investigate actual
Windows equivalents for the remaining compositor/platform differences.

### Next slice: native application size constraints

The current backend reads only native minimum tracking width. Carry a coherent
four-axis native sample through the existing rule and layout bounds instead.
Zero denotes unconstrained; combine minima by maximum and finite maxima by
minimum, with the minimum winning conflicting constraints. A positive effective
fixed height defaults to floating unless an explicit rule chooses otherwise.
Fixed width alone and zero/zero are not that floating heuristic.

Convert outer tracking sizes to visible bounds on both axes using margins from
the same physical coordinate system. Apply Wiri decoration insets at the tile
boundary, without scaling already physical native answers twice. Seed the sample
before admission, and refresh outside engine locks. Successful unconstrained
answers must clear old bounds; timeouts must retain the last good sample.
Freshness/backoff and invalidation are Windows mechanisms; reject obsolete
results after a window is removed and admitted again. Frame, DPI and output
changes must invalidate the relevant conversion.

First demonstrate the missing behavior with a dedicated owned native window
whose WM_GETMINMAXINFO response and query counter can change. Distinguish native
default tracking maxima from explicit application limits with fixture evidence.
Check engine/IPC allocation and neighbors as well as visible rectangles, so the
application refusing an incorrect size cannot create a false passing gate.

### Typed IPC translation and targeting

Preserve typed workspace metadata through the shared parser: SetWorkspaceName's
`workspace` field and the other metadata actions' optional `reference` fields
must retain Index versus Name, including numeric and flag-looking names. Null
optional references mean the default target. Transfer actions must retain an
explicit `focus:false`; SpawnSh carries one complete command string.

Stable workspace Id references require a further resolver slice, not coercion
to a visible index. Targeted transfers follow only when the moved window was
already focused; explicit inactive IDs must preserve unrelated focus. Audit
nontransfer ID actions separately because the legacy executor currently focuses
their targets before acting. The pinned source does not expose skip_empty, so
do not add that as a parity requirement.
