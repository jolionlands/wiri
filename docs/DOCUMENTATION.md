# wiri — Comprehensive Documentation

> A scrollable-tiling window manager for Windows, inspired by [niri](https://github.com/YaLTeR/niri).
> Written in Rust. ~8,400 lines across 32 source files.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [Installation & Usage](#3-installation--usage)
4. [Configuration Guide](#4-configuration-guide)
5. [Key Bindings](#5-key-bindings)
6. [IPC Protocol](#6-ipc-protocol)
7. [Module Reference](#7-module-reference)
8. [Data Structures](#8-data-structures)
9. [Event Flow](#9-event-flow)
10. [Developer Guide](#10-developer-guide)
11. [Testing](#11-testing)

---

## 1. Overview

**wiri** is a native Windows window manager that arranges windows in **columns on an infinite horizontal strip** — opening new windows never causes existing windows to resize. It brings the scrollable-tiling paradigm from the Linux world (niri) to Windows.

### Key Characteristics

| Feature | Description |
|---|---|
| **Scrollable Tiling** | Windows arranged in columns extending infinitely to the right |
| **Dynamic Workspaces** | Per-monitor workspaces arranged vertically (like GNOME) |
| **Multi-Monitor** | Each monitor has its own independent window strip; workspaces preserved across reconnects |
| **Floating Windows** | Toggle any window to/from floating mode |
| **Configurable** | KDL configuration file with hot-reload support |
| **Windows 11 Focus** | Uses `DWMWA_BORDER_COLOR` for colored window borders on Win11+ |
| **IPC** | Named pipe control interface (`\\.\pipe\wiri_control`) |
| **Animation Framework** | Tick-based interpolation with 6 easing functions |
| **Interactive Mouse** | Alt+drag for move/resize via `WH_MOUSE_LL` global hook |

### Project Status

- ✅ Compiles with zero warnings/errors
- ✅ 59 unit tests passing
- ✅ Runs on `aarch64-pc-windows-msvc` (ARM64)
- ✅ 22 default hotkeys (Ctrl+Alt prefix)
- ✅ Full tiling lifecycle (add, remove, focus, move, scroll, fullscreen, float)
- ✅ Interactive move/resize via mouse (Alt+Left/Right drag)
- ✅ DWM border color focus rendering (Windows 11+)
- ✅ Configurable keybindings via KDL `binds {}` section
- ✅ KDL configuration parser with `key=value` and `key "value"` syntax
- ✅ File watcher for live config reload
- ✅ Tray icon with `Shell_NotifyIconW`
- ✅ Animation framework (framework complete, not yet driving layout ticks)
- ✅ Multi-DPI aware (scale factors stored, not fully applied to pixel calculations yet)

---

## 2. Architecture

### High-Level Flow

```
┌─────────────────────────────────────────────────────────────┐
│                      wiri (main.rs)                         │
│                                                              │
│  Config → LayoutConfig → TilingEngine ──→ apply_layout()    │
│     ↑                                      │                 │
│     │                                      ▼                 │
│  Backend (Win32) ←── WinEventHook ─── BackendEvent          │
│     ↑                                      │                 │
│     │                                      ▼                 │
│  MessageLoop (hotkeys) ──→ Action → TilingEngine            │
│                                                              │
│  MouseHook (WH_MOUSE_LL) ──→ GrabState → Move/Resize        │
│                                                              │
│  IPC Server ←── Named Pipe ──→ wiri-ctl                     │
│                                                              │
│  TrayIcon ←── Context Menu ──→ Reload/Quit                  │
└─────────────────────────────────────────────────────────────┘
```

### Module Dependency Graph

```
main.rs
  ├── backend/ ──────────── sends BackendEvent to event handler
  ├── layout/
  │   ├── engine.rs ─────── core tiling logic
  │   ├── workspace.rs ──── Workspace, Column, Tile
  │   ├── mod.rs ────────── Monitor struct
  │   ├── floating.rs ───── FloatManager
  │   ├── borders.rs ────── FocusRenderer
  │   └── animation.rs ──── Animation framework
  ├── window/
  │   ├── mod.rs ────────── Mapped/Unmapped/WindowRef/WindowSet
  │   └── rules.rs ──────── rule resolution
  ├── input/
  │   ├── mod.rs ────────── Action enum, parsers
  │   ├── hotkey.rs ─────── RegisterHotKey management
  │   ├── grab.rs ───────── MoveGrab, ResizeGrab
  │   ├── mouse.rs ──────── MouseTracker
  │   └── low_level_hook.rs ── WH_MOUSE_LL global hook
  ├── config/
  │   ├── types.rs ──────── Config structs
  │   ├── parse.rs ──────── KDL parser
  │   ├── loader.rs ──────── file I/O, file watcher
  │   └── error.rs ──────── error types
  ├── hooks/
  │   ├── mod.rs ────────── SystemIntegration
  │   ├── tray.rs ───────── tray icon
  │   ├── spawner.rs ────── process spawner
  │   └── startup.rs ────── auto-start
  ├── ipc/
  │   └── mod.rs ────────── named pipe server, message types
  ├── utils/
  │   ├── id.rs ─────────── OutputId, WindowId
  │   └── rect.rs ───────── Point, Size, Rect
  └── bin/ctl.rs ────────── CLI client binary
```

### Threading Model

wiri uses several threads:

1. **Main (Tokio async) thread** — Config loading, engine initialization, event handling, IPC server, mouse focus tracking, periodic ticks
2. **Message loop thread** — Windows message pump (`GetMessageW`), hotkey dispatch (`WM_HOTKEY`), key repeat debouncing
3. **WinEvent hook thread** — Receives window event callbacks from `SetWinEventHook`
4. **Config watcher thread** — `notify` file watcher for config reload
5. **Mouse hook thread** — Low-level `WH_MOUSE_LL` global mouse hook

Shared mutable state is protected by `Arc<RwLock<T>>`:
- `TilingEngine` — wrapped in `Arc<RwLock<>>`
- `FocusRenderer` — wrapped in `Arc<RwLock<>>`
- `BackendHandle` — fully cloneable, thread-safe via internal `Arc<RwLock<>>`

---

## 3. Installation & Usage

### Requirements

- Windows 10 1809+ or Windows 11
- Rust toolchain (for building)

### Building

```sh
git clone <repo>
cd WM/wiri
cargo build --release
```

The build produces two binaries:
- `target/release/wiri.exe` — the window manager (2.4 MB)
- `target/release/wiri-ctl.exe` — IPC control CLI (710 KB)

### Running

```sh
wiri                            # with default config
wiri -c my-config.kdl           # with custom config
wiri -v                         # verbose logging (debug level)
wiri --no-tray                  # disable system tray icon
```

### Hot-reload

Edit `config.kdl` while wiri is running and changes take effect automatically via the file watcher. Configured hotkeys also reload without restarting.

### Logging

Logs are written to stderr. Use `RUST_LOG` env var for fine-grained control:
```sh
RUST_LOG=debug wiri
RUST_LOG=wiri::layout=trace wiri
```

---

## 4. Configuration Guide

wiri uses the **KDL** document format (`.kdl` file). The config file path defaults to `config.kdl` in the current directory and can be overridden with `-c <path>`.

### Full Default Config (Conceptual)

```kdl
// ── Input ──
input {
    keyboard-layout "us"
    repeat-delay 250
    repeat-rate 25
    mouse-speed 1.0
    mouse-acceleration 0.0
    natural-scroll false
    tap-to-click false
}

// ── Layout ──
layout {
    column-width 500
    column-width-mode "proportional"   // "proportional" or "fixed"
    inner-gaps 16
    outer-gaps 8
    border-width 4
    border-color "#333333"
    border-color-focused "#0066cc"
    focus-ring-width 3
    focus-ring-color "#00aaff"
    dim-unfocused 1.0
    scroll-step 200
    split-ratio 0.5
    auto-balance false
}

// ── Animations ──
animations {
    enabled false
    duration 200
    easing "cubic-bezier"
    focus-transition true
    workspace-transition true
    window-move true
    window-resize true
}

// ── Key Bindings ──
binds {
    Ctrl+Alt+Left focus-column-left
    Ctrl+Alt+Right focus-column-right
    Ctrl+Alt+Up focus-up
    Ctrl+Alt+Down focus-down
    Ctrl+Alt+Shift+Left move-column-left
    Ctrl+Alt+Shift+Right move-column-right
    Ctrl+Alt+Q close-window
    Ctrl+Alt+Enter spawn "cmd.exe"
    Ctrl+Alt+F toggle-fullscreen
    Ctrl+Alt+T toggle-floating
    Ctrl+Alt+H scroll-left
    Ctrl+Alt+L scroll-right
    Ctrl+Alt+Tab overview
    Ctrl+Alt+O overview-select
    Ctrl+Alt+Shift+Q quit
    Ctrl+Alt+1 switch-workspace 1
    Ctrl+Alt+2 switch-workspace 2
    Ctrl+Alt+3 switch-workspace 3
    Ctrl+Alt+4 switch-workspace 4
    Ctrl+Alt+5 switch-workspace 5
    Ctrl+Alt+6 switch-workspace 6
    Ctrl+Alt+7 switch-workspace 7
    Ctrl+Alt+8 switch-workspace 8
    Ctrl+Alt+9 switch-workspace 9
}

// ── Window Rules ──
window-rules {
    class "CalcFrame" {
        float true
    }
    class "CASCADIA_HOSTING_WINDOW_CLASS" {
        workspace "2"
    }
    title "MyApp" {
        opacity 0.85
    }
}

// ── Outputs (Monitors) ──
output "\\\\.\\DISPLAY1" {
    mode "1920x1080@60"
    scale 1.0
    position 0 0
    primary true
}

output "\\\\.\\DISPLAY2" {
    mode "1920x1080@60"
    scale 1.0
    position 1920 0
}
```

### Config `input {}` Block

| Property | Type | Default | Description |
|---|---|---|---|
| `keyboard-layout` | string | `"us"` | Keyboard layout identifier |
| `repeat-delay` | u32 | `250` | Key repeat initial delay (ms) |
| `repeat-rate` | u32 | `25` | Key repeat rate (repeats/sec) |
| `mouse-speed` | f64 | `1.0` | Mouse sensitivity multiplier |
| `mouse-acceleration` | f64 | `0.0` | Mouse acceleration factor |
| `natural-scroll` | bool | `false` | Reverse scroll direction |
| `tap-to-click` | bool | `false` | Enable tap-to-click |

### Config `layout {}` Block

| Property | Type | Default | Description |
|---|---|---|---|
| `column-width` | u32 | `500` | Default column width (px) |
| `column-width-mode` | string | `"proportional"` | `"proportional"` (fill screen) or `"fixed"` (per-column width) |
| `inner-gaps` | u32 | `16` | Gap between columns and stacked tiles (px) |
| `outer-gaps` | u32 | `8` | Margin around the monitor edges (px) |
| `border-width` | u32 | `4` | Window border thickness (px) |
| `border-color` | string | `"#333333"` | Unfocused window border color (hex) |
| `border-color-focused` | string | `"#0066cc"` | Focused window border color (hex) |
| `focus-ring-width` | u32 | `3` | Extra inset for focused windows (px) |
| `focus-ring-color` | string | `"#00aaff"` | Focus ring color (hex) |
| `dim-unfocused` | f32 | `1.0` | Opacity multiplier for unfocused windows (0.0–1.0) |
| `scroll-step` | u32 | `200` | Pixels scrolled per scroll action |
| `split-ratio` | f64 | `0.5` | Default split ratio for new columns |
| `auto-balance` | bool | `false` | Auto-balance column widths |

### Config `animations {}` Block

| Property | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Master animation toggle |
| `duration` | u32 | `200` | Animation duration (ms) |
| `easing` | string | `"cubic-bezier"` | Easing function: `none`, `linear`, `ease-in`, `ease-out`, `ease-in-out`, `cubic-bezier` |
| `focus-transition` | bool | `true` | Animate focus changes |
| `workspace-transition` | bool | `true` | Animate workspace switches |
| `window-move` | bool | `true` | Animate window moves |
| `window-resize` | bool | `true` | Animate window resizes |

### Config `window-rules {}` Block

Each rule has **matchers** (criteria to match a window) and **properties** (applied on match).

**Matchers:**

| Property | Type | Description |
|---|---|---|
| `class` | string | Match window class name |
| `title` | string | Match window title |
| `instance` | string | Match window instance name |
| `pid` | u32 | Match process ID |

**Properties:**

| Property | Type | Default | Description |
|---|---|---|---|
| `float` | bool | `false` | Start window as floating |
| `workspace` | string | — | Assign to specific workspace |
| `opacity` | f64 | `1.0` | Window opacity (0.0–1.0) |
| `blur` | bool | `false` | Enable background blur |
| `sticky` | bool | `false` | Appear on all workspaces |
| `scale` | f64 | — | Custom scale factor |

### Config `binds {}` Block

Each line: `<Modifiers>+<Key> <Action> [args...]`

**Modifiers:** `Ctrl`, `Alt`, `Shift`, `Win` (case-insensitive, any order)

**Syntax examples:**
```kdl
binds {
    Ctrl+Alt+Left focus-column-left
    Ctrl+Alt+F toggle-fullscreen
    Ctrl+Alt+Enter spawn "cmd.exe"
    Ctrl+Alt+1 switch-workspace 1
}
```

If a `binds {}` section exists, the defaults are **replaced entirely** (not merged).

### Config `output {}` Blocks

| Property | Type | Default | Description |
|---|---|---|---|
| `mode` | string | — | Display mode (e.g. `"1920x1080@60"`) |
| `scale` | f64 | `1.0` | DPI scale factor |
| `position` | i32 i32 | `0 0` | Monitor position (x y) |
| `primary` | bool | `false` | Set as primary monitor |

---

## 5. Key Bindings

### Default Key Bindings

All default bindings use the `Ctrl+Alt` prefix:

| Key | Action | Description |
|---|---|---|
| ← / → | `focus-column-left` / `focus-column-right` | Focus adjacent column |
| ↑ / ↓ | `focus-up` / `focus-down` | Focus tile above/below in current column |
| Shift+← / Shift+→ | `move-column-left` / `move-column-right` | Move entire column left/right |
| Q | `close-window` | Close focused window |
| Enter | `spawn cmd.exe` | Open terminal |
| F | `toggle-fullscreen` | Toggle focused window fullscreen |
| T | `toggle-floating` | Toggle focused window floating |
| H | `scroll-left` | Scroll workspace left |
| L | `scroll-right` | Scroll workspace right |
| Tab | `overview` | Toggle overview (zoom out) mode |
| O | `overview-select` | Select window and exit overview |
| Shift+Q | `quit` | Exit wiri |
| 1–9 | `switch-workspace 1`–`9` | Switch to workspace |

### Mouse Bindings

| Action | Description |
|---|---|
| **Alt + Left-click drag** | Interactive move — drag a tiled window to reorder columns |
| **Alt + Right-click drag (edge)** | Interactive resize — drag window edges to resize |

### Supported Key Names

| Category | Keys |
|---|---|
| **Letters** | `a`–`z` (case-insensitive) |
| **Digits** | `0`–`9` |
| **Arrows** | `Left`, `Right`, `Up`, `Down` |
| **Arrow aliases** | `Arrow-Left`, `Arrow-Right`, `Arrow-Up`, `Arrow-Down` |
| **Special** | `Enter`, `Return`, `Space`, `Tab`, `Escape`, `Esc`, `Backspace`, `Delete`, `Del`, `Home`, `End`, `Page-Up`, `Prior`, `Page-Down`, `Next`, `Insert`, `Ins` |
| **Lock** | `Caps-Lock`, `Num-Lock`, `Scroll-Lock` |
| **Function** | `F1`–`F12` |
| **Other** | `Print`, `Print-Screen`, `Pause` |

### Supported Action Names

| Action Name | Aliases | Params | Description |
|---|---|---|---|
| `focus-column-left` | `focus-left` | — | Focus column left |
| `focus-column-right` | `focus-right` | — | Focus column right |
| `focus-up` | — | — | Focus tile above |
| `focus-down` | — | — | Focus tile below |
| `move-column-left` | `move-left` | — | Move column left |
| `move-column-right` | `move-right` | — | Move column right |
| `close-window` | `close` | — | Close focused window |
| `toggle-fullscreen` | `fullscreen` | — | Toggle fullscreen |
| `toggle-floating` | `float` | — | Toggle floating |
| `scroll-left` | — | — | Scroll workspace left |
| `scroll-right` | — | — | Scroll workspace right |
| `quit` | `exit` | — | Exit wiri |
| `switch-workspace` | `workspace` | `n` | Focus workspace `n` (1–9) |
| `move-workspace` | — | `n` | Move window to workspace `n` |
| `spawn` | `exec` | `"cmd"` | Launch a program |
| `maximize` | — | — | Maximize window |
| `minimize` | — | — | Minimize window |
| `center-window` | — | — | Center window |
| `switch-monitor` | — | — | Focus next monitor |
| `refresh` | — | — | Re-apply layout |
| `overview` | `overview-toggle`, `zoom-out` | — | Enter/exit overview |
| `overview-left` | — | — | Navigate left in overview |
| `overview-right` | — | — | Navigate right in overview |
| `overview-select` | `overview-accept` | — | Select window, exit overview |

---

## 6. IPC Protocol

wiri exposes a **named pipe** at `\\.\pipe\wiri_control` for external control via the `wiri-ctl` CLI tool.

### Connection

The pipe uses JSON-serialized messages over a Windows named pipe. Each message is a single JSON object terminated by a newline.

### Message Types

All messages use a `type`+`data` discriminated union format:

#### Requests

**Tile a window:**
```json
{"type":"tile_request","data":{"window_hwnd":123456,"target_workspace":"2"}}
```

**Move a window:**
```json
{"type":"window_move","data":{"window_hwnd":123456,"x":100,"y":200}}
```

**Resize a window:**
```json
{"type":"window_resize","data":{"window_hwnd":123456,"width":800,"height":600}}
```

**Get engine state:**
```json
{"type":"get_state"}
```

**List windows:**
```json
{"type":"window_list"}
```

**Focus a window:**
```json
{"type":"focus_window","data":{"window_hwnd":123456}}
```

**Switch workspace:**
```json
{"type":"switch_workspace","data":{"id":2}}
```

**Close a window:**
```json
{"type":"close_window","data":{"window_hwnd":123456}}
```

**Subscribe to events:**
```json
{"type":"subscribe_events","data":{"event_types":["window_created","window_destroyed"]}}
```

### Using wiri-ctl

```sh
wiri-ctl tile 123456          # Tile a window (by HWND)
wiri-ctl workspace 2          # Switch to workspace 2
wiri-ctl list                 # List all windows
wiri-ctl focus 123456         # Focus a window
wiri-ctl close 123456         # Close a window
wiri-ctl state                # Dump engine state
```

---

## 7. Module Reference

### 7.1 Backend (`src/backend/`)

Encapsulates all direct Win32 API interactions.

#### `mod.rs` — Core Backend Types

**`Backend`** — Main backend struct. On construction:
1. Enumerates all monitors via `EnumDisplayMonitors`
2. Enumerates all visible windows via `EnumWindows`
3. Starts `WinEventHook` for real-time window events

```rust
// Key methods
Backend::new() -> Result<Self>;
Backend::handle(&self) -> BackendHandle;
Backend::take_event_rx(&mut self) -> UnboundedReceiver<BackendEvent>;
Backend::run(&mut self) -> Result<()>;  // Blocks (message loop)
```

**`BackendHandle`** — Clonable, thread-safe handle for backend interaction:

| Method | Description |
|---|---|
| `get_windows()` | Return all tracked windows |
| `get_window(hwnd)` | Get window by HWND |
| `get_monitors()` | Return all monitors |
| `get_monitor(id)` | Get monitor by ID |
| `get_primary_monitor()` | Return primary monitor |
| `set_window_position(hwnd, rect, flags)` | Position a window (with 3 fallback strategies) |
| `show_window(hwnd, show)` | Show/hide a window |
| `send_event(event)` | Send a BackendEvent |
| `add_window/remove_window/update_window()` | Manage window cache |

**`WindowInfo`** — Full window metadata:

| Field | Type | Description |
|---|---|---|
| `hwnd` | `isize` | Window handle (raw) |
| `title` | `String` | Window title text |
| `class_name` | `String` | Window class name |
| `process_id` | `u32` | Owning process PID |
| `bounds` | `Rect` | Window position and size (DWM-aware) |
| `state` | `WindowState` | Normal/Minimized/Maximized/Fullscreen |
| `is_visible` | `bool` | Visibility flag |

**`MonitorInfo`** — Monitor metadata:

| Field | Type | Description |
|---|---|---|
| `id` | `OutputId` | Unique identifier |
| `name` | `String` | Device name (e.g. `\\.\DISPLAY1`) |
| `bounds` | `Rect` | Full monitor bounds |
| `work_area` | `Rect` | Usable area (excluding taskbar) |
| `is_primary` | `bool` | Primary monitor flag |
| `scale_factor` | `f64` | DPI scale (1.0 = 96 DPI) |

**`BackendEvent`** — Window events from WinEvent hook:

```rust
pub enum BackendEvent {
    WindowCreated { hwnd: isize },
    WindowDestroyed { hwnd: isize },
    WindowMoved { hwnd: isize, rect: Rect },
    WindowResized { hwnd: isize, rect: Rect },
    WindowShown { hwnd: isize },
    WindowHidden { hwnd: isize },
    MonitorConnected { id: u64 },
    MonitorDisconnected { id: u64 },
    ForegroundChanged { hwnd: isize },
    WindowTitleChanged { hwnd: isize },
}
```

**Window positioning strategy** (in `set_window_position`):

1. **Primary**: `SetWindowPos` with `SWP_ASYNCWINDOWPOS`
2. **Fallback 1**: `MoveWindow`
3. **Fallback 2**: `SetWindowPos` without `SWP_ASYNCWINDOWPOS`

#### `hooks.rs` — WinEvent Hook

Uses `SetWinEventHook` to receive real-time window events:

| Event | Description |
|---|---|
| `EVENT_OBJECT_CREATE (0x8000)` | Window created |
| `EVENT_OBJECT_DESTROY (0x8001)` | Window destroyed |
| `EVENT_OBJECT_SHOW (0x8003)` | Window shown |
| `EVENT_OBJECT_HIDE (0x8004)` | Window hidden |
| `EVENT_OBJECT_LOCATIONCHANGE (0x800B)` | Moved/resized |
| `EVENT_SYSTEM_FOREGROUND (0x0003)` | Focus changed |
| `EVENT_OBJECT_NAMECHANGE (0x800C)` | Title changed |

Uses `WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS` flags. A global `BACKEND_HANDLE` static forwards events as `BackendEvent` messages to the event channel.

#### `message_loop.rs` — Windows Message Pump

Runs a dedicated Windows message loop thread. Responsibilities:

- **Message pump**: `GetMessageW` → `TranslateMessage` → `DispatchMessageW`
- **Hotkey dispatch**: Receives `WM_HOTKEY` messages, converts to `Action` enum, calls engine methods
- **Hotkey registration**: Uses `RegisterHotKey` / `UnregisterHotKey`
- **Software key-repeat debounce**: 125ms interval (8 actions/sec) — avoids `MOD_NOREPEAT` which fails on ARM64
- **Live reload**: Custom `WM_RELOAD_HOTKEYS` message triggers re-read of config binds
- **22 default bindings**: Implemented as `(modifiers, vk_code, Action)` tuples

Key functions:

```rust
fn default_hotkeys() -> Vec<(u32, u32, Action)>;
fn build_hotkey_list(engine: &Arc<RwLock<TilingEngine>>) -> Vec<(u32, u32, Action)>;
```

`MessageLoop::reload_hotkeys()` — Public API: unregisters all, re-reads config, re-registers.

Action dispatch on `WM_HOTKEY`:
- `Quit` → `PostQuitMessage`
- `Spawn` → `CreateProcess` via shell
- `FocusColumnLeft/Right` → `engine.write().focus_left/right()`
- `FocusUp/Down` → `engine.write().focus_up/down()`
- `MoveColumnLeft/Right` → `engine.write().move_column_left/right()`
- `CloseWindow` → `SendMessage(WM_CLOSE)`
- `ToggleFullscreen` → `engine.write().toggle_fullscreen()`
- `ToggleFloating` → `engine.write().toggle_floating()`
- `ScrollLeft/Right` → `engine.write().scroll()`
- `FocusWorkspace(n)` → `engine.write().switch_workspace()`
- `OverviewToggle` → `engine.write().toggle_overview()`

### 7.2 Layout (`src/layout/`)

The layout engine implements the scrollable-tiling paradigm where windows are arranged in columns on an infinite horizontal strip.

#### `engine.rs` — TilingEngine

The central orchestrator for window management.

**Key state fields:**

| Field | Type | Description |
|---|---|---|
| `monitors` | `HashMap<OutputId, Monitor>` | All monitors with workspaces |
| `config` | `LayoutConfig` | Current layout configuration |
| `tiled_windows` | `HashMap<WindowId, WindowInfo>` | All tiled windows |
| `focused_output` | `Option<OutputId>` | Currently focused monitor |
| `fullscreen_windows` | `HashSet<WindowId>` | Fullscreen windows |
| `floating_windows` | `HashSet<WindowId>` | Floating windows |
| `saved_styles` | `HashMap<WindowId, u32>` | Saved window styles for fullscreen restore |
| `failed_position` | `HashSet<WindowId>` | Windows that reject positioning |
| `animation` | `AnimationManager` | Animation state machine |
| `overview` | `Option<f64>` | Overview zoom level (`Some(zoom_factor)`) |
| `window_rules` | `Vec<WindowRule>` | Active window matching rules |

**Key methods:**

| Method | Description |
|---|---|
| `new(config)` | Create empty engine |
| `register_monitor_with_scale(id, bounds, work_area, scale)` | Add a monitor |
| `unregister_monitor(id)` | Remove a monitor |
| `add_window(window_info, backend)` | Add window: check rules, strip frame, apply opacity |
| `remove_window(window_id, backend)` | Remove window, clean up state |
| `apply_layout_for_monitor(output_id, backend)` | Recalculate + apply positions for a monitor |
| `calculate_positions(workspace, work_rect)` | Compute window rects from layout |
| `focus_left/right/up/down()` | Move focus |
| `move_column_left/right(backend)` | Reorder columns |
| `scroll(direction, backend)` | Scroll workspace (with clamping) |
| `toggle_fullscreen(window_id, backend)` | Toggle fullscreen, save/restore styles |
| `toggle_floating(window_id, backend)` | Move in/out of tiling |
| `switch_workspace(id, backend)` | Switch workspace on focused monitor |
| `set_full_config(config)` | Apply window rules from config |
| `set_dwm_border_color(hwnd, color)` | Set DWM border (Windows 11+) |
| `strip_frame_for_tiling(hwnd)` | Remove invisible 7px resize borders |
| `apply_window_opacity(hwnd, opacity)` | Set window opacity |

**Layout calculation algorithm** (`calculate_positions`):

1. Determine proportional or fixed column widths
2. Calculate column X positions with gaps
3. Apply scroll offset (subtract `workspace.scroll_offset.x`)
4. Apply overview zoom (scale and center)
5. Cull off-screen columns (when scroll puts them outside view)
6. Distribute column height evenly among tiles
7. Return `Vec<(WindowId, Rect)>`

**Column width modes:**

- `Proportional`: `(view_width - total_gaps) / num_columns`, minimum `column_width / 2`
- `Fixed`: Uses each column's explicit `width` hint, or falls back to `column_width`

**Overview mode:**
- Zoom < 1.0 scales all columns proportionally
- Columns are centered in the view
- Each column and tile gets centered with padding

#### `mod.rs` — Monitor

Represents a physical display with independent workspaces.

| Field | Type | Description |
|---|---|---|
| `output_id` | `OutputId` | Unique monitor ID |
| `bounds` | `Rect` | Full display bounds |
| `work_area` | `Rect` | Usable area (sans taskbar) |
| `scale_factor` | `f64` | DPI scale (1.0 = 96 DPI) |
| `workspaces` | `HashMap<i32, Workspace>` | Workspace collection |
| `active_workspace` | `i32` | Current workspace ID |
| `focus_column` | `Option<usize>` | Focused column index |
| `focus_window` | `Option<WindowId>` | Focused window |

Key methods: `add_window`, `add_window_with_width`, `remove_window`, `focus_left/right/up/down`, `switch_workspace`.

#### `workspace.rs` — Workspace, Column, Tile

**`Tile`** — A single window in a column:
```
Tile { window_id: WindowId, size: Size }
```

**`Column`** — A vertical stack of tiles:
```
Column { tiles: Vec<Tile>, width: Option<u32> }
```
- `width: Option<u32>` — Explicit width hint for `ColumnWidthMode::Fixed`

**`Workspace`** — A scrollable workspace:
```
Workspace { columns: Vec<Column>, scroll_offset: Point }
```
- `scroll_offset.x` — Horizontal scroll position (modified by `scroll()` action)
- Methods: `add_window_to_new_column()`, `add_window_to_new_column_with_width()`, `add_window_to_column()`, `remove_window()`, `find_window_column()`, `move_window()`

#### `floating.rs` — FloatManager

Manages windows excluded from tiling layout:
- `FloatWindow { window_id, rect, saved_rect }`
- `FloatManager` is a `HashMap<WindowId, FloatWindow>` wrapper with: `insert`, `remove`, `get`, `get_mut`, `contains`, `bring_to_front`

#### `borders.rs` — FocusRenderer

Visual focus indication:

**Windows 11+ strategy:** Sets `DWMWA_BORDER_COLOR` via `DwmSetWindowAttribute` for colored window borders. Resets with `0xFFFFFFFF` (default) on focus loss.

**Fallback strategy:** The engine applies a wider visual gap for focused windows (`border_w + 4` inset vs `border_w`), providing visual distinction even without DWM support.

**Auto-detection:** Detects DWM support on first use and caches result to avoid repeated failed calls.

#### `animation.rs` — Animation Framework

Tick-based interpolation system.

**`Easing` enum:**
```rust
pub enum Easing {
    None,        // Instant transition
    Linear,      // Constant speed
    EaseIn,      // Slow start, fast end
    EaseOut,     // Fast start, slow end
    EaseInOut,   // Slow start and end
    CubicOut,    // Cubic bézier approximation (ease-out-cubic)
}
```

**`Animation` struct:**
```
Animation { start: f64, end: f64, duration_ms: u32, elapsed_ms: u32, easing: Easing, finished: bool }
```
- `tick(delta_ms)` → Advance and return current value
- `current()` → Current value without advancing
- Supports instant transitions via `Animation::instant(end)`

**`AnimationTarget` enum:**
```rust
pub enum AnimationTarget {
    ScrollX, WindowX, WindowY, WindowW, WindowH, Opacity,
}
```

**`AnimationManager`:**
- `animations: HashMap<AnimationTarget, Animation>`
- `tick(delta_ms)` → Advances all animations, returns completed targets
- `start_animation(target, animation)` → Creates/replaces an animation

**6 easing functions implemented:** `apply_easing(t, easing)` — applies the easing curve to normalized time `t` (0.0–1.0).

**Status:** 8 unit tests passing. Not yet wired into the main event loop's periodic tick.

### 7.3 Window (`src/window/`)

Window state tracking, mirroring niri's window lifecycle architecture.

**Key types:**

| Type | Description |
|---|---|
| `HwndWrapper` | Newtype around `HWND` with `Hash + Eq` |
| `WindowState` | `Normal \| Floating \| Maximized \| Fullscreen \| Minimized` |
| `WindowHandle` | Window metadata: `title`, `class_name`, `process_id` |
| `ResolvedWindowRules` | Computed properties: `float`, `workspace`, `column`, `follow_cursor`, `border`, `opacity` |
| `Mapped` | A visible, managed window |
| `Unmapped` | A window waiting for configure |
| `WindowRef` | `Mapped(Mapped) \| Unmapped(Unmapped)` |
| `WindowSet` | Collection of mapped + unmapped windows |

**`Mapped` fields:**
```rust
pub struct Mapped {
    pub id: WindowId,
    pub handle: WindowHandle,
    pub bounds: Rect,
    pub state: WindowState,
    pub rules: ResolvedWindowRules,
    pub activated: bool,
    pub decorated: bool,
}
```

**Window filtering** (`should_skip_window`):

Skips windows that are:
- Desktop windows: `"Progman"`, `"WorkerW"`
- Shell tool windows: `"Shell_TrayWnd"`, `"Shell_SecondaryTrayWnd"`, `"Shell_TrayWindow"`
- Popup menus (`"#32768"`), tooltips (`"tooltips_class32"`)
- Without visible styles or with popup/disabled styles
- Not owned by the current desktop (input desktop)

### 7.4 Input (`src/input/`)

#### `mod.rs` — Action Definitions & Parsers

**`Action` enum** — All possible actions:
```rust
pub enum Action {
    Quit,
    Spawn(String),
    CloseWindow,
    MoveColumnLeft,
    MoveColumnRight,
    FocusColumnLeft,
    FocusColumnRight,
    FocusUp,
    FocusDown,
    FocusWorkspace(i32),
    MoveWorkspace(i32),
    ToggleFullscreen,
    ToggleFloating,
    Maximize,
    Minimize,
    CenterWindow,
    SwitchMonitor,
    Refresh,
    ScrollLeft,
    ScrollRight,
    OverviewToggle,
    OverviewLeft,
    OverviewRight,
    OverviewSelect,
}
```

**Parsers:**

| Function | Signature | Description |
|---|---|---|
| `parse_action_name(name, args)` | `(&str, &[String]) -> Option<Action>` | Convert config string to Action |
| `parse_key_name(name)` | `(&str) -> Option<u32>` | Convert key name to Windows VK code |
| `parse_modifiers(names)` | `(&[String]) -> u32` | Convert modifier names to MOD_* flags |

Supporting types: `ModKey` (`Alt, Ctrl, Shift, Win`), `KeyboardConfig`, `MouseConfig`, `PointerVisibility`.

#### `hotkey.rs` — HotkeyManager

Dynamic hotkey registration using `RegisterHotKey`:

**`HotkeyBinding`** — `{ id: HotkeyId, modifiers: Vec<ModKey>, key_code: u32, action: Action }`
- `modifier_flags()` → Convert to Windows `MOD_*` flags

**`HotkeyManager`** — `{ bindings: HashMap<u32, HotkeyBinding>, next_id: u32 }`
- `register(modifiers, key_code, action)` → `Result<HotkeyId>`
- `unregister(id)` → Remove a binding

**Note:** Primary hotkey loop resides in `message_loop.rs`. `HotkeyManager` exists for future dynamic re-binding support.

#### `grab.rs` — Interactive Move/Resize

**`MoveGrab`** — Tracks window movement:
- Fields: `window_id`, `initial_cursor`, `initial_window_pos`, `initial_column`
- `delta(current_cursor)` → Movement delta from start
- `new_position(current_cursor)` → New window position
- `target_column(current_cursor, col_width, gap, total_cols)` → Determine column at cursor

**`ResizeGrab`** — Tracks window resizing:
- Fields: `window_id`, `edge`, `initial_cursor`, `initial_rect`
- `new_rect(current_cursor)` → New rectangle based on edge+drag

**`ResizeEdge`** — `Left | Right | Top | Bottom | TopLeft | TopRight | BottomLeft | BottomRight`

**`GrabResult`** — `Consumed | Defer | Done`

#### `mouse.rs` — MouseTracker

Tracks cursor position for focus-follows-mouse behavior:
- `update_cursor_pos()` → Query `GetCursorPos`, return current point
- `cursor_pos()`, `cursor_moved()`, `cursor_delta()`
- `MouseFocusConfig`: `focus_follows_mouse`, `warp_on_focus`, `focus_delay_ms`, `raise_on_click`

#### `low_level_hook.rs` — WH_MOUSE_LL Global Hook

Uses `SetWindowsHookExW(WH_MOUSE_LL, ...)` for global mouse interception:

**`GrabState`** — `None | Move(MoveGrab) | Resize(ResizeGrab)`

**Hook flow:**
1. **Alt+Left-click on tiled window** → Begin `MoveGrab`
2. **Alt+Right-click on window edge** → Detect edge → Begin `ResizeGrab`
3. Mouse move while grab active → Update position (consumes event)
4. Button release → End grab, re-tile window

**Thread safety:** Uses `LazyLock<Mutex<HookState>>` for shared state accessed from hook callback and main thread.

### 7.5 Config (`src/config/`)

KDL configuration system with file watching.

#### `types.rs` — Config Structs

| Struct | Description |
|---|---|
| `Config` | Root config: `input`, `output`, `layout`, `workspace`, `window_rules`, `binds`, `animations` |
| `InputConfig` | Keyboard/mouse settings |
| `LayoutConfig` | Tiling appearance and behavior |
| `OutputConfig` | Per-monitor settings |
| `WorkspaceConfig` | Workspace layout settings |
| `WindowRule` | Matching criteria + properties |
| `BindsConfig` | `hotkeys: Vec<HotkeyBinding>` |
| `AnimationsConfig` | Animation settings |
| `HotkeyBinding` | Single keybinding: `modifiers`, `key`, `command`, `args`, `repeat`, `release` |

**`HotkeyBinding` methods:**
- `parse_action()` → `Option<Action>` (delegates to `parse_action_name`)
- `mod_flags()` → `u32` MOD_* flags (delegates to `parse_modifiers`)
- `vk_code()` → `Option<u32>` VK code (delegates to `parse_key_name`)

**Utility:** `parse_color(color_str)` → `Option<[u8; 4]>` RGBA — parses `#RRGGBB` and `#RRGGBBAA`.

#### `parse.rs` — KDL Parser

Parses KDL config text into `Config` struct.

**Supported syntax:**
- `key "value"` — Quoted string value
- `key=value` — Equals syntax
- `section { ... }` — Nested sections
- `// comment` — Line comments
- `key "value" // inline comment` — Inline comments (parsed correctly inside strings)

**Section handling:**
- `input { }` → `InputConfig`
- `layout { }` → `LayoutConfig` (also handles sub-sections `gaps { }`, `borders { }`, `focus-ring { }`, `shadows { }`)
- `animations { }` → `AnimationsConfig`
- `binds { }` → Each line parsed as `HotkeyBinding`
- `window-rules { }` → Each rule block parsed as `WindowRule`
- `output "NAME" { }` → `OutputConfig`

#### `loader.rs` — Config Loader

File-based config loading with hot-reload:
- `ConfigLoader::new(path)` — Load on construction, start watcher
- `load()` / `reload()` — Read and parse config file
- `get_config()` — Return current config (thread-safe via `Arc<RwLock<>>`)
- `start_watching()` — Use `notify` crate to watch config file for changes
- **Debounce**: 300ms debounce between reloads
- **Error handling**: Falls back to `Config::default()` on parse errors

### 7.6 IPC (`src/ipc/`)

Named pipe server for external control.

**`IpcServer`:**
- `new()` → Create server
- `set_engine(engine)` — Bind to engine
- `run()` → Start accepting connections (async)

**`IpcMessage`** — Adjacently-tagged JSON enum:
```rust
pub enum IpcMessage {
    TileRequest { window_hwnd: isize, target_workspace: Option<String> },
    WindowMove { window_hwnd: isize, x: i32, y: i32 },
    WindowResize { window_hwnd: isize, width: u32, height: u32 },
    GetState,
    SubscribeEvents { event_types: Vec<String> },
    WindowList,
    FocusWindow { window_hwnd: isize },
    SwitchWorkspace { id: i32 },
    CloseWindow { window_hwnd: isize },
}
```

**`IpcError`** — Typed error enum: `ConnectionFailed`, `ReadError`, `WriteError`, `SerializationError`, `PipeCreationFailed`, `ClientDisconnected`, `Timeout`, `InvalidMessage`, `WindowsError`.

**Pipe path:** `\\.\pipe\wiri_control`

### 7.7 Hooks (`src/hooks/`)

System-level integrations.

**`SystemIntegration`:**
- `new(backend)` → Initialize: startup manager, spawner, tray icon
- `poll_tray_action()` → Check for tray menu events
- `run()` → Main loop (async)
- `startup_manager()`, `spawner()`, `tray_icon()`

**Components:**
- `TrayIcon` — `Shell_NotifyIconW` system tray icon with context menu (Reload, Quit)
- `Spawner` — Process creation via `CreateProcess`
- `StartupManager` — Registry-based auto-start management

### 7.8 Utils (`src/utils/`)

**`id.rs` — Identity Types:**

**`OutputId(u64)`** — Monitor identifier:
- `from_name(name)` → Hash-based ID from monitor device name
- Deterministic hash: `fold by 31 * byte_value`
- `Display: "Output:{hex}"`

**`WindowId(isize)`** — Window identifier (wraps HWND):
- `new(hwnd)` → Create from raw HWND
- `as_isize()` → Extract raw value
- `Display: "Window:{raw}"`

**`rect.rs` — Geometry Types:**

**`Point`** — `{ x: i32, y: i32 }` with `add`, `sub`, `contains_point`
**`Size`** — `{ w: u32, h: u32 }`
**`Rect`** — `{ loc: Point, size: Size }` with `contains_point`, `center`, `inset`

---

## 8. Data Structures

### Layout Data Model

```
TilingEngine
  └── monitors: HashMap<OutputId, Monitor>
        └── Monitor
              ├── output_id: OutputId
              ├── bounds: Rect
              ├── work_area: Rect
              ├── scale_factor: f64
              ├── active_workspace: i32
              ├── focus_column: Option<usize>
              ├── focus_window: Option<WindowId>
              └── workspaces: HashMap<i32, Workspace>
                    └── Workspace
                          ├── scroll_offset: Point
                          └── columns: Vec<Column>
                                └── Column
                                      ├── width: Option<u32>
                                      └── tiles: Vec<Tile>
                                            └── Tile
                                                  ├── window_id: WindowId
                                                  └── size: Size
```

### Config Data Model

```
Config
  ├── input: InputConfig
  ├── output: Vec<OutputConfig>
  ├── layout: LayoutConfig
  ├── workspace: Vec<WorkspaceConfig>
  ├── window_rules: Vec<WindowRule>
  ├── binds: BindsConfig { hotkeys: Vec<HotkeyBinding> }
  └── animations: AnimationsConfig
```

### Engine Internal State

```
TilingEngine
  ├── monitors: HashMap<OutputId, Monitor>
  ├── config: LayoutConfig
  ├── tiled_windows: HashMap<WindowId, WindowInfo>
  ├── focused_output: Option<OutputId>
  ├── fullscreen_windows: HashSet<WindowId>
  ├── floating_windows: HashSet<WindowId>
  ├── window_sizing: HashMap<WindowId, SizingMode>
  ├── saved_styles: HashMap<WindowId, u32>
  ├── saved_ex_styles: HashMap<WindowId, u32>
  ├── window_rules: Vec<WindowRule>
  ├── full_config: Option<Config>
  ├── animation: AnimationManager
  ├── overview: Option<f64>
  └── failed_position: HashSet<WindowId>
```

---

## 9. Event Flow

### Window Creation

```
WinEventHook (EVENT_OBJECT_CREATE)
  → winevent_proc callback
    → BackendHandle::send_event(BackendEvent::WindowCreated { hwnd })
      → event_rx (tokio mpsc)
        → handle_backend_event()
          → BackendHandle::add_window() (cache)
          → if should_tile_window():
              → engine.write().add_window(window_info, backend)
                → Check WindowRules (float? workspace?)
                → Monitor::add_window_with_width()
                → strip_frame_for_tiling()
                → apply_layout_for_monitor()
                  → calculate_positions()
                  → backend.set_window_position() for each window
                  → set_dwm_border_color() for focused
                  → apply_window_opacity() for dimmed
```

### Window Destruction

```
WinEventHook (EVENT_OBJECT_DESTROY)
  → winevent_proc callback
    → BackendHandle::send_event(BackendEvent::WindowDestroyed { hwnd })
      → handle_backend_event()
        → engine.write().remove_window(window_id, backend)
          → clear fullscreen/floating/sizing state
          → Monitor::remove_window()
          → apply_layout_for_monitor()
```

### Key Binding

```
RegisterHotKey(hwnd=NULL, id, mod_flags, vk_code)
  → WM_HOTKEY { wParam = id, lParam = mod_flags<<16 | vk_code }
    → MessageLoop message pump
      → match registered action:
          Action::FocusColumnLeft → engine.write().focus_left()
          Action::ToggleFullscreen → engine.write().toggle_fullscreen()
          Action::Spawn(cmd) → CreateProcess(cmd)
          // ... etc
      → apply_layout_for_monitor() (if layout changed)
```

### Mouse Move/Resize

```
WH_MOUSE_LL hook → MSLLHOOKSTRUCT
  → Alt key down + WM_LBUTTONDOWN on tiled window
    → begin_move_grab(window_id, cursor, window_pos, column)
    → GrabState::Move(MoveGrab)
  → WM_MOUSEMOVE while grab active
    → update window position via SetWindowPos
    → return 1 (consume event)
  → WM_LBUTTONUP
    → end grab
    → re-tile window at new position
    → GrabState::None
```

### Config Reload

```
notify file watcher detects change
  → ConfigLoader::reload()
    → read file → parse KDL → new Config
    → store in Arc<RwLock<Config>>
    → send via reload_tx channel
  → main loop receives new config
    → engine.write().set_full_config()
    → message_loop.reload_hotkeys()
      → PostThreadMessage(WM_RELOAD_HOTKEYS)
        → message loop: UnregisterHotKey all → read config binds → RegisterHotKey all
    → apply_layout_for_monitor()
```

---

## 10. Developer Guide

### Project Layout

```
WM/wiri/
├── src/
│   ├── main.rs              # Entry point, event loop, initialization
│   ├── lib.rs               # Module declarations
│   ├── backend/             # Win32 window/monitor management
│   │   ├── mod.rs           # Backend, BackendHandle, WindowInfo, MonitorInfo
│   │   ├── hooks.rs         # WinEventHook (SetWinEventHook)
│   │   └── message_loop.rs  # Windows message pump, hotkey action dispatch
│   ├── layout/              # Tiling layout engine
│   │   ├── engine.rs        # TilingEngine — core logic
│   │   ├── workspace.rs     # Workspace, Column, Tile
│   │   ├── mod.rs           # Monitor struct + unit tests
│   │   ├── floating.rs      # FloatManager
│   │   ├── borders.rs       # FocusRenderer (DWM border colors)
│   │   └── animation.rs     # Animation framework
│   ├── window/              # Window state management
│   │   ├── mod.rs           # Mapped/Unmapped/WindowRef/WindowSet
│   │   └── rules.rs         # Window rule resolution
│   ├── input/               # Input handling
│   │   ├── mod.rs           # Action enum, parse functions
│   │   ├── hotkey.rs        # HotkeyManager
│   │   ├── grab.rs          # MoveGrab, ResizeGrab
│   │   ├── mouse.rs         # MouseTracker
│   │   └── low_level_hook.rs # WH_MOUSE_LL global hook
│   ├── config/              # KDL configuration
│   │   ├── types.rs         # Config structs
│   │   ├── parse.rs         # KDL parser
│   │   ├── loader.rs        # File loading + watcher
│   │   └── error.rs         # Config errors
│   ├── hooks/               # System integration
│   │   ├── mod.rs           # SystemIntegration
│   │   ├── tray.rs          # Tray icon
│   │   ├── spawner.rs       # Process spawner
│   │   └── startup.rs       # Auto-start
│   ├── ipc/                 # Named pipe IPC
│   │   └── mod.rs           # IpcServer, IpcMessage
│   └── utils/               # Utilities
│       ├── mod.rs           # Re-exports
│       ├── id.rs            # OutputId, WindowId
│       └── rect.rs          # Point, Size, Rect
├── docs/
│   ├── SPEC.md              # Full specification
│   ├── PROGRESS.md          # Implementation progress
│   └── DOCUMENTATION.md     # This file
└── resources/               # Resources
```

### Building and Testing

```sh
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run specific test
cargo test test_monitor_add_window

# Run with logging
RUST_LOG=debug cargo run

# Build with feature flags
cargo build --features "tray"    # tray enabled (default)
cargo build --no-default-features  # without tray support
```

### Adding a New Feature

1. **New hotkey action:**
   - Add variant to `Action` enum in `input/mod.rs`
   - Add parsing in `parse_action_name()`
   - Add dispatch case in `message_loop.rs`
   - Add default binding in `default_hotkeys()`
   - Implement the action in `TilingEngine` (engine.rs)

2. **New config option:**
   - Add field to relevant `*Config` struct in `config/types.rs`
   - Add parsing in `config/parse.rs` (`apply_*_property`)
   - Apply in `LayoutConfig::from_config()` if layout-related

3. **New window rule property:**
   - Add field to `WindowRule` in `config/types.rs`
   - Add parsing in `config/parse.rs`
   - Apply in `engine.rs` `add_window()` / rule resolution

4. **New backend event:**
   - Add variant to `BackendEvent` in `backend/mod.rs`
   - Register new event in `WinEventHook::new()` (hooks.rs)
   - Handle in `handle_backend_event()` (main.rs)

### ARM64 Compatibility Notes

- Do NOT use `MOD_NOREPEAT` (0x4000) — it causes `RegisterHotKey` to fail on ARM64 Windows
- Use software key-repeat debouncing instead (125ms interval in message_loop.rs)
- All Win32 API usage is through the `windows` crate, which handles ARM64 translation

### Code Style

- Follow Rust 2021 edition idioms
- All errors use `anyhow::Result` or typed error enums
- Thread safety via `Arc<RwLock<>>` for shared state
- Use `tracing` for logging (not `println`)
- Prefer `Windows::Win32::*` APIs through the `windows` crate
- Tests in `#[cfg(test)] mod tests {}` blocks within source files

---

## 11. Testing

### Test Organization

Tests are located in `#[cfg(test)] mod tests {}` blocks within the relevant source files:

| Module | Test Count | Focus |
|---|---|---|
| `layout/mod.rs` | ~14 | Monitor operations, focus navigation, workspace switching |
| `layout/animation.rs` | 8 | Animation math, easing functions, tick logic |
| `input/mod.rs` | ~20 | Action parsing, key name parsing, modifier parsing |
| `window/mod.rs` | ~12 | Window set operations, state transitions, HWND conversion |
| `utils/id.rs` | ~9 | OutputId/WindowId creation, equality, hashing |
| Total | ~59 | All passing |

### Running Tests

```sh
# All tests
cargo test

# With output
cargo test -- --nocapture

# Specific module
cargo test monitor_tests

# Specific test
cargo test test_monitor_add_window

# Release mode tests (slower compile, but more realistic)
cargo test --release
```

### Key Test Areas

**Monitor tests:**
- Creating monitors with/without scale factors
- Adding/removing windows, focus state tracking
- Workspace switching and window preservation
- Focus left/right/up/down boundary conditions
- Empty monitor edge cases (no panic on focus ops)

**Animation tests:**
- Easing function correctness
- Tick advancement and completion detection
- Linear, ease-in, ease-out, cubic-out curves
- Instant (zero-duration) animations

**Input parser tests:**
- All action name variants and aliases
- Key name parsing (letters, digits, arrows, special, function keys)
- Modifier parsing (individual, combined, case-insensitive)
- Missing argument handling

**Window set tests:**
- Mapped/unmapped window lifecycle
- Count tracking (mapped, unmapped, total)
- State checks (is_floating, is_fullscreen, etc.)
- Bounds updates

**ID tests:**
- Deterministic OutputId hashing
- WindowId equality and display formatting
- Hash set uniqueness

### Debugging

```sh
# Verbose mode
wiri -v

# Environment variable logging
RUST_LOG=wiri=debug wiri
RUST_LOG=wiri::layout=trace wiri
RUST_LOG=wiri::input=debug wiri

# Check log file
cat wiri_log.txt
```

Common debugging areas:
- `SetWindowPos` failures (logged as debug messages)
- Window filtering (check `should_skip_window` / `should_tile_window`)
- Hotkey registration failures (logged as warnings)
- Config parse errors (logged as warnings, defaults used)

---

## Appendices

### A. Win32 API Reference

| API | Usage | Module |
|---|---|---|
| `EnumWindows` | Enumerate all top-level windows | `backend/mod.rs` |
| `EnumDisplayMonitors` | Enumerate monitors | `backend/mod.rs` |
| `GetMonitorInfoW` | Monitor bounds, work area | `backend/mod.rs` |
| `SetWindowPos` | Position/resize windows | `backend/mod.rs` |
| `MoveWindow` | Fallback window positioning | `backend/mod.rs` |
| `ShowWindow` | Show/hide windows | `backend/mod.rs` |
| `GetWindowTextW` | Window title | `backend/mod.rs` |
| `GetClassNameW` | Window class name | `backend/mod.rs` |
| `GetWindowRect` | Window bounds | `backend/mod.rs` |
| `DwmGetWindowAttribute` | Extended frame bounds | `backend/mod.rs` |
| `DwmSetWindowAttribute` | DWM border color | `layout/borders.rs` |
| `SetWinEventHook` | Real-time window events | `backend/hooks.rs` |
| `RegisterHotKey` | Global hotkeys | `input/hotkey.rs`, `backend/message_loop.rs` |
| `UnregisterHotKey` | Unregister hotkeys | `backend/message_loop.rs` |
| `SetWindowsHookExW(WH_MOUSE_LL)` | Global mouse hook | `input/low_level_hook.rs` |
| `GetCursorPos` | Cursor position | `input/mouse.rs` |
| `GetMessageW` / `DispatchMessageW` | Message pump | `backend/message_loop.rs` |
| `CreateWindowExW` | Hidden message-only window | `backend/message_loop.rs` |
| `Shell_NotifyIconW` | System tray icon | `hooks/tray.rs` |
| `PostThreadMessageW` | Cross-thread messages | `backend/message_loop.rs` |
| `SetProcessDpiAwarenessContext` | Per-monitor DPI | `main.rs` |
| `GetDpiForMonitor` | Monitor DPI | `backend/mod.rs` |
| `SetLayeredWindowAttributes` | Window opacity | `layout/engine.rs` |

### B. File Size Breakdown

| File | Lines | Description |
|---|---|---|
| `src/main.rs` | ~456 | Entry point, event loop |
| `src/backend/mod.rs` | ~340 | Backend + BackendHandle |
| `src/backend/message_loop.rs` | ~420 | Hotkey message pump |
| `src/backend/hooks.rs` | ~120 | WinEvent hook |
| `src/layout/engine.rs` | ~540 | TilingEngine core |
| `src/layout/mod.rs` | ~280 | Monitor + tests |
| `src/layout/workspace.rs` | ~180 | Workspace, Column, Tile |
| `src/layout/floating.rs` | ~120 | FloatManager |
| `src/layout/borders.rs` | ~140 | FocusRenderer |
| `src/layout/animation.rs` | ~240 | Animation framework |
| `src/window/mod.rs` | ~600 | Window state management |
| `src/window/rules.rs` | ~100 | Rule resolution |
| `src/input/mod.rs` | ~380 | Action enum + parsers |
| `src/input/hotkey.rs` | ~180 | HotkeyManager |
| `src/input/grab.rs` | ~200 | MoveGrab, ResizeGrab |
| `src/input/mouse.rs` | ~160 | MouseTracker |
| `src/input/low_level_hook.rs` | ~260 | WH_MOUSE_LL hook |
| `src/config/types.rs` | ~280 | Config structs |
| `src/config/parse.rs` | ~400 | KDL parser |
| `src/config/loader.rs` | ~200 | File watcher |
| `src/config/error.rs` | ~60 | Error types |
| `src/ipc/mod.rs` | ~350 | IPC server |
| `src/hooks/*.rs` | ~300 | System integration |
| `src/utils/*.rs` | ~200 | Geometry + IDs |
| Total | ~8,387 | 32 source files |