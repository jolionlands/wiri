//! Windows keep the final say over their size (niri `src/layout/scrolling.rs`).
//!
//! niri treats the size a client committed as the truth and the column width it asked for as
//! a request.  `Column::width` is *the widest size its windows committed*
//! (`scrolling.rs:4860-4875`), the columns to the right move with that width
//! (`ScrollingSpace::update_window`, `scrolling.rs:1287-1305`), and the request
//! (`ColumnWidth`, `scrolling.rs:4961-4972`) is only what the next configure asks for - and
//! it is asked again for the same value, so a client that keeps its own size is not fought.
//!
//! wiri applies the same rule for a window that resizes itself once it has been laid out:
//! `note_window_self_resized` records the size the window chose, the column is laid out as
//! wide as that (so the target rectangle equals what the window already is, and wiri never
//! asks again), and the neighbours reflow around it.
use super::*;

impl TilingEngine {
    /// The window `id` resized itself: an `EVENT_OBJECT_LOCATIONCHANGE` whose size is not the
    /// rectangle wiri's own SetWindowPos batch left there.  Returns true when the layout
    /// changed (and has been re-applied).
    pub fn note_window_self_resized(&mut self, id: WindowId, backend: &BackendHandle) -> bool {
        let Some(observed) = Self::live_window_outer_rect(id) else {
            return false;
        };
        if !self.adopt_self_resize(id, observed) {
            return false;
        }
        self.apply_all(backend);
        true
    }

    /// [`Self::note_window_self_resized`] with the window's live rectangle supplied: the
    /// decision and the layout change, so a pure test can drive it against fake windows.
    ///
    /// `observed` is the window's outer rectangle (what `GetWindowRect` answers, the same
    /// space as [`AppliedState::rect`]), because that is the rectangle wiri hands to
    /// SetWindowPos and therefore the only one it can compare its own writes against.
    pub(crate) fn adopt_self_resize(&mut self, id: WindowId, observed: Rect) -> bool {
        if self.applying_batch || self.frame_in_progress {
            // wiri's own batch or animation frame put the window there.
            return false;
        }
        if self.native_moves.contains_key(&id) || self.animating_rects.contains_key(&id) {
            // A native caption/edge loop owns the HWND until MOVESIZEEND, and an animation
            // timeline is on its way to the layout's target.
            return false;
        }
        if self.floating_windows.contains(&id) || self.fullscreen_windows.contains(&id) {
            return false;
        }
        if self
            .window_sizing
            .get(&id)
            .is_some_and(|mode| *mode != SizingMode::Normal)
        {
            // Fullscreen and maximized tiles are sized by the sizing mode, not by the window.
            return false;
        }
        if self.is_overview() {
            return false;
        }
        if self.transitions.preparing {
            // A transition is re-placing windows for its own snapshots.
            return false;
        }
        // Only a size that differs from the rectangle wiri applied last is the window's own
        // doing.  An unknown target (never applied, or invalidated by a reload) proves
        // nothing, so it is not adopted either.
        let Some(prior) = self
            .applied_state
            .get(&id)
            .map(|state| state.rect)
            .filter(|rect| rect.size.w > 0 && rect.size.h > 0)
        else {
            return false;
        };
        if Self::same_window_rect(prior, observed) {
            return false;
        }
        let Some((output, workspace_id, col_idx)) = self.locate_window(id) else {
            return false;
        };
        let cfg = self
            .effective_config_for_workspace(output, workspace_id)
            .clone();
        // The outer rectangle wiri applies is the tile shrunk by the border inset
        // (`resolved_window_geometry`), so the window's own size in tile space is the
        // observed size plus that inset on both sides.
        let inset = self.rule_border_inset(id, &cfg);
        let tile_w = observed.size.w as i32 + 2 * inset;
        let tile_h = observed.size.h as i32 + 2 * inset;
        if !(Self::MIN_COLUMN_PX..=Self::MAX_COLUMN_PX).contains(&tile_w) {
            return false;
        }
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return false;
        };
        let work = Self::tiling_work_rect(monitor, &cfg);
        let available_height = (work.size.h as i32 - cfg.window_gap).max(1);
        let Some(column) = monitor
            .workspaces
            .get_mut(&workspace_id)
            .and_then(|workspace| workspace.columns.get_mut(col_idx))
        else {
            return false;
        };
        if column.maximized {
            // A maximized column is as wide as the view on purpose.
            return false;
        }
        let stacked = column.visible_tile_indices().len() > 1;
        let mut changed = false;
        if let Some(tile) = column.tiles.iter_mut().find(|tile| tile.window_id == id) {
            if tile.observed_width != Some(tile_w as u32) {
                tile.observed_width = Some(tile_w as u32);
                changed = true;
            }
            // A column of several stacked windows gives the resized one its height and
            // shares what is left among the others (niri `Column::update_window`, which
            // moves the windows below in tandem, `scrolling.rs:4480-4527`).  A column of one
            // window fills its height, and a tabbed column shows one window at a time, so
            // neither takes a height from the window.
            if stacked {
                let height = tile_h.clamp(1, available_height) as u32;
                if tile.preferred_height != Some(height) {
                    tile.preferred_height = Some(height);
                    // A preset or proportion of its own would outrank the window's height on
                    // the next pass; the window's own size wins, as niri lets the committed
                    // size win over the requested one.
                    tile.height_proportion = None;
                    tile.height_preset_idx = None;
                    changed = true;
                }
            }
        }
        if !changed {
            return false;
        }
        // The camera keeps the resized column visible, as niri does when the active column
        // resizes (`scrolling.rs:1381-1401`, `animate_view_offset_to_column`).
        let focused_column = self.monitors.focused_id() == Some(output)
            && self
                .monitors
                .get(&output)
                .is_some_and(|monitor| monitor.focus_column == Some(col_idx));
        if focused_column {
            self.reveal_focused_column(output);
        }
        true
    }

    /// The window's live outer rectangle: the same space as the rectangle wiri applies with
    /// SetWindowPos, unlike the DWM visible frame that `EVENT_OBJECT_LOCATIONCHANGE`
    /// reports.  `None` when the handle is gone or the window is minimised (its rectangle
    /// then says nothing about the size it wants).
    pub(crate) fn live_window_outer_rect(id: WindowId) -> Option<Rect> {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsWindow};
        let hwnd = HWND(id.as_isize() as *mut std::ffi::c_void);
        // SAFETY: IsWindow and IsIconic only read the window's state, and GetWindowRect
        // writes the rectangle into a local. A handle that went stale only makes IsWindow
        // false, which returns first; nothing is dereferenced.
        unsafe {
            if !IsWindow(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return None;
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return None;
            }
            Some(Rect::new(
                rect.left,
                rect.top,
                (rect.right - rect.left).max(0) as u32,
                (rect.bottom - rect.top).max(0) as u32,
            ))
        }
    }

    /// Whether two outer rectangles describe the same geometry.  One pixel of slack covers
    /// a frame the system adjusted by a pixel; a real self-resize is never that small.
    fn same_window_rect(a: Rect, b: Rect) -> bool {
        (a.loc.x - b.loc.x).abs() <= 1
            && (a.loc.y - b.loc.y).abs() <= 1
            && a.size.w.abs_diff(b.size.w) <= 1
            && a.size.h.abs_diff(b.size.h) <= 1
    }
}
