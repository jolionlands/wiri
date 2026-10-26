from pathlib import Path

def edit(path, old, new, all=False):
    p=Path(path); raw=p.read_bytes(); crlf=b'\r\n' in raw
    s=raw.decode('utf-8').replace('\r\n','\n')
    assert (s.count(old)>0 if all else s.count(old)==1), (path, old[:100], s.count(old))
    s=s.replace(old,new) if all else s.replace(old,new,1)
    p.write_bytes(s.replace('\n','\r\n').encode('utf-8') if crlf else s.encode('utf-8'))

p='src/layout/workspace.rs'
edit(p,'    pub tiles: Vec<Tile>,\n    /// Explicit column width', '''    pub tiles: Vec<Tile>,
    /// Remembered independently of monitor MRU history (which is bounded). Window ids
    /// survive tile insertion, reorder and column swaps without index repair.
    pub active_window: Option<WindowId>,
    /// Requested proportion of the view; `width` is its resolved pixel cache.
    pub width_proportion: Option<f64>,
    /// Explicit column width''')
edit(p,'            tiles: Vec::new(),\n            width: None,','            tiles: Vec::new(),\n            active_window: None,\n            width_proportion: None,\n            width: None,')
edit(p,'        self.preset_idx = None;\n        self.width = width;','        self.preset_idx = None;\n        self.width_proportion = None;\n        self.width = width;')
edit(p,'    /// niri `toggle_full_width` (`maximize-column`)', '''    pub fn set_proportion(&mut self, proportion: f64, view: i32, gap: i32) {
        self.set_width(Some(crate::input::proportion_px(view, gap, proportion).max(1) as u32));
        self.width_proportion = Some(proportion);
    }

    pub fn active_tile(&self) -> Option<WindowId> {
        self.active_window
            .filter(|id| self.index_of(*id).is_some())
            .or_else(|| self.tiles.first().map(|tile| tile.window_id))
    }

    /// niri `toggle_full_width` (`maximize-column`)''')
edit(p,'''    pub fn remove_tile(&mut self, window_id: WindowId) -> Option<Tile> {
        self.tiles
            .iter()
            .position(|t| t.window_id == window_id)
            .map(|i| self.tiles.remove(i))
    }''','''    pub fn remove_tile(&mut self, window_id: WindowId) -> Option<Tile> {
        let index = self.index_of(window_id)?;
        let tile = self.tiles.remove(index);
        if self.active_window == Some(window_id) {
            self.active_window = self.tiles.get(index.min(self.tiles.len().saturating_sub(1)))
                .map(|tile| tile.window_id);
        }
        self.clamp_active_tab();
        Some(tile)
    }''')
edit(p,'    pub column_width: Option<u32>,\n    pub column_display:', '    pub column_width: Option<u32>,\n    pub column_width_proportion: Option<f64>,\n    pub column_display:')
edit(p,'    pub columns: Vec<Column>,\n    pub scroll_offset:', '    pub columns: Vec<Column>,\n    /// The active column is anchored by its remembered active window.\n    pub active_window: Option<WindowId>,\n    pub scroll_offset:')
edit(p,'            columns: Vec::new(),\n            scroll_offset:', '            columns: Vec::new(),\n            active_window: None,\n            scroll_offset:',all=True)
edit(p,'    /// Add a window to an existing column\n', '''    /// niri's default insertion: right of this workspace's active column, not
    /// the end of the strip. The empty workspace starts at column zero.
    pub fn insert_window_after_active(&mut self, window_id: WindowId, width: Option<u32>) -> usize {
        let at = self.active_window.and_then(|id| self.find_window_column(id))
            .map_or(self.columns.len(), |col| col + 1);
        let mut column = Column::new();
        column.width = width;
        column.add_tile(Tile::new(window_id));
        self.columns.insert(at, column);
        self.remember_focus(window_id);
        at
    }

    pub fn remember_focus(&mut self, window_id: WindowId) {
        if let Some(index) = self.find_window_column(window_id) {
            self.active_window = Some(window_id);
            let column = &mut self.columns[index];
            column.active_window = Some(window_id);
            if let ColumnDisplay::Tabbed { active_tab } = &mut column.display {
                *active_tab = column.tiles.iter().position(|t| t.window_id == window_id).unwrap_or(0);
            }
        }
    }

    pub fn remembered_focus(&self) -> Option<(usize, WindowId)> {
        self.active_window.and_then(|id| self.find_window_column(id).map(|col| (col, id)))
            .or_else(|| self.columns.iter().enumerate().find_map(|(col, c)| c.active_tile().map(|id| (col, id))))
    }

    /// Add a window to an existing column
''')
edit(p,'            column_width: col.width,\n            column_display:', '            column_width: col.width,\n            column_width_proportion: col.width_proportion,\n            column_display:')
edit(p,'        col.width = column_width;\n        col.maximized', '        col.width = column_width;\n        col.width_proportion = slot.column_width_proportion;\n        col.maximized')
edit(p,'        new_col.width = source.width;\n        new_col.maximized', '        new_col.width = source.width;\n        new_col.width_proportion = source.width_proportion;\n        new_col.preset_idx = source.preset_idx;\n        new_col.maximized')
# Retain the column width when a move makes a new column.
edit(p,'        // Find and remove the window\n        let removed = self.remove_window(window_id);', '        // A newly created column inherits the source width, not a fresh default.\n        let width = self.find_window_column(window_id).map(|i| (self.columns[i].width, self.columns[i].width_proportion));\n        let removed = self.remove_window(window_id);')
edit(p,'            // Append a new column for this tile\n            let mut col = Column::new();\n            col.add_tile(tile);','            // Append a new column for this tile\n            let mut col = Column::new();\n            if let Some((pixels, proportion)) = width {\n                col.width = pixels;\n                col.width_proportion = proportion;\n            }\n            col.add_tile(tile);')
# Active window ids, not active indices, need no repair for these operations.
p='src/layout/mod.rs'
edit(p,'workspace.add_window_to_new_column_with_width(window_id, width_hint);','workspace.insert_window_after_active(window_id, width_hint);')
edit(p,'''        if let Some(workspace) = self.workspace() {
            if let Some(col_idx) = workspace.find_window_column(window_id) {
                self.focus_column = Some(col_idx);
            }
        }
    }

    pub fn remove_window''','''        if let Some(workspace) = self.workspace_mut() {
            workspace.remember_focus(window_id);
            if let Some(col_idx) = workspace.find_window_column(window_id) {
                self.focus_column = Some(col_idx);
            }
        }
    }

    pub fn remove_window''')
edit(p,'''            self.focus_ring
                .iter_mru()
                .find_map(|wid| workspace.find_window_column(wid).map(|col| (col, wid)))''','''            workspace.remembered_focus().or_else(|| self.focus_ring
                .iter_mru()
                .find_map(|wid| workspace.find_window_column(wid).map(|col| (col, wid))))''')
# One setter updates both memories for all directional focus paths.
edit(p,'''                if let Some(wid) = new_focus {
                    self.focus_window = Some(wid);
                    self.focus_ring.push(wid);
                }''','''                if let Some(wid) = new_focus {
                    self.set_focus(wid);
                }''')
edit(p,'''                self.focus_column = Some(new_col);
                self.focus_window = Some(window_id);
                self.focus_ring.push(window_id);''','''                self.focus_column = Some(new_col);
                self.set_focus(window_id);''')
edit(p,'''            self.focus_column = Some(col);
            self.focus_window = Some(wid);
            self.focus_ring.push(wid);''','''            self.focus_column = Some(col);
            self.set_focus(wid);''')
edit(p,'''            self.focus_window = Some(wid);
            self.focus_ring.push(wid);''','''            self.set_focus(wid);''',all=True)
# Only these focus paths use the first tile; other fallbacks may keep first.
edit(p,'.and_then(|c| c.tiles.first())\n                    .map(|t| t.window_id);','.and_then(|c| c.active_tile());')
edit(p,'.and_then(|col| col.tiles.first().map(|tile| (new_col, tile.window_id)))','.and_then(|col| col.active_tile().map(|id| (new_col, id)))')
edit(p,'.and_then(|c| c.tiles.first())\n            .map(|t| t.window_id);','.and_then(|c| c.active_tile());')
p='src/layout/engine/rules.rs'
edit(p,'.add_window_to_new_column_with_width(window_id, width_hint);','.insert_window_after_active(window_id, width_hint);')
