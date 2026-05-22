use crate::utils::{Rect, Point, Size, WindowId};
use super::{LayoutElement, ConfigureIntent, ConfigureThrottle};

#[derive(Debug, Clone)]
pub struct Tile {
    pub window_id: WindowId,
    pub size: Size,
    /// Cached bounds from the last layout pass.
    /// Starts as `Rect::default()` (all-zero) until `calculate_positions` has run.
    /// Populated by `TilingEngine::apply_layout_for_monitor` after each pass.
    pub cached_bounds: Rect,
    /// Rate-limiter for configure dispatching (~60 fps by default).
    /// `intent()` is consulted by `apply_layout_for_monitor` to skip Throttled
    /// positions and `mark_sent()` is called for every tile that had its
    /// position actually issued.
    pub configure_throttle: ConfigureThrottle,
    /// Preferred height from a window rule's `default_height`.
    /// Stored here but not yet used by the layout engine — layout currently distributes
    /// height equally. Wire into calculate_positions when per-tile height is implemented.
    pub preferred_height: Option<u32>,
    /// niri-style relative tile-height weight inside a column. When
    /// every tile in a column carries the default weight of 1.0 the
    /// engine distributes height equally (current behavior). Adjusting
    /// the weight via `Action::GrowTileHeight` / `ShrinkTileHeight`
    /// reallocates the column's vertical real estate proportionally.
    /// Always >= 0.1 by construction in the resize helpers.
    pub height_weight: f32,
}

impl Tile {
    pub fn new(window_id: WindowId) -> Self {
        Self {
            window_id,
            size: Size::new(800, 600),
            cached_bounds: Rect::default(),
            configure_throttle: ConfigureThrottle::default_60fps(),
            preferred_height: None,
            height_weight: 1.0,
        }
    }
}

impl LayoutElement for Tile {
    fn window_id(&self) -> WindowId {
        self.window_id
    }

    fn bounds(&self) -> Rect {
        // cached_bounds is written by TilingEngine::apply_layout_for_monitor at
        // the end of each pass. Before the first pass it is `Rect::default()`
        // (all-zero) — callers should treat that as "unknown".
        self.cached_bounds
    }

    fn configure_intent(&self) -> ConfigureIntent {
        self.configure_throttle.intent()
    }
}

/// Controls how tiles within a column are displayed.
///
/// `Stacked` — all tiles are stacked vertically and share the column height.
/// `Tabbed`  — only the active tab is visible; it occupies the full column height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnDisplay {
    Stacked,
    Tabbed { active_tab: usize },
}

impl Default for ColumnDisplay {
    fn default() -> Self {
        ColumnDisplay::Stacked
    }
}

#[derive(Debug, Clone)]
/// A vertical column in the workspace. Can contain multiple stacked tiles.
pub struct Column {
    pub tiles: Vec<Tile>,
    /// Explicit column width in pixels (None = use layout default)
    pub width: Option<u32>,
    /// Display mode: stacked (default) or tabbed.
    pub display: ColumnDisplay,
    /// niri-style per-column maximized state — when true the column should
    /// occupy the full work-area height (taking precedence over any per-tile
    /// height distribution). Separate from `SizingMode::Fullscreen`, which
    /// applies to a single window across the whole monitor including
    /// reserved bars.
    pub maximized: bool,
}

impl Column {
    pub fn new() -> Self {
        Self {
            tiles: Vec::new(),
            width: None,
            display: ColumnDisplay::Stacked,
            maximized: false,
        }
    }

    pub fn add_tile(&mut self, tile: Tile) {
        self.tiles.push(tile);
    }

    pub fn remove_tile(&mut self, window_id: WindowId) -> Option<Tile> {
        self.tiles
            .iter()
            .position(|t| t.window_id == window_id)
            .map(|i| self.tiles.remove(i))
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn get_tile(&self, index: usize) -> Option<&Tile> {
        self.tiles.get(index)
    }

    pub fn get_tile_mut(&mut self, index: usize) -> Option<&mut Tile> {
        self.tiles.get_mut(index)
    }

    pub fn index_of(&self, window_id: WindowId) -> Option<usize> {
        self.tiles.iter().position(|t| t.window_id == window_id)
    }

    // ---- Tabbed column API ----

    /// Switch this column to tabbed display mode.
    /// The active tab is reset to index 0.
    pub fn switch_to_tabbed(&mut self) {
        self.display = ColumnDisplay::Tabbed { active_tab: 0 };
    }

    /// Switch this column back to stacked (normal) display mode.
    pub fn switch_to_stacked(&mut self) {
        self.display = ColumnDisplay::Stacked;
    }

    /// Move to the next tab, wrapping around at the end.
    /// No-op if the column is in Stacked mode.
    pub fn next_tab(&mut self) {
        if let ColumnDisplay::Tabbed { ref mut active_tab } = self.display {
            let count = self.tiles.len();
            if count > 0 {
                *active_tab = (*active_tab + 1) % count;
            }
        }
    }

    /// Move to the previous tab, wrapping around at the beginning.
    /// No-op if the column is in Stacked mode.
    pub fn prev_tab(&mut self) {
        if let ColumnDisplay::Tabbed { ref mut active_tab } = self.display {
            let count = self.tiles.len();
            if count > 0 {
                *active_tab = active_tab.checked_sub(1).unwrap_or(count - 1);
            }
        }
    }

    /// Returns indices of tiles that should be visible.
    ///
    /// In `Stacked` mode all tile indices are returned.
    /// In `Tabbed` mode only the single active-tab index is returned.
    pub fn visible_tile_indices(&self) -> Vec<usize> {
        match &self.display {
            ColumnDisplay::Stacked => (0..self.tiles.len()).collect(),
            ColumnDisplay::Tabbed { active_tab } => {
                if self.tiles.is_empty() {
                    vec![]
                } else {
                    vec![(*active_tab).min(self.tiles.len() - 1)]
                }
            }
        }
    }
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-workspace layout algorithm (Item 5 — niri-parity).
///
/// `Scrolling`  — default niri-style horizontal scroll.
/// `BStack`     — bottom-stack: first column takes 50% width, remaining
///                columns split the right 50% stacked vertically.
/// `Spiral`     — golden-ratio recursive bisection: alternating horizontal/vertical
///                splits produce a spiral arrangement where each tile takes half of
///                the remaining rectangle. Implemented in
///                `TilingEngine::calculate_positions_spiral`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceLayout {
    /// Default: horizontal scrollable tiling (niri-style).
    Scrolling,
    /// Main-on-left, rest stacked vertically on the right half.
    BStack,
    /// Golden-ratio recursive bisection — alternating H/V splits.
    Spiral,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        WorkspaceLayout::Scrolling
    }
}

impl WorkspaceLayout {
    /// Parse a layout-mode string from config (case-insensitive).
    /// Unknown strings fall back to `Scrolling`.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "bstack" | "bottom-stack" | "bottomstack" => WorkspaceLayout::BStack,
            "spiral" => WorkspaceLayout::Spiral,
            _ => WorkspaceLayout::Scrolling,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub columns: Vec<Column>,
    pub scroll_offset: Point,
    /// Per-workspace layout algorithm; defaults to `Scrolling`.
    pub layout_mode: WorkspaceLayout,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
            scroll_offset: Point::default(),
            layout_mode: WorkspaceLayout::Scrolling,
        }
    }

    /// Create a new workspace with an explicit layout mode.
    pub fn with_layout(layout_mode: WorkspaceLayout) -> Self {
        Self {
            columns: Vec::new(),
            scroll_offset: Point::default(),
            layout_mode,
        }
    }

    /// Add a window to a new column at the end.
    /// If width is specified, the column uses that explicit width.
    pub fn add_window_to_new_column(&mut self, window_id: WindowId) -> usize {
        self.add_window_to_new_column_with_width(window_id, None)
    }

    /// Add a window to a new column with an explicit width hint.
    pub fn add_window_to_new_column_with_width(&mut self, window_id: WindowId, width: Option<u32>) -> usize {
        let tile = Tile::new(window_id);
        let mut column = Column::new();
        column.width = width;
        column.add_tile(tile);
        self.columns.push(column);
        self.columns.len() - 1
    }

    /// Add a window to an existing column
    pub fn add_window_to_column(&mut self, column_index: usize, window_id: WindowId) -> bool {
        if column_index < self.columns.len() {
            let tile = Tile::new(window_id);
            self.columns[column_index].add_tile(tile);
            true
        } else {
            false
        }
    }

    /// Remove a window from the workspace
    pub fn remove_window(&mut self, window_id: WindowId) -> Option<(usize, Tile)> {
        for (col_idx, column) in self.columns.iter_mut().enumerate() {
            if let Some(tile) = column.remove_tile(window_id) {
                // Remove empty columns
                if column.tiles.is_empty() {
                    self.columns.remove(col_idx);
                }
                return Some((col_idx, tile));
            }
        }
        None
    }

    /// Find which column a window is in
    pub fn find_window_column(&self, window_id: WindowId) -> Option<usize> {
        for (col_idx, column) in self.columns.iter().enumerate() {
            if column.tiles.iter().any(|t| t.window_id == window_id) {
                return Some(col_idx);
            }
        }
        None
    }

    /// niri-parity "consume into column": take the focused window out of its
    /// current column and append it to the column immediately to the right.
    /// Returns true on success, false when there is no column to the right
    /// or the window cannot be located. Empty source columns are dropped.
    pub fn consume_window_into_right_column(&mut self, window_id: WindowId) -> bool {
        let Some(src_col_idx) = self.find_window_column(window_id) else { return false };
        // Need a column to the right of the source.
        if src_col_idx + 1 >= self.columns.len() {
            return false;
        }
        // Remove the tile from its source column.
        let Some(tile) = self.columns[src_col_idx].remove_tile(window_id) else { return false };
        let source_now_empty = self.columns[src_col_idx].tiles.is_empty();
        // If the source becomes empty after removal it is dropped, shifting
        // the target index left by one.
        let target_col_idx = if source_now_empty {
            self.columns.remove(src_col_idx);
            src_col_idx // src+1 - 1 (shifted)
        } else {
            src_col_idx + 1
        };
        if let Some(target_col) = self.columns.get_mut(target_col_idx) {
            target_col.add_tile(tile);
            true
        } else {
            // Should not happen because we checked bounds above, but be safe.
            false
        }
    }

    /// niri-parity "expel from column": take the focused window out of its
    /// current column (only meaningful when the column hosts multiple tiles)
    /// and make it the sole tile of a brand-new column inserted immediately
    /// to the right of the source. Returns true on success.
    pub fn expel_window_into_new_column(&mut self, window_id: WindowId) -> bool {
        let Some(src_col_idx) = self.find_window_column(window_id) else { return false };
        // No-op when the source column has only one tile — there is nothing
        // to expel (the column would just be cloned and the source removed).
        if self.columns.get(src_col_idx).map(|c| c.tiles.len()).unwrap_or(0) <= 1 {
            return false;
        }
        let Some(tile) = self.columns[src_col_idx].remove_tile(window_id) else { return false };
        let mut new_col = Column::new();
        new_col.add_tile(tile);
        let insert_at = src_col_idx + 1;
        self.columns.insert(insert_at, new_col);
        true
    }

    /// Move a window to a different column
    pub fn move_window(&mut self, window_id: WindowId, target_col: usize, target_index: usize) -> bool {
        // Find and remove the window
        let removed = self.remove_window(window_id);
        if removed.is_none() {
            return false;
        }
        let (_, tile) = removed.unwrap();

        // Clamp target_col to valid insert range: 0..=len (not arbitrary beyond len).
        // `remove_window` may have already reduced `columns.len()` by 1 if the source
        // column became empty, so clamp *after* removal.
        let target_col = target_col.min(self.columns.len());

        if target_col == self.columns.len() {
            // Append a new column for this tile
            let mut col = Column::new();
            col.add_tile(tile);
            self.columns.push(col);
        } else {
            // Insert at target position within an existing column
            let col = &mut self.columns[target_col];
            if target_index >= col.tiles.len() {
                col.add_tile(tile);
            } else {
                col.tiles.insert(target_index, tile);
            }
        }

        // Reap any empty columns that may have been left behind
        self.columns.retain(|c| !c.tiles.is_empty());

        true
    }

    /// Scroll the workspace view
    pub fn scroll_left(&mut self, amount: i32) {
        self.scroll_offset.x = self.scroll_offset.x.saturating_sub(amount).max(0);
    }

    pub fn scroll_right(&mut self, amount: i32) {
        self.scroll_offset.x = self.scroll_offset.x.saturating_add(amount);
    }

    pub fn set_scroll_offset(&mut self, offset: Point) {
        self.scroll_offset.x = offset.x.max(0);
        self.scroll_offset.y = offset.y.max(0);
    }

    /// Get the total width of all columns, respecting per-column explicit widths.
    pub fn total_width(&self, column_width: i32, gap: i32) -> i32 {
        if self.columns.is_empty() {
            return 0;
        }
        self.columns.iter().enumerate().map(|(i, col)| {
            let w = col.width.map(|w| w as i32).unwrap_or(column_width);
            w + if i < self.columns.len() - 1 { gap } else { 0 }
        }).sum()
    }

    /// Ensure a specific column is visible by adjusting scroll offset.
    /// Supports variable-width columns.
    /// Returns true if scroll offset was changed.
    pub fn scroll_to_column(&mut self, column_idx: usize, default_width: i32, gap: i32, view_width: i32) -> bool {
        if column_idx >= self.columns.len() || view_width <= 0 {
            return false;
        }
        let (col_start, col_width) = self.column_x_and_width(column_idx, default_width, gap);
        let col_end = col_start + col_width;
        let view_start = self.scroll_offset.x;
        let view_end = view_start + view_width;

        let old_offset = self.scroll_offset.x;

        // Already fully visible — no scroll needed
        if col_start >= view_start && col_end <= view_end {
            return false;
        }

        if col_start < view_start {
            // Column is off-screen to the left: scroll left so the column's
            // left edge aligns with the view's left edge.
            self.scroll_offset.x = col_start.max(0);
        } else {
            // Column is off-screen to the right: center the column in the view.
            let col_center = col_start + col_width / 2;
            let target_scroll = (col_center - view_width / 2).max(0);
            self.scroll_offset.x = target_scroll;
        }

        self.scroll_offset.x != old_offset
    }

    /// Clamp scroll offset so it doesn't go below 0 or past the last column.
    /// Supports variable-width columns.
    pub fn clamp_scroll(&mut self, default_width: i32, gap: i32, view_width: i32) {
        if self.columns.is_empty() {
            self.scroll_offset.x = 0;
            return;
        }
        // Compute total width with variable column widths
        let total: i32 = self.columns.iter().enumerate().map(|(i, col)| {
            let w = col.width.map(|w| w as i32).unwrap_or(default_width);
            w + if i < self.columns.len() - 1 { gap } else { 0 }
        }).sum();
        // Allow scrolling until the last column's right edge
        // reaches the left edge of the view. This gives a niri-like
        // "infinite scroll" feel where you can always scroll right.
        let max_scroll = total.max(view_width) - view_width;
        self.scroll_offset.x = self.scroll_offset.x.clamp(0, max_scroll);
    }

    /// Compute the x-position of a column index given per-column widths.
    /// Returns (column_start_x, column_width).
    pub fn column_x_and_width(&self, col_idx: usize, default_width: i32, gap: i32) -> (i32, i32) {
        let mut x = 0i32;
        for (i, col) in self.columns.iter().enumerate() {
            let w = col.width.map(|w| w as i32).unwrap_or(default_width);
            if i == col_idx {
                return (x, w);
            }
            x += w + gap;
        }
        (0, default_width)
    }

    /// Get column at a specific x position, accounting for per-column variable widths.
    pub fn column_at_x(&self, x: i32, column_width: i32, gap: i32) -> Option<usize> {
        if self.columns.is_empty() {
            return None;
        }

        let adjusted_x = x + self.scroll_offset.x;
        let mut col_left = 0i32;
        for (i, col) in self.columns.iter().enumerate() {
            let w = col.width.map(|w| w as i32).unwrap_or(column_width);
            let col_right = col_left + w;
            if adjusted_x >= col_left && adjusted_x < col_right {
                return Some(i);
            }
            col_left = col_right + gap;
        }
        None
    }

    /// Return the calculated x position of a window given per-column layout.
    /// `default_width` is the fallback column width when a column has no explicit width.
    /// Returns `None` if the window is not found.
    pub fn current_position_for(&self, window_id: WindowId, default_width: i32, gap: i32) -> Option<crate::utils::Rect> {
        let mut x = 0i32;
        for col in &self.columns {
            let w = col.width.map(|w| w as i32).unwrap_or(default_width);
            for tile in &col.tiles {
                if tile.window_id == window_id {
                    return Some(crate::utils::Rect::new(x, 0, w as u32, tile.size.h));
                }
            }
            x += w + gap;
        }
        None
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn get_column(&self, index: usize) -> Option<&Column> {
        self.columns.get(index)
    }

    pub fn get_column_mut(&mut self, index: usize) -> Option<&mut Column> {
        self.columns.get_mut(index)
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the layout for a single column
pub fn compute_column_layout(column: &Column, column_rect: Rect, gap: i32) -> Vec<(WindowId, Rect)> {
    let tile_count = column.tile_count();
    if tile_count == 0 {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(tile_count);
    let col_height = column_rect.size.h as i32;
    let total_gap = gap * (tile_count as i32 - 1).max(0);
    let tile_height = (col_height - total_gap) / tile_count as i32;

    for (i, tile) in column.tiles.iter().enumerate() {
        let y = column_rect.loc.y + (i as i32 * (tile_height + gap));
        let rect = Rect::new(column_rect.loc.x, y, column_rect.size.w, tile_height as u32);
        result.push((tile.window_id, rect));
    }

    result
}

/// Compute the layout for an entire workspace
pub fn compute_workspace_layout(
    workspace: &Workspace,
    output_rect: Rect,
    column_width: i32,
    column_gap: i32,
    window_gap: i32,
) -> Vec<(WindowId, Rect)> {
    let mut result = Vec::new();
    let scroll_offset = workspace.scroll_offset;

    for (col_idx, column) in workspace.columns.iter().enumerate() {
        // Calculate column X position
        let x = output_rect.loc.x + (col_idx as i32 * (column_width + column_gap)) - scroll_offset.x;

        // Skip columns outside the visible area
        if x + column_width < output_rect.loc.x || x > output_rect.loc.x + output_rect.size.w as i32 {
            continue;
        }

        let column_rect = Rect::new(x, output_rect.loc.y, column_width as u32, output_rect.size.h);
        let col_tiles = compute_column_layout(column, column_rect, window_gap);
        result.extend(col_tiles);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_new() {
        let ws: Workspace = Workspace::new();
        assert_eq!(ws.column_count(), 0);
        assert_eq!(ws.scroll_offset.x, 0);
    }

    #[test]
    fn test_add_column() {
        let mut ws: Workspace = Workspace::new();
        let idx = ws.add_window_to_new_column(WindowId::new(1));
        assert_eq!(idx, 0);
        assert_eq!(ws.column_count(), 1);
    }

    #[test]
    fn test_scroll() {
        let mut ws: Workspace = Workspace::new();
        ws.scroll_right(100);
        assert_eq!(ws.scroll_offset.x, 100);
        ws.scroll_left(50);
        assert_eq!(ws.scroll_offset.x, 50);
        ws.scroll_left(100);
        // Scroll offset should not go below 0
        assert_eq!(ws.scroll_offset.x, 0);
    }

    #[test]
    fn test_remove_window() {
        let mut ws: Workspace = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        assert_eq!(ws.column_count(), 2);
        
        let removed = ws.remove_window(WindowId::new(1));
        assert!(removed.is_some());
        assert_eq!(ws.column_count(), 1);
    }

    #[test]
    fn test_compute_layout() {
        let mut ws: Workspace = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        
        let output_rect = Rect::new(0, 0, 1920, 1080);
        let positions = compute_workspace_layout(&ws, output_rect, 600, 16, 8);
        
        assert_eq!(positions.len(), 2);
    }

    #[test]
    fn test_workspace_column_at_x() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        ws.add_window_to_new_column(WindowId::new(3));

        // col0 at x=0, col1 at x=808, col2 at x=1616
        let idx = ws.column_at_x(0, 800, 8);
        assert_eq!(idx, Some(0));
        let idx = ws.column_at_x(808, 800, 8);
        assert_eq!(idx, Some(1));
        let idx = ws.column_at_x(1616, 800, 8);
        assert_eq!(idx, Some(2));
        let idx = ws.column_at_x(5000, 800, 8);
        assert_eq!(idx, None);
    }

    #[test]
    fn test_workspace_column_at_x_empty() {
        let ws = Workspace::new();
        assert_eq!(ws.column_at_x(0, 800, 8), None);
    }

    #[test]
    fn test_workspace_scroll_to_column() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        ws.add_window_to_new_column(WindowId::new(3));

        // View=800, col_width=500, gap=8
        // Col2 starts at 1016, outside view
        let scrolled = ws.scroll_to_column(2, 500, 8, 800);
        assert!(scrolled);
        assert!(ws.scroll_offset.x > 0);

        // Scrolling back to col0 should work
        let scrolled = ws.scroll_to_column(0, 500, 8, 800);
        assert!(scrolled);
        assert_eq!(ws.scroll_offset.x, 0);
    }

    #[test]
    fn test_workspace_scroll_to_column_already_visible() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        // View=1920, everything visible
        let scrolled = ws.scroll_to_column(1, 800, 8, 1920);
        assert!(!scrolled); // Already visible, no scroll needed
    }

    #[test]
    fn test_workspace_total_width() {
        let mut ws = Workspace::new();
        assert_eq!(ws.total_width(500, 8), 0);
        ws.add_window_to_new_column(WindowId::new(1));
        assert_eq!(ws.total_width(500, 8), 500);
        ws.add_window_to_new_column(WindowId::new(2));
        assert_eq!(ws.total_width(500, 8), 1008);
        ws.add_window_to_new_column(WindowId::new(3));
        assert_eq!(ws.total_width(500, 8), 1516);
    }

    #[test]
    fn test_workspace_move_window() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        ws.add_window_to_new_column(WindowId::new(3));
        assert_eq!(ws.columns.len(), 3);

        // Move window 1 from col0 to col2
        let moved = ws.move_window(WindowId::new(1), 1, 0);
        assert!(moved);
        // Col0 removed (was empty), now 2 columns
        assert_eq!(ws.columns.len(), 2);
    }

    #[test]
    fn test_workspace_find_window_column() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        assert_eq!(ws.find_window_column(WindowId::new(1)), Some(0));
        assert_eq!(ws.find_window_column(WindowId::new(2)), Some(1));
        assert_eq!(ws.find_window_column(WindowId::new(99)), None);
    }

    #[test]
    fn test_workspace_column_x_and_width_variable() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column_with_width(WindowId::new(1), Some(400));
        ws.add_window_to_new_column_with_width(WindowId::new(2), Some(600));
        ws.add_window_to_new_column_with_width(WindowId::new(3), None); // uses default

        let (x0, w0) = ws.column_x_and_width(0, 500, 8);
        assert_eq!(x0, 0);
        assert_eq!(w0, 400);

        let (x1, w1) = ws.column_x_and_width(1, 500, 8);
        assert_eq!(x1, 408); // 400 + 8
        assert_eq!(w1, 600);

        let (x2, w2) = ws.column_x_and_width(2, 500, 8);
        assert_eq!(x2, 1016); // 400 + 8 + 600 + 8
        assert_eq!(w2, 500);
    }

    #[test]
    fn test_workspace_clamp_scroll() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));

        // Set an unreasonably large scroll offset
        ws.scroll_offset.x = 10000;
        ws.clamp_scroll(500, 8, 1920);
        // Total = 1008, max_scroll = max(1008 - 1920, 0) = 0
        assert_eq!(ws.scroll_offset.x, 0);
    }

    #[test]
    fn test_workspace_clamp_scroll_with_overflow() {
        let mut ws = Workspace::new();
        for i in 1..=10 {
            ws.add_window_to_new_column(WindowId::new(i));
        }
        // Total = 10*500 + 9*8 = 5072
        // Max scroll = 5072 - 1920 = 3152
        ws.scroll_offset.x = 10000;
        ws.clamp_scroll(500, 8, 1920);
        assert_eq!(ws.scroll_offset.x, 3152);
    }

    #[test]
    fn test_compute_column_layout() {
        let mut col = Column::new();
        col.add_tile(Tile::new(WindowId::new(1)));
        col.add_tile(Tile::new(WindowId::new(2)));
        col.add_tile(Tile::new(WindowId::new(3)));

        let rect = Rect::new(100, 0, 500, 900);
        let layout = compute_column_layout(&col, rect, 10);
        assert_eq!(layout.len(), 3);
        // Each tile gets (900 - 2*10) / 3 = 293
        assert_eq!(layout[0].1.size.h, 293);
        // Check y positions
        assert_eq!(layout[0].1.loc.y, 0);
        assert_eq!(layout[1].1.loc.y, 303); // 293 + 10
        assert_eq!(layout[2].1.loc.y, 606); // 303 + 293 + 10
    }

    #[test]
    fn test_compute_workspace_layout_scroll() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        ws.add_window_to_new_column(WindowId::new(3));

        // With no scroll, col0 at x=0
        let positions = compute_workspace_layout(&ws, Rect::new(0, 0, 1920, 1080), 800, 8, 10);
        assert_eq!(positions[0].1.loc.x, 0);

        // With scroll offset
        ws.scroll_offset.x = 100;
        let positions = compute_workspace_layout(&ws, Rect::new(0, 0, 1920, 1080), 800, 8, 10);
        // Col0 is now at x = 0 - 100 = -100 (partially off-screen)
        assert_eq!(positions[0].1.loc.x, -100);
    }


    #[test]
    fn test_scroll_to_column_centers() {
        let mut ws = Workspace::new();
        // 5 columns, each 500px, 16px gaps
        // Total = 5*500 + 4*16 = 2564
        for i in 1..=5 {
            ws.add_window_to_new_column(WindowId::new(i));
        }

        // View=800px. Scroll to column 4 (last one).
        let scrolled = ws.scroll_to_column(4, 500, 16, 800);
        assert!(scrolled);
        // Column 4 starts at x=2064, center=2314
        // Target scroll = 2314 - 400 = 1914
        // But clamped by max_scroll = 2564 - 800 = 1764
        // So scroll should be clamped to 1764
        // Clamp scroll to valid range
        ws.clamp_scroll(500, 16, 800);
        assert!(ws.scroll_offset.x > 0);
        assert!(ws.scroll_offset.x <= 1764, "scroll={} > 1764", ws.scroll_offset.x);
    }

    #[test]
    fn test_scroll_to_column_first_visible() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));

        // Both columns fit in 1920px view
        let scrolled = ws.scroll_to_column(1, 500, 16, 1920);
        assert!(!scrolled); // Already visible, no scroll needed
    }

    // --- Tabbed column tests ---

    #[test]
    fn test_column_default_stacked() {
        let col = Column::new();
        assert_eq!(col.display, ColumnDisplay::Stacked);
    }

    #[test]
    fn test_column_switch_to_tabbed() {
        let mut col = Column::new();
        col.add_tile(Tile::new(WindowId::new(1)));
        col.add_tile(Tile::new(WindowId::new(2)));
        col.add_tile(Tile::new(WindowId::new(3)));

        col.switch_to_tabbed();
        assert_eq!(col.display, ColumnDisplay::Tabbed { active_tab: 0 });
        assert_eq!(col.visible_tile_indices(), vec![0]);

        col.switch_to_stacked();
        assert_eq!(col.display, ColumnDisplay::Stacked);
        assert_eq!(col.visible_tile_indices(), vec![0, 1, 2]);
    }

    #[test]
    fn test_column_next_tab_wraps() {
        let mut col = Column::new();
        col.add_tile(Tile::new(WindowId::new(10)));
        col.add_tile(Tile::new(WindowId::new(11)));
        col.add_tile(Tile::new(WindowId::new(12)));
        col.switch_to_tabbed();

        assert_eq!(col.visible_tile_indices(), vec![0]);
        col.next_tab();
        assert_eq!(col.visible_tile_indices(), vec![1]);
        col.next_tab();
        assert_eq!(col.visible_tile_indices(), vec![2]);
        // Wrap around: next from last goes to 0.
        col.next_tab();
        assert_eq!(col.visible_tile_indices(), vec![0]);

        // prev_tab wraps backwards.
        col.prev_tab();
        assert_eq!(col.visible_tile_indices(), vec![2]);
    }

    // --- LayoutElement trait tests ---

    #[test]
    fn test_layout_element_for_tile() {
        use super::super::LayoutElement;
        let tile = Tile::new(WindowId::new(42));
        assert_eq!(tile.window_id(), WindowId::new(42));
        // bounds() returns cached_bounds which starts as default (all-zero).
        assert_eq!(tile.bounds(), Rect::default());
        assert_eq!(tile.min_size(), Size::new(50, 50));
        assert!(tile.max_size().is_none());
        assert!(!tile.is_focused());
        assert!(!tile.is_urgent());
        // Fresh tile has never sent a configure, so intent = ShouldSend.
        use super::super::ConfigureIntent;
        assert_eq!(tile.configure_intent(), ConfigureIntent::ShouldSend);
    }

    // --- Item 5: WorkspaceLayout ---

    #[test]
    fn test_workspace_layout_mode_default_is_scrolling() {
        let ws = Workspace::new();
        assert_eq!(ws.layout_mode, WorkspaceLayout::Scrolling);
    }

    #[test]
    fn test_workspace_layout_from_str() {
        assert_eq!(WorkspaceLayout::from_str("bstack"), WorkspaceLayout::BStack);
        assert_eq!(WorkspaceLayout::from_str("bottom-stack"), WorkspaceLayout::BStack);
        assert_eq!(WorkspaceLayout::from_str("spiral"), WorkspaceLayout::Spiral);
        assert_eq!(WorkspaceLayout::from_str("tile"), WorkspaceLayout::Scrolling);
        assert_eq!(WorkspaceLayout::from_str(""), WorkspaceLayout::Scrolling);
        assert_eq!(WorkspaceLayout::from_str("BSTACK"), WorkspaceLayout::BStack);
    }

    #[test]
    fn test_workspace_with_layout() {
        let ws = Workspace::with_layout(WorkspaceLayout::BStack);
        assert_eq!(ws.layout_mode, WorkspaceLayout::BStack);
        assert!(ws.columns.is_empty());
    }

}
