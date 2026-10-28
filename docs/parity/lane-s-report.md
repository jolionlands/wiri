# Lane S: screenshots and non-layout actions

Baseline: integrated `a499d20`; niri main `e19df12cf8`. This lane owns matrix B
sections 7 and 12, not layout/workspace/parser/rule rewrites.

## Implemented

1. **Frozen selector.** Capture the focused output before creating the topmost,
   initially nonactivating popup. Inclusive drag selection, remembered region,
   Space-drag movement followed by resumed resizing, arrow movement and Shift-arrow
   resizing, P pointer toggle, Enter/Space/capture-button save + copy, Ctrl+C
   copy-only, Escape cancellation. Clean and pointed images are frozen together;
   saving crops those images rather than recapturing UI pixels. Absolute paths and
   pointer options share bind/CLI/typed-IPC validation. Selection is confined to
   one output. Final mouse-release coordinates are applied before reentrant
   ReleaseCapture; held Space autorepeat cannot accidentally confirm after a drag.
   Primary touch relies on Windows mouse synthesis; not touch-tested.
2. **Immediate captures.** `screenshot-screen` and `screenshot-window` share
   `write-to-disk=`, `show-pointer=`, and absolute destination handling. Disk false
   takes precedence over an explicit destination, while every capture copies to
   the clipboard. Screen/UI pointer defaults to true; window defaults to false.
   Explicit managed-window `--id` captures without focusing. Window pointer is
   composited only when its top-level window receives it. Existing WGC-first /
   PrintWindow fallback, PNG encoding, clipboard, events and notification paths
   are retained.
3. **System actions.** Idempotent overview open/close, urgency toggle/set/unset
   with managed `--id`, internal urgency/rules/events and FlashWindowEx without
   activation; focused windows cannot become urgent. Foreground synchronization
   clears urgency immediately, without waiting for the separate hook-clear event.
   Foreground-thread keyboard layout next/prev/zero-based index uses the same
   loaded-HKL ordering as queries. Quit binds and typed Action::Quit confirm
   asynchronously unless explicitly skipped; legacy `wiri-ctl quit` remains
   immediate for management compatibility. `load-config-file` validates before
   switching the active file/watcher, keeps the last good config on failure and
   reloads the active file when omitted. Shell-free spawn expands a leading `~`
   only in argv[0], never later arguments. Monitor power and suspend have an
   append-only `WIRI_SYSTEM_ACTIONS_DRY_RUN` audit hook; unit tests unconditionally
   no-op them. Typed IPC validates semantic fields rather than silently dropping
   ids, layout targets, config paths or quit flags.

## Release verification

Required commands with `RUSTFLAGS=-D warnings` and `-j 2`: release tests,
all-target check and release build pass. **1241 library + 14 CLI tests pass**;
one documentation example is ignored. Pure coverage includes selection/clamping/
cropping, release-coordinate and autorepeat regressions, screenshot option defaults
and rejection, bind properties (including quit), typed-wire adapters, executable-only
home expansion, layout target wrapping and urgency state/rules/events/focus clearing.
An intermediate build failed because our stalled live daemon still held wiri.exe;
it passed after stopping only the verified owned daemon and its two harness parents.

`git diff --check` passes. The prescribed formatter ran earlier in this lane, but
its final reruns were refused by the runner (unknown shared-script contents;
reading `C:\wiri-dev\fmt.cmd` was then scope-denied). The narrow access request is
pending. Final corrective commits therefore still need the prescribed formatter.

## Interrupted desktop evidence — not full sign-off

The locked desktop run reached Lane S, then stalled serializing Get-Content's
provider-decorated strings in the spawn evidence. There is **no completed
`.live/summary.json`** for this run and no new total for the existing suite.
Persisted Lane S evidence: **21 of 25 gates recorded, 18 pass, 3 fail**.

- All **9 H46** gates passed: nonactivation, PNG/clipboard dimensions, Space-drag,
  cancellation, copy-only, button and Space save, frozen pixels and pointer toggle.
- All **6 H47** gates passed: screen pointer pixels, disk-false override, typed
  window destination/options, nonfocusing managed id, relative-path rejection and
  bind properties. Window pointer compositing itself was not separately pixel-tested.
- H48 overview idempotence and nonfocusing urgency set/unset/toggle passed. Layout
  next/prev/index passed only with the single US HKL: both directions wrap to index 0.
- **`h48_urgency_events_and_focus_clear` failed**: true/false events arrived but the
  engine retained urgency after focus. Corrected both backend urgency-event mirroring
  and immediate foreground clearing; added a pure urgency/rule regression. Not rerun.
- **`h49_load_config_file_and_reload_current`** and
  **`h49_bad_config_keeps_last_good` failed** their hotkey assertions. Two successful
  ConfigLoaded events and the invalid-file error were recorded. Product registered
  F10, but the harness Chord map ended at F9 and injected key 0. Add a lane-local
  F10 mapping without relaxing either assertion. Not rerun.
- The owned spawn probe wrote both literal arguments correctly. Its gate was not
  persisted because evidence serialization stalled. Read file lines through
  `[IO.File]::ReadAllLines` instead; the later dry-run power, quit-cancel and
  quit-skip gates were not reached. No real power mutation was performed.

Evidence retained locally (git-ignored):

- `.live/lane-s/evidence.json`
- `.live/lane-s/ba1f17dae6374c32bea7d684bef744d0/daemon.stderr`
- the same fixture directory's `wire.ndjson`, `events.ndjson`, PNGs and spawn marker.

The harness now includes H46-H49 through the single `lane-s.ps1` dot-source and
writes incremental stage/evidence records. A required full rerun was attempted
but refused by the safety auditor over dry-run inheritance/enforcement assurance,
even with the worktree audit environment set before invoking the harness. The
unique pipe's normal graceful quit was separately scope-denied; its narrow access
request remains pending. Only the three path/parent-verified processes started by
this task were stopped. No foreign daemon was stopped. Do not treat the fixes or
prior integrated 440-gate result as a completed pass of this branch.

## Cost and remaining limits

No new crate, idle timer, polling loop or resident thread. Screenshot memory/GDI,
selector HWND/message thread, confirmation dialog and power workers are allocated
on demand and released at completion. Keyboard, pointer, foreground and urgency
updates use the existing input/WinEvent pumps. Existing overview animation clock
runs only while active. Config reload uses an event-driven channel. No new idle
measurement is claimed; the interrupted run did not reach the perf phase.

Remaining work: authorized formatter and complete locked live rerun, especially
all three failed gates and the four unreached H49 gates. Multiple loaded keyboard
layouts, multiple physical outputs, touch, forced PrintWindow fallback, elevated/
protected windows and actual notification visibility remain unverified. Real
monitor power/suspend, privilege policy and wake behavior are deliberately untested.
Windows applications may reject language-change requests; no caller-thread layout
activation or system keyboard setting is changed. OS-owned PrintScreen/Win+Shift+S,
automatic client shortcut inhibition, hotkey-overlay presentation and touchpad
toggling retain their existing partial/missing rows outside this lane.
