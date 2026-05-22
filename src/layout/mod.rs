pub mod workspace;
pub mod floating;
pub mod engine;
pub mod animation;
pub mod snap;
pub mod snapshot;

pub use workspace::{Workspace, Column, ColumnDisplay, Tile, compute_column_layout, compute_workspace_layout};
pub use workspace::{WorkspaceLayout};
pub use floating::{FloatManager, FloatWindow};
pub use floating::FloatingCorner;
pub use engine::TilingEngine;
pub use engine::LayoutConfig;
pub use engine::ColumnWidthMode;
pub use engine::AddWindowTarget;
pub use engine::ColumnWidthPreset;
pub use engine::WorkspaceDirection;
pub use animation::{Animation, AnimationManager, AnimationTarget, Easing};

use std::collections::HashMap;
use std::time::{Duration, Instant};
use crate::utils::{Rect, Size, OutputId, WindowId};

/// MRU (most-recently-used) focus stack.
/// The most recently focused window is at the front (index 0).
/// Capped at `capacity` entries (default 32).
#[derive(Debug, Clone)]
pub struct FocusRing {
    stack: Vec<WindowId>,
    capacity: usize,
}

impl FocusRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            stack: Vec::with_capacity(capacity.min(64)),
            capacity,
        }
    }

    /// Push a window to the front of the ring.
    /// If it is already present it is moved to the front (deduplication).
    /// The ring is capped at `capacity`; oldest entries are dropped from the back.
    pub fn push(&mut self, window_id: WindowId) {
        // Remove existing occurrence so we can re-insert at front.
        self.stack.retain(|&id| id != window_id);
        self.stack.insert(0, window_id);
        // Trim to capacity.
        self.stack.truncate(self.capacity);
    }

    /// Remove a window from the ring.
    pub fn remove(&mut self, window_id: WindowId) {
        self.stack.retain(|&id| id != window_id);
    }

    /// Return the most recently focused window (front of stack), if any.
    pub fn current(&self) -> Option<WindowId> {
        self.stack.first().copied()
    }

    /// Return the second entry (previous focus) without modifying the stack.
    pub fn previous(&self) -> Option<WindowId> {
        self.stack.get(1).copied()
    }

    /// Number of entries in the ring.
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// True when the ring contains no entries.
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SizingMode {
    Normal,
    Maximized,
    Fullscreen,
}

/// Four-state lifecycle for window configure dispatching.
/// Replaces the old Show/Hide/None stub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureIntent {
    /// No configure needed right now.
    NotNeeded,
    /// Configure is pending but a rate-limit backoff is active; skip for now.
    Throttled,
    /// Ready to dispatch a configure at the next convenient point.
    CanSend,
    /// Urgent — dispatch a configure as soon as possible.
    ShouldSend,
}

/// Rate-limiter that produces a `ConfigureIntent` based on elapsed time since
/// the last configure was dispatched.
///
/// Default `min_interval` is 16 ms (≈60 fps). Call `mark_sent()` each time a
/// configure is actually dispatched to reset the timer.
///
/// Wired into `apply_layout_for_monitor` in `engine.rs`: every pass collects
/// per-tile intents, skips `Throttled`/`NotNeeded` positions, and calls
/// `mark_sent()` for tiles that were actually dispatched.
#[derive(Debug, Clone)]
pub struct ConfigureThrottle {
    pub last_send: Option<Instant>,
    pub min_interval: Duration,
}

impl ConfigureThrottle {
    pub fn new(min_interval: Duration) -> Self {
        Self { last_send: None, min_interval }
    }

    /// Default throttle: ~60 fps (16 ms between configures).
    pub fn default_60fps() -> Self {
        Self::new(Duration::from_millis(16))
    }

    /// Compute the current intent based on time elapsed since last dispatch.
    pub fn intent(&self) -> ConfigureIntent {
        match self.last_send {
            None => ConfigureIntent::ShouldSend,
            Some(t) => {
                if t.elapsed() >= self.min_interval {
                    ConfigureIntent::CanSend
                } else {
                    ConfigureIntent::Throttled
                }
            }
        }
    }

    /// Record that a configure was just dispatched. Resets the timer.
    pub fn mark_sent(&mut self) {
        self.last_send = Some(Instant::now());
    }
}

impl Default for ConfigureThrottle {
    fn default() -> Self {
        Self::default_60fps()
    }
}

/// Trait that abstracts positionable layout elements such as tiles.
///
/// The engine does not consume this trait yet — it is introduced so that
/// future algorithms can operate generically over any tile-like type.
pub trait LayoutElement {
    fn window_id(&self) -> WindowId;
    fn bounds(&self) -> Rect;
    fn min_size(&self) -> Size { Size::new(50, 50) }
    fn max_size(&self) -> Option<Size> { None }
    fn is_focused(&self) -> bool { false }
    fn is_urgent(&self) -> bool { false }
    fn configure_intent(&self) -> ConfigureIntent;
}

/// Ordered map of monitors with an insertion-order primary and an explicit
/// focused-monitor cursor.
///
/// Wraps `HashMap<K, V>` and keeps a parallel `Vec<K>` for stable ordering.
/// `primary()` returns the first-inserted monitor (index 0 of `order`).
/// `focused()` returns the monitor pointed to by `focused` (if any).
#[derive(Debug)]
pub struct MonitorSet<K, V> {
    map: HashMap<K, V>,
    order: Vec<K>,
    focused: Option<K>,
}

impl<K, V> MonitorSet<K, V>
where
    K: std::hash::Hash + Eq + Clone + Copy,
{
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: Vec::new(),
            focused: None,
        }
    }

    /// Insert a monitor. Appends to insertion order; does not change `focused`.
    pub fn insert(&mut self, id: K, monitor: V) {
        if !self.map.contains_key(&id) {
            self.order.push(id);
        }
        self.map.insert(id, monitor);
    }

    /// Remove a monitor. If it was the focused one, `focused` becomes `None`.
    pub fn remove(&mut self, id: &K) -> Option<V> {
        self.order.retain(|k| k != id);
        if self.focused.as_ref() == Some(id) {
            self.focused = None;
        }
        self.map.remove(id)
    }

    pub fn get(&self, id: &K) -> Option<&V> {
        self.map.get(id)
    }

    pub fn get_mut(&mut self, id: &K) -> Option<&mut V> {
        self.map.get_mut(id)
    }

    pub fn contains_key(&self, id: &K) -> bool {
        self.map.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Iterate in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> + '_ {
        let map = &self.map;
        self.order.iter().filter_map(move |k| map.get(k).map(|v| (k, v)))
    }

    /// Iterate mutably in insertion order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> + '_ {
        // Can't easily yield (&K, &mut V) in insertion order without unsafe.
        // Delegate to HashMap::iter_mut (unordered) — callers that need order
        // should use `keys()` and call `get_mut` in a loop.
        self.map.iter_mut()
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &K> + '_ {
        self.order.iter()
    }

    /// Values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        let map = &self.map;
        self.order.iter().filter_map(move |k| map.get(k))
    }

    /// Values mutably — unordered (HashMap limitation with mutable references).
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> + '_ {
        self.map.values_mut()
    }

    // ---- focused cursor ----

    /// Set the focused monitor by id. The id must already be present.
    pub fn set_focused(&mut self, id: K) {
        if self.map.contains_key(&id) {
            self.focused = Some(id);
        }
    }

    /// Clear the focused cursor.
    pub fn clear_focused(&mut self) {
        self.focused = None;
    }

    /// Return the focused monitor id.
    pub fn focused_id(&self) -> Option<K> {
        self.focused
    }

    /// Return a reference to the focused monitor.
    pub fn focused(&self) -> Option<&V> {
        self.focused.as_ref().and_then(|id| self.map.get(id))
    }

    /// Return a mutable reference to the focused monitor.
    pub fn focused_mut(&mut self) -> Option<&mut V> {
        self.focused.and_then(move |id| self.map.get_mut(&id))
    }

    // ---- primary (first-inserted) ----

    /// Return the first-inserted monitor's id, if any.
    pub fn primary_id(&self) -> Option<K> {
        self.order.first().copied()
    }

    /// Return a reference to the first-inserted monitor.
    pub fn primary(&self) -> Option<&V> {
        self.order.first().and_then(|k| self.map.get(k))
    }
}

impl<K, V> Default for MonitorSet<K, V>
where
    K: std::hash::Hash + Eq + Clone + Copy,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, K, V> IntoIterator for &'a MonitorSet<K, V>
where
    K: Eq + std::hash::Hash + Copy,
{
    type Item = (&'a K, &'a V);
    type IntoIter = std::vec::IntoIter<(&'a K, &'a V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.order.iter()
            .filter_map(|k| self.map.get(k).map(|v| (k, v)))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut MonitorSet<K, V>
where
    K: Eq + std::hash::Hash + Copy,
{
    type Item = (&'a K, &'a mut V);
    type IntoIter = std::vec::IntoIter<(&'a K, &'a mut V)>;

    fn into_iter(self) -> Self::IntoIter {
        // Split borrows so HashMap::iter_mut() and &order can coexist.
        let MonitorSet { map, order, .. } = self;
        let mut pairs: Vec<(&'a K, &'a mut V)> = map.iter_mut().collect();
        // Sort by insertion order (position in `order`) to match &MonitorSet's iteration order.
        pairs.sort_by_key(|(k, _)| {
            order.iter().position(|ok| ok == *k).unwrap_or(usize::MAX)
        });
        pairs.into_iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Left,
    Right,
}

/// Monitor represents a physical display with its own workspace.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub output_id: OutputId,
    pub bounds: Rect,
    pub work_area: Rect,
    /// DPI scaling factor (1.0 = 96 DPI, 1.5 = 144 DPI, 2.0 = 192 DPI)
    pub scale_factor: f64,
    pub workspaces: HashMap<i32, Workspace>,
    pub active_workspace: i32,
    pub focus_column: Option<usize>,
    pub focus_window: Option<WindowId>,
    /// MRU focus stack — most recently focused window is at the front.
    pub focus_ring: FocusRing,
}

impl Monitor {
    pub fn new(output_id: OutputId, bounds: Rect, work_area: Rect) -> Self {
        Self::with_scale(output_id, bounds, work_area, 1.0)
    }

    pub fn with_scale(output_id: OutputId, bounds: Rect, work_area: Rect, scale_factor: f64) -> Self {
        let mut workspaces = HashMap::new();
        workspaces.insert(0, Workspace::new());
        Self {
            output_id,
            bounds,
            work_area,
            scale_factor,
            workspaces,
            active_workspace: 0,
            focus_column: None,
            focus_window: None,
            focus_ring: FocusRing::new(32),
        }
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(&self.active_workspace)
    }

    pub fn workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.workspaces.get_mut(&self.active_workspace)
    }

    pub fn active_workspace_id(&self) -> i32 {
        self.active_workspace
    }

    pub fn switch_workspace(&mut self, id: i32) {
        if !self.workspaces.contains_key(&id) {
            self.workspaces.insert(id, Workspace::new());
        }
        self.active_workspace = id;
        self.focus_column = None;
        self.focus_window = None;
    }

    /// Get the logical width of the work area (accounts for DPI scaling).
    /// Note: With PER_MONITOR_AWARE_V2, work_area is already in logical pixels.
    pub fn logical_work_width(&self) -> i32 {
        self.work_area.size.w as i32
    }

    /// Get the logical height of the work area (accounts for DPI scaling).
    pub fn logical_work_height(&self) -> i32 {
        self.work_area.size.h as i32
    }

    /// Convert physical pixels to logical pixels using the monitor's scale factor.
    /// Physical pixels = actual screen pixels, Logical pixels = DPI-scaled coordinates.
    /// Note: With PER_MONITOR_AWARE_V2, most coordinates from Windows APIs are already logical.
    pub fn physical_to_logical(&self, physical: i32) -> i32 {
        (physical as f64 / self.scale_factor) as i32
    }

    /// Convert logical pixels to physical pixels using the monitor's scale factor.
    pub fn logical_to_physical(&self, logical: i32) -> i32 {
        (logical as f64 * self.scale_factor) as i32
    }

    /// Get a DPI-aware rect for positioning. 
    /// Returns a rect suitable for use with SetWindowPos.
    /// Note: With PER_MONITOR_AWARE_V2, this returns the work_area as-is.
    pub fn positioning_rect(&self) -> Rect {
        // With PER_MONITOR_AWARE_V2, work_area is already in the coordinate system
        // expected by SetWindowPos, so no conversion is needed.
        // The scale_factor is retained for custom UI elements or calculations.
        self.work_area
    }

    /// Add a window to the workspace. If width_hint is provided,
    /// the column will use that width instead of the default.
    pub fn add_window(&mut self, window_id: WindowId) {
        self.add_window_with_width(window_id, None);
    }

    pub fn add_window_with_width(&mut self, window_id: WindowId, width_hint: Option<u32>) {
        if let Some(workspace) = self.workspace_mut() {
            let column_idx = workspace.add_window_to_new_column_with_width(window_id, width_hint);
            self.focus_column = Some(column_idx);
            self.focus_window = Some(window_id);
        }
        self.focus_ring.push(window_id);
    }

    /// Set focus to a specific window, updating both focus_window and the MRU ring.
    pub fn set_focus(&mut self, window_id: WindowId) {
        self.focus_window = Some(window_id);
        self.focus_ring.push(window_id);
        // Also update focus_column to match.
        if let Some(workspace) = self.workspace() {
            if let Some(col_idx) = workspace.find_window_column(window_id) {
                self.focus_column = Some(col_idx);
            }
        }
    }

    pub fn remove_window(&mut self, window_id: WindowId) {
        // Remove from MRU ring unconditionally.
        self.focus_ring.remove(window_id);

        if let Some(workspace) = self.workspace_mut() {
            let removed = workspace.remove_window(window_id);
            if let Some((col_idx, _)) = removed {
                if self.focus_window == Some(window_id) {
                    self.focus_window = None;
                    // Try to restore focus from MRU ring first; fall back to
                    // column-index arithmetic if the previous window is gone.
                    let mru_candidate = self.focus_ring.current();
                    let new_focus = if let Some(workspace) = self.workspace() {
                        let num_cols = workspace.columns.len();
                        if num_cols == 0 {
                            None
                        } else if let Some(mru_wid) = mru_candidate {
                            // Verify the MRU candidate still exists in this workspace.
                            workspace.find_window_column(mru_wid)
                                .map(|col| (col, Some(mru_wid)))
                        } else {
                            // No MRU entry — fall back to adjacent column.
                            let adj = col_idx.min(num_cols - 1);
                            let focus_wid = workspace.columns.get(adj)
                                .and_then(|col| col.tiles.first())
                                .map(|tile| tile.window_id);
                            Some((adj, focus_wid))
                        }
                    } else { None };
                    if let Some((adj, focus_wid)) = new_focus {
                        self.focus_column = Some(adj);
                        self.focus_window = focus_wid;
                        // Keep MRU consistent with new focus.
                        if let Some(fwid) = focus_wid {
                            self.focus_ring.push(fwid);
                        }
                    } else {
                        self.focus_column = None;
                    }
                } else {
                    // A non-focused window was removed. Indices may have shifted left if
                    // the removed window's column was deleted. Re-anchor focus_column by
                    // searching for the focused window's new column index.
                    let focused_wid = self.focus_window;
                    let new_focus = if let Some(workspace) = self.workspace() {
                        let num_cols = workspace.columns.len();
                        if num_cols == 0 {
                            None
                        } else if let Some(fwid) = focused_wid {
                            // Find where the focused window ended up
                            workspace.find_window_column(fwid)
                                .map(|col| (col, fwid))
                        } else {
                            // No focused window — clamp focus_column to valid range
                            let clamped = self.focus_column
                                .unwrap_or(0)
                                .min(num_cols - 1);
                            let focus_wid = workspace.columns.get(clamped)
                                .and_then(|col| col.tiles.first())
                                .map(|t| t.window_id);
                            focus_wid.map(|wid| (clamped, wid))
                        }
                    } else { None };
                    if let Some((col, wid)) = new_focus {
                        self.focus_column = Some(col);
                        self.focus_window = Some(wid);
                    } else {
                        // Clamp to a valid index even if no focused window is trackable
                        if let Some(workspace) = self.workspace() {
                            let num_cols = workspace.columns.len();
                            if num_cols == 0 {
                                self.focus_column = None;
                            } else {
                                self.focus_column = Some(
                                    self.focus_column.unwrap_or(0).min(num_cols - 1),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn focus_left(&mut self) {
        if let Some(col_idx) = self.focus_column {
            if col_idx > 0 {
                self.focus_column = Some(col_idx - 1);
                let new_focus = self
                    .workspace()
                    .and_then(|w| w.columns.get(col_idx - 1))
                    .and_then(|c| c.tiles.first())
                    .map(|t| t.window_id);
                if let Some(wid) = new_focus {
                    self.focus_window = Some(wid);
                    self.focus_ring.push(wid);
                }
            }
        }
    }

    pub fn focus_right(&mut self) {
        if let Some(col_idx) = self.focus_column {
            let info = self.workspace().and_then(|workspace| {
                if col_idx < workspace.columns.len().saturating_sub(1) {
                    let new_col = col_idx + 1;
                    workspace.columns.get(new_col).and_then(|col| {
                        col.tiles.first().map(|tile| (new_col, tile.window_id))
                    })
                } else {
                    None
                }
            });
            if let Some((new_col, window_id)) = info {
                self.focus_column = Some(new_col);
                self.focus_window = Some(window_id);
                self.focus_ring.push(window_id);
            }
        }
    }

    pub fn focus_up(&mut self) {
        let new_wid = if let Some(workspace) = self.workspace() {
            if let Some(col_idx) = self.focus_column {
                if let Some(col) = workspace.columns.get(col_idx) {
                    let focus_wid = match self.focus_window {
                        Some(wid) => wid,
                        None => return,
                    };
                    let mut found = None;
                    for (i, tile) in col.tiles.iter().enumerate() {
                        if tile.window_id == focus_wid {
                            if i > 0 {
                                found = Some(col.tiles[i - 1].window_id);
                            }
                            break;
                        }
                    }
                    found
                } else { None }
            } else { None }
        } else { None };
        if let Some(wid) = new_wid {
            self.focus_window = Some(wid);
            self.focus_ring.push(wid);
        }
    }

    pub fn focus_down(&mut self) {
        let new_wid = if let Some(workspace) = self.workspace() {
            if let Some(col_idx) = self.focus_column {
                if let Some(col) = workspace.columns.get(col_idx) {
                    let focus_wid = match self.focus_window {
                        Some(wid) => wid,
                        None => return,
                    };
                    let mut found = None;
                    for (i, tile) in col.tiles.iter().enumerate() {
                        if tile.window_id == focus_wid {
                            if i < col.tiles.len().saturating_sub(1) {
                                found = Some(col.tiles[i + 1].window_id);
                            }
                            break;
                        }
                    }
                    found
                } else { None }
            } else { None }
        } else { None };
        if let Some(wid) = new_wid {
            self.focus_window = Some(wid);
            self.focus_ring.push(wid);
        }
    }
}

// Tests to add to various modules
// 1. Monitor tests in layout/mod.rs
// 2. IPC deserialization roundtrip tests
// 3. Engine lifecycle tests

// === layout/mod.rs additions ===

#[cfg(test)]
mod monitor_tests {
    use super::*;
    use crate::utils::Rect;

    fn make_monitor() -> Monitor {
        Monitor::new(
            OutputId::from_name("Test"),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        )
    }

    #[test]
    fn test_monitor_new() {
        let m = make_monitor();
        assert_eq!(m.active_workspace, 0);
        assert!(m.focus_column.is_none());
        assert!(m.focus_window.is_none());
        assert!((m.scale_factor - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_monitor_with_scale() {
        let m = Monitor::with_scale(
            OutputId::from_name("HiDPI"),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
            1.5,
        );
        assert!((m.scale_factor - 1.5).abs() < 0.01);
    }

    #[test]
    fn test_monitor_add_window() {
        let mut m = make_monitor();
        m.add_window(WindowId::new(100));
        assert_eq!(m.focus_column, Some(0));
        assert_eq!(m.focus_window, Some(WindowId::new(100)));

        m.add_window(WindowId::new(101));
        assert_eq!(m.focus_column, Some(1));
        assert_eq!(m.focus_window, Some(WindowId::new(101)));
    }

    #[test]
    fn test_monitor_add_window_with_width() {
        let mut m = make_monitor();
        m.add_window_with_width(WindowId::new(100), Some(400));
        let ws = m.workspace().unwrap();
        assert_eq!(ws.columns[0].width, Some(400));
    }

    #[test]
    fn test_monitor_remove_window_focus() {
        let mut m = make_monitor();
        m.add_window(WindowId::new(100));
        m.add_window(WindowId::new(101));
        m.add_window(WindowId::new(102));

        // Focus on middle window
        m.focus_left(); // from col2 to col1
        assert_eq!(m.focus_window, Some(WindowId::new(101)));

        // Remove focused window
        m.remove_window(WindowId::new(101));
        // Should have 2 columns left
        assert_eq!(m.workspace().unwrap().columns.len(), 2);
        // Focus should shift to adjacent column
        assert!(m.focus_column.is_some());
    }

    #[test]
    fn test_monitor_remove_last_window() {
        let mut m = make_monitor();
        m.add_window(WindowId::new(100));
        m.remove_window(WindowId::new(100));
        assert!(m.focus_column.is_none() || m.workspace().unwrap().columns.len() == 0);
    }

    #[test]
    fn test_monitor_switch_workspace() {
        let mut m = make_monitor();
        m.add_window(WindowId::new(100));

        m.switch_workspace(1);
        assert_eq!(m.active_workspace, 1);
        assert!(m.focus_column.is_none());
        assert!(m.focus_window.is_none());

        // Workspace 1 should be empty
        let ws = m.workspace().unwrap();
        assert_eq!(ws.columns.len(), 0);
    }

    #[test]
    fn test_monitor_switch_back_workspace() {
        let mut m = make_monitor();
        m.add_window(WindowId::new(100));

        m.switch_workspace(1);
        m.switch_workspace(0);

        // Window should still be there
        let ws = m.workspace().unwrap();
        assert_eq!(ws.columns.len(), 1);
    }

    #[test]
    fn test_monitor_focus_left_right() {
        let mut m = make_monitor();
        m.add_window(WindowId::new(100));
        m.add_window(WindowId::new(101));
        m.add_window(WindowId::new(102));

        assert_eq!(m.focus_column, Some(2));
        m.focus_left();
        assert_eq!(m.focus_column, Some(1));
        m.focus_left();
        assert_eq!(m.focus_column, Some(0));
        m.focus_left(); // at boundary, should stay
        assert_eq!(m.focus_column, Some(0));

        m.focus_right();
        assert_eq!(m.focus_column, Some(1));
        m.focus_right();
        assert_eq!(m.focus_column, Some(2));
        m.focus_right(); // at boundary, should stay
        assert_eq!(m.focus_column, Some(2));
    }

    #[test]
    fn test_monitor_focus_up_down() {
        let mut m = make_monitor();
        m.add_window(WindowId::new(100));
        // Add second tile to column 0
        if let Some(ws) = m.workspace_mut() {
            ws.add_window_to_column(0, WindowId::new(101));
        }

        m.focus_column = Some(0);
        m.focus_window = Some(WindowId::new(100));

        m.focus_down();
        assert_eq!(m.focus_window, Some(WindowId::new(101)));
        m.focus_down(); // at bottom
        assert_eq!(m.focus_window, Some(WindowId::new(101)));

        m.focus_up();
        assert_eq!(m.focus_window, Some(WindowId::new(100)));
        m.focus_up(); // at top
        assert_eq!(m.focus_window, Some(WindowId::new(100)));
    }

    #[test]
    fn test_monitor_empty_focus() {
        let mut m = make_monitor();
        // No windows — focus ops should not panic
        m.focus_left();
        m.focus_right();
        m.focus_up();
        m.focus_down();
        assert!(m.focus_column.is_none());
    }

    #[test]
    fn test_scroll_direction_equality() {
        assert_eq!(ScrollDirection::Left, ScrollDirection::Left);
        assert_ne!(ScrollDirection::Left, ScrollDirection::Right);
    }

    #[test]
    fn test_sizing_mode() {
        assert_ne!(SizingMode::Normal, SizingMode::Maximized);
        assert_ne!(SizingMode::Maximized, SizingMode::Fullscreen);
    }

    // --- FocusRing tests ---

    #[test]
    fn test_focus_ring_push_dedup() {
        let mut ring = FocusRing::new(32);
        ring.push(WindowId::new(1));
        ring.push(WindowId::new(2));
        ring.push(WindowId::new(3));
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.current(), Some(WindowId::new(3)));
        // Push an existing entry — should move to front, no duplicate.
        ring.push(WindowId::new(1));
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.current(), Some(WindowId::new(1)));
        assert_eq!(ring.previous(), Some(WindowId::new(3)));
    }

    #[test]
    fn test_focus_ring_remove() {
        let mut ring = FocusRing::new(32);
        ring.push(WindowId::new(1));
        ring.push(WindowId::new(2));
        ring.push(WindowId::new(3));
        // Remove the current window — next one becomes current.
        ring.remove(WindowId::new(3));
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.current(), Some(WindowId::new(2)));
        // Remove a non-existent window — no-op.
        ring.remove(WindowId::new(99));
        assert_eq!(ring.len(), 2);
        // Remove all.
        ring.remove(WindowId::new(2));
        ring.remove(WindowId::new(1));
        assert!(ring.is_empty());
        assert_eq!(ring.current(), None);
        assert_eq!(ring.previous(), None);
    }

    #[test]
    fn test_focus_ring_capacity() {
        let mut ring = FocusRing::new(4);
        for i in 1..=6 {
            ring.push(WindowId::new(i));
        }
        // Capacity is 4 — oldest two entries are dropped.
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.current(), Some(WindowId::new(6)));
        // Window 1 and 2 were evicted.
        ring.remove(WindowId::new(1)); // no-op, already gone
        assert_eq!(ring.len(), 4);
    }

    #[test]
    fn test_focus_ring_empty() {
        let ring = FocusRing::new(32);
        assert!(ring.is_empty());
        assert_eq!(ring.current(), None);
        assert_eq!(ring.previous(), None);
        assert_eq!(ring.len(), 0);
    }

    // --- MonitorSet tests ---

    #[test]
    fn test_monitor_set_insert_order() {
        let mut ms: MonitorSet<OutputId, i32> = MonitorSet::new();
        let a = OutputId::from_name("A");
        let b = OutputId::from_name("B");
        let c = OutputId::from_name("C");
        ms.insert(a, 1);
        ms.insert(b, 2);
        ms.insert(c, 3);
        assert_eq!(ms.len(), 3);
        // Primary is first inserted.
        assert_eq!(ms.primary_id(), Some(a));
        let keys: Vec<_> = ms.keys().copied().collect();
        assert_eq!(keys, vec![a, b, c]);
    }

    #[test]
    fn test_monitor_set_remove_updates_focused() {
        let mut ms: MonitorSet<OutputId, i32> = MonitorSet::new();
        let a = OutputId::from_name("A");
        let b = OutputId::from_name("B");
        ms.insert(a, 10);
        ms.insert(b, 20);
        ms.set_focused(a);
        assert_eq!(ms.focused_id(), Some(a));
        // Removing the focused monitor clears focused.
        ms.remove(&a);
        assert_eq!(ms.focused_id(), None);
        assert_eq!(ms.len(), 1);
    }

    #[test]
    fn test_monitor_set_primary_after_remove() {
        let mut ms: MonitorSet<OutputId, i32> = MonitorSet::new();
        let a = OutputId::from_name("A");
        let b = OutputId::from_name("B");
        ms.insert(a, 1);
        ms.insert(b, 2);
        ms.remove(&a);
        // After removing A, primary becomes B.
        assert_eq!(ms.primary_id(), Some(b));
    }

    #[test]
    fn test_monitor_set_focused_get() {
        let mut ms: MonitorSet<OutputId, i32> = MonitorSet::new();
        let a = OutputId::from_name("A");
        ms.insert(a, 42);
        ms.set_focused(a);
        assert_eq!(ms.focused(), Some(&42));
    }

    // --- ConfigureThrottle tests ---

    #[test]
    fn test_configure_throttle_initial_should_send() {
        let throttle = ConfigureThrottle::default_60fps();
        assert_eq!(throttle.intent(), ConfigureIntent::ShouldSend);
    }

    #[test]
    fn test_configure_throttle_after_mark_sent_throttled() {
        let mut throttle = ConfigureThrottle::new(std::time::Duration::from_secs(60));
        throttle.mark_sent();
        // Immediately after sending, we haven't waited 60s so it's Throttled.
        assert_eq!(throttle.intent(), ConfigureIntent::Throttled);
    }

    #[test]
    fn test_configure_throttle_after_elapsed_can_send() {
        let mut throttle = ConfigureThrottle::new(std::time::Duration::from_millis(0));
        throttle.mark_sent();
        // 0ms interval — immediately eligible.
        assert_eq!(throttle.intent(), ConfigureIntent::CanSend);
    }

    #[test]
    fn test_monitor_focus_ring_wired() {
        let mut m = make_monitor();
        m.add_window(WindowId::new(100));
        m.add_window(WindowId::new(101));
        // Ring should have both, with 101 at the front.
        assert_eq!(m.focus_ring.current(), Some(WindowId::new(101)));
        assert_eq!(m.focus_ring.previous(), Some(WindowId::new(100)));
        // Focus left — ring should update.
        m.focus_left();
        assert_eq!(m.focus_ring.current(), Some(WindowId::new(100)));
        // Remove focused window (100) — should fall back to MRU (101).
        m.remove_window(WindowId::new(100));
        assert_eq!(m.focus_window, Some(WindowId::new(101)));
        assert_eq!(m.focus_ring.current(), Some(WindowId::new(101)));
    }

    // --- MonitorSet IntoIterator tests ---

    #[test]
    fn test_monitor_set_into_iter_insertion_order() {
        let mut ms: MonitorSet<OutputId, i32> = MonitorSet::new();
        let a = OutputId::from_name("A");
        let b = OutputId::from_name("B");
        let c = OutputId::from_name("C");
        ms.insert(a, 10);
        ms.insert(b, 20);
        ms.insert(c, 30);

        // &MonitorSet IntoIterator should yield entries in insertion order.
        let pairs: Vec<_> = (&ms).into_iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0], (a, 10));
        assert_eq!(pairs[1], (b, 20));
        assert_eq!(pairs[2], (c, 30));
    }

    #[test]
    fn test_monitor_set_into_iter_empty() {
        let ms: MonitorSet<OutputId, i32> = MonitorSet::new();
        let pairs: Vec<_> = (&ms).into_iter().collect();
        assert!(pairs.is_empty());
    }

    #[test]
    fn test_monitor_set_into_iter_after_remove() {
        let mut ms: MonitorSet<OutputId, i32> = MonitorSet::new();
        let a = OutputId::from_name("A");
        let b = OutputId::from_name("B");
        ms.insert(a, 1);
        ms.insert(b, 2);
        ms.remove(&a);

        let pairs: Vec<_> = (&ms).into_iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], (b, 2));
    }
}

