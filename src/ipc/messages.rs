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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateResponse {
    pub windows: Vec<WindowInfoIpc>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub active_workspace: Option<String>,
}
