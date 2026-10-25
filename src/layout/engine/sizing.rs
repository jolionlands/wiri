//! Fullscreen, maximized to the edges, and keeping the place in the strip while a window is
//! either.
//!
//! niri's model: a fullscreen or maximized window stays a participant of the strip.  It keeps
//! its column (and its place in it), the other columns stay where they are, and focusing one of
//! them scrolls the strip, so that the fullscreen window moves away with its slot instead of
//! staying on top of everything.  wiri does the same on Windows:
//!
//! * The window's column is made as wide as the view while it is fullscreen or maximized (the
//!   state `maximize-column` toggles), so the neighbours are out of the way and come into view
//!   when they are focused.  The column gets its old width back afterwards.
//! * The window is *drawn* over the whole monitor (fullscreen: no frame, placed exactly, the
//!   taskbar marked away) or the whole work area (maximize-window-to-edges: no gaps, no border),
//!   shifted by whatever the strip is scrolled by, so it parks off screen like any other tile
//!   that is out of view.
//! * The other windows are neither hidden nor moved: they stay in the taskbar and Alt+Tab.
//!
//! An application that goes fullscreen on its own (a video player, F11 in a browser) or whose
//! maximize button is pressed is followed, not fought: the window is adopted into the same
//! states, and when the application leaves them wiri tiles the window again.

use super::*;
use std::time::{Duration, Instant};

/// How long after a window went fullscreen a report that it no longer covers its monitor is
/// taken for something the application did to the window (leaving fullscreen), not for the
/// fuss it makes while its frame is taken away.
const FULLSCREEN_SETTLE: Duration = Duration::from_millis(1500);

/// How the layout draws the tile of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DrawMode {
    /// The rectangle of the tile less the border: every ordinary tile.
    Inset,
    /// The whole work area of the monitor, no gap, no border (`maximize-window-to-edges`).
    Edges,
    /// The whole monitor, placed exactly: the frame of a fullscreen window is gone.
    Exact,
}

/// The rectangle a tile is drawn in.  `slot` is its place in the strip (laid out inside
/// `work_rect`, scrolled), `work_area` the work area and `monitor` the bounds of the monitor.
/// A fullscreen or maximized window is drawn over the monitor or the work area, shifted by
/// how far its slot is from the start of the view, so that it moves with the strip.
pub(super) fn state_rect(
    mode: DrawMode,
    slot: Rect,
    work_rect: Rect,
    work_area: Rect,
    monitor: Rect,
) -> Rect {
    let shift = slot.loc.x - work_rect.loc.x;
    match mode {
        DrawMode::Inset => slot,
        DrawMode::Edges => Rect::new(
            work_area.loc.x + shift,
            work_area.loc.y,
            work_area.size.w,
            work_area.size.h,
        ),
        DrawMode::Exact => Rect::new(
            monitor.loc.x + shift,
            monitor.loc.y,
            monitor.size.w,
            monitor.size.h,
        ),
    }
}

/// What the engine knows about a window when a size class report for it comes in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SizingState {
    pub fullscreen: bool,
    pub maximized_edges: bool,
    /// The window was asked to give up its native maximized state; the value is whether it is
    /// to be maximized to the edges once it has.
    pub unzoom_pending: Option<bool>,
    /// The rectangle wiri last put the window in is as big as its monitor (a maximized column
    /// without gaps on a monitor without a taskbar), so a covering window is wiri's own doing.
    pub own_rect_covers: bool,
    /// The window has been fullscreen for a while.
    pub fullscreen_settled: bool,
}

/// What to do about a window that reports its size class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SizeReaction {
    Nothing,
    /// The application maximized itself (its title bar button): take the native state away
    /// and, when the window reports it is gone, set maximized-to-edges to `wants_maximized`.
    RestoreNatively { wants_maximized: bool },
    /// The native state is gone: set maximized-to-edges to this.
    SetMaximized(bool),
    /// The window covers its monitor on its own: it is fullscreen.
    EnterFullscreen,
    /// The window stopped covering its monitor on its own: it left fullscreen.
    LeaveFullscreen,
}

/// The decision, apart from the Win32 calls it leads to.
pub(super) fn size_reaction(zoomed: bool, covers_monitor: bool, state: SizingState) -> SizeReaction {
    if zoomed {
        // niri: an application that maximizes itself is maximized to the edges (and asked
        // again, a window that is maximized to the edges gets back to its tile).
        return SizeReaction::RestoreNatively {
            wants_maximized: !state.maximized_edges,
        };
    }
    if let Some(wanted) = state.unzoom_pending {
        return SizeReaction::SetMaximized(wanted);
    }
    if covers_monitor && !state.fullscreen && !state.own_rect_covers {
        return SizeReaction::EnterFullscreen;
    }
    if !covers_monitor && state.fullscreen && state.fullscreen_settled {
        return SizeReaction::LeaveFullscreen;
    }
    SizeReaction::Nothing
}

impl TilingEngine {
    /// The window is maximized to the edges of the work area (niri `maximize-window-to-edges`).
    pub fn is_maximized_to_edges(&self, window_id: WindowId) -> bool {
        self.maximized_edges.contains(&window_id)
    }

    /// How each window of `windows` is drawn, when it is not the ordinary way.
    pub(super) fn sizing_draw_modes(
        &self,
        windows: &HashSet<WindowId>,
    ) -> HashMap<WindowId, DrawMode> {
        let mut modes = HashMap::new();
        for window_id in windows {
            if self.floating_windows.contains(window_id) {
                continue;
            }
            if self.fullscreen_windows.contains(window_id) {
                modes.insert(*window_id, DrawMode::Exact);
            } else if self.maximized_edges.contains(window_id) {
                modes.insert(*window_id, DrawMode::Edges);
            }
        }
        modes
    }

    /// The positions of a layout pass with the fullscreen and maximized tiles drawn where
    /// they belong (see [`state_rect`]).
    pub(super) fn apply_sizing_states(
        positions: Vec<(WindowId, Rect)>,
        modes: &HashMap<WindowId, DrawMode>,
        work_rect: Rect,
        work_area: Rect,
        monitor: Rect,
    ) -> Vec<(WindowId, Rect)> {
        if modes.is_empty() {
            return positions;
        }
        positions
            .into_iter()
            .map(|(window_id, slot)| {
                let mode = modes.get(&window_id).copied().unwrap_or(DrawMode::Inset);
                (window_id, state_rect(mode, slot, work_rect, work_area, monitor))
            })
            .collect()
    }

    /// Put `hwnd` on top of the windows on its monitor, without activating it and without
    /// waiting for the application.
    fn raise_to_top(&self, hwnd: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, HWND_TOP, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        };
        unsafe {
            let _ = SetWindowPos(
                HWND(hwnd as *mut std::ffi::c_void),
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    /// The width of the view of `output`, which a column is made while its window is
    /// fullscreen or maximized (the width `maximize-column` gives).
    fn slot_view_width(&self, output: OutputId) -> Option<u32> {
        let monitor = self.monitors.get(&output)?;
        Some(Self::view_width_for_monitor(monitor, self.effective_config(output)).max(50) as u32)
    }

    /// The window takes the whole view while it is fullscreen or maximized: its column is made
    /// as wide as the view, unless it is already (`maximize-column`).  What was done is
    /// remembered, so that [`Self::release_slot`] gives the old width back.
    pub(super) fn hold_slot(&mut self, window_id: WindowId) {
        let Some((output, workspace, column)) = self.locate_window(window_id) else {
            return;
        };
        let Some(view) = self.slot_view_width(output) else {
            return;
        };
        let Some(column) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspaces.get_mut(&workspace))
            .and_then(|w| w.columns.get_mut(column))
        else {
            return;
        };
        if !column.maximized {
            column.toggle_maximized(view);
            self.slot_forced.insert(window_id);
        }
    }

    /// The window is not fullscreen nor maximized any more: its column gets its old width
    /// back, when it was this window that made it as wide as the view and nothing else wants
    /// that now.
    fn release_slot(&mut self, window_id: WindowId) {
        if self.fullscreen_windows.contains(&window_id) || self.maximized_edges.contains(&window_id)
        {
            return;
        }
        if !self.slot_forced.remove(&window_id) {
            return;
        }
        let Some((output, workspace, column)) = self.locate_window(window_id) else {
            return;
        };
        let Some(view) = self.slot_view_width(output) else {
            return;
        };
        if let Some(column) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspaces.get_mut(&workspace))
            .and_then(|w| w.columns.get_mut(column))
        {
            if column.maximized {
                column.toggle_maximized(view);
            }
        }
    }

    /// niri `fullscreen-window` for one window: toggle.
    pub fn toggle_fullscreen_for(&mut self, window_id: WindowId, backend: &BackendHandle) {
        let on = !self.fullscreen_windows.contains(&window_id);
        self.set_fullscreen(window_id, on, false, backend);
    }

    /// Make a window fullscreen (`on`) or take it out of fullscreen.  `by_app` says that the
    /// window did it on its own (a video player, F11): its frame is not touched then, and wiri
    /// follows it out of fullscreen when the application leaves.
    ///
    /// A tile keeps its column (and its place in it) and is drawn over the monitor by the layout
    /// pass; a floating window covers the monitor and gets its rectangle back afterwards.  The
    /// shell is told, so that the taskbar goes away (and comes back).
    pub fn set_fullscreen(
        &mut self,
        window_id: WindowId,
        on: bool,
        by_app: bool,
        backend: &BackendHandle,
    ) {
        let Some(hwnd) = self.tiled_windows.get(&window_id).map(|w| w.hwnd) else {
            return;
        };
        if self.fullscreen_windows.contains(&window_id) == on {
            return;
        }
        let is_float = self.floating_windows.contains(&window_id);
        let output = if is_float {
            self.float_location(window_id).map(|(o, _)| o)
        } else {
            self.locate_window(window_id).map(|(o, _, _)| o)
        };
        let Some(output) = output.or_else(|| self.monitors.focused_id()) else {
            return;
        };
        let Some(monitor_bounds) = self.monitors.get(&output).map(|m| m.bounds) else {
            return;
        };

        if on {
            if is_float {
                if let Some(rect) = self.float_rect_of(window_id) {
                    self.float_memory.insert(window_id, rect);
                }
            }
            self.fullscreen_windows.insert(window_id);
            self.fullscreen_since.insert(window_id, Instant::now());
            self.window_sizing.insert(window_id, SizingMode::Fullscreen);
            if by_app {
                self.app_fullscreen.insert(window_id);
            }
            if !is_float {
                self.hold_slot(window_id);
            }
            if !by_app {
                // Strip the frame and cover the monitor.  A tile is placed again by the
                // layout pass, which draws it where its slot is.
                self.make_fullscreen(hwnd, monitor_bounds, backend);
            }
            crate::backend::taskbar::mark_fullscreen(hwnd, true);
            self.raise_to_top(hwnd);
            info!(
                "Window {} is fullscreen{}",
                hwnd,
                if by_app { " (its own doing)" } else { "" }
            );
        } else {
            self.fullscreen_windows.remove(&window_id);
            self.fullscreen_since.remove(&window_id);
            let by_app_before = self.app_fullscreen.remove(&window_id);
            self.window_sizing.insert(
                window_id,
                if self.maximized_edges.contains(&window_id) {
                    SizingMode::Maximized
                } else {
                    SizingMode::Normal
                },
            );
            if !by_app_before {
                self.restore_window_style(hwnd);
            }
            if is_float {
                if let Some(rect) = self.float_memory.get(&window_id).copied() {
                    self.place_float(window_id, rect, backend);
                }
            } else {
                self.release_slot(window_id);
            }
            crate::backend::taskbar::mark_fullscreen(hwnd, false);
            info!("Window {} left fullscreen", hwnd);
        }

        // The fullscreen rect is applied behind the applied-state cache's back.  Forget
        // what the cache knows about this window, or leaving fullscreen finds its old
        // tile rect "already applied" and the window stays as big as the screen.
        self.applied_state.remove(&window_id);
        self.reveal_focused_column(output);
        self.apply_layout_for_monitor(output, backend);
    }

    /// niri `maximize-window-to-edges`: the window fills the work area of its monitor, with no
    /// gap and no border, and keeps its place in its column.  Toggling it again gives the
    /// window its tile back.  A floating window cannot be maximized.
    pub fn set_maximized_to_edges(
        &mut self,
        window_id: WindowId,
        on: bool,
        backend: &BackendHandle,
    ) {
        if !self.tiled_windows.contains_key(&window_id)
            || self.floating_windows.contains(&window_id)
            || self.maximized_edges.contains(&window_id) == on
        {
            return;
        }
        let Some((output, _, _)) = self.locate_window(window_id) else {
            return;
        };
        if on {
            self.maximized_edges.insert(window_id);
            if !self.fullscreen_windows.contains(&window_id) {
                self.window_sizing.insert(window_id, SizingMode::Maximized);
            }
            self.hold_slot(window_id);
        } else {
            self.maximized_edges.remove(&window_id);
            self.window_sizing.insert(
                window_id,
                if self.fullscreen_windows.contains(&window_id) {
                    SizingMode::Fullscreen
                } else {
                    SizingMode::Normal
                },
            );
            self.release_slot(window_id);
        }
        info!(
            "Window {} is {} maximized to the edges",
            window_id.as_isize(),
            if on { "now" } else { "no longer" }
        );
        self.applied_state.remove(&window_id);
        self.reveal_focused_column(output);
        self.apply_layout_for_monitor(output, backend);
    }

    /// niri `maximize-window-to-edges` for one window: toggle.
    pub fn toggle_maximize_window_to_edges_for(
        &mut self,
        window_id: WindowId,
        backend: &BackendHandle,
    ) {
        let on = !self.maximized_edges.contains(&window_id);
        self.set_maximized_to_edges(window_id, on, backend);
    }

    /// A window that is about to leave the strip (it floats, or is minimized) is not
    /// fullscreen any more, and not maximized to the edges unless `keep_maximized` (a
    /// minimized window comes back maximized).  Its column gets its width back.  Nothing is
    /// laid out: the caller moves the window next.
    pub(super) fn leave_sizing_states(&mut self, window_id: WindowId, keep_maximized: bool) {
        if self.fullscreen_windows.contains(&window_id) {
            self.fullscreen_windows.remove(&window_id);
            self.fullscreen_since.remove(&window_id);
            let by_app = self.app_fullscreen.remove(&window_id);
            self.window_sizing.insert(window_id, SizingMode::Normal);
            if let Some(hwnd) = self.tiled_windows.get(&window_id).map(|w| w.hwnd) {
                if !by_app {
                    self.restore_window_style(hwnd);
                }
                crate::backend::taskbar::mark_fullscreen(hwnd, false);
            }
        }
        if !keep_maximized {
            self.maximized_edges.remove(&window_id);
        }
        self.release_slot(window_id);
    }

    /// The window is gone: forget what it was doing.
    pub(super) fn forget_sizing(&mut self, window_id: WindowId) {
        self.maximized_edges.remove(&window_id);
        self.app_fullscreen.remove(&window_id);
        self.slot_forced.remove(&window_id);
        self.unzoom_pending.remove(&window_id);
        self.fullscreen_since.remove(&window_id);
    }

    /// The window reports its size class: it became (or stopped being) natively maximized, or as
    /// big as its monitor, on its own.  The engine follows it (see the module description).
    pub fn window_size_class_changed(
        &mut self,
        window_id: WindowId,
        zoomed: bool,
        covers_monitor: bool,
        backend: &BackendHandle,
    ) {
        let Some(hwnd) = self.tiled_windows.get(&window_id).map(|w| w.hwnd) else {
            return;
        };
        // A float keeps whatever its application does with it; a minimized window reports
        // nothing that means anything.
        if self.floating_windows.contains(&window_id) || self.minimized.contains_key(&window_id) {
            return;
        }
        let Some((output, _, _)) = self.locate_window(window_id) else {
            return;
        };
        let monitor_bounds = self.monitors.get(&output).map(|m| m.bounds);
        let own_rect_covers = match (self.applied_state.get(&window_id), monitor_bounds) {
            (Some(applied), Some(bounds)) => {
                applied.rect.size.w > 0
                    && crate::backend::hooks::covers_monitor(applied.rect, bounds)
            }
            _ => false,
        };
        let state = SizingState {
            fullscreen: self.fullscreen_windows.contains(&window_id),
            maximized_edges: self.maximized_edges.contains(&window_id),
            unzoom_pending: self.unzoom_pending.get(&window_id).copied(),
            own_rect_covers,
            fullscreen_settled: self
                .fullscreen_since
                .get(&window_id)
                .map_or(true, |since| since.elapsed() >= FULLSCREEN_SETTLE),
        };
        match size_reaction(zoomed, covers_monitor, state) {
            SizeReaction::Nothing => {}
            SizeReaction::RestoreNatively { wants_maximized } => {
                debug!(
                    "window {} maximized itself: taking its native state away, maximized to the edges becomes {}",
                    hwnd, wants_maximized
                );
                self.unzoom_pending.insert(window_id, wants_maximized);
                backend.restore_window_async(hwnd);
            }
            SizeReaction::SetMaximized(wanted) => {
                self.unzoom_pending.remove(&window_id);
                self.set_maximized_to_edges(window_id, wanted, backend);
            }
            SizeReaction::EnterFullscreen => {
                self.set_fullscreen(window_id, true, true, backend);
            }
            SizeReaction::LeaveFullscreen => {
                self.set_fullscreen(window_id, false, false, backend);
            }
        }
    }

    /// A window that opens maximized or as big as its monitor (it was like that when wiri
    /// started, or its application restored that state) is adopted like one that did it later.
    pub(super) fn adopt_app_sizing_at_open(
        &mut self,
        window_id: WindowId,
        frame: Rect,
        backend: &BackendHandle,
    ) {
        let Some(hwnd) = self.tiled_windows.get(&window_id).map(|w| w.hwnd) else {
            return;
        };
        if self.floating_windows.contains(&window_id) {
            return;
        }
        let Some((output, _, _)) = self.locate_window(window_id) else {
            return;
        };
        let zoomed = unsafe {
            windows::Win32::UI::WindowsAndMessaging::IsZoomed(windows::Win32::Foundation::HWND(
                hwnd as *mut std::ffi::c_void,
            ))
            .as_bool()
        };
        if zoomed {
            self.unzoom_pending.insert(window_id, true);
            backend.restore_window_async(hwnd);
            return;
        }
        let covers = self
            .monitors
            .get(&output)
            .map_or(false, |m| crate::backend::hooks::covers_monitor(frame, m.bounds));
        if covers && !self.fullscreen_windows.contains(&window_id) {
            self.set_fullscreen(window_id, true, true, backend);
        }
    }

    /// niri `maximize-window-to-edges` for the window the engine's own state points at.
    pub fn toggle_maximize_window_to_edges(&mut self, backend: &BackendHandle) {
        if let Some(target) = self.state_target() {
            self.toggle_maximize_window_to_edges_for(target.window, backend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect::new(x, y, w, h)
    }

    // ---- pure decisions ----------------------------------------------------------------------

    #[test]
    fn a_fullscreen_window_is_drawn_over_the_monitor_and_moves_with_its_slot() {
        let work_rect = rect(8, 8, 1350, 712);
        let work_area = rect(0, 0, 1366, 728);
        let monitor = rect(0, 0, 1366, 768);
        // Aligned with the start of the view: the monitor.
        let slot = rect(8, 8, 1350, 712);
        assert_eq!(state_rect(DrawMode::Exact, slot, work_rect, work_area, monitor), monitor);
        // The strip scrolled by a view: the slot is a view to the left of the view's start, and
        // so is the window (by the same distance).
        let scrolled = rect(8 - 1366, 8, 1350, 712);
        assert_eq!(
            state_rect(DrawMode::Exact, scrolled, work_rect, work_area, monitor),
            rect(-1366, 0, 1366, 768)
        );
        // Whatever row of its column the tile is in, the window covers the monitor.
        let lower = rect(8, 300, 1350, 400);
        assert_eq!(state_rect(DrawMode::Exact, lower, work_rect, work_area, monitor), monitor);
    }

    #[test]
    fn a_maximized_window_is_drawn_over_the_work_area() {
        let work_rect = rect(8, 8, 1350, 712);
        let work_area = rect(0, 0, 1366, 728);
        let monitor = rect(0, 0, 1366, 768);
        assert_eq!(
            state_rect(DrawMode::Edges, rect(8, 8, 1350, 712), work_rect, work_area, monitor),
            work_area,
            "no gap and no border: the whole work area, the taskbar stays visible"
        );
        assert_eq!(
            state_rect(DrawMode::Edges, rect(1374, 8, 1350, 712), work_rect, work_area, monitor),
            rect(1366, 0, 1366, 728),
            "the neighbour to the right is a view away"
        );
        let slot = rect(40, 40, 100, 100);
        assert_eq!(state_rect(DrawMode::Inset, slot, work_rect, work_area, monitor), slot);
    }

    #[test]
    fn an_application_that_maximizes_itself_gets_maximized_to_the_edges() {
        let state = SizingState::default();
        assert_eq!(
            size_reaction(true, false, state),
            SizeReaction::RestoreNatively { wants_maximized: true }
        );
        // Its button pressed again while it is maximized to the edges: back to its tile.
        let state = SizingState {
            maximized_edges: true,
            ..SizingState::default()
        };
        assert_eq!(
            size_reaction(true, false, state),
            SizeReaction::RestoreNatively { wants_maximized: false }
        );
        // A window maximized on a monitor without a taskbar covers it too: maximized wins.
        assert_eq!(
            size_reaction(true, true, SizingState::default()),
            SizeReaction::RestoreNatively { wants_maximized: true }
        );
    }

    #[test]
    fn the_toggle_is_set_when_the_native_state_is_gone() {
        let state = SizingState {
            unzoom_pending: Some(true),
            ..SizingState::default()
        };
        assert_eq!(size_reaction(false, false, state), SizeReaction::SetMaximized(true));
        let state = SizingState {
            unzoom_pending: Some(false),
            maximized_edges: true,
            ..SizingState::default()
        };
        assert_eq!(size_reaction(false, false, state), SizeReaction::SetMaximized(false));
    }

    #[test]
    fn a_window_that_covers_its_monitor_on_its_own_is_fullscreen_and_leaves_it_when_it_shrinks() {
        assert_eq!(
            size_reaction(false, true, SizingState::default()),
            SizeReaction::EnterFullscreen
        );
        // Already fullscreen: nothing to do.
        let fullscreen = SizingState {
            fullscreen: true,
            fullscreen_settled: true,
            ..SizingState::default()
        };
        assert_eq!(size_reaction(false, true, fullscreen), SizeReaction::Nothing);
        // It stops covering the monitor: it left fullscreen...
        assert_eq!(size_reaction(false, false, fullscreen), SizeReaction::LeaveFullscreen);
        // ...unless it has only just been made fullscreen, when the frame going away makes noise.
        let fresh = SizingState {
            fullscreen_settled: false,
            ..fullscreen
        };
        assert_eq!(size_reaction(false, false, fresh), SizeReaction::Nothing);
    }

    #[test]
    fn what_wiri_placed_over_the_monitor_itself_is_not_an_application_going_fullscreen() {
        let own = SizingState {
            own_rect_covers: true,
            ..SizingState::default()
        };
        assert_eq!(size_reaction(false, true, own), SizeReaction::Nothing);
    }

    #[test]
    fn nothing_special_is_nothing() {
        assert_eq!(size_reaction(false, false, SizingState::default()), SizeReaction::Nothing);
    }

    // ---- the engine ----------------------------------------------------------------------------

    fn engine() -> TilingEngine {
        let mut engine = TilingEngine::new(LayoutConfig {
            outer_gaps: (8, 8, 8, 8),
            ..LayoutConfig::default()
        });
        engine.register_monitor(
            OutputId::from_name("M"),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        engine
    }

    fn window(hwnd: isize) -> WindowInfo {
        WindowInfo {
            hwnd,
            title: format!("Window {}", hwnd),
            class_name: "TestClass".to_string(),
            process_id: 1000,
            bounds: Rect::new(100, 100, 700, 500),
            state: crate::backend::WindowState::Normal,
            is_visible: true,
        }
    }

    fn be() -> BackendHandle {
        BackendHandle::default_for_test()
    }

    fn with_three_windows() -> TilingEngine {
        let mut engine = engine();
        for hwnd in [1, 2, 3] {
            engine.add_window(window(hwnd), &be());
        }
        engine
    }

    fn column_of(engine: &TilingEngine, hwnd: isize) -> crate::layout::Column {
        let oid = OutputId::from_name("M");
        let (_, workspace, column) = engine.locate_window(WindowId::new(hwnd)).unwrap();
        engine.monitors.get(&oid).unwrap().workspaces[&workspace].columns[column].clone()
    }

    #[test]
    fn a_fullscreen_window_keeps_its_column_and_the_others_stay_where_they_are() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        let before = engine.locate_window(two).unwrap();
        let width_before = column_of(&engine, 2).width;
        engine.toggle_fullscreen_for(two, &be());
        assert!(engine.is_fullscreen(two));
        assert_eq!(engine.locate_window(two), Some(before), "the same place in the strip");
        // The column is as wide as the view for as long as it is fullscreen...
        let column = column_of(&engine, 2);
        assert!(column.maximized);
        assert_eq!(column.width, Some(1920 - 16));
        // ...and nobody else was touched: no other window is fullscreen, nothing was hidden.
        assert!(!engine.is_fullscreen(WindowId::new(1)));
        assert_eq!(engine.tiled_windows().len(), 3);
        for other in [1, 3] {
            let applied = engine.applied_state.get(&WindowId::new(other));
            assert!(applied.map_or(true, |a| a.visible), "window {} is not hidden", other);
        }
        // Leaving fullscreen gives the column its width back.
        engine.toggle_fullscreen_for(two, &be());
        assert!(!engine.is_fullscreen(two));
        let column = column_of(&engine, 2);
        assert!(!column.maximized);
        assert_eq!(column.width, width_before);
        assert_eq!(engine.locate_window(two), Some(before));
    }

    #[test]
    fn a_column_that_was_maximized_before_stays_maximized_after_fullscreen() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        engine.monitors.focused_mut().unwrap().focus_window = Some(two);
        engine.monitors.focused_mut().unwrap().focus_column = Some(1);
        engine.toggle_maximize_focused_column(&be());
        assert!(column_of(&engine, 2).maximized);
        engine.toggle_fullscreen_for(two, &be());
        engine.toggle_fullscreen_for(two, &be());
        assert!(
            column_of(&engine, 2).maximized,
            "the user's maximize-column is not undone by fullscreen"
        );
    }

    #[test]
    fn the_layout_draws_a_fullscreen_tile_over_the_monitor_and_the_neighbours_a_view_away() {
        let mut engine = with_three_windows();
        let oid = OutputId::from_name("M");
        engine.toggle_fullscreen_for(WindowId::new(2), &be());
        let cfg = engine.config.clone();
        let monitor = engine.monitors.get(&oid).unwrap();
        let positions = engine.calculate_positions(
            monitor.workspace().unwrap(),
            Rect::new(8, 8, 1904, 1024),
            &cfg,
        );
        let ids: HashSet<WindowId> = [1, 2, 3].iter().map(|h| WindowId::new(*h)).collect();
        let modes = engine.sizing_draw_modes(&ids);
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[&WindowId::new(2)], DrawMode::Exact);
        let drawn = TilingEngine::apply_sizing_states(
            positions,
            &modes,
            Rect::new(8, 8, 1904, 1024),
            monitor.work_area,
            monitor.bounds,
        );
        let at = |hwnd: isize| drawn.iter().find(|(w, _)| *w == WindowId::new(hwnd)).unwrap().1;
        // The strip is not scrolled in this test: column 2 starts at column 1's end.
        let second = at(2);
        assert_eq!((second.size.w, second.size.h), (1920, 1080));
        assert_eq!(second.loc.y, 0);
        // The third column starts after the fullscreen one's whole slot, a gap beyond.
        assert!(at(3).loc.x >= second.loc.x + 1904, "the neighbour is out of the way");
    }

    #[test]
    fn maximize_window_to_edges_keeps_the_column_and_toggles() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        let before = engine.locate_window(two).unwrap();
        engine.toggle_maximize_window_to_edges_for(two, &be());
        assert!(engine.is_maximized_to_edges(two));
        assert!(!engine.is_fullscreen(two));
        assert_eq!(engine.locate_window(two), Some(before));
        assert!(column_of(&engine, 2).maximized);
        let ids: HashSet<WindowId> = [1, 2, 3].iter().map(|h| WindowId::new(*h)).collect();
        assert_eq!(engine.sizing_draw_modes(&ids)[&two], DrawMode::Edges);
        engine.toggle_maximize_window_to_edges_for(two, &be());
        assert!(!engine.is_maximized_to_edges(two));
        assert!(!column_of(&engine, 2).maximized);
        assert!(engine.sizing_draw_modes(&ids).is_empty());
    }

    #[test]
    fn fullscreen_wins_over_maximized_and_leaving_it_goes_back_to_maximized() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        engine.toggle_maximize_window_to_edges_for(two, &be());
        engine.toggle_fullscreen_for(two, &be());
        let ids: HashSet<WindowId> = [two].into_iter().collect();
        assert_eq!(engine.sizing_draw_modes(&ids)[&two], DrawMode::Exact);
        engine.toggle_fullscreen_for(two, &be());
        assert_eq!(engine.sizing_draw_modes(&ids)[&two], DrawMode::Edges);
        assert!(column_of(&engine, 2).maximized, "still maximized to the edges");
        engine.toggle_maximize_window_to_edges_for(two, &be());
        assert!(!column_of(&engine, 2).maximized);
    }

    #[test]
    fn a_floating_window_cannot_be_maximized_and_floating_ends_the_states_of_a_tile() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        engine.toggle_floating_for(two, &be());
        engine.toggle_maximize_window_to_edges_for(two, &be());
        assert!(!engine.is_maximized_to_edges(two), "a float stays a float");
        engine.toggle_floating_for(two, &be());
        // Tiles: fullscreen and maximized end when the window floats.
        let one = WindowId::new(1);
        engine.toggle_maximize_window_to_edges_for(one, &be());
        engine.toggle_fullscreen_for(one, &be());
        engine.toggle_floating_for(one, &be());
        assert!(engine.is_floating(one));
        assert!(!engine.is_fullscreen(one));
        assert!(!engine.is_maximized_to_edges(one));
    }

    #[test]
    fn an_application_going_fullscreen_is_followed_and_leaving_is_followed_too() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        let before = engine.locate_window(two).unwrap();
        engine.window_size_class_changed(two, false, true, &be());
        assert!(engine.is_fullscreen(two));
        assert!(engine.app_fullscreen.contains(&two), "its own doing: its frame is not touched");
        assert_eq!(engine.locate_window(two), Some(before));
        assert!(column_of(&engine, 2).maximized);
        // Just after: the noise of the frame going away is not "leaving".
        engine.window_size_class_changed(two, false, false, &be());
        assert!(engine.is_fullscreen(two));
        // Later: the application shrinks the window: it left fullscreen.
        engine
            .fullscreen_since
            .insert(two, Instant::now() - Duration::from_secs(5));
        engine.window_size_class_changed(two, false, false, &be());
        assert!(!engine.is_fullscreen(two));
        assert!(!column_of(&engine, 2).maximized);
        assert!(!engine.app_fullscreen.contains(&two));
    }

    #[test]
    fn a_window_wiri_put_over_the_monitor_is_not_taken_for_an_application_going_fullscreen() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        let mut applied = AppliedState::unset();
        applied.rect = Rect::new(0, 0, 1920, 1080);
        engine.applied_state.insert(two, applied);
        engine.window_size_class_changed(two, false, true, &be());
        assert!(!engine.is_fullscreen(two));
    }

    #[test]
    fn an_application_that_maximizes_itself_is_maximized_to_the_edges_once_it_has_been_restored() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        // The maximize button: the native state is taken away first...
        engine.window_size_class_changed(two, true, false, &be());
        assert_eq!(engine.unzoom_pending.get(&two), Some(&true));
        assert!(!engine.is_maximized_to_edges(two));
        // ...and when the window reports that it is a normal window again, the state is set.
        engine.window_size_class_changed(two, false, false, &be());
        assert!(engine.is_maximized_to_edges(two));
        assert!(engine.unzoom_pending.is_empty());
        // The button again: the window is taken back to its tile.
        engine.window_size_class_changed(two, true, false, &be());
        assert_eq!(engine.unzoom_pending.get(&two), Some(&false));
        engine.window_size_class_changed(two, false, false, &be());
        assert!(!engine.is_maximized_to_edges(two));
    }

    #[test]
    fn a_window_that_opens_as_big_as_the_monitor_is_adopted_as_fullscreen() {
        let mut engine = engine();
        let mut covering = window(7);
        covering.bounds = Rect::new(0, 0, 1920, 1080);
        engine.add_window(covering, &be());
        assert!(engine.is_fullscreen(WindowId::new(7)));
        assert!(engine.app_fullscreen.contains(&WindowId::new(7)));
        // An ordinary window is not.
        engine.add_window(window(8), &be());
        assert!(!engine.is_fullscreen(WindowId::new(8)));
    }

    #[test]
    fn minimizing_a_fullscreen_window_gives_the_column_back_and_restoring_makes_it_fullscreen_again() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        let width_before = column_of(&engine, 2).width;
        engine.toggle_fullscreen_for(two, &be());
        engine.minimize_window(two, &be());
        assert!(!engine.is_fullscreen(two));
        assert!(engine.is_minimized(two));
        engine.restore_window(two, &be());
        assert!(engine.is_fullscreen(two));
        assert!(column_of(&engine, 2).maximized);
        engine.toggle_fullscreen_for(two, &be());
        assert_eq!(column_of(&engine, 2).width, width_before);
    }

    #[test]
    fn a_window_that_closes_leaves_nothing_of_its_states_behind() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        engine.toggle_fullscreen_for(two, &be());
        engine.toggle_maximize_window_to_edges_for(two, &be());
        engine.remove_window(two, &be());
        assert!(!engine.is_fullscreen(two));
        assert!(!engine.is_maximized_to_edges(two));
        assert!(!engine.slot_forced.contains(&two));
        assert!(!engine.fullscreen_since.contains_key(&two));
    }

    #[test]
    fn the_open_rules_maximize_to_the_edges_and_fullscreen() {
        let mut engine = engine();
        engine.set_window_rules(vec![
            crate::config::WindowRule {
                match_groups: vec![crate::config::types::MatchGroup {
                    conds: vec![crate::config::types::Matcher::TitleSearch(
                        crate::config::types::RegexPat::new("^Window 5").unwrap(),
                    )],
                }],
                open_maximized_to_edges: Some(true),
                ..crate::config::WindowRule::default()
            },
        ]);
        engine.add_window(window(5), &be());
        assert!(engine.is_maximized_to_edges(WindowId::new(5)));
        engine.add_window(window(6), &be());
        assert!(!engine.is_maximized_to_edges(WindowId::new(6)));
    }
}
