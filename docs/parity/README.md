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
| Layout and behaviour (226 rows) | 2 | 85 | 124 | 15 |
| Config, input and IPC (259 rows) | 11 | 87 | 91 | 70 |

Line numbers in both files refer to commit 1532635. The live fixes since then (up to
4b9d834) closed some rows; each matrix ends with a list of the working-tree edits that
were in flight while it was written.

## Plan

**Phase 0, done (1532635..4b9d834):** wiri runs on a live desktop. No repositioning loop,
IPC never blocks, every window is tiled, focus follows the OS, xkb/niri key names, a
GUI-subsystem build with a log file, crash-safe hidden windows, Escape reaches
applications, the overview no longer freezes, `move-column-left/right` swaps. Verified by
`scripts/live-test.ps1` on a spare Windows 11 laptop.

**Phase 1, safe to daily-drive (P0):** reject unknown modifiers (today `Mod+T` registers a
global `T`) and add `Mod` tied to `mod-key`; `spawn` and `spawn-at-startup` keep their
arguments and search PATH; every action targets the real foreground window, floating ones
included; minimized windows leave no gap; floating windows belong to their workspace;
niri's keymap semantics (`consume-or-expel`, `maximize-column`, first/last moves) and its
action names and argument grammar (`focus-workspace 2`, `set-column-width "+10%"`);
workspace model (index addressing, previous-workspace memory, real named workspaces);
monitor unplug keeps the windows; hide inactive workspaces by DWM cloaking instead of
`SW_HIDE` so windows stay in the taskbar and Alt+Tab.

**Phase 2, niri feel (P1):** a real KDL parser with visible errors; a low-level keyboard
bind engine (Win-key chords, repeat, cooldown); mouse and wheel binds; one overlay layer
for focus ring, border, shadow, tab indicator and insert hint; the spring animation
pipeline; tabs; the fullscreen and maximize model; the window-rule engine; an overview
that zooms out the workspaces and a recent-windows switcher; hotkey overlay; a generic
IPC action request and niri's event stream; PNG and clipboard screenshots.

**Phase 3, shine:** elevated windows (uiAccess or a task launch), per-user pipe names,
quoted autostart path, ARM64 release builds, docs and a demo.

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
after a crash restart).
