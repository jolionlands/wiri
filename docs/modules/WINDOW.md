# Window Module Documentation

## Overview

The window module (`src/window/`) manages window states throughout their lifecycle - from discovery via Win32 API through managed visibility to closure. This module provides the foundation for all window operations in wiri.

## Module Structure

```
window/
+-- mod.rs           # WindowRef, WindowId, ResolvedWindowRules, window traits
+-- mapped.rs        # Mapped window state (visible, managed by layout)
+-- unmapped.rs      # Unmapped window state (detected but not tiled)
+-- hooks.rs         # CBT hook registration and window event handling
```

## Core Types

### WindowId

Unique identifier for a window using its Win32 HWND.

```rust
pub struct WindowId(pub HWND);
```

### WindowRef<'a>

Reference to either a mapped or unmapped window.

```rust
pub enum WindowRef<'a> {
    Unmapped(&'a Unmapped),
    Mapped(&'a Mapped),
}
```

### Mapped

Window that is visible and managed by the layout tiling algorithm.

- Positioned and sized by the layout engine via `SetWindowPos`
- Receives input focus
- Has resolved rules applied
- Tracks floating state

```rust
pub struct Mapped {
    pub id: WindowId,
    pub hwnd: HWND,
    pub state: WindowState,
    pub bounds: Rectangle<i32, Physical>,
    pub floating: bool,
    pub resolved_rules: ResolvedWindowRules,
    pub is_urgent: bool,
    pub handle: WindowHandle,
}
```

### Unmapped

Window detected but not yet integrated into the tiling layout.

- Created when `EnumWindows` discovers a window or CBT hook fires
- Waits for initial positioning and sizing
- May have pending window rules

```rust
pub struct Unmapped {
    pub id: WindowId,
    pub hwnd: HWND,
    pub detected_bounds: Rectangle<i32, Physical>,
    pub pending_rules: Option<ResolvedWindowRules>,
    pub handle: WindowHandle,
}
```

### WindowHandle

Wrapper for Win32 window properties used in rule matching.

```rust
pub struct WindowHandle {
    pub hwnd: HWND,
    pub title: Option<Vec<u16>>,        // Raw UTF-16 from GetWindowTextW
    pub class_name: Option<Vec<u16>>,  // Raw UTF-16 from GetClassNameW
    pub process_id: u32,
    pub thread_id: u32,
}

impl WindowHandle {
    pub fn title(&self) -> String;
    pub fn class_name(&self) -> String;
    pub fn app_id(&self) -> Option<String>;  // Via GetWindowThreadProcessId + query
}
```

### WindowState

The visual/operational state of a window.

```rust
pub enum WindowState {
    Normal,      // Standard tiled window
    Floating,    // Floating above tiling layout
    Maximized,   // Maximized (may span workspace or monitor)
    Fullscreen,  // Fullscreen (no borders, covers display)
    Minimized,   // Minimized to taskbar (not managed)
}
```

### ResolvedWindowRules

Fully resolved per-window configuration from rules and system metrics.

```rust
pub struct ResolvedWindowRules {
    pub default_width: Option<i32>,
    pub default_height: Option<i32>,
    pub default_position: Option<Point<i32, Physical>>,
    pub open_on_output: Option<String>,
    pub open_on_workspace: Option<String>,
    pub open_maximized: Option<bool>,
    pub open_fullscreen: Option<bool>,
    pub open_floating: Option<bool>,
    pub open_focused: Option<bool>,
    pub min_width: Option<i32>,
    pub min_height: Option<i32>,
    pub max_width: Option<i32>,
    pub max_height: Option<i32>,
    pub border: BorderRule,
    pub opacity: Option<f32>,
    pub baba_is_float: Option<bool>,
    pub scroll_factor: Option<f64>,
}
```

### Rule Computation

```rust
impl ResolvedWindowRules {
    pub fn compute(rules: &[WindowRule], window: WindowRef, is_at_startup: bool) -> Self;
    pub fn compute_open_floating(&self) -> bool;
    pub fn apply_min_size(&self, min_size: Size<i32, Physical>) -> Size<i32, Physical>;
    pub fn apply_max_size(&self, max_size: Size<i32, Physical>) -> Size<i32, Physical>;
}
```

## Window Discovery

### EnumWindows Discovery

Windows are discovered via `EnumWindows` enumeration:

```rust
pub fn discover_windows() -> Vec<WindowId>;
```

### CBT Hook Integration

Window creation/activation events via `SetWindowsHookEx` with `WH_CBT`:

```rust
pub enum CbtEvent {
    HcbtCreatewnd { hwnd: HWND },
    HcbtDestroywnd { hwnd: HWND },
    HcbtMove { hwnd: HWND, x: i32, y: i32 },
    HcbtSize { hwnd: HWND, width: u32, height: u32 },
    HcbtActivate { hwnd: HWND },
    HcbtMinmax { hwnd: HWND, cmd: MinMaxCommand },
    HcbtSetfocus { hwnd: HWND },
    HcbtKillfocus { hwnd: HWND },
}
```

## Window Filtering

Windows matching these conditions are excluded from tiling management:

### Filter Criteria

```rust
pub fn should_manage(hwnd: HWND) -> bool {
    // Exclude:
    // - Invisible windows (WS_VISIBLE not set)
    // - Tool windows (WS_EX_TOOLWINDOW)
    // - Glass windows (DWMs have special handling)
    // - No activate flags (WS_EX_NOACTIVATE)
    // - Already owned windows (popup dropdowns, etc.)
    // - Taskbar (Shell_TrayWnd)
    // - Start menu (Shell_TrayWnd or class matching)
    // - Program Manager (Progman)
    // - Desktop windows
    // - Hidden or transparent windows
}
```

### Excluded Classes

```
- Shell_TrayWnd (taskbar)
- Progman (program manager)
- WorkerW (desktop worker)
- DV2RenderHost (input host)
- ApplicationFrameWindow (UWP apps, except their child windows)
```

### Window Styles Check

```rust
pub fn is_manageable_window(hwnd: HWND) -> bool {
    let style = GetWindowLongW(hwnd, GWL_STYLE);
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
    
    // Must have WS_VISIBLE
    // Must NOT have WS_EX_TOOLWINDOW
    // Must NOT have WS_EX_NOACTIVATE
    // Should have WS_EX_APPWINDOW for proper toplevel detection
}
```

## Window Positioning

### SetWindowPos Integration

Window positioning uses `SetWindowPos` with appropriate flags:

```rust
pub fn apply_bounds(hwnd: HWND, bounds: Rectangle<i32, Physical>, mode: PositioningMode) {
    SetWindowPos(
        hwnd,
        HWND_TOP,
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER
    )
}
```

### True Bounds via DWM

Extended frame bounds retrieved via `DwmGetWindowAttribute`:

```rust
pub fn get_true_bounds(hwnd: HWND) -> Option<RECT> {
    let mut bounds = RECT::default();
    let result = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut bounds,
        std::mem::size_of::<RECT>()
    );
    
    if result.is_ok() {
        Some(bounds)
    } else {
        None
    }
}
```

## Min/Max Size Handling

### Size Constraints

Min/max sizes are enforced via `GetWindowRect` combined with rules:

```rust
pub fn enforce_size_constraints(hwnd: HWND, rules: &ResolvedWindowRules) -> bool {
    let mut rect = RECT::default();
    GetWindowRect(hwnd, &mut rect).is_ok()
    
    let current_size = Size {
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    };
    
    // Apply minimum size
    let min = Size {
        width: rules.min_width.unwrap_or(0),
        height: rules.min_height.unwrap_or(0),
    };
    
    // Apply maximum size
    let max = Size {
        width: rules.max_width.unwrap_or(i32::MAX),
        height: rules.max_height.unwrap_or(i32::MAX),
    };
    
    current_size.width >= min.width
        && current_size.height >= min.height
        && current_size.width <= max.width
        && current_size.height <= max.height
}
```

### System Metrics Integration

```rust
pub fn get_system_min_window_size() -> Size<i32, Physical> {
    // SM_CXMIN, SM_CYMIN from GetSystemMetrics
}
```

## Window Property Accessors

### Title Retrieval

```rust
pub fn get_window_title(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd) + 1;
    let mut buffer = vec![0u16; len as usize];
    GetWindowTextW(hwnd, &mut buffer);
    String::from_utf16_lossy(&buffer[..len as usize - 1])
}
```

### Class Name Retrieval

```rust
pub fn get_class_name(hwnd: HWND) -> String {
    let mut buffer = vec![0u16; 256];
    GetClassNameW(hwnd, &mut buffer);
    String::from_utf16_lossy(&buffer)
}
```

### Process/Thread ID

```rust
pub fn get_window_process_info(hwnd: HWND) -> (u32, u32) {
    let mut process_id = 0u32;
    let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    (process_id, thread_id)
}
```

## LayoutElement Trait Implementation

`Mapped` windows implement `LayoutElement` for integration with the layout engine:

```rust
pub trait LayoutElement {
    type Id = WindowId;

    fn id(&self) -> &Self::Id;
    fn size(&self) -> Size<i32, Physical>;
    fn position(&self) -> Point<i32, Physical>;
    fn bounds(&self) -> Rectangle<i32, Physical>;
    fn is_in_input_region(&self, point: Point<f64, Physical>) -> bool;
    fn request_size(&mut self, size: Size<i32, Physical>, mode: SizingMode, animate: bool);
    fn set_activated(&mut self, active: bool);
    fn set_floating(&mut self, floating: bool);
    fn is_urgent(&self) -> bool;
    fn configure_intent(&self) -> ConfigureIntent;
}

impl LayoutElement for Mapped {
    fn id(&self) -> &WindowId { &self.id }
    fn size(&self) -> Size<i32, Physical> { self.bounds.size }
    fn position(&self) -> Point<i32, Physical> { self.bounds.origin }
    fn bounds(&self) -> Rectangle<i32, Physical> { self.bounds }
    fn set_floating(&mut self, floating: bool) { self.floating = floating; }
    // ... etc
}
```

## Window Rule Matching

### Match Conditions

Window rules are matched using these conditions:

- `class-name`: Window class name (regex)
- `title`: Window title (regex)
- `process-name`: Executable name (regex)
- `is-active`, `is-focused`, `is-floating`
- `at-startup`: True for first 60 seconds after wiri launch

### Example Configuration

```kdl
window-rule {
    match class-name="^Firefox$"
    open-maximized true
    opacity 0.95
}

window-rule {
    match title="^Save As.*"
    baba_is_float true
}
```

## Window Lifecycle State Machine

```
                    +-----------------+
                    |   Unmanaged     |
                    |  (not tracked)  |
                    +--------+--------+
                             | should_manage()
                             v
                    +-----------------+
                    |   Unmapped      |
                    | (awaiting tile) |
                    +--------+--------+
                             | add_to_workspace()
                             v
                    +-----------------+
                    |    Mapped       |
                    |  (in layout)    |
                    +--------+--------+
                             | remove_from_workspace()
                             v
                    +-----------------+
                    |   Unmapped      |
                    | (can re-tile)   |
                    +-----------------+
```

## Integration Points

### With Layout Module
- `Mapped` implements `LayoutElement` trait
- Layout positions and sizes mapped windows via `SetWindowPos`
- Window movements update workspace column positions

### With Input Module
- Window focus is managed through keyboard focus system
- Window move/resize operations operate on mapped windows

### With Hooks Module
- CBT hooks provide window creation/destruction events
- `EnumWindows` used for initial window discovery

### With Render Module
- Window bounds used for drawing borders and shadows
- Focus indicators rendered per-window

## Windows API Mapping

| Niri (Wayland) | wiri (Win32) |
|----------------|--------------|
| WaylandSurface | HWND |
| xdg-surface configure | SetWindowPos + WM_WINDOWPOSCHANGED |
| surface commit | Window state change events |
| buffer attachment | Direct draw (no compositor involvement) |
| toplevel decorations | DWM decorations via DWMWA_USE_HOSTBACKDROPBRUSH |
| surface mapping | WS_VISIBLE + CBT hook |

## Thread Safety

Window operations primarily occur on the main thread due to Win32 API requirements:
- CBT hooks run on the thread that registered them
- `SetWindowPos` must be called from the thread owning the window
- Window enumeration via `EnumWindows` is thread-safe but results are marshaled
