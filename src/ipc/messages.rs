// Pure IPC message types — no Win32, no tokio.
use serde::{Deserialize, Serialize};

// ---- Error type ----

#[derive(thiserror::Error, Debug)]
pub enum IpcError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Read error: {0}")]
    ReadError(String),
    #[error("Write error: {0}")]
    WriteError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Pipe creation failed: {0}")]
    PipeCreationFailed(String),
    #[error("Client disconnected")]
    ClientDisconnected,
    #[error("Timeout")]
    Timeout,
    #[error("Invalid message: {0}")]
    InvalidMessage(String),
    #[error("Windows API error: {0}")]
    WindowsError(String),
}

impl From<std::io::Error> for IpcError {
    fn from(e: std::io::Error) -> Self {
        IpcError::ReadError(e.to_string())
    }
}

impl From<serde_json::Error> for IpcError {
    fn from(e: serde_json::Error) -> Self {
        IpcError::SerializationError(e.to_string())
    }
}

// ---- Message types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IpcMessage {
    // ---- Window Management ----
    #[serde(rename = "tile_request")]
    TileRequest {
        window_hwnd: isize,
        target_workspace: Option<String>,
    },
    #[serde(rename = "window_move")]
    WindowMove {
        window_hwnd: isize,
        x: i32,
        y: i32,
    },
    #[serde(rename = "window_resize")]
    WindowResize {
        window_hwnd: isize,
        width: u32,
        height: u32,
    },
    #[serde(rename = "window_list")]
    WindowList,
    #[serde(rename = "window_find")]
    WindowFind {
        query: String,
    },
    #[serde(rename = "focus_window")]
    FocusWindow { window_hwnd: isize },
    #[serde(rename = "close_window")]
    CloseWindow { window_hwnd: isize },
    #[serde(rename = "window_float")]
    WindowFloat { window_hwnd: isize },
    #[serde(rename = "window_unfloat")]
    WindowUnfloat { window_hwnd: isize },
    #[serde(rename = "window_move_to_workspace")]
    WindowMoveToWorkspace {
        window_hwnd: isize,
        workspace_id: i32,
    },

    // ---- Workspace Management ----
    #[serde(rename = "workspace_list")]
    WorkspaceList,
    #[serde(rename = "workspace_create")]
    WorkspaceCreate { id: Option<i32> },
    #[serde(rename = "workspace_delete")]
    WorkspaceDelete { id: i32 },
    #[serde(rename = "switch_workspace")]
    SwitchWorkspace { id: i32 },

    // ---- Layout Presets ----
    #[serde(rename = "preset_list")]
    PresetList,
    #[serde(rename = "preset_save")]
    PresetSave { name: String },
    #[serde(rename = "preset_load")]
    PresetLoad { name: String },
    #[serde(rename = "preset_delete")]
    PresetDelete { name: String },
    #[serde(rename = "layout_export")]
    LayoutExport,

    // ---- Niri-parity Layout Commands ----
    #[serde(rename = "move_window_to_workspace")]
    MoveWindowToWorkspace { workspace_id: i32 },
    #[serde(rename = "move_window_to_monitor")]
    MoveWindowToMonitor { direction: String /* "left" or "right" */ },
    #[serde(rename = "center_column")]
    CenterColumn,
    #[serde(rename = "set_column_width")]
    SetColumnWidth { preset: String /* "1/3", "1/2", "2/3", "full", "cycle" */ },
    #[serde(rename = "resize_column")]
    ResizeColumn { delta_px: i32 },
    #[serde(rename = "focus_workspace_next")]
    FocusWorkspaceNext,
    #[serde(rename = "focus_workspace_previous")]
    FocusWorkspacePrevious,
    #[serde(rename = "spawn_command")]
    SpawnCommand { command: String },

    // ---- Niri-parity Round-2 ----
    #[serde(rename = "focus_previous")]
    FocusPrevious,
    #[serde(rename = "toggle_always_on_top")]
    ToggleAlwaysOnTop,
    #[serde(rename = "focus_workspace_named")]
    FocusWorkspaceNamed { name: String },
    #[serde(rename = "set_auto_tile_threshold")]
    SetAutoTileThreshold { threshold: Option<usize> },

    // ---- Niri-parity Round 4 (rearrange / sizing) ----
    /// Consume the focused tile into the column on its right.
    #[serde(rename = "consume_window")]
    ConsumeWindow,
    /// Take the focused tile out and place it in a new column to the right.
    #[serde(rename = "expel_window")]
    ExpelWindow,
    /// Expand the focused column to fill the remaining work-area width.
    #[serde(rename = "expand_column")]
    ExpandColumn,
    /// Toggle the focused column's maximize state (full work-area height).
    #[serde(rename = "maximize_column")]
    MaximizeColumn,
    /// Grow the focused column by 5% of the work-area width.
    #[serde(rename = "grow_column")]
    GrowColumn,
    /// Shrink the focused column by 5% of the work-area width.
    #[serde(rename = "shrink_column")]
    ShrinkColumn,
    /// Grow the focused tile's share of its column height by 5%.
    #[serde(rename = "grow_tile")]
    GrowTile,
    /// Shrink the focused tile's share of its column height by 5%.
    #[serde(rename = "shrink_tile")]
    ShrinkTile,
    /// Move the entire focused column to the monitor on the
    /// given side (`"left"` or `"right"`).
    #[serde(rename = "move_column_to_monitor")]
    MoveColumnToMonitor { direction: String },

    // ---- Diagnostics ----
    /// Enumerate every monitor the engine currently tracks, returning
    /// per-monitor geometry (bounds, work area), DPI scale factor, the
    /// currently active workspace id, and the active focused-output marker.
    /// Used by `wiri-ctl list-monitors`.
    #[serde(rename = "monitor_list")]
    MonitorList,

    // ---- System ----
    #[serde(rename = "get_state")]
    GetState,
    #[serde(rename = "subscribe_events")]
    SubscribeEvents {
        event_types: Vec<String>,
    },
    #[serde(rename = "reload_config")]
    ReloadConfig,
    #[serde(rename = "quit")]
    Quit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IpcEvent {
    #[serde(rename = "workspaces_changed")]
    WorkspacesChanged { workspaces: Vec<WorkspaceInfo> },
    #[serde(rename = "window_opened")]
    WindowOpened { window: WindowInfoIpc },
    #[serde(rename = "window_closed")]
    WindowClosed { window_hwnd: isize },
    #[serde(rename = "workspace_activated")]
    WorkspaceActivated { workspace: String },
    #[serde(rename = "config_loaded")]
    ConfigLoaded { config_path: String },
    #[serde(rename = "window_focused")]
    WindowFocused { window_hwnd: isize },
    #[serde(rename = "monitor_changed")]
    MonitorChanged { monitor_name: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowInfoIpc {
    pub hwnd: isize,
    pub title: String,
    pub class_name: String,
    pub process_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
    pub id: u32,
    pub window_count: usize,
}

/// Per-monitor diagnostic info returned by `MonitorList`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Engine-internal OutputId (a deterministic 64-bit name hash).
    pub output_id: u64,
    pub bounds_x: i32,
    pub bounds_y: i32,
    pub bounds_width: u32,
    pub bounds_height: u32,
    pub work_area_x: i32,
    pub work_area_y: i32,
    pub work_area_width: u32,
    pub work_area_height: u32,
    /// DPI scale factor (1.0 = 96 DPI, 1.5 = 144 DPI, 2.0 = 192 DPI).
    pub scale_factor: f64,
    /// Active workspace id on this monitor.
    pub active_workspace: i32,
    /// Total number of windows currently tiled on this monitor.
    pub window_count: usize,
    /// True for the focused output (where new window operations go).
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateResponse {
    pub windows: Vec<WindowInfoIpc>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub active_workspace: Option<String>,
}
