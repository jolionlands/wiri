use std::collections::HashMap;
use crate::utils::{Rect, Size, WindowId, Point};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, SetWindowPos, HWND_TOP,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE,
};

/// PiP corner snap destinations (Item 3 — niri-parity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatingCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

#[derive(Debug, Clone)]
pub struct FloatWindow {
    pub window_id: WindowId,
    pub rect: Rect,
    pub saved_rect: Rect,
}

impl FloatWindow {
    pub fn new(window_id: WindowId, rect: Rect) -> Self {
        let saved_rect = rect;
        Self {
            window_id,
            rect,
            saved_rect,
        }
    }

    pub fn restore(&mut self) {
        self.rect = self.saved_rect;
    }

    pub fn resize(&mut self, new_size: Size) {
        self.rect.size = new_size;
    }

    pub fn move_to(&mut self, point: Point) {
        self.rect.loc = point;
    }
}

#[derive(Clone)]
pub struct FloatManager {
    windows: HashMap<WindowId, FloatWindow>,
}

impl FloatManager {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
        }
    }

    pub fn insert(&mut self, window_id: WindowId, rect: Rect) -> &mut FloatWindow {
        let float = FloatWindow::new(window_id, rect);
        self.windows.insert(window_id, float);
        self.windows.get_mut(&window_id).unwrap()
    }

    pub fn remove(&mut self, window_id: WindowId) -> Option<FloatWindow> {
        self.windows.remove(&window_id)
    }

    pub fn get(&self, window_id: WindowId) -> Option<&FloatWindow> {
        self.windows.get(&window_id)
    }

    pub fn get_mut(&mut self, window_id: WindowId) -> Option<&mut FloatWindow> {
        self.windows.get_mut(&window_id)
    }

    pub fn contains(&self, window_id: WindowId) -> bool {
        self.windows.contains_key(&window_id)
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn window_ids(&self) -> Vec<WindowId> {
        self.windows.keys().cloned().collect()
    }

    pub fn bring_to_front(&mut self, window_id: WindowId) -> bool {
        if !self.windows.contains_key(&window_id) {
            return false;
        }
        let hwnd_isize = window_id.as_isize();
        let hwnd = HWND(hwnd_isize as *mut std::ffi::c_void);
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return false;
            }
            SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)
                .is_ok()
        }
    }

    pub fn save_current_layout(&mut self, window_id: WindowId) {
        if let Some(float) = self.windows.get_mut(&window_id) {
            float.saved_rect = float.rect;
        }
    }

    pub fn restore_all(&mut self) {
        for float in self.windows.values_mut() {
            float.restore();
        }
    }

    pub fn find_window_at(&self, point: Point) -> Option<WindowId> {
        for (window_id, float) in self.windows.iter() {
            if float.rect.contains_point(point) {
                return Some(*window_id);
            }
        }
        None
    }

    /// Compute a snapped `Rect` for `window_id` at the given `corner` of `work_rect`.
    /// The window's current size is preserved; the returned rect is clamped inside
    /// `work_rect` with a fixed 24-pixel margin from each edge.
    /// Returns `None` when the window is not tracked.
    pub fn snap_to_corner(
        &mut self,
        window_id: WindowId,
        corner: FloatingCorner,
        work_rect: Rect,
        win_size: Size,
    ) -> Rect {
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
        // Update the stored rect if this window is tracked.
        if let Some(fw) = self.windows.get_mut(&window_id) {
            fw.rect = snapped;
        }
        snapped
    }

    pub fn adjust_to_output(&mut self, output_rect: Rect) {
        for float in self.windows.values_mut() {
            if !output_rect.intersects(float.rect) {
                float.rect.loc.x = output_rect.loc.x;
                float.rect.loc.y = output_rect.loc.y;
            }

            let max_x = output_rect.right() - float.rect.size.w as i32;
            let max_y = output_rect.bottom() - float.rect.size.h as i32;
            float.rect.loc.x = float.rect.loc.x.min(max_x).max(output_rect.loc.x);
            float.rect.loc.y = float.rect.loc.y.min(max_y).max(output_rect.loc.y);
        }
    }
}

impl Default for FloatManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FloatManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FloatManager")
            .field("window_count", &self.windows.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_float_manager_insert_remove() {
        let mut fm = FloatManager::new();
        let rect = Rect::new(100, 100, 200, 150);
        fm.insert(WindowId::new(1), rect);
        assert_eq!(fm.len(), 1);
        assert!(fm.contains(WindowId::new(1)));

        let removed = fm.remove(WindowId::new(1));
        assert!(removed.is_some());
        assert!(fm.is_empty());
    }

    #[test]
    fn test_find_window_at() {
        let mut fm = FloatManager::new();
        fm.insert(WindowId::new(1), Rect::new(0, 0, 100, 100));
        fm.insert(WindowId::new(2), Rect::new(200, 200, 100, 100));

        assert_eq!(fm.find_window_at(Point::new(50, 50)), Some(WindowId::new(1)));
        assert_eq!(fm.find_window_at(Point::new(250, 250)), Some(WindowId::new(2)));
        assert_eq!(fm.find_window_at(Point::new(150, 150)), None);
    }

    // --- Item 3: PiP corner snap ---

    #[test]
    fn test_floating_snap_top_left() {
        let mut fm = FloatManager::new();
        let win_size = crate::utils::Size::new(320, 180);
        // work_rect: full 1920×1080 at origin
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let wid = WindowId::new(42);
        fm.insert(wid, Rect::new(500, 500, win_size.w, win_size.h));

        let snapped = fm.snap_to_corner(wid, FloatingCorner::TopLeft, work_rect, win_size);

        // Expected: margin=24 from top-left
        assert_eq!(snapped.loc.x, 24, "x should be 24 (left margin)");
        assert_eq!(snapped.loc.y, 24, "y should be 24 (top margin)");
        assert_eq!(snapped.size.w, win_size.w, "width preserved");
        assert_eq!(snapped.size.h, win_size.h, "height preserved");
        // Stored rect is also updated
        assert_eq!(fm.get(wid).unwrap().rect.loc.x, 24);
    }

    #[test]
    fn test_floating_snap_bottom_right() {
        let mut fm = FloatManager::new();
        let win_size = crate::utils::Size::new(400, 300);
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let wid = WindowId::new(7);
        fm.insert(wid, Rect::new(0, 0, win_size.w, win_size.h));

        let snapped = fm.snap_to_corner(wid, FloatingCorner::BottomRight, work_rect, win_size);

        // Expected: 1920 - 400 - 24 = 1496, 1080 - 300 - 24 = 756
        assert_eq!(snapped.loc.x, 1496);
        assert_eq!(snapped.loc.y, 756);
    }

    #[test]
    fn test_floating_snap_center() {
        let mut fm = FloatManager::new();
        let win_size = crate::utils::Size::new(200, 100);
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let wid = WindowId::new(9);
        fm.insert(wid, Rect::new(0, 0, win_size.w, win_size.h));

        let snapped = fm.snap_to_corner(wid, FloatingCorner::Center, work_rect, win_size);

        // (1920 - 200) / 2 = 860, (1080 - 100) / 2 = 490
        assert_eq!(snapped.loc.x, 860);
        assert_eq!(snapped.loc.y, 490);
    }

    #[test]
    fn test_floating_snap_untracked_window_returns_rect() {
        // snap_to_corner on an untracked window_id just returns the rect without panic.
        let mut fm = FloatManager::new();
        let win_size = crate::utils::Size::new(100, 100);
        let work_rect = Rect::new(0, 0, 1920, 1080);
        let wid = WindowId::new(999);
        // Not inserted — snap_to_corner must not panic.
        let snapped = fm.snap_to_corner(wid, FloatingCorner::TopRight, work_rect, win_size);
        // x = 1920 - 100 - 24 = 1796
        assert_eq!(snapped.loc.x, 1796);
    }
}
