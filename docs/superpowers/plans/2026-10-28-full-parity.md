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
- [ ] Review and run the real deadline/reload/admission desktop phase H85.

### Task 9: Stable workspace IDs

- [x] Observe all seven typed actions rejecting valid u64 IDs before implementation.
- [x] Preserve lossless typed IDs and resolve current live workspace objects
  across outputs, reorder/rename/relocation and reused local keys.
- [x] Pass wire, engine and server regressions on Tridaris, including unknown-ID
  no-op behavior and malformed-reference rejection.
- [ ] Review, document and run desktop phase H86; retain hardware limits explicitly.

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
- [ ] Observe consumer regressions before replacing lossy argv translation with
  a bounded structured size-action transport.
- [ ] Preserve fixed/proportion units and Set/Adjust kinds; validate malformed
  variants and numeric domains, including explicit window targets.
- [ ] Review and verify actual dimensions and foreground preservation.

### Task 12: Fixed-height DPI

- [x] Trace logical fixed size in pinned niri and Wiri's width conversion;
  confirm tiled height resolves directly against physical view dimensions.
- [ ] Observe 1.5-scale fixed/adjusted height regressions, with width control.
- [ ] Scale fixed changes using the target output, retaining proportion units,
  per-workspace layout and focus behavior; review floating consistency.
- [ ] Pass unit and native checks without treating 96-DPI evidence as mixed-DPI proof.

### Following audit queue

Confirmed remaining candidates require their own source-backed specs and tests:
curated hotkey essential ordering/fallbacks, Mod/Super spawn eligibility and
command descriptions, title markup, scroll-factor axis semantics/bounds, and
middle-button emulation. Remaining matrix rows have not all been re-audited;
retain their current status until implementation and evidence justify a change.

### Task 13: Explicit width targets and IPC geometry

- [x] Add server-boundary typed/legacy width regressions for inactive tiled and
  floating targets, 150% DPI, unrelated columns, focus and invalid IDs.
- [ ] Observe width-target failures, add a direct engine width operation and
  preserve legacy `set-column-width` extension behavior separately.
- [x] Observe fractional-DPI IPC geometry failures for disabled borders and
  inactive workspace overrides; retain enabled-border controls.
- [ ] Verify IPC reports the same per-window border inset used for native layout.
- [ ] Review and include native foreground/dimension evidence.

### Task 14: Native margin context provenance

- [x] Observe failed-refresh cache loss and verify the same-context retention fix.
- [x] Add the between-query transition regression: margins from context A must
  not survive failed measurement in stable context B.
- [ ] Observe RED, preserve margin context provenance or invalidate at the
  transition boundary, and retain same-context timeout behavior.
- [ ] Review the complete native query/cache path after covering GREEN.

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

- [ ] Observe compiling decode regressions: gap 2.5 and strut -4.5 must survive
  global/output/workspace parsing and reload without early integer rounding.
- [ ] Observe engine geometry regressions through existing APIs: gap 2.5 becomes
  3 physical pixels at 1x and 4 at 1.5x; zero stays zero. Signed struts snap the
  resulting physical origin upward and reduce size by the same snap delta.
- [ ] Implement canonical values and pure projection, retaining fractional
  strut intermediates and avoiding independent side rounding or projection drift.
- [ ] Verify 1x -> 1.5x -> 1x, inactive-workspace relocation, partial precedence,
  native/IPC rectangle agreement and a 10px runtime reservation at both scales.
- [ ] Review and add an owned desktop fixture at the observed scale. Keep
  synthetic 1.5x evidence separate from actual mixed-DPI hardware qualification.

### Performance qualification after the integrated checkpoint

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
