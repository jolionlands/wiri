# Changelog

All notable changes to wiri are documented in this file. The format loosely
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added — opus pass I (2026-05-18)
- **Workspace-slide animation engine wiring** — `switch_workspace` now calls
  `AnimationManager::start_workspace_slide(...)` with `+work_area_height` (or
  `-work_area_height` when stepping to a lower workspace) and
  `apply_layout_for_monitor` adds the per-monitor `workspace_slide_offset(oid)`
  to every tile's Y position.  Honours `animations.enabled` and
  `animations.workspace_transition` — disabled = snap, no slide.
- **Per-output `layout {}` engine wiring** — new `TilingEngine.config_per_monitor:
  HashMap<OutputId, LayoutConfig>` populated from every
  `output { layout { … } }` block at `set_full_config` time, plus
  `effective_config(output_id) -> &LayoutConfig` accessor that falls through to
  the global default.  `apply_layout_for_monitor` + `calculate_positions_in_section`
  read through the accessor so per-monitor column-width / border / gap / mode
  overrides take effect on multi-monitor setups.
- **`Action::SpawnCmd(String)`** — new dispatcher arm for `spawn-cmd "<command>"`
  binds.  Routes through `cmd.exe /C "<command>"` so cmd-builtins, env-var
  expansion, and PowerShell one-liners work as the user typed them.
  `HotkeyBinding::parse_action` now returns `SpawnCmd` for the `spawn-cmd` /
  `spawn-sh` aliases.
- **`binds { extend-defaults true | false }` directive (niri parity)** — default
  `true`.  When extending, config binds merge with the shipped defaults; config
  wins on `(mods, vk)` collision, non-colliding config binds are appended.
  Startup logs `Hotkeys: N from defaults, M from config (X overridden)`.
  Fixes the long-standing "new shipped actions disappear after editing config".
- **Engine-level snapshot/restore + `wiri-ctl save-snapshot/load-snapshot/list-snapshots/delete-snapshot`** —
  new `src/layout/snapshot.rs` module serialises monitor → workspace → column →
  tile structure to JSON at `%APPDATA%\wiri\snapshots\<name>.json` (override
  with `WIRI_SNAPSHOT_DIR`).  On restore, HWNDs that no longer exist are
  skipped silently so yesterday's snapshot still loads today.  Snapshot name
  is sanitised to ASCII alphanumerics + `-_.` so `..\\..\\evil` cannot escape
  the dir.  New IPC messages: `SaveSnapshot`, `LoadSnapshot`, `ListSnapshots`,
  `DeleteSnapshot`.
- **Interactive resize mode (niri-`Mod+R` parity)** — `Action::EnterResizeMode`
  toggles `TilingEngine.resize_mode`; while engaged the WM_HOTKEY dispatcher
  re-routes the `Mod+Arrow` chords to grow/shrink the focused column / tile by
  5% and `Esc` exits (Esc is registered globally + state-gated on
  `is_overview() || is_resize_mode()`).  Default chord moves: `Mod+R` becomes
  EnterResizeMode, CenterColumn moves to `Mod+Shift+R`.  New
  `wiri-ctl resize-mode` IPC + ctl subcommand backed by `IpcMessage::ToggleResizeMode`.

### Added
- **Niri-style multi-workspace overview** — `enter_overview` now collects ALL
  non-empty workspaces of the focused monitor, stacks them vertically with a
  50 px gap, and picks a single zoom that fits both width and height. New
  `OverviewState { zoom, workspace_offsets }` replaces the old `Option<f64>`;
  `overview_zoom()` and `overview_state()` accessors retain backwards compat.
  `exit_overview` now hides every tile that doesn't belong to the active
  workspace so the user is back to a single-workspace view.
- **Overview banner overlay** (`src/overlay/overview_banner.rs`) — translucent
  "Overview — Space or Esc to exit" strip pinned at the top of the focused
  monitor while overview is active.  Wired via a process-wide singleton so
  `engine.rs::enter_overview` / `exit_overview` can toggle it without
  threading state through.
- **GPU-hotkey-conflict detector** (`src/backend/hotkey_conflicts.rs`) —
  startup probe (x86_64 only) that logs a WARN when Intel HotKey Service or
  AMD Radeon registry keys are present, since those drivers steal
  `Ctrl+Alt+Arrows` for screen rotation.  No-op on aarch64.
- **`wiri-ctl test-bindings`** — diagnostic subcommand that prints every
  hotkey binding the running daemon successfully registered with Windows
  (chord, modifier mask, VK code, action).  Backed by new
  `IpcMessage::ListBindings` + `ListBindingsResponse { bindings: Vec<BindingInfo> }`
  + `MessageLoop::current_bindings()` helper.
- **State-gated `Escape` overview-exit binding** — registered unconditionally
  so the OS routes Esc to wiri while overview is active, but the WM_HOTKEY
  dispatcher swallows it (no action) when overview is off, keeping Esc
  available to other applications in normal mode.
- TROUBLESHOOTING.md sections: "Known hotkey conflicts on Windows" + "How to
  verify wiri received your keypress".

### Changed
- **Default overview chord moved from `Ctrl+Alt+Tab` to `Ctrl+Alt+Space`**
  (plus `Escape` to exit).  Windows' accessibility task switcher reserves
  `Ctrl+Alt+Tab` system-wide — `RegisterHotKey` succeeds but the OS still
  intercepts the keystroke, so the binding silently never fires.  Default
  fixed in both `resources/default_config.kdl` and the in-code defaults in
  `backend/message_loop.rs::default_hotkeys`.

- `wiri-ctl validate-config <path>` — parse and validate a config file without
  needing a running daemon
- Live `MatcherContext` flags (`is_active`, `is_floating`, `is_urgent`,
  `at_startup`) now populated from real engine state when resolving window rules
- DWM drop-shadow toggle via `LayoutConfig.shadow_enable` (uses the canonical
  1-px-top `DwmExtendFrameIntoClientArea` trick; applied only on first tile to
  avoid the Windows Terminal blank-paint regression)
- Two-finger pinch gesture coalescing in `input/touch.rs` (frame-coalesced
  WM_POINTER updates emit `Gesture::Pinch { scale }` when distance changes >8 px)
- **KDL `include "…"` directive (niri parity)** — pre-processed before parsing;
  paths resolve relative to the including file or absolute; depth capped at 8;
  cycles detected.  New API: `Config::load_from_str_with_base(input, base_dir)`
  in `src/config/loader.rs`
- **Per-output `layout { … }` override block (niri parity)** — `output "DP-1"
  { layout { column-width 800 } }` populates `OutputConfig.layout_override:
  Option<LayoutConfigPartial>`.  `LayoutConfigPartial::apply_to(&base)` folds
  Some-fields onto a base `LayoutConfig` to produce the effective per-monitor
  config
- **niri-parity `open-*` window-rule properties** — `open-on-output`,
  `open-in-workspace`, `open-fullscreen`, `open-floating`, `open-max-bounds`
  (accepts `WxH`, `W H`, `W,H`).  Surfaced on `ResolvedWindowRules`; resolver
  in `src/window/rules.rs` folds them with last-rule-wins semantics
- **Workspace slide animation primitive** —
  `AnimationTarget::WorkspaceX(OutputId)` plus
  `AnimationManager::start_workspace_slide(output, from_px, to_px, duration_ms)`
  and `workspace_slide_offset(output)` reader.  Sign convention matches niri:
  positive = slide in from the right, negative = from the left
- **`spawn-cmd "<command>"` bind (niri-`spawn-sh` parity)** — recognised by
  `HotkeyBinding::is_spawn_cmd()`; `shell_command_argv()` returns
  `["cmd.exe", "/C", "<command>"]` for the dispatcher
- **Output `scale` validation** — values outside `0.5..=4.0` log a warning but
  are still stored (warn-and-keep, matches niri)
- `MAX_INCLUDE_DEPTH` constant (`8`) exposed in `src/config/loader.rs`
- Default config (`resources/default_config.kdl`) documents the new `include`,
  `spawn-cmd`, and per-output `layout {}` blocks with commented examples

### Requests to Agent E (engine wiring not in this pass)
- **`OutputConfig.layout_override` is parsed but not yet applied.** The
  engine currently holds one global `LayoutConfig`.  Wire per-monitor
  effective configs by folding `output.layout_override.as_ref().map(|p|
  p.apply_to(&global))` onto a `HashMap<OutputId, LayoutConfig>` (or store
  the partial on `Monitor` and merge at the layout-calculation site)
- **`AnimationTarget::WorkspaceX` needs a reader in
  `apply_layout_for_monitor`.** During a workspace-switch, call
  `engine.animation.workspace_slide_offset(oid)` and add the f64 to every
  column's X position on the active workspace.  Engine should call
  `start_workspace_slide()` in `switch_workspace(...)` when
  `config.animations.workspace_transition` is true
- **`ResolvedWindowRules.open_on_output / open_in_workspace /
  open_fullscreen / open_max_bounds` are populated but not consumed.** Wire
  them in `engine.rs::add_window` (or the matching `add_window_with_target`
  path): route by `open_on_output` → matching `OutputId`; pick workspace by
  `open_in_workspace`; cap initial bounds by `open_max_bounds`; flip
  fullscreen state when `open_fullscreen == Some(true)`
- **`HotkeyBinding::is_spawn_cmd() / shell_command_argv()` need a
  dispatcher arm** in `backend/message_loop.rs::WM_HOTKEY` handler.  The
  existing `Action::Spawn(cmd)` path can be extended with a sibling
  `Action::SpawnCmd(cmd)` variant in `input/mod.rs`; route it to
  `CreateProcessW` with the argv from `shell_command_argv()`

### Changed
- `build.rs` watches `.git/logs/HEAD` so `--version` git hash refreshes on
  every commit (previously stale until `cargo clean`)
- `Config::load_sync_inner` (in `loader.rs`) now resolves the file's parent
  as the include base directory and routes through `load_from_str_with_base`,
  so on-disk configs gain `include` support automatically

## [0.1.0] — 2026-05-17

Initial public release. See `docs/PROGRESS.md` for the full feature matrix.

### Added
- Scrollable-tiling layout (columns on an infinite horizontal strip)
- Multi-monitor with per-monitor dynamic workspaces
- KDL configuration with `notify`-based hot-reload and 300 ms debounce
- 22 default key bindings (Ctrl+Alt prefix), all rebindable via `binds { … }`
- Interactive move (Alt + left-drag) and edge-resize (Alt + right-drag) via
  `WH_MOUSE_LL` global hook
- Per-monitor DPI awareness (`PER_MONITOR_AWARE_V2`)
- DWM border colour focus rendering on Windows 11+
- Named-pipe IPC at `\\.\pipe\wiri_control` with ACL restricted to the
  current user + Administrators
- `wiri-ctl` companion CLI with 27 subcommands, human-formatted output, and a
  global `--json` opt-out for scripting
- Tray icon with menu: Show/Hide, Focus Last Tile, Open Config Folder,
  Reload Config, About, Take Screenshot, Quit
- Workspace switcher overlay (multi-monitor aware, 100 ms ease-in fade,
  Win11 DWM rounded corners)
- Animation framework (tick-based, 6 easing functions, auto-driven at ~60 fps
  whenever animations are active)
- Window rules with `class`, `title`, `instance`, `pid` matchers; regex via
  `match { regex … }` blocks
- Per-rule properties: `float`, `workspace`, `opacity` (now `Option<f64>`,
  so explicit `opacity 0` is honoured), `blur`, `sticky`, `scale`
- Urgent-window detection via `EVENT_SYSTEM_ALERT` WinEvent hook
- Auto-engage overview zoom when column count exceeds `auto_tile_threshold`
- Position-failure circuit breaker (3 strikes → auto-float; prevents runaway
  `SetWindowPos` retries against UAC and anti-cheat windows)
- Configurable terminal spawn (Mod+Enter; configurable via the `spawn` action
  argument, with `cmd.exe` as final fallback)
- Mouse-wheel workspace scrolling when modifier is held (respects
  `natural_scroll` and `scroll_speed` from config)
- Monitor cycling via the `switch-monitor` action (sorted by X position)
- Screenshot capture to `%USERPROFILE%\Pictures\wiri-<unix>.bmp` (falls back
  to the current working directory), bound to `Ctrl+Alt+P`
- `wiri-ctl list-monitors` for diagnosing multi-monitor / HiDPI configuration
- ARM64-safe hotkey registration: software key-repeat debounce at 125 ms
  instead of `MOD_NOREPEAT` (which fails on aarch64-pc-windows-msvc)
- 329 unit tests, all passing on `aarch64-pc-windows-msvc`

### Known limitations
- Fullscreen-exclusive games cannot be tiled (no compositor can)
- Some UWP apps expose limited HWND control
- A handful of anti-cheat shims dislike `WH_MOUSE_LL` — disable the mouse
  hook in those games

[Unreleased]: https://github.com/jolionlands/wiri/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jolionlands/wiri/releases/tag/v0.1.0
