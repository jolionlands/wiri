# wiri Implementation Progress

## Build Status
- ✅ Compiles cleanly (release build, all targets)
- ✅ 330+ unit tests across config / ipc / layout / overlay / window / utils
- ✅ Release build: 2.4 MB (wiri.exe), 710 KB (wiri-ctl.exe)
- ✅ 40+ source files, ~9 000 lines of Rust
- ✅ Runs on aarch64-pc-windows-msvc (ARM64) and x86_64-pc-windows-msvc

## What's Implemented

### Core Architecture
- ✅ **Backend** — Window/monitor enumeration, WinEvent hooks, DPI awareness
- ✅ **Layout Engine** — Multi-monitor, multi-workspace, scrollable-tiling
- ✅ **Proportional & Fixed column widths** — `ColumnWidthMode::Proportional` (default) / `Fixed`
- ✅ **Per-column variable widths** — `Column.width` hint, respected in Fixed mode
- ✅ **Scroll-to-focus** — Auto-scrolls when focusing columns
- ✅ **Fullscreen toggle** — Saves/restores window styles
- ✅ **Floating toggle** — Removes from tiling, re-adds on un-float
- ✅ **Workspace switching** — Hides/shows windows per workspace
- ✅ **Focus renderer** — DWM border color (Win11+)
- ✅ **Window rules** — Auto-float, per-workspace, opacity on creation
- ✅ **IPC** — Named pipe server with real engine state queries
- ✅ **Tray icon** — Shell_NotifyIconW, context menu, config reload
- ✅ **KDL config** — Full parser with key=value and "value" syntax
- ✅ **KDL bind section** — Custom keybindings from config file
- ✅ **Config→Layout bridge** — `LayoutConfig::from_config()`, live reload

### Input System
- ✅ **Hotkey registration** — `RegisterHotKey` without MOD_NOREPEAT (ARM64-safe)
- ✅ **Software key-repeat debounce** — 125ms (8 actions/sec)
- ✅ **Default hotkeys** — 22 bindings (Ctrl+Alt prefix), configurable via KDL
- ✅ **Hot-reloading keybindings** — `WM_RELOAD_HOTKEYS` message to hotkey thread
- ✅ **Low-level mouse hook** — `WH_MOUSE_LL` for interactive move/resize
- ✅ **Alt+Left-click drag** — Move grab (re-tiles on release)
- ✅ **Alt+Right-click edge drag** — Resize grab (re-tiles on release)
- ✅ **Mouse focus tracking** — Focus-follows-mouse with configurable delay
- ✅ **Interactive grab state machine** — `MoveGrab`, `ResizeGrab`, edge detection
- ✅ **Shared key/action parsing** — `parse_action_name()`, `parse_key_name()`, `parse_modifiers()`

### Animation System
- ✅ **Animation framework** — `AnimationManager`, `Animation`, `Easing` enum
- ✅ **6 easing functions** — None, Linear, EaseIn, EaseOut, EaseInOut, CubicOut
- ✅ **Tick-based interpolation** — `Animation::tick(delta_ms)`, `AnimationManager::tick()`
- ✅ **Animation targets** — ScrollX, WindowX/Y/W/H, Opacity
- ✅ **Config-driven** — Enabled/duration/easing from KDL config
- ✅ **8 unit tests** for animation math

### Multi-DPI Support
- ✅ **Per-monitor scale factor** — `Monitor.scale_factor: f64` from `GetDpiForMonitor`
- ✅ **`register_monitor_with_scale()`** — Engine stores DPI per monitor
- ✅ **`PER_MONITOR_AWARE_V2`** — Set in main before window operations

### Hotkey Bindings (configurable via KDL binds section)

| Key | Action |
|-----|--------|
| ←/→ | Focus column left/right |
| ↑/↓ | Focus up/down in column |
| Shift+←/→ | Move column left/right |
| Q | Close window |
| Enter | Open terminal (cmd.exe) |
| F | Toggle fullscreen |
| T | Toggle floating |
| H/L | Scroll workspace left/right |
| Shift+Q | Quit |
| 1-9 | Switch workspace |
| Alt+Left-drag | Interactive move |
| Alt+Right-drag edge | Interactive resize |

## New in This Session

### Cleanup (54 warnings → 0)
- ✅ Removed `#![allow(unused)]` blanket from lib.rs
- ✅ Fixed all unused import/variable/must_use warnings
- ✅ Fixed unreachable patterns in resize_edge_from_point
- ✅ Removed dead `InputHandler` (message_loop handles all actions)

### KDL Bind Parsing
- ✅ `parse_bind_line()` — parses `Ctrl+Alt+Left focus-column-left` syntax
- ✅ `build_hotkey_list()` — uses config binds, falls back to defaults
- ✅ `TilingEngine::set_full_config()` stores full Config for binds access
- ✅ Default config includes complete `binds {}` section

### Low-Level Mouse Hook (NEW)
- ✅ `input/low_level_hook.rs` — WH_MOUSE_LL global hook
- ✅ `GrabState` enum — None/Move/Resize
- ✅ Alt+Left-click on tiled window → move grab
- ✅ Alt+Right-click on window edge → resize grab
- ✅ Grab consumes mouse events, re-tiles on button release
- ✅ SendHwnd/SendHook wrappers for thread-safe Win32 handles

### Per-Column Variable Widths (NEW)
- ✅ `Column.width: Option<u32>` — explicit width hint
- ✅ `add_window_to_new_column_with_width()` — passes window width
- ✅ `calculate_positions()` rewritten with per-column widths
- ✅ `column_x_and_width()` helper for variable-width scroll calculations
- ✅ `scroll_to_column()` / `clamp_scroll()` support variable widths

### Animation System (NEW)
- ✅ `layout/animation.rs` — full animation framework
- ✅ `AnimationManager` with tick-based updates
- ✅ Config-driven: enabled/duration/easing from KDL
- ✅ 8 unit tests for animation math

### Multi-DPI Aware Layout (NEW)
- ✅ `Monitor.scale_factor: f64` from `GetDpiForMonitor`
- ✅ `Monitor::with_scale()` constructor
- ✅ `register_monitor_with_scale()` in engine
- ✅ Main passes `monitor.scale_factor` to engine

### Hot-Relloading Keybindings (NEW)
- ✅ `WM_RELOAD_HOTKEYS` custom message to hotkey thread
- ✅ `MessageLoop::reload_hotkeys()` — public API for triggering reload
- ✅ Config reload from tray calls `reload_hotkeys()`
- ✅ Hotkey thread unregisters all, re-reads config, re-registers

## Improvements Implemented (2024-05-04)

### Animation Tick Wired into Main Loop (NEW)
- ✅ `TilingEngine::tick_animations(delta_ms)` — advances all animations
- ✅ `TilingEngine::has_active_animations()` — checks if animations running
- ✅ `TilingEngine::animate_scroll()` — triggers scroll animations
- ✅ `TilingEngine::animate_focus_change()` — triggers focus animations
- ✅ Main loop ticks animations at ~60fps (16ms intervals)
- ✅ Layout applied when animations are active for smooth transitions

### Multi-DPI Helper Methods (NEW)
- ✅ `Monitor::physical_to_logical()` — convert physical to logical pixels
- ✅ `Monitor::logical_to_physical()` — convert logical to physical pixels
- ✅ `Monitor::positioning_rect()` — get DPI-aware rect for SetWindowPos
- ✅ Note: With PER_MONITOR_AWARE_V2, coordinates are already logical

### CBT Hook for Pre-Window Events (NEW)
- ✅ `backend/cbt_hook.rs` — CBT hook module
- ✅ `WH_CBT` hook installed via `SetWindowsHookEx`
- ✅ `HCBT_CREATEWND` — fires BEFORE window creation
- ✅ `HCBT_DESTROYWND` — fires BEFORE window destruction
- ✅ `HCBT_ACTIVATE` — fires BEFORE window activation
- ✅ `HCBT_MOVESIZE` — fires BEFORE window move/resize
- ✅ `HCBT_SETFOCUS` — fires BEFORE window gets focus
- ✅ `BackendEvent::Cbt*` variants for all CBT events
- ✅ Optional hook (doesn't fail if unavailable)

## Polish Pass (2026-05)

The polish pass closed the user-facing audit TODOs across IPC, ctl, overlay,
tray, and config.  Highlights:

### `wiri-ctl` (CLI)
- ✅ Human-formatted output by default (`state` shows summary block; `windows`
      shows aligned hwnd/pid/class/ws/title table; `events` streams one event
      per line with HH:MM:SS prefix).
- ✅ Global `--json` flag for scripting (raw JSON body).
- ✅ `--version` now embeds the short git commit hash via `build.rs`.
- ✅ Better connection errors (suggests `is wiri running?` when the pipe is
      missing instead of `os error 2`).
- ✅ New subcommands: `float-window`, `unfloat-window`, `tile-window`,
      `move-window-to-workspace-by-hwnd`.

### IPC server (`src/ipc/`)
- ✅ Per-HWND handlers wired through to the engine (FocusWindow, TileRequest,
      WindowFloat, WindowUnfloat, WindowMoveToWorkspace).  Uses a new
      `focus_window_by_hwnd` helper that walks monitors/workspaces, updates
      focus state, and re-applies the layout.
- ✅ Named pipe ACL: `BUILTIN\Administrators` + Owner only, applied to every
      pipe instance via `ServerOptions::create_with_security_attributes_raw`.
- ✅ `wiri-ctl windows` now reports per-window workspace id + floating flag
      (extra JSON fields beside the typed `WindowInfoIpc`).

### Overlay (`src/overlay/`)
- ✅ Multi-monitor support: new `WorkspaceIndicator::show_at(label, bounds)`
      centres the overlay on a caller-supplied monitor.  Legacy `show(label)`
      keeps the primary-monitor placement for backwards compatibility.
- ✅ 100 ms ease-in fade plus the existing 300 ms fade-out.
- ✅ DWM `DWMWA_WINDOW_CORNER_PREFERENCE = ROUND` on Windows 11; older Windows
      silently ignores the attribute.

### Tray (`src/hooks/tray.rs`)
- ✅ New menu items: "Focus Last Tile", "Open Config Folder…", "About wiri…".
- ✅ Single left-click on the tray icon brings focus to the MRU tile.
- ✅ "Open Config Folder" creates `%APPDATA%\wiri\` if missing then launches
      Explorer there.

### Config (`src/config/`)
- ✅ New flat fields on `InputConfig`: `focus_delay_ms`, `warp_on_focus`,
      `numlock`, `touch_enabled`, `touch_tap_to_click`, `touch_scroll_speed`.
- ✅ New flat fields on `LayoutConfig`: `border_color_urgent`, `border_radius`,
      `border_padding`, `focus_ring_gap`, `focus_ring_inactive_color`,
      `shadow_spread`.
- ✅ KDL parser accepts both kebab-case and snake_case for every new field.
- ✅ `default_config_dir()` exposes `%APPDATA%\wiri\` for the tray menu.
- ✅ Config-reload debounce confirmed 100 ms (matches SPEC.md).

### Docs (`docs/`)
- ✅ `QUICKSTART.md` — 3-minute getting-started guide.
- ✅ `COMPARISON.md` — wiri vs niri vs komorebi vs FancyZones table.

## What's Not Yet Implemented

### Priority (High)
- [x] ~~Wire animation tick into main event loop~~ (DONE)
- [x] ~~Use scale_factor in position calculations~~ (DONE - helpers added)
- [x] ~~Proper CBT hook for intercepting window creation before display~~ (DONE)
- [x] ~~Named-pipe ACL (Admins + owner only)~~ (DONE)
- [x] ~~Per-HWND IPC operations (float / tile / move-to-workspace)~~ (DONE)

### Priority (Medium)
- [ ] Overview mode (show all windows) — scaffolding exists, polish pending
- [ ] Per-workspace scroll offset persistence
- [ ] Window tabbing within columns
- [ ] Screenshot functionality (Spawner has the BMP capture path; needs a hotkey)
- [ ] Hook overlay into main.rs so workspace switching triggers `show_at(...)`

### Priority (Low)
- [x] ~~Touch input fields in config~~ (DONE — input/touch.rs consumer pending)
- [ ] Touch swipe gesture handler
- [ ] Wallpaper/accent color integration
