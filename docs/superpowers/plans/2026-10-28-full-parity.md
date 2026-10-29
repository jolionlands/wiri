# Full Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Track completion here and in the ignored execution ledger.

**Goal:** Finish the Windows-applicable behaviour and verification gaps against
niri e19df12cf81ffe805609fee497da1a097066ad2f.

**Architecture:** Extend the existing Rust action/IPC, native input and scalar
animation contracts. Integrate small independent feature lanes through one
Tridaris build/desktop controller; retain exact source and gate evidence per row.

**Tech Stack:** Rust, Win32, KDL, named-pipe JSON IPC, PowerShell live harness.

**Spec:** `docs/superpowers/specs/2026-10-28-full-parity-design.md`.

## Global Constraints

Use Tridaris for all native tests/builds, `-j 2`, warnings denied, one live desktop
owner and the existing global mutex. Do not write global input/display settings
or stop foreign processes. Preserve existing default behaviour and the owner's
dirty main checkout. Source pin and the parity matrices define the target;
hardware-pending and platform differences cannot become false passing claims.

## Review Focus

- Omitted action axes, nonzero working-area origins, DPI and inactive window IDs.
- Bezier endpoints, flat derivatives, valid overshoot and invalid/nonfinite data.
- Button edges, injected-event recursion, reload/cancel cleanup and default no-op.
- Typed IPC errors/defaults and CLI argument preservation.
- Focus/geometry stability on the full desktop with real application windows.

## Execution

- [x] Recover PR #2 and repair its 19 demonstrated failures: f217e15, 1,800 tests,
  62 affected desktop gates. The frozen full baseline later reached 775/782;
  its seven failures are tracked below and still require final desktop verification.
- [x] Reaudit class-(d) claims against current code and download/verify pinned
  niri reference. Record stale claims rather than reimplementing them.

### Task 1: Floating-window positioning

**Files:** `src/ipc/messages.rs`, `src/ipc/wire.rs`, `src/ipc/server.rs`,
`src/bin/ctl.rs`, `docs/IPC_SCHEMA.json`, narrow plumbing in `src/layout/engine.rs`, and a focused new
floating-position module. Do not modify the config bind vocabulary or input hook.

**Interfaces:** Native `MoveFloatingWindow` typed IPC/CLI, optional target ID,
PositionChange x/y, engine positioning that preserves unrelated windows.

- [x] Write parsing and geometry regressions against pinned niri behaviour;
  observe current action rejection before implementation.
- [x] Implement argument/default translation and pure position resolution.
- [x] Apply to floating targets, including inactive targets, with partial
  visibility clamps; update stored positions for tiled targets with pinned
  guards. Exclude only implicit/currently dragged targets during a grab.
- [x] Test fixed/proportional set/adjust, one-axis defaults, bad arguments,
  working-area origin/DPI, tiled stored-position guards, offscreen limits and
  proportional versus fixed negative adjustment clamps.
- [x] Review, integrate and pass nine H82 desktop checks for typed IPC/CLI,
  including refused legacy activation and foreground preservation.

### Task 2: Typed Bezier easing

**Files:** `src/config/decode/animations_decode.rs`,
`src/config/animation_config.rs`, `src/layout/animation/timeline.rs`,
`src/overlay/recent_windows.rs` close-fade snapshot, and a focused
solver module if appropriate. Coordinate any additional consumer enum matches.

**Interfaces:** Typed Easing control points shared by the animation timeline and
recent-window fade; strict KDL decoder for four coordinates.

- [x] Add regression showing valid four-coordinate input is rejected or sampled
  incorrectly by the existing implementation; observe its failure.
- [x] Decode and validate controls; implement deterministic bounded inversion.
- [x] Preserve all four controls through the real close-fade setter/getter.
- [x] Cover endpoints, identity, asymmetric x inversion (0,0,0,1 at x=.125 gives
  y=.5), flat slopes, overshoot,
  invalid x/y/count/properties/children, slowdown and existing curve behaviour.
- [x] Review/integrate and pass nine H81 checks for visible configured camera
  motion, distinct linear/Bezier trajectories and instant-animation controls.

### Task 3: Button-scroll hold/lock

**Files:** new `src/input/scroll_emulation.rs`, module registration in
`src/input/mod.rs`, `src/input/low_level_hook.rs`,
`src/input/pointer_settings.rs`, and the narrow pointer config validation/decoder
sections. Own these separately from Task 1's IPC/action files.

**Interfaces:** Pure state reducer for configured physical button/motion and a
native adapter producing tagged wheel events. Explicit configuration only.

- [x] Add regression for canonical evdev button configuration and current
  unimplemented behaviour; observe failure before the product implementation.
- [x] Decode all integral u32 button codes; report unsupported Windows buttons
  separately from syntax errors. Preserve a coherent mouse-scoped tuple and
  test device-block order permutations and partially configured mouse blocks.
- [x] Implement supported button mapping, consumed press/release, hold and
  lock/unlock transitions, motion accumulation and recursive-injection bypass.
- [x] Cover default no-op, unsupported buttons, horizontal/vertical motion,
  threshold/residual handling, reload cancellation and existing modal precedence.
- [ ] Report per-device and libinput timing differences honestly; add owned-input
  desktop fixtures without changing Windows pointer settings.
- [ ] Review and integrate.

### Task 4: Evidence and next batch

- [x] Reconcile PR #2's four eligible layout rows and stale next-lanes claims;
  retain remaining config semantics explicitly.
- [ ] Run full desktop harness and focused new-feature cases on Tridaris; record
  binary/source hashes and score with `live-report.py`.
- [ ] Recount exact/partial/hardware-pending outcomes and update the next task
  list from actual remaining code, including native bounds/startup/IPC semantics.
- [ ] Continue implementation; do not stop at this first batch or claim full
  parity from its test count.

### Task 5: Native application size constraints and refresh

**Files:** `src/backend/mod.rs`, new `src/layout/engine/native_bounds.rs`, narrow
state/admission/bounds/refresh sections of `src/layout/engine.rs`, `src/main.rs`,
and `src/layout/engine/rules.rs`; new owned fixture phase
`scripts/live-phases/lane-native-bounds.ps1`. Coordinate shared engine sections
with the floating-position worker before editing.

- [x] Audit pinned four-axis constraint merging, fixed-height opening, current
  Win32 sampling, consumers and cache lifetime; existing rule bounds are retained.
- [x] Observe pure constraint regressions before implementation; demonstrate
  seven native desktop failures against frozen pre-change f217e15 binaries.
- [x] Implement coherent native samples, physical margin conversion, effective
  bounds, pre-admission fixed-height policy and safe outside-lock refresh.
- [x] Test successful removal, timeout retention, retry, stale admission results,
  conflicting rules, asymmetric margins and frame/DPI invalidation.
- [x] Verify real engine allocation, neighboring windows and fixture query counts;
  preserve separately pending mixed-DPI evidence if hardware is unavailable.
- [x] Complete native-bounds round-three review and pass all eleven H84 checks.

### Task 6: Typed IPC adapter options

**Files:** `src/ipc/wire.rs`, preserving the floating-position additions.

- [x] Add decode-to-shared-parser regressions for explicit workspace targets,
  optional null references, six false-focus transfers and SpawnSh strings.
- [x] Observe all five intended assertion failures on Tridaris in
  `parity-typed-adapter-red.log`.
- [x] Fix translation without changing unrelated action syntax; preserve
  Index/Name identity and reject malformed payloads.
- [ ] Review, run unit/desktop effects, and integrate.
- [ ] Follow with stable workspace Id resolution and explicit inactive-window
  focus preservation as separate tested slices; keep unsupported compositor
  protocol actions and absent skip_empty outside invented contracts.

### Task 7: Explicit targets and smart focus

- [x] Observe failures for inactive transfers and explicit fullscreen/height.
- [x] Preserve focus for explicit fullscreen/height targets and Name/Id transfers;
  retain the focused-height wrapper. Initial targeted tests pass.
- [ ] Match Index transfers' monitor-local smart activation while keeping global
  focus stable; repair the positive-follow test's initial focus explicitly.
- [ ] Review cross-output/inactive/floating coverage and verify real foreground.

### Task 8: Startup rule phase

- [x] Observe initial four rule regressions and the independent owned-overlay
  policy regression before implementation.
- [x] Add one fixed 60-second phase, reload-stable deadline, one-shot dynamic
  reevaluation and owned-overlay refresh; keep initial-scan admission separate.
- [x] Pass 13 deterministic regressions on Tridaris.
- [x] Review and run the real deadline/reload/admission desktop phase H85:
  ten checks passed in the frozen 861 checkpoint. The final R3 source has
  twenty passing startup regressions after the expiry placement fixes.

### Task 9: Stable workspace IDs

- [x] Observe all seven typed actions rejecting valid u64 IDs before implementation.
- [x] Preserve lossless typed IDs and resolve current live workspace objects
  across outputs, reorder/rename/relocation and reused local keys.
- [x] Pass wire, engine and server regressions on Tridaris, including unknown-ID
  no-op behavior and malformed-reference rejection.
- [x] Review, document and run desktop phase H86: ten checks passed in the
  frozen 861 checkpoint. Cross-output monitor relocation remains unverified
  because that run had only one physical output.

### Task 10: Frozen full-baseline failures

- [x] Preserve and score the complete f217e15 run: 775 pass, seven fail,
  zero phase errors. Establish causes from native geometry, logs and probe records.
- [x] Observe H25 activation-completion and H66 gap-reload unit regressions.
- [x] Verify their fixes and pass all 65 affected H22-H25/H66 desktop checks.
- [x] Observe H69's queued-spawn fixture regression, then pass fixture checks
  after waiting for its unique child without resending the request.
- [x] Pass all eight H69 environment/startup gates in a separate repaired
  focused run; preserve the earlier missing-runner-helper failure as setup evidence.

### Task 11: Typed size changes

- [x] Reaudit the pinned percentage contract: JSON `SetProportion(10)` is 10%,
  and typed negative Set values must retain their Set kind through dispatch.
- [x] Observe consumer regressions before replacing lossy argv translation with
  a bounded structured size-action transport.
- [x] Preserve fixed/proportion units and Set/Adjust kinds; validate malformed
  variants and numeric domains, including explicit window targets.
- [ ] Review and verify actual dimensions and foreground preservation.

### Task 12: Fixed-height DPI

- [x] Trace logical fixed size in pinned niri and Wiri's width conversion;
  confirm tiled height resolves directly against physical view dimensions.
- [x] Observe 1.5-scale fixed/adjusted height regressions, with width control.
- [x] Scale fixed changes using the target output, retaining proportion units,
  per-workspace layout and focus behavior; review floating consistency.
- [ ] Pass unit and native checks without treating 96-DPI evidence as mixed-DPI proof.

### Following audit queue

Confirmed remaining candidates require their own source-backed specs and tests:
curated hotkey essential ordering/fallbacks, Mod/Super spawn eligibility and
command descriptions, title markup, scroll-factor axis semantics/bounds, and
middle-button emulation. The initial 94 partial rows have been audited by source;
the action census added two missing floating-preset consumers, making 96 partial.
Retain their status until implementation and matching evidence justify a change.

### Task 13: Explicit width targets and IPC geometry

- [x] Add server-boundary typed/legacy width regressions for inactive tiled and
  floating targets, 150% DPI, unrelated columns, focus and invalid IDs.
- [x] Observe width-target failures, add a direct engine width operation and
  preserve legacy `set-column-width` extension behavior separately.
- [x] Observe fractional-DPI IPC geometry failures for disabled borders and
  inactive workspace overrides; retain enabled-border controls.
- [x] Verify IPC reports the same per-window border inset used for native layout
  in deterministic engine/server tests. This does not qualify mixed-DPI hardware.
- [ ] Review and include native foreground/dimension evidence.

### Task 14: Native margin context provenance

- [x] Observe failed-refresh cache loss and verify the same-context retention fix.
- [x] Add the between-query transition regression: margins from context A must
  not survive failed measurement in stable context B.
- [x] Observe RED, preserve margin context provenance or invalidate at the
  transition boundary, and retain same-context timeout behavior.
- [x] Review the complete native query/cache path after covering GREEN;
  native-bounds round three is clean and all 28 current regressions pass.

### Task 15: Canonical fractional spacing and signed struts

This is the first slice of the confirmed logical-metrics debt. Border/ring,
tab and shadow projection follow separately. Five appearance rows have been
corrected from done to partial; do not preserve their previous classification.

**Contract:** Preserve logical fractional gaps and signed struts in canonical
configuration. Merge global, output and named-workspace values before projecting
once for each output. Legacy reserve-* config aliases and runtime reserve_area
pixels retain their physical-pixel contract, with one authoritative value per edge.

**Ownership:** Config/projection owns config types/decoding and a new pure metrics
module. Engine integration owns effective views, strip calculation, reload/scale
invalidation, IPC geometry and runtime reservation plumbing. Freeze the shared API
before editing these files in parallel. Appearance consumers begin after spacing.

Reuse `config::Edges<T>` for canonical `struts: Edges<InsetValue>` and partial
`Edges<Option<InsetValue>>`, where `InsetValue` distinguishes logical `f64` from
legacy physical `i32`. Gaps become canonical `f64`. A pure `project_spacing`
produces integer physical gaps and fractional physical strut intermediates;
`spacing_work_rect` returns `PhysicalWorkArea { loc: Point, width: f64, height: f64 }`,
with the shared origin ceiling and corresponding size adjustment. Preserve its
fractional extent through proportional sizing until final placement conversion;
early flooring changes `floor(903.75 * 0.8) = 723` to `floor(903 * 0.8) = 722`.
There is no implicit integer-rectangle conversion. Validate/fallback output scale
at this boundary. Engine caches are
derived from canonical values; no inverse projection may recreate configuration.

Config/projection owns `config/types.rs`, `config/decode.rs`, the metrics module
and config fixtures. Engine integration owns `engine.rs`, `named_config.rs`,
`strip_model.rs`, height consumers in `rules.rs`/`presets.rs`, `ipcview.rs`,
`input/focus.rs`, runtime-reservation IPC and startup/reload call sites. Replace
runtime cache mutation with optional physical reservation requests, preserving
explicit zero and existing global/output/workspace precedence. Project every
connected output, including outputs without a layout override. Give input focus
the same workspace work rectangle used by placement and height sizing.

Sequence the shared config-file edits: Task15 config/projection freeze, then
Task17 input-factor config edits. Task15 engine consumers can proceed alongside
Task17 input consumers after that handoff; do not assign simultaneous ownership
of different sections of `config/types.rs` or `config/decode.rs`.

- [x] Observe compiling decode regressions: gap 2.5 and strut -4.5 must survive
  global/output/workspace parsing and reload without early integer rounding.
- [x] Observe engine geometry regressions through existing APIs: gap 2.5 becomes
  3 physical pixels at 1x and 4 at 1.5x; zero stays zero. Signed struts snap the
  resulting physical origin upward and reduce size by the same snap delta.
- [x] Observe a trailing-fraction regression before implementation: left -4.5,
  right 0.25 and proportion 0.8 retain width 903.75 until yielding width 723.
- [x] Qualify final logical window-request rounding separately: at 1x, left
  -4.5/right 0.5/proportion 0.8 requests 722.8 logical pixels and settles at
  722; at 1.5x, the same logical struts on a 900px physical parent request
  482.8 logical pixels and settle at 482 logical / 723 physical pixels.
  Preserve fractional work extents until this request boundary. Camera/reveal
  precision remains a separate qualification from settled tile geometry.
  Both final-request cases now have observed compiling assertion failures.
  Resolve canonical proportional requests in logical units, subtract the
  effective physical border in logical units, floor the client width, then
  rasterize it to physical pixels. Share resolved native/tile rectangles and
  the explicit logical client width with IPC. At 1.5x, an accepted logical
  width of 483 becomes 725 physical pixels: IPC tile width is 483.333..., while
  window width remains 483. Preserve the standalone physical API and existing
  height policy. This width correction does not qualify appearance scaling,
  height request rounding, camera precision or actual native acknowledgement.
- [x] Implement canonical values and pure projection, retaining fractional
  strut intermediates and avoiding independent side rounding or projection drift.
- [x] Verify 1x -> 1.5x -> 1x, inactive-workspace relocation, partial precedence,
  native/IPC rectangle agreement and a 10px runtime reservation at both scales.
- [x] Review and add an owned desktop fixture at the observed scale. Keep
  synthetic 1.5x evidence separate from actual mixed-DPI hardware qualification.

### Performance qualification after the integrated checkpoint

The next observer should acknowledge geometry on the owned sentinel's UI thread.
Attach a perf-only `NativeWindow` listener on that thread, arm a generation and
baseline outside the timed interval, then capture QPC at the first armed
`WM_WINDOWPOSCHANGED` whose immediate rectangle read differs. The injector records
QPC before the complete SendInput batch, its accepted count and return timestamp,
then waits once on an event. Store collector wake time separately from the target
timestamp. Keep the previous polling metric as historical evidence, with a new
observer/schema version for acknowledgement results.

This endpoint includes target-thread message delivery; it is an upper bound on
the rectangle change, not its exact native commit or a displayed frame. An
out-of-context WinEvent callback includes its own queued delivery delay, and
`DwmFlush` covers changes queued by the caller rather than a session-wide frame.
See Microsoft's [NativeWindow documentation](https://learn.microsoft.com/en-us/dotnet/api/system.windows.forms.nativewindow?view=netframework-4.8.1),
[WM_WINDOWPOSCHANGED](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-windowposchanged)
and [DwmFlush](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmflush).

Before collecting new latency samples, cover generation/arm/cancel/late-callback
state, first-result retention and unchanged rectangles. Calibrate with an owned
resize and deliberately delayed collector retrieval: target timestamps must stay
independent of collector wake delay. Verify UI-thread attachment/release and
destruction/timeout cleanup; preserve foreground/key preflight, partial-injection
releases and settled-transition confirmation. Do not change global timer
resolution, spin an observer, or claim a product speedup from changing endpoints.

The f217e15 full baseline included concurrent compilation and a 33.47ms observer
gap. Its timings cannot enforce the 16ms movement target. Current source already
has bounded runtime workers and demand-driven event/reconcile/animation wakeups;
the older poller roadmap in PERF.md is historical.

- [ ] Run the frozen complete desktop harness with root compilation stopped;
  retain its one cold start, 30s idle window, ten spaced keys and resource endpoints.
- [ ] Improve the external target-acknowledgement observer before enforcing
  sub-frame latency; report uncertainty and actual rectangles independently.
- [ ] Measure focus/cursor acknowledgement and engine-lock duration around the
  synchronous activation fallback; preserve generation and completion guarantees.
- [ ] Measure ordinary animated frame spacing separately: the existing perf
  collector disables animations. Attribute runtime costs before optimization.

### Task 16: Hotkey-overlay selection and descriptions

Twelve tests in `overlay/hotkey_rows.rs` use existing production APIs. Their
baseline compiled and produced ten meaningful assertion failures and two
passing controls. Production implementation is now authorized within this lane.

- [x] Observe RED for the pinned 18 essentials/order, action-wide custom/null
  title precedence, first-binding selection, Mod/Super spawn eligibility and
  command-specific launch labels; retain two positive controls.
- [x] Implement exact action selection and dynamic quit/workspace-transfer
  fallbacks, preserving the selected bound focus variant and its description.
- [x] Preserve literal modifier provenance through bind registration. Equivalent
  resolved Ctrl and Mod masks are insufficient to implement pinned eligibility.
- [x] Carry typed command-head descriptions rather than reverse-parsing Rust
  Debug strings. Preserve custom titles and complete action-wide suppression.
- [x] Review and verify the actual panel, including fallback wording. Curation
  completion does not imply Pango markup rendering parity.

### Task 17: Signed axis scroll factors

Eight tests in decoder and accumulator modules compiled and produced eight
meaningful failures. Valid-syntax tests prove acceptance, not retained axis
values; storage and consumers require their own assertions after the API exists.

- [x] Observe RED for mouse/touchpad axis syntax, invalid shapes, negative axis
  reversal and residual preservation while alternating unequal axis factors.
- [x] Store optional typed factors: base 0..100, axes -100..100, with each axis
  resolving override -> base -> 1. Preserve explicit legacy alias behavior.
- [x] Apply mouse device factors after physical binding dispatch and compose
  with the nonnegative window-rule factor. Button scrolling remains window-only.
- [x] Retain zero suppression, self-injection bypass and independent residuals;
  add actual stored/resolved-value and consumer regressions after the API exists.
- [x] Review and verify native wheel effects. Touchpad syntax/storage must not
  imply independent device routing through the device-agnostic WH_MOUSE_LL path.

### Task 18: Canonical typed action and output command forms

The pinned census matches all 136 bindable names; it does not establish argument
fields or effects. Twelve tests are authored in the existing wire, output-query
and CLI test modules. The narrow canonical correction at 0f1c4fc passes 75
unique scoped tests, including all 12 new cases. Preserve friendly extensions as explicit
compatibility controls while accepting canonical niri shapes.

- [x] Observe RED for `SwitchLayout { layout: Next/Prev/Index }`, output
  Mode/Scale `Automatic`, and `output NAME position set X Y`.
- [x] Accept the pinned fields/enums/grammar and reject malformed or truncated
  canonical requests. Keep legacy accepted aliases separate from the contract.
- [x] Review and verify typed decoding and CLI request output. Native keyboard
  layout acceptance and physical output changes require their own evidence.
- [ ] Follow with the source-confirmed explicit-ID focus and typed requiredness,
  numeric-range/default gaps; do not repeat the already fixed workspace IDs,
  false-focus transfers, size units or floating-position defaults.

### Task 19: Floating window height presets

Both the general preset-height config row and action/back row remain partial:
parsing, storage and tiled behavior work, but the floating dispatch explicitly
does nothing. Pinned workspace routing reaches `floating.toggle_window_height`,
which applies the configured fixed/proportional list and retains a preset index.
The two matrix rows describe one shared implementation gap.

- [x] Add and observe dispatch/state regressions for current and explicit
  floating targets: nonpreset first step, forward/back wrap, remembered index,
  target effective configuration, fixed-size DPI and proportional units.
- [ ] Implement floating height preset resolution and lifecycle without changing
  tiled column behavior. Preserve inactive explicit-target focus where required.
- [ ] Review and verify owned native height changes and refresh/reload retention;
  keep actual mixed-DPI hardware evidence separate from deterministic tests.

### Following source-backed queue

- Full configured-title Pango semantics, literal ampersands and consistent styled
  measurement/painting; invalid markup falls back to the whole original string.
  A declared small subset must retain partial status.
- General non-transfer viewport anchoring and partial-window clipping, separate
  from the now-reviewed workspace-transfer anchor helper.
- Ordinary application self-resize reconciliation, beyond four-axis bounds queries.
- Floating-to-fullscreen strip semantics and correct restoration.
- Admission-focus heuristics, continuous four-finger/touch-overview behavior,
  and an exhaustive pinned action/CLI contract census.


### R4 execution checkpoint, 2026-10-29 UTC

All 17 combined source gates passed: 2,037 full-suite Rust tests, the separate
graphics test, seven focused groups and both pure CI fixtures. H87 native scroll
passed 12/12 against the new frozen binaries, versus 5/12 on R3. Numeric height
oracles were retained while repairing an integer-allocation regression. See
[the evidence report](../../parity/2026-10-29-tridaris-r4.md).

Task15 native geometry and Task16 rendered panel checks remain pending. Next
observe the prepared four-case mouse-origin binding baseline, then the separate
floating-height preset baseline. No matrix promotion or new performance claim
is made; the old full R3 desktop cycle remains failed.
