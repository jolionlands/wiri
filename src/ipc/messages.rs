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

    /// Enumerate every hotkey binding currently registered with Windows
    /// (after the most recent `register_hotkeys` pass, including hot
    /// reloads).  Returned data lets the user diagnose "which chord does
    /// wiri really listen for?" — particularly useful when a binding is
    /// silently dropped because RegisterHotKey rejected it (typical with
    /// Win+anything or chords reserved by an external tool).
    /// Used by `wiri-ctl test-bindings`.
    #[serde(rename = "list_bindings")]
    ListBindings,

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

    /// Capture a single window's bounding rect to a BMP file using
    /// `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)`.  When `path` is
    /// `None`, the server picks a default under
    /// `%USERPROFILE%\Pictures\wiri-window-<hwnd>-<unix>.bmp`.  Server
    /// response includes the resolved path under `result.path`.
    #[serde(rename = "capture_window")]
    CaptureWindow {
        window_hwnd: isize,
        #[serde(default)]
        path: Option<String>,
    },

    // ---- Snapshot / restore (niri-parity layout persistence) ----
    /// Serialise the engine's current monitor → workspace → column → tile
    /// structure to JSON at `%APPDATA%\wiri\snapshots\<name>.json`.
    #[serde(rename = "save_snapshot")]
    SaveSnapshot { name: String },
    /// Deserialise a previously-saved snapshot and reapply its tile layout.
    /// Windows whose HWNDs no longer exist are skipped silently so a
    /// snapshot taken yesterday can still restore today's surviving tiles.
    #[serde(rename = "load_snapshot")]
    LoadSnapshot { name: String },
    /// Enumerate every saved snapshot in `%APPDATA%\wiri\snapshots\`.
    #[serde(rename = "list_snapshots")]
    ListSnapshots,
    /// Delete a saved snapshot by name.
    #[serde(rename = "delete_snapshot")]
    DeleteSnapshot { name: String },

    /// Toggle niri-style interactive resize mode.  Returns the new state
    /// under `result.active`.
    #[serde(rename = "toggle_resize_mode")]
    ToggleResizeMode,

    /// niri parity: move the focused column to the workspace immediately
    /// above the current one (`current_workspace_id - 1`).  Creates the
    /// destination workspace if missing.
    #[serde(rename = "move_column_to_workspace_up")]
    MoveColumnToWorkspaceUp,
    /// niri parity: move the focused column to the workspace immediately
    /// below the current one (`current_workspace_id + 1`).
    #[serde(rename = "move_column_to_workspace_down")]
    MoveColumnToWorkspaceDown,

    /// niri parity: swap the focused workspace with the workspace
    /// immediately above it on the same monitor.  Focus follows the moved
    /// workspace.  No-op when the focused workspace is the lowest.
    #[serde(rename = "move_workspace_up")]
    MoveWorkspaceUp,
    /// niri parity: swap the focused workspace with the workspace
    /// immediately below it on the same monitor.
    #[serde(rename = "move_workspace_down")]
    MoveWorkspaceDown,
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

/// Diagnostic info for one registered hotkey binding (`ListBindings`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingInfo {
    /// Win32 hotkey id assigned by `RegisterHotKey`.
    pub id: i32,
    /// Win32 modifier bitmask: 1=Alt, 2=Ctrl, 4=Shift, 8=Win.
    pub modifiers: u32,
    /// Win32 virtual key code (e.g. 0x20 = Space, 0x1B = Escape).
    pub vk_code: u32,
    /// Human-readable action label (Debug-formatted `Action` enum variant).
    pub action: String,
    /// Pre-formatted "Ctrl+Alt+Space" style label for display.
    pub chord: String,
}

/// Response payload for `IpcMessage::ListBindings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBindingsResponse {
    pub bindings: Vec<BindingInfo>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_window_round_trip_with_path() {
        let msg = IpcMessage::CaptureWindow {
            window_hwnd: 0xDEAD_BEEF,
            path: Some(r"C:\tmp\out.bmp".to_string()),
        };
        let raw = serde_json::to_string(&msg).expect("serialize");
        assert!(raw.contains("capture_window"), "tag should be capture_window: {}", raw);
        let parsed: IpcMessage = serde_json::from_str(&raw).expect("deserialize");
        match parsed {
            IpcMessage::CaptureWindow { window_hwnd, path } => {
                assert_eq!(window_hwnd, 0xDEAD_BEEF);
                assert_eq!(path.as_deref(), Some(r"C:\tmp\out.bmp"));
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn capture_window_round_trip_default_path() {
        let msg = IpcMessage::CaptureWindow {
            window_hwnd: 7,
            path: None,
        };
        let raw = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&raw).expect("deserialize");
        match parsed {
            IpcMessage::CaptureWindow { window_hwnd, path } => {
                assert_eq!(window_hwnd, 7);
                assert!(path.is_none());
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn capture_window_path_missing_field_defaults_to_none() {
        // The `#[serde(default)]` on `path` lets older clients omit the key.
        let raw = r#"{"type":"capture_window","data":{"window_hwnd":42}}"#;
        let parsed: IpcMessage = serde_json::from_str(raw).expect("deserialize");
        match parsed {
            IpcMessage::CaptureWindow { window_hwnd, path } => {
                assert_eq!(window_hwnd, 42);
                assert!(path.is_none());
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }
}
