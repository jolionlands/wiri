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

/// The arguments of `run_action`: strings, or numbers and booleans standing for their text
/// (`[2]` is `["2"]`), so that a client written in a typed language can pass what niri's
/// own action arguments are.
fn deserialize_action_args<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    values
        .into_iter()
        .map(|value| match value {
            serde_json::Value::String(text) => Ok(text),
            serde_json::Value::Number(number) => Ok(number.to_string()),
            serde_json::Value::Bool(flag) => Ok(flag.to_string()),
            other => Err(D::Error::custom(format!(
                "an action argument is a string, a number or a boolean, not {}",
                other
            ))),
        })
        .collect()
}

// ---- Message types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IpcMessage {
    // ---- Window Management ----
    /// Force `window_hwnd` into the tiling layout, un-floating it if necessary.
    /// Optionally moves it to the named workspace afterwards.
    #[serde(rename = "tile_request")]
    TileRequest {
        window_hwnd: isize,
        target_workspace: Option<String>,
    },

    /// Move `window_hwnd` to the absolute screen position (`x`, `y`).
    #[serde(rename = "window_move")]
    WindowMove { window_hwnd: isize, x: i32, y: i32 },

    /// Resize `window_hwnd` to the given pixel dimensions.
    #[serde(rename = "window_resize")]
    WindowResize {
        window_hwnd: isize,
        width: u32,
        height: u32,
    },

    /// Return all windows currently tracked by the tiling engine.
    #[serde(rename = "window_list")]
    WindowList,

    /// Search tracked windows by title substring. Returns matching windows.
    #[serde(rename = "window_find")]
    WindowFind { query: String },

    /// Set keyboard focus to `window_hwnd`.
    #[serde(rename = "focus_window")]
    FocusWindow { window_hwnd: isize },

    /// Send `WM_CLOSE` to `window_hwnd`. The window may defer or ignore it.
    #[serde(rename = "close_window")]
    CloseWindow { window_hwnd: isize },

    /// Take `window_hwnd` out of the tiling layout into floating mode.
    #[serde(rename = "window_float")]
    WindowFloat { window_hwnd: isize },

    /// Return `window_hwnd` from floating mode back into the tiling layout.
    #[serde(rename = "window_unfloat")]
    WindowUnfloat { window_hwnd: isize },

    /// Move the window identified by `window_hwnd` to the given workspace id.
    #[serde(rename = "window_move_to_workspace")]
    WindowMoveToWorkspace {
        window_hwnd: isize,
        workspace_id: i32,
    },

    // ---- Workspace Management ----
    /// List all active workspaces (one per monitor). Returns an array of `WorkspaceInfo`.
    #[serde(rename = "workspace_list")]
    WorkspaceList,

    /// Create a new workspace. `id` is auto-assigned when `None`.
    #[serde(rename = "workspace_create")]
    WorkspaceCreate { id: Option<i32> },

    /// Delete the workspace with the given id. No-op when the workspace is empty.
    #[serde(rename = "workspace_delete")]
    WorkspaceDelete { id: i32 },

    /// Switch the focused monitor to workspace `id` (1–9 typical).
    #[serde(rename = "switch_workspace")]
    SwitchWorkspace { id: i32 },

    // ---- Layout Presets ----
    /// List all saved layout presets by name.
    #[serde(rename = "preset_list")]
    PresetList,

    /// Save the current layout as a named preset for later recall.
    #[serde(rename = "preset_save")]
    PresetSave { name: String },

    /// Apply a previously-saved named preset, restoring its layout.
    #[serde(rename = "preset_load")]
    PresetLoad { name: String },

    /// Delete a saved preset by name.
    #[serde(rename = "preset_delete")]
    PresetDelete { name: String },

    /// Export the current layout as a raw JSON object (for debugging).
    #[serde(rename = "layout_export")]
    LayoutExport,

    // ---- Niri-parity Layout Commands ----
    /// Move the focused window to the given workspace id.
    #[serde(rename = "move_window_to_workspace")]
    MoveWindowToWorkspace { workspace_id: i32 },

    /// Move the focused window to the monitor in the given direction (`"left"` or `"right"`).
    #[serde(rename = "move_window_to_monitor")]
    MoveWindowToMonitor {
        direction: String, /* "left" or "right" */
    },

    /// Scroll the focused column to the horizontal center of the work area.
    #[serde(rename = "center_column")]
    CenterColumn,

    /// Change the width of the focused column: niri's size argument (`"50%"`, `"+10%"`,
    /// `"-10%"`, `"1200"`, `"+100"`) or a width preset (`"1/3"`, `"1/2"`, `"2/3"`, `"full"`,
    /// `"cycle"`).
    #[serde(rename = "set_column_width")]
    SetColumnWidth {
        preset: String, /* "50%", "+10%", "1200", "1/3", "full", "cycle", ... */
    },

    /// Resize the focused column by `delta_px` pixels (positive = wider, negative = narrower).
    #[serde(rename = "resize_column")]
    ResizeColumn { delta_px: i32 },

    /// Switch the focused monitor to the workspace below (niri `focus-workspace-down`;
    /// nothing happens at the bottom).
    #[serde(rename = "focus_workspace_next")]
    FocusWorkspaceNext,

    /// Switch the focused monitor to the workspace above (niri `focus-workspace-up`;
    /// nothing happens at the top).
    #[serde(rename = "focus_workspace_previous")]
    FocusWorkspacePrevious,

    /// Spawn an external process via the shared process spawner.
    ///
    /// With `args` empty, `command` is a whole command line, split with the
    /// Windows quoting rules (`"C:\Program Files\x\x.exe" -d "a b"`).  With
    /// `args`, `command` is the program alone and `args` are its arguments,
    /// each passed through intact.
    #[serde(rename = "spawn_command")]
    SpawnCommand {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },

    /// Run any action by its niri name, exactly as if its key were pressed
    /// (`"focus-column-left"`, `"set-column-width"` with `["+10%"]`,
    /// `"focus-workspace"` with `[2]`, ...).  The arguments are the words
    /// that follow the action name in a `binds {}` line; a number or a boolean
    /// stands for its text (`2` is `"2"`).  `quoted[i]` says that `args[i]` was
    /// written in double quotes: `focus-workspace 2` is the second workspace,
    /// `focus-workspace "2"` the workspace named "2".  A window can be named
    /// with `--id <hwnd>` (niri's `--id`; `--window-id` where niri has that
    /// spelling), see `docs/IPC_PROTOCOL.md`.  The message is also accepted
    /// as `{"type":"action","data":{"action":...}}`.
    #[serde(rename = "run_action", alias = "action")]
    RunAction {
        #[serde(alias = "action")]
        name: String,
        #[serde(default, deserialize_with = "deserialize_action_args")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        quoted: Vec<bool>,
    },

    // ---- Niri-parity Round-2 ----
    /// Focus the previously focused window using the MRU history (like Alt+Tab).
    #[serde(rename = "focus_previous")]
    FocusPrevious,

    /// Toggle the always-on-top (`HWND_TOPMOST`) flag for the focused window.
    #[serde(rename = "toggle_always_on_top")]
    ToggleAlwaysOnTop,

    /// Switch to the workspace whose name matches `name` as defined in config.
    #[serde(rename = "focus_workspace_named")]
    FocusWorkspaceNamed { name: String },

    /// Set the window-count threshold above which auto-tile activates.
    /// `None` disables auto-tile entirely.
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
    /// Return the full engine state: managed windows, workspaces, and the active workspace name.
    #[serde(rename = "get_state")]
    GetState,

    /// Open a long-lived event subscription. The pipe stays open; the server
    /// streams newline-delimited JSON events until the client disconnects.
    /// An empty `event_types` list (or `["*"]`) subscribes to all event types.
    #[serde(rename = "subscribe_events")]
    SubscribeEvents { event_types: Vec<String> },

    /// Hot-reload `config.kdl` without restarting the daemon.
    /// A `config_loaded` event is broadcast after a successful reload.
    #[serde(rename = "reload_config")]
    ReloadConfig,

    /// Gracefully shut down the wiri daemon. Sends a oneshot signal to `main`.
    #[serde(rename = "quit")]
    Quit,

    /// Capture a tracked window to PNG and the clipboard (WGC, PrintWindow fallback).
    /// When `path` is `None`, screenshot-path decides the destination, including
    /// clipboard-only. Response result.path is the resolved path or null.
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

    // ---- Round-4 parity ----
    /// Toggle sticky (visible on all workspaces) for the focused window.
    /// A sticky window appears on every workspace on its monitor.
    #[serde(rename = "toggle_sticky")]
    ToggleSticky,

    /// Set the active workspace's layout mode.
    /// Accepted `mode` values: `"scrolling"` (default), `"bstack"`, `"spiral"`.
    #[serde(rename = "set_workspace_layout")]
    SetWorkspaceLayout { mode: String },

    /// Rename a workspace by its numeric id. The new name is persisted in engine
    /// state and broadcast via a `workspaces_changed` event.
    #[serde(rename = "rename_workspace")]
    RenameWorkspace { workspace_id: i32, name: String },

    /// Screenshot a window (by optional HWND) to an optional output path.
    /// When `hwnd` is `None`, the focused window is captured.
    /// When `path` is `None`, screenshot-path applies (Pictures default or clipboard-only).
    #[serde(rename = "screenshot_window")]
    ScreenshotWindow {
        #[serde(default)]
        hwnd: Option<isize>,
        #[serde(default)]
        path: Option<String>,
    },

    /// Return the currently focused window HWND, workspace id, and monitor OutputId.
    /// Returns `null` for `window_hwnd` when nothing is focused.
    #[serde(rename = "get_focus")]
    GetFocus,

    /// List all workspaces across all monitors with their ids, names, and tiled window counts.
    #[serde(rename = "get_workspace_list")]
    GetWorkspaceList,

    /// Return the live hotkey bindings table in a format suited for the
    /// cheatsheet overlay and `wiri-ctl bindings`.  Each entry carries a
    /// human-readable `chord` string plus the action label.
    /// Falls back to an empty list when the daemon has not yet completed
    /// hotkey registration.
    #[serde(rename = "get_bindings")]
    GetBindings,

    /// Reserve screen-edge pixels for an external bar or panel.
    ///
    /// `side` must be one of `"top"`, `"bottom"`, `"left"`, `"right"`.
    /// `pixels` is the number of logical pixels to subtract from each
    /// monitor's tiling work area on that edge.  Pass `pixels: 0` to release
    /// a previous reservation.
    ///
    /// `monitor_id` is accepted but ignored in v1 (reservation is global).
    ///
    /// The reservation is **in-memory only** — it is not written to
    /// `config.kdl` and is reset when wiri restarts.  External bars should
    /// re-issue this message on every wiri reconnect.
    #[serde(rename = "reserve_area")]
    ReserveArea {
        /// Which edge to reserve: `"top"` | `"bottom"` | `"left"` | `"right"`.
        side: String,
        /// Number of pixels to reserve on that edge.
        pixels: u32,
        /// Optional monitor ID (numeric). Currently ignored — reservation
        /// applies globally to all monitors.
        // TODO(audit): per-monitor reservations
        #[serde(default)]
        monitor_id: Option<u64>,
    },

    // ---- niri's request/reply forms (IPC schema 1.2) ----
    //
    // The requests above answer with wiri's own shapes.  These five answer
    // with niri's: the same objects the event stream describes (niri's
    // `Workspace` and `Window`), so that a client written against niri reads
    // them without knowing wiri.
    /// The workspaces of every monitor, in niri's shape.
    #[serde(rename = "workspaces", alias = "Workspaces")]
    Workspaces,

    /// The windows wiri manages, in niri's shape.
    #[serde(rename = "windows", alias = "Windows")]
    Windows,

    /// The window that has the keyboard focus, in niri's shape (`null` when no
    /// window wiri manages has it).
    #[serde(rename = "focused_window", alias = "FocusedWindow")]
    FocusedWindow,

    /// Whether the overview is open.
    #[serde(rename = "overview_state", alias = "OverviewState")]
    OverviewState,

    /// Deliberately return an error, for client error-path checks.
    #[serde(rename = "return_error", alias = "ReturnError")]
    ReturnError,

    /// The version of the running daemon.
    #[serde(rename = "version", alias = "Version")]
    Version,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum IpcEvent {
    // ---- niri's event stream: the state first (replayed to a new subscriber), then deltas ----
    /// niri `WorkspacesChanged`: the workspaces were created, removed, renamed, moved or
    /// reordered.  Carries the full list, which replaces what the client knew.
    #[serde(rename = "workspaces_changed")]
    WorkspacesChanged { workspaces: Vec<WorkspaceIpc> },

    /// niri `WorkspaceUrgencyChanged`: a window of the workspace asked for attention, or the
    /// last one that did stopped.
    #[serde(rename = "workspace_urgency_changed")]
    WorkspaceUrgencyChanged { id: u64, urgent: bool },

    /// niri `WorkspaceActivated`: the workspace became the active one of its output.
    /// `focused` says that it is now the focused workspace (the active one of the focused
    /// output): every other workspace is not focused any more, but may stay active on its own
    /// output.  Sent as well when the focus moves to another output.
    #[serde(rename = "workspace_activated")]
    WorkspaceActivated { id: u64, focused: bool },

    /// niri `WorkspaceActiveWindowChanged`: the window that is the active one of the
    /// workspace changed (`null`: none).
    #[serde(rename = "workspace_active_window_changed")]
    WorkspaceActiveWindowChanged {
        workspace_id: u64,
        active_window_id: Option<u64>,
    },

    /// niri `WindowsChanged`: every window that wiri manages.  Replaces what the client
    /// knew; part of the replay a new subscriber gets first.
    #[serde(rename = "windows_changed")]
    WindowsChanged { windows: Vec<WindowIpc> },

    /// niri `WindowOpenedOrChanged`: a window appeared, or its title, application,
    /// workspace, floating state (and wiri's fullscreen, maximized and minimized states)
    /// changed.  Focus, urgency and the place in the strip have events of their own.
    #[serde(rename = "window_opened_or_changed")]
    WindowOpenedOrChanged { window: WindowIpc },

    /// niri `WindowFocusChanged`: the window that has the keyboard focus changed (`null`:
    /// a window wiri does not manage has it, or none).
    #[serde(rename = "window_focus_changed")]
    WindowFocusChanged { id: Option<u64> },

    /// niri `WindowUrgencyChanged`: the window asks for attention, or stopped.
    #[serde(rename = "window_urgency_changed")]
    WindowUrgencyChanged { id: u64, urgent: bool },

    /// niri `WindowLayoutsChanged`: the windows' places in their strips changed.
    #[serde(rename = "window_layouts_changed")]
    WindowLayoutsChanged {
        changes: Vec<(u64, WindowLayoutIpc)>,
    },

    /// niri `OverviewOpenedOrClosed`.
    #[serde(rename = "overview_opened_or_closed")]
    OverviewOpenedOrClosed { is_open: bool },

    /// niri ScreenshotCaptured: path is null when the image was only copied to the clipboard.
    #[serde(rename = "screenshot_captured")]
    ScreenshotCaptured { path: Option<String> },

    // ---- wiri's own events (kept from before the niri ones; they carry the window handle) ----
    /// Fired when a new window is added to the tiling engine layout.
    /// Includes full `WindowInfoIpc` with title, class, PID, and initial geometry.
    #[serde(rename = "window_opened")]
    WindowOpened { window: WindowInfoIpc },

    /// niri `WindowClosed`: a window wiri managed is gone, destroyed or taken out of the
    /// layout.  `window_hwnd` and `id` are the same number (the window handle).
    #[serde(rename = "window_closed")]
    WindowClosed {
        window_hwnd: isize,
        #[serde(default)]
        id: u64,
    },

    /// Fired when `config.kdl` is hot-reloaded, by the file watcher, `reload_config` or the
    /// tray menu.  niri's `ConfigLoaded { failed }`: `failed` is true when the file has errors
    /// and the last good config stays in use; `error` then says where the first error is
    /// (`file:line:column: error: message`).  `warnings` counts what the file asks for that
    /// wiri ignores.
    #[serde(rename = "config_loaded")]
    ConfigLoaded {
        config_path: String,
        #[serde(default)]
        failed: bool,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        warnings: u32,
    },

    /// Fired when keyboard focus moves to a different tracked window.
    #[serde(rename = "window_focused")]
    WindowFocused { window_hwnd: isize },

    /// Fired when the focused output (monitor) changes — i.e. focus crossed a monitor boundary.
    #[serde(rename = "monitor_changed")]
    MonitorChanged { monitor_name: String },
    /// Emitted when a tiled window is moved by the engine or externally.
    /// `x`/`y` are the new top-left in screen pixels; `width`/`height` are
    /// the new client dimensions in physical pixels.
    #[serde(rename = "window_moved")]
    WindowMoved {
        window_hwnd: isize,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    /// Emitted when a tiled window's size changes (width or height differs
    /// from the previous layout pass).
    #[serde(rename = "window_resized")]
    WindowResized {
        window_hwnd: isize,
        width: u32,
        height: u32,
    },
    /// Emitted when the user begins an interactive move/resize drag on a window.
    #[serde(rename = "window_move_resize_start")]
    WindowMoveResizeStart { window_hwnd: isize },
    /// Emitted when the user finishes an interactive move/resize drag.
    #[serde(rename = "window_move_resize_end")]
    WindowMoveResizeEnd { window_hwnd: isize },
    /// Emitted when a window signals an urgent / attention-request state.
    /// `urgent: false` means the urgency flag was cleared.
    #[serde(rename = "window_urgent")]
    WindowUrgent { window_hwnd: isize, urgent: bool },
    /// Emitted when the active workspace on a monitor changes.
    #[serde(rename = "workspace_switched")]
    WorkspaceSwitched { workspace_id: i32, monitor_id: u64 },
    /// Emitted when the focused column changes (derived from ForegroundChanged).
    #[serde(rename = "column_focused")]
    ColumnFocused {
        window_hwnd: isize,
        column_index: usize,
        workspace_id: i32,
    },
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

/// niri's `WindowLayout`, the part wiri knows: where the window sits in its workspace's
/// strip.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowLayoutIpc {
    /// Column and tile of the window in the scrolling layout, both counted from 1; `null`
    /// for a floating or minimized window.
    pub pos_in_scrolling_layout: Option<(usize, usize)>,
}

/// A window as the event stream describes it: niri's `Window` (`id`, `title`, `app_id`,
/// `pid`, `workspace_id`, `is_focused`, `is_floating`, `is_urgent`, `layout`), plus what
/// wiri adds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowIpc {
    /// The window handle (HWND) as a number; it is what `--id` takes in `run_action`.
    pub id: u64,
    /// The title; `null` while the window has none.
    pub title: Option<String>,
    /// The file name of the window's program (`WindowsTerminal.exe`): the closest thing
    /// Windows has to a Wayland `app_id`, and what the `app-id` of a window rule matches
    /// first.  `null` when it cannot be read (an elevated program).
    pub app_id: Option<String>,
    pub pid: Option<u32>,
    /// The workspace the window is on (see [`WorkspaceIpc::id`]).
    pub workspace_id: Option<u64>,
    pub is_focused: bool,
    pub is_floating: bool,
    pub is_urgent: bool,
    pub layout: WindowLayoutIpc,
    // ---- what wiri adds ----
    /// The Win32 window class (`CASCADIA_HOSTING_WINDOW_CLASS`).
    pub class_name: String,
    /// The device name of the monitor the window is on (`\\.\DISPLAY1`).
    pub output: Option<String>,
    pub is_fullscreen: bool,
    /// Maximized to the edges of the work area (niri `maximize-window-to-edges`).
    pub is_maximized: bool,
    pub is_minimized: bool,
}

/// A workspace as the event stream describes it: niri's `Workspace`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceIpc {
    /// Unique among all workspaces of all monitors, and the same for as long as the
    /// workspace exists (an unplugged monitor's workspaces get new ids on the monitor that
    /// takes them).  Small enough for a JavaScript number.
    pub id: u64,
    /// The number of the workspace on its monitor, counted from 1 at the top: what
    /// `focus-workspace 2` means.
    pub idx: u32,
    pub name: Option<String>,
    /// The device name of the monitor the workspace is on (`\\.\DISPLAY1`).
    pub output: Option<String>,
    pub is_urgent: bool,
    /// The workspace is the active one of its monitor.
    pub is_active: bool,
    /// The workspace is the active one of the focused monitor.
    pub is_focused: bool,
    pub active_window_id: Option<u64>,
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
    /// The device name of the monitor (`\\.\DISPLAY1`): what `open-on-output` in a window
    /// rule names.  Empty when the engine was not told.
    #[serde(default)]
    pub name: String,
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

// ---- the replies of the niri-shaped requests (IPC schema 1.2) ----
//
// Each of them is what the request answers with under `result`.  The window and
// workspace objects are the ones the event stream describes (niri's `Window` and
// `Workspace`), so a client reads the same fields in the reply and in the
// stream.

/// The reply of the `workspaces` request: the workspaces of every monitor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacesResponse {
    pub workspaces: Vec<WorkspaceIpc>,
}

/// The reply of the `windows` request: every window wiri manages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsResponse {
    pub windows: Vec<WindowIpc>,
}

/// The reply of the `focused_window` request: the window that has the keyboard
/// focus, or `null` when no window wiri manages has it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusedWindowResponse {
    pub focused_window: Option<WindowIpc>,
}

/// The reply of the `overview_state` request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverviewStateResponse {
    pub is_open: bool,
}

/// The reply of the `version` request: the version of the running daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionResponse {
    pub version: String,
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
        assert!(
            raw.contains("capture_window"),
            "tag should be capture_window: {}",
            raw
        );
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
    fn run_action_and_spawn_accept_missing_args() {
        // Old clients send `spawn_command` without `args`; `run_action` may omit them too.
        let spawn: IpcMessage =
            serde_json::from_str(r#"{"type":"spawn_command","data":{"command":"wt.exe"}}"#)
                .expect("deserialize spawn_command");
        match spawn {
            IpcMessage::SpawnCommand { command, args } => {
                assert_eq!(command, "wt.exe");
                assert!(args.is_empty());
            }
            other => panic!("unexpected variant: {:?}", other),
        }
        let run: IpcMessage =
            serde_json::from_str(r#"{"type":"run_action","data":{"name":"close-window"}}"#)
                .expect("deserialize run_action");
        match run {
            IpcMessage::RunAction { name, args, quoted } => {
                assert_eq!(name, "close-window");
                assert!(args.is_empty());
                assert!(quoted.is_empty());
            }
            other => panic!("unexpected variant: {:?}", other),
        }
        let with_args = IpcMessage::RunAction {
            name: "set-column-width".to_string(),
            args: vec!["+10%".to_string()],
            quoted: Vec::new(),
        };
        let raw = serde_json::to_string(&with_args).expect("serialize");
        assert!(raw.contains("\"run_action\""), "{}", raw);
        assert!(
            !raw.contains("quoted"),
            "an empty `quoted` is not written: {}",
            raw
        );
        let back: IpcMessage = serde_json::from_str(&raw).expect("deserialize");
        match back {
            IpcMessage::RunAction { name, args, .. } => {
                assert_eq!(name, "set-column-width");
                assert_eq!(args, vec!["+10%".to_string()]);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    fn run_action_of(raw: &str) -> (String, Vec<String>, Vec<bool>) {
        match serde_json::from_str::<IpcMessage>(raw).expect("deserialize") {
            IpcMessage::RunAction { name, args, quoted } => (name, args, quoted),
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn an_action_takes_numbers_and_booleans_as_their_text() {
        // `niri msg action focus-workspace 2` passes a number; a typed client sends one.
        let (name, args, _) =
            run_action_of(r#"{"type":"run_action","data":{"name":"focus-workspace","args":[2]}}"#);
        assert_eq!(name, "focus-workspace");
        assert_eq!(args, vec!["2".to_string()]);
        let (_, args, _) = run_action_of(
            r#"{"type":"run_action","data":{"name":"spawn","args":["wt.exe","-d",3,true,0.5]}}"#,
        );
        assert_eq!(args, vec!["wt.exe", "-d", "3", "true", "0.5"]);
        // Anything else is not an argument.
        let bad = serde_json::from_str::<IpcMessage>(
            r#"{"type":"run_action","data":{"name":"spawn","args":[["a"]]}}"#,
        );
        assert!(bad.is_err(), "an array is not an action argument");
        let bad = serde_json::from_str::<IpcMessage>(
            r#"{"type":"run_action","data":{"name":"spawn","args":[null]}}"#,
        );
        assert!(bad.is_err(), "null is not an action argument");
    }

    #[test]
    fn an_action_is_also_spelled_action() {
        let (name, args, quoted) = run_action_of(
            r#"{"type":"action","data":{"action":"focus-workspace","args":["2"],"quoted":[true]}}"#,
        );
        assert_eq!(name, "focus-workspace");
        assert_eq!(args, vec!["2".to_string()]);
        assert_eq!(
            quoted,
            vec![true],
            "the workspace named \"2\", not the second one"
        );
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

    // ---- the niri-shaped requests and their replies (IPC schema 1.2) ----

    /// A window as the event stream describes it: everything wiri knows about it.
    fn sample_window() -> WindowIpc {
        WindowIpc {
            id: 42,
            title: Some("A window".into()),
            app_id: Some("wt.exe".into()),
            pid: Some(1234),
            workspace_id: Some(7),
            is_focused: true,
            is_floating: false,
            is_urgent: false,
            layout: WindowLayoutIpc {
                pos_in_scrolling_layout: Some((1, 1)),
            },
            class_name: "CASCADIA_HOSTING_WINDOW_CLASS".into(),
            output: Some("\\\\.\\DISPLAY1".into()),
            is_fullscreen: false,
            is_maximized: false,
            is_minimized: false,
        }
    }

    /// A workspace as the event stream describes it.
    fn sample_workspace() -> WorkspaceIpc {
        WorkspaceIpc {
            id: 7,
            idx: 1,
            name: None,
            output: Some("\\\\.\\DISPLAY1".into()),
            is_urgent: false,
            is_active: true,
            is_focused: true,
            active_window_id: Some(42),
        }
    }

    #[test]
    fn the_niri_requests_are_named_the_way_wiri_names_them() {
        let cases = [
            (IpcMessage::Workspaces, "workspaces"),
            (IpcMessage::Windows, "windows"),
            (IpcMessage::FocusedWindow, "focused_window"),
            (IpcMessage::OverviewState, "overview_state"),
            (IpcMessage::Version, "version"),
        ];
        for (message, name) in cases {
            let json = serde_json::to_string(&message).expect("serialize");
            assert_eq!(json, format!("{{\"type\":\"{}\"}}", name));
            let back: IpcMessage = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(json, serde_json::to_string(&back).expect("serialize"));
        }
    }

    #[test]
    fn the_niri_requests_are_read_in_niris_spelling_too() {
        // niri writes the request as `"Workspaces"`; wiri writes `"workspaces"`.
        for (raw, name) in [
            (r#"{"type":"Workspaces"}"#, "workspaces"),
            (r#"{"type":"Windows"}"#, "windows"),
            (r#"{"type":"FocusedWindow"}"#, "focused_window"),
            (r#"{"type":"OverviewState"}"#, "overview_state"),
            (r#"{"type":"Version"}"#, "version"),
        ] {
            let parsed: IpcMessage = serde_json::from_str(raw).expect("deserialize");
            assert_eq!(
                serde_json::to_string(&parsed).expect("serialize"),
                format!("{{\"type\":\"{}\"}}", name)
            );
        }
    }

    #[test]
    fn the_workspaces_reply_has_the_fields_niri_has() {
        let reply = WorkspacesResponse {
            workspaces: vec![sample_workspace()],
        };
        let json = serde_json::to_value(&reply).expect("serialize");
        let workspace = &json["workspaces"][0];
        for field in [
            "id",
            "idx",
            "name",
            "output",
            "is_active",
            "is_focused",
            "active_window_id",
        ] {
            assert!(
                workspace.get(field).is_some(),
                "workspace.{} is missing: {}",
                field,
                workspace
            );
        }
        // A workspace without a name says so with null; the key is not left out.
        assert!(workspace["name"].is_null());
        assert_eq!(workspace["active_window_id"], serde_json::json!(42));
        let back: WorkspacesResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, reply);
    }

    #[test]
    fn the_windows_reply_has_the_fields_niri_has() {
        let reply = WindowsResponse {
            windows: vec![sample_window()],
        };
        let json = serde_json::to_value(&reply).expect("serialize");
        let window = &json["windows"][0];
        for field in [
            "id",
            "title",
            "app_id",
            "pid",
            "workspace_id",
            "is_focused",
            "is_floating",
        ] {
            assert!(
                window.get(field).is_some(),
                "window.{} is missing: {}",
                field,
                window
            );
        }
        assert_eq!(window["id"], serde_json::json!(42));
        assert_eq!(window["workspace_id"], serde_json::json!(7));
        let back: WindowsResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, reply);
    }

    #[test]
    fn what_windows_cannot_tell_is_null_and_not_missing() {
        // A program whose name cannot be read (an elevated one), a window whose
        // process is unknown, a floating window on a monitor that is away: the
        // field is there and says "unknown".
        let mut window = sample_window();
        window.app_id = None;
        window.pid = None;
        window.workspace_id = None;
        window.title = None;
        let json = serde_json::to_value(WindowsResponse {
            windows: vec![window],
        })
        .expect("serialize");
        let window = &json["windows"][0];
        for field in ["title", "app_id", "pid", "workspace_id"] {
            assert!(
                window.get(field).is_some(),
                "window.{} is missing: {}",
                field,
                window
            );
            assert!(window[field].is_null(), "window.{} should be null", field);
        }
    }

    #[test]
    fn the_focused_window_reply_is_null_when_no_window_is_focused() {
        let reply = FocusedWindowResponse {
            focused_window: None,
        };
        let json = serde_json::to_value(&reply).expect("serialize");
        assert!(json.get("focused_window").is_some(), "{}", json);
        assert!(json["focused_window"].is_null());
        let back: FocusedWindowResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, reply);

        let reply = FocusedWindowResponse {
            focused_window: Some(sample_window()),
        };
        let json = serde_json::to_value(&reply).expect("serialize");
        assert_eq!(json["focused_window"]["id"], serde_json::json!(42));
        let back: FocusedWindowResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, reply);
    }

    #[test]
    fn the_overview_and_the_version_say_what_they_are() {
        let json =
            serde_json::to_value(OverviewStateResponse { is_open: true }).expect("serialize");
        assert_eq!(json["is_open"], serde_json::json!(true));
        let back: OverviewStateResponse = serde_json::from_value(json).expect("deserialize");
        assert!(back.is_open);

        let json = serde_json::to_value(VersionResponse {
            version: "0.1.0 (commit 1234567)".into(),
        })
        .expect("serialize");
        assert_eq!(json["version"], serde_json::json!("0.1.0 (commit 1234567)"));
        let back: VersionResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.version, "0.1.0 (commit 1234567)");
    }
}
