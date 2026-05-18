# Changelog

All notable changes to wiri are documented in this file. The format loosely
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- `wiri-ctl validate-config <path>` — parse and validate a config file without
  needing a running daemon
- Live `MatcherContext` flags (`is_active`, `is_floating`, `is_urgent`,
  `at_startup`) now populated from real engine state when resolving window rules
- DWM drop-shadow toggle via `LayoutConfig.shadow_enable` (uses the canonical
  1-px-top `DwmExtendFrameIntoClientArea` trick; applied only on first tile to
  avoid the Windows Terminal blank-paint regression)
- Two-finger pinch gesture coalescing in `input/touch.rs` (frame-coalesced
  WM_POINTER updates emit `Gesture::Pinch { scale }` when distance changes >8 px)

### Changed
- `build.rs` watches `.git/logs/HEAD` so `--version` git hash refreshes on
  every commit (previously stale until `cargo clean`)

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
