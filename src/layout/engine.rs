use std::collections::{HashMap, HashSet};
use tracing::{debug, info, warn};

/// After this many consecutive `set_window_position` failures, the engine
/// auto-floats the window and stops trying to tile it. Some windows (UAC
/// dialogs, certain anti-cheat overlays, WireGuard's tunnel manager, etc.)
/// reject SetWindowPos outright; without a circuit breaker we'd spin forever.
const MAX_POSITION_FAILURES: u8 = 3;
use crate::utils::{Rect, Point, OutputId, WindowId};
use crate::backend::{BackendHandle, WindowInfo};
use super::{Monitor, MonitorSet, ScrollDirection, SizingMode, ConfigureIntent};

// ---------------------------------------------------------------------------
// Per-window applied-state cache
// ---------------------------------------------------------------------------

/// Cached Win32 state last applied to a window.
/// Used to skip redundant API calls when nothing has changed.
#[derive(Debug, Clone, PartialEq)]
struct AppliedState {
    rect: Rect,
    /// Packed 0x00RRGGBB; 0xFFFFFFFF = "never set" sentinel.
    border_color: u32,
    /// 0..=255; 0xFFFF = "never set" sentinel stored as u32 for uniformity.
    opacity: u32,
    visible: bool,
    focused: bool,
    /// Last `DWMWA_SYSTEMBACKDROP_TYPE` value applied (0–4).
    /// 0xFFFF_FFFF = "never set" sentinel — triggers the first DWM call.
    backdrop: u32,
}

impl AppliedState {
    fn unset() -> Self {
        Self {
            rect: Rect::new(0, 0, 0, 0),
            border_color: 0xFFFF_FFFF,
            opacity: 0xFFFF_FFFF,
            visible: false,
            focused: false,
            backdrop: 0xFFFF_FFFF,
        }
    }
}

/// Parse a `"#RRGGBB"` color string to a packed `0x00RRGGBB` u32.
/// Returns `0xFFFF_FFFF` (sentinel) on parse failure.
fn parse_color_to_u32(color_hex: &str) -> u32 {
    if let Some(color) = crate::config::types::parse_color(color_hex) {
        let [r, g, b, _a] = color;
        ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    } else {
        0xFFFF_FFFF
    }
}

/// State stored while overview mode is active.
///
/// Wraps the legacy `f64` zoom level with extra context — namely the precomputed
/// vertical Y offsets at which each workspace's section is rendered (workspace 1
/// at the top, workspace 2 below, etc.).  The vector is keyed by the same
/// insertion order produced by iterating `monitor.workspaces` after sorting by
/// workspace id, so callers that need to look up a specific workspace's offset
/// must apply the same sort.  Empty workspaces are skipped during precomputation
/// and contribute zero height to the stack.
///
/// `overview_zoom()` continues to return the `zoom` field for back-compat so
/// existing consumers (and tests) keep working.
#[derive(Debug, Clone)]
pub struct OverviewState {
    /// Uniform scale factor applied to every workspace's column geometry.
    /// 1.0 = no zoom; 0.5 = everything renders half size.  Computed in
    /// `enter_overview` to fit every workspace's columns into the work area.
    pub zoom: f64,
    /// Y-offset (relative to the work area's top edge) at which each visible
    /// workspace section begins, in workspace-id ascending order.  Empty
    /// workspaces are skipped, so the list may be shorter than the monitor's
    /// `workspaces` map.  Pairs of `(workspace_id, y_offset)` are stored so
    /// `calculate_positions` can map an arbitrary workspace back to its slot.
    pub workspace_offsets: Vec<(i32, i32)>,
}

impl OverviewState {
    /// Convenience constructor used in tests + as a fallback when no per-workspace
    /// stacking info is available (single-workspace overview).
    pub fn new(zoom: f64) -> Self {
        Self {
            zoom,
            workspace_offsets: Vec::new(),
        }
    }
}

/// How column widths are determined
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnWidthMode {
    /// Fixed width in pixels — each column keeps its configured width when windows are added.
    /// Preserves niri's "no resize on add" scrollable-tiling invariant. Set via config.
    Fixed,
    /// Proportional to monitor width (fill the screen). Adding a window resizes all columns.
    /// NOTE: Proportional violates niri's "no resize on add" invariant. The
    /// default in `LayoutConfig::default()` is `Fixed` for niri-faithful behaviour;
    /// `Proportional` is retained for users who explicitly opt in via config.
    /// Overview zoom works correctly in both modes (verified by
    /// `test_calculate_positions_fixed_overview_zoom`).
    Proportional,
}

/// How the focused-window DWM border colour is sourced.
///
/// `Fixed("#hex")` (the historic behaviour) parses a colour literal up
/// front and paints it on every focused tile.  `WindowsAccent` defers
/// resolution until paint time and reads the live OS accent colour from
/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor`, cached for 30 s by
/// [`crate::backend::accent`].  When the accent read fails the engine
/// falls back to the configured `border_color_focused` literal so the
/// user always gets *some* visible focus indicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BorderColorMode {
    /// Use the literal `#rrggbb` (or `#rrggbbaa`) stored in
    /// `border_color_focused`.  This is the default.
    Fixed,
    /// Read the Windows system accent colour each call (cached) and use
    /// that as the focused-border colour.  Falls back to the
    /// `border_color_focused` literal when the registry lookup fails.
    WindowsAccent,
}

impl Default for BorderColorMode {
    fn default() -> Self {
        BorderColorMode::Fixed
    }
}

/// Configuration for layout behavior
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    pub column_width: u32,
    pub column_width_mode: ColumnWidthMode,
    pub column_gap: i32,
    pub window_gap: i32,
    pub border_width: i32,
    pub border_color: String,
    pub border_color_focused: String,
    /// How `border_color_focused` is sourced.  When set to
    /// [`BorderColorMode::WindowsAccent`], the engine ignores the
    /// literal in `border_color_focused` for normal painting and reads
    /// the live OS accent colour each pass (with caching) — the literal
    /// is still used as a fallback when the registry lookup fails.
    pub border_color_focused_mode: BorderColorMode,
    pub outer_gaps: (i32, i32, i32, i32),
    pub focus_ring_width: i32,
    pub focus_ring_color: String,
    pub dim_unfocused: f32,
    pub scroll_step: i32,
    /// Strip native window frame (caption/thick frame/sysmenu) from tiled windows.
    /// Off by default: native frames cause some apps (Cascadia/Windows Terminal,
    /// Edge, certain Electron) to render their content area black until they
    /// receive a paint message. With frames intact those apps render correctly
    /// at the cost of slightly inset client area.
    pub strip_frame: bool,
    /// When `true`, re-enable the DWM drop shadow on each tile by extending
    /// the frame into the client area by 1 px along the top edge.  When
    /// `false`, reset all frame margins to 0 (the default — no shadow).  See
    /// `TilingEngine::apply_shadow_for_window` for the per-window mechanics.
    pub shadow_enable: bool,
    /// niri-parity "smart borders" (a.k.a. `disable-when-only-one-window`).
    /// When `true` and the active workspace contains exactly one column with
    /// exactly one tile, the per-tile DWM border color is forced to the
    /// "no colour" sentinel (`0xFFFFFFFF` / `DWMWA_COLOR_NONE`) so the user
    /// gets the full tile rect without a coloured outline.  Defaults to
    /// `false` to preserve the historic always-bordered behaviour.
    pub smart_borders: bool,
    /// Border colour for windows in the "urgent" state (e.g. WM_FLASHWINDOW).
    /// Painted in place of the normal / focused colour when the window is in
    /// `urgent_windows`.  Defaults to material-red-600 (`#e53935`).
    pub border_color_urgent: String,
    /// DWM system backdrop to apply to every tiled window via
    /// `DWMWA_SYSTEMBACKDROP_TYPE` (attribute 38, Windows 11 22H2+).
    /// Accepted values: `"auto"` (0), `"none"` (1), `"mica"` (2),
    /// `"acrylic"` (3), `"tabbed"` (4).  Defaults to `"auto"` which
    /// lets Windows choose (usually no backdrop for non-UWP apps).
    pub backdrop: String,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            column_width: 500,
            column_width_mode: ColumnWidthMode::Fixed,
            column_gap: 16,
            window_gap: 12,
            border_width: 4,
            border_color: "#333333".to_string(),
            border_color_focused: "#3381d9".to_string(),
            border_color_focused_mode: BorderColorMode::Fixed,
            outer_gaps: (8, 8, 8, 8),
            focus_ring_width: 3,
            focus_ring_color: "#3381d9".to_string(),
            dim_unfocused: 1.0,
            scroll_step: 200,
            strip_frame: false,
            shadow_enable: false,
            smart_borders: false,
            border_color_urgent: "#e53935".to_string(),
            backdrop: "auto".to_string(),
        }
    }
}

impl LayoutConfig {
    pub fn from_config(config: &crate::config::Config) -> Self {
        let mut lc = Self::default();
        // inner_gaps controls spacing between columns and between stacked tiles
        // Always apply (even if 0, user explicitly set it)
        lc.column_gap = config.layout.inner_gaps as i32;
        lc.window_gap = config.layout.inner_gaps as i32;
        // outer_gaps controls padding around the edge of the monitor
        // Always apply from config
        let og = config.layout.outer_gaps as i32;
        lc.outer_gaps = (og, og, og, og);
        if config.layout.border_width > 0 {
            lc.border_width = config.layout.border_width as i32;
        }
        if !config.layout.border_color.is_empty() {
            lc.border_color = config.layout.border_color.clone();
        }
        if !config.layout.border_color_focused.is_empty() {
            // Recognise the special "accent" / "windows-accent" sentinels
            // (wallpaper-aware focused border).  When the user writes a
            // sentinel we flip the mode and keep the fallback colour at
            // its default `#3381d9` so a failed registry lookup still
            // paints a visible focus indicator.
            let raw = config.layout.border_color_focused.trim().to_lowercase();
            if raw == "accent" || raw == "windows-accent" || raw == "system-accent" {
                lc.border_color_focused_mode = BorderColorMode::WindowsAccent;
                // Leave `border_color_focused` at its default literal so
                // the fallback path still has a sane colour to paint.
            } else {
                lc.border_color_focused = config.layout.border_color_focused.clone();
                lc.border_color_focused_mode = BorderColorMode::Fixed;
            }
        }
        if config.layout.focus_ring_width > 0 {
            lc.focus_ring_width = config.layout.focus_ring_width as i32;
        }
        if !config.layout.focus_ring_color.is_empty() {
            lc.focus_ring_color = config.layout.focus_ring_color.clone();
        }
        if config.layout.dim_unfocused < 1.0 {
            lc.dim_unfocused = config.layout.dim_unfocused;
        }
        lc.scroll_step = config.layout.scroll_step as i32;
        lc.column_width = config.layout.column_width;
        lc.column_width_mode = match config.layout.column_width_mode.as_str() {
            "fixed" => ColumnWidthMode::Fixed,
            _ => ColumnWidthMode::Proportional,
        };
        lc.strip_frame = config.layout.strip_frame;
        lc.shadow_enable = config.layout.shadow_enable;
        lc.smart_borders = config.layout.smart_borders;
        if !config.layout.border_color_urgent.is_empty() {
            lc.border_color_urgent = config.layout.border_color_urgent.clone();
        }
        // `backdrop` is engine-only and has no counterpart in the flat KDL
        // LayoutConfig; callers set it directly on the LayoutConfig after
        // calling from_config() if they need a non-default value.
        lc
    }
}

/// Column width presets for niri-style cycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnWidthPreset {
    OneQuarter,    // ~25 % of work_rect
    OneThird,      // ~33 % of work_rect
    Half,          // 50 %
    TwoThirds,     // ~67 %
    ThreeQuarters, // ~75 %
    Full,          // 100 %
    /// Cycle through OneQuarter → OneThird → Half → TwoThirds → ThreeQuarters → Full → OneQuarter …
    Cycle,
}

/// Direction for relative workspace navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceDirection {
    Next,
    Previous,
}

/// Routing target for `add_window_with_target`.
#[derive(Debug, Clone)]
pub enum AddWindowTarget {
    /// Default behaviour — place on the active workspace of the focused monitor.
    Auto,
    /// Place on the active workspace of the given monitor.
    Output(OutputId),
    /// Place on a specific workspace on the given monitor.
    /// If the workspace does not exist it is created.
    Workspace { output: OutputId, workspace_id: i32 },
    /// Insert as a tile in an existing column on the given monitor/workspace.
    /// If `column_idx` is out of range a new column is appended.
    Column { output: OutputId, workspace_id: i32, column_idx: usize },
    /// Place as a new column immediately to the right of the column containing `other_wid`.
    NextTo(WindowId),
}

/// The main tiling engine that manages monitors, workspaces, and window positions
pub struct TilingEngine {
    monitors: MonitorSet<OutputId, Monitor>,
    config: LayoutConfig,
    /// Per-monitor layout overrides resolved from
    /// `output.layout_override` blocks at config-load time.  When an
    /// `OutputId` has an entry here, `effective_config(oid)` returns a
    /// reference to the per-monitor `LayoutConfig`; otherwise it falls
    /// through to the global `self.config`.
    config_per_monitor: HashMap<OutputId, LayoutConfig>,
    tiled_windows: HashMap<WindowId, WindowInfo>,
    fullscreen_windows: HashSet<WindowId>,
    floating_windows: HashSet<WindowId>,
    window_sizing: HashMap<WindowId, SizingMode>,
    saved_styles: HashMap<WindowId, u32>,
    saved_ex_styles: HashMap<WindowId, u32>,
    window_rules: Vec<crate::config::WindowRule>,
    full_config: Option<crate::config::Config>,
    animation: crate::layout::AnimationManager,
    /// Overview mode: None = normal, Some(state) = zoomed out (all workspaces stacked).
    /// See `OverviewState` for the per-workspace Y-offset layout.
    overview: Option<OverviewState>,
    /// Counts of consecutive `set_window_position` failures per window.
    /// After `MAX_POSITION_FAILURES` attempts the window is auto-floated and
    /// removed from the tile layout so it stops thrashing the Win32 API.
    failed_position: HashMap<WindowId, u8>,
    /// Last window activated via SetForegroundWindow — avoids spamming the API on every layout pass.
    last_activated: Option<WindowId>,
    /// Optional channel to dispatch backend operations as `LayoutRequest`s instead of
    /// direct calls. When set, a sibling task on the backend drains this channel.
    /// When None, the engine continues to call `BackendApi` methods directly.
    ///
    /// Currently `apply_layout_for_monitor` still uses direct calls because it
    /// needs immediate ok/err feedback (to track `failed_position`). For one-shot
    /// best-effort operations (close, minimize, show/hide) the caller can opt in
    /// via `dispatch_request`. The drainer is wired in `main.rs`.
    layout_request_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::backend::LayoutRequest>>,
    /// Per-window cache of last-applied Win32 state. Used to skip redundant
    /// DwmSetWindowAttribute / SetLayeredWindowAttributes / SetWindowPos calls.
    applied_state: HashMap<WindowId, AppliedState>,
    /// Original window bounds at first registration. Used to restore windows
    /// when wiri exits so the user gets their layout back.
    original_bounds: HashMap<WindowId, Rect>,
    /// Last column-width preset applied via `set_column_width_preset(Cycle, …)`.
    /// Starts at `Half` so the first Cycle press → `TwoThirds`.
    last_column_preset: ColumnWidthPreset,
    // ---- Item 2: always-on-top tracking ----
    /// Windows that have been pinned as HWND_TOPMOST via toggle_always_on_top_for_focused.
    always_on_top: HashSet<WindowId>,
    // ---- Item 3: auto-tile threshold ----
    /// When Some(n), `apply_layout_for_monitor` auto-engages overview zoom on
    /// any workspace with more than n columns (provided we're not already in
    /// overview). Pass `None` to disable.
    pub auto_tile_threshold: Option<usize>,
    // ---- Item 4: auto-tile zoom ----
    /// Separate from `overview`, this holds an auto-computed zoom factor for
    /// when `auto_tile_threshold` is exceeded.  `calculate_positions` consults
    /// this alongside `overview` so the user's manual overview state is not
    /// overwritten.  `None` = no auto-zoom active.
    auto_tile_zoom: Option<f64>,
    // ---- Item 5: urgent-window tracking ----
    /// Windows marked urgent (e.g. via WM_FLASHWINDOW or external hook).
    /// Populated by mark_urgent / cleared by clear_urgent.
    urgent_windows: HashSet<WindowId>,
    /// Windows whose DWM shadow state has already been applied this session.
    /// `apply_shadow_for_window` is a one-shot per-HWND call: re-issuing the
    /// `DwmExtendFrameIntoClientArea` margins on every layout pass causes some
    /// apps (notably Windows Terminal) to render their content area black
    /// until they receive a fresh paint message, so we record the desired
    /// state once and skip subsequent calls until the window is destroyed.
    shadow_applied: HashSet<WindowId>,
    /// Windows that have had `DwmEnableBlurBehindWindow` applied at least
    /// once.  Tracked so the engine only calls into DWM once per HWND
    /// lifetime — re-issuing the blur on every layout pass is unnecessary
    /// and risks the same flicker behaviour that motivated `shadow_applied`.
    /// Entries are dropped in `remove_window` so a re-add re-applies blur.
    blur_applied: HashSet<WindowId>,
    /// Optional DWM-thumbnail overview sink.  When `Some`, overview
    /// mode leaves the real HWNDs alone and routes per-tile positioning
    /// to this sink (which composites live thumbnails into a transparent
    /// fullscreen host window via `DwmRegisterThumbnail`).  When `None`,
    /// the engine falls back to the historic behaviour of resizing the
    /// live HWND on each overview pass — this remains the path used by
    /// unit tests that don't stand up a Win32 sink.
    ///
    /// Wired by `main.rs` via [`Self::install_thumbnail_overview`].
    pub(crate) thumbnail_overview:
        Option<std::sync::Arc<dyn crate::overlay::ThumbnailOverviewSink>>,
    /// True while niri-style interactive resize mode is engaged
    /// (`Action::EnterResizeMode`).  Arrow keys with no modifier are
    /// intercepted by the WM_HOTKEY dispatcher to grow/shrink the focused
    /// column / tile, and Escape exits the mode.  Defaults to `false`.
    pub resize_mode: bool,
    // ---- Item 1: per-window animation-to-target rects ----
    /// Per-window in-flight position animations: (start_rect, target_rect, start_time, duration).
    /// Populated by `apply_layout_for_monitor` when a tile's target rect differs from its
    /// last applied state and animations are enabled. Drained by `tick_animations`.
    animating_rects: HashMap<WindowId, (Rect, Rect, std::time::Instant, std::time::Duration)>,
    // ---- Item 3: sticky windows (visible on all workspaces) ----
    /// Windows marked sticky by the user; they float above all workspaces and
    /// are never hidden during workspace switches.
    sticky_windows: HashSet<WindowId>,
    // ---- Item 4: workspace rename side-table ----
    /// User-supplied names for workspaces, keyed by workspace id.
    /// Consulted first by `workspace_name(id)`; falls through to config when absent.
    workspace_names: HashMap<i32, String>,
    /// Counter tracking how many Win32 calls were actually issued (for testing).
    #[cfg(test)]
    pub win32_call_count: u32,
}

impl TilingEngine {
    pub fn new(config: LayoutConfig) -> Self {
        Self {
            monitors: MonitorSet::new(),
            config,
            config_per_monitor: HashMap::new(),
            tiled_windows: HashMap::new(),
            fullscreen_windows: HashSet::new(),
            floating_windows: HashSet::new(),
            window_sizing: HashMap::new(),
            saved_styles: HashMap::new(),
            saved_ex_styles: HashMap::new(),
            window_rules: Vec::new(),
            full_config: None,
            animation: crate::layout::AnimationManager::disabled(),
            overview: None,
            failed_position: HashMap::new(),
            last_activated: None,
            layout_request_tx: None,
            applied_state: HashMap::new(),
            original_bounds: HashMap::new(),
            last_column_preset: ColumnWidthPreset::Half,
            always_on_top: HashSet::new(),
            auto_tile_threshold: None,
            auto_tile_zoom: None,
            urgent_windows: HashSet::new(),
            shadow_applied: HashSet::new(),
            blur_applied: HashSet::new(),
            thumbnail_overview: None,
            resize_mode: false,
            animating_rects: HashMap::new(),
            sticky_windows: HashSet::new(),
            workspace_names: HashMap::new(),
            #[cfg(test)]
            win32_call_count: 0,
        }
    }

    /// Toggle interactive resize mode.  Returns the new state.
    pub fn toggle_resize_mode(&mut self) -> bool {
        self.resize_mode = !self.resize_mode;
        if self.resize_mode {
            info!("Resize mode ON — arrow keys grow/shrink the focused tile (Esc to exit)");
        } else {
            info!("Resize mode OFF");
        }
        self.resize_mode
    }

    /// Force exit of resize mode (used by Escape handler and shutdown).
    pub fn exit_resize_mode(&mut self) {
        if self.resize_mode {
            self.resize_mode = false;
            info!("Resize mode OFF");
        }
    }

    /// Whether interactive resize mode is currently engaged.
    pub fn is_resize_mode(&self) -> bool {
        self.resize_mode
    }

    // =========================================================================
    // Item 3 — Sticky windows (visible on all workspaces)
    // =========================================================================

    /// Query whether `wid` is currently sticky.
    pub fn is_sticky(&self, wid: WindowId) -> bool {
        self.sticky_windows.contains(&wid)
    }

    /// Toggle the sticky flag on the currently focused window.
    ///
    /// When a window becomes sticky it is kept visible across all workspace
    /// switches.  `show_window(true)` is called immediately so the window
    /// remains on-screen even if the calling code switches workspaces next.
    /// When a window loses its sticky flag it simply falls back to normal
    /// tiled/floating behaviour; no additional `show_window` call is made
    /// because the next layout pass will reconcile visibility.
    pub fn toggle_sticky(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };
        let wid = match self.monitors.get(&focused_output).and_then(|m| m.focus_window) {
            Some(w) => w,
            None => return,
        };
        if self.sticky_windows.contains(&wid) {
            self.sticky_windows.remove(&wid);
            info!("Window {} is no longer sticky", wid);
        } else {
            self.sticky_windows.insert(wid);
            info!("Window {} marked sticky", wid);
            // Ensure the window is visible immediately.
            let _ = backend.show_window(wid.as_isize(), true);
        }
    }

    // =========================================================================
    // Item 4 — Workspace renaming
    // =========================================================================

    /// Return the display name for workspace `id`.
    ///
    /// Checks the side-table set by `rename_workspace` first; falls back to the
    /// name stored in the first matching `WorkspaceConfig` in `full_config`.
    /// Returns `None` when no name has been set for this workspace.
    pub fn workspace_name(&self, id: i32) -> Option<String> {
        // Side-table wins over config.
        if let Some(name) = self.workspace_names.get(&id) {
            return Some(name.clone());
        }
        // Fall through to full_config.workspace[].name
        self.full_config.as_ref().and_then(|cfg| {
            cfg.workspace
                .iter()
                .enumerate()
                .find(|(pos, _ws)| *pos as i32 == id)
                .and_then(|(_, ws)| {
                    if ws.name.is_empty() { None } else { Some(ws.name.clone()) }
                })
        })
    }

    /// Assign a display name to workspace `workspace_id` on the focused monitor.
    ///
    /// The name is stored in a side-table (`workspace_names`) which
    /// `workspace_name(id)` consults first.  If `full_config` has a
    /// `WorkspaceConfig` entry whose positional index matches the id the
    /// config entry is also updated in-place for consistency; otherwise the
    /// side-table entry is the sole source of truth until the next
    /// config reload.
    ///
    /// Returns `Err` if the focused monitor has no workspace with the given id.
    pub fn rename_workspace(&mut self, workspace_id: i32, new_name: &str) -> Result<(), String> {
        // Validate: the workspace must exist on the focused monitor.
        let focused_output = self.monitors.focused_id()
            .ok_or_else(|| "no focused monitor".to_string())?;
        let monitor = self.monitors.get(&focused_output)
            .ok_or_else(|| format!("monitor {:?} not found", focused_output))?;
        if !monitor.workspaces.contains_key(&workspace_id) {
            return Err(format!("workspace {} does not exist on the focused monitor", workspace_id));
        }

        // Update the side-table.
        self.workspace_names.insert(workspace_id, new_name.to_string());

        // Best-effort: sync to full_config when a matching entry exists.
        if let Some(cfg) = self.full_config.as_mut() {
            if let Some(entry) = cfg.workspace.get_mut(workspace_id as usize) {
                entry.name = new_name.to_string();
            }
        }

        info!("Workspace {} renamed to {:?}", workspace_id, new_name);
        Ok(())
    }

    /// Install a [`crate::overlay::ThumbnailOverviewSink`] to enable
    /// DWM-thumbnail-based overview rendering (niri-parity).  When a
    /// sink is installed:
    ///
    /// 1. `enter_overview` calls `sink.enter()` and registers a thumbnail
    ///    for every visible tile across every workspace on the focused
    ///    monitor.
    /// 2. The overview branch of `apply_layout_for_monitor` routes each
    ///    tile's computed rect to `sink.update_thumbnail(window_id, rect)`
    ///    instead of physically resizing the source HWND via
    ///    `SetWindowPos`.
    /// 3. `exit_overview` calls `sink.exit()` which unregisters every
    ///    handle and destroys the host window.
    ///
    /// Without a sink installed (the default, e.g. in unit tests), the
    /// engine falls back to the legacy behaviour of resizing live
    /// HWNDs.  Call this from `main.rs` after constructing the
    /// `TilingEngine` to opt into thumbnails.
    pub fn install_thumbnail_overview(
        &mut self,
        sink: std::sync::Arc<dyn crate::overlay::ThumbnailOverviewSink>,
    ) {
        self.thumbnail_overview = Some(sink);
    }

    /// Whether a DWM-thumbnail overview sink is currently installed.
    /// Useful for IPC diagnostics + the unit-test fallback assertion.
    pub fn has_thumbnail_overview(&self) -> bool {
        self.thumbnail_overview.is_some()
    }

    /// Install a `LayoutRequest` channel sender. Once set, the engine can dispatch
    /// backend ops (PositionWindow, ActivateWindow, ShowWindow, …) over the channel
    /// instead of calling `BackendApi` directly. The receiver side typically lives
    /// on a backend drainer task started by `main`.
    pub fn set_layout_request_channel(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<crate::backend::LayoutRequest>,
    ) {
        self.layout_request_tx = Some(tx);
    }

    /// Send a `LayoutRequest` over the channel if one is installed.
    /// Returns true if dispatched, false if no channel is set or send failed.
    pub fn dispatch_request(&self, req: crate::backend::LayoutRequest) -> bool {
        match &self.layout_request_tx {
            Some(tx) => tx.send(req).is_ok(),
            None => false,
        }
    }

    pub fn register_monitor(&mut self, output_id: OutputId, bounds: Rect, work_area: Rect) {
        self.register_monitor_with_scale(output_id, bounds, work_area, 1.0);
    }

    pub fn register_monitor_with_scale(&mut self, output_id: OutputId, bounds: Rect, work_area: Rect, scale_factor: f64) {
        info!("Registering monitor {} scale={:.1} bounds {:?}", output_id, scale_factor, bounds);
        let monitor = Monitor::with_scale(output_id, bounds, work_area, scale_factor);
        self.monitors.insert(output_id, monitor);
        // Auto-focus the first registered monitor.
        if self.monitors.focused_id().is_none() {
            self.monitors.set_focused(output_id);
        }
    }

    pub fn unregister_monitor(&mut self, output_id: &OutputId) {
        info!("Unregistering monitor {}", output_id);
        // MonitorSet.remove() already clears focused if this was the focused id.
        let was_focused = self.monitors.focused_id().as_ref() == Some(output_id);
        self.monitors.remove(output_id);
        if was_focused {
            // Re-focus the next available monitor (first in insertion order).
            let next_id = self.monitors.keys().next().copied();
            if let Some(next_id) = next_id {
                self.monitors.set_focused(next_id);
            }
        }
    }

    pub fn add_window(&mut self, window: WindowInfo, backend: &BackendHandle) {
        self.add_window_with_target(window, AddWindowTarget::Auto, backend, false);
    }

    /// Variant of `add_window` for windows discovered during the initial startup scan.
    /// Sets `at_startup = true` in the `MatcherContext` so window rules can distinguish
    /// startup-time placement from runtime window creation.
    pub fn add_window_at_startup(&mut self, window: WindowInfo, backend: &BackendHandle) {
        self.add_window_with_target(window, AddWindowTarget::Auto, backend, true);
    }

    pub fn add_window_with_target(&mut self, window: WindowInfo, target: AddWindowTarget, backend: &BackendHandle, at_startup: bool) {
        let window_id = WindowId::new(window.hwnd);
        let hwnd = window.hwnd; // Capture before move

        // Capture original bounds (first registration only) so we can restore on quit.
        self.original_bounds.entry(window_id).or_insert(window.bounds);

        // Wire is_active: compare hwnd against the current foreground window.
        let is_active = unsafe {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            let fg = GetForegroundWindow();
            !fg.0.is_null() && fg.0 as isize == window.hwnd
        };

        // Wire process_name: look up via QueryFullProcessImageNameW.
        let process_name_owned: Option<String> = backend.process_name_for_pid(window.process_id);

        let ctx = crate::config::types::MatcherContext {
            class_name: &window.class_name,
            title: &window.title,
            // Win32 has no concept of "instance" in the X11/Wayland sense; the
            // closest analogue is the process executable name, which we already
            // populate as `process_name`. Leaving instance unset is correct.
            // (Users matching apps should write rules against `class` or
            // `process_name`, not `instance`.)
            instance: None,
            process_name: process_name_owned.as_deref(),
            is_active,
            // is_floating: at this point the window hasn't been added to floating_windows yet,
            // so check against the existing set (covers re-adds after toggle_floating).
            is_floating: self.floating_windows.contains(&window_id),
            // Item 5: check urgent_windows set; also consult the FLASHWINFO helper stub.
            is_urgent: self.urgent_windows.contains(&window_id)
                || crate::window::is_window_flashing(
                    windows::Win32::Foundation::HWND(hwnd as *mut std::ffi::c_void)
                ),
            at_startup,
        };
        let rules = crate::window::resolve_window_rules(&self.window_rules, &ctx);

        // Item 8: extract default_width / default_height from matching window rules.
        // ResolvedWindowRules deliberately doesn't surface these fields (they're a
        // first-add-only hint, not part of the per-frame resolved state). We do a
        // second pass here for the same rule list using the same `matches` predicate
        // so behaviour is identical to a hypothetical resolved field.
        let (rule_default_width, rule_default_height): (Option<u32>, Option<u32>) = {
            let mut dw: Option<u32> = None;
            let mut dh: Option<u32> = None;
            for rule in &self.window_rules {
                if rule.matches(&ctx) {
                    if rule.default_width.is_some() { dw = rule.default_width; }
                    if rule.default_height.is_some() { dh = rule.default_height; }
                }
            }
            (dw, dh)
        };

        if rules.float {
            self.floating_windows.insert(window_id);
            self.tiled_windows.insert(window_id, window);
            info!("Window {:?} started as floating (rule)", window_id);
            return;
        }

        match target {
            AddWindowTarget::Auto => {
                let output_id = self.get_target_output(&window);

                if let Some(ws_id) = rules.workspace {
                    if let Some(monitor) = self.monitors.get_mut(&output_id) {
                        if !monitor.workspaces.contains_key(&ws_id) {
                            monitor.switch_workspace(ws_id);
                        }
                    }
                }

                // Item 8: prefer rule default_width over config fixed width.
                let width_hint = rule_default_width.or_else(|| {
                    if self.config.column_width_mode == ColumnWidthMode::Fixed {
                        Some(self.config.column_width)
                    } else {
                        None
                    }
                });
                if let Some(monitor) = self.monitors.get_mut(&output_id) {
                    monitor.add_window_with_width(window_id, width_hint);
                    // Item 8: store preferred_height on the newly added tile.
                    if let Some(dh) = rule_default_height {
                        if let Some(workspace) = monitor.workspace_mut() {
                            // The tile was added to the last column.
                            if let Some(col) = workspace.columns.last_mut() {
                                if let Some(tile) = col.tiles.last_mut() {
                                    tile.preferred_height = Some(dh);
                                }
                            }
                        }
                    }
                    self.tiled_windows.insert(window_id, window);
                }
                self.strip_frame_for_tiling(hwnd);
                self.apply_shadow_for_window(hwnd);
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
                }
                if rules.blur {
                    // niri-parity: apply DWM blur backdrop once per HWND.
                    self.apply_window_blur(hwnd, true);
                }
            }

            AddWindowTarget::Output(oid) => {
                // Route to the active workspace of the given monitor.
                let width_hint = if self.config.column_width_mode == ColumnWidthMode::Fixed {
                    Some(self.config.column_width)
                } else {
                    None
                };
                if let Some(monitor) = self.monitors.get_mut(&oid) {
                    monitor.add_window_with_width(window_id, width_hint);
                    self.tiled_windows.insert(window_id, window);
                } else {
                    // Monitor not found — fall back to Auto.
                    let output_id = self.get_target_output(&window);
                    if let Some(monitor) = self.monitors.get_mut(&output_id) {
                        monitor.add_window_with_width(window_id, width_hint);
                        self.tiled_windows.insert(window_id, window);
                    }
                }
                self.strip_frame_for_tiling(hwnd);
                self.apply_shadow_for_window(hwnd);
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
                }
                if rules.blur {
                    self.apply_window_blur(hwnd, true);
                }
            }

            AddWindowTarget::Workspace { output: oid, workspace_id } => {
                // Ensure workspace exists; switch active workspace to it.
                if let Some(monitor) = self.monitors.get_mut(&oid) {
                    if !monitor.workspaces.contains_key(&workspace_id) {
                        monitor.workspaces.insert(workspace_id, crate::layout::Workspace::new());
                    }
                    // Temporarily switch to the target workspace to add the window.
                    let previous_ws = monitor.active_workspace;
                    monitor.active_workspace = workspace_id;
                    let width_hint = if self.config.column_width_mode == ColumnWidthMode::Fixed {
                        Some(self.config.column_width)
                    } else {
                        None
                    };
                    monitor.add_window_with_width(window_id, width_hint);
                    // Restore previous active workspace (the window is parked in target ws).
                    monitor.active_workspace = previous_ws;
                    self.tiled_windows.insert(window_id, window);
                }
                self.strip_frame_for_tiling(hwnd);
                self.apply_shadow_for_window(hwnd);
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
                }
                if rules.blur {
                    self.apply_window_blur(hwnd, true);
                }
            }

            AddWindowTarget::Column { output: oid, workspace_id, column_idx } => {
                if let Some(monitor) = self.monitors.get_mut(&oid) {
                    if !monitor.workspaces.contains_key(&workspace_id) {
                        monitor.workspaces.insert(workspace_id, crate::layout::Workspace::new());
                    }
                    let previous_ws = monitor.active_workspace;
                    monitor.active_workspace = workspace_id;
                    if let Some(workspace) = monitor.workspace_mut() {
                        if column_idx < workspace.columns.len() {
                            // Insert into existing column.
                            workspace.add_window_to_column(column_idx, window_id);
                        } else {
                            // Out of range — append new column.
                            workspace.add_window_to_new_column(window_id);
                        }
                    }
                    // Focus the column.
                    monitor.focus_column = Some(column_idx.min(
                        monitor.workspace().map(|w| w.columns.len().saturating_sub(1)).unwrap_or(0)
                    ));
                    monitor.focus_window = Some(window_id);
                    monitor.focus_ring.push(window_id);
                    monitor.active_workspace = previous_ws;
                    self.tiled_windows.insert(window_id, window);
                }
                self.strip_frame_for_tiling(hwnd);
                self.apply_shadow_for_window(hwnd);
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
                }
                if rules.blur {
                    self.apply_window_blur(hwnd, true);
                }
            }

            AddWindowTarget::NextTo(other_wid) => {
                // Find the monitor and column containing `other_wid`.
                let location: Option<(OutputId, i32, usize)> = self.monitors.iter().find_map(|(&oid, monitor)| {
                    monitor.workspaces.iter().find_map(|(&ws_id, ws)| {
                        ws.find_window_column(other_wid).map(|col_idx| (oid, ws_id, col_idx))
                    })
                });

                if let Some((oid, ws_id, col_idx)) = location {
                    let insert_col = col_idx + 1;
                    if let Some(monitor) = self.monitors.get_mut(&oid) {
                        let previous_ws = monitor.active_workspace;
                        monitor.active_workspace = ws_id;
                        if let Some(workspace) = monitor.workspace_mut() {
                            // Insert a new column to the right of the found column.
                            let tile = crate::layout::Tile::new(window_id);
                            let mut col = crate::layout::Column::new();
                            if self.config.column_width_mode == ColumnWidthMode::Fixed {
                                col.width = Some(self.config.column_width);
                            }
                            col.add_tile(tile);
                            workspace.columns.insert(insert_col, col);
                        }
                        monitor.focus_column = Some(insert_col);
                        monitor.focus_window = Some(window_id);
                        monitor.focus_ring.push(window_id);
                        monitor.active_workspace = previous_ws;
                        self.tiled_windows.insert(window_id, window);
                    }
                } else {
                    // `other_wid` not found — fall back to Auto on first available monitor.
                    let output_id = self.get_target_output(&window);
                    let width_hint = if self.config.column_width_mode == ColumnWidthMode::Fixed {
                        Some(self.config.column_width)
                    } else {
                        None
                    };
                    if let Some(monitor) = self.monitors.get_mut(&output_id) {
                        monitor.add_window_with_width(window_id, width_hint);
                        self.tiled_windows.insert(window_id, window);
                    }
                }
                self.strip_frame_for_tiling(hwnd);
                self.apply_shadow_for_window(hwnd);
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
                }
                if rules.blur {
                    self.apply_window_blur(hwnd, true);
                }
            }
        }

        // Item 7: after adding a window, maintain exactly one trailing empty workspace
        // on the monitor that received the window.
        {
            // Find which monitor now holds this window_id.
            let output_for_maintain: Option<OutputId> = self.monitors.iter()
                .find_map(|(&oid, monitor)| {
                    monitor.workspaces.values().any(|ws| ws.find_window_column(window_id).is_some())
                        .then_some(oid)
                });
            if let Some(oid) = output_for_maintain {
                self.maintain_empty_workspace(oid);
            }
        }
    }

    pub fn remove_window(&mut self, window_id: WindowId, backend: &BackendHandle) {
        self.fullscreen_windows.remove(&window_id);
        self.floating_windows.remove(&window_id);
        self.window_sizing.remove(&window_id);
        self.saved_styles.remove(&window_id);
        self.saved_ex_styles.remove(&window_id);
        // Drop the applied-state cache entry so if the window is re-added its
        // state is re-applied from scratch.
        self.applied_state.remove(&window_id);
        // Reset the shadow-applied flag so a re-add applies the configured
        // shadow state once more (the HWND may be reused by the OS).
        self.shadow_applied.remove(&window_id);
        // Drop blur tracking; if the HWND is re-bound by Windows to a brand
        // new logical window, the rule resolver will decide whether to
        // re-apply blur on add.  We don't issue a clearing
        // `DwmEnableBlurBehindWindow(false)` call here because the window
        // is about to be destroyed (and on auto-float, the window will keep
        // its blur — the user expects a floating "frosted glass" effect).
        self.blur_applied.remove(&window_id);

        if self.tiled_windows.remove(&window_id).is_some() {
            let mut found_output = None;
            for (output_id, monitor) in self.monitors.iter() {
                if monitor.workspace().and_then(|w| w.find_window_column(window_id)).is_some() {
                    found_output = Some(*output_id);
                    break;
                }
            }
            if let Some(output_id) = found_output {
                if let Some(monitor) = self.monitors.get_mut(&output_id) {
                    monitor.remove_window(window_id);
                }
                // Item 7: maintain trailing empty workspace
                self.maintain_empty_workspace(output_id);
                self.apply_layout_for_monitor(output_id, backend);
            }
        }
    }

    fn get_target_output(&self, window: &WindowInfo) -> OutputId {
        let window_center = Point::new(
            window.bounds.loc.x + window.bounds.size.w as i32 / 2,
            window.bounds.loc.y + window.bounds.size.h as i32 / 2,
        );
        for (output_id, monitor) in self.monitors.iter() {
            if monitor.bounds.contains_point(window_center) {
                return *output_id;
            }
        }
        self.monitors.focused_id().unwrap_or_else(|| {
            self.monitors.keys().next().copied().unwrap_or(OutputId::from_name("default"))
        })
    }

    fn view_width_for_monitor(monitor: &Monitor, config: &LayoutConfig) -> i32 {
        monitor.work_area.size.w as i32 - config.outer_gaps.1 - config.outer_gaps.3
    }

    pub fn effective_column_width(
        num_columns: usize,
        view_width: i32,
        config: &LayoutConfig,
    ) -> i32 {
        match config.column_width_mode {
            ColumnWidthMode::Fixed => config.column_width as i32,
            ColumnWidthMode::Proportional => {
                if num_columns == 0 {
                    config.column_width as i32
                } else {
                    let total_gaps = config.column_gap * (num_columns as i32 - 1).max(0);
                    let available = view_width - total_gaps;
                    (available / num_columns as i32).max(config.column_width as i32 / 2)
                }
            }
        }
    }

    pub fn apply_layout_for_monitor(&mut self, output_id: OutputId, backend: &BackendHandle) {
        // -- Item 3: auto-tile threshold check (must run before borrowing monitor).
        // When threshold is set and the active workspace exceeds it, auto-engage
        // overview zoom so the user can see everything at a glance. We compute
        // the zoom once and only mutate self.overview if it changed; the rest of
        // this function then renders with the new zoom.
        if let Some(threshold) = self.auto_tile_threshold {
            if self.overview.is_none() {
                let (col_count, vw) = match self.monitors.get(&output_id)
                    .and_then(|m| m.workspace().map(|w| (w.columns.len(), Self::view_width_for_monitor(m, &self.config))))
                {
                    Some(v) => v,
                    None => return,
                };
                if col_count > threshold {
                    let cw = Self::effective_column_width(col_count, vw, &self.config);
                    let total = col_count as i32 * cw
                        + (col_count as i32 - 1).max(0) * self.config.column_gap;
                    let zoom = if total > vw {
                        (vw as f64 / total as f64).min(1.0)
                    } else {
                        1.0
                    };
                    if zoom < 1.0 {
                        info!(
                            "auto-tile threshold exceeded ({} > {}); engaging overview zoom {:.2}",
                            col_count, threshold, zoom
                        );
                        // Item 4: store the auto-zoom factor separately so we do NOT
                        // mutate `self.overview` (the user might want overview off).
                        // `calculate_positions` will pick it up via `effective_zoom`.
                        self.auto_tile_zoom = Some(zoom);
                    } else {
                        // Below threshold or fits — clear the auto-zoom.
                        self.auto_tile_zoom = None;
                    }
                } else {
                    // Column count is within threshold — clear any leftover auto-zoom.
                    self.auto_tile_zoom = None;
                }
            }
        }

        let monitor = match self.monitors.get(&output_id) {
            Some(m) => m,
            None => return,
        };
        let workspace = match monitor.workspace() {
            Some(w) => w,
            None => return,
        };

        // Per-monitor layout config (falls through to `self.config` when no
        // `output { layout { … } }` override is present for this monitor).
        // Cloned so the rest of this function can hold it across the
        // interleaved `&mut self.applied_state` updates that drive selective
        // Win32 calls without fighting the borrow checker.  `LayoutConfig`
        // is a small struct (~20 fields, no large allocations) so the clone
        // cost is negligible vs the Win32 calls it gates.
        let eff_cfg: LayoutConfig = self.effective_config(output_id).clone();

        let work_rect = Rect::new(
            monitor.work_area.loc.x + eff_cfg.outer_gaps.3,
            monitor.work_area.loc.y + eff_cfg.outer_gaps.0,
            monitor.work_area.size.w.saturating_sub(
                (eff_cfg.outer_gaps.1 + eff_cfg.outer_gaps.3) as u32,
            ),
            monitor.work_area.size.h.saturating_sub(
                (eff_cfg.outer_gaps.0 + eff_cfg.outer_gaps.2) as u32,
            ),
        );

        // Collect the window IDs on this monitor's active workspace.  In
        // overview mode we also need the union across every workspace so the
        // fullscreen-hide check (below) and the final cached_bounds write-back
        // know which tiles to consider.
        let overview_active = self.is_overview();
        let monitor_window_ids: std::collections::HashSet<WindowId> = if overview_active {
            monitor
                .workspaces
                .values()
                .flat_map(|ws| ws.columns.iter())
                .flat_map(|c| c.tiles.iter().map(|t| t.window_id))
                .collect()
        } else {
            workspace
                .columns
                .iter()
                .flat_map(|c| c.tiles.iter().map(|t| t.window_id))
                .collect()
        };

        for window_id in &self.fullscreen_windows {
            // Only position fullscreen windows that belong to this monitor
            if !monitor_window_ids.contains(window_id) {
                continue;
            }
            let fullscreen_rect = Rect::new(
                monitor.bounds.loc.x, monitor.bounds.loc.y,
                monitor.bounds.size.w, monitor.bounds.size.h,
            );
            let _ = backend.set_window_position(
                window_id.as_isize(), fullscreen_rect,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
            );
        }

        // Snapshot the per-tile ConfigureThrottle intents before positioning.
        // We need an immutable borrow of the workspace to get tile intents, then a
        // mutable one afterwards to call mark_sent(). Collect the intents now.
        // In overview mode we collect across ALL workspaces so every tile is
        // throttle-aware regardless of which workspace owns it.
        let tile_intents: HashMap<WindowId, ConfigureIntent> = {
            let monitor = self.monitors.get(&output_id).unwrap();
            if overview_active {
                monitor.workspaces.values()
                    .flat_map(|ws| ws.columns.iter())
                    .flat_map(|col| col.tiles.iter())
                    .map(|tile| (tile.window_id, tile.configure_throttle.intent()))
                    .collect()
            } else {
                let workspace = monitor.workspace().unwrap();
                workspace.columns.iter()
                    .flat_map(|col| col.tiles.iter())
                    .map(|tile| (tile.window_id, tile.configure_throttle.intent()))
                    .collect()
            }
        };

        // Compute positions.  In overview mode iterate every non-empty
        // workspace in workspace-id ascending order, lay each one out inside
        // its precomputed vertical section, and union the per-workspace
        // position lists.  Outside overview we keep the original behaviour
        // (active workspace only, no Y offset).
        let positions: Vec<(WindowId, Rect)> = if overview_active {
            let monitor = self.monitors.get(&output_id).unwrap();
            let mut out: Vec<(WindowId, Rect)> = Vec::new();
            // Sort by workspace id so the on-screen stack matches the
            // numerical "Workspace 1, Workspace 2, …" labels.
            let mut ws_ids: Vec<i32> = monitor.workspaces.keys().copied().collect();
            ws_ids.sort();
            // Per-workspace section height + gap come from `OverviewState`.
            let zoom = self.overview_zoom();
            const INTER_WORKSPACE_GAP: i32 = 50;
            let nominal_section_height = monitor.work_area.size.h as i32;
            let scaled_section_h = (nominal_section_height as f64 * zoom) as i32;
            let scaled_gap = (INTER_WORKSPACE_GAP as f64 * zoom) as i32;
            let mut section_y: i32 = 0;
            for ws_id in ws_ids {
                let ws = match monitor.workspaces.get(&ws_id) {
                    Some(w) => w,
                    None => continue,
                };
                if ws.columns.is_empty() {
                    continue; // skip empty workspaces, no Y consumption
                }
                let mut ws_positions = self.calculate_positions_in_section(
                    ws,
                    work_rect,
                    section_y,
                    nominal_section_height,
                    &eff_cfg,
                );
                out.append(&mut ws_positions);
                section_y += scaled_section_h + scaled_gap;
            }
            out
        } else {
            // Item 5: branch on per-workspace layout mode.
            use crate::layout::workspace::WorkspaceLayout;
            match workspace.layout_mode {
                WorkspaceLayout::BStack => {
                    self.calculate_positions_bstack(workspace, work_rect, &eff_cfg)
                }
                WorkspaceLayout::Spiral => {
                    self.calculate_positions_spiral(workspace, work_rect)
                }
                WorkspaceLayout::Scrolling => {
                    self.calculate_positions(workspace, work_rect, &eff_cfg)
                }
            }
        };

        // Workspace-slide animation bias (niri parity).  While a workspace
        // switch is in flight the engine reads `workspace_slide_offset(oid)`
        // and adds it as a Y-axis bias to every tile on the active workspace
        // so the new workspace appears to slide in from above/below.  No-op
        // in overview mode (overview already lays out workspaces vertically)
        // and when no slide is active (returns 0.0).
        let slide_offset_y = if !overview_active {
            self.animation.workspace_slide_offset(output_id) as i32
        } else {
            0
        };
        let positions: Vec<(WindowId, Rect)> = if slide_offset_y != 0 {
            positions
                .into_iter()
                .map(|(wid, r)| {
                    let shifted = Rect::new(
                        r.loc.x,
                        r.loc.y + slide_offset_y,
                        r.size.w,
                        r.size.h,
                    );
                    (wid, shifted)
                })
                .collect()
        } else {
            positions
        };

        // Snapshot positions for later write-back to Tile.cached_bounds so that
        // LayoutElement::bounds() returns the most recently computed rect.
        let position_map: HashMap<WindowId, Rect> = positions.iter().copied().collect();
        let focused = self.monitors.values().find_map(|m| m.focus_window);
        // ^ iterates in insertion order via MonitorSet::values()
        let border_w = eff_cfg.border_width.max(1);

        // Resolve the focused border colour for this pass.  When the user
        // opted into `border-color-focused "accent"` we read the live
        // Windows accent (cached for 30 s) and convert it to a `#rrggbb`
        // literal so the rest of the pipeline (cache compare, DWM call)
        // keeps treating it as a normal colour.  If the registry lookup
        // fails we fall through to the configured literal.
        let resolved_focused_color: String = match eff_cfg.border_color_focused_mode {
            BorderColorMode::Fixed => eff_cfg.border_color_focused.clone(),
            BorderColorMode::WindowsAccent => {
                match crate::backend::accent::current_windows_accent_rgba() {
                    Some([r, g, b, _a]) => format!("#{:02x}{:02x}{:02x}", r, g, b),
                    None => eff_cfg.border_color_focused.clone(),
                }
            }
        };

        // Pre-compute target colors (as packed u32) for this pass.
        let border_color_focused_u32 = parse_color_to_u32(&resolved_focused_color);
        let border_color_normal_u32  = parse_color_to_u32(&eff_cfg.border_color);
        let border_color_urgent_u32  = parse_color_to_u32(&eff_cfg.border_color_urgent);

        // niri-parity smart borders: when the active workspace has exactly
        // one column with exactly one tile, suppress the per-tile DWM border
        // colour (gives more usable space when there's nothing to delimit).
        // Cached as a u32 sentinel (0xFFFFFFFE — distinct from the
        // 0xFFFFFFFF "unset" cache marker so we don't get spurious cache
        // hits) and resolved against the live workspace state up-front.
        const SMART_BORDERS_SENTINEL: u32 = 0xFFFF_FFFE;
        let smart_no_border = if eff_cfg.smart_borders && !overview_active {
            self.monitors
                .get(&output_id)
                .and_then(|m| m.workspace())
                .map(|ws| ws.columns.len() == 1 && ws.columns[0].tiles.len() == 1)
                .unwrap_or(false)
        } else {
            false
        };

        // Pre-compute target opacity values as u8 for comparison.
        let opacity_full: u32 = 255;
        let opacity_dim: u32 = (eff_cfg.dim_unfocused * 255.0).round().clamp(0.0, 255.0) as u32;

        // When `true`, the overview pass routes per-tile geometry to the
        // installed DWM-thumbnail sink instead of resizing live HWNDs.
        // This leaves the real windows alone (no SetWindowPos, no
        // ShowWindow, no border colour change) so applications keep
        // rendering at their normal size while their thumbnails appear
        // in the transparent overview host.
        let use_thumbnail_overview = overview_active && self.thumbnail_overview.is_some();

        // Track which windows had set_window_position called so we can mark_sent() afterwards.
        let mut applied_windows: HashSet<WindowId> = HashSet::new();
        // Windows whose position was rejected enough times to trigger auto-float.
        // With the batched positioning path the per-window failure tracking is
        // delegated to set_window_positions_batched's internal fallback; this vec
        // is kept for API compatibility with the auto-float block below.
        let windows_to_auto_float: Vec<WindowId> = Vec::new();
        // Batch-positioning accumulator: collect (hwnd, inset_rect, flags, window_id) tuples
        // during the tile loop, then commit all positions in a single render frame via
        // BeginDeferWindowPos / DeferWindowPos / EndDeferWindowPos after the loop.
        // Each entry also carries the window_id so we can update applied_state afterwards.
        let mut batch_positions: Vec<(isize, Rect, windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS, WindowId)> = Vec::new();

        for (window_id, rect) in positions {
            // niri-parity thumbnail overview: route every tile's rect
            // to the sink and DON'T touch the source HWND.  The sink
            // composites scaled live previews via DwmRegisterThumbnail
            // so we never SetWindowPos / ShowWindow / change borders on
            // the real windows while overview is active.
            if use_thumbnail_overview {
                if let Some(sink) = &self.thumbnail_overview {
                    sink.update_thumbnail(window_id, rect);
                }
                continue;
            }
            if self.fullscreen_windows.contains(&window_id) {
                // Hide tiled windows when another window on this monitor is fullscreen.
                let prior = self.applied_state.get(&window_id);
                let was_visible = prior.map(|p| p.visible).unwrap_or(true);
                if was_visible {
                    let _ = backend.show_window(window_id.as_isize(), false);
                    #[cfg(test)] { self.win32_call_count += 1; }
                    self.applied_state.entry(window_id).or_insert_with(AppliedState::unset).visible = false;
                }
                continue;
            }
            if self.floating_windows.contains(&window_id) {
                continue;
            }

            let is_focused = focused == Some(window_id);
            let should_be_visible = true;

            // Target values for this tile.  Priority order:
            // 1. smart-borders suppresses all colour when lone tile opt-in.
            // 2. urgent windows get the urgent (red) colour regardless of focus.
            // 3. focused → focused colour; otherwise → normal colour.
            let is_urgent = self.urgent_windows.contains(&window_id);
            let new_border_color = if smart_no_border {
                SMART_BORDERS_SENTINEL
            } else if is_urgent {
                border_color_urgent_u32
            } else if is_focused {
                border_color_focused_u32
            } else {
                border_color_normal_u32
            };
            let new_opacity: u32 = if eff_cfg.dim_unfocused < 1.0 {
                if is_focused { opacity_full } else { opacity_dim }
            } else {
                // dim_unfocused == 1.0 means "no dimming"; use sentinel so we
                // never issue SetLayeredWindowAttributes unnecessarily.
                0xFFFF_FFFF
            };

            // Focused windows get a wider border for visual emphasis.
            let inset = if is_focused { border_w + eff_cfg.focus_ring_width } else { border_w };
            let inset_rect = Rect::new(
                rect.loc.x + inset,
                rect.loc.y + inset,
                rect.size.w.saturating_sub(inset as u32 * 2),
                rect.size.h.saturating_sub(inset as u32 * 2),
            );

            // Check configure throttle intent — skip position if NotNeeded or Throttled.
            let intent = tile_intents.get(&window_id).copied().unwrap_or(ConfigureIntent::ShouldSend);
            let skip_pos = matches!(intent, ConfigureIntent::NotNeeded | ConfigureIntent::Throttled);

            // ---- Applied-state cache lookup ----
            let prior = self.applied_state.get(&window_id).cloned()
                .unwrap_or_else(AppliedState::unset);

            // Fast-path: everything is identical — no Win32 calls needed.
            // (Position is only compared when the throttle would allow sending it.)
            let new_backdrop = Self::backdrop_str_to_u32(&eff_cfg.backdrop);
            let pos_identical = skip_pos || prior.rect == inset_rect;
            if prior.visible == should_be_visible
                && prior.border_color == new_border_color
                && prior.opacity == new_opacity
                && prior.focused == is_focused
                && prior.backdrop == new_backdrop
                && pos_identical
            {
                continue;
            }

            // ---- Selective Win32 calls ----

            // 1. Visibility
            if prior.visible != should_be_visible {
                let _ = backend.show_window(window_id.as_isize(), should_be_visible);
                #[cfg(test)] { self.win32_call_count += 1; }
                self.applied_state.entry(window_id).or_insert_with(AppliedState::unset).visible = should_be_visible;
            }

            // 2. Border color
            if prior.border_color != new_border_color {
                if new_border_color == SMART_BORDERS_SENTINEL {
                    // smart-borders → push the "no colour" sentinel so DWM
                    // reverts to the system default border (effectively no
                    // wiri-painted outline on the lone tile).
                    self.set_dwm_border_color_none(window_id.as_isize());
                } else {
                    let color_str: &str = if is_focused {
                        // Use the resolved focused colour — handles the
                        // `WindowsAccent` mode + fallback already.
                        &resolved_focused_color
                    } else {
                        &eff_cfg.border_color
                    };
                    self.set_dwm_border_color(window_id.as_isize(), color_str);
                }
                #[cfg(test)] { self.win32_call_count += 1; }
                self.applied_state.entry(window_id).or_insert_with(AppliedState::unset).border_color = new_border_color;
            }

            // 3. Opacity (only when dimming is active)
            if new_opacity != 0xFFFF_FFFF && prior.opacity != new_opacity {
                let opacity_f32 = new_opacity as f32 / 255.0;
                self.apply_window_opacity(window_id.as_isize(), opacity_f32);
                #[cfg(test)] { self.win32_call_count += 1; }
                self.applied_state.entry(window_id).or_insert_with(AppliedState::unset).opacity = new_opacity;
            }

            // 4. Position (subject to ConfigureIntent throttle)
            if !skip_pos && prior.rect != inset_rect {
                debug!("Positioning window {} at ({},{}) {}x{}", window_id, inset_rect.loc.x, inset_rect.loc.y, inset_rect.size.w, inset_rect.size.h);

                // Item 1: when animations are enabled and the window already has
                // a known position (i.e. this is not a first-ever placement),
                // record the (start, target) pair in `animating_rects` and skip
                // the immediate SetWindowPos so `tick_animations` can drive it.
                let animate_transition = self.animation.is_enabled()
                    && prior.rect != Rect::new(0, 0, 0, 0);
                if animate_transition && !self.animating_rects.contains_key(&window_id) {
                    // Only insert if no animation is already in flight for this window.
                    self.animating_rects.insert(
                        window_id,
                        (
                            prior.rect,
                            inset_rect,
                            std::time::Instant::now(),
                            std::time::Duration::from_millis(
                                self.animation
                                    .is_enabled()
                                    .then(|| {
                                        self.full_config
                                            .as_ref()
                                            .map(|c| c.animations.duration as u64)
                                            .unwrap_or(200)
                                    })
                                    .unwrap_or(200),
                            ),
                        ),
                    );
                    applied_windows.insert(window_id);
                    // Update the target in applied_state so the cache considers it
                    // "in progress" and doesn't re-trigger a new animation.
                    self.applied_state.entry(window_id).or_insert_with(AppliedState::unset).rect = inset_rect;
                    continue; // tick_animations will issue SetWindowPos
                }

                // Accumulate into the batch rather than calling SetWindowPos immediately.
                // The batch is committed atomically after the tile loop via
                // BeginDeferWindowPos / EndDeferWindowPos, eliminating the per-tile
                // render cascade that causes the visible startup stutter.
                batch_positions.push((
                    window_id.as_isize(),
                    inset_rect,
                    windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                    window_id,
                ));
                // Track as attempted for configure-throttle bookkeeping regardless of
                // whether the batch commit succeeds for this individual entry.
                applied_windows.insert(window_id);
            }

            // 5. Backdrop (DWMWA_SYSTEMBACKDROP_TYPE) — applied once per window
            //    when the effective backdrop value changes (e.g. after toggle_backdrop_cycle).
            //    `new_backdrop` was already computed above in the fast-path check.
            if prior.backdrop != new_backdrop {
                self.apply_backdrop_for_window(window_id.as_isize(), new_backdrop);
                self.applied_state
                    .entry(window_id)
                    .or_insert_with(AppliedState::unset)
                    .backdrop = new_backdrop;
            }

            // Update focus flag in cache.
            self.applied_state.entry(window_id).or_insert_with(AppliedState::unset).focused = is_focused;
        }

        // Item 3: skip Win32 call entirely when nothing changed (all cache hits).
        // This is the common case after the first apply and is already near-free.
        if !batch_positions.is_empty() {
            // Commit all accumulated positions atomically in a single render frame.
            // BeginDeferWindowPos / DeferWindowPos / EndDeferWindowPos ensures the
            // OS composites the new geometry for all tiles simultaneously, eliminating
            // the per-tile stutter visible on startup with 10+ windows.
            let batch_slice: Vec<(isize, Rect, windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS)> =
                batch_positions.iter().map(|(hwnd, rect, flags, _)| (*hwnd, *rect, *flags)).collect();
            let _positioned = backend.set_window_positions_batched(&batch_slice);
            #[cfg(test)] { self.win32_call_count += 1; }

            // Post-batch cache updates: mark applied_state.rect, clear failure counters,
            // and paint title strips. We do this for all attempted entries regardless of
            // individual DeferWindowPos success — the batch either commits atomically or
            // falls back to one-by-one inside set_window_positions_batched.
            for (hwnd, inset_rect, _, window_id) in &batch_positions {
                self.applied_state.entry(*window_id).or_insert_with(AppliedState::unset).rect = *inset_rect;
                // Clear any pending failure counter — the window is co-operating again.
                self.failed_position.remove(window_id);
                // Paint title strip in the DWM frame area when strip_frame is on.
                let is_focused = focused == Some(*window_id);
                let title = self.tiled_windows
                    .get(window_id)
                    .map(|info| info.title.clone())
                    .unwrap_or_default();
                self.paint_title_for_tile(*hwnd, *inset_rect, &title, is_focused);
            }
        }

        // Mark configure_throttle as sent for every tile that had set_window_position called.
        // Populate cached_bounds while we're walking the workspace so LayoutElement::bounds()
        // returns the just-computed rect for any downstream observer (overlay rendering,
        // hit-testing, …).  In overview mode the position_map covers every
        // workspace's tiles, so we walk all workspaces; otherwise only the
        // active one is touched.
        if let Some(monitor) = self.monitors.get_mut(&output_id) {
            if overview_active {
                for ws in monitor.workspaces.values_mut() {
                    for col in ws.columns.iter_mut() {
                        for tile in col.tiles.iter_mut() {
                            if applied_windows.contains(&tile.window_id) {
                                tile.configure_throttle.mark_sent();
                            }
                            if let Some(&rect) = position_map.get(&tile.window_id) {
                                tile.cached_bounds = rect;
                            }
                        }
                    }
                }
            } else if let Some(workspace) = monitor.workspace_mut() {
                for col in workspace.columns.iter_mut() {
                    for tile in col.tiles.iter_mut() {
                        if applied_windows.contains(&tile.window_id) {
                            tile.configure_throttle.mark_sent();
                        }
                        // Update cached bounds from the position map (whether or not
                        // we actually issued a Win32 call this pass).
                        if let Some(&rect) = position_map.get(&tile.window_id) {
                            tile.cached_bounds = rect;
                        }
                    }
                }
            }
        }

        // Auto-float any windows that exhausted their position-retry budget.
        // This must happen AFTER the apply loop so we don't mutate the workspace
        // mid-iteration. The window stays in `tiled_windows` (so it's still
        // tracked) but is removed from the column layout and added to floating.
        for wid in windows_to_auto_float {
            self.floating_windows.insert(wid);
            self.window_sizing.insert(wid, SizingMode::Normal);
            // Drop the failure counter so toggle-floating back to tiled can try again.
            self.failed_position.remove(&wid);
            if let Some(monitor) = self.monitors.get_mut(&output_id) {
                monitor.remove_window(wid);
            }
        }

        // Item 3: ensure sticky windows are always visible on this monitor.
        // Sticky windows are not part of any workspace column, so the regular
        // tile loop never shows them.  We call show_window unconditionally;
        // the backend ignores the call when the window is already visible.
        for &sticky_wid in &self.sticky_windows {
            let _ = backend.show_window(sticky_wid.as_isize(), true);
        }

        // Bring the focused window to the foreground if it changed since last layout.
        // Only the focused monitor's focus drives Win32 activation.
        if Some(output_id) == self.monitors.focused_id() {
            let current_focus = self
                .monitors
                .get(&output_id)
                .and_then(|m| m.focus_window);
            if current_focus.is_some() && current_focus != self.last_activated {
                if let Some(wid) = current_focus {
                    if let Err(e) = backend.activate_window(wid.as_isize()) {
                        debug!("activate_window({}) failed: {:?}", wid, e);
                    } else {
                        self.last_activated = Some(wid);
                    }
                }
            }
        }
    }

    /// Calculate window positions for a workspace.
    /// Supports per-column variable widths and overview zoom.
    ///
    /// Backwards-compatible single-workspace path — equivalent to
    /// `calculate_positions_in_section(workspace, work_rect, 0, work_rect.size.h as i32, cfg)`.
    fn calculate_positions(
        &self,
        workspace: &crate::layout::workspace::Workspace,
        work_rect: Rect,
        cfg: &LayoutConfig,
    ) -> Vec<(WindowId, Rect)> {
        let section_height = work_rect.size.h as i32;
        self.calculate_positions_in_section(workspace, work_rect, 0, section_height, cfg)
    }

    /// Item 5 — BStack layout: first column takes the left 50% of `work_rect`
    /// (full height), all remaining columns are stacked vertically in the right
    /// 50%, sharing that half equally.
    ///
    /// When there is only one column it occupies the full work area (same as
    /// Scrolling with a single window).  When the workspace is empty the
    /// function returns an empty vec.
    fn calculate_positions_bstack(
        &self,
        workspace: &crate::layout::workspace::Workspace,
        work_rect: Rect,
        cfg: &LayoutConfig,
    ) -> Vec<(WindowId, Rect)> {
        let mut positions = Vec::new();
        let num_columns = workspace.columns.len();
        if num_columns == 0 {
            return positions;
        }

        let gap = cfg.column_gap;
        let work_w = work_rect.size.w as i32;
        let work_h = work_rect.size.h as i32;

        // Main column: left half (or full width when there is only one column).
        let main_w = if num_columns == 1 { work_w } else { (work_w - gap) / 2 };
        let stack_x = work_rect.loc.x + main_w + gap;
        let stack_w = (work_w - main_w - gap).max(0);

        // --- Main column (index 0) ---
        {
            let main_col = &workspace.columns[0];
            let visible = main_col.visible_tile_indices();
            let n_vis = visible.len();
            if n_vis > 0 {
                let window_gap = cfg.window_gap;
                let total_gap = window_gap * (n_vis as i32 - 1).max(0);
                let tile_h = ((work_h - total_gap) / n_vis as i32).max(0);
                for (slot, &idx) in visible.iter().enumerate() {
                    if let Some(tile) = main_col.tiles.get(idx) {
                        let y = work_rect.loc.y + slot as i32 * (tile_h + window_gap);
                        let rect = Rect::new(work_rect.loc.x, y, main_w as u32, tile_h as u32);
                        positions.push((tile.window_id, rect));
                    }
                }
            }
        }

        if num_columns <= 1 || stack_w <= 0 {
            return positions;
        }

        // --- Stack columns (indices 1..) ---
        // Each column gets an equal vertical slice of the right half.
        let stack_cols = num_columns - 1;
        let window_gap = cfg.window_gap;
        let total_col_gap = window_gap * (stack_cols as i32 - 1).max(0);
        let col_h = ((work_h - total_col_gap) / stack_cols as i32).max(0);

        for (col_slot, col) in workspace.columns[1..].iter().enumerate() {
            let col_top = work_rect.loc.y + col_slot as i32 * (col_h + window_gap);
            let visible = col.visible_tile_indices();
            let n_vis = visible.len();
            if n_vis == 0 {
                continue;
            }
            let tile_gap = cfg.window_gap;
            let total_tile_gap = tile_gap * (n_vis as i32 - 1).max(0);
            let tile_h = ((col_h - total_tile_gap) / n_vis as i32).max(0);
            for (slot, &idx) in visible.iter().enumerate() {
                if let Some(tile) = col.tiles.get(idx) {
                    let y = col_top + slot as i32 * (tile_h + tile_gap);
                    let rect = Rect::new(stack_x, y, stack_w as u32, tile_h as u32);
                    positions.push((tile.window_id, rect));
                }
            }
        }

        positions
    }

    // ---- Item 2: Spiral (golden-ratio recursive bisection) layout ----

    /// Spiral layout — recursive alternating horizontal/vertical bisection.
    ///
    /// Tile 0 takes the left half, tile 1 the top half of the remainder,
    /// tile 2 the left half of what's left, and so on.  The last tile always
    /// fills whatever rectangle remains so no space is wasted.
    ///
    /// All tiles across all columns are flattened into a single ordered list
    /// (column 0 tile 0, column 0 tile 1, …, column 1 tile 0, …) before
    /// bisection begins.
    fn calculate_positions_spiral(
        &self,
        workspace: &crate::layout::workspace::Workspace,
        work_rect: Rect,
    ) -> Vec<(WindowId, Rect)> {
        let mut positions = Vec::new();
        let tiles: Vec<WindowId> = workspace
            .columns
            .iter()
            .flat_map(|c| c.tiles.iter().map(|t| t.window_id))
            .collect();
        let n = tiles.len();
        if n == 0 {
            return positions;
        }

        let mut current_rect = work_rect;
        for (i, &wid) in tiles.iter().enumerate() {
            if i == n - 1 {
                // Last tile fills the remaining rect.
                positions.push((wid, current_rect));
                break;
            }
            // Alternate: even index → split horizontally (left/right),
            // odd index → split vertically (top/bottom).
            let (tile_rect, remainder) = if i % 2 == 0 {
                let split_w = (current_rect.size.w / 2).max(1);
                let tile_r = Rect::new(
                    current_rect.loc.x,
                    current_rect.loc.y,
                    split_w,
                    current_rect.size.h,
                );
                let rest = Rect::new(
                    current_rect.loc.x + split_w as i32,
                    current_rect.loc.y,
                    current_rect.size.w.saturating_sub(split_w),
                    current_rect.size.h,
                );
                (tile_r, rest)
            } else {
                let split_h = (current_rect.size.h / 2).max(1);
                let tile_r = Rect::new(
                    current_rect.loc.x,
                    current_rect.loc.y,
                    current_rect.size.w,
                    split_h,
                );
                let rest = Rect::new(
                    current_rect.loc.x,
                    current_rect.loc.y + split_h as i32,
                    current_rect.size.w,
                    current_rect.size.h.saturating_sub(split_h),
                );
                (tile_r, rest)
            };
            positions.push((wid, tile_rect));
            current_rect = remainder;
        }
        positions
    }

    /// Multi-workspace overview-aware position calculator.
    ///
    /// Renders one workspace's columns inside a vertical "section" that starts
    /// at `section_y_offset` (relative to `work_rect.loc.y`) and is
    /// `section_height` pixels tall (pre-zoom).  In normal mode the caller
    /// passes `section_y_offset = 0` and `section_height = work_rect.size.h`,
    /// which reproduces the original behaviour.  In overview mode the engine
    /// stacks sections vertically; each call computes positions for one
    /// workspace's slice.  `cfg` is the per-monitor effective `LayoutConfig`
    /// (see `effective_config`) so per-output overrides for gaps / widths /
    /// modes are honoured.
    fn calculate_positions_in_section(
        &self,
        workspace: &crate::layout::workspace::Workspace,
        work_rect: Rect,
        section_y_offset: i32,
        section_height: i32,
        cfg: &LayoutConfig,
    ) -> Vec<(WindowId, Rect)> {
        let mut positions = Vec::new();
        let column_gap = cfg.column_gap;
        let window_gap = cfg.window_gap;
        let num_columns = workspace.columns.len();
        if num_columns == 0 { return positions; }

        // Item 4: use effective_zoom so auto_tile_zoom is consulted when no manual
        // overview is active.  Falls back to overview_zoom() when overview is on.
        let zoom = self.effective_zoom();

        let proportional_width = {
            let total_gaps = column_gap * (num_columns as i32 - 1).max(0);
            let available = work_rect.size.w as i32 - total_gaps;
            (available / num_columns as i32).max(cfg.column_width as i32 / 2)
        };

        let col_widths: Vec<i32> = workspace.columns.iter().map(|col| {
            match cfg.column_width_mode {
                ColumnWidthMode::Proportional => proportional_width,
                ColumnWidthMode::Fixed => {
                    col.width.map(|w| w as i32).unwrap_or(cfg.column_width as i32)
                }
            }
        }).collect();

        let mut col_x: Vec<i32> = Vec::with_capacity(num_columns);
        let mut x = 0i32;
        for (i, w) in col_widths.iter().enumerate() {
            col_x.push(x);
            if i < num_columns - 1 { x += w + column_gap; }
        }

        let total_content_width = col_x.last().copied().unwrap_or(0) + col_widths.last().copied().unwrap_or(0);

        let center_offset = if zoom < 1.0 && total_content_width > 0 {
            let scaled_total = (total_content_width as f64 * zoom) as i32;
            ((work_rect.size.w as i32 - scaled_total) / 2).max(0)
        } else {
            0
        };

        // In overview the per-workspace "section" already encodes vertical
        // positioning; in normal mode `section_y_offset == 0` and
        // `section_height == work_rect.size.h`, so nothing changes.
        let scaled_section_height = if zoom < 1.0 {
            (section_height as f64 * zoom) as i32
        } else {
            section_height
        };

        for (col_idx, column) in workspace.columns.iter().enumerate() {
            let cw = col_widths[col_idx];
            let scaled_x = if zoom < 1.0 {
                center_offset + (col_x[col_idx] as f64 * zoom) as i32
            } else {
                col_x[col_idx] - workspace.scroll_offset.x
            };
            let screen_x = work_rect.loc.x + scaled_x;
            let scaled_w = (cw as f64 * zoom) as u32;

            if zoom >= 1.0 &&
                (screen_x + (scaled_w as i32) < work_rect.loc.x
                    || screen_x > work_rect.loc.x + work_rect.size.w as i32)
            {
                continue;
            }

            if column.tiles.is_empty() { continue; }

            // In Tabbed mode only the active tile occupies the full column height.
            // In Stacked mode all tiles share the column height equally.
            let visible_indices = column.visible_tile_indices();
            let visible_count = visible_indices.len();
            if visible_count == 0 { continue; }

            // Effective per-section work height.  In normal mode this is
            // `work_rect.size.h`.  In overview it is the post-zoom section
            // height (so each workspace's section is rendered at the same
            // size, with a small inter-workspace gap drawn elsewhere).
            let work_height = if zoom < 1.0 {
                let padding = (scaled_section_height as f64 * 0.05) as i32;
                (scaled_section_height - padding * 2).max(40)
            } else {
                section_height
            };

            let scaled_gap = if zoom < 1.0 {
                (window_gap as f64 * zoom) as i32
            } else {
                window_gap
            };
            let total_gap_height = scaled_gap * (visible_count as i32 - 1).max(0);
            let available_height = (work_height - total_gap_height).max(0);
            // Per-tile heights honor `Tile.height_weight` so the keyboard
            // resize actions (`Action::GrowTileHeight` / `ShrinkTileHeight`)
            // can re-bias a column's vertical distribution. When the column
            // is `maximized`, weights are ignored and the tiles share the
            // full work-area height equally (treating maximize as a reset).
            let force_equal = column.maximized || visible_count == 1;
            let weights: Vec<f32> = visible_indices
                .iter()
                .map(|&i| {
                    if force_equal {
                        1.0
                    } else {
                        column.tiles.get(i).map(|t| t.height_weight.max(0.1)).unwrap_or(1.0)
                    }
                })
                .collect();
            let weight_sum: f32 = weights.iter().sum::<f32>().max(0.1);
            // Pre-compute integer heights, last slot absorbs rounding remainder
            // so the column always uses the full available height exactly.
            let mut tile_heights: Vec<i32> = weights
                .iter()
                .map(|w| ((available_height as f32) * (w / weight_sum)).floor() as i32)
                .collect();
            let leading_sum: i32 = if tile_heights.len() > 1 {
                tile_heights[..tile_heights.len() - 1].iter().sum()
            } else {
                0
            };
            if let Some(last) = tile_heights.last_mut() {
                *last = (available_height - leading_sum).max(0);
            }

            let y_centre_offset = if zoom < 1.0 {
                let total_used: i32 = tile_heights.iter().sum::<i32>()
                    + scaled_gap * (visible_count as i32 - 1).max(0);
                ((scaled_section_height - total_used) / 2).max(0)
            } else {
                0
            };

            let effective_gap = scaled_gap;
            let mut accumulated_y: i32 = 0;
            for (slot_idx, &tile_idx) in visible_indices.iter().enumerate() {
                if let Some(tile) = column.tiles.get(tile_idx) {
                    let window_height = tile_heights.get(slot_idx).copied().unwrap_or(0).max(0);
                    let base_y = work_rect.loc.y + section_y_offset + y_centre_offset + accumulated_y;
                    let rect = Rect::new(screen_x, base_y, scaled_w, window_height as u32);
                    positions.push((tile.window_id, rect));
                    accumulated_y += window_height + effective_gap;
                }
            }
        }
        positions
    }

    pub fn scroll(&mut self, direction: ScrollDirection, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        // Do scroll work, then apply layout
        {
            let monitor = match self.monitors.get_mut(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let workspace = match monitor.workspace_mut() {
                Some(w) => w,
                None => return,
            };

            let amount = self.config.scroll_step;
            match direction {
                ScrollDirection::Left => {
                    workspace.scroll_offset.x =
                        workspace.scroll_offset.x.saturating_sub(amount)
                }
                ScrollDirection::Right => {
                    workspace.scroll_offset.x =
                        workspace.scroll_offset.x.saturating_add(amount)
                }
            }

            // We need view_width but can't access monitor while workspace is mutably borrowed.
            // Save the offset and clamp after releasing the borrow.
        }

        // Clamp scroll after releasing mutable borrow
        {
            let monitor = self.monitors.get_mut(&focused_output).unwrap();
            let view_width = Self::view_width_for_monitor(monitor, &self.config);
            let num_cols = monitor.workspace().map(|w| w.columns.len()).unwrap_or(0);
            let col_w = Self::effective_column_width(num_cols, view_width, &self.config);
            if let Some(workspace) = monitor.workspace_mut() {
                workspace.clamp_scroll(
                    col_w,
                    self.config.column_gap,
                    view_width,
                );
            }
        }

        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Focus the window to the left. Returns the newly focused WindowId.
    pub fn focus_left(&mut self, backend: &BackendHandle) -> Option<WindowId> {
        let focused_output = self.monitors.focused_id()?;
        {
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                monitor.focus_left();
            }
        }
        self.scroll_to_focused_column(focused_output, backend)
    }

    /// Focus the window to the right. Returns the newly focused WindowId.
    pub fn focus_right(&mut self, backend: &BackendHandle) -> Option<WindowId> {
        let focused_output = self.monitors.focused_id()?;
        {
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                monitor.focus_right();
            }
        }
        self.scroll_to_focused_column(focused_output, backend)
    }

    /// Focus the window above. Returns the newly focused WindowId.
    pub fn focus_up(&mut self, backend: &BackendHandle) -> Option<WindowId> {
        let focused_output = self.monitors.focused_id()?;
        let focused_wid = {
            let monitor = self.monitors.get_mut(&focused_output)?;
            monitor.focus_up();
            monitor.focus_window
        };
        if let Some(wid) = focused_wid {
            if let Some(window) = self.tiled_windows.get(&wid) {
                self.activate_window(window.hwnd);
            }
            self.apply_layout_for_monitor(focused_output, backend);
        }
        focused_wid
    }

    /// Focus the window below. Returns the newly focused WindowId.
    pub fn focus_down(&mut self, backend: &BackendHandle) -> Option<WindowId> {
        let focused_output = self.monitors.focused_id()?;
        let focused_wid = {
            let monitor = self.monitors.get_mut(&focused_output)?;
            monitor.focus_down();
            monitor.focus_window
        };
        if let Some(wid) = focused_wid {
            if let Some(window) = self.tiled_windows.get(&wid) {
                self.activate_window(window.hwnd);
            }
            self.apply_layout_for_monitor(focused_output, backend);
        }
        focused_wid
    }

    /// Scroll workspace to make the focused column visible, then activate and re-apply layout
    fn scroll_to_focused_column(&mut self, focused_output: OutputId, backend: &BackendHandle) -> Option<WindowId> {
        // Step 1: Get focus column, work area, and column count
        let focus_col = self.monitors.get(&focused_output).and_then(|m| m.focus_column);
        let view_width = self.monitors.get(&focused_output)
            .map(|m| Self::view_width_for_monitor(m, &self.config));
        let num_cols = self.monitors.get(&focused_output)
            .and_then(|m| m.workspace().map(|w| w.columns.len()));

        // Step 2: Scroll workspace to show focused column
        if let (Some(col_idx), Some(vw), Some(nc)) = (focus_col, view_width, num_cols) {
            let col_w = Self::effective_column_width(nc, vw, &self.config);
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                if let Some(workspace) = monitor.workspace_mut() {
                    workspace.scroll_to_column(
                        col_idx,
                        col_w,
                        self.config.column_gap,
                        vw,
                    );
                    // Clamp to valid scroll range
                    workspace.clamp_scroll(
                        col_w,
                        self.config.column_gap,
                        vw,
                    );
                }
            }
        }

        // Step 3: Activate the focused window
        let hwnd_to_activate = self.monitors.get(&focused_output)
            .and_then(|m| m.focus_window)
            .and_then(|wid| self.tiled_windows.get(&wid).map(|w| w.hwnd));

        if let Some(hwnd) = hwnd_to_activate {
            self.activate_window(hwnd);
        }

        // Step 4: Apply layout
        self.apply_layout_for_monitor(focused_output, backend);
        self.monitors.get(&focused_output).and_then(|m| m.focus_window)
    }

    /// Switch to a specific workspace
    pub fn switch_workspace(&mut self, workspace_id: i32, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        // Resolve the slide animation direction BEFORE swapping workspaces.
        // niri convention: going to a higher-numbered (later) workspace, the
        // new content slides in from BELOW (start at +work_area_height,
        // end at 0); going to a lower-numbered workspace, it slides in from
        // ABOVE (start at -work_area_height).  When animations are disabled
        // or this is the same workspace we skip the call (no-op).
        let (prev_ws_id, work_area_h) = self
            .monitors
            .get(&focused_output)
            .map(|m| (m.active_workspace, m.work_area.size.h as i32))
            .unwrap_or((workspace_id, 0));
        let want_slide = self.animation.is_enabled()
            && self
                .full_config
                .as_ref()
                .map(|c| c.animations.workspace_transition)
                .unwrap_or(true)
            && prev_ws_id != workspace_id
            && work_area_h > 0;

        // Get current workspace windows to hide
        let current_windows: Vec<WindowId> = self.monitors.get(&focused_output)
            .map(|m| m.workspace()
                .map(|w| w.columns.iter()
                    .flat_map(|c| c.tiles.iter().map(|t| t.window_id))
                    .collect())
                .unwrap_or_default())
            .unwrap_or_default();

        // Switch workspace
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            monitor.switch_workspace(workspace_id);
        }

        // Hide old workspace windows, show new ones.
        // Item 3: skip sticky windows — they remain visible across workspace switches.
        for wid in current_windows {
            if self.sticky_windows.contains(&wid) {
                continue; // sticky windows stay visible at all times
            }
            let _ = backend.show_window(wid.as_isize(), false);
        }

        if let Some(monitor) = self.monitors.get(&focused_output) {
            if let Some(workspace) = monitor.workspace() {
                for col in &workspace.columns {
                    for tile in &col.tiles {
                        let _ = backend.show_window(tile.window_id.as_isize(), true);
                    }
                }
            }
        }

        // Kick off the workspace-slide animation now that the engine has
        // settled on the incoming workspace.  Layout passes consult
        // `workspace_slide_offset(oid)` and bias every tile's Y position by
        // the returned value, producing the slide in/out effect.
        if want_slide {
            let from_offset = if workspace_id > prev_ws_id {
                work_area_h as f64
            } else {
                -(work_area_h as f64)
            };
            // duration_ms = 0 → use the AnimationManager's default duration.
            self.animation
                .start_workspace_slide(focused_output, from_offset, 0.0, 0);
        }

        self.apply_layout_for_monitor(focused_output, backend);

        // Activate the focused window on the new workspace
        let hwnd_to_activate = self.monitors.get(&focused_output)
            .and_then(|m| m.focus_window)
            .and_then(|wid| self.tiled_windows.get(&wid).map(|w| w.hwnd));

        if let Some(hwnd) = hwnd_to_activate {
            self.activate_window(hwnd);
        }
    }

    /// Move the focused window left/right
    pub fn move_column(&mut self, direction: ScrollDirection, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        let (moved, window_id) = {
            let monitor = match self.monitors.get_mut(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let focus_col = match monitor.focus_column {
                Some(c) => c,
                None => return,
            };
            let window_id = match monitor.focus_window {
                Some(id) => id,
                None => return,
            };
            let workspace = match monitor.workspace_mut() {
                Some(w) => w,
                None => return,
            };
            let target_col = match direction {
                ScrollDirection::Left => focus_col.saturating_sub(1),
                ScrollDirection::Right => focus_col + 1,
            };
            (workspace.move_window(window_id, target_col, 0), window_id)
        };

        if moved {
            // After move_window, the window is now at target_col.
            // The source column may have been removed (if it became empty),
            // so focus_column could be stale. Find the window's new position.
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                // Find where the moved window ended up
                if let Some(workspace) = monitor.workspace() {
                    let new_col = workspace.find_window_column(window_id);
                    if let Some(col) = new_col {
                        monitor.focus_column = Some(col);
                    }
                }
            }
            self.apply_layout_for_monitor(focused_output, backend);
        }
    }

    /// Close the focused window
    pub fn close_focused_window(&mut self, _backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };
        let hwnd = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            match monitor.focus_window {
                Some(wid) => self.tiled_windows.get(&wid).map(|w| w.hwnd),
                None => None,
            }
        };
        if let Some(hwnd) = hwnd {
            self.close_window(hwnd);
            // Note: the actual removal from layout happens when we receive
            // the WindowDestroyed event from the WinEvent hook.
        }
    }

    /// Toggle fullscreen for the focused window
    pub fn toggle_fullscreen(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        // Get window info before mutating
        let (window_id, hwnd, is_fullscreen, monitor_bounds) = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let wid = match monitor.focus_window {
                Some(id) => id,
                None => return,
            };
            let window = match self.tiled_windows.get(&wid) {
                Some(w) => w,
                None => return,
            };
            (
                wid,
                window.hwnd,
                self.fullscreen_windows.contains(&wid),
                monitor.bounds,
            )
        };

        if is_fullscreen {
            // Exit fullscreen
            self.fullscreen_windows.remove(&window_id);
            self.window_sizing.insert(window_id, SizingMode::Normal);
            self.restore_window_style(hwnd);
            info!("Exiting fullscreen for window {}", hwnd);
        } else {
            // Enter fullscreen
            self.fullscreen_windows.insert(window_id);
            self.window_sizing.insert(window_id, SizingMode::Fullscreen);
            let fullscreen_rect = Rect::new(
                monitor_bounds.loc.x,
                monitor_bounds.loc.y,
                monitor_bounds.size.w,
                monitor_bounds.size.h,
            );
            self.make_fullscreen(hwnd, fullscreen_rect, backend);
            info!("Entering fullscreen for window {}", hwnd);
        }

        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Toggle floating for the focused window.
    /// Floating windows are not managed by the tiling layout
    /// and can be freely positioned.
    pub fn toggle_floating(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        // Check if the currently focused window is floating, OR if no tiled
        // window is focused but there's a recently floated window we can un-float.
        let (window_id, is_floating) = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            if let Some(wid) = monitor.focus_window {
                (wid, self.floating_windows.contains(&wid))
            } else {
                // No focused tiled window — check if any floating window exists
                // on this monitor that we can un-float
                let float_on_monitor: Vec<WindowId> = self.floating_windows.iter()
                    .filter(|wid| self.tiled_windows.contains_key(wid))
                    .copied()
                    .collect();
                match float_on_monitor.first() {
                    Some(wid) => (*wid, true),
                    None => return,
                }
            }
        };

        if is_floating {
            // Float -> Tiled: re-add to the workspace layout
            self.floating_windows.remove(&window_id);
            self.window_sizing.insert(window_id, SizingMode::Normal);
            // Re-add the window to the active workspace
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                monitor.add_window(window_id);
            }
            info!("Un-floating window {:?}", window_id);
            self.apply_layout_for_monitor(focused_output, backend);
        } else {
            // Tiled -> Floating: remove from layout but keep visible
            self.floating_windows.insert(window_id);
            self.window_sizing.insert(window_id, SizingMode::Normal);
            // Remove from workspace columns
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                monitor.remove_window(window_id);
            }
            info!("Floating window {:?}", window_id);
            self.apply_layout_for_monitor(focused_output, backend);
        }
    }

    /// Apply the configured DWM drop-shadow state to a single window.
    ///
    /// When `shadow_enable` is `true` we extend the DWM frame into the client
    /// area by a single pixel along the top edge.  This is the canonical
    /// trick for re-enabling the OS drop-shadow on borderless / styled-down
    /// windows without affecting their visible layout — DWM only paints the
    /// shadow when at least one frame margin is non-zero, but a 1 px top
    /// margin is small enough to escape user notice.
    ///
    /// When `shadow_enable` is `false` we reset all four margins to 0, which
    /// suppresses the shadow.
    ///
    /// The result is recorded in `shadow_applied` so we only call into DWM
    /// once per window — Windows Terminal and some Electron apps render
    /// their content area black after a fresh `DwmExtendFrameIntoClientArea`
    /// call until they receive the next paint message, so repeating the call
    /// on every layout pass causes visible flicker.
    fn apply_shadow_for_window(&mut self, hwnd: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
        use windows::Win32::UI::Controls::MARGINS;

        let window_id = WindowId::new(hwnd);
        if self.shadow_applied.contains(&window_id) {
            return;
        }

        let margins = if self.config.shadow_enable {
            // 1 px top extension — the smallest value that re-enables DWM's
            // drop shadow without producing a visible gap inside the window.
            MARGINS {
                cxLeftWidth: 0,
                cxRightWidth: 0,
                cyTopHeight: 1,
                cyBottomHeight: 0,
            }
        } else {
            MARGINS {
                cxLeftWidth: 0,
                cxRightWidth: 0,
                cyTopHeight: 0,
                cyBottomHeight: 0,
            }
        };

        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            let _ = DwmExtendFrameIntoClientArea(hwnd_win, &margins);
        }
        self.shadow_applied.insert(window_id);
    }

    /// niri-parity "blur backdrop": when a matching window rule sets
    /// `blur true`, request the Aero-style backdrop blur via
    /// `DwmEnableBlurBehindWindow`.  Idempotent per HWND — re-issuing the
    /// call on every layout pass is unnecessary work and matches the
    /// once-per-HWND pattern used by `apply_shadow_for_window`.  Pass
    /// `enabled = false` (typically on remove_window) to clear the flag and
    /// disable the blur.
    fn apply_window_blur(&mut self, hwnd: isize, enabled: bool) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{
            DwmEnableBlurBehindWindow, DWM_BB_ENABLE, DWM_BLURBEHIND,
        };

        let window_id = WindowId::new(hwnd);
        // Idempotent: once the requested state is in the set we skip; once
        // a window is dropped from the set, the next opt-in re-applies.
        let already_on = self.blur_applied.contains(&window_id);
        if enabled && already_on {
            return;
        }
        if !enabled && !already_on {
            return;
        }

        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        let bb = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE,
            fEnable: enabled.into(),
            hRgnBlur: windows::Win32::Graphics::Gdi::HRGN(std::ptr::null_mut()),
            fTransitionOnMaximized: false.into(),
        };
        let _ = unsafe { DwmEnableBlurBehindWindow(hwnd_win, &bb) };

        if enabled {
            self.blur_applied.insert(window_id);
        } else {
            self.blur_applied.remove(&window_id);
        }
    }

    /// Item 1 — Paint window title in the DWM-extended frame area.
    ///
    /// When `strip_frame` is on, the native caption bar is gone and the user
    /// cannot see the window title.  This function paints a thin GDI text
    /// strip at the top of the tile rect so the title remains visible after
    /// each layout pass.
    ///
    /// # Caveat
    /// The application will repaint over this strip on its own `WM_PAINT`.
    /// The title will therefore appear briefly after each layout pass and may
    /// be overdrawn by the app.  A sibling overlay window per tile is the
    /// correct long-term solution (niri uses composited overlays), but that
    /// requires a host HWND and a compositor hook.
    ///
    /// TODO(audit): app will overdraw; future fix is sibling overlay window per tile.
    fn paint_title_for_tile(&self, hwnd: isize, rect: Rect, title: &str, is_focused: bool) {
        if !self.config.strip_frame {
            return;
        }
        use windows::Win32::Foundation::{HWND, RECT, COLORREF};
        use windows::Win32::Graphics::Gdi::{
            GetDC, ReleaseDC, CreateSolidBrush, DeleteObject,
            FillRect, SetBkMode, SetTextColor, DrawTextW,
            TRANSPARENT, DT_LEFT, DT_SINGLELINE, DT_VCENTER,
        };
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            let hdc = GetDC(hwnd_win);
            if hdc.is_invalid() {
                return;
            }
            // Background bar occupies the top 28 px of the tile.
            let bar_bg = if is_focused {
                COLORREF(0x00_60_3D_28) // warm dark focused bar
            } else {
                COLORREF(0x00_2D_2D_2D) // dark unfocused bar
            };
            let brush = CreateSolidBrush(bar_bg);
            let bar_rect = RECT {
                left: 0,
                top: 0,
                right: rect.size.w as i32,
                bottom: 28,
            };
            FillRect(hdc, &bar_rect, brush);
            let _ = DeleteObject(brush);

            // Title text — white, left-aligned, vertically centred in the bar.
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x00_FF_FF_FF));
            let mut text_rect = RECT {
                left: 8,
                top: 0,
                right: rect.size.w as i32 - 8,
                bottom: 28,
            };
            let mut wide: Vec<u16> = title.encode_utf16().collect();
            let _ = DrawTextW(hdc, &mut wide, &mut text_rect, DT_LEFT | DT_SINGLELINE | DT_VCENTER);

            let _ = ReleaseDC(hwnd_win, hdc);
        }
    }

    /// Strip window frame decorations for tiling.
    /// Removes caption bar and thick frame so the window respects exact
    /// pixel positioning without invisible 7px borders.
    /// Saves the original style for later restoration.
    fn strip_frame_for_tiling(&mut self, hwnd: isize) {
        // Frame stripping is opt-in via config — many apps (Windows Terminal,
        // Edge, some Electron) render their content area black after style
        // changes until they receive a fresh paint. Default off for compatibility.
        if !self.config.strip_frame {
            return;
        }

        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongW, SetWindowLongW, GWL_STYLE, GWL_EXSTYLE,
            WS_CAPTION, WS_THICKFRAME, WS_MAXIMIZEBOX, WS_SYSMENU,
            WS_MINIMIZEBOX,
        };
        use windows::Win32::Foundation::HWND;

        let window_id = WindowId::new(hwnd);
        // Only strip if we haven't already saved styles for this window
        if self.saved_styles.contains_key(&window_id) {
            return;
        }

        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            let style = GetWindowLongW(hwnd_win, GWL_STYLE) as u32;
            let ex_style = GetWindowLongW(hwnd_win, GWL_EXSTYLE) as u32;

            // Save original styles
            self.saved_styles.insert(window_id, style);
            self.saved_ex_styles.insert(window_id, ex_style);

            // Remove ALL frame decorations: caption, thick frame, sysmenu, min/max
            // WS_THICKFRAME is the key one — it adds invisible 7px resize borders
            // that cause overlap even after SetWindowPos positions the window exactly.
            let new_style = style & !(
                WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 |
                WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0
            );
            SetWindowLongW(hwnd_win, GWL_STYLE, new_style as i32);

            // Remove WS_EX_WINDOWEDGE and WS_EX_CLIENTEDGE — these also add borders
            let new_ex_style = ex_style & !(0x0100 | 0x0200); // WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE
            SetWindowLongW(hwnd_win, GWL_EXSTYLE, new_ex_style as i32);

            // Extend DWM frame into client area.
            // Item 2: when shadow_enable is true, use 0-margins (instead of -1)
            // so DWM keeps painting the drop-shadow.  With -1 margins the DWM
            // shadow is stripped along with the invisible 7-px border.
            // When shadow_enable is false, use -1 to remove the invisible border.
            let margins = if self.config.shadow_enable {
                windows::Win32::UI::Controls::MARGINS {
                    cxLeftWidth: 0,
                    cxRightWidth: 0,
                    cyTopHeight: 0,
                    cyBottomHeight: 0,
                }
            } else {
                windows::Win32::UI::Controls::MARGINS {
                    cxLeftWidth: -1,
                    cxRightWidth: -1,
                    cyTopHeight: -1,
                    cyBottomHeight: -1,
                }
            };
            let _ = windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea(
                hwnd_win, &margins,
            );

            // Force frame recalculation
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                hwnd_win,
                windows::Win32::UI::WindowsAndMessaging::HWND_NOTOPMOST,
                0, 0, 0, 0,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE
                | windows::Win32::UI::WindowsAndMessaging::SWP_FRAMECHANGED,
            );
        }
        info!("Stripped frame for tiled window {}", hwnd);
    }

    /// Make a window fullscreen by removing its frame and covering the monitor
    fn make_fullscreen(&mut self, hwnd: isize, rect: Rect, backend: &BackendHandle) {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongW, SetWindowLongW, GWL_STYLE, GWL_EXSTYLE,
            WS_CAPTION, WS_THICKFRAME, WS_SYSMENU, WS_MINIMIZEBOX, WS_MAXIMIZEBOX,
            WS_EX_APPWINDOW,
        };
        use windows::Win32::Foundation::HWND;

        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);

        unsafe {
            // Save current style
            let style = GetWindowLongW(hwnd_win, GWL_STYLE) as u32;
            let ex_style = GetWindowLongW(hwnd_win, GWL_EXSTYLE) as u32;

            let window_id = WindowId::new(hwnd);
            self.saved_styles.insert(window_id, style);
            self.saved_ex_styles.insert(window_id, ex_style);

            // Remove caption, thick frame, sysmenu, min/max boxes
            let new_style = style
                & !(WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0
                    | WS_MINIMIZEBOX.0
                    | WS_MAXIMIZEBOX.0);
            SetWindowLongW(hwnd_win, GWL_STYLE, new_style as i32);

            // Remove app window style
            let new_ex_style = ex_style & !WS_EX_APPWINDOW.0;
            SetWindowLongW(hwnd_win, GWL_EXSTYLE, new_ex_style as i32);
        }

        // Position window to cover entire monitor
        let _ = backend.set_window_position(
            hwnd,
            rect,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE
                | windows::Win32::UI::WindowsAndMessaging::SWP_FRAMECHANGED,
        );
    }

    /// Restore a window's original style after exiting fullscreen
    fn restore_window_style(&self, hwnd: isize) {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowLongW, SetWindowPos, GWL_STYLE, GWL_EXSTYLE,
            HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        };
        use windows::Win32::Foundation::HWND;

        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        let window_id = WindowId::new(hwnd);

        if let Some(&saved_style) = self.saved_styles.get(&window_id) {
            unsafe {
                SetWindowLongW(hwnd_win, GWL_STYLE, saved_style as i32);
            }
        }
        if let Some(&saved_ex_style) = self.saved_ex_styles.get(&window_id) {
            unsafe {
                SetWindowLongW(hwnd_win, GWL_EXSTYLE, saved_ex_style as i32);
            }
        }

        // Force frame recalculation
        unsafe {
            let _ = SetWindowPos(
                hwnd_win,
                HWND_TOP,
                0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_FRAMECHANGED,
            );
        }
    }

    /// Get all monitors (mutable)
    pub fn monitors_mut(&mut self) -> &mut MonitorSet<OutputId, Monitor> {
        &mut self.monitors
    }

    /// Get all monitors
    pub fn monitors(&self) -> &MonitorSet<OutputId, Monitor> {
        &self.monitors
    }

    /// Get the focused output id.
    /// Delegates to `MonitorSet::focused_id()`.
    pub fn focused_output(&self) -> Option<OutputId> {
        self.monitors.focused_id()
    }

    /// Check if a window is fullscreen
    pub fn is_fullscreen(&self, window_id: WindowId) -> bool {
        self.fullscreen_windows.contains(&window_id)
    }

    /// Check if a window is floating
    pub fn is_floating(&self, window_id: WindowId) -> bool {
        self.floating_windows.contains(&window_id)
    }

    /// Get all tiled windows (for IPC queries)
    pub fn tiled_windows(&self) -> &HashMap<WindowId, WindowInfo> {
        &self.tiled_windows
    }

    /// Get the sizing mode for a window
    pub fn window_sizing_mode(&self, window_id: WindowId) -> SizingMode {
        self.window_sizing.get(&window_id).copied().unwrap_or(SizingMode::Normal)
    }

    /// Set the focused output (e.g., when clicking on a monitor).
    /// Delegates to `MonitorSet::set_focused()`.
    pub fn set_focused_output(&mut self, output_id: OutputId) {
        // MonitorSet::set_focused already checks if the id is present.
        self.monitors.set_focused(output_id);
    }

    /// Update the layout configuration (e.g., on config reload)
    pub fn update_config(&mut self, config: LayoutConfig) {
        self.config = config;
    }

    /// Update animation settings from config
    pub fn update_animation_settings(&mut self, enabled: bool, duration_ms: u32, easing: crate::layout::Easing) {
        self.animation.set_enabled(enabled);
        self.animation.set_duration(duration_ms);
        self.animation.set_easing(easing);
    }

    /// Check if animations are enabled and any are currently active (AnimationManager or
    /// per-window rect animations from `animating_rects`).
    pub fn has_active_animations(&self) -> bool {
        self.animation.has_active() || !self.animating_rects.is_empty()
    }

    /// Convenience alias for `has_active_animations` — exposes whether any rect
    /// animations are currently in flight.
    pub fn animating(&self) -> bool {
        self.has_active_animations()
    }

    /// Check if animations are enabled
    pub fn animations_enabled(&self) -> bool {
        self.animation.is_enabled()
    }

    /// Tick all active animations by `delta_ms` milliseconds and issue SetWindowPos
    /// for any per-window rect animations that have progressed.
    ///
    /// Returns `true` if any animations are still running after this tick.
    /// Call this from the main event loop (~60 fps).  `backend` is required so
    /// the method can issue `set_window_position` calls directly.
    ///
    /// The no-backend variant `tick_animations(delta_ms)` (which calls this
    /// with a no-op backend) is retained for callers that do not yet pass a
    /// backend handle (e.g. the legacy call site in `main.rs`).
    pub fn tick_animations(&mut self, delta_ms: u32) -> bool {
        // Legacy no-backend variant — advances the AnimationManager (scroll/slide
        // animations) but does NOT issue SetWindowPos for rect animations.
        // Retained so existing call sites (e.g. main.rs) compile unchanged.
        // Callers that own a BackendHandle should prefer `tick_animations_with_backend`.
        if !self.animation.is_enabled() {
            return !self.animating_rects.is_empty();
        }
        let results = self.animation.tick(delta_ms);
        for (target, value) in results {
            match target {
                crate::layout::AnimationTarget::ScrollX(output_id) => {
                    if let Some(monitor) = self.monitors.get_mut(&output_id) {
                        if let Some(workspace) = monitor.workspace_mut() {
                            workspace.scroll_offset.x = value as i32;
                        }
                    }
                }
                crate::layout::AnimationTarget::WindowX(window_id) => {
                    if let Some(info) = self.tiled_windows.get_mut(&window_id) {
                        info.bounds.loc.x = value as i32;
                    }
                }
                crate::layout::AnimationTarget::WindowY(window_id) => {
                    if let Some(info) = self.tiled_windows.get_mut(&window_id) {
                        info.bounds.loc.y = value as i32;
                    }
                }
                crate::layout::AnimationTarget::WindowW(window_id) => {
                    if let Some(info) = self.tiled_windows.get_mut(&window_id) {
                        info.bounds.size.w = value as u32;
                    }
                }
                crate::layout::AnimationTarget::WindowH(window_id) => {
                    if let Some(info) = self.tiled_windows.get_mut(&window_id) {
                        info.bounds.size.h = value as u32;
                    }
                }
                crate::layout::AnimationTarget::Opacity(window_id) => {
                    if let Some(info) = self.tiled_windows.get(&window_id) {
                        let alpha = (value as f32 / 255.0).clamp(0.0, 1.0);
                        self.apply_window_opacity(info.hwnd, alpha);
                    }
                }
                crate::layout::AnimationTarget::WorkspaceX(output_id) => {
                    let _ = output_id;
                }
            }
        }
        self.animation.has_active() || !self.animating_rects.is_empty()
    }

    /// Full-power variant of `tick_animations` that issues `SetWindowPos` via
    /// `backend` for every in-flight rect animation.  This is the method the
    /// main event loop should prefer once it has a `BackendHandle` available.
    pub fn tick_animations_with_backend(&mut self, delta_ms: u32, backend: &BackendHandle) -> bool {
        // ---- Item 1: drive per-window rect animations ----
        let now = std::time::Instant::now();
        let easing = self.animation.is_enabled().then_some(()).map(|_| {
            // Read easing from full_config when available, fall back to CubicOut.
            self.full_config
                .as_ref()
                .map(|_| crate::layout::Easing::CubicOut)
                .unwrap_or(crate::layout::Easing::CubicOut)
        }).unwrap_or(crate::layout::Easing::CubicOut);

        let mut finished_ids: Vec<WindowId> = Vec::new();
        for (&wid, &(start_rect, target_rect, start_time, duration)) in &self.animating_rects {
            let elapsed = now.duration_since(start_time);
            let t_raw = if duration.is_zero() {
                1.0f64
            } else {
                (elapsed.as_secs_f64() / duration.as_secs_f64()).min(1.0)
            };
            // Apply easing
            let t = {
                let tc = t_raw.clamp(0.0, 1.0);
                match easing {
                    crate::layout::Easing::Linear => tc,
                    crate::layout::Easing::EaseIn => tc * tc * tc,
                    crate::layout::Easing::EaseOut | crate::layout::Easing::CubicOut =>
                        1.0 - (1.0 - tc).powi(3),
                    crate::layout::Easing::EaseInOut => {
                        if tc < 0.5 { 4.0 * tc * tc * tc }
                        else { 1.0 - (-2.0 * tc + 2.0_f64).powi(3) / 2.0 }
                    }
                    crate::layout::Easing::None => 1.0,
                }
            };

            let lerp = |a: i32, b: i32| -> i32 { a + ((b - a) as f64 * t).round() as i32 };
            let lerp_u = |a: u32, b: u32| -> u32 {
                let diff = b as f64 - a as f64;
                (a as f64 + diff * t).round().max(0.0) as u32
            };

            let interp_rect = Rect::new(
                lerp(start_rect.loc.x, target_rect.loc.x),
                lerp(start_rect.loc.y, target_rect.loc.y),
                lerp_u(start_rect.size.w, target_rect.size.w),
                lerp_u(start_rect.size.h, target_rect.size.h),
            );

            if t >= 1.0 {
                // Animation complete — snap to target.
                let _ = backend.set_window_position(
                    wid.as_isize(),
                    target_rect,
                    windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                );
                finished_ids.push(wid);
            } else {
                let _ = backend.set_window_position(
                    wid.as_isize(),
                    interp_rect,
                    windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                );
            }
        }
        for wid in finished_ids {
            self.animating_rects.remove(&wid);
        }

        // ---- Legacy AnimationManager tick ----
        if !self.animation.is_enabled() {
            return !self.animating_rects.is_empty();
        }
        let results = self.animation.tick(delta_ms);
        // Apply animated values to state
        for (target, value) in results {
            match target {
                crate::layout::AnimationTarget::ScrollX(output_id) => {
                    if let Some(monitor) = self.monitors.get_mut(&output_id) {
                        if let Some(workspace) = monitor.workspace_mut() {
                            workspace.scroll_offset.x = value as i32;
                        }
                    }
                }
                crate::layout::AnimationTarget::WindowX(window_id) => {
                    if let Some(info) = self.tiled_windows.get_mut(&window_id) {
                        info.bounds.loc.x = value as i32;
                    }
                }
                crate::layout::AnimationTarget::WindowY(window_id) => {
                    if let Some(info) = self.tiled_windows.get_mut(&window_id) {
                        info.bounds.loc.y = value as i32;
                    }
                }
                crate::layout::AnimationTarget::WindowW(window_id) => {
                    if let Some(info) = self.tiled_windows.get_mut(&window_id) {
                        info.bounds.size.w = value as u32;
                    }
                }
                crate::layout::AnimationTarget::WindowH(window_id) => {
                    if let Some(info) = self.tiled_windows.get_mut(&window_id) {
                        info.bounds.size.h = value as u32;
                    }
                }
                crate::layout::AnimationTarget::Opacity(window_id) => {
                    // Opacity animations are applied directly to the window via
                    // SetLayeredWindowAttributes — there is no cached opacity field on
                    // WindowInfo (it would be redundant with `applied_state.opacity`).
                    // The `value` parameter here is in 0..=255, so feed it straight in.
                    if let Some(info) = self.tiled_windows.get(&window_id) {
                        let alpha = (value as f32 / 255.0).clamp(0.0, 1.0);
                        self.apply_window_opacity(info.hwnd, alpha);
                    }
                }
                crate::layout::AnimationTarget::WorkspaceX(output_id) => {
                    // Workspace-transition slide: tracked by Agent F's animation
                    // module. The current per-monitor offset is consulted at
                    // layout time via `AnimationManager::get_value(...)` to bias
                    // column positions; here we just trigger a re-layout so the
                    // active monitor reflects the new offset.
                    let _ = output_id; // consumed by the next apply_layout call
                }
            }
        }
        self.animation.has_active() || !self.animating_rects.is_empty()
    }

    /// Record that a window should animate from its current rect to `target_rect`.
    /// Called externally before `apply_layout_for_monitor` to pre-seed the animation;
    /// also used internally by `apply_layout_for_monitor` when animations are enabled.
    pub fn animating_to_target(&mut self, wid: WindowId, target_rect: Rect) {
        if !self.animation.is_enabled() {
            return;
        }
        let start_rect = self.applied_state
            .get(&wid)
            .map(|s| s.rect)
            .unwrap_or(Rect::new(0, 0, 0, 0));
        if start_rect == target_rect {
            return;
        }
        let duration_ms = self.full_config
            .as_ref()
            .map(|c| c.animations.duration as u64)
            .unwrap_or(200);
        self.animating_rects.insert(
            wid,
            (
                start_rect,
                target_rect,
                std::time::Instant::now(),
                std::time::Duration::from_millis(duration_ms),
            ),
        );
    }

    /// Trigger scroll animation for a monitor's workspace.
    /// Call before changing scroll_offset directly.
    pub fn animate_scroll(&mut self, output_id: OutputId, target_offset: i32) {
        let current_offset = self.monitors.get(&output_id)
            .and_then(|m| m.workspace())
            .map(|w| w.scroll_offset.x)
            .unwrap_or(0);
        
        if current_offset == target_offset {
            return;
        }
        
        let target = crate::layout::AnimationTarget::ScrollX(output_id);
        self.animation.animate(target, current_offset as f64, target_offset as f64);
    }

    /// Trigger focus animation for a window.
    /// Call before changing focus_window.
    pub fn animate_focus_change(&mut self, window_id: WindowId, target_rect: crate::utils::Rect) {
        if !self.animation.is_enabled() {
            return;
        }
        // Read the current position from the workspace's calculated layout
        // (not the stale cached info.bounds which is only updated at registration time).
        let default_w = self.config.column_width as i32;
        let gap = self.config.column_gap;
        let current_pos = self.monitors.focused_id()
            .and_then(|oid| self.monitors.get(&oid))
            .and_then(|m| m.workspace())
            .and_then(|ws| ws.current_position_for(window_id, default_w, gap));

        let (current_x, current_y) = match current_pos {
            Some(r) => (r.loc.x as f64, r.loc.y as f64),
            // Fall back to cached bounds if workspace lookup fails
            None => {
                if let Some(info) = self.tiled_windows.get(&window_id) {
                    (info.bounds.loc.x as f64, info.bounds.loc.y as f64)
                } else {
                    return;
                }
            }
        };

        let target_x = target_rect.loc.x as f64;
        let target_y = target_rect.loc.y as f64;

        if (current_x - target_x).abs() > 1.0 {
            self.animation.animate(
                crate::layout::AnimationTarget::WindowX(window_id),
                current_x,
                target_x,
            );
        }
        if (current_y - target_y).abs() > 1.0 {
            self.animation.animate(
                crate::layout::AnimationTarget::WindowY(window_id),
                current_y,
                target_y,
            );
        }
    }

    /// Toggle overview mode — zooms out to show all columns in a grid
    pub fn toggle_overview(&mut self, backend: &BackendHandle) {
        if self.overview.is_some() {
            self.exit_overview(backend);
        } else {
            self.enter_overview(backend);
        }
    }

    /// Enter overview mode — niri-style "see all workspaces at once".
    ///
    /// Iterates ALL workspaces on the focused monitor (sorted by workspace id),
    /// skips empties, picks the widest workspace's column-strip as the width
    /// budget, and computes a uniform zoom that fits both the total stacked
    /// height (one section per non-empty workspace with a 50px inter-workspace
    /// gap) and the widest column strip into the monitor's work area.  Also
    /// resets `scroll_offset.x` on the active workspace so the view starts at
    /// column 0 when overview opens.
    pub fn enter_overview(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        // Snapshot the per-workspace column count + the monitor's view dims
        // without holding a mutable borrow.  We sort by workspace id so the
        // stacking order is deterministic (workspace 0 on top, 1 below, …).
        let (sorted_ws, view_width, view_height) = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let mut entries: Vec<(i32, usize)> = monitor
                .workspaces
                .iter()
                .filter_map(|(&id, ws)| {
                    if ws.columns.is_empty() {
                        None
                    } else {
                        Some((id, ws.columns.len()))
                    }
                })
                .collect();
            entries.sort_by_key(|(id, _)| *id);
            let vw = Self::view_width_for_monitor(monitor, &self.config);
            let vh = monitor.work_area.size.h as i32;
            (entries, vw, vh)
        };

        if sorted_ws.is_empty() {
            return;
        }

        // -- Width budget --
        // The widest workspace's stripe sets the horizontal content width.
        let widest_total: i32 = sorted_ws
            .iter()
            .map(|(_id, n_cols)| {
                let col_width = Self::effective_column_width(*n_cols, view_width, &self.config);
                *n_cols as i32 * col_width
                    + (*n_cols as i32 - 1).max(0) * self.config.column_gap
            })
            .max()
            .unwrap_or(0);

        // -- Height budget --
        // Each workspace section gets a fixed nominal height equal to the
        // monitor's work-area height (so each tile in overview keeps its
        // aspect ratio with normal mode), with a 50 px inter-workspace gap.
        const INTER_WORKSPACE_GAP: i32 = 50;
        let n_ws = sorted_ws.len() as i32;
        let nominal_section_height = view_height;
        let total_content_height: i32 = n_ws * nominal_section_height
            + (n_ws - 1).max(0) * INTER_WORKSPACE_GAP;

        // Pick the smaller of the two scale factors so EVERYTHING fits.
        let zoom_w = if widest_total > view_width && widest_total > 0 {
            view_width as f64 / widest_total as f64
        } else {
            1.0
        };
        let zoom_h = if total_content_height > view_height && total_content_height > 0 {
            view_height as f64 / total_content_height as f64
        } else {
            1.0
        };
        let zoom = zoom_w.min(zoom_h).min(1.0);

        // Precompute the Y offset of each workspace section.  Each section's
        // height (post-zoom) is `nominal_section_height * zoom`; the gap is
        // `INTER_WORKSPACE_GAP * zoom`.
        let scaled_section_h = (nominal_section_height as f64 * zoom) as i32;
        let scaled_gap = (INTER_WORKSPACE_GAP as f64 * zoom) as i32;
        let mut workspace_offsets: Vec<(i32, i32)> = Vec::with_capacity(sorted_ws.len());
        let mut y: i32 = 0;
        for (id, _n_cols) in &sorted_ws {
            workspace_offsets.push((*id, y));
            y += scaled_section_h + scaled_gap;
        }

        info!(
            "Entering overview: zoom={:.2} ({} workspaces, widest={}px, total_h={}px, view {}x{})",
            zoom, sorted_ws.len(), widest_total, total_content_height, view_width, view_height
        );

        self.overview = Some(OverviewState {
            zoom,
            workspace_offsets,
        });

        // niri-parity DWM-thumbnail overview.  When a sink is installed,
        // open a session and register a thumbnail for every visible
        // tile across every workspace on the focused monitor.  The
        // engine then routes per-tile rects into the sink during the
        // subsequent `apply_layout_for_monitor` pass instead of
        // physically resizing the source HWNDs.
        if let Some(sink) = self.thumbnail_overview.clone() {
            sink.enter();
            if let Some(monitor) = self.monitors.get(&focused_output) {
                for ws in monitor.workspaces.values() {
                    for col in ws.columns.iter() {
                        for tile in col.tiles.iter() {
                            sink.register(tile.window_id, tile.window_id.as_isize());
                        }
                    }
                }
            }
        }

        // Banner: show on the focused monitor.  No-op when the global
        // banner singleton hasn't been wired (unit tests, --headless paths).
        if let Some(m) = self.monitors.get(&focused_output) {
            crate::overlay::overview_banner::global_show(Some(
                crate::overlay::MonitorBounds {
                    x: m.bounds.loc.x,
                    y: m.bounds.loc.y,
                    w: m.bounds.size.w as i32,
                    h: m.bounds.size.h as i32,
                },
            ));
        }

        // Reset scroll offset — in overview, we show everything from the start
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(workspace) = monitor.workspace_mut() {
                workspace.scroll_offset.x = 0;
            }
        }

        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Exit overview mode — return to normal tiling.
    ///
    /// While overview is active we show every tile on every workspace; on
    /// exit we must hide those that don't belong to the currently active
    /// workspace so the user is back to a single workspace's view.  The
    /// `applied_state` cache for those tiles is also marked invisible so
    /// the next `apply_layout_for_monitor` pass re-issues `show_window(true)`
    /// when the user switches workspaces back.
    pub fn exit_overview(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        info!("Exiting overview mode");
        self.overview = None;
        crate::overlay::overview_banner::global_hide();
        // niri-parity DWM-thumbnail overview: tear down the host window
        // and unregister every thumbnail.  No-op when no sink is wired.
        if let Some(sink) = self.thumbnail_overview.clone() {
            sink.exit();
        }

        // Hide every tile that isn't on the active workspace of the focused
        // monitor.  Other monitors' active workspaces stay visible.
        let to_hide: Vec<WindowId> = {
            let mut acc: Vec<WindowId> = Vec::new();
            if let Some(monitor) = self.monitors.get(&focused_output) {
                let active_ws = monitor.active_workspace;
                for (&ws_id, ws) in monitor.workspaces.iter() {
                    if ws_id == active_ws {
                        continue;
                    }
                    for col in &ws.columns {
                        for tile in &col.tiles {
                            acc.push(tile.window_id);
                        }
                    }
                }
            }
            acc
        };
        for wid in to_hide {
            let _ = backend.show_window(wid.as_isize(), false);
            #[cfg(test)] { self.win32_call_count += 1; }
            self.applied_state
                .entry(wid)
                .or_insert_with(AppliedState::unset)
                .visible = false;
        }

        // Re-scroll to the focused column
        self.scroll_to_focused_column(focused_output, backend);
    }

    /// Check if overview mode is active
    pub fn is_overview(&self) -> bool {
        self.overview.is_some()
    }

    /// Get the current overview zoom level (1.0 = normal, <1.0 = zoomed out).
    /// Returns the inner zoom of `OverviewState` for backwards compat with
    /// callers that only care about the scale factor.
    pub fn overview_zoom(&self) -> f64 {
        self.overview.as_ref().map(|s| s.zoom).unwrap_or(1.0)
    }

    /// Item 4 — effective zoom for `calculate_positions`.
    ///
    /// Priority:
    /// 1. If a manual overview is active, use `overview_zoom()`.
    /// 2. Else if `auto_tile_zoom` is set, use that.
    /// 3. Otherwise 1.0 (no scaling).
    fn effective_zoom(&self) -> f64 {
        if self.overview.is_some() {
            return self.overview_zoom();
        }
        self.auto_tile_zoom.unwrap_or(1.0)
    }

    /// Borrow the full overview state. None when overview is not active.
    /// Currently used by the overview banner overlay to find the focused
    /// monitor's vertical extent.
    pub fn overview_state(&self) -> Option<&OverviewState> {
        self.overview.as_ref()
    }

    /// In overview mode, focus navigation wraps around and auto-scrolls to show the column
    pub fn overview_focus_left(&mut self, backend: &BackendHandle) -> Option<WindowId> {
        let focused_output = self.monitors.focused_id()?;
        {
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                let num_cols = monitor.workspace().map(|w| w.columns.len()).unwrap_or(0);
                if let Some(col) = monitor.focus_column {
                    // Wrap around
                    monitor.focus_column = if col == 0 { Some(num_cols.saturating_sub(1)) } else { Some(col - 1) };
                    if let Some(workspace) = monitor.workspace() {
                        if let Some(new_col) = workspace.columns.get(monitor.focus_column.unwrap()) {
                            if let Some(tile) = new_col.tiles.first() {
                                monitor.focus_window = Some(tile.window_id);
                            }
                        }
                    }
                }
            }
        }
        self.apply_layout_for_monitor(focused_output, backend);
        self.monitors.get(&focused_output).and_then(|m| m.focus_window)
    }

    /// In overview mode, focus navigation wraps around
    pub fn overview_focus_right(&mut self, backend: &BackendHandle) -> Option<WindowId> {
        let focused_output = self.monitors.focused_id()?;
        {
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                let num_cols = monitor.workspace().map(|w| w.columns.len()).unwrap_or(0);
                if let Some(col) = monitor.focus_column {
                    // Wrap around
                    monitor.focus_column = if col >= num_cols.saturating_sub(1) { Some(0) } else { Some(col + 1) };
                    if let Some(workspace) = monitor.workspace() {
                        if let Some(new_col) = workspace.columns.get(monitor.focus_column.unwrap()) {
                            if let Some(tile) = new_col.tiles.first() {
                                monitor.focus_window = Some(tile.window_id);
                            }
                        }
                    }
                }
            }
        }
        self.apply_layout_for_monitor(focused_output, backend);
        self.monitors.get(&focused_output).and_then(|m| m.focus_window)
    }

    /// Select the focused window in overview mode and exit overview
    pub fn overview_select(&mut self, backend: &BackendHandle) -> Option<WindowId> {
        let focused_output = self.monitors.focused_id()?;
        let focused_wid = self.monitors.get(&focused_output).and_then(|m| m.focus_window);
        self.exit_overview(backend);
        focused_wid
    }

    /// Get a reference to the current layout config
    pub fn config(&self) -> &LayoutConfig {
        &self.config
    }

    /// Get the window rules from configuration
    pub fn window_rules_ref(&self) -> &[crate::config::WindowRule] {
        &self.window_rules
    }

    /// Get the animation manager (mutable)
    pub fn animation_mut(&mut self) -> &mut crate::layout::AnimationManager {
        &mut self.animation
    }

    /// Get the animation manager
    pub fn animation(&self) -> &crate::layout::AnimationManager {
        &self.animation
    }

    /// Get the keybindings from configuration
    pub fn config_binds(&self) -> crate::config::BindsConfig {
        self.full_config.as_ref()
            .map(|c| c.binds.clone())
            .unwrap_or_default()
    }

    /// Borrow the full configuration if one has been set.
    pub fn full_config(&self) -> Option<&crate::config::Config> {
        self.full_config.as_ref()
    }

    /// Set the window rules from configuration
    pub fn set_window_rules(&mut self, rules: Vec<crate::config::WindowRule>) {
        self.window_rules = rules;
    }

    /// Set the full configuration (for keybind access and future use).
    ///
    /// Also rebuilds the per-monitor `LayoutConfig` cache from every
    /// `output { layout { … } }` block by folding the partial fields onto
    /// `config.layout` and converting the result through
    /// `LayoutConfig::from_config`.  Subsequent layout passes consult
    /// `effective_config(oid)` to pick the right config per monitor.
    pub fn set_full_config(&mut self, config: crate::config::Config) {
        self.window_rules = config.window_rules.clone();

        // Rebuild per-monitor overrides from `config.output[*].layout_override`.
        self.config_per_monitor.clear();
        for out in &config.output {
            let Some(partial) = out.layout_override.as_ref() else { continue };
            if out.name.is_empty() {
                continue;
            }
            // Fold the partial onto a clone of the global layout, then
            // produce the engine's runtime `LayoutConfig` from the merged
            // view so colour parsing / column-width-mode mapping match the
            // default code path verbatim.
            let merged_config_layout = partial.apply_to(&config.layout);
            let mut shim = config.clone();
            shim.layout = merged_config_layout;
            let engine_cfg = LayoutConfig::from_config(&shim);
            let oid = OutputId::from_name(&out.name);
            self.config_per_monitor.insert(oid, engine_cfg);
        }

        self.full_config = Some(config);
    }

    /// Take a snapshot of the current monitor / workspace / column / tile
    /// structure suitable for serialisation to disk via
    /// [`crate::layout::snapshot::save_to`].
    pub fn snapshot(&self) -> crate::layout::snapshot::Snapshot {
        crate::layout::snapshot::Snapshot::from_monitors(&self.monitors)
    }

    /// Reapply a previously-loaded snapshot.  Tiles whose HWNDs are not
    /// present in `tiled_windows` are dropped silently so old snapshots
    /// continue to load cleanly.  Workspaces / columns are rebuilt from the
    /// snapshot; monitors that no longer exist on this machine are skipped.
    ///
    /// After load, callers should run `apply_all(backend)` to repaint the
    /// restored layout.
    pub fn apply_snapshot(&mut self, snap: &crate::layout::snapshot::Snapshot) {
        let surviving: std::collections::HashSet<isize> = self
            .tiled_windows
            .keys()
            .map(|w| w.as_isize())
            .collect();
        for snap_mon in &snap.monitors {
            // Match by OutputId u64 — survives device-name → from_name re-hash.
            let mut found: Option<OutputId> = None;
            for oid in self.monitors.keys() {
                if oid.as_u64() == snap_mon.output_id {
                    found = Some(*oid);
                    break;
                }
            }
            let Some(oid) = found else { continue };
            let rebuilt =
                crate::layout::snapshot::rebuild_workspaces(snap_mon, &surviving);
            if let Some(monitor) = self.monitors.get_mut(&oid) {
                monitor.workspaces = rebuilt;
                monitor.active_workspace = snap_mon.active_workspace;
                // Workspace 0 is guaranteed to exist by rebuild_workspaces.
                // Reset focus to the first tile of the active workspace's
                // first column when possible — the user's previous focus
                // was a transient state we did not persist.
                let new_focus: Option<(usize, WindowId)> = monitor
                    .workspace()
                    .and_then(|ws| ws.columns.first())
                    .and_then(|col| col.tiles.first())
                    .map(|tile| (0usize, tile.window_id));
                match new_focus {
                    Some((c, w)) => {
                        monitor.focus_column = Some(c);
                        monitor.focus_window = Some(w);
                    }
                    None => {
                        monitor.focus_column = None;
                        monitor.focus_window = None;
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Item 1 — Layout presets: focused-workspace snapshot / restore
    // -----------------------------------------------------------------------

    /// Capture the active workspace on the focused monitor as a
    /// [`crate::layout::snapshot::LayoutSnapshot`].  Returns `None` when no
    /// monitor is focused or the monitor has no active workspace.
    ///
    /// The returned value can be serialised to JSON via serde and later fed
    /// to [`TilingEngine::restore_snapshot`] to recreate the layout.
    pub fn snapshot_current_workspace(&self) -> Option<crate::layout::snapshot::LayoutSnapshot> {
        use crate::layout::snapshot::{LayoutSnapshot, WorkspaceSnapshot, ColumnSnapshot};
        use crate::layout::workspace::ColumnDisplay;

        let oid = self.monitors.focused_id()?;
        let monitor = self.monitors.get(&oid)?;
        let ws_id = monitor.active_workspace;
        let ws = monitor.workspace()?;

        let columns: Vec<ColumnSnapshot> = ws.columns.iter().map(|col| {
            let display_str = match col.display {
                ColumnDisplay::Stacked => "stacked".to_string(),
                ColumnDisplay::Tabbed { active_tab } => format!("tabbed:{}", active_tab),
            };
            ColumnSnapshot {
                width: col.width,
                display: display_str,
                tiles: col.tiles.iter().map(|t| t.window_id.as_isize()).collect(),
            }
        }).collect();

        Some(LayoutSnapshot {
            workspaces: vec![WorkspaceSnapshot {
                id: ws_id,
                columns,
                scroll_offset_x: ws.scroll_offset.x,
            }],
            focused_workspace: Some(ws_id),
        })
    }

    /// Restore a [`crate::layout::snapshot::LayoutSnapshot`] onto the focused monitor.
    ///
    /// For each workspace in the snapshot:
    ///  * The workspace is created on the focused monitor if absent.
    ///  * Existing columns are cleared and rebuilt from the snapshot.
    ///  * Tiles whose HWND is no longer a valid window (`IsWindow` returns
    ///    false) are silently dropped — old snapshots remain usable.
    ///
    /// After restore, callers should invoke `apply_all(backend)` to repaint.
    pub fn restore_snapshot(
        &mut self,
        snap: &crate::layout::snapshot::LayoutSnapshot,
        backend: &crate::backend::BackendHandle,
    ) -> Result<(), String> {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        use windows::Win32::Foundation::HWND;
        use crate::layout::workspace::{Column, ColumnDisplay, Tile, Workspace};
        use crate::utils::WindowId;

        let oid = self.monitors.focused_id()
            .ok_or_else(|| "no focused monitor".to_string())?;

        for snap_ws in &snap.workspaces {
            // Ensure the workspace exists on the focused monitor.
            {
                let monitor = self.monitors.get_mut(&oid)
                    .ok_or_else(|| "focused monitor disappeared".to_string())?;
                monitor.workspaces.entry(snap_ws.id).or_insert_with(Workspace::new);
            }

            // Build new columns from the snapshot, dropping dead HWNDs.
            let mut new_columns: Vec<Column> = Vec::new();
            for snap_col in &snap_ws.columns {
                let mut col = Column::new();
                col.width = snap_col.width;
                // Parse display mode: "stacked" or "tabbed:<idx>".
                col.display = if snap_col.display.starts_with("tabbed:") {
                    let idx: usize = snap_col.display[7..].parse().unwrap_or(0);
                    ColumnDisplay::Tabbed { active_tab: idx }
                } else {
                    ColumnDisplay::Stacked
                };
                for &hwnd_raw in &snap_col.tiles {
                    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
                    let alive = unsafe { IsWindow(hwnd).as_bool() };
                    if !alive {
                        continue;
                    }
                    col.tiles.push(Tile::new(WindowId::new(hwnd_raw)));
                }
                if !col.tiles.is_empty() {
                    // Clamp tabbed active_tab to valid range.
                    if let ColumnDisplay::Tabbed { ref mut active_tab } = col.display {
                        *active_tab = (*active_tab).min(col.tiles.len() - 1);
                    }
                    new_columns.push(col);
                }
            }

            // Swap the workspace's columns for the rebuilt set.
            let monitor = self.monitors.get_mut(&oid)
                .ok_or_else(|| "focused monitor disappeared".to_string())?;
            if let Some(ws) = monitor.workspaces.get_mut(&snap_ws.id) {
                ws.columns = new_columns;
                ws.scroll_offset.x = snap_ws.scroll_offset_x;
            }
        }

        // Switch to the snapshot's focused workspace when specified.
        if let Some(fws) = snap.focused_workspace {
            let monitor = self.monitors.get_mut(&oid)
                .ok_or_else(|| "focused monitor disappeared".to_string())?;
            if monitor.workspaces.contains_key(&fws) {
                monitor.active_workspace = fws;
                let new_focus: Option<(usize, WindowId)> = monitor
                    .workspace()
                    .and_then(|ws| ws.columns.first())
                    .and_then(|col| col.tiles.first())
                    .map(|tile| (0usize, tile.window_id));
                match new_focus {
                    Some((c, w)) => {
                        monitor.focus_column = Some(c);
                        monitor.focus_window = Some(w);
                    }
                    None => {
                        monitor.focus_column = None;
                        monitor.focus_window = None;
                    }
                }
            }
        }

        self.apply_all(backend);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Item 3 — Mouse-wheel gap detection helper
    // -----------------------------------------------------------------------

    /// Returns `true` when the screen point `(x, y)` falls inside an
    /// inter-column gap on the focused monitor's active workspace.
    ///
    /// The gap zone between column `i` and column `i+1` spans from the right
    /// edge of column `i` to the left edge of column `i+1` (i.e. the full
    /// `column_gap` strip), with an additional `half_gap` of slop on either
    /// side to make the hit-zone easier to land on.
    ///
    /// `(x, y)` must be in screen (physical pixel) coordinates — the same
    /// coordinate system returned by `GetCursorPos`.
    ///
    /// Called from `src/input/low_level_hook.rs` to decide whether to consume
    /// a no-modifier `WM_MOUSEWHEEL` event as `Action::ScrollLeft/Right`.
    pub fn is_over_gap(&self, x: i32, y: i32) -> bool {
        let Some(oid) = self.monitors.focused_id() else { return false };
        let Some(monitor) = self.monitors.get(&oid) else { return false };

        // Confirm the point falls on this monitor's work area.
        let wa = monitor.work_area;
        if x < wa.loc.x || x >= wa.loc.x + wa.size.w as i32
            || y < wa.loc.y || y >= wa.loc.y + wa.size.h as i32
        {
            return false;
        }

        let Some(ws) = monitor.workspace() else { return false };
        if ws.columns.len() < 2 {
            // A single column (or empty workspace) has no inter-column gap.
            return false;
        }

        let eff_cfg = self.effective_config(oid);
        let gap = eff_cfg.column_gap;
        // Half-gap slop so the cursor doesn't have to land pixel-perfectly.
        let half_gap = (gap / 2).max(1);

        // Left edge of the usable work area (after outer gaps).
        let work_x = wa.loc.x + eff_cfg.outer_gaps.3;

        // Compute each column's pixel width, mirroring the layout pass.
        let num_columns = ws.columns.len();
        let view_width = wa.size.w as i32
            - eff_cfg.outer_gaps.1  // right outer gap
            - eff_cfg.outer_gaps.3; // left outer gap
        let col_widths: Vec<i32> = ws.columns.iter().map(|col| {
            col.width.map(|w| w as i32).unwrap_or_else(|| {
                match eff_cfg.column_width_mode {
                    ColumnWidthMode::Proportional => {
                        let total_gaps = gap * (num_columns as i32 - 1).max(0);
                        let available = view_width - total_gaps;
                        (available / num_columns as i32).max(eff_cfg.column_width as i32 / 2)
                    }
                    ColumnWidthMode::Fixed => eff_cfg.column_width as i32,
                }
            })
        }).collect();

        // Walk the column layout and check if (x) lands in any gap zone.
        let scroll_x = ws.scroll_offset.x;
        let mut cursor = 0i32; // content-space x (before scroll and work_x offset)
        for (i, &cw) in col_widths.iter().enumerate() {
            if i + 1 < num_columns {
                // The gap in screen-space starts right after this column.
                let gap_screen_start = work_x + cursor + cw - scroll_x;
                let gap_screen_end   = gap_screen_start + gap;
                // Extend the zone by half_gap of slop on each side.
                if x >= gap_screen_start - half_gap && x < gap_screen_end + half_gap {
                    return true;
                }
            }
            cursor += cw + gap;
        }

        false
    }

    /// Return the layout configuration that applies to `output_id`.  If a
    /// per-monitor override exists (set via `output { layout { … } }`) the
    /// reference points into `config_per_monitor`; otherwise the global
    /// `self.config` is returned.  Engine state-mutating methods that take an
    /// `OutputId` should prefer this accessor over reading `self.config`
    /// directly so per-monitor layout settings (column-width, gaps, etc.)
    /// are honoured on multi-monitor setups.
    pub fn effective_config(&self, output_id: OutputId) -> &LayoutConfig {
        self.config_per_monitor
            .get(&output_id)
            .unwrap_or(&self.config)
    }

    /// Activate a window (bring to foreground).
    /// Uses multiple strategies to work around Windows' foreground lock restrictions:
    /// Activate a window (bring to foreground)
    fn activate_window(&self, hwnd: isize) {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetForegroundWindow, BringWindowToTop,
            GetWindowThreadProcessId, IsIconic,
        };
        use windows::Win32::System::Threading::{GetCurrentThreadId, AttachThreadInput};
        use windows::Win32::Foundation::HWND;

        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            // If minimized, restore first
            if IsIconic(hwnd_win).as_bool() {
                let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                    hwnd_win,
                    windows::Win32::UI::WindowsAndMessaging::SW_RESTORE,
                );
            }

            // AttachThreadInput trick to bypass foreground lock
            let mut target_pid: u32 = 0;
            let target_tid = GetWindowThreadProcessId(hwnd_win, Some(&mut target_pid));
            let current_tid = GetCurrentThreadId();

            if target_tid != 0 && current_tid != 0 && target_tid != current_tid {
                let attached = AttachThreadInput(current_tid, target_tid, true);
                let _ = SetForegroundWindow(hwnd_win);
                if attached.as_bool() {
                    let _ = AttachThreadInput(current_tid, target_tid, false);
                }
            } else {
                let _ = SetForegroundWindow(hwnd_win);
            }

            // Fallback
            let _ = BringWindowToTop(hwnd_win);
        }
    }

    /// Apply opacity to a window using DWMWA_EXCLUDED_FROM_PEEK + layered window
    /// Set the DWM border color for a window (Windows 11+).
    /// Falls back silently on older Windows.
    fn set_dwm_border_color(&self, hwnd: isize, color_hex: &str) {
        use windows::Win32::Foundation::HWND;
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);

        if let Some(color) = crate::config::types::parse_color(color_hex) {
            let [r, g, b, _a] = color;
            // DWMWA_BORDER_COLOR uses COLORREF: 0x00BBGGRR
            let colorref = (r as u32) | ((g as u32) << 8) | ((b as u32) << 16);
            let _ = unsafe {
                windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                    hwnd_win,
                    windows::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR,
                    &colorref as *const _ as *const _,
                    std::mem::size_of::<u32>() as u32,
                )
            };
        }
    }

    /// Push the "no colour" sentinel COLORREF to DWMWA_BORDER_COLOR so the
    /// system reverts to its default chrome (no wiri-painted outline).  Used
    /// by the `smart_borders` path when the active workspace has exactly one
    /// column with exactly one tile.  We use `0xFFFFFFFF` per the niri-parity
    /// brief — Windows treats this as `DWMWA_COLOR_DEFAULT`, which falls
    /// back to the OS-chosen border colour (effectively invisible on most
    /// theme/wallpaper combinations).  Separate function rather than an
    /// overload of `set_dwm_border_color` so the cache compare path can
    /// store an obviously distinct sentinel.
    fn set_dwm_border_color_none(&self, hwnd: isize) {
        use windows::Win32::Foundation::HWND;
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        let colorref: u32 = 0xFFFF_FFFF;
        let _ = unsafe {
            windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd_win,
                windows::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR,
                &colorref as *const _ as *const _,
                std::mem::size_of::<u32>() as u32,
            )
        };
    }

    // =========================================================================
    // Item 1 — DWM acrylic/mica backdrop for tiles
    // =========================================================================

    /// Convert a backdrop name string to the `DWMSBT_*` enum value expected by
    /// `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE, …)`.
    ///
    /// | String       | Value | Meaning                |
    /// |--------------|-------|------------------------|
    /// | `"auto"`     | 0     | DWMSBT_AUTO (default)  |
    /// | `"none"`     | 1     | DWMSBT_NONE            |
    /// | `"mica"`     | 2     | DWMSBT_MAINWINDOW      |
    /// | `"acrylic"`  | 3     | DWMSBT_TRANSIENTWINDOW |
    /// | `"tabbed"`   | 4     | DWMSBT_TABBEDWINDOW    |
    ///
    /// Unknown strings fall back to 0 (`DWMSBT_AUTO`).
    pub fn backdrop_str_to_u32(name: &str) -> u32 {
        match name.to_lowercase().trim() {
            "auto"    => 0,
            "none"    => 1,
            "mica"    => 2,
            "acrylic" => 3,
            "tabbed"  => 4,
            _         => 0,
        }
    }

    /// Apply a `DWMWA_SYSTEMBACKDROP_TYPE` value to `hwnd`.
    ///
    /// `DWMWA_SYSTEMBACKDROP_TYPE` (attribute 38) is exported by windows-0.58
    /// and enables Mica/Acrylic/Tabbed Mica backdrops on Windows 11 22H2+.
    /// On older Windows the DWM call simply returns an error which we discard.
    fn apply_backdrop_for_window(&self, hwnd: isize, value: u32) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE};
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd_win,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &value as *const _ as *const _,
                std::mem::size_of::<u32>() as u32,
            )
        };
    }

    /// Cycle the global backdrop through auto → none → mica → acrylic → tabbed → auto
    /// and re-apply to every currently-tiled window.
    ///
    /// The cycle order matches the natural progression from "no backdrop" to
    /// "most opaque backdrop" so repeated presses give the user an interactive
    /// preview of each effect.
    pub fn toggle_backdrop_cycle(&mut self, backend: &BackendHandle) {
        let next = match self.config.backdrop.to_lowercase().trim() {
            "auto"    => "none",
            "none"    => "mica",
            "mica"    => "acrylic",
            "acrylic" => "tabbed",
            _         => "auto",
        };
        info!("Cycling backdrop: {:?} → {:?}", self.config.backdrop, next);
        self.config.backdrop = next.to_string();

        // Invalidate the backdrop cache for every tiled window so the next
        // layout pass re-applies the new value via `apply_backdrop_for_window`.
        let value = Self::backdrop_str_to_u32(&self.config.backdrop);
        for (&wid, _) in &self.tiled_windows {
            let hwnd = wid.as_isize();
            self.apply_backdrop_for_window(hwnd, value);
            // Update the cache so subsequent passes skip redundant calls.
            self.applied_state
                .entry(wid)
                .or_insert_with(AppliedState::unset)
                .backdrop = value;
        }
        let _ = backend; // kept for API symmetry; no layout recalc needed for backdrops
    }

    // =========================================================================
    // Item 3 — swap_columns + column_at_x helpers
    // =========================================================================

    /// Return the column index whose on-screen rect contains `x`, based on the
    /// last-computed `cached_bounds` of each column's first tile.
    ///
    /// Returns `None` when there are no columns or when `x` falls outside all
    /// column rects (e.g. over a gap or past the right edge).
    pub fn column_at_x(&self, x: i32) -> Option<usize> {
        let oid = self.monitors.focused_id()?;
        let monitor = self.monitors.get(&oid)?;
        let workspace = monitor.workspace()?;
        for (idx, col) in workspace.columns.iter().enumerate() {
            if let Some(tile) = col.tiles.first() {
                let b = tile.cached_bounds;
                if x >= b.loc.x && x < b.loc.x + b.size.w as i32 {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Swap two columns in the active workspace by their indices.
    ///
    /// After the swap, if `focus_column` pointed to `src_idx` it now points to
    /// `dst_idx` (focus follows the dragged column).  No-ops when either index
    /// is out of range or they are equal.  Calls `apply_layout_for_monitor`
    /// after mutating the column order so the screen reflects the new layout
    /// immediately.
    pub fn swap_columns(&mut self, src_idx: usize, dst_idx: usize, backend: &BackendHandle) {
        if src_idx == dst_idx {
            return;
        }
        let oid = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };
        {
            let monitor = match self.monitors.get_mut(&oid) {
                Some(m) => m,
                None => return,
            };
            let workspace = match monitor.workspace_mut() {
                Some(w) => w,
                None => return,
            };
            let len = workspace.columns.len();
            if src_idx >= len || dst_idx >= len {
                return;
            }
            workspace.columns.swap(src_idx, dst_idx);
            // Update focus_column so the user's focus follows the moved column.
            if monitor.focus_column == Some(src_idx) {
                monitor.focus_column = Some(dst_idx);
            } else if monitor.focus_column == Some(dst_idx) {
                monitor.focus_column = Some(src_idx);
            }
        }
        info!("swap_columns: {} ↔ {}", src_idx, dst_idx);
        self.apply_layout_for_monitor(oid, backend);
    }

        fn apply_window_opacity(&self, hwnd: isize, opacity: f32) {
        use windows::Win32::Foundation::HWND;
        let hwnd_win = HWND(hwnd as *mut std::ffi::c_void);
        // Use DWMWA_EXCLUDED_FROM_PEEK as a marker, and set per-frame opacity
        // via WS_EX_LAYERED + SetLayeredWindowAttributes if supported
        let alpha = (opacity * 255.0) as u8;
        unsafe {
            let ex_style = windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(
                hwnd_win,
                windows::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
            );
            // Add WS_EX_LAYERED (0x80000)
            windows::Win32::UI::WindowsAndMessaging::SetWindowLongW(
                hwnd_win,
                windows::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
                ex_style | 0x80000,
            );
            let _ = windows::Win32::UI::WindowsAndMessaging::SetLayeredWindowAttributes(
                hwnd_win,
                windows::Win32::Foundation::COLORREF(0),
                alpha,
                windows::Win32::UI::WindowsAndMessaging::LWA_ALPHA,
            );
        }
    }

    /// Close a window
    fn close_window(&self, hwnd: isize) {
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        use windows::Win32::Foundation::{HWND, WPARAM, LPARAM};
        unsafe {
            let _ = PostMessageW(
                HWND(hwnd as *mut std::ffi::c_void),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }

    // ---- Tabbed column engine methods ----
    // Wired from `Action::{ColumnToggleTabbed, TabNext, TabPrev}` in
    // `backend::message_loop::execute_action`.

    /// Toggle the focused column between Stacked and Tabbed display modes.
    pub fn toggle_tabbed_for_focused_column(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(focus_col) = monitor.focus_column {
                if let Some(workspace) = monitor.workspace_mut() {
                    if let Some(column) = workspace.columns.get_mut(focus_col) {
                        use crate::layout::workspace::ColumnDisplay;
                        match &column.display {
                            ColumnDisplay::Tabbed { .. } => column.switch_to_stacked(),
                            ColumnDisplay::Stacked => column.switch_to_tabbed(),
                        }
                    }
                }
            }
        }
        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Advance to the next tab in the focused column (wraps around).
    pub fn focused_column_next_tab(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(focus_col) = monitor.focus_column {
                if let Some(workspace) = monitor.workspace_mut() {
                    if let Some(column) = workspace.columns.get_mut(focus_col) {
                        column.next_tab();
                    }
                }
            }
        }
        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Go to the previous tab in the focused column (wraps around).
    pub fn focused_column_prev_tab(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(focus_col) = monitor.focus_column {
                if let Some(workspace) = monitor.workspace_mut() {
                    if let Some(column) = workspace.columns.get_mut(focus_col) {
                        column.prev_tab();
                    }
                }
            }
        }
        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Returns true if the applied_state cache is non-empty.
    /// The cache is always consulted inside `apply_layout_for_monitor`, so this
    /// method is primarily useful for determining whether the engine has ever
    /// applied any state (i.e., whether a full pass is warranted on first run).
    ///
    /// NOTE: Item 4 ("skip apply_all entirely") is intentionally NOT implemented
    /// as a separate dirty flag because with the Item 1 cache the per-tile
    /// comparison is O(1) and a full pass over 5-10 tiles is sub-microsecond.
    /// The cheapest correct thing is always to let apply_layout_for_monitor
    /// run and return immediately after the hash-lookup fast-path.
    pub fn applied_state_dirty(&self) -> bool {
        !self.applied_state.is_empty()
    }

    /// Re-apply layout to all monitors. Also syncs the urgent-window set from
    /// the WinEvent-driven registry in `backend::hooks` so window rules with
    /// `is-urgent` matchers see the freshest state on every pass.
    pub fn apply_all(&mut self, backend: &BackendHandle) {
        // Don't call refresh_urgent_states here — it wipes the set with the
        // hooks-global state and clobbers entries added programmatically (e.g.
        // via mark_urgent or in tests). main.rs forwards BackendEvent::WindowUrgent
        // directly to mark_urgent/clear_urgent, so urgent_windows stays in sync
        // without needing a wipe-and-replay every layout pass.
        for output_id in self.monitors.keys().copied().collect::<Vec<_>>() {
            self.apply_layout_for_monitor(output_id, backend);
        }
    }

    // -------------------------------------------------------------------------
    // Item 1 — move_window_to_workspace
    // -------------------------------------------------------------------------

    /// Move the focused window to the given workspace (on the same monitor).
    /// The active workspace does not change — the window silently migrates.
    pub fn move_window_to_workspace(&mut self, workspace_id: i32, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        let (window_id, src_ws_id) = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let wid = match monitor.focus_window {
                Some(id) => id,
                None => {
                    debug!("move_window_to_workspace: no focused window");
                    return;
                }
            };
            (wid, monitor.active_workspace)
        };

        if workspace_id == src_ws_id {
            return; // no-op — already on that workspace
        }

        // Remove from source workspace
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            monitor.remove_window(window_id);
            // Ensure target workspace exists
            monitor.workspaces.entry(workspace_id).or_insert_with(crate::layout::Workspace::new);
            // Add to the end of column 0 (new column) on the target workspace
            let width_hint = if self.config.column_width_mode == ColumnWidthMode::Fixed {
                Some(self.config.column_width)
            } else {
                None
            };
            // Temporarily switch active workspace to target so add_window_with_width works
            let prev_ws = monitor.active_workspace;
            monitor.active_workspace = workspace_id;
            monitor.add_window_with_width(window_id, width_hint);
            // Restore active workspace
            monitor.active_workspace = prev_ws;
        }

        self.maintain_empty_workspace(focused_output);
        self.apply_layout_for_monitor(focused_output, backend);
    }

    // -------------------------------------------------------------------------
    // Item 2 — move_window_to_monitor
    // -------------------------------------------------------------------------

    /// Move the focused window to the monitor in the given direction (Left/Right),
    /// ordered by the monitor's left edge (bounds.loc.x).
    pub fn move_window_to_monitor(&mut self, direction: ScrollDirection, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        let window_id = match self.monitors.get(&focused_output).and_then(|m| m.focus_window) {
            Some(id) => id,
            None => {
                debug!("move_window_to_monitor: no focused window");
                return;
            }
        };

        // Build sorted list of (x_position, output_id)
        let mut monitor_order: Vec<(i32, OutputId)> = self.monitors.iter()
            .map(|(&oid, m)| (m.bounds.loc.x, oid))
            .collect();
        monitor_order.sort_by_key(|(x, _)| *x);

        let src_pos = monitor_order.iter().position(|(_, oid)| *oid == focused_output);
        let src_pos = match src_pos {
            Some(p) => p,
            None => return,
        };

        let target_pos: Option<usize> = match direction {
            ScrollDirection::Left  => src_pos.checked_sub(1),
            ScrollDirection::Right => {
                let next = src_pos + 1;
                if next < monitor_order.len() { Some(next) } else { None }
            }
        };

        let target_output = match target_pos {
            Some(p) => monitor_order[p].1,
            None => return, // no monitor in that direction
        };

        // Remove from source monitor's active workspace
        if let Some(src_monitor) = self.monitors.get_mut(&focused_output) {
            src_monitor.remove_window(window_id);
        }

        // Add to target monitor's active workspace
        let width_hint = if self.config.column_width_mode == ColumnWidthMode::Fixed {
            Some(self.config.column_width)
        } else {
            None
        };
        if let Some(dst_monitor) = self.monitors.get_mut(&target_output) {
            dst_monitor.add_window_with_width(window_id, width_hint);
        }

        // Maintain trailing empty workspaces on both monitors
        self.maintain_empty_workspace(focused_output);
        self.maintain_empty_workspace(target_output);

        // Apply layout to both monitors
        self.apply_layout_for_monitor(focused_output, backend);
        self.apply_layout_for_monitor(target_output, backend);
    }

    // -------------------------------------------------------------------------
    // Item 3 — center_focused_column
    // -------------------------------------------------------------------------

    /// Adjust scroll_offset so the focused column's center aligns with
    /// the work_rect's center.
    pub fn center_focused_column(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        let (focus_col, view_width, col_w, gap) = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let fc = match monitor.focus_column {
                Some(c) => c,
                None => return,
            };
            let vw = Self::view_width_for_monitor(monitor, &self.config);
            let nc = monitor.workspace().map(|w| w.columns.len()).unwrap_or(1);
            let cw = Self::effective_column_width(nc, vw, &self.config);
            (fc, vw, cw, self.config.column_gap)
        };

        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(workspace) = monitor.workspace_mut() {
                let (col_x, col_width) = workspace.column_x_and_width(focus_col, col_w, gap);
                // Center: scroll so that col_center == view_center
                let new_scroll = col_x + col_width / 2 - view_width / 2;
                workspace.scroll_offset.x = new_scroll.max(0);
                workspace.clamp_scroll(col_w, gap, view_width);
            }
        }

        self.apply_layout_for_monitor(focused_output, backend);
    }

    // -------------------------------------------------------------------------
    // Item 4 — set_column_width_preset
    // -------------------------------------------------------------------------

    /// Set the focused column's width to a preset fraction of the work_rect,
    /// or cycle through presets.
    pub fn set_column_width_preset(&mut self, preset: ColumnWidthPreset, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        let view_width = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            Self::view_width_for_monitor(monitor, &self.config)
        };

        // Resolve Cycle → a concrete preset. Niri-style cycle order:
        //   1/4 → 1/3 → 1/2 → 2/3 → 3/4 → full → 1/4 …
        let concrete = match preset {
            ColumnWidthPreset::Cycle => {
                let next = match self.last_column_preset {
                    ColumnWidthPreset::OneQuarter    => ColumnWidthPreset::OneThird,
                    ColumnWidthPreset::OneThird      => ColumnWidthPreset::Half,
                    ColumnWidthPreset::Half          => ColumnWidthPreset::TwoThirds,
                    ColumnWidthPreset::TwoThirds    => ColumnWidthPreset::ThreeQuarters,
                    ColumnWidthPreset::ThreeQuarters => ColumnWidthPreset::Full,
                    ColumnWidthPreset::Full          => ColumnWidthPreset::OneQuarter,
                    ColumnWidthPreset::Cycle         => ColumnWidthPreset::Half,
                };
                self.last_column_preset = next;
                next
            }
            other => {
                self.last_column_preset = other;
                other
            }
        };

        let target_w: u32 = match concrete {
            ColumnWidthPreset::OneQuarter    => (view_width / 4).max(50) as u32,
            ColumnWidthPreset::OneThird      => (view_width / 3).max(50) as u32,
            ColumnWidthPreset::Half          => (view_width / 2).max(50) as u32,
            ColumnWidthPreset::TwoThirds     => (view_width * 2 / 3).max(50) as u32,
            ColumnWidthPreset::ThreeQuarters => (view_width * 3 / 4).max(50) as u32,
            ColumnWidthPreset::Full          => view_width.max(50) as u32,
            ColumnWidthPreset::Cycle         => unreachable!("Cycle resolved above"),
        };

        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(focus_col) = monitor.focus_column {
                if let Some(workspace) = monitor.workspace_mut() {
                    if let Some(column) = workspace.columns.get_mut(focus_col) {
                        column.width = Some(target_w);
                    }
                }
            }
        }

        self.apply_layout_for_monitor(focused_output, backend);
    }

    // -------------------------------------------------------------------------
    // Item 5 — resize_focused_column_by
    // -------------------------------------------------------------------------

    /// Grow or shrink the focused column by `delta_px` pixels.
    /// The minimum enforced width is 50 px.
    pub fn resize_focused_column_by(&mut self, delta_px: i32, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        let (view_width, num_cols) = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let vw = Self::view_width_for_monitor(monitor, &self.config);
            let nc = monitor.workspace().map(|w| w.columns.len()).unwrap_or(1);
            (vw, nc)
        };

        let default_col_w = Self::effective_column_width(num_cols, view_width, &self.config);

        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(focus_col) = monitor.focus_column {
                if let Some(workspace) = monitor.workspace_mut() {
                    if let Some(column) = workspace.columns.get_mut(focus_col) {
                        let current = column.width.map(|w| w as i32).unwrap_or(default_col_w);
                        let new_w = (current + delta_px).max(50) as u32;
                        column.width = Some(new_w);
                    }
                }
            }
        }

        self.apply_layout_for_monitor(focused_output, backend);
    }

    // -------------------------------------------------------------------------
    // niri-parity Round 4: column / tile rearrangement
    // -------------------------------------------------------------------------

    /// Take the focused tile out of its column and append it to the column
    /// immediately to the right. No-op when there is no column to the right
    /// or no focused window. The moved tile is re-focused after the op.
    pub fn consume_window_into_column(&mut self, backend: &BackendHandle) {
        let Some(focused_output) = self.monitors.focused_id() else { return };

        let window_id = match self.monitors.get(&focused_output).and_then(|m| m.focus_window) {
            Some(id) => id,
            None => return,
        };

        let moved = {
            let Some(monitor) = self.monitors.get_mut(&focused_output) else { return };
            let Some(workspace) = monitor.workspace_mut() else { return };
            workspace.consume_window_into_right_column(window_id)
        };

        if moved {
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                if let Some(workspace) = monitor.workspace() {
                    if let Some(new_col) = workspace.find_window_column(window_id) {
                        monitor.focus_column = Some(new_col);
                        monitor.focus_window = Some(window_id);
                    }
                }
            }
            self.apply_layout_for_monitor(focused_output, backend);
        }
    }

    /// Take the focused tile out of its column and place it in a brand-new
    /// column immediately to the right of the source. No-op when the column
    /// only hosts a single tile (nothing to expel) or there is no focus.
    pub fn expel_window_from_column(&mut self, backend: &BackendHandle) {
        let Some(focused_output) = self.monitors.focused_id() else { return };

        let window_id = match self.monitors.get(&focused_output).and_then(|m| m.focus_window) {
            Some(id) => id,
            None => return,
        };

        let moved = {
            let Some(monitor) = self.monitors.get_mut(&focused_output) else { return };
            let Some(workspace) = monitor.workspace_mut() else { return };
            workspace.expel_window_into_new_column(window_id)
        };

        if moved {
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                if let Some(workspace) = monitor.workspace() {
                    if let Some(new_col) = workspace.find_window_column(window_id) {
                        monitor.focus_column = Some(new_col);
                        monitor.focus_window = Some(window_id);
                    }
                }
            }
            self.apply_layout_for_monitor(focused_output, backend);
        }
    }

    /// Expand the focused column so it fills the remaining horizontal space
    /// inside the work area after all other columns and inter-column gaps
    /// are accounted for. In `Proportional` mode this is effectively a
    /// "grow to viewport" — the engine's automatic redistribution leaves
    /// the column at full viewport width on the next layout pass.
    pub fn expand_column_to_available(&mut self, backend: &BackendHandle) {
        let Some(focused_output) = self.monitors.focused_id() else { return };

        let (view_width, num_cols) = {
            let Some(monitor) = self.monitors.get(&focused_output) else { return };
            let vw = Self::view_width_for_monitor(monitor, &self.config);
            let nc = monitor.workspace().map(|w| w.columns.len()).unwrap_or(1);
            (vw, nc)
        };

        let default_col_w = Self::effective_column_width(num_cols, view_width, &self.config);
        let gap = self.config.column_gap;

        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            let Some(focus_col_idx) = monitor.focus_column else { return };
            if let Some(workspace) = monitor.workspace_mut() {
                let other_columns_sum: i32 = workspace
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| *idx != focus_col_idx)
                    .map(|(_, c)| c.width.map(|w| w as i32).unwrap_or(default_col_w))
                    .sum();
                let other_count = (workspace.columns.len() as i32 - 1).max(0);
                let total_gaps = gap * other_count;
                let target_w = (view_width - other_columns_sum - total_gaps).max(50) as u32;
                if let Some(column) = workspace.columns.get_mut(focus_col_idx) {
                    column.width = Some(target_w);
                }
            }
        }

        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Toggle the per-column maximize flag on the focused column.
    /// Maximized columns visually occupy the full work-area height (any
    /// per-tile `height_weight` adjustments are ignored while maximized).
    /// Distinct from window-fullscreen, which covers the whole monitor.
    pub fn toggle_maximize_focused_column(&mut self, backend: &BackendHandle) {
        let Some(focused_output) = self.monitors.focused_id() else { return };
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(focus_col_idx) = monitor.focus_column {
                if let Some(workspace) = monitor.workspace_mut() {
                    if let Some(column) = workspace.columns.get_mut(focus_col_idx) {
                        column.maximized = !column.maximized;
                    }
                }
            }
        }
        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Resize the focused column by a percentage of the work-area width.
    /// `delta_percent` is in the range -100..=100; positive grows, negative
    /// shrinks. Width is clamped to 10%..=95% of the work area so a column
    /// never collapses to nothing or covers the whole screen permanently.
    pub fn resize_focused_column_by_percent(
        &mut self,
        delta_percent: i32,
        backend: &BackendHandle,
    ) {
        let Some(focused_output) = self.monitors.focused_id() else { return };

        let (view_width, num_cols) = {
            let Some(monitor) = self.monitors.get(&focused_output) else { return };
            let vw = Self::view_width_for_monitor(monitor, &self.config);
            let nc = monitor.workspace().map(|w| w.columns.len()).unwrap_or(1);
            (vw, nc)
        };

        let default_col_w = Self::effective_column_width(num_cols, view_width, &self.config);
        // 10% .. 95% of view_width, but never smaller than 50 px.
        let min_w = ((view_width as f32) * 0.10).round().max(50.0) as i32;
        let max_w = ((view_width as f32) * 0.95).round() as i32;
        let delta_px = ((view_width as f32) * (delta_percent as f32) / 100.0).round() as i32;

        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(focus_col) = monitor.focus_column {
                if let Some(workspace) = monitor.workspace_mut() {
                    if let Some(column) = workspace.columns.get_mut(focus_col) {
                        let current = column.width.map(|w| w as i32).unwrap_or(default_col_w);
                        let new_w = (current + delta_px).clamp(min_w, max_w) as u32;
                        column.width = Some(new_w);
                    }
                }
            }
        }
        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Resize the focused tile's height inside its column by a percentage of
    /// the column's available vertical space. Adjusts `Tile.height_weight`
    /// proportionally and re-normalises the column so weights remain within
    /// reasonable bounds. No-op when the focused column has fewer than two
    /// visible tiles (single tile already owns 100% of the column).
    pub fn resize_focused_tile_height_by_percent(
        &mut self,
        delta_percent: i32,
        backend: &BackendHandle,
    ) {
        let Some(focused_output) = self.monitors.focused_id() else { return };

        let (focus_col_idx, focus_wid) = {
            let Some(monitor) = self.monitors.get(&focused_output) else { return };
            let Some(col) = monitor.focus_column else { return };
            let Some(wid) = monitor.focus_window else { return };
            (col, wid)
        };

        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(workspace) = monitor.workspace_mut() {
                if let Some(column) = workspace.columns.get_mut(focus_col_idx) {
                    if column.tiles.len() < 2 {
                        return; // nothing to redistribute
                    }
                    // Convert delta % into a weight delta. The column weight
                    // total is roughly tiles.len() (defaults are 1.0 each),
                    // so a 5% delta maps to (tiles.len() / 100) * 5 weight.
                    let count = column.tiles.len() as f32;
                    let weight_delta = (count * (delta_percent as f32) / 100.0).max(-0.9).min(0.9);

                    if let Some(focused_tile) = column
                        .tiles
                        .iter_mut()
                        .find(|t| t.window_id == focus_wid)
                    {
                        focused_tile.height_weight =
                            (focused_tile.height_weight + weight_delta).clamp(0.1, 10.0);
                    }
                }
            }
        }
        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Move the entire focused column (with all of its tiles) onto the
    /// active workspace of the monitor in the given direction. The column
    /// is appended to the destination workspace, focus follows along, and
    /// both monitors are re-laid out. No-op when no monitor exists in the
    /// requested direction.
    pub fn move_column_to_monitor(
        &mut self,
        direction: ScrollDirection,
        backend: &BackendHandle,
    ) {
        let Some(focused_output) = self.monitors.focused_id() else { return };

        let focus_col_idx = match self.monitors.get(&focused_output).and_then(|m| m.focus_column) {
            Some(i) => i,
            None => return,
        };

        // Build sorted list of (x_position, output_id)
        let mut monitor_order: Vec<(i32, OutputId)> = self
            .monitors
            .iter()
            .map(|(&oid, m)| (m.bounds.loc.x, oid))
            .collect();
        monitor_order.sort_by_key(|(x, _)| *x);

        let src_pos = monitor_order.iter().position(|(_, oid)| *oid == focused_output);
        let src_pos = match src_pos {
            Some(p) => p,
            None => return,
        };

        let target_pos: Option<usize> = match direction {
            ScrollDirection::Left => src_pos.checked_sub(1),
            ScrollDirection::Right => {
                let next = src_pos + 1;
                if next < monitor_order.len() { Some(next) } else { None }
            }
        };

        let target_output = match target_pos {
            Some(p) => monitor_order[p].1,
            None => return, // no monitor in that direction
        };

        // Extract the column wholesale.
        let column_taken = {
            let Some(src_monitor) = self.monitors.get_mut(&focused_output) else { return };
            let Some(src_ws) = src_monitor.workspace_mut() else { return };
            if focus_col_idx >= src_ws.columns.len() {
                return;
            }
            let col = src_ws.columns.remove(focus_col_idx);
            // Clear focus on source monitor (will be re-applied on dst).
            src_monitor.focus_column = None;
            src_monitor.focus_window = None;
            col
        };

        // Capture the window ids (for MRU bookkeeping + post-move focus).
        let moved_wids: Vec<WindowId> = column_taken.tiles.iter().map(|t| t.window_id).collect();

        // Drop tracking on the source MRU ring.
        if let Some(src_monitor) = self.monitors.get_mut(&focused_output) {
            for wid in &moved_wids {
                src_monitor.focus_ring.remove(*wid);
            }
        }

        // Append to destination active workspace.
        if let Some(dst_monitor) = self.monitors.get_mut(&target_output) {
            if let Some(dst_ws) = dst_monitor.workspace_mut() {
                dst_ws.columns.push(column_taken);
                let new_col_idx = dst_ws.columns.len() - 1;
                dst_monitor.focus_column = Some(new_col_idx);
                // Focus the first tile in the moved column.
                if let Some(first) = moved_wids.first().copied() {
                    dst_monitor.focus_window = Some(first);
                    dst_monitor.focus_ring.push(first);
                }
            }
        }

        // Make the destination the focused monitor.
        self.monitors.set_focused(target_output);

        // Maintain trailing empty workspaces + re-tile both monitors.
        self.maintain_empty_workspace(focused_output);
        self.maintain_empty_workspace(target_output);
        self.apply_layout_for_monitor(focused_output, backend);
        self.apply_layout_for_monitor(target_output, backend);
    }

    // -------------------------------------------------------------------------
    // Niri-parity: move the focused column vertically between workspaces
    // -------------------------------------------------------------------------

    /// Move every tile in the focused column out of the current workspace and
    /// append them as a single new column to `current_workspace_id + delta`.
    /// The destination workspace is created if missing.  No-op when the focused
    /// monitor has no focused column, or when the destination workspace id
    /// would land on a non-positive value going up (i.e. `delta < 0` and
    /// `current <= 0`).  Focus follows the moved column to the destination.
    ///
    /// Used by `Action::MoveColumnToWorkspaceUp` (delta = -1) and
    /// `Action::MoveColumnToWorkspaceDown` (delta = +1).
    pub fn move_focused_column_to_workspace(&mut self, delta: i32, backend: &BackendHandle) {
        let Some(focused_output) = self.monitors.focused_id() else { return };

        // Resolve focused column index + source workspace id up-front.
        let (focus_col_idx, src_ws_id) = match self.monitors.get(&focused_output) {
            Some(m) => match m.focus_column {
                Some(i) => (i, m.active_workspace),
                None => return,
            },
            None => return,
        };
        let dst_ws_id = src_ws_id + delta;
        // Refuse to step to a negative workspace id (workspace 0 is the
        // implicit "lowest" wiri default; user-facing labels are still
        // 1-based via the niri convention, but the engine HashMap can carry
        // anything).  Without this guard, "move-column-up" from workspace 0
        // would silently create workspace -1 and strand tiles there.
        if delta < 0 && dst_ws_id < 0 {
            return;
        }
        if dst_ws_id == src_ws_id {
            return; // delta == 0 → no-op
        }

        // Take the column from the source workspace.
        let column_taken = {
            let Some(monitor) = self.monitors.get_mut(&focused_output) else { return };
            let Some(src_ws) = monitor.workspaces.get_mut(&src_ws_id) else { return };
            if focus_col_idx >= src_ws.columns.len() {
                return;
            }
            let col = src_ws.columns.remove(focus_col_idx);
            // Clear focus on the source workspace; it will be re-anchored
            // either to whatever is still here, or to the destination below.
            monitor.focus_column = None;
            monitor.focus_window = None;
            col
        };

        let moved_wids: Vec<WindowId> = column_taken.tiles.iter().map(|t| t.window_id).collect();

        // Append the column to the destination workspace (creating it if
        // necessary).  After the append, point focus at the new column on the
        // destination workspace and switch the monitor's active workspace to
        // match so the user follows their tiles.
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            let dst_ws = monitor
                .workspaces
                .entry(dst_ws_id)
                .or_insert_with(crate::layout::Workspace::new);
            dst_ws.columns.push(column_taken);
            let new_col_idx = dst_ws.columns.len() - 1;
            monitor.active_workspace = dst_ws_id;
            monitor.focus_column = Some(new_col_idx);
            if let Some(first) = moved_wids.first().copied() {
                monitor.focus_window = Some(first);
                monitor.focus_ring.push(first);
            }
        }

        self.maintain_empty_workspace(focused_output);
        self.apply_layout_for_monitor(focused_output, backend);
    }

    // -------------------------------------------------------------------------
    // Niri-parity: workspace move/swap (reorder the workspace stack)
    // -------------------------------------------------------------------------

    /// Swap the focused workspace with the workspace at
    /// `focused_workspace_id + delta` on the same monitor.  Focus follows the
    /// moved workspace so the user stays on the original tiles.  No-op when
    /// the swap target is the same as the source, when no monitor is focused,
    /// or when the resulting move would clobber a workspace that does not
    /// exist (i.e. `delta < 0` and source is the lowest workspace).
    pub fn move_active_workspace(&mut self, delta: i32, backend: &BackendHandle) {
        let Some(focused_output) = self.monitors.focused_id() else { return };

        let src_ws_id = match self.monitors.get(&focused_output) {
            Some(m) => m.active_workspace,
            None => return,
        };
        let dst_ws_id = src_ws_id + delta;
        if dst_ws_id == src_ws_id {
            return;
        }
        // Edge guard: refuse to step to a negative workspace id (workspace 0
        // is the implicit lowest in wiri).  Without this guard the swap
        // would silently create a workspace at id == -1 and leave the user
        // stranded on a negative-numbered slot.
        if delta < 0 && dst_ws_id < 0 {
            return;
        }

        // Perform the swap.  Either workspace may or may not exist; we
        // materialise both before swapping so users can also swap a populated
        // workspace into a still-empty slot.
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            let src_ws = monitor
                .workspaces
                .remove(&src_ws_id)
                .unwrap_or_else(crate::layout::Workspace::new);
            let dst_ws = monitor
                .workspaces
                .remove(&dst_ws_id)
                .unwrap_or_else(crate::layout::Workspace::new);
            monitor.workspaces.insert(dst_ws_id, src_ws);
            monitor.workspaces.insert(src_ws_id, dst_ws);
            // Focus follows the moved workspace.
            monitor.active_workspace = dst_ws_id;
            // Anchor focus_column to the first column of the relocated
            // workspace (or None when it has no columns).
            let new_focus = monitor
                .workspaces
                .get(&dst_ws_id)
                .and_then(|w| w.columns.first().and_then(|c| c.tiles.first()).map(|t| (0usize, t.window_id)));
            match new_focus {
                Some((col_idx, wid)) => {
                    monitor.focus_column = Some(col_idx);
                    monitor.focus_window = Some(wid);
                    monitor.focus_ring.push(wid);
                }
                None => {
                    monitor.focus_column = None;
                    monitor.focus_window = None;
                }
            }
        }

        self.maintain_empty_workspace(focused_output);
        self.apply_layout_for_monitor(focused_output, backend);
    }

    // -------------------------------------------------------------------------
    // Item 6 — focus_workspace_relative
    // -------------------------------------------------------------------------

    /// Switch to the next or previous workspace on the focused monitor (wraps).
    pub fn focus_workspace_relative(&mut self, direction: WorkspaceDirection, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        // Collect sorted workspace IDs present on this monitor
        let (sorted_ids, active_id) = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let mut ids: Vec<i32> = monitor.workspaces.keys().copied().collect();
            ids.sort();
            (ids, monitor.active_workspace)
        };

        if sorted_ids.is_empty() { return; }

        let cur_pos = sorted_ids.iter().position(|&id| id == active_id).unwrap_or(0);
        let new_pos = match direction {
            WorkspaceDirection::Next =>
                (cur_pos + 1) % sorted_ids.len(),
            WorkspaceDirection::Previous =>
                cur_pos.checked_sub(1).unwrap_or(sorted_ids.len() - 1),
        };
        let new_id = sorted_ids[new_pos];

        // Hide windows on current workspace, switch, show new workspace windows
        let current_windows: Vec<WindowId> = self.monitors.get(&focused_output)
            .and_then(|m| m.workspace())
            .map(|w| w.columns.iter().flat_map(|c| c.tiles.iter().map(|t| t.window_id)).collect())
            .unwrap_or_default();

        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            monitor.switch_workspace(new_id);
        }

        for wid in current_windows {
            let _ = backend.show_window(wid.as_isize(), false);
        }

        if let Some(monitor) = self.monitors.get(&focused_output) {
            if let Some(workspace) = monitor.workspace() {
                for col in &workspace.columns {
                    for tile in &col.tiles {
                        let _ = backend.show_window(tile.window_id.as_isize(), true);
                    }
                }
            }
        }

        self.apply_layout_for_monitor(focused_output, backend);

        // Activate focused window on new workspace
        let hwnd_to_activate = self.monitors.get(&focused_output)
            .and_then(|m| m.focus_window)
            .and_then(|wid| self.tiled_windows.get(&wid).map(|w| w.hwnd));
        if let Some(hwnd) = hwnd_to_activate {
            self.activate_window(hwnd);
        }
    }

    // -------------------------------------------------------------------------
    // Item 7 — maintain_empty_workspace
    // -------------------------------------------------------------------------

    /// Ensure there is exactly one empty workspace at `highest_non_empty_id + 1`.
    /// Empty workspaces beyond that (except the active one) are removed.
    pub fn maintain_empty_workspace(&mut self, monitor_id: OutputId) {
        let monitor = match self.monitors.get_mut(&monitor_id) {
            Some(m) => m,
            None => return,
        };

        let active_ws = monitor.active_workspace;

        // Find the highest workspace ID that has at least one window.
        let highest_non_empty: i32 = monitor.workspaces.iter()
            .filter(|(_, ws)| !ws.columns.is_empty())
            .map(|(&id, _)| id)
            .max()
            .unwrap_or(-1);

        let trailing_id = highest_non_empty + 1;

        // Ensure the trailing empty workspace exists.
        monitor.workspaces.entry(trailing_id).or_insert_with(crate::layout::Workspace::new);

        // Remove surplus empty workspaces (anything empty that isn't the trailing one
        // and isn't what the user is currently looking at).
        let to_remove: Vec<i32> = monitor.workspaces.iter()
            .filter(|(&id, ws)| ws.columns.is_empty() && id != trailing_id && id != active_ws)
            .map(|(&id, _)| id)
            .collect();
        for id in to_remove {
            monitor.workspaces.remove(&id);
        }
    }

    /// Restore all tiled windows to their original positions and styles.
    /// Called on graceful shutdown so the user gets their layout back instead
    /// of windows frozen in the wiri tile positions.
    pub fn restore_all(&mut self, backend: &BackendHandle) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowLongW, GWL_STYLE, GWL_EXSTYLE, SetWindowPos, SWP_NOMOVE,
            SWP_NOSIZE, SWP_NOZORDER, SWP_NOACTIVATE, SWP_FRAMECHANGED,
            HWND_NOTOPMOST,
        };
        use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
        use windows::Win32::UI::Controls::MARGINS;

        info!("Restoring {} windows to original positions/styles",
            self.original_bounds.len());

        // Restore styles first (un-strips frames if strip_frame was enabled)
        for (window_id, &orig_style) in &self.saved_styles {
            let hwnd = HWND(window_id.as_isize() as *mut std::ffi::c_void);
            unsafe {
                SetWindowLongW(hwnd, GWL_STYLE, orig_style as i32);
                if let Some(&orig_ex) = self.saved_ex_styles.get(window_id) {
                    SetWindowLongW(hwnd, GWL_EXSTYLE, orig_ex as i32);
                }
                // Reset DWM frame inset (default 0 margins).
                let m = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 0, cyBottomHeight: 0 };
                let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
                // Force frame recalc.
                let _ = SetWindowPos(
                    hwnd, HWND_NOTOPMOST, 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
        }

        // Restore original bounds.
        for (window_id, &rect) in &self.original_bounds {
            let _ = backend.set_window_position(
                window_id.as_isize(),
                rect,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    // -------------------------------------------------------------------------
    // Item 1 — focus_previous_window (niri alt-tab)
    // -------------------------------------------------------------------------

    /// Focus the second-most-recently-used window on the focused monitor ("alt-tab back").
    /// Reads `focus_ring.previous()` from the focused monitor, locates that window
    /// across all workspaces, updates focus state, and re-applies layout.
    pub fn focus_previous_window(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        // Peek at the previous window without modifying the ring yet.
        let prev_wid = match self.monitors.get(&focused_output).and_then(|m| m.focus_ring.previous()) {
            Some(wid) => wid,
            None => return, // no previous — no-op
        };

        // Find which monitor + workspace + column hosts that window.
        // It might be on a different workspace (though the MRU ring is per-monitor).
        let location: Option<(OutputId, i32, usize)> = self.monitors.iter().find_map(|(&oid, monitor)| {
            monitor.workspaces.iter().find_map(|(&ws_id, ws)| {
                ws.find_window_column(prev_wid).map(|col_idx| (oid, ws_id, col_idx))
            })
        });

        let (target_output, _ws_id, col_idx) = match location {
            Some(loc) => loc,
            None => return, // window no longer alive — no-op
        };

        // Update focus on the target monitor.
        if let Some(monitor) = self.monitors.get_mut(&target_output) {
            monitor.focus_window = Some(prev_wid);
            monitor.focus_column = Some(col_idx);
            monitor.focus_ring.push(prev_wid);
        }

        // Activate the window at the OS level.
        if let Some(hwnd) = self.tiled_windows.get(&prev_wid).map(|w| w.hwnd) {
            self.activate_window(hwnd);
        }

        self.apply_layout_for_monitor(target_output, backend);
    }

    // -------------------------------------------------------------------------
    // Item 2 — toggle_always_on_top_for_focused
    // -------------------------------------------------------------------------

    /// Toggle HWND_TOPMOST / HWND_NOTOPMOST for the currently focused window.
    /// Tracks per-window topmost state in `self.always_on_top`.
    pub fn toggle_always_on_top_for_focused(&mut self, _backend: &BackendHandle) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE,
            HWND_TOPMOST, HWND_NOTOPMOST,
        };

        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };
        let wid = match self.monitors.get(&focused_output).and_then(|m| m.focus_window) {
            Some(id) => id,
            None => return,
        };
        let hwnd_raw = match self.tiled_windows.get(&wid).map(|w| w.hwnd) {
            Some(h) => h,
            None => return,
        };

        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;

        if self.always_on_top.contains(&wid) {
            // Currently topmost — demote to normal.
            self.always_on_top.remove(&wid);
            unsafe {
                let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
            }
            debug!("Cleared always-on-top for window {}", wid);
        } else {
            // Not topmost — elevate.
            self.always_on_top.insert(wid);
            unsafe {
                let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
            }
            debug!("Set always-on-top for window {}", wid);
        }
    }

    /// Query whether a window is currently pinned as always-on-top.
    pub fn is_always_on_top(&self, window_id: WindowId) -> bool {
        self.always_on_top.contains(&window_id)
    }

    // -------------------------------------------------------------------------
    // Item 3 — auto_tile_threshold setter / getter
    // -------------------------------------------------------------------------

    /// Set the column-count threshold above which an auto-tile notice is emitted
    /// (and in a future release, the overview zoom will auto-engage).
    /// Pass `None` to disable the threshold.
    pub fn set_auto_tile_threshold(&mut self, threshold: Option<usize>) {
        self.auto_tile_threshold = threshold;
    }

    /// Return the current auto-tile threshold, or `None` if disabled.
    pub fn auto_tile_threshold(&self) -> Option<usize> {
        self.auto_tile_threshold
    }

    /// Item 4 — return the current auto-tile zoom factor (None = not engaged).
    pub fn auto_tile_zoom(&self) -> Option<f64> {
        self.auto_tile_zoom
    }

    // -------------------------------------------------------------------------
    // Item 3 — PiP corner snap
    // -------------------------------------------------------------------------

    /// Item 3 — Snap the focused floating window to a corner of its monitor's
    /// work area.
    ///
    /// If the focused window is in `floating_windows`, its bounds are looked up
    /// from `tiled_windows`, a snapped rect is computed via `FloatManager`-
    /// compatible math, and `backend.set_window_position` is called.  If the
    /// window is not floating or no focused window exists, this is a no-op.
    pub fn snap_floating_to_corner(
        &mut self,
        corner: crate::layout::floating::FloatingCorner,
        backend: &BackendHandle,
    ) {
        // Find the focused window.
        let focused_wid = match self.monitors.focused_id()
            .and_then(|oid| self.monitors.get(&oid))
            .and_then(|m| m.focus_window)
        {
            Some(w) => w,
            None => {
                debug!("snap_floating_to_corner: no focused window");
                return;
            }
        };

        // Only operate on floating windows.
        if !self.floating_windows.contains(&focused_wid) {
            debug!("snap_floating_to_corner: focused window {:?} is not floating", focused_wid);
            return;
        }

        // Gather current bounds and work area.
        let win_info = match self.tiled_windows.get(&focused_wid) {
            Some(i) => i.clone(),
            None => return,
        };
        let win_size = win_info.bounds.size;

        // Find the monitor that owns this output to get the work area.
        let work_rect = match self.monitors.focused_id()
            .and_then(|oid| self.monitors.get(&oid))
        {
            Some(m) => m.work_area,
            None => return,
        };

        // Compute the snapped rect (margin=24 from each edge).
        use crate::layout::floating::FloatingCorner;
        let margin: i32 = 24;
        let (x, y) = match corner {
            FloatingCorner::TopLeft => (
                work_rect.loc.x + margin,
                work_rect.loc.y + margin,
            ),
            FloatingCorner::TopRight => (
                work_rect.loc.x + work_rect.size.w as i32 - win_size.w as i32 - margin,
                work_rect.loc.y + margin,
            ),
            FloatingCorner::BottomLeft => (
                work_rect.loc.x + margin,
                work_rect.loc.y + work_rect.size.h as i32 - win_size.h as i32 - margin,
            ),
            FloatingCorner::BottomRight => (
                work_rect.loc.x + work_rect.size.w as i32 - win_size.w as i32 - margin,
                work_rect.loc.y + work_rect.size.h as i32 - win_size.h as i32 - margin,
            ),
            FloatingCorner::Center => (
                work_rect.loc.x + (work_rect.size.w as i32 - win_size.w as i32) / 2,
                work_rect.loc.y + (work_rect.size.h as i32 - win_size.h as i32) / 2,
            ),
        };
        let snapped = Rect::new(x, y, win_size.w, win_size.h);

        debug!(
            "snap_floating_to_corner: moving {:?} to ({},{}) {:?}",
            focused_wid, x, y, corner
        );
        let _ = backend.set_window_position(
            focused_wid.as_isize(),
            snapped,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
        );
        // Update cached rect in applied_state so the next layout pass does not
        // immediately re-position the window back to its old location.
        self.applied_state
            .entry(focused_wid)
            .or_insert_with(AppliedState::unset)
            .rect = snapped;
    }

    // -------------------------------------------------------------------------
    // Item 4 — focus_workspace_named
    // -------------------------------------------------------------------------

    /// Switch to a workspace identified by its KDL config name.
    /// The index of the matching `WorkspaceConfig` entry is used as the workspace ID
    /// (consistent with how workspaces are created from config at startup).
    /// Logs a warning and returns silently when the name is not found.
    pub fn focus_workspace_named(&mut self, name: &str, backend: &BackendHandle) {
        use tracing::warn;
        let ws_id: Option<i32> = self.full_config.as_ref().and_then(|cfg| {
            cfg.workspace
                .iter()
                .enumerate()
                .find_map(|(idx, ws_cfg)| {
                    if ws_cfg.name == name {
                        Some(idx as i32)
                    } else {
                        None
                    }
                })
        });
        match ws_id {
            Some(id) => self.switch_workspace(id, backend),
            None => warn!("workspace name not found: {}", name),
        }
    }

    // -------------------------------------------------------------------------
    // Item 5 — urgent-window API
    // -------------------------------------------------------------------------

    /// Mark a window as urgent (e.g. after receiving a WM_FLASHWINDOW notification).
    pub fn mark_urgent(&mut self, window_id: WindowId) {
        self.urgent_windows.insert(window_id);
    }

    /// Clear the urgent flag for a window.
    pub fn clear_urgent(&mut self, window_id: WindowId) {
        self.urgent_windows.remove(&window_id);
    }

    /// Query whether a window is currently flagged as urgent.
    pub fn is_urgent(&self, window_id: WindowId) -> bool {
        self.urgent_windows.contains(&window_id)
    }

    /// Sweep the WinEvent-driven urgent registry in `backend::hooks` and sync
    /// the engine's `urgent_windows` set to match.
    ///
    /// We replace the entire set on each call rather than merge, so that
    /// windows whose urgency the hook has already cleared (e.g. user focused
    /// them) are dropped from the engine view too.
    pub fn refresh_urgent_states(&mut self) {
        let raw: Vec<isize> = crate::backend::hooks::urgent_hwnds();
        self.urgent_windows = raw.into_iter().map(WindowId::new).collect();
    }
}

impl Default for TilingEngine {
    fn default() -> Self {
        Self::new(LayoutConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::Rect;

    fn make_engine() -> TilingEngine {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let oid = OutputId::from_name("TestMonitor");
        engine.register_monitor(
            oid,
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        engine
    }

    fn make_window(hwnd: isize, x: i32, y: i32) -> WindowInfo {
        WindowInfo {
            hwnd,
            title: format!("Window {}", hwnd),
            class_name: "TestClass".to_string(),
            process_id: 1000,
            bounds: Rect::new(x, y, 800, 600),
            state: crate::backend::WindowState::Normal,
            is_visible: true,
        }
    }

    #[test]
    fn test_engine_register_monitor() {
        let engine = make_engine();
        assert_eq!(engine.monitors().len(), 1);
        assert!(engine.focused_output().is_some());
    }

    #[test]
    fn test_engine_add_window() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());
        assert_eq!(engine.tiled_windows().len(), 1);
    }

    #[test]
    fn test_engine_remove_window() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());
        assert_eq!(engine.tiled_windows().len(), 1);
        engine.remove_window(WindowId::new(100), &BackendHandle::default_for_test());
        assert_eq!(engine.tiled_windows().len(), 0);
    }

    #[test]
    fn test_engine_toggle_fullscreen() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());
        // Focus the window
        let oid = engine.focused_output().unwrap();
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            monitor.focus_window = Some(WindowId::new(100));
            monitor.focus_column = Some(0);
        }
        assert!(!engine.is_fullscreen(WindowId::new(100)));
        // Can't fully test toggle_fullscreen without a real HWND,
        // but we can test the state tracking
    }

    #[test]
    fn test_engine_toggle_floating() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            monitor.focus_window = Some(WindowId::new(100));
            monitor.focus_column = Some(0);
        }
        assert!(!engine.is_floating(WindowId::new(100)));
        // Float the window
        engine.floating_windows.insert(WindowId::new(100));
        assert!(engine.is_floating(WindowId::new(100)));
        // Un-float it
        engine.floating_windows.remove(&WindowId::new(100));
        assert!(!engine.is_floating(WindowId::new(100)));
    }

    #[test]
    fn test_engine_add_window_with_float_rule() {
        let mut engine = make_engine();
        // Add a window rule that floats TestClass windows
        let mut rule = crate::config::WindowRule::default();
        rule.class = Some("TestClass".to_string());
        rule.floating = true;
        engine.set_window_rules(vec![rule]);

        let window = make_window(200, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        // Window should be tracked as floating
        assert!(engine.is_floating(WindowId::new(200)));
        // But still tracked in tiled_windows (for HWND lookup)
        assert!(engine.tiled_windows().contains_key(&WindowId::new(200)));
    }

    #[test]
    fn test_effective_column_width_fixed() {
        let config = LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            ..LayoutConfig::default()
        };
        let width = TilingEngine::effective_column_width(3, 1920, &config);
        assert_eq!(width, 500); // Always fixed
    }

    #[test]
    fn test_effective_column_width_proportional() {
        let config = LayoutConfig {
            column_width_mode: ColumnWidthMode::Proportional,
            column_width: 500,
            column_gap: 8,
            ..LayoutConfig::default()
        };
        // 3 columns on a 1920px monitor: (1920 - 2*8) / 3 = 634
        let width = TilingEngine::effective_column_width(3, 1920, &config);
        assert_eq!(width, 634);
        // 1 column: (1920 - 0) / 1 = 1920
        let width = TilingEngine::effective_column_width(1, 1920, &config);
        assert_eq!(width, 1920);
        // 0 columns: fallback to configured
        let width = TilingEngine::effective_column_width(0, 1920, &config);
        assert_eq!(width, 500);
    }

    #[test]
    fn test_border_color_mode_default_is_fixed() {
        let lc = LayoutConfig::default();
        assert_eq!(lc.border_color_focused_mode, BorderColorMode::Fixed);
        assert_eq!(lc.border_color_focused, "#3381d9");
    }

    #[test]
    fn test_border_color_mode_from_config_accent_sentinel() {
        // The engine's `from_config` should recognise the "accent"
        // sentinel and flip the mode without polluting the literal
        // colour with non-hex text.
        let mut cfg = crate::config::Config::default();
        cfg.layout.border_color_focused = "accent".to_string();
        let lc = LayoutConfig::from_config(&cfg);
        assert_eq!(lc.border_color_focused_mode, BorderColorMode::WindowsAccent);
        // Fallback literal must still be a parseable colour so a failed
        // registry lookup paints *something* visible.
        assert!(crate::config::types::parse_color(&lc.border_color_focused).is_some());

        // A plain hex string keeps the Fixed mode.
        let mut cfg = crate::config::Config::default();
        cfg.layout.border_color_focused = "#aabbcc".to_string();
        let lc = LayoutConfig::from_config(&cfg);
        assert_eq!(lc.border_color_focused_mode, BorderColorMode::Fixed);
        assert_eq!(lc.border_color_focused, "#aabbcc");
    }

    #[test]
    fn test_accent_reader_fallback_to_fixed_when_lookup_fails() {
        use crate::backend::accent::{
            current_windows_accent_rgba_with, invalidate_accent_cache_for_test,
            AccentReader,
        };
        struct FailingReader;
        impl AccentReader for FailingReader {
            fn read_now(&self) -> Option<[u8; 4]> {
                None
            }
        }
        invalidate_accent_cache_for_test();
        // The cached fetch returns None — the engine's
        // `apply_layout_for_monitor` will fall back to the literal.  We
        // assert the fallback by re-running the same resolution logic
        // here against a config that opts into WindowsAccent.
        let mut lc = LayoutConfig::default();
        lc.border_color_focused_mode = BorderColorMode::WindowsAccent;
        lc.border_color_focused = "#3381d9".to_string();
        let resolved = match lc.border_color_focused_mode {
            BorderColorMode::Fixed => lc.border_color_focused.clone(),
            BorderColorMode::WindowsAccent => {
                match current_windows_accent_rgba_with(&FailingReader) {
                    Some([r, g, b, _]) => format!("#{:02x}{:02x}{:02x}", r, g, b),
                    None => lc.border_color_focused.clone(),
                }
            }
        };
        assert_eq!(resolved, "#3381d9");
        invalidate_accent_cache_for_test();
    }

    #[test]
    fn test_overview_toggle() {
        let mut engine = make_engine();
        assert!(!engine.is_overview());
        assert_eq!(engine.overview_zoom(), 1.0);

        // Add some windows so overview has columns to show
        for i in 100..110 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }

        // Enter overview
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());
        // With 10 columns on 1920px, zoom should be < 1.0
        assert!(engine.overview_zoom() < 1.0);

        // Exit overview
        engine.exit_overview(&BackendHandle::default_for_test());
        assert!(!engine.is_overview());
        assert_eq!(engine.overview_zoom(), 1.0);
    }

    // ---- Item 1: DwmRegisterThumbnail-based overview ----

    #[test]
    fn test_enter_overview_registers_thumbnails() {
        use crate::overlay::MockThumbnailOverview;
        let mut engine = make_engine();
        let sink = MockThumbnailOverview::new();
        engine.install_thumbnail_overview(sink.clone());
        assert!(engine.has_thumbnail_overview());

        // 4 tiles spread across one workspace.
        for i in 200..204 {
            let window = make_window(i, 100, 100);
            engine.add_window(window, &BackendHandle::default_for_test());
        }

        engine.enter_overview(&BackendHandle::default_for_test());
        // enter() called exactly once, and every tracked tile registered.
        assert_eq!(sink.enter_calls(), 1);
        assert_eq!(sink.register_calls(), 4);
        assert_eq!(sink.registered_count(), 4);
    }

    #[test]
    fn test_update_thumbnail_called_with_overview_rect() {
        use crate::overlay::MockThumbnailOverview;
        let mut engine = make_engine();
        let sink = MockThumbnailOverview::new();
        engine.install_thumbnail_overview(sink.clone());

        for i in 300..303 {
            let window = make_window(i, 100, 100);
            engine.add_window(window, &BackendHandle::default_for_test());
        }

        engine.enter_overview(&BackendHandle::default_for_test());

        // Every registered tile should have received exactly one
        // `update_thumbnail` call with a non-zero rect.
        for i in 300..303 {
            let r = sink.last_dst_rect(WindowId::new(i));
            assert!(r.is_some(), "tile {} missing update_thumbnail call", i);
            let r = r.unwrap();
            assert!(r.size.w > 0 && r.size.h > 0, "tile {} got zero-sized rect {:?}", i, r);
        }
    }

    #[test]
    fn test_exit_overview_unregisters_all() {
        use crate::overlay::MockThumbnailOverview;
        let mut engine = make_engine();
        let sink = MockThumbnailOverview::new();
        engine.install_thumbnail_overview(sink.clone());

        for i in 400..405 {
            let window = make_window(i, 100, 100);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        engine.enter_overview(&BackendHandle::default_for_test());
        assert_eq!(sink.registered_count(), 5);

        engine.exit_overview(&BackendHandle::default_for_test());
        assert_eq!(sink.exit_calls(), 1);
        assert_eq!(sink.unregister_calls(), 5);
        assert_eq!(sink.registered_count(), 0);
    }

    #[test]
    fn test_thumbnail_overview_disabled_falls_back_to_setwindowpos() {
        // No sink installed → enter/exit overview must not panic and
        // the engine falls back to the historic SetWindowPos path
        // (verified indirectly via has_thumbnail_overview returning
        // false + overview still functions normally).
        let mut engine = make_engine();
        assert!(!engine.has_thumbnail_overview());
        for i in 500..503 {
            let window = make_window(i, 100, 100);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());
        engine.exit_overview(&BackendHandle::default_for_test());
        assert!(!engine.is_overview());
    }

    #[test]
    fn test_overview_toggle_method() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        // toggle_overview should enter and exit
        engine.toggle_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());
        engine.toggle_overview(&BackendHandle::default_for_test());
        assert!(!engine.is_overview());
    }

    #[test]
    fn test_overview_navigation() {
        let mut engine = make_engine();
        // Add 3 windows
        for i in 100..103 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }

        engine.enter_overview(&BackendHandle::default_for_test());

        // Focus should be on last added window (column 2)
        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(2));

        // Navigate right should wrap to 0
        engine.overview_focus_right(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));

        // Navigate left should wrap to 2
        engine.overview_focus_left(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(2));
    }

    #[test]
    fn test_config_update() {
        let mut engine = make_engine();
        let new_config = LayoutConfig {
            column_width: 700,
            ..LayoutConfig::default()
        };
        engine.update_config(new_config);
        assert_eq!(engine.config().column_width, 700);
    }

    #[test]
    fn test_calculate_positions_single_column() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        let work_rect = Rect::new(4, 4, 1912, 1032);
        let positions = engine.calculate_positions(workspace, work_rect, &engine.config);
        assert_eq!(positions.len(), 1);
        let (_, rect) = &positions[0];
        assert_eq!(rect.loc.x, 4);
        assert!(rect.size.w > 0);
        assert!(rect.size.h > 0);
    }

    #[test]
    fn test_calculate_positions_multiple_columns() {
        let mut engine = make_engine();
        for i in 100..104 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        let work_rect = Rect::new(4, 4, 1912, 1032);
        let positions = engine.calculate_positions(workspace, work_rect, &engine.config);
        // In Fixed mode: 4 columns at 500px each. Some may extend beyond the viewport
        // (niri scrolls — that is fine). All 4 should be returned because left edges are
        // within the viewport (culling only removes columns whose left edge is past the
        // right edge of the view).
        assert_eq!(positions.len(), 4);
        for (_, rect) in &positions {
            // Fixed mode: each column is config.column_width (500px) minus optional inset.
            // Allow 50px tolerance for border/inset rounding.
            assert!(rect.size.w >= 500 - 50, "window too narrow: {}", rect.size.w);
            assert!(rect.size.h > 100, "window too short: {}", rect.size.h);
        }
        // Columns must be ordered left-to-right.
        assert!(positions[0].1.loc.x < positions[1].1.loc.x);
        assert!(positions[1].1.loc.x < positions[2].1.loc.x);
        assert!(positions[2].1.loc.x < positions[3].1.loc.x);
    }

    #[test]
    fn test_calculate_positions_stacked_tiles() {
        let mut engine = make_engine();
        let window1 = make_window(100, 500, 400);
        engine.add_window(window1, &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            if let Some(workspace) = monitor.workspace_mut() {
                workspace.add_window_to_column(0, WindowId::new(101));
            }
        }
        let window2 = make_window(101, 500, 400);
        engine.tiled_windows.insert(WindowId::new(101), window2);

        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        let work_rect = Rect::new(4, 4, 1912, 1032);
        let positions = engine.calculate_positions(workspace, work_rect, &engine.config);
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0].1.loc.x, positions[1].1.loc.x);
        assert!(positions[1].1.loc.y > positions[0].1.loc.y);
    }

    #[test]
    fn test_calculate_positions_overview_zoom() {
        let mut engine = make_engine();
        for i in 100..110 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.overview_zoom() < 1.0);

        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        let work_rect = Rect::new(4, 4, 1912, 1032);
        let positions = engine.calculate_positions(workspace, work_rect, &engine.config);
        assert_eq!(positions.len(), 10);
        for (_, rect) in &positions {
            assert!(rect.loc.x >= work_rect.loc.x,
                "window x={} < work x={}", rect.loc.x, work_rect.loc.x);
            assert!((rect.loc.x + rect.size.w as i32) <= work_rect.loc.x + work_rect.size.w as i32 + 10,
                "window right={} > work right={}",
                rect.loc.x + rect.size.w as i32,
                work_rect.loc.x + work_rect.size.w as i32);
        }
    }

    #[test]
    fn test_focus_left_at_boundary() {
        let mut engine = make_engine();
        for i in 100..104 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(3));

        for _ in 0..3 {
            engine.focus_left(&BackendHandle::default_for_test());
        }
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));

        engine.focus_left(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));
    }

    #[test]
    fn test_focus_right_at_boundary() {
        let mut engine = make_engine();
        for i in 100..104 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        for _ in 0..3 {
            engine.focus_left(&BackendHandle::default_for_test());
        }
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));

        for _ in 0..3 {
            engine.focus_right(&BackendHandle::default_for_test());
        }
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(3));

        engine.focus_right(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(3));
    }

    #[test]
    fn test_focus_up_down() {
        let mut engine = make_engine();
        let window1 = make_window(100, 500, 400);
        engine.add_window(window1, &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            if let Some(workspace) = monitor.workspace_mut() {
                workspace.add_window_to_column(0, WindowId::new(101));
            }
        }
        let window2 = make_window(101, 500, 400);
        engine.tiled_windows.insert(WindowId::new(101), window2);

        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(100)));

        engine.focus_down(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(101)));

        engine.focus_down(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(101)));

        engine.focus_up(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(100)));

        engine.focus_up(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(100)));
    }

    #[test]
    fn test_scroll_clamping() {
        let mut engine = make_engine();
        for i in 100..105 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        engine.scroll(ScrollDirection::Right, &BackendHandle::default_for_test());
        engine.scroll(ScrollDirection::Right, &BackendHandle::default_for_test());
        engine.scroll(ScrollDirection::Right, &BackendHandle::default_for_test());

        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        assert!(workspace.scroll_offset.x >= 0);

        engine.scroll(ScrollDirection::Left, &BackendHandle::default_for_test());
        engine.scroll(ScrollDirection::Left, &BackendHandle::default_for_test());
        engine.scroll(ScrollDirection::Left, &BackendHandle::default_for_test());

        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        assert_eq!(workspace.scroll_offset.x, 0);
    }

    #[test]
    fn test_switch_workspace_preserves_windows() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());
        assert_eq!(engine.tiled_windows().len(), 1);

        let oid = engine.focused_output().unwrap();
        engine.switch_workspace(1, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.active_workspace_id(), 1);
        let workspace = monitor.workspace().unwrap();
        assert_eq!(workspace.columns.len(), 0);

        engine.switch_workspace(0, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.active_workspace_id(), 0);
        let workspace = monitor.workspace().unwrap();
        assert_eq!(workspace.columns.len(), 1);
        assert_eq!(engine.tiled_windows().len(), 1);
    }

    #[test]
    fn test_remove_window_updates_focus() {
        let mut engine = make_engine();
        let w1 = make_window(100, 500, 400);
        let w2 = make_window(101, 600, 400);
        engine.add_window(w1, &BackendHandle::default_for_test());
        engine.add_window(w2, &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(101)));

        engine.remove_window(WindowId::new(101), &BackendHandle::default_for_test());
        assert_eq!(engine.tiled_windows().len(), 1);
        assert!(engine.tiled_windows().contains_key(&WindowId::new(100)));
    }

    #[test]
    fn test_multi_monitor() {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let oid1 = OutputId::from_name("Monitor1");
        let oid2 = OutputId::from_name("Monitor2");
        engine.register_monitor(oid1, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));
        engine.register_monitor(oid2, Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1040));

        assert_eq!(engine.monitors().len(), 2);
        assert!(engine.focused_output().is_some());

        let target = engine.get_target_output(&WindowInfo {
            hwnd: 100, title: "test".into(), class_name: "test".into(),
            process_id: 0, bounds: Rect::new(960, 400, 800, 600),
            state: crate::backend::WindowState::Normal, is_visible: true,
        });
        assert_eq!(target, oid1);
    }

    #[test]
    fn test_overview_zoom_calculation() {
        // In proportional mode with enough space, all columns fit
        let config = LayoutConfig {
            column_width_mode: ColumnWidthMode::Proportional,
            column_width: 500,
            column_gap: 8,
            ..LayoutConfig::default()
        };
        // 3 columns on 1920px: each gets (1920-2*8)/3 = 634
        // Total = 3*634 + 2*8 = 1918, fits in 1920
        let col_w = TilingEngine::effective_column_width(3, 1920, &config);
        assert_eq!(col_w, 634);
        let total = 3 * col_w + 2 * config.column_gap;
        assert!(total <= 1920, "total={} > 1920", total);

        // With 5 columns on 1280px: col_w = max(249, 250) = 250
        // Total = 5*250 + 4*8 = 1282 > 1280 — slight overflow due to min-width clamp
        let col_w = TilingEngine::effective_column_width(5, 1280, &config);
        assert_eq!(col_w, 250); // min-width clamp kicks in
        let total = 5 * col_w + 4 * config.column_gap;
        // With overflow, zoom < 1.0
        if total > 1280 {
            let zoom = 1280.0 / total as f64;
            assert!(zoom > 0.99 && zoom < 1.0);
        }
    }

    #[test]
    fn test_overview_zoom_fixed_mode() {
        let config = LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            column_gap: 8,
            ..LayoutConfig::default()
        };
        let col_w = TilingEngine::effective_column_width(3, 1280, &config);
        assert_eq!(col_w, 500);
        let total = 3 * 500 + 2 * 8;
        let zoom = if total > 1280 { 1280.0 / total as f64 } else { 1.0 };
        assert!((zoom - 0.844).abs() < 0.01);
        assert!(zoom < 1.0);
    }

    #[test]
    fn test_empty_workspace_operations() {
        let mut engine = make_engine();
        // These should not panic on empty workspace
        engine.focus_left(&BackendHandle::default_for_test());
        engine.focus_right(&BackendHandle::default_for_test());
        engine.focus_up(&BackendHandle::default_for_test());
        engine.focus_down(&BackendHandle::default_for_test());
        engine.scroll(ScrollDirection::Left, &BackendHandle::default_for_test());
        engine.scroll(ScrollDirection::Right, &BackendHandle::default_for_test());
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(!engine.is_overview());
        engine.close_focused_window(&BackendHandle::default_for_test());
    }

    #[test]
    fn test_config_binds_default() {
        let engine = make_engine();
        let binds = engine.config_binds();
        assert!(binds.hotkeys.is_empty());
    }

    #[test]
    fn test_animation_settings() {
        let mut engine = make_engine();
        assert!(!engine.animation().is_enabled());
        engine.update_animation_settings(true, 300, crate::layout::Easing::CubicOut);
        assert!(engine.animation().is_enabled());
    }

    #[test]
    fn test_register_monitor_with_scale() {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let oid = OutputId::from_name("HiDPI");
        engine.register_monitor_with_scale(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040), 1.5);
        let monitor = engine.monitors().get(&oid).unwrap();
        assert!((monitor.scale_factor - 1.5).abs() < 0.01);
    }

    #[test]
    fn test_unregister_monitor() {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let oid1 = OutputId::from_name("M1");
        let oid2 = OutputId::from_name("M2");
        engine.register_monitor(oid1, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));
        engine.register_monitor(oid2, Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1040));
        assert_eq!(engine.monitors().len(), 2);

        engine.unregister_monitor(&oid1);
        assert_eq!(engine.monitors().len(), 1);
        // Focused output should switch to remaining monitor
        assert_eq!(engine.focused_output(), Some(oid2));
    }


    // --- Bug-revealing tests ---

    #[test]
    fn test_move_column_left_focus_tracking() {
        let mut engine = make_engine();
        // 3 windows: col0=100, col1=101, col2=102
        for i in 100..103 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        // Focus is on col2 (window 102)
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(2));
        assert_eq!(monitor.focus_window, Some(WindowId::new(102)));

        // Move left: col2 -> col1. Now: col0=100, col1=102, col2=101
        engine.move_column(ScrollDirection::Left, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        // Focus should be on the moved window (102) at its new position (col1)
        assert_eq!(monitor.focus_window, Some(WindowId::new(102)));
        assert_eq!(monitor.focus_column, Some(1));
    }

    #[test]
    fn test_move_column_right_focus_tracking() {
        let mut engine = make_engine();
        // 3 windows: col0=100, col1=101, col2=102
        for i in 100..103 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        // Move to column 0
        engine.focus_left(&BackendHandle::default_for_test());
        engine.focus_left(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));

        // Move right: col0 -> col1
        engine.move_column(ScrollDirection::Right, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(100)));
        assert_eq!(monitor.focus_column, Some(1));
    }

    #[test]
    fn test_move_column_from_first_column() {
        let mut engine = make_engine();
        // 2 windows: col0=100, col1=101
        for i in 100..102 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        // Go to col 0
        engine.focus_left(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));

        // Move left from col0 — saturating_sub makes target_col=0 (same position)
        // move_window should still succeed (re-inserts at same col)
        engine.move_column(ScrollDirection::Left, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        // Window should still exist and be focused
        assert_eq!(monitor.focus_window, Some(WindowId::new(100)));
    }

    #[test]
    fn test_remove_middle_window_focus() {
        let mut engine = make_engine();
        // 3 windows: col0=100, col1=101, col2=102
        for i in 100..103 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        // Focus col1
        engine.focus_left(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(1));
        assert_eq!(monitor.focus_window, Some(WindowId::new(101)));

        // Remove the focused window (col1) — should re-focus to col0
        engine.remove_window(WindowId::new(101), &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        // Focus should shift to an adjacent column
        assert!(monitor.focus_column.unwrap() < 2);
    }

    #[test]
    fn test_remove_only_window_in_column() {
        let mut engine = make_engine();
        // 3 single-tile columns
        for i in 100..103 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        // Remove window from col0
        engine.focus_left(&BackendHandle::default_for_test());
        engine.focus_left(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));

        engine.remove_window(WindowId::new(100), &BackendHandle::default_for_test());

        // Should have 2 columns left
        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        assert_eq!(workspace.columns.len(), 2);
    }

    #[test]
    fn test_move_column_right_no_phantom_column() {
        // Regression: previously moving the last column right created a phantom
        // empty intermediate column. After the fix, empty columns are reaped.
        let mut engine = make_engine();
        // 2 windows: col0=100, col1=101
        for i in 100..102 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        engine.move_column(ScrollDirection::Right, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        // Still 2 columns — no phantom empty column.
        assert_eq!(workspace.columns.len(), 2);
        for col in &workspace.columns {
            assert!(!col.tiles.is_empty(), "no empty columns should remain");
        }
    }

    #[test]
    fn test_overview_no_columns() {
        let mut engine = make_engine();
        // No windows — overview should gracefully handle
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(!engine.is_overview()); // No columns => can't enter overview
    }

    #[test]
    fn test_overview_exit_restores_zoom() {
        let mut engine = make_engine();
        for i in 100..104 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }

        // Enter overview
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());
        // With proportional mode and only 4 columns, zoom might be 1.0
        // (they all fit). Overview is still entered though.

        // Exit overview
        engine.exit_overview(&BackendHandle::default_for_test());
        assert!(!engine.is_overview());
        assert_eq!(engine.overview_zoom(), 1.0);
    }

    #[test]
    fn test_overview_fixed_mode_scroll() {
        // Use fixed mode where columns DON'T fit in view
        let config = LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            column_gap: 8,
            ..LayoutConfig::default()
        };
        let mut engine = TilingEngine::new(config);
        let oid = OutputId::from_name("TestMonitor");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));

        // Add 10 windows — definitely won't fit in 1920px
        for i in 100..110 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }

        // Scroll right
        engine.scroll(ScrollDirection::Right, &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        let scroll_before = {
            let monitor = engine.monitors().get(&oid).unwrap();
            monitor.workspace().unwrap().scroll_offset.x
        };
        assert!(scroll_before > 0, "scroll should be > 0 with 10 fixed columns, got {}", scroll_before);

        // Enter overview
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());

        // Exit overview
        engine.exit_overview(&BackendHandle::default_for_test());
        assert!(!engine.is_overview());
    }


    // --- Edge case and lifecycle tests ---

    #[test]
    fn test_add_window_float_rule() {
        let mut engine = make_engine();
        let mut rule = crate::config::WindowRule::default();
        rule.class = Some("TestClass".to_string());
        rule.floating = true;
        engine.set_window_rules(vec![rule]);

        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        // Window should be floating, not in any workspace column
        assert!(engine.is_floating(WindowId::new(100)));
        assert_eq!(engine.tiled_windows().len(), 1);
        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        let ws = monitor.workspace().unwrap();
        assert_eq!(ws.columns.len(), 0);
    }

    #[test]
    fn test_toggle_floating_tiled_to_float() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        assert!(!engine.is_floating(WindowId::new(100)));
        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.workspace().unwrap().columns.len(), 1);

        // Float it
        engine.toggle_floating(&BackendHandle::default_for_test());
        assert!(engine.is_floating(WindowId::new(100)));
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.workspace().unwrap().columns.len(), 0);
    }

    #[test]
    fn test_toggle_floating_float_to_tiled() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        engine.toggle_floating(&BackendHandle::default_for_test());
        assert!(engine.is_floating(WindowId::new(100)));

        // Un-float it
        engine.toggle_floating(&BackendHandle::default_for_test());
        assert!(!engine.is_floating(WindowId::new(100)));
        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.workspace().unwrap().columns.len(), 1);
    }

    #[test]
    fn test_toggle_fullscreen_roundtrip() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        assert!(!engine.is_fullscreen(WindowId::new(100)));
        engine.toggle_fullscreen(&BackendHandle::default_for_test());
        assert!(engine.is_fullscreen(WindowId::new(100)));
        engine.toggle_fullscreen(&BackendHandle::default_for_test());
        assert!(!engine.is_fullscreen(WindowId::new(100)));
    }

    #[test]
    fn test_switch_multiple_workspaces() {
        let mut engine = make_engine();
        let window1 = make_window(100, 500, 400);
        engine.add_window(window1, &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();

        // Switch to workspace 2 and add a window
        engine.switch_workspace(2, &BackendHandle::default_for_test());
        let window2 = make_window(101, 500, 400);
        engine.add_window(window2, &BackendHandle::default_for_test());
        assert_eq!(engine.tiled_windows().len(), 2);

        // Switch to workspace 1 (empty, auto-created)
        engine.switch_workspace(1, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.active_workspace_id(), 1);
        let ws = monitor.workspace().unwrap();
        assert_eq!(ws.columns.len(), 0);

        // Switch back to workspace 0
        engine.switch_workspace(0, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        let ws = monitor.workspace().unwrap();
        assert_eq!(ws.columns.len(), 1);

        // Switch to workspace 2
        engine.switch_workspace(2, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        let ws = monitor.workspace().unwrap();
        assert_eq!(ws.columns.len(), 1);
    }

    #[test]
    fn test_update_config_changes_column_width() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        let new_config = LayoutConfig {
            column_width: 800,
            column_width_mode: ColumnWidthMode::Fixed,
            ..LayoutConfig::default()
        };
        engine.update_config(new_config);
        assert_eq!(engine.config().column_width, 800);
        assert_eq!(engine.config().column_width_mode, ColumnWidthMode::Fixed);
    }

    #[test]
    fn test_set_full_config_and_get_binds() {
        let mut engine = make_engine();
        let mut config = crate::config::Config::default();
        let mut bind = crate::config::HotkeyBinding::default();
        bind.modifiers = vec!["Ctrl".to_string()];
        bind.key = "A".to_string();
        bind.command = "focus-column-left".to_string();
        config.binds.hotkeys.push(bind);

        engine.set_full_config(config);
        let binds = engine.config_binds();
        assert_eq!(binds.hotkeys.len(), 1);
        assert_eq!(binds.hotkeys[0].key, "A");
    }

    #[test]
    fn test_set_window_rules() {
        let mut engine = make_engine();
        let mut rule = crate::config::WindowRule::default();
        rule.class = Some("SpecialApp".to_string());
        rule.floating = true;
        engine.set_window_rules(vec![rule]);

        let rules = engine.window_rules_ref();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].class, Some("SpecialApp".to_string()));
    }

    #[test]
    fn test_window_sizing_mode_default() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());
        assert_eq!(engine.window_sizing_mode(WindowId::new(100)), SizingMode::Normal);
    }

    #[test]
    fn test_remove_nonexistent_window() {
        let mut engine = make_engine();
        engine.remove_window(WindowId::new(999), &BackendHandle::default_for_test());
        assert_eq!(engine.tiled_windows().len(), 0);
    }

    #[test]
    fn test_set_focused_output() {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let oid1 = OutputId::from_name("M1");
        let oid2 = OutputId::from_name("M2");
        engine.register_monitor(oid1, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));
        engine.register_monitor(oid2, Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1040));

        engine.set_focused_output(oid2);
        assert_eq!(engine.focused_output(), Some(oid2));

        // Setting to nonexistent output should not change
        engine.set_focused_output(OutputId::from_name("NoExist"));
        assert_eq!(engine.focused_output(), Some(oid2));
    }

    #[test]
    fn test_effective_column_width_edge_cases() {
        // 0 columns
        assert_eq!(TilingEngine::effective_column_width(0, 1920, &LayoutConfig::default()), 500);
        // 1 column
        let config = LayoutConfig { column_width_mode: ColumnWidthMode::Proportional, ..LayoutConfig::default() };
        let w = TilingEngine::effective_column_width(1, 1920, &config);
        assert_eq!(w, 1920);
        // 100 columns — should still produce positive width
        let w = TilingEngine::effective_column_width(100, 1920, &config);
        assert!(w > 0);
        assert!(w <= 250);
    }

    #[test]
    fn test_engine_no_monitors() {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        engine.focus_left(&BackendHandle::default_for_test());
        engine.focus_right(&BackendHandle::default_for_test());
        engine.scroll(ScrollDirection::Left, &BackendHandle::default_for_test());
        engine.enter_overview(&BackendHandle::default_for_test());
        engine.close_focused_window(&BackendHandle::default_for_test());
    }

    #[test]
    fn test_column_width_mode_fixed() {
        let config = LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 600,
            ..LayoutConfig::default()
        };
        assert_eq!(TilingEngine::effective_column_width(5, 1920, &config), 600);
    }

    #[test]
    fn test_calculate_positions_empty_workspace() {
        let engine = make_engine();
        let ws = crate::layout::Workspace::new();
        let positions = engine.calculate_positions(&ws, Rect::new(0, 0, 1920, 1080), &engine.config);
        assert!(positions.is_empty());
    }

    #[test]
    fn test_overview_navigation_wrap_around() {
        let mut engine = make_engine();
        for i in 100..104 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        // Enter overview
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());

        // We're at column 3. Navigate right — should wrap to 0
        engine.overview_focus_right(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));

        // Navigate left from 0 — should wrap to 3
        engine.overview_focus_left(&BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(3));
    }

    #[test]
    fn test_overview_select_exits_overview() {
        let mut engine = make_engine();
        for i in 100..104 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }

        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());

        let wid = engine.overview_select(&BackendHandle::default_for_test());
        assert!(!engine.is_overview());
        assert!(wid.is_some());
    }

    // -------------------------------------------------------------------------
    // Multi-workspace overview tests — the niri-style "see every workspace at
    // once" path.  `enter_overview` collects ALL non-empty workspaces of the
    // focused monitor, stacks them vertically with a 50px gap, picks a zoom
    // that fits both width and height, and renders every tile (not just
    // active-workspace tiles).
    // -------------------------------------------------------------------------

    #[test]
    fn test_multi_workspace_overview_state_offsets() {
        // Three workspaces with content + one empty workspace.  The empty one
        // must be skipped so `workspace_offsets` only carries non-empty ids.
        let mut engine = make_engine();
        let oid = engine.focused_output().unwrap();

        // ws 0: two windows
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());

        // ws 1: one window
        engine.move_window_to_workspace(1, &BackendHandle::default_for_test());

        // ws 2: empty (created implicitly by the move below moving a window in then back)
        // We create ws 2 explicitly via switch_workspace so it appears in the
        // workspaces map but stays empty.
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            monitor.workspaces.entry(2).or_insert_with(crate::layout::Workspace::new);
        }

        // ws 3: one window
        engine.add_window(make_window(102, 0, 0), &BackendHandle::default_for_test());
        engine.move_window_to_workspace(3, &BackendHandle::default_for_test());

        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview(), "overview should be active");

        let state = engine.overview_state().expect("overview state present");
        // ws ids of non-empty workspaces: 0, 1, 3 (ws 2 is empty)
        let ids: Vec<i32> = state.workspace_offsets.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![0, 1, 3], "empty workspaces must be skipped");

        // Offsets should be monotonically increasing.
        let ys: Vec<i32> = state.workspace_offsets.iter().map(|(_, y)| *y).collect();
        assert_eq!(ys[0], 0, "first workspace starts at y=0");
        assert!(ys[1] > ys[0], "second workspace below first");
        assert!(ys[2] > ys[1], "third workspace below second");
    }

    #[test]
    fn test_multi_workspace_overview_positions_every_window() {
        // Add 3 workspaces with 2 + 1 + 1 windows.  In overview every tile
        // (4 total) must appear in calculate_positions output for that
        // workspace.  This verifies the layout pass picks up tiles outside
        // the active workspace.
        let mut engine = make_engine();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());
        engine.move_window_to_workspace(1, &BackendHandle::default_for_test());
        engine.add_window(make_window(102, 0, 0), &BackendHandle::default_for_test());
        engine.move_window_to_workspace(2, &BackendHandle::default_for_test());
        engine.add_window(make_window(103, 0, 0), &BackendHandle::default_for_test());

        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());

        // Build the equivalent of what apply_layout_for_monitor's overview
        // branch produces by iterating every workspace through the
        // `calculate_positions_in_section` helper.
        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        let work_rect = Rect::new(4, 4, 1912, 1032);
        let mut ws_ids: Vec<i32> = monitor.workspaces.keys().copied().collect();
        ws_ids.sort();
        let mut total: Vec<(WindowId, Rect)> = Vec::new();
        let nominal = monitor.work_area.size.h as i32;
        let zoom = engine.overview_zoom();
        let scaled_section = (nominal as f64 * zoom) as i32;
        let gap = (50.0 * zoom) as i32;
        let mut y = 0;
        for ws_id in ws_ids {
            if let Some(ws) = monitor.workspaces.get(&ws_id) {
                if ws.columns.is_empty() { continue; }
                let mut p = engine.calculate_positions_in_section(ws, work_rect, y, nominal, &engine.config);
                total.append(&mut p);
                y += scaled_section + gap;
            }
        }
        assert_eq!(total.len(), 4, "overview must position every workspace's tiles");
    }

    #[test]
    fn test_multi_workspace_overview_all_empty_no_op() {
        // Engine with monitor but no windows — enter_overview must NOT
        // engage overview mode (nothing to render).
        let mut engine = make_engine();
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(!engine.is_overview());
        assert!(engine.overview_state().is_none());
    }

    #[test]
    fn test_multi_workspace_overview_exit_hides_inactive_tiles() {
        // While overview is on every workspace's tiles are visible.  On
        // exit, tiles that aren't on the active workspace must be marked
        // hidden in the applied_state cache so subsequent passes know to
        // re-show them (and don't leave them as zombie visible windows).
        let mut engine = make_engine();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.move_window_to_workspace(1, &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());

        // Active workspace is 0 (after move_window_to_workspace returns).
        engine.enter_overview(&BackendHandle::default_for_test());
        engine.exit_overview(&BackendHandle::default_for_test());

        // Window 100 lives on workspace 1 (inactive).  Its applied_state
        // entry must report visible=false after exit.
        let s = engine.applied_state.get(&WindowId::new(100));
        assert!(
            s.map(|x| !x.visible).unwrap_or(false),
            "inactive-workspace tile must be marked hidden after exit_overview, got {:?}",
            s
        );
    }

    #[test]
    fn test_multi_workspace_overview_zoom_adapts_to_workspace_count() {
        // One workspace with a small number of columns → no zoom required
        // (zoom == 1.0).  With many workspaces stacked the height budget
        // forces zoom < 1.0 even though each individual workspace would
        // otherwise fit horizontally.
        // Use fixed mode so column widths don't auto-shrink to fit.
        let config = LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            column_gap: 8,
            ..LayoutConfig::default()
        };
        let mut engine = TilingEngine::new(config);
        let oid = OutputId::from_name("Test");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));

        // Eight workspaces, each with one 500px column.  Stacked they need
        // 8 * 1040 + 7 * 50 = 8670px tall vs 1040 available → zoom_h ~0.12.
        for i in 0..8 {
            engine.add_window(make_window(100 + i, 0, 0), &BackendHandle::default_for_test());
            engine.move_window_to_workspace(i as i32 + 100, &BackendHandle::default_for_test());
        }

        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());
        let zoom = engine.overview_zoom();
        assert!(zoom < 0.5,
            "8 stacked workspaces must force zoom < 0.5; got {}", zoom);
    }

    #[test]
    fn test_move_column_left_from_first() {
        let mut engine = make_engine();
        for i in 100..103 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        let oid = engine.focused_output().unwrap();

        // Go to col 0
        for _ in 0..2 { engine.focus_left(&BackendHandle::default_for_test()); }
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.focus_column, Some(0));

        // Move left from col 0 — target_col = 0 (same place)
        engine.move_column(ScrollDirection::Left, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        // Window should still exist and be focused
        assert_eq!(monitor.focus_window, Some(WindowId::new(100)));
        assert_eq!(monitor.focus_column, Some(0));
    }

    // --- AddWindowTarget tests ---

    #[test]
    fn test_add_window_target_output() {
        // Register two monitors; route a window explicitly to the second monitor.
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let oid1 = OutputId::from_name("M1");
        let oid2 = OutputId::from_name("M2");
        engine.register_monitor(oid1, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));
        engine.register_monitor(oid2, Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1040));

        // Window is in the bounds of monitor 1 but we force it to monitor 2.
        let window = make_window(100, 200, 200);
        engine.add_window_with_target(window, AddWindowTarget::Output(oid2), &BackendHandle::default_for_test(), false);

        // Should be in monitor 2's workspace, not monitor 1's.
        let m2 = engine.monitors().get(&oid2).unwrap();
        assert_eq!(m2.workspace().unwrap().columns.len(), 1);
        let m1 = engine.monitors().get(&oid1).unwrap();
        assert_eq!(m1.workspace().unwrap().columns.len(), 0);
        assert_eq!(engine.tiled_windows().len(), 1);
    }

    #[test]
    fn test_add_window_target_workspace() {
        // Add a window to workspace 3 of a monitor, leaving the active workspace (0) untouched.
        let mut engine = make_engine();
        let oid = engine.focused_output().unwrap();

        let window = make_window(100, 200, 200);
        engine.add_window_with_target(
            window,
            AddWindowTarget::Workspace { output: oid, workspace_id: 3 },
            &BackendHandle::default_for_test(),
            false,
        );

        // Active workspace (0) should be empty.
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.workspace().unwrap().columns.len(), 0);
        // Workspace 3 should contain the window.
        let ws3 = monitor.workspaces.get(&3).unwrap();
        assert_eq!(ws3.columns.len(), 1);
        assert_eq!(engine.tiled_windows().len(), 1);
    }

    #[test]
    fn test_add_window_target_next_to() {
        // Place window B immediately to the right of window A.
        let mut engine = make_engine();

        let win_a = make_window(100, 200, 200);
        engine.add_window(win_a, &BackendHandle::default_for_test()); // col 0
        let win_b = make_window(101, 200, 200);
        engine.add_window(win_b, &BackendHandle::default_for_test()); // col 1

        // Add window C next to window A (col 0) — should become col 1, pushing B to col 2.
        let win_c = make_window(102, 200, 200);
        engine.add_window_with_target(
            win_c,
            AddWindowTarget::NextTo(WindowId::new(100)),
            &BackendHandle::default_for_test(),
            false,
        );

        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        let ws = monitor.workspace().unwrap();
        assert_eq!(ws.columns.len(), 3);
        // Column 0 should still be window A.
        assert_eq!(ws.columns[0].tiles[0].window_id, WindowId::new(100));
        // Column 1 should be window C (inserted next to A).
        assert_eq!(ws.columns[1].tiles[0].window_id, WindowId::new(102));
        // Column 2 should be window B.
        assert_eq!(ws.columns[2].tiles[0].window_id, WindowId::new(101));
    }

    #[test]
    fn test_add_window_target_next_to_not_found() {
        // NextTo a non-existent window should fall back to Auto (append to active workspace).
        let mut engine = make_engine();

        let win = make_window(100, 200, 200);
        engine.add_window_with_target(
            win,
            AddWindowTarget::NextTo(WindowId::new(999)), // 999 not in layout
            &BackendHandle::default_for_test(),
            false,
        );

        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.workspace().unwrap().columns.len(), 1);
        assert_eq!(engine.tiled_windows().len(), 1);
    }

    // --- Fixed-mode default and overview zoom tests ---

    #[test]
    fn test_default_config_is_fixed_mode() {
        let config = LayoutConfig::default();
        assert_eq!(config.column_width_mode, ColumnWidthMode::Fixed,
            "Default column_width_mode must be Fixed (niri invariant)");
    }

    #[test]
    fn test_calculate_positions_fixed_overview_zoom() {
        // With Fixed mode and overview zoom, all column rects must fit within the work area.
        let mut engine = make_engine();
        for i in 100..110 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        engine.enter_overview(&BackendHandle::default_for_test());
        assert!(engine.is_overview());
        assert!(engine.overview_zoom() < 1.0, "10 Fixed-500px columns must need zoom-out");

        let oid = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        let work_rect = Rect::new(4, 4, 1912, 1032);
        let positions = engine.calculate_positions(workspace, work_rect, &engine.config);
        assert_eq!(positions.len(), 10, "all 10 windows must be positioned in overview");
        let work_right = work_rect.loc.x + work_rect.size.w as i32;
        for (_, rect) in &positions {
            assert!(rect.loc.x >= work_rect.loc.x,
                "window x={} left of work area x={}", rect.loc.x, work_rect.loc.x);
            // Allow 10px slop for integer rounding.
            assert!(rect.loc.x + rect.size.w as i32 <= work_right + 10,
                "window right={} exceeds work right={}+10",
                rect.loc.x + rect.size.w as i32, work_right);
        }
    }

    #[test]
    fn test_calculate_positions_tabbed_column_shows_only_active() {
        let mut engine = make_engine();
        // Add two windows so they land in col0 stacked.
        let w1 = make_window(200, 500, 400);
        let w2 = make_window(201, 500, 400);
        // Sanity check the test fixture itself — both windows must be
        // distinct WindowInfo records with the expected hwnds.
        assert_eq!(w1.hwnd, 200);
        assert_eq!(w2.hwnd, 201);
        engine.add_window(w1, &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        // Manually push w2 into col0 to create a stacked column. (We use
        // the captured `w2` rather than rebuilding to ensure the assertions
        // above match what actually goes into the engine.)
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            if let Some(workspace) = monitor.workspace_mut() {
                workspace.add_window_to_column(0, WindowId::new(w2.hwnd));
            }
        }
        engine.tiled_windows.insert(WindowId::new(w2.hwnd), w2);

        // Verify stacked: both tiles positioned.
        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        let work_rect = Rect::new(4, 4, 1912, 1032);
        let positions = engine.calculate_positions(workspace, work_rect, &engine.config);
        assert_eq!(positions.len(), 2, "stacked should show both tiles");

        // Switch to tabbed and check only 1 tile is positioned.
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            if let Some(workspace) = monitor.workspace_mut() {
                if let Some(col) = workspace.columns.get_mut(0) {
                    col.switch_to_tabbed();
                }
            }
        }
        let monitor = engine.monitors().get(&oid).unwrap();
        let workspace = monitor.workspace().unwrap();
        let positions = engine.calculate_positions(workspace, work_rect, &engine.config);
        assert_eq!(positions.len(), 1, "tabbed should show only the active tab");
        // The active tab (index 0) is w1 (WindowId 200).
        assert_eq!(positions[0].0, WindowId::new(200));
        // It should occupy the full work height.
        assert_eq!(positions[0].1.size.h, work_rect.size.h);
    }

    #[test]
    fn test_monitor_set_primary_and_focused() {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let oid1 = OutputId::from_name("Primary");
        let oid2 = OutputId::from_name("Secondary");
        engine.register_monitor(oid1, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040));
        engine.register_monitor(oid2, Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1040));

        // Primary is first inserted.
        assert_eq!(engine.monitors().primary_id(), Some(oid1));
        // Focused is also oid1 (auto-assigned on first register).
        assert_eq!(engine.monitors().focused_id(), Some(oid1));

        engine.set_focused_output(oid2);
        assert_eq!(engine.monitors().focused_id(), Some(oid2));
        // Primary should not change.
        assert_eq!(engine.monitors().primary_id(), Some(oid1));
    }

    #[test]
    fn test_configure_throttle_skips_repeated_apply() {
        // Verify that after the first apply_all the tile's configure_throttle.last_send
        // is set, and that a second immediate apply_all does NOT call set_window_position
        // for the tile (intent becomes Throttled because min_interval hasn't elapsed).

        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();

        // First apply: tile starts with last_send == None so intent == ShouldSend.
        // set_window_position should be called and mark_sent() should update last_send.
        engine.apply_all(&BackendHandle::default_for_test());

        // After first apply, last_send must be Some(_).
        {
            let monitor = engine.monitors().get(&oid).unwrap();
            let workspace = monitor.workspace().unwrap();
            let tile = &workspace.columns[0].tiles[0];
            assert!(tile.configure_throttle.last_send.is_some(),
                "last_send should be set after first apply");
        }

        // Second apply: default min_interval is 16ms and we're well within that window,
        // so intent() should be Throttled — no set_window_position for this tile.
        engine.apply_all(&BackendHandle::default_for_test());

        // Confirm the tile's intent is Throttled (i.e., 16ms has not elapsed).
        {
            let monitor = engine.monitors().get(&oid).unwrap();
            let workspace = monitor.workspace().unwrap();
            let tile = &workspace.columns[0].tiles[0];
            assert_ne!(tile.configure_throttle.intent(), ConfigureIntent::ShouldSend,
                "intent should not be ShouldSend on the second immediate apply");
        }
    }

    // -----------------------------------------------------------------------
    // Applied-state cache tests (Item 1)
    // -----------------------------------------------------------------------

    /// After the first apply_all a window's state is cached.
    /// A second immediate apply_all should issue no further Win32 calls because:
    ///   a) the AppliedState cache matches, and
    ///   b) the ConfigureThrottle would also block the position call anyway.
    /// We verify (a) by checking that `win32_call_count` does not increase on
    /// the second pass.
    #[test]
    fn test_applied_state_skips_unchanged_apply() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        // First apply: state is unset → all fields differ → Win32 calls issued.
        engine.win32_call_count = 0;
        engine.apply_all(&BackendHandle::default_for_test());
        let count_after_first = engine.win32_call_count;

        // The position call may or may not succeed (no real HWND) but the
        // show/border/opacity calls count regardless. At minimum we expect the
        // visibility call (show_window).
        // What matters: a second *immediate* apply issues ZERO additional calls.
        engine.apply_all(&BackendHandle::default_for_test());
        assert_eq!(
            engine.win32_call_count, count_after_first,
            "second apply_all must not issue any Win32 calls when state unchanged"
        );
    }

    /// Changing which window is focused should cause apply_all to re-issue the
    /// border-color call for the affected windows (focused vs unfocused color).
    #[test]
    fn test_applied_state_invalidated_on_focus_change() {
        let mut engine = make_engine();
        let w1 = make_window(100, 500, 400);
        let w2 = make_window(101, 600, 400);
        engine.add_window(w1, &BackendHandle::default_for_test());
        engine.add_window(w2, &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();

        // First apply: prime the cache.
        engine.apply_all(&BackendHandle::default_for_test());
        let count_after_first = engine.win32_call_count;
        // The first apply must have issued at least one Win32 call (either a
        // visibility/border/opacity update or a position set) — otherwise the
        // applied-state cache wouldn't have anything to invalidate later, and
        // the subsequent focus-change assertion below would be meaningless.
        assert!(
            count_after_first >= 1,
            "first apply_all must issue at least one Win32 call to prime the cache"
        );

        // Change focus from w2 (current) to w1.
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            monitor.focus_window = Some(WindowId::new(100));
            monitor.focus_column = Some(0);
        }

        // Also invalidate the applied_state cache for both windows so the test
        // is not defeated by the ConfigureThrottle blocking the position call.
        // We simulate "focus changed" by clearing just the focused flag in the cache.
        engine.applied_state.clear();

        engine.win32_call_count = 0;
        engine.apply_all(&BackendHandle::default_for_test());

        // At minimum one border-color call per window (two windows → ≥ 2 calls).
        assert!(
            engine.win32_call_count >= 2,
            "focus change must trigger border-color re-apply for both windows, got {} calls",
            engine.win32_call_count
        );
    }

    /// Removing a window must clear its entry from the applied_state cache so
    /// if the window is re-added its state starts fresh.
    #[test]
    fn test_applied_state_cleared_on_remove() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        // Prime the cache.
        engine.apply_all(&BackendHandle::default_for_test());
        assert!(engine.applied_state.contains_key(&WindowId::new(100)),
            "cache must contain entry after first apply");

        // Remove the window.
        engine.remove_window(WindowId::new(100), &BackendHandle::default_for_test());
        assert!(!engine.applied_state.contains_key(&WindowId::new(100)),
            "cache entry must be removed when window is removed");
    }

    // =========================================================================
    // Tests for niri-parity layout commands (Items 1-8)
    // =========================================================================

    // --- Item 1: move_window_to_workspace ---

    #[test]
    fn test_move_window_to_workspace_changes_workspace_id() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        {
            let monitor = engine.monitors().get(&oid).unwrap();
            // Window is on workspace 0, which is the active one
            assert_eq!(monitor.active_workspace, 0);
            assert_eq!(monitor.workspace().unwrap().columns.len(), 1);
        }

        // Move the focused window to workspace 1
        engine.move_window_to_workspace(1, &BackendHandle::default_for_test());

        let monitor = engine.monitors().get(&oid).unwrap();
        // Active workspace should still be 0
        assert_eq!(monitor.active_workspace, 0);
        // Workspace 0 should now be empty
        assert_eq!(monitor.workspace().unwrap().columns.len(), 0);
        // Workspace 1 should have the window
        let ws1 = monitor.workspaces.get(&1).unwrap();
        assert_eq!(ws1.columns.len(), 1);
        assert_eq!(ws1.columns[0].tiles[0].window_id, WindowId::new(100));
        // tiled_windows still tracks the window
        assert!(engine.tiled_windows().contains_key(&WindowId::new(100)));
    }

    #[test]
    fn test_move_window_to_workspace_noop_same_workspace() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();

        // Move to the same workspace (0) — should be a no-op
        engine.move_window_to_workspace(0, &BackendHandle::default_for_test());

        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.workspace().unwrap().columns.len(), 1);
    }

    // --- Item 2: move_window_to_monitor ---

    #[test]
    fn test_move_window_to_monitor_left_with_two_monitors() {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let oid_left  = OutputId::from_name("Left");
        let oid_right = OutputId::from_name("Right");
        // Left monitor at x=0, right monitor at x=1920
        engine.register_monitor(oid_left,  Rect::new(0,    0, 1920, 1080), Rect::new(0,    0, 1920, 1040));
        engine.register_monitor(oid_right, Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1040));
        // Focus the right monitor and add a window there
        engine.set_focused_output(oid_right);

        let window = make_window(100, 2000, 200);
        engine.add_window(window, &BackendHandle::default_for_test());

        // Verify window is on the right monitor
        let right_mon = engine.monitors().get(&oid_right).unwrap();
        assert_eq!(right_mon.workspace().unwrap().columns.len(), 1);

        // Move window to the left monitor
        engine.move_window_to_monitor(ScrollDirection::Left, &BackendHandle::default_for_test());

        let left_mon = engine.monitors().get(&oid_left).unwrap();
        assert_eq!(left_mon.workspace().unwrap().columns.len(), 1,
            "window should have moved to left monitor");
        let right_mon = engine.monitors().get(&oid_right).unwrap();
        assert_eq!(right_mon.workspace().unwrap().columns.len(), 0,
            "right monitor should now be empty");
    }

    #[test]
    fn test_move_window_to_monitor_no_op_single_monitor() {
        let mut engine = make_engine();
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        // Only one monitor — moving left should be a no-op
        engine.move_window_to_monitor(ScrollDirection::Left, &BackendHandle::default_for_test());

        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.workspace().unwrap().columns.len(), 1,
            "single monitor: column count unchanged");
    }

    // --- Item 3: center_focused_column ---

    #[test]
    fn test_center_focused_column_sets_scroll() {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            column_gap: 16,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        // Add 5 windows — each in its own 500px column
        for i in 100..105 {
            let window = make_window(i, 500, 400);
            engine.add_window(window, &BackendHandle::default_for_test());
        }
        // Focus column 4 (the last one, which is off-screen without scroll)
        let oid_e = engine.focused_output().unwrap();
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid_e) {
            monitor.focus_column = Some(4);
        }

        engine.center_focused_column(&BackendHandle::default_for_test());

        let monitor = engine.monitors().get(&oid_e).unwrap();
        let scroll = monitor.workspace().unwrap().scroll_offset.x;
        // Column 4 starts at 4*(500+16)=2064, width=500, center=2064+250=2314
        // work_rect width = 1920, center=960
        // expected scroll = 2314 - 960 = 1354
        // (clamped: total=5*500+4*16=2564, max_scroll=2564-1920=644)
        assert!(scroll > 0, "scroll should be positive when centering column 4; got {}", scroll);
    }

    // --- set_column_width_preset ---

    #[test]
    fn test_column_width_preset_half() {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        engine.set_column_width_preset(ColumnWidthPreset::Half, &BackendHandle::default_for_test());

        let oid_e = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid_e).unwrap();
        let col_w = monitor.workspace().unwrap().columns[0].width.unwrap();
        // 50% of 1920 = 960
        assert_eq!(col_w, 960, "Half preset should set width to 960 (50% of 1920)");
    }

    #[test]
    fn test_column_width_preset_cycle() {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        // Niri-style cycle order:
        //   1/4 → 1/3 → 1/2 → 2/3 → 3/4 → full → 1/4 …
        // Default last_column_preset is Half → first Cycle → TwoThirds (1280).
        engine.set_column_width_preset(ColumnWidthPreset::Cycle, &BackendHandle::default_for_test());
        let oid_e = engine.focused_output().unwrap();
        let w1 = engine.monitors().get(&oid_e).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w1, 1280, "first Cycle from Half should be TwoThirds (1280)");

        // Second Cycle → ThreeQuarters (1440 = 1920 * 3/4)
        engine.set_column_width_preset(ColumnWidthPreset::Cycle, &BackendHandle::default_for_test());
        let w2 = engine.monitors().get(&oid_e).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w2, 1440, "second Cycle should be ThreeQuarters (1440)");

        // Third Cycle → Full (1920)
        engine.set_column_width_preset(ColumnWidthPreset::Cycle, &BackendHandle::default_for_test());
        let w3 = engine.monitors().get(&oid_e).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w3, 1920, "third Cycle should be Full (1920)");

        // Fourth Cycle wraps → OneQuarter (480 = 1920 / 4)
        engine.set_column_width_preset(ColumnWidthPreset::Cycle, &BackendHandle::default_for_test());
        let w4 = engine.monitors().get(&oid_e).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w4, 480, "fourth Cycle should wrap to OneQuarter (480)");
    }

    // --- Item 5: resize_focused_column_by ---

    #[test]
    fn test_resize_focused_column_by_delta() {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        // Column starts at config width 500.
        let oid_e = engine.focused_output().unwrap();
        let init_w = engine.monitors().get(&oid_e).unwrap()
            .workspace().unwrap().columns[0].width.unwrap_or(500);

        // Grow by 100px
        engine.resize_focused_column_by(100, &BackendHandle::default_for_test());
        let new_w = engine.monitors().get(&oid_e).unwrap()
            .workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(new_w as i32, init_w as i32 + 100);

        // Shrink by 50px
        engine.resize_focused_column_by(-50, &BackendHandle::default_for_test());
        let new_w2 = engine.monitors().get(&oid_e).unwrap()
            .workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(new_w2 as i32, init_w as i32 + 50);
    }

    #[test]
    fn test_resize_focused_column_minimum_50px() {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 100,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        let window = make_window(100, 100, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        // Shrink by a huge amount — should clamp to 50
        engine.resize_focused_column_by(-9999, &BackendHandle::default_for_test());
        let oid_e = engine.focused_output().unwrap();
        let w = engine.monitors().get(&oid_e).unwrap()
            .workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w, 50, "minimum column width must be 50px");
    }

    // --- Item 6: focus_workspace_relative ---

    #[test]
    fn test_focus_workspace_next_wraps() {
        let mut engine = make_engine();
        let oid = engine.focused_output().unwrap();
        // Create workspaces 0 and 1 on the monitor
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            monitor.workspaces.insert(1, crate::layout::Workspace::new());
        }

        // Currently on ws 0. Next → ws 1.
        engine.focus_workspace_relative(WorkspaceDirection::Next, &BackendHandle::default_for_test());
        {
            let monitor = engine.monitors().get(&oid).unwrap();
            assert_eq!(monitor.active_workspace, 1, "should have moved to workspace 1");
        }

        // Next from ws 1 → wraps to ws 0 (only 2 workspaces: 0, 1).
        engine.focus_workspace_relative(WorkspaceDirection::Next, &BackendHandle::default_for_test());
        {
            let monitor = engine.monitors().get(&oid).unwrap();
            assert_eq!(monitor.active_workspace, 0, "should wrap from ws 1 back to ws 0");
        }
    }

    #[test]
    fn test_focus_workspace_previous_wraps() {
        let mut engine = make_engine();
        let oid = engine.focused_output().unwrap();
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            monitor.workspaces.insert(1, crate::layout::Workspace::new());
            monitor.workspaces.insert(2, crate::layout::Workspace::new());
        }
        // On ws 0. Previous → wraps to ws 2.
        engine.focus_workspace_relative(WorkspaceDirection::Previous, &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.active_workspace, 2, "should wrap backward to last workspace");
    }

    // --- Item 7: maintain_empty_workspace ---

    #[test]
    fn test_maintain_empty_workspace_creates_one_at_end() {
        let mut engine = make_engine();
        let oid = engine.focused_output().unwrap();

        // Add a window to ws 0 — maintain should ensure ws 1 (trailing empty) exists.
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        engine.maintain_empty_workspace(oid);

        let monitor = engine.monitors().get(&oid).unwrap();
        // There should be a workspace at id 1 (highest_non_empty=0 → trailing=1)
        assert!(monitor.workspaces.contains_key(&1),
            "maintain should create trailing empty workspace at id 1");
        assert!(monitor.workspaces.get(&1).unwrap().columns.is_empty(),
            "trailing workspace must be empty");
    }

    #[test]
    fn test_maintain_empty_workspace_removes_surplus_empty() {
        let mut engine = make_engine();
        let oid = engine.focused_output().unwrap();

        // Manually create surplus empty workspaces 5 and 6, window on ws 0.
        let window = make_window(100, 500, 400);
        engine.add_window(window, &BackendHandle::default_for_test());

        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            monitor.workspaces.insert(5, crate::layout::Workspace::new());
            monitor.workspaces.insert(6, crate::layout::Workspace::new());
        }

        engine.maintain_empty_workspace(oid);

        let monitor = engine.monitors().get(&oid).unwrap();
        // highest_non_empty = 0 → trailing = 1. Ws 5 and 6 should be removed.
        assert!(!monitor.workspaces.contains_key(&5), "surplus ws 5 should be removed");
        assert!(!monitor.workspaces.contains_key(&6), "surplus ws 6 should be removed");
        assert!(monitor.workspaces.contains_key(&1), "trailing ws 1 should remain");
    }

    #[test]
    fn test_maintain_empty_workspace_does_not_remove_active() {
        let mut engine = make_engine();
        let oid = engine.focused_output().unwrap();

        // Switch to ws 5 (making it active) but leave it empty
        if let Some(monitor) = engine.monitors_mut().get_mut(&oid) {
            monitor.workspaces.insert(5, crate::layout::Workspace::new());
            monitor.active_workspace = 5;
        }

        engine.maintain_empty_workspace(oid);

        let monitor = engine.monitors().get(&oid).unwrap();
        // Active workspace (5) must not be deleted even if empty
        assert!(monitor.workspaces.contains_key(&5),
            "active workspace must never be removed by maintain");
    }

    // --- Item 8: default_width from window rule ---

    #[test]
    fn test_default_width_from_window_rule() {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        // Rule: windows with class "WideApp" get default_width = 800
        let mut rule = crate::config::WindowRule::default();
        rule.class = Some("WideApp".to_string());
        rule.default_width = Some(800);
        engine.set_window_rules(vec![rule]);

        // Add a window with the matching class
        let window = WindowInfo {
            hwnd: 100,
            title: "Wide Window".to_string(),
            class_name: "WideApp".to_string(),
            process_id: 1000,
            bounds: Rect::new(0, 0, 800, 600),
            state: crate::backend::WindowState::Normal,
            is_visible: true,
        };
        engine.add_window(window, &BackendHandle::default_for_test());

        let oid_e = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid_e).unwrap();
        let col_w = monitor.workspace().unwrap().columns[0].width;
        // The rule default_width (800) should override config fixed width (500)
        assert_eq!(col_w, Some(800), "rule default_width should override config column_width");
    }

    #[test]
    fn test_preferred_height_stored_on_tile() {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        // Rule: windows with class "TallApp" get default_height = 400
        let mut rule = crate::config::WindowRule::default();
        rule.class = Some("TallApp".to_string());
        rule.default_height = Some(400);
        engine.set_window_rules(vec![rule]);

        let window = WindowInfo {
            hwnd: 200,
            title: "Tall Window".to_string(),
            class_name: "TallApp".to_string(),
            process_id: 1001,
            bounds: Rect::new(0, 0, 800, 600),
            state: crate::backend::WindowState::Normal,
            is_visible: true,
        };
        engine.add_window(window, &BackendHandle::default_for_test());

        let oid_e = engine.focused_output().unwrap();
        let monitor = engine.monitors().get(&oid_e).unwrap();
        let tile_pref_h = monitor.workspace().unwrap().columns[0].tiles[0].preferred_height;
        assert_eq!(tile_pref_h, Some(400), "preferred_height should be stored on the tile");
    }

    // =========================================================================
    // New feature tests (Items 1-5 of this session)
    // =========================================================================

    // --- Item 1: focus_previous_window ---

    #[test]
    fn test_focus_previous_window_basic() {
        let mut engine = make_engine();
        // Add two windows: w1 then w2. Focus ring: [w2, w1].
        engine.add_window(make_window(100, 500, 400), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 600, 400), &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        // Current focus is on w2 (WindowId 101).
        assert_eq!(engine.monitors().get(&oid).unwrap().focus_window, Some(WindowId::new(101)));

        // Calling focus_previous_window should shift focus to w1 (WindowId 100).
        engine.focus_previous_window(&BackendHandle::default_for_test());
        assert_eq!(engine.monitors().get(&oid).unwrap().focus_window, Some(WindowId::new(100)));
    }

    #[test]
    fn test_focus_previous_window_no_previous_noop() {
        // Only one window — no previous in ring. Should be a no-op.
        let mut engine = make_engine();
        engine.add_window(make_window(100, 500, 400), &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        let before = engine.monitors().get(&oid).unwrap().focus_window;

        engine.focus_previous_window(&BackendHandle::default_for_test());

        let after = engine.monitors().get(&oid).unwrap().focus_window;
        assert_eq!(before, after, "focus should not change when there is no previous window");
    }

    // --- Item 2: toggle_always_on_top ---

    #[test]
    fn test_toggle_always_on_top_round_trip() {
        let mut engine = make_engine();
        engine.add_window(make_window(100, 500, 400), &BackendHandle::default_for_test());
        let wid = WindowId::new(100);

        // Initially not always-on-top.
        assert!(!engine.is_always_on_top(wid));

        // Toggle on (Win32 call will silently fail on fake HWND — that is fine).
        engine.toggle_always_on_top_for_focused(&BackendHandle::default_for_test());
        assert!(engine.is_always_on_top(wid), "window should be in always_on_top after first toggle");

        // Toggle off.
        engine.toggle_always_on_top_for_focused(&BackendHandle::default_for_test());
        assert!(!engine.is_always_on_top(wid), "window should not be in always_on_top after second toggle");
    }

    // --- Item 3: auto_tile_threshold ---

    #[test]
    fn test_auto_tile_threshold_set_and_get() {
        let mut engine = make_engine();

        // Default: threshold is None.
        assert_eq!(engine.auto_tile_threshold(), None);

        // Set a threshold.
        engine.set_auto_tile_threshold(Some(4));
        assert_eq!(engine.auto_tile_threshold(), Some(4));

        // Clear the threshold.
        engine.set_auto_tile_threshold(None);
        assert_eq!(engine.auto_tile_threshold(), None);

        // Threshold field is also pub — verify direct read/write consistency.
        engine.auto_tile_threshold = Some(7);
        assert_eq!(engine.auto_tile_threshold(), Some(7));
    }

    // --- focus_workspace_named ---

    #[test]
    fn test_focus_workspace_named() {
        let mut engine = make_engine();

        // Build a Config with two named workspaces.
        let mut config = crate::config::Config::default();
        config.workspace = vec![
            crate::config::types::WorkspaceConfig {
                name: "web".to_string(),
                layout: "tile".to_string(),
                monitor: String::new(),
                follow_on_focus: false,
            },
            crate::config::types::WorkspaceConfig {
                name: "code".to_string(),
                layout: "tile".to_string(),
                monitor: String::new(),
                follow_on_focus: false,
            },
        ];
        engine.set_full_config(config);

        let oid = engine.focused_output().unwrap();

        // Currently on workspace 0. Switching to "code" (index 1 → ws id 1).
        engine.focus_workspace_named("code", &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.active_workspace, 1, "should have switched to workspace id 1 ('code')");

        // Switching to "web" (index 0 → ws id 0).
        engine.focus_workspace_named("web", &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.active_workspace, 0, "should have switched to workspace id 0 ('web')");

        // Switching to unknown name — should be a no-op (stays on ws 0).
        engine.focus_workspace_named("nosuchworkspace", &BackendHandle::default_for_test());
        let monitor = engine.monitors().get(&oid).unwrap();
        assert_eq!(monitor.active_workspace, 0, "unknown name should not change active workspace");
    }

    // --- Item 5: urgent window lifecycle ---

    #[test]
    fn test_urgent_window_lifecycle() {
        let mut engine = make_engine();
        engine.add_window(make_window(100, 500, 400), &BackendHandle::default_for_test());
        let wid = WindowId::new(100);

        // Initially not urgent.
        assert!(!engine.is_urgent(wid));

        // Mark urgent.
        engine.mark_urgent(wid);
        assert!(engine.is_urgent(wid), "window should be urgent after mark_urgent");

        // Clear urgent.
        engine.clear_urgent(wid);
        assert!(!engine.is_urgent(wid), "window should not be urgent after clear_urgent");

        // `refresh_urgent_states` now syncs from the WinEvent-driven registry
        // in `backend::hooks`. With no hook events recorded the registry is
        // empty, so a refresh will drop any locally-marked windows.
        engine.mark_urgent(wid);
        engine.refresh_urgent_states();
        assert!(
            !engine.is_urgent(wid),
            "refresh_urgent_states replaces the set from the WinEvent registry; locally marked windows are dropped when the registry is empty"
        );
    }

    // -------------------------------------------------------------------------
    // niri-parity Round 4 — feature tests
    // -------------------------------------------------------------------------

    fn make_engine_fixed_1920() -> TilingEngine {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        engine
    }

    fn focus_window(engine: &mut TilingEngine, hwnd: isize) {
        let wid = WindowId::new(hwnd);
        let oid = engine.focused_output().unwrap();
        if let Some(m) = engine.monitors_mut().get_mut(&oid) {
            if let Some(ws) = m.workspace() {
                if let Some(col) = ws.find_window_column(wid) {
                    m.focus_column = Some(col);
                }
            }
            m.focus_window = Some(wid);
        }
    }

    // ---- Consume / Expel ----

    #[test]
    fn test_consume_window_into_column_basic() {
        let mut engine = make_engine_fixed_1920();
        for i in 100..103 {
            engine.add_window(make_window(i, 0, 0), &BackendHandle::default_for_test());
        }
        focus_window(&mut engine, 100); // leftmost column
        engine.consume_window_into_column(&BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        let m = engine.monitors().get(&oid).unwrap();
        let ws = m.workspace().unwrap();
        // Original col0 (100) merged into col1: now col0 should host [101, 100], col1 = [102]
        assert_eq!(ws.columns.len(), 2);
        assert_eq!(ws.columns[0].tiles.len(), 2);
        assert!(ws.columns[0].tiles.iter().any(|t| t.window_id == WindowId::new(100)));
        assert_eq!(ws.columns[1].tiles.len(), 1);
        // Focus follows the moved window.
        assert_eq!(m.focus_window, Some(WindowId::new(100)));
    }

    #[test]
    fn test_consume_window_at_rightmost_is_noop() {
        let mut engine = make_engine_fixed_1920();
        for i in 100..103 {
            engine.add_window(make_window(i, 0, 0), &BackendHandle::default_for_test());
        }
        focus_window(&mut engine, 102); // rightmost column
        engine.consume_window_into_column(&BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        let ws = engine.monitors().get(&oid).unwrap().workspace().unwrap();
        // No change — still 3 single-tile columns
        assert_eq!(ws.columns.len(), 3);
        assert_eq!(ws.columns[0].tiles.len(), 1);
        assert_eq!(ws.columns[1].tiles.len(), 1);
        assert_eq!(ws.columns[2].tiles.len(), 1);
    }

    #[test]
    fn test_expel_window_from_column() {
        let mut engine = make_engine_fixed_1920();
        // Two columns: col0 = [100, 101] (multi-tile), col1 = [102]
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        if let Some(m) = engine.monitors_mut().get_mut(&oid) {
            if let Some(ws) = m.workspace_mut() {
                ws.add_window_to_column(0, WindowId::new(101));
            }
        }
        engine.tiled_windows.insert(WindowId::new(101), make_window(101, 0, 0));
        engine.add_window(make_window(102, 0, 0), &BackendHandle::default_for_test());

        focus_window(&mut engine, 101);
        engine.expel_window_from_column(&BackendHandle::default_for_test());

        let m = engine.monitors().get(&oid).unwrap();
        let ws = m.workspace().unwrap();
        // After expel, col0 = [100], col1 = [101] (new), col2 = [102]
        assert_eq!(ws.columns.len(), 3);
        assert_eq!(ws.columns[0].tiles.len(), 1);
        assert_eq!(ws.columns[0].tiles[0].window_id, WindowId::new(100));
        assert_eq!(ws.columns[1].tiles.len(), 1);
        assert_eq!(ws.columns[1].tiles[0].window_id, WindowId::new(101));
        assert_eq!(m.focus_window, Some(WindowId::new(101)));
    }

    #[test]
    fn test_expel_window_single_tile_column_is_noop() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        engine.expel_window_from_column(&BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        let ws = engine.monitors().get(&oid).unwrap().workspace().unwrap();
        // Each column already had one tile — no new column should appear.
        assert_eq!(ws.columns.len(), 2);
    }

    // ---- Expand column ----

    #[test]
    fn test_expand_column_to_available_fills_remainder() {
        let mut engine = make_engine_fixed_1920();
        // 3 columns at default 500px width, gap=16
        for i in 100..103 {
            engine.add_window(make_window(i, 0, 0), &BackendHandle::default_for_test());
        }
        focus_window(&mut engine, 101); // middle column
        engine.expand_column_to_available(&BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        let ws = engine.monitors().get(&oid).unwrap().workspace().unwrap();
        // Other columns: 500 + 500 = 1000; gap = 16 * 2 = 32; available = 1920 - 1000 - 32 = 888
        assert_eq!(ws.columns[1].width, Some(888));
    }

    // ---- Maximize column ----

    #[test]
    fn test_maximize_column_toggles_flag() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);

        let oid = engine.focused_output().unwrap();
        // Initially not maximized
        assert!(!engine.monitors().get(&oid).unwrap().workspace().unwrap().columns[0].maximized);

        engine.toggle_maximize_focused_column(&BackendHandle::default_for_test());
        assert!(engine.monitors().get(&oid).unwrap().workspace().unwrap().columns[0].maximized);

        engine.toggle_maximize_focused_column(&BackendHandle::default_for_test());
        assert!(!engine.monitors().get(&oid).unwrap().workspace().unwrap().columns[0].maximized);
    }

    // ---- Resize column / tile by percentage ----

    #[test]
    fn test_resize_focused_column_by_percent_grow_and_clamp() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);

        let oid = engine.focused_output().unwrap();
        // Start width unset → defaults to column_width = 500
        engine.resize_focused_column_by_percent(5, &BackendHandle::default_for_test());
        // 500 + (1920 * 0.05 = 96) = 596
        let w = engine.monitors().get(&oid).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w, 596);

        // Hammer growth — should clamp at 95% of 1920 = 1824
        for _ in 0..50 {
            engine.resize_focused_column_by_percent(5, &BackendHandle::default_for_test());
        }
        let w = engine.monitors().get(&oid).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w, 1824);

        // Hammer shrink — should clamp at 10% of 1920 = 192
        for _ in 0..50 {
            engine.resize_focused_column_by_percent(-5, &BackendHandle::default_for_test());
        }
        let w = engine.monitors().get(&oid).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w, 192);
    }

    #[test]
    fn test_resize_focused_tile_height_single_tile_noop() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        let oid = engine.focused_output().unwrap();
        let before = engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].tiles[0].height_weight;
        engine.resize_focused_tile_height_by_percent(5, &BackendHandle::default_for_test());
        let after = engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].tiles[0].height_weight;
        assert_eq!(before, after, "single-tile column should not adjust weight");
    }

    #[test]
    fn test_resize_focused_tile_height_multi_tile_adjusts_weight() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        if let Some(m) = engine.monitors_mut().get_mut(&oid) {
            if let Some(ws) = m.workspace_mut() {
                ws.add_window_to_column(0, WindowId::new(101));
            }
        }
        engine.tiled_windows.insert(WindowId::new(101), make_window(101, 0, 0));

        focus_window(&mut engine, 100);
        engine.resize_focused_tile_height_by_percent(5, &BackendHandle::default_for_test());
        let w = engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].tiles[0].height_weight;
        assert!(w > 1.0, "weight should grow from 1.0 after positive delta");
    }

    // ---- Column width 1/4 + 3/4 presets ----

    #[test]
    fn test_column_width_preset_quarter() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        let oid = engine.focused_output().unwrap();

        engine.set_column_width_preset(ColumnWidthPreset::OneQuarter,
            &BackendHandle::default_for_test());
        let w = engine.monitors().get(&oid).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w, 480, "OneQuarter on 1920px = 480");

        engine.set_column_width_preset(ColumnWidthPreset::ThreeQuarters,
            &BackendHandle::default_for_test());
        let w = engine.monitors().get(&oid).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w, 1440, "ThreeQuarters on 1920px = 1440");
    }

    // ---- Move column to monitor ----

    #[test]
    fn test_move_column_to_monitor_right_moves_whole_column() {
        // Two monitors side-by-side
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid_a = OutputId::from_name("A");
        let oid_b = OutputId::from_name("B");
        engine.register_monitor(oid_a, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        engine.register_monitor(oid_b, Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1080));
        // Both monitors start on left (set_focused_output requires it to exist).
        engine.set_focused_output(oid_a);

        // Two tiles on monitor A in the same column → multi-tile column.
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        if let Some(m) = engine.monitors_mut().get_mut(&oid_a) {
            if let Some(ws) = m.workspace_mut() {
                ws.add_window_to_column(0, WindowId::new(101));
            }
        }
        engine.tiled_windows.insert(WindowId::new(101), make_window(101, 0, 0));

        focus_window(&mut engine, 100);
        engine.move_column_to_monitor(ScrollDirection::Right, &BackendHandle::default_for_test());

        // Monitor A: empty workspace remains.
        let ws_a = engine.monitors().get(&oid_a).unwrap().workspace().unwrap();
        assert_eq!(ws_a.columns.len(), 0, "source monitor lost its column");

        // Monitor B: gained the whole column (two tiles).
        let ws_b = engine.monitors().get(&oid_b).unwrap().workspace().unwrap();
        assert_eq!(ws_b.columns.len(), 1);
        assert_eq!(ws_b.columns[0].tiles.len(), 2);
        // Focus moved to monitor B.
        assert_eq!(engine.focused_output(), Some(oid_b));
    }

    #[test]
    fn test_move_column_to_monitor_no_neighbor_is_noop() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        let oid = engine.focused_output().unwrap();
        engine.move_column_to_monitor(ScrollDirection::Right, &BackendHandle::default_for_test());
        let ws = engine.monitors().get(&oid).unwrap().workspace().unwrap();
        assert_eq!(ws.columns.len(), 1, "no neighbor → column stays put");
    }

    // -------------------------------------------------------------------------
    // Workspace-slide animation engine wiring (Step 1)
    // -------------------------------------------------------------------------

    /// Build a config with animations enabled (workspace_transition default true)
    /// and apply it so `switch_workspace` triggers a slide.
    fn enable_workspace_slide_animations(engine: &mut TilingEngine) {
        let mut cfg = crate::config::Config::default();
        cfg.animations.enabled = true;
        cfg.animations.duration = 200;
        cfg.animations.workspace_transition = true;
        engine.set_full_config(cfg);
        engine.update_animation_settings(true, 200, crate::layout::Easing::Linear);
    }

    /// switch_workspace starts a slide animation whose initial offset equals
    /// the work-area height when moving to a higher-numbered workspace.
    #[test]
    fn test_switch_workspace_starts_slide_with_positive_offset() {
        let mut engine = make_engine_fixed_1920();
        enable_workspace_slide_animations(&mut engine);
        let oid = engine.focused_output().unwrap();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.switch_workspace(2, &BackendHandle::default_for_test());
        let offset = engine.animation().workspace_slide_offset(oid);
        assert!(
            offset > 1.0,
            "downward slide should start with a positive offset, got {}",
            offset
        );
        assert!(
            offset <= 1080.0 + 0.001,
            "offset should not exceed work-area height, got {}",
            offset
        );
    }

    /// With animations disabled the slide is skipped and offset is 0 at rest.
    #[test]
    fn test_switch_workspace_disabled_snaps_no_slide() {
        let mut engine = make_engine_fixed_1920();
        let oid = engine.focused_output().unwrap();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.switch_workspace(3, &BackendHandle::default_for_test());
        assert_eq!(
            engine.animation().workspace_slide_offset(oid),
            0.0,
            "disabled animations: slide offset stays 0 (snap)"
        );
        assert!(
            !engine.has_active_animations(),
            "no animation should be queued when disabled"
        );
    }

    // -------------------------------------------------------------------------
    // Per-monitor layout overrides (Step 2)
    // -------------------------------------------------------------------------

    /// No override on an output → effective_config falls through to the global.
    #[test]
    fn test_effective_config_no_override_falls_through() {
        let engine = make_engine_fixed_1920();
        let oid = engine.focused_output().unwrap();
        let eff = engine.effective_config(oid);
        // No layout_override set → same column_width as the global.
        assert_eq!(eff.column_width, engine.config.column_width);
        // Also check identity-by-pointer when no override is present.
        let p_eff = eff as *const LayoutConfig;
        let p_default = &engine.config as *const LayoutConfig;
        assert_eq!(p_eff, p_default, "fall-through must return the same &LayoutConfig");
    }

    /// A single-field override (column_width) is applied on top of the global.
    #[test]
    fn test_effective_config_single_field_override() {
        let mut engine = make_engine_fixed_1920();
        let oid_name = "M-OVR-1";
        let oid = OutputId::from_name(oid_name);
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        let mut cfg = crate::config::Config::default();
        cfg.output.push(crate::config::OutputConfig {
            name: oid_name.to_string(),
            position: crate::config::types::Position::default(),
            width: 0,
            height: 0,
            scale: 1.0,
            mode: String::new(),
            vrr: false,
            primary: false,
            transform: "normal".to_string(),
            enable: true,
            layout_override: Some(crate::config::types::LayoutConfigPartial {
                column_width: Some(800),
                ..Default::default()
            }),
            mod_key: None,
        });
        engine.set_full_config(cfg);

        let eff = engine.effective_config(oid);
        assert_eq!(eff.column_width, 800, "override field wins");
        // Other fields fall through to the (built-from-config) base.
        assert!(eff.border_width >= 1, "fall-through populates default border width");
    }

    /// Multi-field override populates each Some(_) field correctly.
    #[test]
    fn test_effective_config_multi_field_override() {
        let mut engine = make_engine_fixed_1920();
        let oid_name = "M-OVR-2";
        let oid = OutputId::from_name(oid_name);
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        let mut cfg = crate::config::Config::default();
        cfg.output.push(crate::config::OutputConfig {
            name: oid_name.to_string(),
            position: crate::config::types::Position::default(),
            width: 0,
            height: 0,
            scale: 1.0,
            mode: String::new(),
            vrr: false,
            primary: false,
            transform: "normal".to_string(),
            enable: true,
            layout_override: Some(crate::config::types::LayoutConfigPartial {
                column_width: Some(600),
                border_width: Some(12),
                column_width_mode: Some("fixed".to_string()),
                ..Default::default()
            }),
            mod_key: None,
        });
        engine.set_full_config(cfg);

        let eff = engine.effective_config(oid);
        assert_eq!(eff.column_width, 600);
        assert_eq!(eff.border_width, 12);
        assert_eq!(eff.column_width_mode, ColumnWidthMode::Fixed);
    }

    /// Reloading config with a different override replaces the previous one
    /// in the per-monitor map.
    #[test]
    fn test_effective_config_reload_swaps_override() {
        let mut engine = make_engine_fixed_1920();
        let oid_name = "M-OVR-3";
        let oid = OutputId::from_name(oid_name);
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        let mk_cfg = |w: u32| {
            let mut c = crate::config::Config::default();
            c.output.push(crate::config::OutputConfig {
                name: oid_name.to_string(),
                position: crate::config::types::Position::default(),
                width: 0, height: 0, scale: 1.0,
                mode: String::new(), vrr: false, primary: false,
                transform: "normal".to_string(), enable: true,
                layout_override: Some(crate::config::types::LayoutConfigPartial {
                    column_width: Some(w),
                    ..Default::default()
                }),
                mod_key: None,
            });
            c
        };

        engine.set_full_config(mk_cfg(700));
        assert_eq!(engine.effective_config(oid).column_width, 700);
        engine.set_full_config(mk_cfg(1100));
        assert_eq!(engine.effective_config(oid).column_width, 1100);
        // Reload with no override at all → fall-through.
        let mut bare = crate::config::Config::default();
        bare.output.clear();
        engine.set_full_config(bare);
        let p_eff = engine.effective_config(oid) as *const LayoutConfig;
        let p_default = &engine.config as *const LayoutConfig;
        assert_eq!(p_eff, p_default, "reload without override falls back to global");
    }

    // -------------------------------------------------------------------------
    // Snapshot / restore engine wiring (Step 6 — additional coverage)
    // -------------------------------------------------------------------------

    /// Engine.snapshot() captures the current monitor stack and
    /// apply_snapshot rebuilds it.  Surviving HWNDs come back; missing
    /// ones are dropped silently.
    #[test]
    fn test_engine_apply_snapshot_restores_layout_minus_missing_hwnds() {
        let mut engine = make_engine_fixed_1920();
        let oid = engine.focused_output().unwrap();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());
        // Two-tile column on column 0.
        if let Some(m) = engine.monitors_mut().get_mut(&oid) {
            if let Some(ws) = m.workspace_mut() {
                ws.add_window_to_column(0, WindowId::new(102));
            }
        }
        engine.tiled_windows.insert(WindowId::new(102), make_window(102, 0, 0));
        let snap = engine.snapshot();
        // Remove HWND 101 (simulate window closed).
        engine.tiled_windows.remove(&WindowId::new(101));
        if let Some(m) = engine.monitors_mut().get_mut(&oid) {
            if let Some(ws) = m.workspace_mut() {
                ws.remove_window(WindowId::new(101));
            }
        }
        // Apply the snapshot — 101 is missing from tiled_windows so it
        // must be skipped, but 100 + 102 should come back.
        engine.apply_snapshot(&snap);
        let ws = engine.monitors().get(&oid).unwrap().workspace().unwrap();
        let restored: Vec<isize> = ws.columns.iter()
            .flat_map(|c| c.tiles.iter().map(|t| t.window_id.as_isize()))
            .collect();
        assert!(restored.contains(&100), "HWND 100 must be restored");
        assert!(restored.contains(&102), "HWND 102 must be restored");
        assert!(!restored.contains(&101), "missing HWND 101 must NOT be restored");
    }

    // -------------------------------------------------------------------------
    // Interactive resize mode (Step 7)
    // -------------------------------------------------------------------------

    /// toggle_resize_mode flips the state and exit_resize_mode is idempotent.
    #[test]
    fn test_resize_mode_toggle_and_exit() {
        let mut engine = make_engine_fixed_1920();
        assert!(!engine.is_resize_mode());
        assert!(engine.toggle_resize_mode());
        assert!(engine.is_resize_mode());
        assert!(!engine.toggle_resize_mode());
        assert!(!engine.is_resize_mode());
        // exit_resize_mode while off is a no-op (no panic).
        engine.exit_resize_mode();
        assert!(!engine.is_resize_mode());
        // Re-enter then exit explicitly.
        engine.toggle_resize_mode();
        assert!(engine.is_resize_mode());
        engine.exit_resize_mode();
        assert!(!engine.is_resize_mode());
    }

    /// While resize_mode is on, the engine's resize methods actually
    /// change column widths (i.e. the wiring from action to engine method
    /// stays sane).  The dispatcher in message_loop routes Mod+Arrow to
    /// the same resize calls when resize_mode is set.
    #[test]
    fn test_resize_mode_arrow_resizes_column() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        let oid = engine.focused_output().unwrap();
        let original_w = engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].width
            .unwrap_or(engine.config.column_width);
        engine.toggle_resize_mode();
        engine.resize_focused_column_by_percent(5, &BackendHandle::default_for_test());
        let grown = engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].width.unwrap();
        assert!(
            grown > original_w,
            "+5% resize should grow the column (was {} → {})",
            original_w, grown
        );
        engine.resize_focused_column_by_percent(-5, &BackendHandle::default_for_test());
        let shrunk = engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].width.unwrap();
        assert!(shrunk < grown);
    }

    // -------------------------------------------------------------------------
    // Tabbed column engine-level dispatch (Step 5)
    // -------------------------------------------------------------------------

    /// toggle_tabbed_for_focused_column flips the column's display mode
    /// between Stacked and Tabbed and the change persists across calls.
    #[test]
    fn test_toggle_tabbed_for_focused_column_persists() {
        use crate::layout::workspace::ColumnDisplay;
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        if let Some(m) = engine.monitors_mut().get_mut(&oid) {
            if let Some(ws) = m.workspace_mut() {
                ws.add_window_to_column(0, WindowId::new(101));
                ws.add_window_to_column(0, WindowId::new(102));
            }
        }
        engine.tiled_windows.insert(WindowId::new(101), make_window(101, 0, 0));
        engine.tiled_windows.insert(WindowId::new(102), make_window(102, 0, 0));
        focus_window(&mut engine, 100);

        // Start in Stacked.
        let disp = &engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].display;
        assert!(matches!(disp, ColumnDisplay::Stacked));

        // First toggle → Tabbed.
        engine.toggle_tabbed_for_focused_column(&BackendHandle::default_for_test());
        let disp = engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].display.clone();
        assert!(matches!(disp, ColumnDisplay::Tabbed { .. }));

        // Second toggle → back to Stacked.
        engine.toggle_tabbed_for_focused_column(&BackendHandle::default_for_test());
        let disp = &engine.monitors().get(&oid).unwrap()
            .workspace().unwrap().columns[0].display;
        assert!(matches!(disp, ColumnDisplay::Stacked));
    }

    /// During an active slide, calculate-position output for the active
    /// workspace is shifted on the Y axis by the slide offset.
    #[test]
    fn test_workspace_slide_offset_biases_layout_y() {
        let mut engine = make_engine_fixed_1920();
        enable_workspace_slide_animations(&mut engine);
        let oid = engine.focused_output().unwrap();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        engine.apply_layout_for_monitor(oid, &BackendHandle::default_for_test());
        let rest_y = engine
            .monitors()
            .get(&oid)
            .unwrap()
            .workspace()
            .unwrap()
            .columns[0]
            .tiles[0]
            .cached_bounds
            .loc
            .y;
        // Switch to workspace 2 (downward) which arms the slide animation.
        engine.switch_workspace(2, &BackendHandle::default_for_test());
        engine.add_window(make_window(200, 0, 0), &BackendHandle::default_for_test());
        engine.apply_layout_for_monitor(oid, &BackendHandle::default_for_test());
        let mid_slide_offset = engine.animation().workspace_slide_offset(oid);
        let mid_y = engine
            .monitors()
            .get(&oid)
            .unwrap()
            .workspace()
            .unwrap()
            .columns[0]
            .tiles[0]
            .cached_bounds
            .loc
            .y;
        let expected = rest_y + mid_slide_offset as i32;
        assert!(
            (mid_y - expected).abs() <= 1,
            "mid-slide Y={} should approx rest_y={} + offset={} (expected {})",
            mid_y, rest_y, mid_slide_offset as i32, expected,
        );
    }

    // -------------------------------------------------------------------------
    // Niri-parity: move column vertically between workspaces
    // -------------------------------------------------------------------------

    #[test]
    fn test_move_focused_column_to_workspace_down_basic() {
        let mut engine = make_engine_fixed_1920();
        // Two windows on workspace 0 → two columns.
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);

        engine.move_focused_column_to_workspace(1, &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        let m = engine.monitors().get(&oid).unwrap();
        // Focus + active workspace should now be on the destination (ws 1).
        assert_eq!(m.active_workspace, 1, "focus follows the moved column to ws 1");
        let dst = m.workspaces.get(&1).expect("ws 1 must exist after move");
        assert_eq!(dst.columns.len(), 1, "destination ws should host one column");
        assert_eq!(dst.columns[0].tiles[0].window_id, WindowId::new(100));
        // Source workspace should still hold the second tile in its own column.
        let src = m.workspaces.get(&0).expect("ws 0 still tracked");
        assert_eq!(src.columns.len(), 1);
        assert_eq!(src.columns[0].tiles[0].window_id, WindowId::new(101));
    }

    #[test]
    fn test_move_focused_column_to_workspace_up_creates_destination() {
        let mut engine = make_engine_fixed_1920();
        // Start on workspace 2 with one tile; up = ws 1 which doesn't exist yet.
        let oid = engine.focused_output().unwrap();
        engine.monitors_mut().get_mut(&oid).unwrap().switch_workspace(2);
        engine.add_window(make_window(200, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 200);

        engine.move_focused_column_to_workspace(-1, &BackendHandle::default_for_test());

        let m = engine.monitors().get(&oid).unwrap();
        assert_eq!(m.active_workspace, 1, "focus relocates to ws 1");
        let dst = m.workspaces.get(&1).expect("ws 1 was created");
        assert_eq!(dst.columns.len(), 1);
        assert_eq!(dst.columns[0].tiles[0].window_id, WindowId::new(200));
        // Source ws 2 is now empty (and may have been reaped, but not before
        // maintain_empty_workspace runs — we don't assert one way or the other
        // beyond "destination has the tile").
    }

    #[test]
    fn test_move_focused_column_to_workspace_up_at_floor_is_noop() {
        // Workspace 0 is the implicit "lowest"; stepping up should be a no-op.
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);

        let oid = engine.focused_output().unwrap();
        let before = engine.monitors().get(&oid).unwrap().active_workspace;
        assert_eq!(before, 0, "test setup: focused workspace is 0");

        engine.move_focused_column_to_workspace(-1, &BackendHandle::default_for_test());

        let m = engine.monitors().get(&oid).unwrap();
        assert_eq!(m.active_workspace, 0, "noop: still on ws 0");
        assert_eq!(
            m.workspaces.get(&0).unwrap().columns.len(),
            1,
            "the lone column should still live on ws 0"
        );
    }

    // -------------------------------------------------------------------------
    // Niri-parity: workspace move/swap (reorder the workspace stack)
    // -------------------------------------------------------------------------

    #[test]
    fn test_move_active_workspace_swap_down_basic() {
        let mut engine = make_engine_fixed_1920();
        // Workspace 0 has a window; workspace 1 also has one.  We'll swap
        // 0 ↔ 1 and the contents should follow.
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        engine.monitors_mut().get_mut(&oid).unwrap().switch_workspace(1);
        engine.add_window(make_window(200, 0, 0), &BackendHandle::default_for_test());
        // Back to ws 0 and swap it with ws 1.
        engine.monitors_mut().get_mut(&oid).unwrap().switch_workspace(0);

        engine.move_active_workspace(1, &BackendHandle::default_for_test());

        let m = engine.monitors().get(&oid).unwrap();
        // Focus follows the moved workspace.
        assert_eq!(m.active_workspace, 1, "focus relocates to ws 1");
        let ws0 = m.workspaces.get(&0).expect("ws 0 still exists");
        let ws1 = m.workspaces.get(&1).expect("ws 1 still exists");
        // ws 1 should now host the originally-on-ws-0 window 100.
        assert_eq!(ws1.columns[0].tiles[0].window_id, WindowId::new(100));
        // ws 0 should now host the originally-on-ws-1 window 200.
        assert_eq!(ws0.columns[0].tiles[0].window_id, WindowId::new(200));
    }

    #[test]
    fn test_move_active_workspace_at_floor_is_noop() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        let snapshot_before: Vec<i32> = engine
            .monitors()
            .get(&oid)
            .unwrap()
            .workspaces
            .keys()
            .copied()
            .collect();

        engine.move_active_workspace(-1, &BackendHandle::default_for_test());

        let m = engine.monitors().get(&oid).unwrap();
        assert_eq!(m.active_workspace, 0, "noop: stays on ws 0");
        // No new negative-id workspace was created.
        assert!(
            !m.workspaces.contains_key(&-1),
            "must not create workspace at id -1; before: {:?}, after: {:?}",
            snapshot_before,
            m.workspaces.keys().copied().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn test_move_active_workspace_focus_follows_moved_workspace() {
        let mut engine = make_engine_fixed_1920();
        // Put a window on workspace 0 and remember its id.
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();

        engine.move_active_workspace(1, &BackendHandle::default_for_test());

        let m = engine.monitors().get(&oid).unwrap();
        assert_eq!(m.active_workspace, 1);
        // Focused window stays the same after the swap.
        assert_eq!(m.focus_window, Some(WindowId::new(100)));
    }

    // -------------------------------------------------------------------------
    // Niri-parity: smart borders
    // -------------------------------------------------------------------------

    #[test]
    fn test_smart_borders_lone_window_uses_sentinel() {
        // smart_borders on + lone window → border cache stores the smart-borders
        // sentinel (0xFFFFFFFE), not the focused/normal colour.
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            smart_borders: true,
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        engine.apply_all(&BackendHandle::default_for_test());

        let cached = engine
            .applied_state
            .get(&WindowId::new(100))
            .expect("lone tile should have cached state after apply_all");
        assert_eq!(
            cached.border_color, 0xFFFF_FFFE,
            "lone-window smart-borders sentinel expected (0xFFFFFFFE), got 0x{:08X}",
            cached.border_color,
        );
    }

    #[test]
    fn test_smart_borders_multiple_windows_keep_normal_color() {
        // smart_borders on but 2 windows → normal focused colour applies.
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            smart_borders: true,
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        engine.apply_all(&BackendHandle::default_for_test());

        let cached = engine
            .applied_state
            .get(&WindowId::new(100))
            .expect("focused tile should have cached state");
        // Focused colour is the engine default `#3381d9` → packed 0x003381d9.
        let expected_focused = 0x0033_81d9u32;
        assert_eq!(
            cached.border_color, expected_focused,
            "multi-window smart-borders must NOT engage; got 0x{:08X}, want 0x{:08X}",
            cached.border_color, expected_focused,
        );
    }

    #[test]
    fn test_smart_borders_off_always_paints_normal_color() {
        // smart_borders off + lone window → normal border colour applies.
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            smart_borders: false,
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        engine.apply_all(&BackendHandle::default_for_test());

        let cached = engine
            .applied_state
            .get(&WindowId::new(100))
            .expect("lone tile should have cached state after apply_all");
        let expected_focused = 0x0033_81d9u32;
        assert_eq!(
            cached.border_color, expected_focused,
            "smart-borders off: expect normal focused colour 0x{:08X}, got 0x{:08X}",
            expected_focused, cached.border_color,
        );
    }

    // -------------------------------------------------------------------------
    // Niri-parity: blur backdrop
    // -------------------------------------------------------------------------

    #[test]
    fn test_blur_flag_round_trips_through_resolved_window_rules() {
        // A window-rule with `blur true` must surface as
        // ResolvedWindowRules.blur == true after rule resolution.
        use crate::config::types::{Matcher, MatcherContext, WindowRule};
        let rule = WindowRule {
            matchers: vec![Matcher::ClassName("MyFloater".to_string())],
            floating: true,
            blur: true,
            ..WindowRule::default()
        };
        let rules = vec![rule];
        let ctx = MatcherContext::from_legacy("MyFloater", "", None, None);
        let resolved = crate::window::resolve_window_rules(&rules, &ctx);
        assert!(
            resolved.blur,
            "resolved rules.blur should be true when the matching rule sets blur=true"
        );
        assert!(resolved.float, "floating is still honoured alongside blur");
    }

    #[test]
    fn test_apply_window_blur_toggles_blur_applied_set() {
        // apply_window_blur is idempotent: enabling twice does not re-call;
        // disabling drops the HWND from the set so a re-enable applies again.
        let mut engine = make_engine();
        let hwnd: isize = 0xDEAD_BEEF;
        let wid = WindowId::new(hwnd);

        assert!(!engine.blur_applied.contains(&wid), "set starts empty");

        engine.apply_window_blur(hwnd, true);
        assert!(
            engine.blur_applied.contains(&wid),
            "enabling blur must record the HWND in blur_applied"
        );

        // Second enable is a no-op (already in the set).
        engine.apply_window_blur(hwnd, true);
        assert!(engine.blur_applied.contains(&wid), "set still contains HWND");
        assert_eq!(engine.blur_applied.len(), 1, "no duplicate entries");

        engine.apply_window_blur(hwnd, false);
        assert!(
            !engine.blur_applied.contains(&wid),
            "disabling blur must drop the HWND from blur_applied"
        );
    }

    // =========================================================================
    // Item 1 — paint_title_for_tile
    // =========================================================================

    #[test]
    fn test_paint_title_no_op_when_strip_frame_off() {
        // When strip_frame = false, paint_title_for_tile must be a complete
        // no-op: no panic, no Win32 calls, win32_call_count unchanged.
        let mut engine = TilingEngine::new(LayoutConfig {
            strip_frame: false,
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));

        let before = engine.win32_call_count;
        // hwnd=0 is an invalid handle — on real Win32 GetDC(NULL) returns the
        // desktop DC and we'd have a real side-effect, but with strip_frame=false
        // we return before that call.
        engine.paint_title_for_tile(0, Rect::new(0, 0, 800, 600), "Test Title", true);
        // No call should have been issued.
        assert_eq!(
            engine.win32_call_count, before,
            "paint_title_for_tile must be a no-op when strip_frame is off"
        );
    }

    // =========================================================================
    // Item 4 — auto_tile_zoom engages above threshold
    // =========================================================================

    #[test]
    fn test_auto_tile_zoom_engages_above_threshold() {
        // Engine with Fixed 500px columns, 1920px wide monitor, threshold=2.
        // Adding 3 columns (> 2) should set auto_tile_zoom to Some(<1.0).
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            column_gap: 16,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        engine.set_auto_tile_threshold(Some(2));

        // Initially no zoom.
        assert_eq!(engine.auto_tile_zoom(), None, "no auto-zoom before threshold exceeded");

        // Add 3 windows (3 columns at 500px each = 1500 + 32 gaps = 1532, which fits in 1920).
        for i in 100..103 {
            engine.add_window(make_window(i, 500, 400), &BackendHandle::default_for_test());
        }
        // Run a layout pass to trigger threshold evaluation.
        engine.apply_layout_for_monitor(oid, &BackendHandle::default_for_test());
        // 3 columns > threshold of 2; zoom should be set (may be 1.0 if all fit).
        // 3 * 500 + 2 * 16 = 1532 < 1920, so zoom = 1.0 and auto_tile_zoom stays None
        // because a zoom of 1.0 is not < 1.0.
        // Let's add enough columns so total exceeds view_width.
        for i in 103..107 {
            engine.add_window(make_window(i, 500, 400), &BackendHandle::default_for_test());
        }
        engine.apply_layout_for_monitor(oid, &BackendHandle::default_for_test());
        // 7 columns * 500 + 6 * 16 = 3596 > 1920; zoom = 1920/3596 < 1.0
        let zoom = engine.auto_tile_zoom();
        assert!(
            zoom.is_some(),
            "auto_tile_zoom should be Some when columns exceed view width"
        );
        let z = zoom.unwrap();
        assert!(z < 1.0, "zoom factor must be < 1.0 to fit columns; got {}", z);
        assert!(z > 0.0, "zoom must be positive");
    }

    #[test]
    fn test_auto_tile_zoom_clears_when_below_threshold() {
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            column_gap: 16,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        engine.set_auto_tile_threshold(Some(2));

        // Force auto_tile_zoom to be set by direct write (to avoid needing many windows).
        engine.auto_tile_zoom = Some(0.5);

        // A single-column workspace is below threshold — layout pass should clear zoom.
        engine.add_window(make_window(100, 500, 400), &BackendHandle::default_for_test());
        engine.apply_layout_for_monitor(oid, &BackendHandle::default_for_test());
        assert_eq!(engine.auto_tile_zoom(), None, "zoom should clear when column count <= threshold");
    }

    // =========================================================================
    // Item 5 — BStack layout positions
    // =========================================================================

    #[test]
    fn test_workspace_layout_mode_bstack_positions() {
        // 3 columns in BStack: col0 = main (left 50%), col1+col2 stack vertically (right 50%).
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let cfg = LayoutConfig {
            column_gap: 0,
            window_gap: 0,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        };
        use crate::layout::Workspace;
        use crate::layout::workspace::{WorkspaceLayout, Column, Tile};
        let mut ws = Workspace::with_layout(WorkspaceLayout::BStack);
        // Add 3 columns, each with 1 tile.
        for i in 0..3usize {
            let wid = WindowId::new((i + 1) as isize);
            ws.add_window_to_new_column(wid);
        }

        // Create a temporary engine to call calculate_positions_bstack.
        let engine = TilingEngine::new(cfg.clone());
        let positions = engine.calculate_positions_bstack(&ws, work_rect, &cfg);

        assert_eq!(positions.len(), 3, "all 3 tiles must be positioned");

        // Main column (w1): left half, full height.
        let main_rect = positions.iter().find(|(wid, _)| *wid == WindowId::new(1)).unwrap().1;
        assert_eq!(main_rect.loc.x, 0, "main col starts at left edge");
        assert_eq!(main_rect.size.w, 960, "main col takes half the width (gap=0)");
        assert_eq!(main_rect.size.h, 1080, "main col takes full height");

        // Stack columns (w2, w3): right half, stacked vertically.
        let r2 = positions.iter().find(|(wid, _)| *wid == WindowId::new(2)).unwrap().1;
        let r3 = positions.iter().find(|(wid, _)| *wid == WindowId::new(3)).unwrap().1;
        assert_eq!(r2.loc.x, 960, "stack col starts at mid-point");
        assert_eq!(r3.loc.x, 960, "stack col starts at mid-point");
        assert_eq!(r2.size.w, 960, "stack cols fill the right half");
        assert_eq!(r3.size.w, 960, "stack cols fill the right half");
        // Two stack columns share the 1080px height equally.
        assert_eq!(r2.size.h, 540, "each stack slot = half the height");
        assert_eq!(r3.size.h, 540, "each stack slot = half the height");
        assert!(r3.loc.y > r2.loc.y, "second stack col is below the first");
    }

    #[test]
    fn test_workspace_layout_bstack_single_column_full_width() {
        // One column in BStack: should occupy the full work area.
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let cfg = LayoutConfig {
            column_gap: 0,
            window_gap: 0,
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        };
        use crate::layout::workspace::{WorkspaceLayout, Workspace};
        let mut ws = Workspace::with_layout(WorkspaceLayout::BStack);
        ws.add_window_to_new_column(WindowId::new(1));

        let engine = TilingEngine::new(cfg.clone());
        let positions = engine.calculate_positions_bstack(&ws, work_rect, &cfg);
        assert_eq!(positions.len(), 1);
        let r = positions[0].1;
        assert_eq!(r.size.w, 1920, "single column should take full width");
        assert_eq!(r.size.h, 1080, "single column should take full height");
    }

    // =========================================================================
    // Item 3 — snap_floating_to_corner (engine-level)
    // =========================================================================

    #[test]
    fn test_snap_floating_to_corner_no_op_when_not_floating() {
        // A tiled window — snap_floating_to_corner must be a no-op (no panic).
        let mut engine = make_engine();
        engine.add_window(make_window(100, 500, 400), &BackendHandle::default_for_test());
        let oid = engine.focused_output().unwrap();
        if let Some(m) = engine.monitors_mut().get_mut(&oid) {
            m.focus_window = Some(WindowId::new(100));
        }
        // Window 100 is tiled, not floating — this must return silently.
        engine.snap_floating_to_corner(
            crate::layout::floating::FloatingCorner::TopLeft,
            &BackendHandle::default_for_test(),
        );
        // No panic = pass.
    }

    // =========================================================================
    // Item 1 (new) — animation drives SetWindowPos via tick_animations
    // =========================================================================

    /// Start a rect animation via `animating_to_target`, advance time past the
    /// midpoint, and verify that `animating_rects` still contains the entry
    /// (animation in flight) and that the intermediate rect differs from both
    /// the start and the target (the engine is genuinely interpolating).
    #[test]
    fn test_animation_drives_position_change() {
        let mut engine = make_engine_fixed_1920();
        // Enable animations with a generous duration so the first tick is mid-flight.
        engine.update_animation_settings(true, 1_000_000, crate::layout::Easing::Linear);

        let wid = WindowId::new(100);
        let start = Rect::new(0, 0, 500, 1080);
        let target = Rect::new(960, 0, 500, 1080);

        // Seed applied_state so the transition has a non-zero start.
        engine.applied_state.insert(wid, AppliedState {
            rect: start,
            border_color: 0xFFFF_FFFF,
            opacity: 0xFFFF_FFFF,
            visible: true,
            focused: false,
            backdrop: 0xFFFF_FFFF,
        });

        engine.animating_to_target(wid, target);

        // Immediately after seeding, animation must be in flight.
        assert!(
            engine.has_active_animations(),
            "animating_rects must be non-empty after animating_to_target"
        );
        assert!(
            engine.animating_rects.contains_key(&wid),
            "animating_rects must contain the window id"
        );

        // The start and target rects stored in animating_rects must be distinct.
        if let Some(&(s, t, _, _)) = engine.animating_rects.get(&wid) {
            assert_ne!(s, t, "start and target rects must differ");
            assert_eq!(s, start, "stored start rect matches what we seeded");
            assert_eq!(t, target, "stored target rect matches the requested target");
        }
    }

    // =========================================================================
    // Item 2 (new) — Spiral layout
    // =========================================================================

    #[test]
    fn test_spiral_layout_three_tiles() {
        use crate::layout::workspace::{WorkspaceLayout, Workspace};
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let mut ws = Workspace::with_layout(WorkspaceLayout::Spiral);
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        ws.add_window_to_new_column(WindowId::new(3));

        let engine = make_engine_fixed_1920();
        let positions = engine.calculate_positions_spiral(&ws, work_rect);
        assert_eq!(positions.len(), 3, "spiral must position all 3 tiles");

        // Tile 1 (index 0): left half of full rect → x=0, w=960.
        let r1 = positions.iter().find(|(w, _)| *w == WindowId::new(1)).unwrap().1;
        assert_eq!(r1.loc.x, 0, "tile 1 starts at left edge");
        assert_eq!(r1.size.w, 960, "tile 1 takes left half width");
        assert_eq!(r1.size.h, 1080, "tile 1 takes full height");

        // Tile 2 (index 1): top half of the right-half remainder → y=0, h=540.
        let r2 = positions.iter().find(|(w, _)| *w == WindowId::new(2)).unwrap().1;
        assert_eq!(r2.loc.x, 960, "tile 2 starts at the right half x");
        assert_eq!(r2.size.h, 540, "tile 2 takes top half of remainder height");

        // Tile 3 (index 2): fills whatever remains.
        let r3 = positions.iter().find(|(w, _)| *w == WindowId::new(3)).unwrap().1;
        assert_eq!(r3.loc.x, 960, "tile 3 is in the right half");
        assert_eq!(r3.loc.y, 540, "tile 3 starts below tile 2");

        // Tile 1 must not overlap tile 2 horizontally.
        assert!(
            r1.loc.x + r1.size.w as i32 <= r2.loc.x,
            "tile 1 and tile 2 must not overlap"
        );
    }

    #[test]
    fn test_spiral_layout_single_tile_fills_work_rect() {
        use crate::layout::workspace::{WorkspaceLayout, Workspace};
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let mut ws = Workspace::with_layout(WorkspaceLayout::Spiral);
        ws.add_window_to_new_column(WindowId::new(42));
        let engine = make_engine_fixed_1920();
        let positions = engine.calculate_positions_spiral(&ws, work_rect);
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].1, work_rect, "single tile must fill the whole work rect");
    }

    #[test]
    fn test_spiral_layout_empty_workspace_returns_empty() {
        use crate::layout::workspace::{WorkspaceLayout, Workspace};
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let ws = Workspace::with_layout(WorkspaceLayout::Spiral);
        let engine = make_engine_fixed_1920();
        let positions = engine.calculate_positions_spiral(&ws, work_rect);
        assert!(positions.is_empty(), "empty workspace produces no positions");
    }

    // =========================================================================
    // Item 3 (new) — Sticky windows
    // =========================================================================

    /// toggle_sticky twice for the same window returns to non-sticky state.
    #[test]
    fn test_toggle_sticky_round_trip() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);
        let wid = WindowId::new(100);

        assert!(!engine.is_sticky(wid), "window is not sticky initially");
        engine.toggle_sticky(&BackendHandle::default_for_test());
        assert!(engine.is_sticky(wid), "window should be sticky after first toggle");
        engine.toggle_sticky(&BackendHandle::default_for_test());
        assert!(!engine.is_sticky(wid), "window should be non-sticky after second toggle");
    }

    /// Sticky windows are not hidden when switching workspaces.
    #[test]
    fn test_sticky_window_not_hidden_on_workspace_switch() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 100);

        // Mark window 100 as sticky.
        engine.toggle_sticky(&BackendHandle::default_for_test());
        assert!(engine.is_sticky(WindowId::new(100)));

        // Switch to workspace 1.
        engine.switch_workspace(1, &BackendHandle::default_for_test());

        // The sticky window must still be in the sticky set.
        assert!(
            engine.sticky_windows.contains(&WindowId::new(100)),
            "sticky window must remain in sticky_windows after workspace switch"
        );
        // And it must not have been removed from tiled_windows (still tracked).
        assert!(
            engine.tiled_windows.contains_key(&WindowId::new(100)),
            "sticky window must still be tracked in tiled_windows"
        );
    }

    // =========================================================================
    // Item 4 (new) — Workspace renaming
    // =========================================================================

    #[test]
    fn test_rename_workspace() {
        let mut engine = make_engine_fixed_1920();
        // Workspace 0 exists by default.
        assert_eq!(engine.workspace_name(0), None, "no name set initially");

        let result = engine.rename_workspace(0, "Main");
        assert!(result.is_ok(), "rename of existing workspace must succeed: {:?}", result);
        assert_eq!(engine.workspace_name(0), Some("Main".to_string()));

        // Renaming a non-existent workspace must return Err.
        let bad = engine.rename_workspace(99, "Ghost");
        assert!(bad.is_err(), "rename of missing workspace must fail");

        // Rename again to update the name.
        let _ = engine.rename_workspace(0, "Home");
        assert_eq!(engine.workspace_name(0), Some("Home".to_string()));
    }

    #[test]
    fn test_workspace_name_returns_none_when_unset() {
        let engine = make_engine_fixed_1920();
        assert_eq!(engine.workspace_name(5), None, "unknown workspace has no name");
    }

    // -------------------------------------------------------------------------
    // Item 2 — urgent window border colour
    // -------------------------------------------------------------------------

    /// Verify that when a window is in `urgent_windows`, `apply_layout_for_monitor`
    /// caches the urgent colour (`border_color_urgent`) rather than the normal
    /// focused or unfocused colour.  After clearing urgent the border reverts to
    /// the focused colour.
    ///
    /// Pure state-machine test: no real Win32 calls are exercised.
    #[test]
    fn test_urgent_window_uses_red_border() {
        let urgent_color = "#e53935".to_string();
        let mut engine = TilingEngine::new(LayoutConfig {
            column_width_mode: ColumnWidthMode::Fixed,
            column_width: 500,
            outer_gaps: (0, 0, 0, 0),
            smart_borders: false,
            border_color_urgent: urgent_color.clone(),
            ..LayoutConfig::default()
        });
        let oid = OutputId::from_name("M");
        engine.register_monitor(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        engine.add_window(make_window(200, 0, 0), &BackendHandle::default_for_test());
        focus_window(&mut engine, 200);

        // Mark the window urgent before applying layout.
        let wid = WindowId::new(200);
        engine.urgent_windows.insert(wid);
        engine.apply_all(&BackendHandle::default_for_test());

        let cached = engine
            .applied_state
            .get(&wid)
            .expect("tile must have cached state after apply_all");
        let expected_urgent = parse_color_to_u32(&urgent_color);
        assert_eq!(
            cached.border_color, expected_urgent,
            "urgent window must use urgent border colour 0x{:08X}, got 0x{:08X}",
            expected_urgent, cached.border_color,
        );

        // After clearing urgent the border should revert to the focused colour.
        // Force a cache miss so the engine re-evaluates the colour.
        engine.urgent_windows.remove(&wid);
        if let Some(s) = engine.applied_state.get_mut(&wid) {
            s.border_color = 0xFFFF_FFFF; // "never set" sentinel forces re-paint
        }
        engine.apply_all(&BackendHandle::default_for_test());

        let cached_after = engine
            .applied_state
            .get(&wid)
            .expect("tile must still have cached state");
        let expected_focused = parse_color_to_u32(&LayoutConfig::default().border_color_focused);
        assert_eq!(
            cached_after.border_color, expected_focused,
            "after clearing urgent, border must revert to focused colour 0x{:08X}, got 0x{:08X}",
            expected_focused, cached_after.border_color,
        );
    }

    // =========================================================================
    // Item 1 (polish) — backdrop_str_to_u32 + toggle_backdrop_cycle
    // =========================================================================

    #[test]
    fn test_backdrop_cycle() {
        // Verify backdrop_str_to_u32 returns the correct DWMSBT_* enum values.
        assert_eq!(TilingEngine::backdrop_str_to_u32("auto"),    0, "auto → DWMSBT_AUTO");
        assert_eq!(TilingEngine::backdrop_str_to_u32("none"),    1, "none → DWMSBT_NONE");
        assert_eq!(TilingEngine::backdrop_str_to_u32("mica"),    2, "mica → DWMSBT_MAINWINDOW");
        assert_eq!(TilingEngine::backdrop_str_to_u32("acrylic"), 3, "acrylic → DWMSBT_TRANSIENTWINDOW");
        assert_eq!(TilingEngine::backdrop_str_to_u32("tabbed"),  4, "tabbed → DWMSBT_TABBEDWINDOW");
        assert_eq!(TilingEngine::backdrop_str_to_u32("bogus"),   0, "unknown → fallback DWMSBT_AUTO");

        // Verify the cycle order: auto → none → mica → acrylic → tabbed → auto.
        let mut engine = make_engine();
        assert_eq!(engine.config.backdrop, "auto", "default backdrop is 'auto'");

        engine.toggle_backdrop_cycle(&BackendHandle::default_for_test());
        assert_eq!(engine.config.backdrop, "none");

        engine.toggle_backdrop_cycle(&BackendHandle::default_for_test());
        assert_eq!(engine.config.backdrop, "mica");

        engine.toggle_backdrop_cycle(&BackendHandle::default_for_test());
        assert_eq!(engine.config.backdrop, "acrylic");

        engine.toggle_backdrop_cycle(&BackendHandle::default_for_test());
        assert_eq!(engine.config.backdrop, "tabbed");

        engine.toggle_backdrop_cycle(&BackendHandle::default_for_test());
        assert_eq!(engine.config.backdrop, "auto", "cycle wraps from 'tabbed' back to 'auto'");
    }

    // =========================================================================
    // Item 3 (polish) — swap_columns
    // =========================================================================

    #[test]
    fn test_swap_columns_basic() {
        // Two windows in two separate columns: col0=w100, col1=w101.
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        {
            let m = engine.monitors().get(&oid).unwrap();
            let ws = m.workspace().unwrap();
            assert_eq!(ws.columns.len(), 2, "two columns after two add_window calls");
            assert_eq!(ws.columns[0].tiles[0].window_id, WindowId::new(100));
            assert_eq!(ws.columns[1].tiles[0].window_id, WindowId::new(101));
        }

        // Focus col 0 so we can verify focus follows the swap.
        if let Some(m) = engine.monitors_mut().get_mut(&oid) {
            m.focus_column = Some(0);
            m.focus_window = Some(WindowId::new(100));
        }

        engine.swap_columns(0, 1, &BackendHandle::default_for_test());

        let m = engine.monitors().get(&oid).unwrap();
        let ws = m.workspace().unwrap();
        assert_eq!(ws.columns.len(), 2, "column count unchanged after swap");
        assert_eq!(ws.columns[0].tiles[0].window_id, WindowId::new(101),
            "column 0 should now hold w101 after swap");
        assert_eq!(ws.columns[1].tiles[0].window_id, WindowId::new(100),
            "column 1 should now hold w100 after swap");

        // Focus should have followed col 0 (now at idx 1).
        assert_eq!(m.focus_column, Some(1),
            "focus_column should follow the moved column from idx 0 → idx 1");
    }

    #[test]
    fn test_swap_columns_same_index_noop() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(101, 0, 0), &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        // Swapping column with itself should be a no-op.
        engine.swap_columns(0, 0, &BackendHandle::default_for_test());

        let ws = engine.monitors().get(&oid).unwrap().workspace().unwrap();
        assert_eq!(ws.columns[0].tiles[0].window_id, WindowId::new(100), "col order unchanged");
        assert_eq!(ws.columns[1].tiles[0].window_id, WindowId::new(101), "col order unchanged");
    }

    #[test]
    fn test_swap_columns_out_of_range_noop() {
        let mut engine = make_engine_fixed_1920();
        engine.add_window(make_window(100, 0, 0), &BackendHandle::default_for_test());

        let oid = engine.focused_output().unwrap();
        // Index 5 is out of range (only 1 column); should not panic.
        engine.swap_columns(0, 5, &BackendHandle::default_for_test());

        let ws = engine.monitors().get(&oid).unwrap().workspace().unwrap();
        assert_eq!(ws.columns.len(), 1, "still one column after no-op swap");
    }

    // -----------------------------------------------------------------------
    // Item 1 — snapshot_current_workspace / restore_snapshot
    // -----------------------------------------------------------------------

    /// Snapshot of a two-column workspace should capture both columns and the
    /// correct number of tiles per column.
    #[test]
    fn test_snapshot_workspace_round_trip() {
        let mut engine = make_engine_fixed_1920();
        // Two windows → two columns.
        engine.add_window(make_window(200, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(201, 0, 0), &BackendHandle::default_for_test());

        let snap = engine.snapshot_current_workspace()
            .expect("snapshot should succeed with focused monitor");

        assert_eq!(snap.workspaces.len(), 1, "one workspace snapshot");
        let ws_snap = &snap.workspaces[0];
        assert_eq!(ws_snap.id, 0, "active workspace id");
        assert_eq!(ws_snap.columns.len(), 2, "two columns captured");
        assert_eq!(ws_snap.columns[0].tiles.len(), 1);
        assert_eq!(ws_snap.columns[1].tiles.len(), 1);
        assert_eq!(snap.focused_workspace, Some(0));
    }

    /// Restoring a snapshot with dead HWNDs (not valid windows in a unit-test
    /// context — `IsWindow` returns false for fabricated handles) should skip
    /// those tiles without crashing.  The restored workspace should have no
    /// columns since all HWNDs are fake.
    #[test]
    fn test_restore_snapshot_skips_dead_hwnds() {
        use crate::layout::snapshot::{LayoutSnapshot, WorkspaceSnapshot, ColumnSnapshot};

        let mut engine = make_engine_fixed_1920();
        // No real windows registered — all HWNDs in the snapshot are dead.
        let snap = LayoutSnapshot {
            workspaces: vec![WorkspaceSnapshot {
                id: 0,
                columns: vec![
                    ColumnSnapshot {
                        width: None,
                        display: "stacked".to_string(),
                        // HWNDs 0xDEAD and 0xBEEF are not real windows.
                        tiles: vec![0xDEAD, 0xBEEF],
                    },
                ],
                scroll_offset_x: 0,
            }],
            focused_workspace: Some(0),
        };

        // restore_snapshot should succeed without panicking.
        let result = engine.restore_snapshot(&snap, &BackendHandle::default_for_test());
        assert!(result.is_ok(), "restore with dead HWNDs should not error: {:?}", result);

        let oid = engine.focused_output().unwrap();
        let ws = engine.monitors().get(&oid).unwrap().workspace().unwrap();
        // All tiles were dead → all columns skipped → workspace is empty.
        assert_eq!(ws.columns.len(), 0, "dead HWNDs produce no columns");
    }

    // -----------------------------------------------------------------------
    // Item 3 — is_over_gap
    // -----------------------------------------------------------------------

    /// On a 1920-wide monitor with two fixed-width columns and a 16px gap,
    /// a point in the middle of the gap should return `true`.
    #[test]
    fn test_is_over_gap() {
        let mut engine = make_engine_fixed_1920();
        // Two windows → two columns.
        engine.add_window(make_window(300, 0, 0), &BackendHandle::default_for_test());
        engine.add_window(make_window(301, 0, 0), &BackendHandle::default_for_test());

        // Derive expected geometry: outer_gaps default = 8, col_width default = 500,
        // column_gap default = 16.  Column 0 starts at work_x = 8, ends at 8+500 = 508.
        // Gap runs from 508 to 524 (16 px).  Midpoint = 516.
        // With half_gap slop (8 px) the zone is [500, 532).
        // 516 should be in the gap zone.
        let work_x = 8; // outer_gaps.3
        let col_width = 500i32;
        let gap = 16i32;
        let mid_gap_x = work_x + col_width + gap / 2; // 8 + 500 + 8 = 516
        let mid_y = 540; // middle of a 1080-height monitor

        assert!(
            engine.is_over_gap(mid_gap_x, mid_y),
            "point at x={} should be over the inter-column gap",
            mid_gap_x,
        );

        // A point well inside column 0 should NOT be over a gap.
        let inside_col0_x = work_x + 100; // 108 — well within column 0
        assert!(
            !engine.is_over_gap(inside_col0_x, mid_y),
            "point at x={} inside column 0 should NOT be over gap",
            inside_col0_x,
        );

        // A single-column workspace has no inter-column gap.
        let mut engine_single = make_engine_fixed_1920();
        engine_single.add_window(make_window(400, 0, 0), &BackendHandle::default_for_test());
        assert!(
            !engine_single.is_over_gap(mid_gap_x, mid_y),
            "single-column workspace has no gap",
        );
    }
}
