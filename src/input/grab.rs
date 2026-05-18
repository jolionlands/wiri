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
}

impl MoveGrab {
    pub fn new(window_id: WindowId, cursor: Point, window_pos: Point, initial_size: Size, column: usize) -> Self {
        Self {
            window_id,
            initial_cursor: cursor,
            initial_window_pos: window_pos,
            initial_size,
            initial_column: column,
        }
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

/// Determines the resize edge from a hit-test position relative to the window.
///
/// Returns `None` if the point lies outside the window rect entirely, preventing
/// spurious edge matches (e.g. a point left of the window matching Top/Bottom).
pub fn resize_edge_from_point(point: Point, window_rect: Rect, border_size: i32) -> Option<ResizeEdge> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
