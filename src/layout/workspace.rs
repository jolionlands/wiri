use super::{ConfigureIntent, ConfigureThrottle, LayoutElement};
use crate::utils::{Point, Rect, Size, WindowId};

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
    /// The height a window rule asks for (`default-window-height`, the older
    /// `default-height`), in pixels of the whole tile: a column of several windows gives
    /// this tile that much and shares the rest among the others by their weights.  A
    /// column of one window always fills the view.  The user's own `set-window-height`
    /// takes it away.
    pub preferred_height: Option<u32>,
    /// niri-style relative tile-height weight inside a column. When
    /// every tile in a column carries the default weight of 1.0 the
    /// engine distributes height equally (current behavior). Adjusting
    /// the weight via `Action::GrowTileHeight` / `ShrinkTileHeight`
    /// reallocates the column's vertical real estate proportionally.
    /// Always >= 0.1 by construction in the resize helpers.
    pub height_weight: f32,
    /// The narrowest the window can be made, as a visible width in pixels: what the window
    /// answers to `WM_GETMINMAXINFO` (a Windows Terminal window will not go below about
    /// 462 px), set by `TilingEngine::note_window_min_width`.  0 = no limit known.  The
    /// column of the tile is never narrower than this: asking the window for less only
    /// makes it overlap its neighbour.  A window rule's `min-width` counts as well (the
    /// larger of the two).
    pub min_width: u32,
    /// The widest the tile may be, in pixels of the whole tile (a window rule's `max-width`;
    /// 0 = no limit).  The column of the tile is never wider than the smallest limit of its
    /// tiles, unless some tile cannot be narrower than that (the minimum wins).
    pub max_width: u32,
    /// The least and the most the tile may be tall (`min-height` / `max-height`, in pixels
    /// of the whole tile; 0 = no limit).
    pub min_height: u32,
    pub max_height: u32,
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
            min_width: 0,
            max_width: 0,
            min_height: 0,
            max_height: 0,
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
    /// Remembered independently of monitor MRU history (which is bounded). Window ids
    /// survive tile insertion, reorder and column swaps without index repair.
    pub active_window: Option<WindowId>,
    /// Requested proportion of the view; `width` is its resolved pixel cache.
    pub width_proportion: Option<f64>,
    pub width_fixed_logical: Option<f64>,
    /// Explicit column width in pixels (None = use layout default)
    pub width: Option<u32>,
    /// Display mode: stacked (default) or tabbed.
    pub display: ColumnDisplay,
    /// niri `maximize-column`: the column is as wide as the view (`width` holds
    /// that width) until it is toggled back or given a width of its own.
    /// Separate from `SizingMode::Fullscreen`, which applies to a single window
    /// across the whole monitor including reserved bars.
    pub maximized: bool,
    /// The width the column had when it was maximized (`Some(None)`: the
    /// layout default), to restore when it is toggled back.
    pub pre_max_width: Option<Option<u32>>,
    /// The preset (index into niri's `preset-column-widths`) that
    /// `switch-preset-column-width` gave the column last; the next switch goes on from
    /// there.  Any other width forgets it.
    pub preset_idx: Option<usize>,
}

impl Column {
    pub fn new() -> Self {
        Self {
            tiles: Vec::new(),
            active_window: None,
            width_proportion: None,
            width_fixed_logical: None,
            width: None,
            display: ColumnDisplay::Stacked,
            maximized: false,
            pre_max_width: None,
            preset_idx: None,
        }
    }

    /// Give the column an explicit width.  This ends a maximized state: a column
    /// that has a width of its own is not "as wide as the view" any more.
    pub fn set_width(&mut self, width: Option<u32>) {
        self.maximized = false;
        self.pre_max_width = None;
        self.preset_idx = None;
        self.width_proportion = None;
        self.width_fixed_logical = None;
        self.width = width;
    }

    pub fn set_proportion(&mut self, proportion: f64, view: i32, gap: i32) {
        self.set_width(Some(
            crate::input::proportion_px(view, gap, proportion).clamp(50, 100_000) as u32,
        ));
        self.width_proportion = Some(proportion);
    }

    pub fn set_fixed_logical(&mut self, logical: f64, scale: f64, border: i32) {
        self.set_width(Some(
            ((logical * scale).round() as i32 + border).clamp(50, 100_000) as u32,
        ));
        self.width_fixed_logical = Some(logical);
    }

    pub fn active_tile(&self) -> Option<WindowId> {
        self.active_window
            .filter(|id| self.index_of(*id).is_some())
            .or_else(|| self.tiles.first().map(|tile| tile.window_id))
    }

    /// niri `toggle_full_width` (`maximize-column`): make the column `view_width`
    /// wide, remembering the width it had; toggling again gives that width back.
    /// Works for a column of any number of tiles.
    pub fn toggle_maximized(&mut self, view_width: u32) {
        if self.maximized {
            self.maximized = false;
            self.width = self.pre_max_width.take().unwrap_or(None);
        } else {
            self.pre_max_width = Some(self.width);
            self.maximized = true;
            self.width = Some(view_width);
        }
    }

    /// The narrowest this column can be: the widest minimum of its windows (0 = none known).
    pub fn min_width(&self) -> u32 {
        self.tiles.iter().map(|t| t.min_width).max().unwrap_or(0)
    }

    /// The widest this column may be: the smallest limit among its windows (0 = none).
    /// A minimum above it wins (see [`Column::resolved_width`]).
    pub fn max_width(&self) -> u32 {
        self.tiles
            .iter()
            .map(|t| t.max_width)
            .filter(|w| *w > 0)
            .min()
            .unwrap_or(0)
    }

    /// The width the column has: its own width, or `default_width` when it has none, but
    /// never less than its windows can be made narrow and never more than they allow
    /// (when the two clash, the minimum wins: a window that cannot be narrower than its
    /// column is wide would lie over the next one).
    pub fn resolved_width(&self, default_width: i32) -> i32 {
        let min = self.min_width() as i32;
        let mut width = self.width.map(|w| w as i32).unwrap_or(default_width);
        let max = self.max_width() as i32;
        if max > 0 {
            width = width.min(max);
        }
        width.max(min)
    }

    /// Keep the active tab of a tabbed column on a tile that exists.
    pub fn clamp_active_tab(&mut self) {
        let last = self.tiles.len().saturating_sub(1);
        if let ColumnDisplay::Tabbed { active_tab } = &mut self.display {
            *active_tab = (*active_tab).min(last);
        }
    }

    pub fn add_tile(&mut self, tile: Tile) {
        self.tiles.push(tile);
    }

    pub fn remove_tile(&mut self, window_id: WindowId) -> Option<Tile> {
        let index = self.index_of(window_id)?;
        let tile = self.tiles.remove(index);
        if let ColumnDisplay::Tabbed { active_tab } = &mut self.display {
            if index < *active_tab {
                *active_tab -= 1;
            }
        }
        if self.active_window == Some(window_id) {
            self.active_window = self
                .tiles
                .get(index.min(self.tiles.len().saturating_sub(1)))
                .map(|tile| tile.window_id);
        }
        self.clamp_active_tab();
        Some(tile)
    }

    /// Show `window_id` if this is a tabbed column. All focus paths use this, including
    /// shell activation and consume/expel, so the focused HWND can never be a hidden tab.
    pub fn select_window(&mut self, window_id: WindowId) {
        if let Some(index) = self.index_of(window_id) {
            self.active_window = Some(window_id);
            if let ColumnDisplay::Tabbed { active_tab } = &mut self.display {
                *active_tab = index;
            }
        }
    }

    /// The tile the column shows as active: the visible tab of a tabbed column, otherwise the
    /// remembered active tile (niri keeps one per column).
    pub fn active_window(&self) -> Option<WindowId> {
        match self.display {
            ColumnDisplay::Tabbed { active_tab } => self.tiles.get(active_tab).map(|t| t.window_id),
            ColumnDisplay::Stacked => self.active_tile(),
        }
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

/// Where a window sat in a workspace's strip, kept while the window is out of the
/// layout for a while (minimized) so that it can be put back where it was.
///
/// Column and tile indexes shift while the window is away, so the place is also
/// described by the windows around it: the ones above and below it in its column
/// and the windows of the columns on either side (all nearest first).  Windows
/// minimized earlier are folded in by [`WindowSlot::learn_from`], so a whole
/// desktop that was minimized at once (Win+D) comes back in its old order
/// whichever window is restored first.
#[derive(Debug, Clone)]
pub struct WindowSlot {
    pub window_id: WindowId,
    pub tile: Tile,
    /// Index of its column and of the tile inside it when it left.
    pub column: usize,
    pub index: usize,
    /// The column's own settings; they are lost with the column when this was its
    /// only window.
    pub column_width: Option<u32>,
    pub column_width_proportion: Option<f64>,
    pub column_width_fixed_logical: Option<f64>,
    pub column_display: ColumnDisplay,
    pub column_maximized: bool,
    /// The width the column had before it was maximized (see [`Column::pre_max_width`]).
    pub column_pre_max_width: Option<Option<u32>>,
    /// Windows above it in its column, nearest first.
    pub above: Vec<WindowId>,
    /// Windows below it in its column, nearest first.
    pub below: Vec<WindowId>,
    /// Windows of the columns to its left, nearest first.
    pub left_of: Vec<WindowId>,
    /// Windows of the columns to its right, nearest first.
    pub right_of: Vec<WindowId>,
}

/// Put `other` into `list` (windows on one side of a window, nearest first).  `beyond`
/// are the windows on the far side of `other`: `other` is nearer than any of them.
fn insert_nearest_first(list: &mut Vec<WindowId>, other: WindowId, beyond: &[WindowId]) {
    let at = list
        .iter()
        .position(|w| beyond.contains(w))
        .unwrap_or(list.len());
    list.insert(at, other);
}

impl WindowSlot {
    /// Fold in a window that was minimized earlier (`earlier`): where it sat relative
    /// to this one is what the earlier slot recorded about this window.
    pub fn learn_from(&mut self, earlier: &WindowSlot) {
        let other = earlier.window_id;
        if [&self.above, &self.below, &self.left_of, &self.right_of]
            .iter()
            .any(|list| list.contains(&other))
        {
            return;
        }
        let me = self.window_id;
        if earlier.below.contains(&me) {
            insert_nearest_first(&mut self.above, other, &earlier.above);
        } else if earlier.above.contains(&me) {
            insert_nearest_first(&mut self.below, other, &earlier.below);
        } else if earlier.right_of.contains(&me) {
            insert_nearest_first(&mut self.left_of, other, &earlier.left_of);
        } else if earlier.left_of.contains(&me) {
            insert_nearest_first(&mut self.right_of, other, &earlier.right_of);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    /// Session-stable identity, independent of output and visible position.
    pub ipc_id: u64,
    pub name: Option<String>,
    pub columns: Vec<Column>,
    /// The active column is anchored by its remembered active window.
    pub active_window: Option<WindowId>,
    /// Last-focused floating layer selection; tiled focus clears this marker.
    pub active_float: Option<WindowId>,
    pub scroll_offset: Point,
    /// Per-workspace layout algorithm; defaults to `Scrolling`.
    pub layout_mode: WorkspaceLayout,
}

impl Workspace {
    pub fn new() -> Self {
        Self::with_layout(WorkspaceLayout::Scrolling)
    }

    /// Create a new workspace with an explicit layout mode.
    pub fn with_layout(layout_mode: WorkspaceLayout) -> Self {
        // Id allocation only occurs on creation; it is not synchronization.
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            ipc_id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            name: None,
            columns: Vec::new(),
            active_window: None,
            active_float: None,
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
    pub fn add_window_to_new_column_with_width(
        &mut self,
        window_id: WindowId,
        width: Option<u32>,
    ) -> usize {
        let tile = Tile::new(window_id);
        let mut column = Column::new();
        column.width = width;
        column.add_tile(tile);
        self.columns.push(column);
        self.columns.len() - 1
    }

    /// niri's default insertion: right of this workspace's active column, not
    /// the end of the strip. The empty workspace starts at column zero.
    pub fn insert_window_after_active(&mut self, window_id: WindowId, width: Option<u32>) -> usize {
        let at = self
            .active_window
            .and_then(|id| self.find_window_column(id))
            .map_or(self.columns.len(), |col| col + 1);
        let mut column = Column::new();
        column.width = width;
        column.add_tile(Tile::new(window_id));
        self.columns.insert(at, column);
        self.remember_focus(window_id);
        at
    }

    /// Transfer a tile to a new strip without reducing its requested width to
    /// the old output's pixel cache. Neighbour anchors only apply in the old strip.
    pub fn insert_slot_after_active(&mut self, mut slot: WindowSlot) -> usize {
        slot.column = self
            .active_window
            .and_then(|id| self.find_window_column(id))
            .map_or(self.columns.len(), |col| col + 1);
        slot.above.clear();
        slot.below.clear();
        slot.left_of.clear();
        slot.right_of.clear();
        let id = slot.window_id;
        let column = self.put_back(slot);
        self.remember_focus(id);
        column
    }

    pub fn remember_focus(&mut self, window_id: WindowId) {
        if let Some(index) = self.find_window_column(window_id) {
            self.active_window = Some(window_id);
            let column = &mut self.columns[index];
            column.active_window = Some(window_id);
            if let ColumnDisplay::Tabbed { active_tab } = &mut column.display {
                *active_tab = column
                    .tiles
                    .iter()
                    .position(|t| t.window_id == window_id)
                    .unwrap_or(0);
            }
        }
    }

    pub fn remembered_focus(&self) -> Option<(usize, WindowId)> {
        self.active_window
            .and_then(|id| self.find_window_column(id).map(|col| (col, id)))
            .or_else(|| {
                self.columns
                    .iter()
                    .enumerate()
                    .find_map(|(col, c)| c.active_tile().map(|id| (col, id)))
            })
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
        if self.active_float == Some(window_id) {
            self.active_float = None;
        }
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

    /// Record where `window_id` sits, so that [`Self::put_back`] can restore it after
    /// the window was taken out (`remove_window`).  `None` when it is not here.
    pub fn slot_of(&self, window_id: WindowId) -> Option<WindowSlot> {
        let column = self.find_window_column(window_id)?;
        let col = &self.columns[column];
        let index = col.index_of(window_id)?;
        let ids_of = |c: &Column| c.tiles.iter().map(|t| t.window_id).collect::<Vec<_>>();
        Some(WindowSlot {
            window_id,
            tile: col.tiles[index].clone(),
            column,
            index,
            column_width: col.width,
            column_width_proportion: col.width_proportion,
            column_width_fixed_logical: col.width_fixed_logical,
            column_display: col.display.clone(),
            column_maximized: col.maximized,
            column_pre_max_width: col.pre_max_width,
            above: col.tiles[..index]
                .iter()
                .rev()
                .map(|t| t.window_id)
                .collect(),
            below: col.tiles[index + 1..].iter().map(|t| t.window_id).collect(),
            left_of: self.columns[..column]
                .iter()
                .rev()
                .flat_map(ids_of)
                .collect(),
            right_of: self.columns[column + 1..].iter().flat_map(ids_of).collect(),
        })
    }

    /// Put a window back where `slot` says it was.  It rejoins the column it shared
    /// with other windows if one of them is still here, at its old place among them;
    /// otherwise it gets a column of its own (with the width it had) next to the
    /// windows that were its neighbours, or, when none of them is here any more, at
    /// the column index it had.  Returns the index of the column it ended up in.
    pub fn put_back(&mut self, slot: WindowSlot) -> usize {
        let WindowSlot {
            mut tile,
            column,
            index,
            column_width,
            column_display,
            column_maximized,
            column_pre_max_width,
            above,
            below,
            left_of,
            right_of,
            ..
        } = slot;
        // The old geometry says nothing about the new place.
        tile.cached_bounds = Rect::default();

        if let Some(col) = above
            .iter()
            .chain(below.iter())
            .find_map(|w| self.find_window_column(*w))
        {
            let tiles = &mut self.columns[col].tiles;
            let position_of =
                |tiles: &Vec<Tile>, w: &WindowId| tiles.iter().position(|t| t.window_id == *w);
            let at = above
                .iter()
                .find_map(|w| position_of(tiles, w))
                .map(|i| i + 1)
                .or_else(|| below.iter().find_map(|w| position_of(tiles, w)))
                .unwrap_or(index)
                .min(tiles.len());
            tiles.insert(at, tile);
            return col;
        }

        let insert_at = left_of
            .iter()
            .find_map(|w| self.find_window_column(*w))
            .map(|c| c + 1)
            .or_else(|| right_of.iter().find_map(|w| self.find_window_column(*w)))
            .unwrap_or(column)
            .min(self.columns.len());
        let mut col = Column::new();
        col.width = column_width;
        col.width_proportion = slot.column_width_proportion;
        col.width_fixed_logical = slot.column_width_fixed_logical;
        col.maximized = column_maximized;
        col.pre_max_width = column_pre_max_width;
        col.display = match column_display {
            // One tile left: its tab is the first.
            ColumnDisplay::Tabbed { .. } => ColumnDisplay::Tabbed { active_tab: 0 },
            other => other,
        };
        col.add_tile(tile);
        self.columns.insert(insert_at, col);
        insert_at
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

    /// Take `window_id` out of the column `col` (which must have another tile) and
    /// wrap it in a new column that has the width of the one it came from.
    fn take_tile_into_new_column(&mut self, col: usize, window_id: WindowId) -> Option<Column> {
        let source = self.columns.get_mut(col)?;
        if source.tiles.len() < 2 {
            return None;
        }
        let tile = source.remove_tile(window_id)?;
        source.clamp_active_tab();
        let mut new_col = Column::new();
        new_col.width = source.width;
        new_col.width_proportion = source.width_proportion;
        new_col.width_fixed_logical = source.width_fixed_logical;
        new_col.preset_idx = source.preset_idx;
        new_col.maximized = source.maximized;
        new_col.pre_max_width = source.pre_max_width;
        new_col.add_tile(tile);
        Some(new_col)
    }

    /// niri `consume-or-expel-window-left`.  A window that is alone in its column
    /// moves to the bottom of the column on its left (and the column it leaves
    /// vanishes); a window in a column of several leaves it as a column of its own
    /// on the left of the one it left.  Returns the index of the column the window
    /// is in now, or `None` when nothing moved (a lone window in the first column).
    pub fn consume_or_expel_left(&mut self, window_id: WindowId) -> Option<usize> {
        let col = self.find_window_column(window_id)?;
        if self.columns[col].tiles.len() == 1 {
            if col == 0 {
                return None;
            }
            let lone = self.columns.remove(col);
            let tile = lone.tiles.into_iter().next()?;
            let target = col - 1;
            self.columns[target].add_tile(tile);
            Some(target)
        } else {
            let expelled = self.take_tile_into_new_column(col, window_id)?;
            self.columns.insert(col, expelled);
            Some(col)
        }
    }

    /// niri `consume-or-expel-window-right`: the mirror image of
    /// [`Self::consume_or_expel_left`] (a lone window goes to the bottom of the
    /// column on its right; one of several leaves as a column on the right).
    pub fn consume_or_expel_right(&mut self, window_id: WindowId) -> Option<usize> {
        let col = self.find_window_column(window_id)?;
        if self.columns[col].tiles.len() == 1 {
            if col + 1 >= self.columns.len() {
                return None;
            }
            let lone = self.columns.remove(col);
            let tile = lone.tiles.into_iter().next()?;
            // The column that was on the right has taken the lone window's index.
            self.columns[col].add_tile(tile);
            Some(col)
        } else {
            let expelled = self.take_tile_into_new_column(col, window_id)?;
            self.columns.insert(col + 1, expelled);
            Some(col + 1)
        }
    }

    /// niri `consume-window-into-column`: the FIRST window of the column on the
    /// right of `col` moves to the bottom of `col` (the column it leaves vanishes
    /// when it was its only window).  Returns the window that moved.
    pub fn consume_into_column(&mut self, col: usize) -> Option<WindowId> {
        if col + 1 >= self.columns.len() {
            return None;
        }
        let neighbour = &mut self.columns[col + 1];
        let window_id = neighbour.tiles.first()?.window_id;
        let tile = neighbour.tiles.remove(0);
        if neighbour.tiles.is_empty() {
            self.columns.remove(col + 1);
        } else {
            neighbour.clamp_active_tab();
        }
        self.columns[col].add_tile(tile);
        Some(window_id)
    }

    /// niri `expel-window-from-column`: the BOTTOM window of `col` becomes a column
    /// of its own on the right of it.  Returns the window that moved, or `None`
    /// when the column has a single window.
    pub fn expel_from_column(&mut self, col: usize) -> Option<WindowId> {
        let window_id = self.columns.get(col)?.tiles.last()?.window_id;
        let expelled = self.take_tile_into_new_column(col, window_id)?;
        self.columns.insert(col + 1, expelled);
        Some(window_id)
    }

    /// niri `move-window-up` / `move-window-down`: `window_id` changes places with
    /// the tile above (`delta` -1) or below (`delta` 1) it in its column.  `false`
    /// at the top / bottom of the column.
    pub fn move_tile_within_column(&mut self, window_id: WindowId, delta: i32) -> bool {
        let Some(col) = self.find_window_column(window_id) else {
            return false;
        };
        let column = &mut self.columns[col];
        let Some(from) = column.index_of(window_id) else {
            return false;
        };
        let to = from as i64 + delta as i64;
        if to < 0 || to >= column.tiles.len() as i64 {
            return false;
        }
        let to = to as usize;
        column.tiles.swap(from, to);
        // A tabbed column keeps showing the same window.
        if let ColumnDisplay::Tabbed { active_tab } = &mut column.display {
            if *active_tab == from {
                *active_tab = to;
            } else if *active_tab == to {
                *active_tab = from;
            }
        }
        true
    }

    /// Move a window to a different column
    pub fn move_window(
        &mut self,
        window_id: WindowId,
        target_col: usize,
        target_index: usize,
    ) -> bool {
        // A newly created column inherits the source width, not a fresh default.
        let width = self.find_window_column(window_id).map(|i| {
            (
                self.columns[i].width,
                self.columns[i].width_proportion,
                self.columns[i].width_fixed_logical,
            )
        });
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
            if let Some((pixels, proportion, logical)) = width {
                col.width = pixels;
                col.width_proportion = proportion;
                col.width_fixed_logical = logical;
            }
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

    /// niri `move-column-left` / `move-column-right`: the whole column at `col`
    /// (all its tiles, in order) changes places with its neighbour `delta`
    /// columns away.  Returns the column's new index, or `None` when it is
    /// already at that edge or `col` does not exist.
    pub fn move_column_by(&mut self, col: usize, delta: i32) -> Option<usize> {
        if col >= self.columns.len() {
            return None;
        }
        let target = col as i64 + delta as i64;
        if target < 0 || target >= self.columns.len() as i64 {
            return None;
        }
        let target = target as usize;
        let column = self.columns.remove(col);
        self.columns.insert(target, column);
        Some(target)
    }

    /// niri `move-column-to-first` / `move-column-to-last`: the column at `col`
    /// goes to index `target`, the columns in between shift by one.  `None` when
    /// it is already there or one of the indexes does not exist.
    pub fn move_column_to(&mut self, col: usize, target: usize) -> Option<usize> {
        if col >= self.columns.len() || target >= self.columns.len() || col == target {
            return None;
        }
        let column = self.columns.remove(col);
        self.columns.insert(target, column);
        Some(target)
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
        self.columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let w = col.resolved_width(column_width);
                w + if i < self.columns.len() - 1 { gap } else { 0 }
            })
            .sum()
    }

    /// Ensure a specific column is visible by adjusting scroll offset.
    /// Supports variable-width columns.
    /// Returns true if scroll offset was changed.
    pub fn scroll_to_column(
        &mut self,
        column_idx: usize,
        default_width: i32,
        gap: i32,
        view_width: i32,
    ) -> bool {
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

    /// Scroll the *least* amount that brings `column_idx` fully into view
    /// (niri's default `center-focused-column "never"`): a column that is
    /// already fully visible does not move the view; one hanging off the left
    /// edge is aligned to the left edge; one hanging off the right edge is
    /// aligned to the right edge, so its left-hand neighbours stay on screen.
    /// A column wider than the view is aligned to the left edge.
    /// Returns true if the scroll offset changed.
    pub fn reveal_column(
        &mut self,
        column_idx: usize,
        default_width: i32,
        gap: i32,
        view_width: i32,
    ) -> bool {
        if column_idx >= self.columns.len() || view_width <= 0 {
            return false;
        }
        let (col_start, col_width) = self.column_x_and_width(column_idx, default_width, gap);
        let col_end = col_start + col_width;
        let view_start = self.scroll_offset.x;
        let view_end = view_start + view_width;
        let old_offset = self.scroll_offset.x;

        if col_width >= view_width || col_start < view_start {
            self.scroll_offset.x = col_start.max(0);
        } else if col_end > view_end {
            self.scroll_offset.x = (col_end - view_width).max(0);
        }

        self.scroll_offset.x != old_offset
    }

    /// Pull the scroll offset back into the range in which the strip leaves no
    /// empty space at its right edge: `0 ..= total_width - view_width` (just
    /// `0` when everything fits).  Unlike [`Self::clamp_scroll`], which lets a
    /// deliberate scroll command pan until the last column reaches the leading
    /// edge, this is what keeps the strip filling the screen after windows
    /// close - otherwise the survivors stay scrolled off to the left.
    pub fn fit_scroll(&mut self, default_width: i32, gap: i32, view_width: i32) {
        if self.columns.is_empty() {
            self.scroll_offset.x = 0;
            return;
        }
        let last = self.columns.len() - 1;
        let (last_start, last_width) = self.column_x_and_width(last, default_width, gap);
        let max_scroll = (last_start + last_width - view_width).max(0);
        self.scroll_offset.x = self.scroll_offset.x.clamp(0, max_scroll);
    }

    /// Clamp scroll offset so it doesn't go below 0 or beyond the last column.
    /// Supports variable-width columns.
    pub fn clamp_scroll(&mut self, default_width: i32, gap: i32, _view_width: i32) {
        if self.columns.is_empty() {
            self.scroll_offset.x = 0;
            return;
        }

        // A scrollable strip should still pan when total content is narrower
        // than the monitor. Clamp at the last column's left edge, not at
        // total_width - viewport_width, so every column can be brought to the
        // leading edge of the viewport.
        let last_col_idx = self.columns.len().saturating_sub(1);
        let (last_col_start, _) = self.column_x_and_width(last_col_idx, default_width, gap);
        let max_scroll = last_col_start.max(0);
        self.scroll_offset.x = self.scroll_offset.x.clamp(0, max_scroll);
    }

    /// Compute the x-position of a column index given per-column widths.
    /// Returns (column_start_x, column_width).
    pub fn column_x_and_width(&self, col_idx: usize, default_width: i32, gap: i32) -> (i32, i32) {
        let mut x = 0i32;
        for (i, col) in self.columns.iter().enumerate() {
            let w = col.resolved_width(default_width);
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
            let w = col.resolved_width(column_width);
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
    pub fn current_position_for(
        &self,
        window_id: WindowId,
        default_width: i32,
        gap: i32,
    ) -> Option<crate::utils::Rect> {
        let mut x = 0i32;
        for col in &self.columns {
            let w = col.resolved_width(default_width);
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
pub fn compute_column_layout(
    column: &Column,
    column_rect: Rect,
    gap: i32,
) -> Vec<(WindowId, Rect)> {
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
        let x =
            output_rect.loc.x + (col_idx as i32 * (column_width + column_gap)) - scroll_offset.x;

        // Skip columns outside the visible area
        if x + column_width < output_rect.loc.x || x > output_rect.loc.x + output_rect.size.w as i32
        {
            continue;
        }

        let column_rect = Rect::new(
            x,
            output_rect.loc.y,
            column_width as u32,
            output_rect.size.h,
        );
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

    fn column_order(ws: &Workspace) -> Vec<Vec<isize>> {
        ws.columns
            .iter()
            .map(|c| c.tiles.iter().map(|t| t.window_id.as_isize()).collect())
            .collect()
    }

    #[test]
    fn test_workspace_move_column_swaps_with_the_neighbour() {
        let mut ws = Workspace::new();
        for id in 1..=3 {
            ws.add_window_to_new_column(WindowId::new(id));
        }
        assert_eq!(ws.move_column_by(2, -1), Some(1));
        assert_eq!(column_order(&ws), vec![vec![1], vec![3], vec![2]]);
        assert_eq!(ws.move_column_by(0, 1), Some(1));
        assert_eq!(column_order(&ws), vec![vec![3], vec![1], vec![2]]);
        // Never merges columns: the count stays the same.
        assert_eq!(ws.columns.len(), 3);
    }

    #[test]
    fn test_workspace_move_column_moves_all_stacked_tiles_together() {
        let mut ws = Workspace::new();
        for id in 1..=3 {
            ws.add_window_to_new_column(WindowId::new(id));
        }
        // Stack window 4 under window 2.
        assert!(ws.add_window_to_column(1, WindowId::new(4)));
        assert_eq!(column_order(&ws), vec![vec![1], vec![2, 4], vec![3]]);
        assert_eq!(ws.move_column_by(1, -1), Some(0));
        assert_eq!(column_order(&ws), vec![vec![2, 4], vec![1], vec![3]]);
    }

    #[test]
    fn test_workspace_move_column_stops_at_the_edges() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        assert_eq!(ws.move_column_by(0, -1), None);
        assert_eq!(ws.move_column_by(1, 1), None);
        assert_eq!(ws.move_column_by(5, -1), None);
        assert_eq!(column_order(&ws), vec![vec![1], vec![2]]);
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
        // Last column starts at x=508, even though total content fits in the
        // viewport. Scrolling can still bring that column to the leading edge.
        assert_eq!(ws.scroll_offset.x, 508);
    }

    #[test]
    fn test_workspace_clamp_scroll_with_overflow() {
        let mut ws = Workspace::new();
        for i in 1..=10 {
            ws.add_window_to_new_column(WindowId::new(i));
        }
        // Last column starts at 9 * (500 + 8) = 4572.
        ws.scroll_offset.x = 10000;
        ws.clamp_scroll(500, 8, 1920);
        assert_eq!(ws.scroll_offset.x, 4572);
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
        // New niri-like clamp allows the last column to reach the leading
        // edge, so the centered target remains valid.
        // Clamp scroll to valid range
        ws.clamp_scroll(500, 16, 800);
        assert_eq!(ws.scroll_offset.x, 1914);
    }

    #[test]
    fn test_reveal_column_scrolls_minimally_to_the_right() {
        let mut ws = Workspace::new();
        // 4 columns of 400 px, 16 px gaps, view 1350 px: columns start at
        // 0, 416, 832, 1248.
        for i in 1..=4 {
            ws.add_window_to_new_column(WindowId::new(i));
        }
        // Column 3 ends at 1648, 298 px past the view: align its right edge.
        assert!(ws.reveal_column(3, 400, 16, 1350));
        assert_eq!(ws.scroll_offset.x, 1648 - 1350);
        // Column 1 (416..816) is still fully inside [298, 1648]: no movement.
        assert!(!ws.reveal_column(1, 400, 16, 1350));
        assert_eq!(ws.scroll_offset.x, 298);
    }

    #[test]
    fn test_reveal_column_scrolls_minimally_to_the_left() {
        let mut ws = Workspace::new();
        for i in 1..=4 {
            ws.add_window_to_new_column(WindowId::new(i));
        }
        ws.scroll_offset.x = 600;
        // Column 1 starts at 416, left of the view start (600): align left edges.
        assert!(ws.reveal_column(1, 400, 16, 1350));
        assert_eq!(ws.scroll_offset.x, 416);
    }

    #[test]
    fn test_fit_scroll_keeps_the_strip_filling_the_view() {
        let mut ws = Workspace::new();
        for i in 1..=4 {
            ws.add_window_to_new_column(WindowId::new(i));
        }
        // 4 columns of 400 px + 3 gaps of 16 = 1648 px of content, view 1350:
        // the furthest useful offset is 1648 - 1350 = 298.
        ws.scroll_offset.x = 1248; // last column at the leading edge
        ws.fit_scroll(400, 16, 1350);
        assert_eq!(ws.scroll_offset.x, 298);
        ws.scroll_offset.x = -50;
        ws.fit_scroll(400, 16, 1350);
        assert_eq!(ws.scroll_offset.x, 0);
        ws.scroll_offset.x = 100;
        ws.fit_scroll(400, 16, 1350);
        assert_eq!(ws.scroll_offset.x, 100, "an in-range offset is left alone");
    }

    #[test]
    fn test_fit_scroll_resets_when_everything_fits() {
        // Two windows are left after the others closed: both must be on screen.
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        ws.scroll_offset.x = 900;
        ws.fit_scroll(400, 16, 1350);
        assert_eq!(ws.scroll_offset.x, 0);
        // No columns at all.
        let mut empty = Workspace::new();
        empty.scroll_offset.x = 500;
        empty.fit_scroll(400, 16, 1350);
        assert_eq!(empty.scroll_offset.x, 0);
    }

    #[test]
    fn test_reveal_column_visible_or_out_of_range_is_a_no_op() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        assert!(!ws.reveal_column(1, 400, 16, 1350));
        assert_eq!(ws.scroll_offset.x, 0);
        assert!(!ws.reveal_column(9, 400, 16, 1350));
        assert!(!ws.reveal_column(0, 400, 16, 0));
    }

    #[test]
    fn test_reveal_column_wider_than_view_aligns_left_edge() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(2));
        assert!(ws.reveal_column(1, 2000, 16, 1350));
        assert_eq!(ws.scroll_offset.x, 2016);
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
        assert_eq!(
            WorkspaceLayout::from_str("bottom-stack"),
            WorkspaceLayout::BStack
        );
        assert_eq!(WorkspaceLayout::from_str("spiral"), WorkspaceLayout::Spiral);
        assert_eq!(
            WorkspaceLayout::from_str("tile"),
            WorkspaceLayout::Scrolling
        );
        assert_eq!(WorkspaceLayout::from_str(""), WorkspaceLayout::Scrolling);
        assert_eq!(WorkspaceLayout::from_str("BSTACK"), WorkspaceLayout::BStack);
    }

    #[test]
    fn test_workspace_with_layout() {
        let ws = Workspace::with_layout(WorkspaceLayout::BStack);
        assert_eq!(ws.layout_mode, WorkspaceLayout::BStack);
        assert!(ws.columns.is_empty());
    }

    // --- window slots (minimize / restore) ---

    /// A workspace whose columns hold the given window ids, top to bottom.
    fn ws_with(columns: &[&[isize]]) -> Workspace {
        let mut ws = Workspace::new();
        for col in columns {
            let c = ws.add_window_to_new_column(WindowId::new(col[0]));
            for w in &col[1..] {
                ws.add_window_to_column(c, WindowId::new(*w));
            }
        }
        ws
    }

    fn ids(ws: &Workspace) -> Vec<Vec<isize>> {
        ws.columns
            .iter()
            .map(|c| c.tiles.iter().map(|t| t.window_id.as_isize()).collect())
            .collect()
    }

    fn take(ws: &mut Workspace, id: isize) -> WindowSlot {
        let slot = ws.slot_of(WindowId::new(id)).expect("window is here");
        ws.remove_window(WindowId::new(id));
        slot
    }

    #[test]
    fn test_slot_of_an_unknown_window_is_none() {
        let ws = ws_with(&[&[1], &[2]]);
        assert!(ws.slot_of(WindowId::new(9)).is_none());
    }

    #[test]
    fn test_put_back_restores_a_single_window_column_in_place() {
        let mut ws = ws_with(&[&[1], &[2], &[3]]);
        ws.columns[1].width = Some(777);
        ws.columns[1].maximized = true;
        let slot = take(&mut ws, 2);
        assert_eq!(ids(&ws), vec![vec![1], vec![3]], "no gap while it is away");
        assert_eq!(ws.put_back(slot), 1);
        assert_eq!(ids(&ws), vec![vec![1], vec![2], vec![3]]);
        assert_eq!(ws.columns[1].width, Some(777), "the column kept its width");
        assert!(ws.columns[1].maximized);
    }

    #[test]
    fn test_put_back_rejoins_the_column_it_shared() {
        let mut ws = ws_with(&[&[1], &[2, 3, 4], &[5]]);
        ws.columns[1].tiles[1].height_weight = 2.5;
        let slot = take(&mut ws, 3);
        assert_eq!(ids(&ws), vec![vec![1], vec![2, 4], vec![5]]);
        // Another column opens at the front meanwhile: every index shifts.
        ws.columns.insert(0, Column::new());
        ws.columns[0].add_tile(Tile::new(WindowId::new(9)));
        assert_eq!(ws.put_back(slot), 2);
        assert_eq!(ids(&ws), vec![vec![9], vec![1], vec![2, 3, 4], vec![5]]);
        assert_eq!(
            ws.columns[2].tiles[1].height_weight, 2.5,
            "the tile keeps its height"
        );
    }

    #[test]
    fn test_put_back_follows_its_neighbours_when_the_indexes_shifted() {
        let mut ws = ws_with(&[&[1], &[2], &[3]]);
        let slot = take(&mut ws, 2);
        // 1 closes, 7 opens at the end: the slot's own column index (1) would be wrong.
        ws.remove_window(WindowId::new(1));
        ws.add_window_to_new_column(WindowId::new(7));
        assert_eq!(ids(&ws), vec![vec![3], vec![7]]);
        // The left neighbour is gone, the right one (3) is here: in front of it.
        assert_eq!(ws.put_back(slot), 0);
        assert_eq!(ids(&ws), vec![vec![2], vec![3], vec![7]]);
    }

    #[test]
    fn test_put_back_uses_the_old_column_index_when_every_neighbour_is_gone() {
        let mut ws = ws_with(&[&[1], &[2], &[3]]);
        let slot = take(&mut ws, 2);
        for gone in [1, 3] {
            ws.remove_window(WindowId::new(gone));
        }
        ws.add_window_to_new_column(WindowId::new(7));
        assert_eq!(ws.put_back(slot), 1);
        assert_eq!(ids(&ws), vec![vec![7], vec![2]]);
        // And into an empty workspace, whatever the old index was.
        let mut empty = Workspace::new();
        let slot = take(&mut ws, 2);
        assert_eq!(empty.put_back(slot), 0);
        assert_eq!(ids(&empty), vec![vec![2]]);
    }

    #[test]
    fn test_a_whole_desktop_comes_back_in_order_whichever_window_is_restored_first() {
        // Win+D: the windows are minimized one after the other.
        let orders = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        for restore_order in orders {
            let mut ws = ws_with(&[&[1], &[2], &[3]]);
            let mut slots: Vec<Option<WindowSlot>> = Vec::new();
            for id in [1, 2, 3] {
                let mut slot = ws.slot_of(WindowId::new(id)).unwrap();
                for earlier in slots.iter().flatten() {
                    slot.learn_from(earlier);
                }
                ws.remove_window(WindowId::new(id));
                slots.push(Some(slot));
            }
            assert!(ws.columns.is_empty());
            for i in restore_order {
                ws.put_back(slots[i].take().unwrap());
            }
            assert_eq!(
                ids(&ws),
                vec![vec![1], vec![2], vec![3]],
                "restore order {:?}",
                restore_order
            );
        }
    }

    #[test]
    fn test_a_whole_column_comes_back_in_order_whichever_window_is_restored_first() {
        let orders = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        for restore_order in orders {
            let mut ws = ws_with(&[&[1, 2, 3]]);
            ws.columns[0].width = Some(500);
            let mut slots: Vec<Option<WindowSlot>> = Vec::new();
            for id in [1, 2, 3] {
                let mut slot = ws.slot_of(WindowId::new(id)).unwrap();
                for earlier in slots.iter().flatten() {
                    slot.learn_from(earlier);
                }
                ws.remove_window(WindowId::new(id));
                slots.push(Some(slot));
            }
            for i in restore_order {
                ws.put_back(slots[i].take().unwrap());
            }
            assert_eq!(
                ids(&ws),
                vec![vec![1, 2, 3]],
                "restore order {:?}",
                restore_order
            );
            assert_eq!(ws.columns[0].width, Some(500), "the column width came back");
        }
    }

    // --- niri column operations ---

    #[test]
    fn test_consume_or_expel_left_moves_a_lone_window_to_the_bottom_of_the_left_column() {
        let mut ws = ws_with(&[&[1], &[2], &[3]]);
        assert_eq!(ws.consume_or_expel_left(WindowId::new(2)), Some(0));
        assert_eq!(ids(&ws), vec![vec![1, 2], vec![3]]);
        assert_eq!(ws.consume_or_expel_left(WindowId::new(3)), Some(0));
        assert_eq!(ids(&ws), vec![vec![1, 2, 3]]);
    }

    #[test]
    fn test_consume_or_expel_left_expels_a_window_to_the_left_of_its_column() {
        let mut ws = ws_with(&[&[1], &[2, 3, 4], &[5]]);
        ws.columns[1].width = Some(640);
        assert_eq!(ws.consume_or_expel_left(WindowId::new(3)), Some(1));
        // At the source column's place: the expelled window ends up on its left.
        assert_eq!(ids(&ws), vec![vec![1], vec![3], vec![2, 4], vec![5]]);
        assert_eq!(
            ws.columns[1].width,
            Some(640),
            "same width as the column it left"
        );
        assert_eq!(ws.columns[2].width, Some(640));
    }

    #[test]
    fn test_consume_or_expel_left_does_nothing_for_a_lone_window_in_the_first_column() {
        let mut ws = ws_with(&[&[1], &[2]]);
        assert_eq!(ws.consume_or_expel_left(WindowId::new(1)), None);
        assert_eq!(ids(&ws), vec![vec![1], vec![2]]);
        assert_eq!(ws.consume_or_expel_left(WindowId::new(9)), None);
    }

    #[test]
    fn test_consume_or_expel_right_moves_a_lone_window_to_the_bottom_of_the_right_column() {
        let mut ws = ws_with(&[&[1], &[2], &[3]]);
        assert_eq!(ws.consume_or_expel_right(WindowId::new(1)), Some(0));
        assert_eq!(ids(&ws), vec![vec![2, 1], vec![3]]);
        assert_eq!(
            ws.consume_or_expel_right(WindowId::new(1)),
            Some(1),
            "now it shares a column: it is expelled to the right"
        );
        assert_eq!(ids(&ws), vec![vec![2], vec![1], vec![3]]);
    }

    #[test]
    fn test_consume_or_expel_right_expels_a_window_to_the_right_of_its_column() {
        let mut ws = ws_with(&[&[1], &[2, 3, 4], &[5]]);
        assert_eq!(ws.consume_or_expel_right(WindowId::new(3)), Some(2));
        assert_eq!(ids(&ws), vec![vec![1], vec![2, 4], vec![3], vec![5]]);
    }

    #[test]
    fn test_consume_or_expel_right_does_nothing_for_a_lone_window_in_the_last_column() {
        let mut ws = ws_with(&[&[1], &[2]]);
        assert_eq!(ws.consume_or_expel_right(WindowId::new(2)), None);
        assert_eq!(ids(&ws), vec![vec![1], vec![2]]);
    }

    #[test]
    fn test_consume_into_column_pulls_the_first_window_of_the_right_column() {
        let mut ws = ws_with(&[&[1], &[2, 3], &[4]]);
        assert_eq!(ws.consume_into_column(0), Some(WindowId::new(2)));
        assert_eq!(ids(&ws), vec![vec![1, 2], vec![3], vec![4]]);
        assert_eq!(ws.consume_into_column(1), Some(WindowId::new(4)));
        assert_eq!(
            ids(&ws),
            vec![vec![1, 2], vec![3, 4]],
            "the emptied column vanishes"
        );
        assert_eq!(
            ws.consume_into_column(1),
            None,
            "nothing on the right of the last column"
        );
    }

    #[test]
    fn test_expel_from_column_takes_the_bottom_window() {
        let mut ws = ws_with(&[&[1, 2, 3], &[4]]);
        assert_eq!(ws.expel_from_column(0), Some(WindowId::new(3)));
        assert_eq!(ids(&ws), vec![vec![1, 2], vec![3], vec![4]]);
        assert_eq!(ws.expel_from_column(0), Some(WindowId::new(2)));
        assert_eq!(ids(&ws), vec![vec![1], vec![2], vec![3], vec![4]]);
        assert_eq!(
            ws.expel_from_column(0),
            None,
            "a single window has nothing to expel"
        );
        assert_eq!(ws.expel_from_column(9), None);
    }

    #[test]
    fn test_a_window_expelled_from_a_maximized_column_is_maximized_too() {
        let mut ws = ws_with(&[&[1, 2]]);
        ws.columns[0].width = Some(500);
        ws.columns[0].toggle_maximized(900);
        assert_eq!(ws.expel_from_column(0), Some(WindowId::new(2)));
        for col in &ws.columns {
            assert!(col.maximized);
            assert_eq!(col.width, Some(900));
            assert_eq!(col.pre_max_width, Some(Some(500)));
        }
    }

    #[test]
    fn test_move_tile_within_column_swaps_with_the_neighbour() {
        let mut ws = ws_with(&[&[1, 2, 3]]);
        assert!(ws.move_tile_within_column(WindowId::new(2), -1));
        assert_eq!(ids(&ws), vec![vec![2, 1, 3]]);
        assert!(
            !ws.move_tile_within_column(WindowId::new(2), -1),
            "already at the top"
        );
        assert!(ws.move_tile_within_column(WindowId::new(2), 1));
        assert!(ws.move_tile_within_column(WindowId::new(2), 1));
        assert_eq!(ids(&ws), vec![vec![1, 3, 2]]);
        assert!(
            !ws.move_tile_within_column(WindowId::new(2), 1),
            "already at the bottom"
        );
        assert!(!ws.move_tile_within_column(WindowId::new(9), 1));
    }

    #[test]
    fn test_a_tabbed_column_keeps_showing_the_same_window_when_tiles_swap() {
        let mut ws = ws_with(&[&[1, 2, 3]]);
        ws.columns[0].display = ColumnDisplay::Tabbed { active_tab: 1 };
        assert!(ws.move_tile_within_column(WindowId::new(2), 1));
        assert_eq!(
            ws.columns[0].display,
            ColumnDisplay::Tabbed { active_tab: 2 }
        );
        assert_eq!(ids(&ws), vec![vec![1, 3, 2]]);
    }

    #[test]
    fn test_move_column_to_an_edge() {
        let mut ws = ws_with(&[&[1], &[2, 3], &[4]]);
        assert_eq!(ws.move_column_to(2, 0), Some(0));
        assert_eq!(ids(&ws), vec![vec![4], vec![1], vec![2, 3]]);
        assert_eq!(ws.move_column_to(0, 2), Some(2));
        assert_eq!(ids(&ws), vec![vec![1], vec![2, 3], vec![4]]);
        assert_eq!(ws.move_column_to(1, 1), None, "already there");
        assert_eq!(ws.move_column_to(5, 0), None);
        assert_eq!(ws.move_column_to(0, 5), None);
    }

    // --- maximize-column ---

    #[test]
    fn test_toggle_maximized_widens_and_restores_the_column() {
        let mut col = Column::new();
        col.add_tile(Tile::new(WindowId::new(1)));
        col.toggle_maximized(900);
        assert!(col.maximized);
        assert_eq!(
            col.width,
            Some(900),
            "even a column of one window gets as wide as the view"
        );
        col.toggle_maximized(900);
        assert!(!col.maximized);
        assert_eq!(col.width, None, "back to the default width");

        col.width = Some(500);
        col.toggle_maximized(900);
        assert_eq!(col.width, Some(900));
        col.toggle_maximized(900);
        assert_eq!(col.width, Some(500), "back to the width it had");
    }

    #[test]
    fn test_an_explicit_width_ends_the_maximized_state() {
        let mut col = Column::new();
        col.width = Some(500);
        col.toggle_maximized(900);
        col.set_width(Some(640));
        assert!(!col.maximized);
        assert_eq!(col.width, Some(640));
        assert_eq!(col.pre_max_width, None);
        // ...so the next toggle maximizes again instead of restoring something stale.
        col.toggle_maximized(900);
        assert_eq!(col.width, Some(900));
        col.toggle_maximized(900);
        assert_eq!(col.width, Some(640));
    }

    #[test]
    fn test_a_column_with_a_minimum_is_wider_and_pushes_the_next_one_over() {
        let mut ws = Workspace::new();
        for id in 1..=3 {
            ws.add_window_to_new_column(WindowId::new(id));
        }
        // Three auto columns of 440 px, 16 px apart.
        assert_eq!(ws.column_x_and_width(0, 440, 16), (0, 440));
        assert_eq!(ws.column_x_and_width(1, 440, 16), (456, 440));
        assert_eq!(ws.column_x_and_width(2, 440, 16), (912, 440));
        // The window of the middle one cannot be narrower than 462 px.
        ws.columns[1].tiles[0].min_width = 462;
        assert_eq!(ws.columns[1].min_width(), 462);
        assert_eq!(ws.column_x_and_width(0, 440, 16), (0, 440));
        assert_eq!(ws.column_x_and_width(1, 440, 16), (456, 462));
        assert_eq!(ws.column_x_and_width(2, 440, 16), (934, 440));
        assert_eq!(ws.total_width(440, 16), 440 + 16 + 462 + 16 + 440);
        // Hit-testing and window rectangles use the same widths.
        assert_eq!(
            ws.column_at_x(900, 440, 16),
            Some(1),
            "the wider column is under 900"
        );
        assert_eq!(
            ws.column_at_x(925, 440, 16),
            None,
            "and 925 is in the gap after it"
        );
        assert_eq!(ws.column_at_x(940, 440, 16), Some(2));
        let rect = ws.current_position_for(WindowId::new(3), 440, 16).unwrap();
        assert_eq!(rect.loc.x, 934);
        // A column of its own width above the minimum is not touched; below it is raised.
        ws.columns[1].width = Some(900);
        assert_eq!(ws.column_x_and_width(1, 440, 16).1, 900);
        ws.columns[1].width = Some(300);
        assert_eq!(ws.column_x_and_width(1, 440, 16).1, 462);
        // The widest minimum of a column's windows counts.
        ws.columns[1].tiles.push(Tile::new(WindowId::new(9)));
        ws.columns[1].tiles[1].min_width = 600;
        assert_eq!(ws.columns[1].min_width(), 600);
    }

    #[test]
    fn test_a_column_is_never_wider_than_the_smallest_maximum_of_its_windows() {
        let mut col = Column::new();
        col.add_tile(Tile::new(WindowId::new(1)));
        col.add_tile(Tile::new(WindowId::new(2)));
        assert_eq!(col.max_width(), 0, "no limit");
        assert_eq!(col.resolved_width(500), 500);
        // One window with a limit: the whole column keeps to it, own width or not.
        col.tiles[0].max_width = 420;
        assert_eq!(col.max_width(), 420);
        assert_eq!(col.resolved_width(500), 420);
        col.width = Some(900);
        assert_eq!(col.resolved_width(500), 420);
        // The smallest nonzero limit counts; a tile without a limit says nothing.
        col.tiles[1].max_width = 380;
        assert_eq!(col.max_width(), 380);
        // A narrower column than the limit is left alone.
        col.width = Some(300);
        assert_eq!(col.resolved_width(500), 300);
        // The minimum wins when a window cannot be as narrow as another allows.
        col.tiles[0].min_width = 462;
        assert_eq!(col.resolved_width(500), 462);
        col.width = Some(900);
        assert_eq!(col.resolved_width(500), 462);
    }

    #[test]
    fn test_learn_from_ignores_a_window_that_is_already_known() {
        let ws = ws_with(&[&[1], &[2]]);
        let first = ws.slot_of(WindowId::new(1)).unwrap();
        let mut second = ws.slot_of(WindowId::new(2)).unwrap();
        second.learn_from(&first); // 1 is already one of its left neighbours
        assert_eq!(second.left_of, vec![WindowId::new(1)]);
    }
}
