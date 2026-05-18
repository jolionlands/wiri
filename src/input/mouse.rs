use std::collections::HashMap;
use std::time::Instant;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::utils::{OutputId, Point, WindowId, Rect};

/// Configuration for mouse-based focus behavior
#[derive(Debug, Clone)]
pub struct MouseFocusConfig {
    /// Whether focus follows the mouse cursor
    pub focus_follows_mouse: bool,
    /// Whether to warp the cursor to the center of a newly focused window
    pub warp_on_focus: bool,
    /// Delay in ms before focus changes (0 = instant)
    pub focus_delay_ms: u32,
    /// Whether clicking a window raises it
    pub raise_on_click: bool,
}

impl Default for MouseFocusConfig {
    fn default() -> Self {
        Self {
            focus_follows_mouse: false,
            warp_on_focus: false,
            focus_delay_ms: 0,
            raise_on_click: true,
        }
    }
}

/// Tracks the mouse position and determines which window/monitor the cursor is over.
/// Used for focus-follows-mouse and interactive move/resize.
pub struct MouseTracker {
    /// Current cursor position (screen coordinates)
    cursor_pos: Point,
    /// Last known cursor position (for delta calculation)
    last_cursor_pos: Point,
    /// Configuration
    config: MouseFocusConfig,
    /// Time of last focus change (for delay-based focus)
    last_focus_change: Option<Instant>,
    /// Pending focus window (waiting for delay)
    pending_focus: Option<WindowId>,
}

impl MouseTracker {
    pub fn new(config: MouseFocusConfig) -> Self {
        Self {
            cursor_pos: Point::default(),
            last_cursor_pos: Point::default(),
            config,
            last_focus_change: None,
            pending_focus: None,
        }
    }

    /// Update the cursor position from the system.
    /// Returns the current cursor position.
    pub fn update_cursor_pos(&mut self) -> Point {
        self.last_cursor_pos = self.cursor_pos;
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            let _ = GetCursorPos(&mut pt);
            self.cursor_pos = Point::new(pt.x, pt.y);
        }
        self.cursor_pos
    }

    /// Get the current cursor position (last known)
    pub fn cursor_pos(&self) -> Point {
        self.cursor_pos
    }

    /// Check if cursor has moved since last update
    pub fn cursor_moved(&self) -> bool {
        self.cursor_pos != self.last_cursor_pos
    }

    /// Get the cursor movement delta since last update
    pub fn cursor_delta(&self) -> Point {
        Point::new(
            self.cursor_pos.x - self.last_cursor_pos.x,
            self.cursor_pos.y - self.last_cursor_pos.y,
        )
    }

    /// Set the cursor position (used for warp-on-focus)
    pub fn set_cursor_pos(&self, pos: Point) {
        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
        unsafe {
            let _ = SetCursorPos(pos.x, pos.y);
        }
    }

    /// Warp the cursor to the center of a rect (for warp-on-focus)
    pub fn warp_to_center(&self, rect: Rect) {
        let center = rect.center();
        self.set_cursor_pos(center);
    }

    /// Determine which monitor the cursor is currently on
    pub fn monitor_at_cursor(&self, monitors: &HashMap<OutputId, (Rect, Rect)>) -> Option<OutputId> {
        for (output_id, (bounds, _work_area)) in monitors {
            if bounds.contains_point(self.cursor_pos) {
                return Some(*output_id);
            }
        }
        None
    }

    /// Check if focus-follows-mouse is enabled
    pub fn focus_follows_mouse(&self) -> bool {
        self.config.focus_follows_mouse
    }

    /// Update the config
    pub fn set_config(&mut self, config: MouseFocusConfig) {
        self.config = config;
    }

    /// Get a reference to the config
    pub fn config(&self) -> &MouseFocusConfig {
        &self.config
    }

    /// Check if the focus delay has elapsed for a pending focus change.
    /// Returns true if the focus change should proceed.
    pub fn check_focus_delay(&mut self) -> bool {
        if self.config.focus_delay_ms == 0 {
            return true;
        }
        if let Some(last) = self.last_focus_change {
            let elapsed = last.elapsed().as_millis() as u32;
            elapsed >= self.config.focus_delay_ms
        } else {
            true
        }
    }

    /// Record that a focus change just happened
    pub fn record_focus_change(&mut self) {
        self.last_focus_change = Some(Instant::now());
    }

    /// Set a pending focus target (used with delay)
    pub fn set_pending_focus(&mut self, window_id: Option<WindowId>) {
        self.pending_focus = window_id;
    }

    /// Get the pending focus target
    pub fn pending_focus(&self) -> Option<WindowId> {
        self.pending_focus
    }
}

impl Default for MouseTracker {
    fn default() -> Self {
        Self::new(MouseFocusConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mouse_tracker_default() {
        let tracker = MouseTracker::default();
        assert!(!tracker.focus_follows_mouse());
    }

    #[test]
    fn test_mouse_tracker_config() {
        let config = MouseFocusConfig {
            focus_follows_mouse: true,
            warp_on_focus: true,
            focus_delay_ms: 200,
            raise_on_click: true,
        };
        let tracker = MouseTracker::new(config);
        assert!(tracker.focus_follows_mouse());
        assert!(tracker.config().warp_on_focus);
    }

    #[test]
    fn test_monitor_at_cursor() {
        let mut monitors = HashMap::new();
        monitors.insert(
            OutputId::from_name("DISPLAY1"),
            (Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1040)),
        );
        monitors.insert(
            OutputId::from_name("DISPLAY2"),
            (Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1040)),
        );

        let mut tracker = MouseTracker::default();
        // Simulate cursor at center of DISPLAY1
        tracker.cursor_pos = Point::new(960, 540);
        assert_eq!(
            tracker.monitor_at_cursor(&monitors),
            Some(OutputId::from_name("DISPLAY1"))
        );

        // Cursor at center of DISPLAY2
        tracker.cursor_pos = Point::new(2880, 540);
        assert_eq!(
            tracker.monitor_at_cursor(&monitors),
            Some(OutputId::from_name("DISPLAY2"))
        );

        // Cursor off-screen
        tracker.cursor_pos = Point::new(-100, -100);
        assert_eq!(tracker.monitor_at_cursor(&monitors), None);
    }

    #[test]
    fn test_cursor_delta() {
        let mut tracker = MouseTracker::default();
        tracker.cursor_pos = Point::new(100, 200);
        tracker.last_cursor_pos = Point::new(80, 190);
        let delta = tracker.cursor_delta();
        assert_eq!(delta.x, 20);
        assert_eq!(delta.y, 10);
    }

    #[test]
    fn test_focus_delay_zero() {
        let mut tracker = MouseTracker::default();
        assert!(tracker.check_focus_delay()); // No delay => always ready
    }
}
