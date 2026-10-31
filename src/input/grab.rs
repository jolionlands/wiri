use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::utils::{Point, Rect, Size, WindowId};

/// Edge of a window for resize grabs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// State for an interactive window move grab (mouse drag)
#[derive(Debug, Clone)]
pub struct MoveGrab {
    /// The window being moved
    pub window_id: WindowId,
    /// Cursor position when the grab started
    pub initial_cursor: Point,
    /// Window position when the grab started
    pub initial_window_pos: Point,
    /// Window size when the grab started (used to avoid hardcoding on each move)
    pub initial_size: Size,
    /// Column index when the grab started
    pub initial_column: usize,
    /// Modifiers that owned this gesture. Releasing one cancels insertion.
    required_modifiers: u32,
    /// Shared by queued motion snapshots so cancellation cannot resurrect a hint.
    cancelled: Arc<AtomicBool>,
    /// Lifecycle invalidation also forbids queued cancellation from restoring a reused HWND.
    source_invalidated: Arc<AtomicBool>,
    /// Once the niri pull-out threshold is crossed, returning to the origin is still a drag.
    drag_started: Arc<AtomicBool>,
}

impl MoveGrab {
    pub fn new(
        window_id: WindowId,
        cursor: Point,
        window_pos: Point,
        initial_size: Size,
        column: usize,
    ) -> Self {
        Self {
            window_id,
            initial_cursor: cursor,
            initial_window_pos: window_pos,
            initial_size,
            initial_column: column,
            required_modifiers: 0,
            cancelled: Arc::new(AtomicBool::new(false)),
            source_invalidated: Arc::new(AtomicBool::new(false)),
            drag_started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_modifiers(mut self, modifiers: u32) -> Self {
        self.required_modifiers = modifiers;
        self
    }

    pub fn modifiers_released(&self, modifiers: u32) -> bool {
        self.required_modifiers != 0
            && modifiers & self.required_modifiers != self.required_modifiers
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn invalidate_source(&self) {
        self.source_invalidated.store(true, Ordering::Release);
        self.cancel();
    }

    pub(crate) fn can_restore_source(&self) -> bool {
        !self.source_invalidated.load(Ordering::Acquire)
    }

    /// Call while holding the engine lock, including after waiting for that lock.
    pub(crate) fn can_process(&self, tracked: bool, minimized: bool) -> bool {
        !self.is_cancelled() && tracked && !minimized
    }

    pub(crate) fn same_gesture(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }

    /// Read the latched threshold without advancing a pending tiled move.
    pub(crate) fn has_drag_started(&self) -> bool {
        self.drag_started.load(Ordering::Acquire)
    }

    /// niri layout/mod.rs: squared logical displacement must reach 256 * 256
    /// before a tiled window leaves its slot. A click/sub-threshold move is not a drop.
    pub(crate) fn tiled_drag_started(&self, cursor: Point, scale: f64) -> bool {
        if !self.drag_started.load(Ordering::Acquire) {
            let scale = if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            };
            let dx = (cursor.x as f64 - self.initial_cursor.x as f64) / scale;
            let dy = (cursor.y as f64 - self.initial_cursor.y as f64) / scale;
            if dx * dx + dy * dy >= 256.0 * 256.0 {
                self.drag_started.store(true, Ordering::Release);
            }
        }
        self.drag_started.load(Ordering::Acquire)
    }

    /// Calculate the delta from the initial cursor position
    pub fn delta(&self, current_cursor: Point) -> Point {
        Point::new(
            current_cursor.x - self.initial_cursor.x,
            current_cursor.y - self.initial_cursor.y,
        )
    }

    /// Calculate the new window position based on cursor movement
    pub fn new_position(&self, current_cursor: Point) -> Point {
        let delta = self.delta(current_cursor);
        Point::new(
            self.initial_window_pos.x + delta.x,
            self.initial_window_pos.y + delta.y,
        )
    }

    /// Determine which column the window should be moved to based on cursor position.
    /// Returns the target column index.
    pub fn target_column(
        &self,
        current_cursor: Point,
        column_width: i32,
        column_gap: i32,
        total_columns: usize,
    ) -> usize {
        let delta_x = current_cursor.x - self.initial_cursor.x;
        let column_delta = delta_x / (column_width + column_gap);
        let target = self.initial_column as i32 + column_delta;
        target.clamp(0, (total_columns as i32).saturating_sub(1)) as usize
    }
}

/// State for an interactive window resize grab
#[derive(Debug, Clone)]
pub struct ResizeGrab {
    /// The window being resized
    pub window_id: WindowId,
    /// Which edge(s) are being dragged
    pub edge: ResizeEdge,
    /// Cursor position when the grab started
    pub initial_cursor: Point,
    /// Window rect when the grab started
    pub initial_rect: Rect,
}

impl ResizeGrab {
    pub fn new(window_id: WindowId, edge: ResizeEdge, cursor: Point, rect: Rect) -> Self {
        Self {
            window_id,
            edge,
            initial_cursor: cursor,
            initial_rect: rect,
        }
    }

    /// Calculate the new rect based on cursor movement and resize edge.
    /// Uses the original edges to avoid recomputation bugs when modifying loc.
    pub fn new_rect(&self, current_cursor: Point, min_size: (i32, i32)) -> Rect {
        let dx = current_cursor.x - self.initial_cursor.x;
        let dy = current_cursor.y - self.initial_cursor.y;

        // Save original edges before any modifications
        let orig_right = self.initial_rect.right();
        let orig_bottom = self.initial_rect.bottom();

        let mut rect = self.initial_rect;

        match self.edge {
            ResizeEdge::Left => {
                let new_x = rect.loc.x + dx;
                let max_x = orig_right - min_size.0;
                rect.loc.x = new_x.min(max_x);
                rect.size.w = (orig_right - rect.loc.x) as u32;
            }
            ResizeEdge::Right => {
                let new_right = orig_right + dx;
                let min_right = rect.loc.x + min_size.0;
                rect.size.w = (new_right.max(min_right) - rect.loc.x) as u32;
            }
            ResizeEdge::Top => {
                let new_y = rect.loc.y + dy;
                let max_y = orig_bottom - min_size.1;
                rect.loc.y = new_y.min(max_y);
                rect.size.h = (orig_bottom - rect.loc.y) as u32;
            }
            ResizeEdge::Bottom => {
                let new_bottom = orig_bottom + dy;
                let min_bottom = rect.loc.y + min_size.1;
                rect.size.h = (new_bottom.max(min_bottom) - rect.loc.y) as u32;
            }
            ResizeEdge::TopLeft => {
                let new_x = rect.loc.x + dx;
                let max_x = orig_right - min_size.0;
                rect.loc.x = new_x.min(max_x);
                rect.size.w = (orig_right - rect.loc.x) as u32;
                let new_y = rect.loc.y + dy;
                let max_y = orig_bottom - min_size.1;
                rect.loc.y = new_y.min(max_y);
                rect.size.h = (orig_bottom - rect.loc.y) as u32;
            }
            ResizeEdge::TopRight => {
                let new_right = orig_right + dx;
                let min_right = rect.loc.x + min_size.0;
                rect.size.w = (new_right.max(min_right) - rect.loc.x) as u32;
                let new_y = rect.loc.y + dy;
                let max_y = orig_bottom - min_size.1;
                rect.loc.y = new_y.min(max_y);
                rect.size.h = (orig_bottom - rect.loc.y) as u32;
            }
            ResizeEdge::BottomLeft => {
                let new_x = rect.loc.x + dx;
                let max_x = orig_right - min_size.0;
                rect.loc.x = new_x.min(max_x);
                rect.size.w = (orig_right - rect.loc.x) as u32;
                let new_bottom = orig_bottom + dy;
                let min_bottom = rect.loc.y + min_size.1;
                rect.size.h = (new_bottom.max(min_bottom) - rect.loc.y) as u32;
            }
            ResizeEdge::BottomRight => {
                let new_right = orig_right + dx;
                let min_right = rect.loc.x + min_size.0;
                rect.size.w = (new_right.max(min_right) - rect.loc.x) as u32;
                let new_bottom = orig_bottom + dy;
                let min_bottom = rect.loc.y + min_size.1;
                rect.size.h = (new_bottom.max(min_bottom) - rect.loc.y) as u32;
            }
        }
        rect
    }
}

// ---------------------------------------------------------------------------
// Column reorder grab (Item 3 — drag-to-reorder columns)
// ---------------------------------------------------------------------------

/// Pixels from the top of a tile that constitute the "column header" hit-zone.
/// When a move-grab starts inside this strip AND `Ctrl` is held, we enter
/// column-reorder mode instead of floating mode.
pub const COLUMN_REORDER_HIT_ZONE_PX: i32 = 32;

/// Grab state for column drag-reorder.
///
/// The user holds Ctrl+Alt and drags a tiled window near its top edge.
/// Instead of floating the window, the grab tracks the source column index.
/// On mouse-up, `TilingEngine::swap_columns` is called with the column that
/// the cursor is currently over as the destination.
#[derive(Debug, Clone)]
pub struct ColumnReorderGrab {
    /// Window being dragged (used for visual feedback / reference only).
    pub window_id: WindowId,
    /// Column index of the dragged window when the grab started.
    pub source_col: usize,
    /// Cursor position when the grab started (used for a drag-threshold).
    pub initial_cursor: Point,
}

impl ColumnReorderGrab {
    pub fn new(window_id: WindowId, source_col: usize, cursor: Point) -> Self {
        Self {
            window_id,
            source_col,
            initial_cursor: cursor,
        }
    }
}

/// Determines the resize edge from a hit-test position relative to the window.
///
/// Returns `None` if the point lies outside the window rect entirely, preventing
/// spurious edge matches (e.g. a point left of the window matching Top/Bottom).
pub fn resize_edge_from_point(
    point: Point,
    window_rect: Rect,
    border_size: i32,
) -> Option<ResizeEdge> {
    // Guard: reject points that are outside the window bounds.
    if point.x < window_rect.loc.x
        || point.x > window_rect.right()
        || point.y < window_rect.loc.y
        || point.y > window_rect.bottom()
    {
        return None;
    }

    let border = border_size.max(4); // At least 4px for grab area
    let near_left = point.x >= window_rect.loc.x && point.x <= window_rect.loc.x + border;
    let near_right = point.x >= window_rect.right() - border && point.x <= window_rect.right();
    let near_top = point.y >= window_rect.loc.y && point.y <= window_rect.loc.y + border;
    let near_bottom = point.y >= window_rect.bottom() - border && point.y <= window_rect.bottom();

    match (near_left, near_right, near_top, near_bottom) {
        // Corners take priority (check first)
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (_, true, true, _) => Some(ResizeEdge::TopRight),
        (true, _, _, true) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        // Edges
        (true, _, false, false) => Some(ResizeEdge::Left),
        (_, true, false, false) => Some(ResizeEdge::Right),
        (false, false, true, _) => Some(ResizeEdge::Top),
        (false, false, _, true) => Some(ResizeEdge::Bottom),
        _ => None,
    }
}

/// Borders preserve their precise edge; client interior selects its nearest corner.
pub fn resize_edge_or_quadrant(point: Point, rect: Rect, border: i32) -> Option<ResizeEdge> {
    if let Some(edge) = resize_edge_from_point(point, rect, border) {
        return Some(edge);
    }
    if point.x < rect.loc.x
        || point.x > rect.right()
        || point.y < rect.loc.y
        || point.y > rect.bottom()
    {
        return None;
    }
    Some(
        match (
            point.x < rect.loc.x + rect.size.w as i32 / 2,
            point.y < rect.loc.y + rect.size.h as i32 / 2,
        ) {
            (true, true) => ResizeEdge::TopLeft,
            (false, true) => ResizeEdge::TopRight,
            (true, false) => ResizeEdge::BottomLeft,
            (false, false) => ResizeEdge::BottomRight,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_cancellation_is_shared_by_queued_snapshots_and_tracks_own_modifiers() {
        let grab = MoveGrab::new(
            WindowId::new(1),
            Point::default(),
            Point::default(),
            Size::new(400, 300),
            0,
        )
        .with_modifiers(1);
        let queued = grab.clone();
        assert!(!grab.modifiers_released(1 | 2));
        assert!(grab.modifiers_released(2));
        assert!(!queued.is_cancelled());
        grab.cancel();
        assert!(queued.is_cancelled());
        let multi = MoveGrab::new(
            WindowId::new(1),
            Point::default(),
            Point::default(),
            Size::new(400, 300),
            0,
        )
        .with_modifiers(1 | 2);
        assert!(multi.modifiers_released(1));
        assert!(!multi.modifiers_released(1 | 2));
    }

    #[test]
    fn queued_moves_require_a_live_source_and_lifecycle_invalidation_forbids_restore() {
        let grab = MoveGrab::new(
            WindowId::new(1),
            Point::default(),
            Point::default(),
            Size::new(400, 300),
            0,
        );
        let queued = grab.clone();
        assert!(queued.can_process(true, false));
        assert!(!queued.can_process(false, false));
        assert!(!queued.can_process(true, true));
        grab.cancel();
        assert!(!queued.can_process(true, false));
        assert!(
            queued.can_restore_source(),
            "Escape may restore the old slot"
        );
        grab.invalidate_source();
        assert!(
            !queued.can_restore_source(),
            "destroy/minimize must not restore it"
        );
        let reused = MoveGrab::new(
            grab.window_id,
            Point::default(),
            Point::default(),
            Size::new(400, 300),
            0,
        );
        assert!(grab.same_gesture(&queued));
        assert!(!grab.same_gesture(&reused));
        assert!(
            !queued.can_process(true, false),
            "retracking the HWND cannot revive it"
        );
        assert!(reused.can_process(true, false));
    }

    #[test]
    fn tiled_move_threshold_is_logical_latched_and_shared_with_queued_release() {
        let origin = Point::new(-400, 100);
        let grab = MoveGrab::new(WindowId::new(1), origin, origin, Size::new(400, 300), 0);
        let queued = grab.clone();
        assert!(!grab.tiled_drag_started(origin, 1.5));
        assert!(!grab.tiled_drag_started(Point::new(-17, 100), 1.5));
        assert!(grab.tiled_drag_started(Point::new(-16, 100), 1.5));
        assert!(
            queued.tiled_drag_started(origin, 1.5),
            "returning to origin remains a drag"
        );
        for scale in [0.0, f64::NAN, f64::INFINITY] {
            let grab = MoveGrab::new(WindowId::new(1), origin, origin, Size::new(400, 300), 0);
            assert!(!grab.tiled_drag_started(Point::new(-145, 100), scale));
            assert!(grab.tiled_drag_started(Point::new(-144, 100), scale));
        }
        let diagonal = MoveGrab::new(WindowId::new(1), origin, origin, Size::new(400, 300), 0);
        assert!(diagonal.tiled_drag_started(Point::new(-218, 282), 1.0));
    }

    #[test]
    fn client_quadrants_choose_resize_corners_without_losing_edges() {
        let rect = Rect::new(100, 100, 400, 300);
        assert_eq!(
            resize_edge_or_quadrant(Point::new(180, 180), rect, 4),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge_or_quadrant(Point::new(420, 320), rect, 4),
            Some(ResizeEdge::BottomRight)
        );
        assert_eq!(
            resize_edge_or_quadrant(Point::new(498, 220), rect, 4),
            Some(ResizeEdge::Right)
        );
        assert_eq!(resize_edge_or_quadrant(Point::new(99, 180), rect, 4), None);
    }

    #[test]
    fn test_move_grab_delta() {
        let grab = MoveGrab::new(
            WindowId::new(1),
            Point::new(100, 100),
            Point::new(200, 200),
            Size::new(600, 400),
            0,
        );
        let delta = grab.delta(Point::new(150, 130));
        assert_eq!(delta.x, 50);
        assert_eq!(delta.y, 30);
    }

    #[test]
    fn test_move_grab_new_position() {
        let grab = MoveGrab::new(
            WindowId::new(1),
            Point::new(100, 100),
            Point::new(200, 200),
            Size::new(600, 400),
            0,
        );
        let new_pos = grab.new_position(Point::new(120, 110));
        assert_eq!(new_pos.x, 220);
        assert_eq!(new_pos.y, 210);
    }

    #[test]
    fn test_move_grab_target_column() {
        let grab = MoveGrab::new(
            WindowId::new(1),
            Point::new(100, 100),
            Point::new(200, 200),
            Size::new(600, 400),
            2,
        );
        // Move right by 508px = exactly one column width (500 + 8 gap)
        let target = grab.target_column(Point::new(608, 100), 500, 8, 5);
        assert_eq!(target, 3);
    }

    #[test]
    fn test_move_grab_target_column_clamp() {
        let grab = MoveGrab::new(
            WindowId::new(1),
            Point::new(100, 100),
            Point::new(200, 200),
            Size::new(600, 400),
            0,
        );
        let target = grab.target_column(Point::new(50, 100), 500, 8, 5);
        assert_eq!(target, 0); // Clamped to 0
    }

    #[test]
    fn test_resize_grab_right() {
        let grab = ResizeGrab::new(
            WindowId::new(1),
            ResizeEdge::Right,
            Point::new(800, 300),
            Rect::new(100, 100, 700, 500),
        );
        let new_rect = grab.new_rect(Point::new(900, 300), (100, 100));
        assert_eq!(new_rect.size.w, 800); // 700 + 100 delta
        assert_eq!(new_rect.loc.x, 100); // Left edge unchanged
    }

    #[test]
    fn test_resize_grab_left() {
        let grab = ResizeGrab::new(
            WindowId::new(1),
            ResizeEdge::Left,
            Point::new(100, 300),
            Rect::new(100, 100, 700, 500),
        );
        let new_rect = grab.new_rect(Point::new(150, 300), (100, 100));
        assert_eq!(new_rect.loc.x, 150); // Left edge moved right
        assert_eq!(new_rect.size.w, 650); // Width decreased (800 - 150)
    }

    #[test]
    fn test_resize_grab_min_size() {
        let grab = ResizeGrab::new(
            WindowId::new(1),
            ResizeEdge::Right,
            Point::new(800, 300),
            Rect::new(100, 100, 700, 500),
        );
        // Drag left past minimum size
        let new_rect = grab.new_rect(Point::new(50, 300), (100, 100));
        assert!(new_rect.size.w >= 100); // Respects minimum width
    }

    #[test]
    fn test_resize_edge_from_point() {
        let rect = Rect::new(100, 100, 500, 400);
        assert_eq!(
            resize_edge_from_point(Point::new(102, 300), rect, 6),
            Some(ResizeEdge::Left)
        );
        assert_eq!(
            resize_edge_from_point(Point::new(595, 300), rect, 6),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            resize_edge_from_point(Point::new(300, 102), rect, 6),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            resize_edge_from_point(Point::new(300, 495), rect, 6),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(
            resize_edge_from_point(Point::new(300, 300), rect, 6),
            None // Center - no resize
        );
    }

    #[test]
    fn test_resize_edge_corners() {
        let rect = Rect::new(100, 100, 500, 400);
        // Top-left corner: should detect left or top (whichever checks first)
        let edge = resize_edge_from_point(Point::new(102, 102), rect, 6);
        assert!(edge.is_some());
        // Bottom-right corner
        let edge = resize_edge_from_point(Point::new(595, 495), rect, 6);
        assert!(edge.is_some());
    }

    #[test]
    fn test_resize_edge_outside_rect() {
        let rect = Rect::new(100, 100, 500, 400);
        // Point outside the rect entirely
        let edge = resize_edge_from_point(Point::new(50, 50), rect, 6);
        assert_eq!(edge, None);
    }

    #[test]
    fn test_resize_grab_top() {
        let grab = ResizeGrab::new(
            WindowId::new(1),
            ResizeEdge::Top,
            Point::new(300, 100),
            Rect::new(100, 100, 500, 400),
        );
        let new_rect = grab.new_rect(Point::new(300, 150), (100, 100));
        assert_eq!(new_rect.loc.y, 150); // Top edge moved down
        assert!(new_rect.size.h < 400); // Height decreased
    }

    #[test]
    fn test_resize_grab_bottom() {
        let grab = ResizeGrab::new(
            WindowId::new(1),
            ResizeEdge::Bottom,
            Point::new(300, 500),
            Rect::new(100, 100, 500, 400),
        );
        let new_rect = grab.new_rect(Point::new(300, 550), (100, 100));
        assert_eq!(new_rect.loc.y, 100); // Top unchanged
        assert_eq!(new_rect.size.h, 450); // Height increased
    }

    #[test]
    fn test_move_grab_column_with_scroll() {
        let grab = MoveGrab::new(
            WindowId::new(1),
            Point::new(100, 100),
            Point::new(200, 200),
            Size::new(600, 400),
            0,
        );
        // Move left — should clamp to 0
        let target = grab.target_column(Point::new(50, 100), 500, 8, 5);
        assert_eq!(target, 0);
    }
}
