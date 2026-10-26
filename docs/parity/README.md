# niri parity: where wiri really stands (2026-10-24)

These two matrices replace the claims in `docs/PARITY.md`. Each row was checked in
wiri's code (config key parsed, then stored, then acted on), not taken from the docs,
against niri `main` at e19df12cf8 (2026-10-22; the latest release tag is v26.04).

- [Layout and behaviour](niri-gaps-layout-behaviour.md): layout, sizing and arrangement
  actions, focus, fullscreen and maximize, floating, workspaces, monitors, tabs, overview,
  recent windows, window rules, effects, animations, gestures.
- [Config, input and IPC](niri-gaps-config-input-ipc.md): config system and KDL, keyboard
  and pointer input, key bindings, hotkey overlay, screenshots, IPC requests and events,
  outputs, non-layout actions, misc and security.

| Matrix | done | partial | missing | N/A on Windows |
| --- | --- | --- | --- | --- |
| Layout and behaviour (226 rows) | 71 | 62 | 78 | 15 |
| Config, input and IPC (259 rows) | 68 | 67 | 54 | 70 |

The counts are a recount of current feature-row statuses including integrated Phase 2B and
retained Phase 2C Lane B/D updates; they are not additional features implemented by A2.
Earlier checkpoints (layout; config): Phase 2A (`b702890`) was 53 / 58 / 100 / 15; 43 / 74 / 72 / 70, Phase 1 was
30 / 70 / 111 / 15; 15 / 86 / 88 / 70, and the original audit was 2 / 85 / 124 / 15;
11 / 87 / 91 / 70. Unqualified line numbers in both files refer to commit `1532635`;
phase-labelled rows relocate the changed code by function name. Each matrix ends with a list
of the working-tree edits that were in flight during the original audit. The
[Phase 1](phase1-report.md), [Phase 2A](phase2a-report.md) and
[Phase 2C Lane D](phase2c-lane-d-report.md) reports distinguish live verification from unit tests.

## Plan

**Phase 0, done (1532635..4b9d834):** wiri runs on a live desktop. No repositioning loop,
IPC never blocks, every window is tiled, focus follows the OS, xkb/niri key names, a
GUI-subsystem build with a log file, crash-safe hidden windows, Escape reaches
applications, the overview no longer freezes, `move-column-left/right` swaps. Verified by
`scripts/live-test.ps1` on a spare Windows 11 laptop.

**Phase 1, done (`02e8aab`..`6ea45e4`, [report](phase1-report.md)), safe to daily-drive (P0):** unknown
modifiers are rejected and `Mod` is tied to `mod-key`; `spawn` and `spawn-at-startup` keep their
arguments and search PATH; every action targets the real foreground window, floating ones
included; minimized windows leave no gap; floating windows belong to their workspace;
niri's keymap semantics (`consume-or-expel`, `maximize-column`, first/last moves) and its
action names and argument grammar (`focus-workspace 2`, `set-column-width "+10%"`);
workspace model (index addressing, previous-workspace memory, real named workspaces); the Phase 0
leftovers (a terminal's minimum width, an exact fullscreen, no `Ctrl+Alt+Tab` default, windows put
back after a crash); monitor unplug keeps the windows (**unit-tested only: the test laptop has one
display**); hiding inactive workspaces by shell cloaking so windows stay in the taskbar and Alt+Tab is
there as the opt-in `hide-method "cloak"` (`SW_HIDE` stays the default).

**Phase 2A, done (`0a0bb75`..`b702890`, [report](phase2a-report.md)):** a real KDL parser with visible
errors (a bad config is never applied half; `cooldown-ms=` and `repeat=false` act); window rules that act
(`open-on-output`, `open-on-workspace`, `open-floating`, `open-fullscreen`, `open-maximized`, `open-focused`,
default sizes, size limits, opacity that follows the window); the fullscreen and maximize model (the window keeps
its column, `maximize-window-to-edges`, an application that goes fullscreen or maximizes itself is followed);
niri's IPC (the event stream: the state first, then every change; `wiri-ctl action` with niri's arguments, quoting
and `--id`).

**Phase 2B Lane A (P1), integrated and validated (`81ff35c`..`bd71c96`):** unified lazy decorations,
configured analytic spring/easing motion, active-only tabbed columns, live DWM workspace/recent previews
and a monitor-fitting Important Hotkeys sheet. The complete locked integrated run passed **440 gates,
0 failed**, including all **102 H16–H21** checks and retained Lane B/D gates; 1114 library + 7 CLI tests
pass. [The Phase 2B report](phase2b-report.md) separates current evidence, earlier-run findings and
remaining compositor approximations. Recent/modal input uses Lane B's existing dispatcher, primary
keyboard engine and pointer/focus architecture, not the older keyboard relay thread. No new dependency,
idle timer or always-on thread; passive decoration HWNDs are lazy and transient UI is session-local.

**Phase 2C Lane B ([report](phase2c-input-report.md)):** primary low-level keyboard bindings,
mouse/wheel bindings and event-driven pointer focus are retained. Other Phase 2 work still includes
interactive screenshot selection/pointer flags and the explicit partial rows in both matrices.

**Phase 2C, Lane D ([report](phase2c-lane-d-report.md)):** focused-output and focused-window PNG
screenshots, the clipboard, `screenshot-path` and capture events/notifications are implemented.
H26-H28 verify both hotkeys, saved PNG content, clipboard and null-on-reload live. Output
identity matching, startup focus, reload-to-real-DPI and explicit-current-mode no-op pass H29;
physical mode/position/rotation/off changes are synthetic-unit-tested only and never exercised
on this laptop. Screenshot selection UI/action flags and output limitations remain explicit in
the matrices/report. Other Phase 2 lanes are separate work.

**Phase 3, shine:** elevated windows (uiAccess or a task launch), per-user pipe names,
ARM64 release builds, docs and a demo. Lane D fixed always-quoted Run autostart command
construction (four pure tests; no real registry/logon test) and documented elevation choices
in [ELEVATION.md](../ELEVATION.md); it does not enable elevation.

**Phase 4, super lightweight and fast (owner, 2026-10-24: "full parity and functionality,
super lightweight performant after core functionality is done"):** once the core works,
measure first (added to the live test as info gates), then enforce budgets:
- idle CPU about 0: event-driven only, no polling timers (today a 750 ms reconciler, a
  500 ms focus-follows-mouse poll and overlay timers wake wiri while nothing happens);
- few threads (the first live run showed 23 at idle; `tokio` is built with `full`), small
  private memory and binary, fast cold start to the first tiled layout;
- hotkey-to-`SetWindowPos` latency within one frame (16 ms) and no extra layout passes
  per event (count `SetWindowPos` calls per action);
- measured on the owner's ARM64 laptop as well as on x64.

## How changes are verified

`scripts/live-test.ps1` runs in an interactive desktop session: it starts the freshly
built `wiri.exe`, opens test windows and terminals, drives wiri over `wiri-ctl` and real key
presses, and writes `summary.json` plus screenshots. A change is done when `cargo test
--release` and `cargo check --release --all-targets` with `RUSTFLAGS=-D warnings` pass and
the live gates hold (no repositioning once settled, every test window tiled without
overlap, no IPC timeouts, no unknown-key warnings, every window visible after quit and
after a crash restart).  A live run holds a machine-wide lock, so two runs can never overlap
and spoil each other.
