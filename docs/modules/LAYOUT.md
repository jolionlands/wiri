# Layout Module Documentation

## Overview

The layout module (`src/layout/`) implements wiri's scrollable tiling engine for Windows. It manages window positioning across multiple monitors using Win32 APIs, dynamically arranging windows in columns that extend infinitely to the right.

Unlike niri (Wayland compositor), wiri does not manage window creation. Instead, it discovers existing windows via `EnumWindows` and hooks them via CBT hooks, then tiles them according to the scrollable tiling layout.

## Module Structure

```
src/layout/
+-- mod.rs           # Main layout orchestrator, traits, and enums
+-- monitor.rs       # Monitor and workspace management per output
+-- workspace.rs     # Workspace implementation with column management
+-- tile.rs          # Individual window tile container
+-- sizing.rs        # Sizing mode and window dimension calculations
+-- configure.rs     # Configure intent and window configuration logic
+-- scrolling.rs     # Scrollable tiling mechanics
+-- focus_ring.rs    # Focus management and focus ring logic
+-- floating.rs      # Floating window management (future)
+-- error.rs         # Error types for layout operations
```

## Core Types

### Layout

Main orchestrator managing all connected monitors and their workspaces.

```rust
pub struct Layout {
    monitor_set: MonitorSet,                    // HashMap of monitors by OutputId
    is_active: bool,                            // Whether layout is currently active
    interactive_move: Option<InteractiveMoveState>,  // Window drag state
    overview_open: bool,                        // Overview mode flag
    overview_progress: Option<f64>,             // Animation progress 0.0 to 1.0
}

impl Layout {
    pub fn new() -> Self;
    pub fn monitor_for_output(&mut self, output_id: &OutputId) -> &mut Monitor;
    pub fn add_window(&mut self, hwnd: HWND, target: AddWindowTarget);
    pub fn remove_window(&mut self, hwnd: HWND);
    pub fn arrange_all(&mut self);
    pub fn handle_output_connected(&mut self, output: OutputInfo);
    pub fn handle_output_disconnected(&mut self, output_id: &OutputId);
}
```

### Monitor

Represents a physical display output with its own workspace stack.

```rust
pub struct Monitor {
    output_id: OutputId,                        // Unique output identifier
    name: String,                               // Display device name
    bounds: Rect<i32, Physical>,                // Total monitor bounds
    work_area: Rect<i32, Physical>,              // Usable area (taskbar excluded)
    scale_factor: f64,                          // DPI scaling factor
    workspaces: Vec<Workspace>,                  // Stack of workspaces
    active_workspace_idx: usize,                 // Index of currently visible workspace
    cursor_pos: Point<i32, Physical>,           // Current cursor position
    is_cursor_visible: bool,
}

impl Monitor {
    pub fn active_workspace(&mut self) -> &mut Workspace;
    pub fn switch_workspace(&mut self, idx: usize);
    pub fn arrange_workspace(&mut self, idx: usize);
    pub fn scroll_columns(&mut self, delta: ScrollDelta);
}
```

### Workspace

Container for a single workspace with columns of tiled windows.

```rust
pub struct Workspace {
    id: WorkspaceId,                            // Unique workspace identifier
    columns: Vec<Column>,                        // Columns extending rightward
    focus_column_idx: usize,                     // Column with keyboard focus
    scroll_offset: f64,                          // Horizontal scroll position
    scroll_extent: f64,                          // Total scrollable width
}

impl Workspace {
    pub fn new(id: WorkspaceId) -> Self;
    pub fn add_column(&mut self) -> ColumnId;
    pub fn remove_column(&mut self, column_id: ColumnId);
    pub fn insert_window(&mut self, hwnd: HWND, column_id: ColumnId, index: usize);
    pub fn remove_window(&mut self, hwnd: HWND);
    pub fn column_mut(&mut self, column_id: ColumnId) -> Option<&mut Column>;
    pub fn arrange(&mut self, work_area: Rect<i32, Physical>);
    pub fn scroll_by(&mut self, delta: f64);
    pub fn scroll_to_fit_column(&mut self, column_idx: usize, view_width: i32);
}
```

### Column

A vertical stack of tiles within a workspace.

```rust
pub struct Column {
    id: ColumnId,
    tiles: Vec<Tile>,                           // Windows stacked vertically
    width: i32,                                 // Column width in pixels
    focus_idx: usize,                           // Focused tile within column
}

impl Column {
    pub fn insert_tile(&mut self, tile: Tile, index: usize);
    pub fn remove_tile(&mut self, hwnd: HWND) -> Option<Tile>;
    pub fn tile_at(&mut self, index: usize) -> Option<&mut Tile>;
    pub fn tile_by_hwnd(&mut self, hwnd: HWND) -> Option<&mut Tile>;
}
```

### Tile

Individual window container within a column.

```rust
pub struct Tile {
    hwnd: HWND,                                 // Win32 window handle
    size: Size<i32, Physical>,                  // Current configured size
    position: Point<i32, Physical>,             // Current position
    sizing_mode: SizingMode,                    // Current sizing state
    is_focused: bool,                           // Has keyboard focus
    is_urgent: bool,                            // Requires attention
    configure_intent: ConfigureIntent,          // Pending configuration
    anim_state: Option<AnimState>,              // Animation state if transitioning
}

impl Tile {
    pub fn new(hwnd: HWND) -> Self;
    pub fn calculate_bounds(&self, column_x: i32, column_width: i32, avail_height: i32) -> WindowRect;
    pub fn apply_position(&self) -> Result<(), Win32Error>;
}
```

## Key Traits

### LayoutElement

Trait for windows that can be positioned by the layout engine.

```rust
pub trait LayoutElement {
    fn hwnd(&self) -> HWND;
    fn id(&self) -> WindowId;
    fn current_bounds(&self) -> Rect<i32, Physical>;
    fn size_hint(&self) -> SizeHint;
    fn is_in_region(&self, point: Point<i32, Physical>) -> bool;
    fn request_configure(&mut self, bounds: Rect<i32, Physical>, mode: SizingMode);
    fn set_focused(&mut self, focused: bool);
    fn set_urgent(&mut self, urgent: bool);
    fn configure_intent(&self) -> ConfigureIntent;
}

pub struct WindowId(u64);  // HWND as unique identifier

#[derive(Clone, Copy)]
pub struct SizeHint {
    pub min: Option<Size<i32, Logical>>,
    pub max: Option<Size<i32, Logical>>,
    pub base: Option<Size<i32, Logical>>,
    pub increment: Option<Size<i32, Logical>>,
}
```

## Enums

### SizingMode

Describes how a window should be sized within the layout.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SizingMode {
    Normal,      // Tile normally within column
    Maximized,   // Fill entire workspace area
    Fullscreen,   // Fill entire monitor area
}

impl SizingMode {
    pub fn applies_to_column_width(&self) -> bool {
        matches!(self, SizingMode::Normal)
    }
}
```

### ConfigureIntent

Tracks when a window needs configuration sent to it.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigureIntent {
    NotNeeded,    // Window matches desired state
    Throttled,    // Debounced, will send when throttle elapses
    CanSend,     // Ready to send configuration immediately
    ShouldSend,   // Must send even if similar to current state
}

impl ConfigureIntent {
    pub fn should_send(&self) -> bool {
        matches!(self, ConfigureIntent::CanSend | ConfigureIntent::ShouldSend)
    }
}
```

### ScrollDirection

Horizontal scrolling within a workspace.

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Left,
    Right,
}

impl ScrollDirection {
    pub fn delta(&self) -> f64 {
        match self {
            ScrollDirection::Left => -SCROLL_STEP,
            ScrollDirection::Right => SCROLL_STEP,
        }
    }
}

const SCROLL_STEP: f64 = 200.0;  // Pixels per scroll tick
```

### AddWindowTarget

Where to place a newly discovered window.

```rust
#[derive(Clone, Copy, Debug)]
pub enum AddWindowTarget {
    Auto,                      // Default workspace
    Output(OutputId),          // Specific output
    Workspace(WorkspaceId),    // Specific workspace
    Column(ColumnId),          // Specific column
    NextTo(HWND),              // Next to existing window
}
```

## Scrollable Tiling

Wiri implements scrollable tiling where windows are arranged in vertical columns extending infinitely to the right. Key properties:

1. **No Resizing on Open**: Opening new windows never resizes existing windows
2. **Infinite Width**: Columns extend rightward; users scroll to see more
3. **Per-Monitor Strips**: Each monitor has independent window arrangements
4. **Per-Workspace Scrolling**: Each workspace maintains its own scroll state

### Column Width Calculation

Columns have configurable widths. Default width is the workspace width divided by column count, but can be overridden:

```rust
impl Workspace {
    pub fn calculate_column_widths(&self, work_area_width: i32) -> Vec<i32> {
        let column_count = self.columns.len();
        if column_count == 0 {
            return vec![];
        }
        
        // Equal width distribution
        let base_width = work_area_width / column_count as i32;
        let mut widths = vec![base_width; column_count];
        
        // First column gets remainder if not evenly divisible
        let remainder = work_area_width % column_count as i32;
        if remainder > 0 {
            widths[0] += remainder;
        }
        
        widths
    }
}
```

### Tile Height Calculation

Tiles within a column share the column height equally:

```rust
impl Column {
    pub fn calculate_tile_heights(&self, column_height: i32) -> Vec<i32> {
        let tile_count = self.tiles.len();
        if tile_count == 0 {
            return vec![];
        }
        
        let base_height = column_height / tile_count as i32;
        let mut heights = vec![base_height; tile_count];
        
        // First tile gets remainder
        let remainder = column_height % tile_count as i32;
        if remainder > 0 {
            heights[0] += remainder;
        }
        
        heights
    }
}
```

## Window Positioning with Win32 APIs

### Setting Window Position

Use `SetWindowPos` for all window positioning:

```rust
fn apply_window_position(hwnd: HWND, rect: Rect<i32, Physical>, mode: SizingMode) -> Result<()> {
    let (flags, z_order) = match mode {
        SizingMode::Normal => (SWP_NOZORDER | SWP_NOACTIVATE, HWND_TOP),
        SizingMode::Maximized => (SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREPOSITION, HWND_TOP),
        SizingMode::Fullscreen => (SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREPOSITION, HWND_TOP),
    };
    
    unsafe {
        SetWindowPos(
            hwnd,
            z_order,
            rect.origin.x,
            rect.origin.y,
            rect.size.cx,
            rect.size.cy,
            flags,
        )
    }.ok()
}
```

### Getting Window Bounds

Use `DwmGetWindowAttribute` with `DWMWA_EXTENDED_FRAME_BOUNDS` for true bounds (accounts for drop shadow):

```rust
fn get_window_bounds(hwnd: HWND) -> Result<Rect<i32, Physical>> {
    let mut rect = RECT::default();
    
    // First try DWM for true bounds including drop shadow
    let mut bounds = std::mem::zeroed();
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut bounds as *mut _ as *mut _,
            std::mem::size_of_val(&bounds) as u32,
        )
    };
    
    if result == S_OK {
        Ok(Rect::from_points(
            Point::new(bounds.left, bounds.top),
            Point::new(bounds.right, bounds.bottom),
        ))
    } else {
        // Fallback to GetWindowRect
        unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
        Ok(Rect::from_ltrb(rect.left, rect.top, rect.right, rect.bottom))
    }
}
```

### True Bounds vs Window Rect

- `GetWindowRect`: Returns the window's position including the non-client area but excluding drop shadow
- `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`: Returns true visible bounds including shadow

Use `DwmGetWindowAttribute` for layout calculations to avoid overlapping shadows.

## Monitor Management

### MonitorSet

HashMap of monitors keyed by OutputId:

```rust
pub struct MonitorSet {
    monitors: HashMap<OutputId, Monitor>,
    primary_output: Option<OutputId>,
}

impl MonitorSet {
    pub fn insert(&mut self, output_id: OutputId, monitor: Monitor) -> Option<Monitor>;
    pub fn remove(&mut self, output_id: &OutputId) -> Option<Monitor>;
    pub fn get(&self, output_id: &OutputId) -> Option<&Monitor>;
    pub fn get_mut(&mut self, output_id: &OutputId) -> Option<&mut Monitor>;
    pub fn outputs(&self) -> impl Iterator<Item = &OutputId>;
    pub fn primary(&self) -> Option<&Monitor>;
}

pub type OutputId = String;  // Device name from EnumDisplayDevices
```

### Monitor Discovery

Monitors are discovered via Windows display APIs:

```rust
pub struct OutputInfo {
    pub id: OutputId,
    pub name: String,           // e.g., "\\.\DISPLAY1"
    pub bounds: Rect<i32, Physical>,
    pub work_area: Rect<i32, Physical>,
    pub scale_factor: f64,
    pub is_primary: bool,
}

impl Layout {
    pub fn refresh_monitors(&mut self) {
        // Use EnumDisplayDevices and EnumDisplayMonitors
        // Create Monitor from OutputInfo
        // Preserve workspace state for reconnected outputs
    }
}
```

## Workspace Management

### Per-Monitor Workspace Stack

Each monitor maintains a stack of workspaces:

```rust
impl Monitor {
    pub fn workspace_count(&self) -> usize {
        self.workspaces.len()
    }
    
    pub fn active_workspace(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_workspace_idx]
    }
    
    pub fn switch_workspace(&mut self, idx: usize) {
        if idx < self.workspaces.len() {
            self.active_workspace_idx = idx;
            self.arrange_workspace(idx);
        }
    }
    
    pub fn create_workspace(&mut self) -> WorkspaceId {
        let id = WorkspaceId::new();
        self.workspaces.push(Workspace::new(id));
        self.active_workspace_idx = self.workspaces.len() - 1;
        id
    }
    
    pub fn delete_workspace(&mut self, idx: usize) -> bool {
        if self.workspaces.len() <= 1 {
            return false;  // Keep at least one workspace
        }
        self.workspaces.remove(idx);
        if self.active_workspace_idx >= self.workspaces.len() {
            self.active_workspace_idx = self.workspaces.len() - 1;
        }
        true
    }
}
```

## Configuration and Intent

### ConfigureIntent Lifecycle

```rust
pub enum ConfigureIntent {
    NotNeeded,    // Window already matches layout
    Throttled,    // Debouncing configuration
    CanSend,      // Ready to configure
    ShouldSend,   // Force send even if similar
}

impl Tile {
    pub fn update_configure_intent(&mut self) {
        let desired_bounds = self.calculate_desired_bounds();
        let current_bounds = self.current_bounds();
        
        self.configure_intent = if desired_bounds == current_bounds {
            ConfigureIntent::NotNeeded
        } else if self.is_animating() {
            ConfigureIntent::Throttled
        } else {
            ConfigureIntent::CanSend
        };
    }
    
    pub fn mark_should_send(&mut self) {
        self.configure_intent = ConfigureIntent::ShouldSend;
    }
}
```

### Configuration Throttling

To prevent configuration storms when windows are being added rapidly:

```rust
pub struct ConfigureThrottle {
    window: HWND,
    pending_bounds: Rect<i32, Physical>,
    timer: Option<Instant>,
}

const THROTTLE_DURATION: Duration = Duration::from_millis(16);  // ~60fps
```

## Focus Management

### Focus Ring

Maintains focus order across workspaces:

```rust
pub struct FocusRing {
    stack: Vec<HWND>,           // Most recently focused first
}

impl FocusRing {
    pub fn focus(&mut self, hwnd: HWND) {
        self.stack.retain(|h| *h != hwnd);
        self.stack.insert(0, hwnd);
    }
    
    pub fn unfocus(&mut self, hwnd: HWND) {
        self.stack.retain(|h| *h != hwnd);
    }
    
    pub fn focused(&self) -> Option<HWND> {
        self.stack.first().copied()
    }
    
    pub fn focus_next(&mut self) -> Option<HWND> {
        if self.stack.len() > 1 {
            let hwnd = self.stack.remove(0);
            self.stack.push(hwnd);
            self.stack.first().copied()
        } else {
            None
        }
    }
}
```

## Layout Calculation Algorithm

### Main Arrangement Pass

```rust
impl Workspace {
    pub fn arrange(&mut self, work_area: Rect<i32, Physical>) {
        let column_widths = self.calculate_column_widths(work_area.size.cx);
        let mut x = work_area.origin.x;
        
        for (idx, column) in self.columns.iter_mut().enumerate() {
            let width = column_widths[idx];
            let column_rect = Rect::new(
                Point::new(x, work_area.origin.y),
                Size::new(width, work_area.size.cy),
            );
            
            column.arrange(column_rect);
            x += width as i32;
        }
        
        self.scroll_extent = x - work_area.origin.x;
        self.clamp_scroll();
    }
}

impl Column {
    pub fn arrange(&mut self, rect: Rect<i32, Physical>) {
        let heights = self.calculate_tile_heights(rect.size.cy);
        let mut y = rect.origin.y;
        
        for (idx, tile) in self.tiles.iter_mut().enumerate() {
            let height = heights[idx];
            tile.set_bounds(Rect::new(
                Point::new(rect.origin.x, y),
                Size::new(rect.size.cx, height),
            ));
            y += height;
        }
    }
}
```

### Scroll Behavior

```rust
impl Workspace {
    fn clamp_scroll(&mut self) {
        let max_scroll = (self.scroll_extent - self.visible_width()).max(0.0);
        self.scroll_offset = self.scroll_offset.clamp(0.0, max_scroll);
    }
    
    fn visible_width(&self) -> f64 {
        // Returns visible workspace width
        // This accounts for partial column visibility at edges
    }
}
```

## Integration Points

### With Window Discovery (EnumWindows)

Windows are discovered and registered with the layout:

```rust
impl Layout {
    pub fn register_window(&mut self, hwnd: HWND) {
        // Create Tile for HWND
        let tile = Tile::new(hwnd);
        
        // Add to active workspace based on rules
        let target = self.classify_window(hwnd);
        self.add_window(tile, target);
    }
    
    pub fn unregister_window(&mut self, hwnd: HWND) {
        self.remove_window(hwnd);
    }
    
    fn classify_window(&self, hwnd: HWND) -> AddWindowTarget {
        // Rules for where to place new windows
        // Could check window class, title, process name
        AddWindowTarget::Auto
    }
}
```

### With CBT Hook

Windows are hooked via SetWindowsHookEx with WH_CBT:

```rust
// CBT hook events relevant to layout:
// HCBT_CREATEWND  - Window created, add to layout
// HCBT_DESTROYWND - Window destroyed, remove from layout
// HCBT_ACTIVATE   - Window activated, update focus ring
// HCBT_MOVESIZE   - Window moved/sized externally, mark for re-tile if floating
```

### With Render Pipeline

Layout provides window render state:

```rust
impl Layout {
    pub fn render_windows(&self) -> impl Iterator<Item = RenderWindow> {
        // Provide window bounds, z-order, focus state for compositing
    }
}
```

## Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("failed to get window bounds: {0}")]
    GetBoundsFailed(#[from] Win32Error),
    
    #[error("failed to set window position: {0}")]
    SetPositionFailed(#[from] Win32Error),
    
    #[error("window {0:?} not found in layout")]
    WindowNotFound(HWND),
    
    #[error("monitor {0:?} not found")]
    MonitorNotFound(OutputId),
    
    #[error("invalid workspace index {0}")]
    InvalidWorkspaceIndex(usize),
}
```

## Performance Considerations

1. **Configuration Batching**: Group multiple window configurations into single arrange pass
2. **Throttling**: Debounce rapid configuration requests (~16ms)
3. **O(1) Lookup**: Use HashMap for HWND to Tile, Column, Workspace lookups
4. **Incremental Arrange**: Only re-arrange affected columns when windows are added/removed

## Future Extensions

### Floating Windows
- Track floating windows separately
- Apply different positioning rules

### Window Shadows
- Query DWM for shadow bounds
- Render shadows as part of compositing

### Animations
- Smooth scrolling between workspaces
- Animated window resize/move
