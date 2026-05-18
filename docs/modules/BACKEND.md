# Backend Module Documentation

## Overview

The backend module (`src/backend/`) provides the Windows-specific implementation for wiri, a tiling window manager. Unlike niri which supports multiple backends (TTY/DRM, Winit, Headless), wiri is Windows-only and exposes a unified `Backend` trait for window and monitor management.

## Module Structure

```
backend/
├── mod.rs           # Backend trait and WindowsBackend implementation
├── message_loop.rs  # Tokio-based async message loop for window events
└── hooks.rs         # Windows hook implementations (WH_CBT, WinEvent hooks)
```

## Core Design Principles

1. **Trait-based abstraction**: The `Backend` trait provides a clean interface between window management logic and platform-specific implementation
2. **Async-first**: All blocking Windows API calls are wrapped in async functions using tokio
3. **Event-driven**: Window events flow through hooks into the message loop, then to the layout module
4. **windows-rs integration**: Uses the `windows-rs` crate for Win32 API bindings

---

## Core Types

### Backend Trait

The central abstraction for all backend operations:

```rust
pub trait Backend: Send + Sync {
    /// Initialize the backend and register window hooks
    fn init(&mut self) -> Result<(), BackendError>;

    /// Run the async message loop
    async fn run(self: Arc<Self>, layout_channel: UnboundedSender<BackendEvent>);

    /// Get all enumerated windows
    fn get_windows(&self) -> Result<Vec<WindowInfo>, BackendError>;

    /// Get windows filtered by predicate
    fn get_windows_filtered<F>(&self, filter: F) -> Result<Vec<WindowInfo>, BackendError>
    where
        F: Fn(&WindowInfo) -> bool;

    /// Position a window using SetWindowPos
    fn set_window_position(
        &self,
        hwnd: HWND,
        rect: WindowRect,
        flags: SetWindowPosFlags,
    ) -> Result<(), BackendError>;

    /// Move and resize a window
    fn move_window(
        &self,
        hwnd: HWND,
        rect: WindowRect,
    ) -> Result<(), BackendError>;

    /// Get the true window bounds using DWM extended frame bounds
    fn get_window_bounds(&self, hwnd: HWND) -> Result<WindowRect, BackendError>;

    /// Get all connected monitors
    fn get_monitors(&self) -> Result<Vec<MonitorInfo>, BackendError>;

    /// Get the monitor containing the given point
    fn get_monitor_at_point(&self, point: Point<i32, Screen>) -> Result<MonitorInfo, BackendError>;

    /// Get the monitor containing the given window
    fn get_monitor_for_window(&self, hwnd: HWND) -> Result<MonitorInfo, BackendError>;

    /// Activate (focus) a window
    fn activate_window(&self, hwnd: HWND) -> Result<(), BackendError>;

    /// Minimize a window
    fn minimize_window(&self, hwnd: HWND) -> Result<(), BackendError>;

    /// Maximize a window
    fn maximize_window(&self, hwnd: HWND) -> Result<(), BackendError>;

    /// Restore a window to normal size
    fn restore_window(&self, hwnd: HWND) -> Result<(), BackendError>;

    /// Close a window
    fn close_window(&self, hwnd: HWND) -> Result<(), BackendError>;

    /// Get the output identifier for IPC purposes
    fn output_id(&self, monitor: &MonitorInfo) -> OutputId;

    /// Shutdown the backend gracefully
    async fn shutdown(&mut self);
}

### WindowsBackend

The primary implementation of the `Backend` trait:

```rust
pub struct WindowsBackend {
    /// Tokio runtime handle for async operations
    runtime: Runtime,
    /// Channel to send events to the layout module
    event_sender: UnboundedSender<BackendEvent>,
    /// Registered window hooks
    hooks: Vec<HHook>,
    /// Cached window enumeration
    window_cache: RwLock<Vec<WindowInfo>>,
    /// Cached monitor enumeration  
    monitor_cache: RwLock<Vec<MonitorInfo>>,
    /// Shutdown flag
    shutdown_flag: AtomicBool,
    /// Thread ID for hook registration
    hook_thread_id: Cell<u32>,
}

impl Backend for WindowsBackend { /* ... */ }
```

---

## Window Management Types

### WindowInfo

Represents a managed window:

```rust
#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// Native window handle
    pub hwnd: HWND,
    /// Window class name
    pub class_name: String,
    /// Window title (from GetWindowTextW)
    pub title: String,
    /// Application filename (for app_id matching)
    pub app_path: String,
    /// Resolved app identifier (derived from app_path)
    pub app_id: AppId,
    /// Current window bounds (DWM-extended)
    pub bounds: WindowRect,
    /// Client area bounds
    pub client_bounds: WindowRect,
    /// Window state (normal, minimized, maximized)
    pub state: WindowState,
    /// Is window visible
    pub visible: bool,
    /// Is window owned by another window (popup/child)
    pub is_owned: bool,
    /// Owner window (if is_owned)
    pub owner: Option<HWND>,
    /// Process ID for filtering
    pub process_id: u32,
    /// Thread ID
    pub thread_id: u32,
}
```

### WindowState

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}
```

### WindowRect

Window rectangle with screen coordinates:

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl WindowRect {
    pub fn width(&self) -> i32 { self.right - self.left }
    pub fn height(&self) -> i32 { self.bottom - self.top }
    pub fn size(&self) -> Size<i32, Screen> { /* ... */ }
    pub fn position(&self) -> Point<i32, Screen> { /* ... */ }
}
```

### SetWindowPosFlags

Flags for `SetWindowPos` operation:

```rust
bitflags::bitflags! {
    pub struct SetWindowPosFlags: u32 {
        const NOSIZE = 0x0001;
        const NOMOVE = 0x0002;
        const NOZORDER = 0x0004;
        const NOREDRAW = 0x0008;
        const NOACTIVATE = 0x0010;
        const DRAWFRAME = 0x0020;
        const FRAMECHANGED = 0x0020;
        const SHOWWINDOW = 0x0040;
        const HIDDENWINDOW = 0x0080;
        const NOCOPYBITS = 0x0100;
        const NOOWNERZORDER = 0x0200;
        const NOSENDCHANGING = 0x0400;
        const DEFERERASE = 0x2000;
        const ASYNCWINDOWPOS = 0x4000;
    }
}
```

---

## Monitor Management Types

### MonitorInfo

Information about a connected display:

```rust
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    /// Native monitor handle (HMONITOR)
    pub hmonitor: HMONITOR,
    /// Device name (e.g., "\\\\.\\DISPLAY1")
    pub device_name: String,
    /// Display device identifier
    pub display_id: DisplayId,
    /// Monitor bounds in screen coordinates
    pub bounds: WindowRect,
    /// Work area (excludes taskbar)
    pub work_area: WindowRect,
    /// Is primary monitor
    pub is_primary: bool,
    /// Refresh rate (Hz)
    pub refresh_rate: u32,
    /// DPI awareness mode
    pub dpi_aware: bool,
    /// Physical size (mm) - optional
    pub physical_size: Option<Size<u32, Physical>>,
}
```

### DisplayId

Unique identifier for a display adapter:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayId(pub u32);
```

---

## Output Identification

### OutputId

Used for IPC and layout coordination:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(pub u64);
```

OutputId encoding:
- Bits 0-31: DisplayId (adapter-relative)
- Bits 32-63: Reserved for multi-adapter configurations

---

## Event Types

### BackendEvent

Events sent from the backend to the layout module:

```rust
pub enum BackendEvent {
    /// A new window was created
    WindowCreated(HWND),
    /// A window was destroyed
    WindowDestroyed(HWND),
    /// A window was moved or resized
    WindowResized(HWND, WindowRect),
    /// A window title or class changed
    WindowUpdated(HWND),
    /// A window was shown or hidden
    WindowVisibilityChanged(HWND, bool),
    /// A window was minimized or maximized
    WindowStateChanged(HWND, WindowState),
    /// A monitor was connected or disconnected
    MonitorChanged(Vec<MonitorInfo>),
    /// An error occurred in the backend
    Error(BackendError),
}
```

### LayoutRequest

Requests sent from the layout module to the backend:

```rust
pub enum LayoutRequest {
    /// Position a window
    PositionWindow(HWND, WindowRect, SetWindowPosFlags),
    /// Activate a window
    ActivateWindow(HWND),
    /// Focus a window (set foreground)
    FocusWindow(HWND),
    /// Minimize a window
    MinimizeWindow(HWND),
    /// Maximize a window
    MaximizeWindow(HWND),
    /// Restore a window
    RestoreWindow(HWND),
    /// Close a window
    CloseWindow(HWND),
}
```

---

## Render Result

Indicates the outcome of a render cycle:

```rust
pub enum RenderResult {
    /// Frame was submitted for presentation
    Submitted,
    /// Rendering occurred but no damage (no changes)
    NoDamage,
    /// Frame rendering was skipped
    Skipped,
}
```

For wiri (Windows), these map to:
- `Submitted`: `SetWindowPos` or similar succeeded
- `NoDamage`: Window positions unchanged from last frame
- `Skipped`: Backend is paused or shutting down

---

## Message Loop Integration

### Async Message Loop

The message loop runs on a tokio runtime and processes Windows events asynchronously:

```rust
impl WindowsBackend {
    pub async fn run(self: Arc<Self>, layout_channel: UnboundedSender<BackendEvent>) {
        // Create a Win32 message-only window for receiving events
        let msg_window = MessageWindow::create().unwrap();
        
        // Register WinEvent hooks for window tracking
        self.register_hooks(&msg_window).await;
        
        // Main event loop
        loop {
            if self.shutdown_flag.load(Ordering::SeqCst) {
                break;
            }
            
            // Process pending Windows messages
            while let Some(msg) = msg_window.peek_message() {
                self.dispatch_message(msg);
            }
            
            // Yield to tokio runtime
            tokio::task::yield_now().await;
        }
        
        // Unregister hooks on shutdown
        self.unregister_hooks().await;
    }
}
```

### Windows Hooks (hooks.rs)

#### CBT Hook

Used for window creation, destruction, and activation events:

```rust
pub struct CbtHook {
    hook_handle: HHook,
    event_sender: UnboundedSender<BackendEvent>,
}

impl CbtHook {
    pub fn new(sender: UnboundedSender<BackendEvent>) -> Self;
    pub fn register() -> Result<Self, HookError>;
    
    fn handle_event(&self, event: CbtEvent) {
        match event {
            CbtEvent::WindowCreated(hwnd) => {
                let _ = self.event_sender.send(BackendEvent::WindowCreated(hwnd));
            }
            CbtEvent::WindowDestroyed(hwnd) => {
                let _ = self.event_sender.send(BackendEvent::WindowDestroyed(hwnd));
            }
            CbtEvent::WindowActivated(hwnd) => {
                // Handle activation if needed
            }
            CbtEvent::WindowMoved(hwnd) | CbtEvent::WindowSized(hwnd) => {
                // These are handled via WinEvent hooks for better info
            }
        }
    }
}
```

#### WinEvent Hook

Used for comprehensive window state change tracking:

```rust
pub struct WinEventHook {
    hook_handle: HHook,
    tracked_windows: RwLock<HashSet<HWND>>,
    event_sender: UnboundedSender<BackendEvent>,
}

impl WinEventHook {
    /// Events we hook into:
    const TRACKED_EVENTS: &'static [u32] = &[
        EVENT_SYSTEM_FOREGROUND,      // Window gained focus
        EVENT_SYSTEM_MOVESIZESTART,   // Window resize started
        EVENT_SYSTEM_MOVESIZEEND,     // Window resize ended
        EVENT_OBJECT_LOCATIONCHANGE,  // Window moved/resized
        EVENT_OBJECT_SHOW,            // Window shown
        EVENT_OBJECT_HIDE,            // Window hidden
        EVENT_OBJECT_DESTROY,         // Window destroyed
        EVENT_OBJECT_REORDER,         // Z-order changed
    ];
    
    pub fn new(sender: UnboundedSender<BackendEvent>) -> Self;
    pub fn register() -> Result<Self, HookError>;
    pub fn track_window(&self, hwnd: HWND);
    pub fn untrack_window(&self, hwnd: HWND);
}
```

---

## Window Enumeration

### Enumeration Strategy

Windows enumeration uses `EnumWindows` with filtering:

```rust
impl WindowsBackend {
    fn enumerate_windows(&self) -> Result<Vec<WindowInfo>, BackendError> {
        let mut windows = Vec::new();
        
        unsafe {
            EnumWindows(Some(enum_windows_callback), &mut windows as *mut _ as *mut _)?;
        }
        
        Ok(windows)
    }
    
    unsafe extern "system" fn enum_windows_callback(
        hwnd: HWND,
        windows: *mut std::ffi::c_void,
    ) -> Bool {
        let windows = &mut *(windows as *mut Vec<WindowInfo>);
        
        // Skip invisible, owned, or tool windows
        if !is_interesting_window(hwnd) {
            return Bool::from_raw(1); // Continue enumeration
        }
        
        if let Some(info) = window_info_from_hwnd(hwnd) {
            windows.push(info);
        }
        
        Bool::from_raw(1)
    }
}

### Window Filtering

The `is_interesting_window` function filters out:
- Invisible windows (`WS_VISIBLE` not set)
- Owned/popup windows (except main application windows)
- Tool windows (`WS_EX_TOOLWINDOW`)
- Desktop windows
- Start menu/Tray windows
- Windows from wiri itself
- Windows from excluded applications

```rust
fn is_interesting_window(hwnd: HWND) -> bool {
    // Must be visible
    if !has_visibility(hwnd) {
        return false;
    }
    
    // Skip tool windows
    if has_ex_style(hwnd, WS_EX_TOOLWINDOW) {
        return false;
    }
    
    // Skip hidden owner windows (popups are OK if visible)
    if let Some(owner) = get_owner(hwnd) {
        if !has_visibility(owner) {
            return false;
        }
    }
    
    // Skip excluded classes
    if is_excluded_class(hwnd) {
        return false;
    }
    
    true
}

fn is_excluded_class(hwnd: HWND) -> bool {
    let class = get_class_name(hwnd);
    matches!(class.as_str(),
        | "Shell_TrayWnd"           // Taskbar
        | "Shell_SecondaryTrayWnd" // Secondary taskbar
        | "Progman"                // Desktop
        | "WorkerW"                // Desktop worker
        | "DV2ControlHost"         // Start menu
        | "Windows.UI.Core.corewindow" // UWP shell
        | "ApplicationFrameWindow" // UWP app frame (filter by app_id)
    )
}
```

---

## Window Positioning

### SetWindowPos Wrapper

The `set_window_position` method wraps `SetWindowPos` with proper error handling:

```rust
impl Backend for WindowsBackend {
    fn set_window_position(
        &self,
        hwnd: HWND,
        rect: WindowRect,
        flags: SetWindowPosFlags,
    ) -> Result<(), BackendError> {
        // Ensure window is not minimized when positioning
        let state = self.get_window_state(hwnd)?;
        if state == WindowState::Minimized {
            return Err(BackendError::WindowMinimized(hwnd));
        }
        
        // Get true window bounds including shadow/border
        let current_bounds = self.get_window_bounds(hwnd)?;
        
        // Skip if position unchanged (optimization)
        if current_bounds == rect && flags.contains(SetWindowPosFlags::NOACTIVATE) {
            return Ok(());
        }
        
        // Perform the move
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOP,  // Or appropriate z-order
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                flags,
            ).ok()
        }.map_err(|e| BackendError::SetWindowPosFailed(e))?;
        
        // Verify the new bounds
        let new_bounds = self.get_window_bounds(hwnd)?;
        
        // Log warning if bounds do not match (DWM decoration offset)
        if new_bounds != rect {
            log::warn!(
                "Window bounds mismatch: requested {:?}, got {:?}",
                rect, new_bounds
            );
        }
        
        Ok(())
    }
}
```

### DWM Extended Frame Bounds

To get accurate window bounds (excluding DWM shadow):

```rust
impl WindowsBackend {
    fn get_dwm_bounds(&self, hwnd: HWND) -> Result<WindowRect, BackendError> {
        unsafe {
            let mut rect = DWM_FRAME_BOUNDS_EXTENDED {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            
            let result = DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut _ as *mut _,
                std::mem::size_of::<DWM_FRAME_BOUNDS_EXTENDED>() as u32,
            );
            
            result.ok().map_err(|e| BackendError::DwmGetAttributeFailed(e))?;
            
            Ok(WindowRect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            })
        }
    }
}
```

---

## Monitor Enumeration

### Multi-Monitor Support

Uses `EnumDisplayMonitors` with callback:

```rust
impl Backend for WindowsBackend {
    fn get_monitors(&self) -> Result<Vec<MonitorInfo>, BackendError> {
        let mut monitors = Vec::new();
        
        unsafe {
            EnumDisplayMonitors(
                None,  // All monitors
                None,  // Full screen area
                Some(monitor_enum_callback),
                &mut monitors as *mut _ as *mut _,
            )?;
        }
        
        Ok(monitors)
    }
}

unsafe extern "system" fn monitor_enum_callback(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    monitors: *mut std::ffi::c_void,
) -> Bool {
    let monitors = &mut *(monitors as *mut Vec<MonitorInfo>);
    
    if let Some(info) = monitor_info_from_hmonitor(hmonitor) {
        monitors.push(info);
    }
    
    Bool::from_raw(1)
}

fn monitor_info_from_hmonitor(hmonitor: HMONITOR) -> Option<MonitorInfo> {
    unsafe {
        let mut info = MONITORINFOEXW {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        };
        
        if !GetMonitorInfoW(hmonitor, &mut info as *mut _ as *mut _) {
            return None;
        }
        
        let device_name = String::from_utf16_lossy(&info.szDevice);
        let bounds = WindowRect { /* from rcMonitor */ };
        let work_area = WindowRect { /* from rcWork */ };
        let is_primary = (info.dwFlags & MONITORINFOF_PRIMARY) != 0;
        
        Some(MonitorInfo {
            hmonitor,
            device_name,
            display_id: derive_display_id(&info.szDevice),
            bounds,
            work_area,
            is_primary,
            refresh_rate: get_refresh_rate(hmonitor),
            dpi_aware: is_dpi_aware(),
            physical_size: get_physical_size(hmonitor),
        })
    }
}
```

---

## Error Types

### BackendError

```rust
#[derive(Debug, Error)]
pub enum BackendError {
    #[error("Failed to initialize hooks: {0}")]
    HookInitFailed(#[from] HookError),
    
    #[error("SetWindowPos failed: {0}")]
    SetWindowPosFailed(#[from] Win32Error),
    
    #[error("DWM GetWindowAttribute failed: {0}")]
    DwmGetAttributeFailed(#[from] Win32Error),
    
    #[error("Window enumeration failed: {0}")]
    EnumerationFailed(#[from] Win32Error),
    
    #[error("Monitor enumeration failed: {0}")]
    MonitorEnumerationFailed(#[from] Win32Error),
    
    #[error("Window {0} is minimized")]
    WindowMinimized(HWND),
    
    #[error("Window {0} not found")]
    WindowNotFound(HWND),
    
    #[error("Channel send failed")]
    ChannelSendFailed,
    
    #[error("Backend shut down")]
    Shutdown,
}
```

### HookError

```rust
#[derive(Debug, Error)]
pub enum HookError {
    #[error("SetWindowsHookEx failed: {0}")]
    SetHookFailed(#[from] Win32Error),
    
    #[error("Unhook failed")]
    UnhookFailed,
}
```

---

## Layout Module Integration

### Integration Pattern

The backend communicates with the layout module through:

1. **Unbounded channel**: `BackendEvent` messages sent from hooks/message loop
2. **Backend handle**: Passed to layout for issuing `LayoutRequest`

```rust
pub struct BackendHandle {
    sender: UnboundedSender<LayoutRequest>,
}

impl BackendHandle {
    pub fn position_window(&self, hwnd: HWND, rect: WindowRect) {
        let _ = self.sender.send(LayoutRequest::PositionWindow(hwnd, rect, SetWindowPosFlags::empty()));
    }
    
    pub fn activate_window(&self, hwnd: HWND) {
        let _ = self.sender.send(LayoutRequest::ActivateWindow(hwnd));
    }
    
    // ... other methods
}
```

### Main Loop Coordination

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create channel for backend -> layout events
    let (backend_tx, backend_rx) = unbounded_channel();
    
    // Create backend
    let backend = Arc::new(WindowsBackend::new(backend_tx));
    
    // Create layout with backend handle
    let layout = Layout::new(BackendHandle { /* ... */ });
    
    // Spawn backend task
    let backend_clone = Arc::clone(&backend);
    let layout_clone = layout.clone();
    tokio::spawn(async move {
        backend_clone.run(layout_clone.event_receiver()).await;
    });
    
    // Run layout main loop
    layout.run().await;
    
    Ok(())
}
```

---

## Windows-Specific Considerations

### DPI Awareness

- wiri should run as **Per-Monitor DPI Aware** (PMA)
- Use `GetDpiForMonitor` for per-monitor DPI values
- Scale calculations must account for mixed DPI environments
- Handle `WM_DPICHANGED` messages for DPI changes

### Multi-Monitor Quirks

- Windows stores monitor state in the registry; some info may be stale
- `HMONITOR` values are not stable across session
- Use `DisplayId` (device adapter + index) for stable identification
- Handle monitor disconnection during layout operations

### Window Z-Order

- Use `HWND_TOP` for normal z-ordering
- Respect always-on-top windows with `HWND_TOPMOST`
- Do not insert windows below `HWND_NOTOPMOST`

### Message-Only Windows

- The message loop window should be **message-only** (`HWND_MESSAGE`)
- This prevents it from appearing in taskbar or Alt-Tab
- Use `SetParent` with `HWND_MESSAGE` or `CreateWindowExW` with `WS_POPUP`

### Timing Considerations

- `SetWindowPos` is synchronous by default
- Use `SWP_ASYNCWINDOWPOS` for deferring to another thread
- Avoid blocking the message loop during window operations

### Security Considerations

- Validate `HWND` values from external sources (messages)
- Do not trust `lParam` from `WM_*` messages without validation
- Avoid accessing window memory after `WindowDestroyed` event

---

## Performance Considerations

### Caching

- Cache window and monitor enumeration results
- Invalidate cache on `MonitorChanged` or major window events
- Use `RwLock` for thread-safe cached access

### Event Coalescing

- Coalesce rapid `WindowResized` events
- Use debouncing for window position updates
- Batch multiple `SetWindowPos` calls when possible

### Hook Efficiency

- Use `WH_GETMESSAGE` or `WH_CALLWNDPROC` instead of `WH_CBT` where sufficient
- Minimize work in hook callbacks (post to channel, do not block)
- Consider `SetWinEventHook` with `WINEVENT_OUTOFCONTEXT` for cross-process safety

---

## Testing Strategy

### Unit Tests
- Mock `WindowsBackend` with `Backend` trait
- Test window filtering logic
- Test rect calculations and comparisons

### Integration Tests
- Test with actual windows-rs bindings
- Use `cargo windows-test` for CI
- Spawn test applications and verify window management

### Manual Testing
- Verify correct window positioning
- Test multi-monitor scenarios
- Test DPI scaling behavior
- Verify hooks fire correctly for all event types
