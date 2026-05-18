# wiri - Windows Tiling Window Manager Specification

## Project Overview

**wiri** (pronounced "wee-ree") is a scrollable-tiling window manager for Windows, inspired by [niri](https://github.com/niri-wm/niri) - a scrollable-tiling Wayland compositor.

### Key Characteristics

- **Scrollable Tiling**: Windows are arranged in columns on an infinite horizontal strip. Opening a new window never causes existing windows to resize.
- **Per-Monitor Workspaces**: Each monitor has its own independent workspace stack, similar to GNOME's dynamic workspaces.
- **Windows-Native**: Built using Win32 APIs, not a layer on top of existing desktop.
- **Live Configuration**: Hot-reloadable KDL configuration with no restart required.

## Inspiration

wiri ports the core concepts from niri to Windows:
- Scrollable tiling (columns extend infinitely to the right)
- Dynamic workspace management
- Window focus management with MRU (Most Recently Used)
- Configurable visual effects (borders, shadows, focus rings)

## Architecture Overview

```
wiri/
├── src/
│   ├── main.rs              # Entry point, CLI setup
│   ├── lib.rs               # Library entry point
│   │
│   ├── backend/             # Windows API integration
│   │   ├── mod.rs           # Backend trait and WindowsBackend
│   │   ├── message_loop.rs  # Windows message loop (tokio integration)
│   │   └── hooks.rs         # CBT and WinEvent hooks
│   │
│   ├── layout/              # Tiling layout engine
│   │   ├── mod.rs           # Layout, Monitor, Workspace, Column, Tile
│   │   ├── scrolling.rs     # Scrollable tiling logic
│   │   └── floating.rs      # Floating window management
│   │
│   ├── window/              # Window state management
│   │   ├── mod.rs           # WindowRef, Mapped, Unmapped, WindowState
│   │   └── rules.rs         # Window rule matching and resolution
│   │
│   ├── input/               # Input handling (keyboard, mouse, touch)
│   │   ├── mod.rs           # Action enum, InputConfig, HotkeyManager
│   │   ├── hotkey.rs         # RegisterHotKey/UnregisterHotKey
│   │   └── grab.rs          # Move/resize grabs
│   │
│   ├── config/              # Configuration system
│   │   ├── mod.rs           # Config struct, ConfigLoader
│   │   ├── parse.rs         # KDL parsing with knuffel
│   │   └── validate.rs      # Configuration validation
│   │
│   ├── ipc/                 # Named pipe IPC for external control
│   │   ├── mod.rs           # IpcServer, IpcClient
│   │   ├── server.rs        # Named pipe server implementation
│   │   └── codec.rs         # JSON message framing
│   │
│   ├── hooks/               # Windows system integration
│   │   ├── mod.rs           # StartupManager, TrayIcon, Spawner
│   │   ├── startup.rs       # Registry-based autostart
│   │   └── tray.rs          # System tray integration
│   │
│   └── utils/               # Shared utilities
│       ├── mod.rs
│       └── rect.rs          # Rectangle operations
│
├── docs/
│   ├── SPEC.md              # This document
│   └── modules/             # Detailed module specifications
│       ├── BACKEND.md
│       ├── LAYOUT.md
│       ├── WINDOW.md
│       ├── INPUT.md
│       ├── CONFIG.md
│       ├── IPC.md
│       └── HOOKS.md
│
└── resources/
    └── wiri.rc              # Windows resource file
```

## Module Responsibilities

### Backend (`src/backend/`)

The backend module provides the Windows API abstraction layer.

**Core Types:**
- `Backend` trait - abstraction for window management operations
- `WindowsBackend` - concrete implementation using Win32 APIs
- `OutputId` - stable identifier for monitors
- `RenderResult` - rendering status (Submitted, NoDamage, Skipped)

**Key Responsibilities:**
1. Window enumeration via `EnumWindows`
2. Window positioning via `SetWindowPos`
3. True window bounds via `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`
4. Monitor enumeration via `EnumDisplayMonitors`
5. Windows hooks for event interception (`WH_CBT`, `SetWinEventHook`)
6. Message loop integration with tokio

**Event Flow:**
```
Windows Message → Backend Event → Layout → UI Update
                   ↓
              BackendCommand (SetWindowPos, etc.)
```

### Layout (`src/layout/`)

The layout module implements the scrollable tiling algorithm.

**Core Types:**
- `Layout<W>` - main orchestrator with `MonitorSet<OutputId, Monitor<W>>`
- `Monitor<W>` - per-output workspace management
- `Workspace<W>` - vertical stack of columns with scroll offset
- `Column<W>` - vertical stack of tiles
- `Tile<W>` - individual window in the layout

**Key Traits:**
- `LayoutElement` - abstraction for positionable elements (implemented by `Mapped` windows)

**Key Enums:**
- `SizingMode` - Normal, Maximized, Fullscreen
- `ConfigureIntent` - NotNeeded, Throttled, CanSend, ShouldSend
- `ScrollDirection` - Left, Right
- `AddWindowTarget` - Auto, Output, Workspace, Column, NextTo

**Algorithm:**
1. Windows are arranged in columns extending infinitely to the right
2. Opening new windows appends to the active column (no resizing of existing windows)
3. Each monitor has its own workspace strip
4. Workspaces are dynamic - created on demand
5. Scrolling moves the viewport horizontally within a workspace

### Window (`src/window/`)

The window module manages window lifecycle and state.

**Core Types:**
- `WindowId` - wrapper around Win32 `HWND`
- `WindowRef<'a>` - enum referencing either `Mapped` or `Unmapped`
- `Mapped` - visible, layout-managed window
- `Unmapped` - detected but not yet tiled
- `WindowState` - Normal, Floating, Maximized, Fullscreen, Minimized
- `ResolvedWindowRules` - per-window configuration

**Window Filtering:**
Excluded from tiling:
- Invisible windows (`!IsWindowVisible`)
- Tool windows (`WS_EX_TOOLWINDOW`)
- Owned windows (popups, dialogs)
- Shell windows (TaskManager, etc.)
- wiri's own windows

**Lifecycle State Machine:**
```
Unmanaged → Unmapped → Mapped → (Floating | Maximized | Fullscreen)
    ↑          ↓
    └── Window closed/destroyed
```

### Input (`src/input/`)

The input module handles global hotkeys and input device management.

**Core Types:**
- `Action` enum - all possible key binding actions
- `ModKey` enum - Alt, Ctrl, Shift, Win
- `KeyCombo` - combination of modifiers + key
- `HotkeyBinding` - KeyCombo → Action mapping
- `InputConfig` - keyboard, mouse, touch settings

**Hotkey Registration:**
- Uses `RegisterHotKey` with `WM_HOTKEY` messages
- Hotkey IDs mapped to action handlers
- Supports `MOD_NOREPEAT` to prevent auto-repeat

**Input Grab System:**
- `MoveGrab` - intercepts mouse for window movement
- `ResizeGrab` - intercepts mouse for window resizing
- Uses `SetCapture`/`ReleaseCapture` for mouse capture

**Mouse Tracking:**
- `GetCursorPos`/`SetCursorPos` for cursor position
- `TrackMouseEvent` for hover/leave detection
- `GetAsyncKeyState` for modifier state tracking

### Config (`src/config/`)

The config module provides KDL-based configuration parsing.

**Core Types:**
- `Config` - root configuration structure
- `InputConfig` - keyboard, mouse, touch configuration
- `OutputConfig` - monitor configuration (position, scale, mode, VRR)
- `LayoutConfig` - gaps, borders, shadows, focus ring, animations
- `WorkspaceConfig` - named workspace definitions
- `WindowRule` - per-window matching and properties
- `BindsConfig` - key bindings

**Configuration File Location:**
```
%APPDATA%/wiri/config.kdl
```

**KDL Example:**
```kdl
version "1.0"

input {
    keyboard {
        layout "us"
        repeat-delay 600
        repeat-rate 25
    }
    mod-key "Super"
}

output "DP-1" {
    x 0
    y 0
    width 2560
    height 1440
}

layout {
    gaps 16
    border {
        width 4
        color "#ffc87f"
    }
    focus-ring {
        width 4
        active-color "#7fc8ff"
    }
}

workspace "main" {
    layout "bstack"
}

binds {
    Mod+T { spawn "alacritty" }
    Mod+Shift+E { quit }
    Mod+1 { focus-workspace 1 }
    Mod+Left { focus-column-left }
    Mod+Right { focus-column-right }
}
```

**Live Reload:**
- File watcher monitors config file changes
- Debounced reload (100ms) to prevent rapid reloading
- Validation before applying

### IPC (`src/ipc/`)

The IPC module provides named pipe communication for external control.

**Core Types:**
- `IpcServer` - async named pipe server
- `IpcClient` - client connection
- `IpcMessage` - commands (TileRequest, WindowMove, WindowResize, GetState, etc.)
- `IpcEvent` - events (WorkspacesChanged, WindowOpened, WindowClosed, etc.)

**Pipe Path:**
```
\\.\pipe\wiri_control
```

**Message Format:**
```json
{
  "type": "command",
  "method": "focus-window",
  "params": { "hwnd": 12345 }
}
```

**Event Subscription:**
- Clients subscribe to specific event types
- Server broadcasts to all subscribers
- 64-event buffer per client

### Hooks (`src/hooks/`)

The hooks module provides Windows system integration.

**Core Types:**
- `StartupManager` - registry-based autostart
- `TrayIcon` - system tray presence
- `Spawner` - process creation with environment
- `ScreenshotEvent` - screen capture triggers

**Startup Registration:**
```
HKCU\Software\Microsoft\Windows\CurrentVersion\Run
```

**System Tray:**
- Shows wiri icon in notification area
- Menu: Show/Hide, Reload Config, Quit
- Balloon notifications for errors

**Spawn Mechanism:**
- `CreateProcessW` for process creation
- Inherits environment from wiri
- `ShellExecuteExW` for elevated operations

## Windows API Mapping

### Niri (Wayland) → wiri (Win32)

| Concept | Niri (Wayland/Smithay) | wiri (Win32) |
|---------|------------------------|--------------|
| Surface | `WlSurface` | `HWND` |
| Output | `Output` | `HMONITOR` / `OutputId` |
| Workspace | `Space` | `Workspace<W>` |
| Tiling | `Layout<W>` | `Layout<W>` |
| Window Move | Internal compositor action | `SetWindowPos` |
| Window Bounds | `SurfaceData` | `GetWindowRect` + `DwmGetWindowAttribute` |
| Event Hook | Smithay handlers | `SetWindowsHookEx(WH_CBT)` |
| Focus Tracking | `SeatHandler` | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` |

### Key Win32 APIs Used

| Category | API | Purpose |
|----------|-----|---------|
| Window Enum | `EnumWindows` | Build window list |
| Window Position | `SetWindowPos` | Tile windows |
| Window Bounds | `GetWindowRect`, `DwmGetWindowAttribute` | Get window dimensions |
| Monitor Enum | `EnumDisplayMonitors`, `GetMonitorInfoW` | Multi-monitor support |
| Hooks | `SetWindowsHookEx(WH_CBT)` | Window event interception |
| Event Hook | `SetWinEventHook` | Foreground tracking |
| Hotkeys | `RegisterHotKey`, `WM_HOTKEY` | Global shortcuts |
| Mouse Capture | `SetCapture`, `ReleaseCapture` | Move/resize grabs |
| DWM | `DwmGetWindowAttribute` | True frame bounds |
| IPC | `CreateNamedPipeW`, `CreateFileW` | Named pipes |
| Registry | `RegOpenKeyExW`, `RegSetValueExW` | Autostart |
| Process | `CreateProcessW` | Spawn apps |

## Data Structures

### OutputId

Stable identifier for monitors derived from device name.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(u64);
```

### WindowId

Wrapper around Win32 HWND.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(isize);  // HWND is isize in windows-rs
```

### WindowInfo

Cached window metadata.

```rust
pub struct WindowInfo {
    pub hwnd: HWND,
    pub title: String,
    pub class_name: String,
    pub process_id: u32,
    pub bounds: RECT,
    pub state: WindowState,
    pub is_visible: bool,
}
```

### MonitorInfo

Cached monitor metadata.

```rust
pub struct MonitorInfo {
    pub id: OutputId,
    pub name: String,
    pub bounds: RECT,        // monitor rectangle
    pub work_area: RECT,     // excluding taskbar
    pub scale_factor: f32,
    pub is_primary: bool,
}
```

## Event Flow

### Window Creation Event

```
1. New window created by application
2. CBT hook (HCBT_CREATEWND) fires
3. BackendEvent::WindowCreated(hwnd) sent
4. Layout receives event, adds to Unmapped
5. Window rule matching applied
6. Initial position calculated
7. SetWindowPos called to tile window
8. State transitions to Mapped
```

### Window Move Event

```
1. User drags window OR API call
2. WinEventHook (EVENT_OBJECT_LOCATIONCHANGE) fires
3. BackendEvent::WindowMoved(hwnd, new_rect) sent
4. Layout updates internal state
5. If window is floating, no layout adjustment
6. If tiled, layout may need to adjust neighbors
```

### Key Binding Event

```
1. User presses key combination
2. WM_HOTKEY message received
3. HotkeyManager maps ID to Action
4. Action::MoveColumnLeft dispatched
5. Layout receives action
6. Focus changes, SetWindowPos adjusts
7. WM_NCCALCSIZE sent to window
8. Window renders in new position
```

## Configuration Schema

### Input Configuration

```rust
pub struct InputConfig {
    pub keyboard: KeyboardConfig,
    pub mouse: MouseConfig,
    pub touch: TouchConfig,
    pub focus: FocusConfig,
    pub mod_key: ModKey,
}

pub struct KeyboardConfig {
    pub layout: String,
    pub repeat_delay: u32,
    pub repeat_rate: u32,
    pub numlock: bool,
}

pub struct MouseConfig {
    pub scroll_speed: f32,
    pub acceleration: f32,
    pub sensitivity: f32,
    pub natural_scroll: bool,
}
```

### Layout Configuration

```rust
pub struct LayoutConfig {
    pub gaps: Edges<i32>,
    pub border: BorderConfig,
    pub focus_ring: FocusRingConfig,
    pub shadow: ShadowConfig,
    pub animations: AnimationsConfig,
}

pub struct Edges<T> {
    pub left: T,
    pub right: T,
    pub top: T,
    pub bottom: T,
}
```

### Window Rule

```rust
pub struct WindowRule {
    pub matchers: Vec<Matcher>,
    pub properties: WindowProperties,
}

pub enum Matcher {
    ClassName(String),
    Title(String),
    ProcessName(String),
    IsActive,
    IsFloating,
    IsUrgent,
    AtStartup,
}

pub struct WindowProperties {
    pub float: Option<bool>,
    pub default_width: Option<f32>,
    pub default_height: Option<f32>,
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub opacity: Option<f32>,
    pub border: Option<bool>,
    pub focus_ring: Option<bool>,
}
```

## Error Handling

### Error Categories

1. **Windows API Errors** - wrapped in `anyhow::Context`
2. **Configuration Errors** - `thiserror` enum with `Parse`, `Regex`, `Validation` variants
3. **IPC Errors** - `IpcError` with `PipeCreate`, `Connect`, `Serialization` variants
4. **Layout Errors** - `LayoutError` with `MonitorNotFound`, `WindowNotFound` variants

### Error Recovery

- Windows API failures: log and continue with stale data
- Configuration errors: show notification, keep previous config
- IPC failures: disconnect client, continue serving others
- Hook failures: log and attempt re-registration

## Performance Considerations

### Caching Strategy

- Window info cached, invalidated on events
- Monitor info cached, refreshed on display change
- Layout calculations use cached bounds

### Event Coalescing

- Mouse move events throttled to 60fps
- Window position updates batched per frame
- Config file watches debounced at 100ms

### Hook Efficiency

- CBT hook lightweight, processes quickly
- WinEventHook uses `WINEVENT_OUTOFCONTEXT` for cross-process
- Unnecessary hooks uninstalled when not needed

## Testing Strategy

### Unit Tests
- Layout algorithm correctness
- Window rule matching
- Configuration parsing

### Integration Tests
- Window tiling end-to-end
- Multi-monitor scenarios
- Hotkey registration and triggering

### Manual Testing
- Real-world usage scenarios
- Edge cases (fullscreen games, Remote Desktop)

## Security Considerations

### Named Pipe Security

- ACL restricts access to current user + Administrators
- No world-readable pipes

### Registry Access

- HKCU only (no HKLM requiring elevation)
- Graceful handling if registry write fails

### Window Handle Validation

- All HWNDs validated with `IsWindow` before use
- Process verification for critical operations

## Platform Requirements

### Minimum Windows Version
- Windows 10 (1809+) for key APIs
- Windows 11 for newest DWM features

### Dependencies
- Windows API (windows-rs crate)
- tokio runtime
- knuffel for KDL parsing

## Future Considerations

### Potential Features
- Tabbed windows (column tabs)
- Window snapping hints
- Multi-DPI monitor support
- Custom shaders for effects
- XWayland-like legacy app support

### Known Limitations
- Cannot tile fullscreen exclusive games
- UWP apps have limited window control
- Some anti-cheat systems conflict with hooks

## Related Projects

- [niri](https://github.com/niri-wm/niri) - The Wayland compositor that inspired wiri
- [PaperWM](https://github.com/paperwm/PaperWM) - GNOME extension with similar concept
- [Amethyst](https://github.com/ianyh/Amethyst) - Tiling window manager for macOS
- [FloatTile](https://github.com/rdeits/FloatTile.jl) - Julia-based tiling manager