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
}

impl AppliedState {
    fn unset() -> Self {
        Self {
            rect: Rect::new(0, 0, 0, 0),
            border_color: 0xFFFF_FFFF,
            opacity: 0xFFFF_FFFF,
            visible: false,
            focused: false,
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
            outer_gaps: (8, 8, 8, 8),
            focus_ring_width: 3,
            focus_ring_color: "#3381d9".to_string(),
            dim_unfocused: 1.0,
            scroll_step: 200,
            strip_frame: false,
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
            lc.border_color_focused = config.layout.border_color_focused.clone();
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
        lc
    }
}

/// Column width presets for niri-style cycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnWidthPreset {
    OneThird,   // ~33 % of work_rect
    Half,       // 50 %
    TwoThirds,  // ~67 %
    Full,       // 100 %
    /// Cycle through OneThird → Half → TwoThirds → Full → OneThird …
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
    tiled_windows: HashMap<WindowId, WindowInfo>,
    fullscreen_windows: HashSet<WindowId>,
    floating_windows: HashSet<WindowId>,
    window_sizing: HashMap<WindowId, SizingMode>,
    saved_styles: HashMap<WindowId, u32>,
    saved_ex_styles: HashMap<WindowId, u32>,
    window_rules: Vec<crate::config::WindowRule>,
    full_config: Option<crate::config::Config>,
    animation: crate::layout::AnimationManager,
    /// Overview mode: None = normal, Some(zoom_level) = zoomed out
    overview: Option<f64>,
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
    // ---- Item 5: urgent-window tracking ----
    /// Windows marked urgent (e.g. via WM_FLASHWINDOW or external hook).
    /// Populated by mark_urgent / cleared by clear_urgent.
    urgent_windows: HashSet<WindowId>,
    /// Counter tracking how many Win32 calls were actually issued (for testing).
    #[cfg(test)]
    pub win32_call_count: u32,
}

impl TilingEngine {
    pub fn new(config: LayoutConfig) -> Self {
        Self {
            monitors: MonitorSet::new(),
            config,
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
            urgent_windows: HashSet::new(),
            #[cfg(test)]
            win32_call_count: 0,
        }
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
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
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
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
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
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
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
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
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
                if let Some(o) = rules.opacity {
                    // Explicit opacity override from a window-rule.
                    // `Some(0.0)` is honoured as fully transparent.
                    self.apply_window_opacity(window_id.as_isize(), o);
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
                        self.overview = Some(zoom);
                    }
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

        let work_rect = Rect::new(
            monitor.work_area.loc.x + self.config.outer_gaps.3,
            monitor.work_area.loc.y + self.config.outer_gaps.0,
            monitor.work_area.size.w.saturating_sub(
                (self.config.outer_gaps.1 + self.config.outer_gaps.3) as u32,
            ),
            monitor.work_area.size.h.saturating_sub(
                (self.config.outer_gaps.0 + self.config.outer_gaps.2) as u32,
            ),
        );

        // Collect the window IDs on this monitor's active workspace
        let monitor_window_ids: std::collections::HashSet<WindowId> = workspace
            .columns
            .iter()
            .flat_map(|c| c.tiles.iter().map(|t| t.window_id))
            .collect();

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
        let tile_intents: HashMap<WindowId, ConfigureIntent> = {
            let monitor = self.monitors.get(&output_id).unwrap();
            let workspace = monitor.workspace().unwrap();
            workspace.columns.iter()
                .flat_map(|col| col.tiles.iter())
                .map(|tile| (tile.window_id, tile.configure_throttle.intent()))
                .collect()
        };

        let positions = self.calculate_positions(workspace, work_rect);
        // Snapshot positions for later write-back to Tile.cached_bounds so that
        // LayoutElement::bounds() returns the most recently computed rect.
        let position_map: HashMap<WindowId, Rect> = positions.iter().copied().collect();
        let focused = self.monitors.values().find_map(|m| m.focus_window);
        // ^ iterates in insertion order via MonitorSet::values()
        let border_w = self.config.border_width.max(1);

        // Pre-compute target colors (as packed u32) for this pass.
        let border_color_focused_u32 = parse_color_to_u32(&self.config.border_color_focused);
        let border_color_normal_u32  = parse_color_to_u32(&self.config.border_color);

        // Pre-compute target opacity values as u8 for comparison.
        let opacity_full: u32 = 255;
        let opacity_dim: u32 = (self.config.dim_unfocused * 255.0).round().clamp(0.0, 255.0) as u32;

        // Track which windows had set_window_position called so we can mark_sent() afterwards.
        let mut applied_windows: HashSet<WindowId> = HashSet::new();
        // Windows whose `set_window_position` failed MAX_POSITION_FAILURES times in a
        // row — promoted to floating after the loop so we stop fighting the OS.
        let mut windows_to_auto_float: Vec<WindowId> = Vec::new();

        for (window_id, rect) in positions {
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

            // Target values for this tile.
            let new_border_color = if is_focused { border_color_focused_u32 } else { border_color_normal_u32 };
            let new_opacity: u32 = if self.config.dim_unfocused < 1.0 {
                if is_focused { opacity_full } else { opacity_dim }
            } else {
                // dim_unfocused == 1.0 means "no dimming"; use sentinel so we
                // never issue SetLayeredWindowAttributes unnecessarily.
                0xFFFF_FFFF
            };

            // Focused windows get a wider border for visual emphasis.
            let inset = if is_focused { border_w + self.config.focus_ring_width } else { border_w };
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
            let pos_identical = skip_pos || prior.rect == inset_rect;
            if prior.visible == should_be_visible
                && prior.border_color == new_border_color
                && prior.opacity == new_opacity
                && prior.focused == is_focused
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
                let color_str = if is_focused {
                    &self.config.border_color_focused
                } else {
                    &self.config.border_color
                };
                self.set_dwm_border_color(window_id.as_isize(), color_str);
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
                let pos_result = backend.set_window_position(
                    window_id.as_isize(),
                    inset_rect,
                    windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                );
                applied_windows.insert(window_id);
                if pos_result.is_ok() {
                    self.applied_state.entry(window_id).or_insert_with(AppliedState::unset).rect = inset_rect;
                    #[cfg(test)] { self.win32_call_count += 1; }
                    // Clear any pending failure counter — the window is co-operating again.
                    self.failed_position.remove(&window_id);
                } else {
                    let count = self.failed_position.entry(window_id).or_insert(0);
                    *count = count.saturating_add(1);
                    if *count >= MAX_POSITION_FAILURES {
                        warn!(
                            "window {} rejected SetWindowPos {}× in a row; auto-floating",
                            window_id, count
                        );
                        windows_to_auto_float.push(window_id);
                    }
                }
            }

            // Update focus flag in cache.
            self.applied_state.entry(window_id).or_insert_with(AppliedState::unset).focused = is_focused;
        }

        // Mark configure_throttle as sent for every tile that had set_window_position called.
        // Populate cached_bounds while we're walking the workspace so LayoutElement::bounds()
        // returns the just-computed rect for any downstream observer (overlay rendering,
        // hit-testing, …).
        if let Some(monitor) = self.monitors.get_mut(&output_id) {
            if let Some(workspace) = monitor.workspace_mut() {
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
    fn calculate_positions(
        &self,
        workspace: &crate::layout::workspace::Workspace,
        work_rect: Rect,
    ) -> Vec<(WindowId, Rect)> {
        let mut positions = Vec::new();
        let column_gap = self.config.column_gap;
        let window_gap = self.config.window_gap;
        let num_columns = workspace.columns.len();
        if num_columns == 0 { return positions; }

        let zoom = self.overview_zoom();

        let proportional_width = {
            let total_gaps = column_gap * (num_columns as i32 - 1).max(0);
            let available = work_rect.size.w as i32 - total_gaps;
            (available / num_columns as i32).max(self.config.column_width as i32 / 2)
        };

        let col_widths: Vec<i32> = workspace.columns.iter().map(|col| {
            match self.config.column_width_mode {
                ColumnWidthMode::Proportional => proportional_width,
                ColumnWidthMode::Fixed => {
                    col.width.map(|w| w as i32).unwrap_or(self.config.column_width as i32)
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

            let work_height = if zoom < 1.0 {
                let padding = (work_rect.size.h as f64 * 0.10) as i32;
                work_rect.size.h as i32 - padding * 2
            } else {
                work_rect.size.h as i32
            };

            let scaled_gap = (window_gap as f64 * zoom) as i32;
            let total_gap_height = scaled_gap * (visible_count as i32 - 1).max(0);
            let available_height = work_height - total_gap_height;
            let window_height = available_height / visible_count as i32;

            let y_offset = if zoom < 1.0 {
                let total_used = visible_count as i32 * (window_height + scaled_gap) - scaled_gap;
                (work_rect.size.h as i32 - total_used) / 2
            } else {
                0
            };

            for (slot_idx, &tile_idx) in visible_indices.iter().enumerate() {
                if let Some(tile) = column.tiles.get(tile_idx) {
                    let y = if zoom < 1.0 {
                        y_offset + slot_idx as i32 * (window_height + scaled_gap)
                    } else {
                        work_rect.loc.y + slot_idx as i32 * (window_height + window_gap)
                    };
                    let rect = if zoom < 1.0 {
                        Rect::new(screen_x, work_rect.loc.y + y, scaled_w, window_height as u32)
                    } else {
                        Rect::new(screen_x, y, scaled_w, window_height as u32)
                    };
                    positions.push((tile.window_id, rect));
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

        // Hide old workspace windows, show new ones
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

            // Extend DWM frame into client area with -1 margins.
            // This removes the invisible 7px DWM-composed border while
            // still allowing DWMWA_BORDER_COLOR to paint visible borders.
            let margins = windows::Win32::UI::Controls::MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
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

    /// Check if animations are enabled and any are currently active
    pub fn has_active_animations(&self) -> bool {
        self.animation.has_active()
    }

    /// Check if animations are enabled
    pub fn animations_enabled(&self) -> bool {
        self.animation.is_enabled()
    }

    /// Tick all active animations by delta_ms milliseconds.
    /// Returns true if any animations are still running after this tick.
    /// Call this from the main event loop (~60fps).
    pub fn tick_animations(&mut self, delta_ms: u32) -> bool {
        if !self.animation.is_enabled() {
            return false;
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
            }
        }
        self.animation.has_active()
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

    /// Enter overview mode — compute a zoom level that fits all columns
    pub fn enter_overview(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        let (num_columns, view_width) = {
            let monitor = match self.monitors.get(&focused_output) {
                Some(m) => m,
                None => return,
            };
            let workspace = match monitor.workspace() {
                Some(w) => w,
                None => return,
            };
            let num_cols = workspace.columns.len();
            let vw = Self::view_width_for_monitor(monitor, &self.config);
            (num_cols, vw)
        };

        if num_columns == 0 { return; }

        // Compute zoom level so all columns fit within the view
        // The total content width = num_cols * col_width + (num_cols-1) * gap
        let col_width = Self::effective_column_width(num_columns, view_width, &self.config);
        let total_content = num_columns as i32 * col_width + (num_columns as i32 - 1).max(0) * self.config.column_gap;

        let zoom = if total_content > view_width {
            (view_width as f64 / total_content as f64).min(1.0)
        } else {
            1.0 // Already fits, no need to zoom out
        };

        info!("Entering overview mode: zoom={:.2} ({} columns, {}px content, {}px view)",
            zoom, num_columns, total_content, view_width);

        self.overview = Some(zoom);

        // Reset scroll offset — in overview, we show everything from the start
        if let Some(monitor) = self.monitors.get_mut(&focused_output) {
            if let Some(workspace) = monitor.workspace_mut() {
                workspace.scroll_offset.x = 0;
            }
        }

        self.apply_layout_for_monitor(focused_output, backend);
    }

    /// Exit overview mode — return to normal tiling
    pub fn exit_overview(&mut self, backend: &BackendHandle) {
        let focused_output = match self.monitors.focused_id() {
            Some(o) => o,
            None => return,
        };

        info!("Exiting overview mode");
        self.overview = None;

        // Re-scroll to the focused column
        self.scroll_to_focused_column(focused_output, backend);
    }

    /// Check if overview mode is active
    pub fn is_overview(&self) -> bool {
        self.overview.is_some()
    }

    /// Get the current overview zoom level (1.0 = normal, <1.0 = zoomed out)
    pub fn overview_zoom(&self) -> f64 {
        self.overview.unwrap_or(1.0)
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

    /// Set the full configuration (for keybind access and future use)
    pub fn set_full_config(&mut self, config: crate::config::Config) {
        self.window_rules = config.window_rules.clone();
        self.full_config = Some(config);
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
        self.refresh_urgent_states();
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

        // Resolve Cycle → a concrete preset
        let concrete = match preset {
            ColumnWidthPreset::Cycle => {
                let next = match self.last_column_preset {
                    ColumnWidthPreset::OneThird  => ColumnWidthPreset::Half,
                    ColumnWidthPreset::Half      => ColumnWidthPreset::TwoThirds,
                    ColumnWidthPreset::TwoThirds => ColumnWidthPreset::Full,
                    ColumnWidthPreset::Full      => ColumnWidthPreset::OneThird,
                    ColumnWidthPreset::Cycle     => ColumnWidthPreset::Half,
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
            ColumnWidthPreset::OneThird  => (view_width / 3).max(50) as u32,
            ColumnWidthPreset::Half      => (view_width / 2).max(50) as u32,
            ColumnWidthPreset::TwoThirds => (view_width * 2 / 3).max(50) as u32,
            ColumnWidthPreset::Full      => view_width.max(50) as u32,
            ColumnWidthPreset::Cycle     => unreachable!("Cycle resolved above"),
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
        let positions = engine.calculate_positions(workspace, work_rect);
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
        let positions = engine.calculate_positions(workspace, work_rect);
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
        let positions = engine.calculate_positions(workspace, work_rect);
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
        let positions = engine.calculate_positions(workspace, work_rect);
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
        let positions = engine.calculate_positions(&ws, Rect::new(0, 0, 1920, 1080));
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
        let positions = engine.calculate_positions(workspace, work_rect);
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
        let positions = engine.calculate_positions(workspace, work_rect);
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
        let positions = engine.calculate_positions(workspace, work_rect);
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

        // Default last_column_preset is Half → first Cycle → TwoThirds
        engine.set_column_width_preset(ColumnWidthPreset::Cycle, &BackendHandle::default_for_test());
        let oid_e = engine.focused_output().unwrap();
        let w1 = engine.monitors().get(&oid_e).unwrap().workspace().unwrap().columns[0].width.unwrap();
        // TwoThirds of 1920 = 1280
        assert_eq!(w1, 1280, "first Cycle from Half should be TwoThirds (1280)");

        // Second Cycle → Full (1920)
        engine.set_column_width_preset(ColumnWidthPreset::Cycle, &BackendHandle::default_for_test());
        let w2 = engine.monitors().get(&oid_e).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w2, 1920, "second Cycle should be Full (1920)");

        // Third Cycle → OneThird (640)
        engine.set_column_width_preset(ColumnWidthPreset::Cycle, &BackendHandle::default_for_test());
        let w3 = engine.monitors().get(&oid_e).unwrap().workspace().unwrap().columns[0].width.unwrap();
        assert_eq!(w3, 640, "third Cycle should be OneThird (640)");
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

}
