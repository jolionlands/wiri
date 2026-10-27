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
use crate::layout::workspace::ColumnDisplay;
use crate::layout::Column;
use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    // Style changes can synchronously report sizes back to the engine. Never interpret
    // those reports as a new application request halfway through a column transition.
    static SIZING_TRANSITION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct SizingTransition;

impl SizingTransition {
    fn begin() -> Self {
        SIZING_TRANSITION_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }

    fn in_progress() -> bool {
        SIZING_TRANSITION_DEPTH.with(|depth| depth.get() != 0)
    }
}

impl Drop for SizingTransition {
    fn drop(&mut self) {
        SIZING_TRANSITION_DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}

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
    /// The rectangle wiri last put the window in is as big as its monitor but beside it (a
    /// fullscreen window scrolled with its slot), so a window that does not cover its monitor is
    /// wiri's own doing, not the application leaving fullscreen.
    pub own_rect_beside_monitor: bool,
    /// The window has been fullscreen for a while.
    pub fullscreen_settled: bool,
}

/// What to do about a window that reports its size class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SizeReaction {
    Nothing,
    /// The application maximized itself (its title bar button): take the native state away
    /// and, when the window reports it is gone, set maximized-to-edges to `wants_maximized`.
    RestoreNatively {
        wants_maximized: bool,
    },
    /// The native state is gone: set maximized-to-edges to this.
    SetMaximized(bool),
    /// The window covers its monitor on its own: it is fullscreen.
    EnterFullscreen,
    /// The window stopped covering its monitor on its own: it left fullscreen.
    LeaveFullscreen,
}

/// The decision, apart from the Win32 calls it leads to.
pub(super) fn size_reaction(
    zoomed: bool,
    covers_monitor: bool,
    state: SizingState,
) -> SizeReaction {
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
    if !covers_monitor
        && state.fullscreen
        && state.fullscreen_settled
        && !state.own_rect_beside_monitor
    {
        return SizeReaction::LeaveFullscreen;
    }
    SizeReaction::Nothing
}

impl TilingEngine {
    /// Only current members share sizing. Stacked tiles, floats and minimized windows
    /// retain their per-window state, even if they used to belong to a tabbed column.
    fn sizing_tabbed_column(&self, window_id: WindowId) -> Option<&Column> {
        if self.floating_windows.contains(&window_id) || self.minimized.contains_key(&window_id) {
            return None;
        }
        let (output, workspace, column) = self.locate_window(window_id)?;
        let column = self
            .monitors
            .get(&output)?
            .workspaces
            .get(&workspace)?
            .columns
            .get(column)?;
        matches!(column.display, ColumnDisplay::Tabbed { .. }).then_some(column)
    }

    fn sizing_members(&self, window_id: WindowId) -> Vec<WindowId> {
        self.sizing_tabbed_column(window_id)
            .map(|column| column.tiles.iter().map(|tile| tile.window_id).collect())
            .unwrap_or_else(|| vec![window_id])
    }

    pub(super) fn effective_fullscreen(&self, window_id: WindowId) -> bool {
        self.sizing_tabbed_column(window_id).map_or_else(
            || self.fullscreen_windows.contains(&window_id),
            |column| {
                column
                    .tiles
                    .iter()
                    .any(|tile| self.fullscreen_windows.contains(&tile.window_id))
            },
        )
    }

    /// The window (or its current tabbed column) fills the work area.
    pub fn is_maximized_to_edges(&self, window_id: WindowId) -> bool {
        self.sizing_tabbed_column(window_id).map_or_else(
            || self.maximized_edges.contains(&window_id),
            |column| {
                column
                    .tiles
                    .iter()
                    .any(|tile| self.maximized_edges.contains(&tile.window_id))
            },
        )
    }

    /// Column-level reservations/decorations only follow sizing in tabbed mode. A
    /// fullscreen stacked tile must not suppress decorations for the other rows.
    pub(super) fn column_sizing_mode(&self, column: &Column) -> DrawMode {
        if !matches!(column.display, ColumnDisplay::Tabbed { .. }) {
            DrawMode::Inset
        } else if column
            .tiles
            .iter()
            .any(|tile| self.fullscreen_windows.contains(&tile.window_id))
        {
            DrawMode::Exact
        } else if column
            .tiles
            .iter()
            .any(|tile| self.maximized_edges.contains(&tile.window_id))
        {
            DrawMode::Edges
        } else {
            DrawMode::Inset
        }
    }

    /// Invalidate geometry, not visibility: forgetting a hidden tab's visibility can
    /// make a reentrant layout show it. Its next selection must still reapply geometry.
    fn invalidate_sizing_geometry(&mut self, window_id: WindowId) {
        if let Some(applied) = self.applied_state.get_mut(&window_id) {
            applied.rect = Rect::default();
        }
        self.animating_rects.remove(&window_id);
        self.window_sizing.insert(
            window_id,
            if self.fullscreen_windows.contains(&window_id) {
                SizingMode::Fullscreen
            } else if self.maximized_edges.contains(&window_id) {
                SizingMode::Maximized
            } else {
                SizingMode::Normal
            },
        );
    }

    /// Record inheritance without native effects. The caller must finish recording its
    /// entire transition before styling the returned newly fullscreen members.
    fn record_sizing_group(&mut self, members: &[WindowId]) -> Vec<WindowId> {
        let fullscreen = members
            .iter()
            .any(|wid| self.fullscreen_windows.contains(wid));
        let edges = members.iter().any(|wid| self.maximized_edges.contains(wid));
        let mut newly_fullscreen = Vec::new();
        if fullscreen || edges {
            let now = Instant::now();
            for &member in members {
                let mut changed = false;
                if fullscreen {
                    let newly = self.fullscreen_windows.insert(member);
                    // An existing non-tab fullscreen already has its native style.
                    self.tab_sizing_styles
                        .entry(member)
                        .or_insert(!newly && !self.app_fullscreen.contains(&member));
                    if newly {
                        self.fullscreen_since.insert(member, now);
                        newly_fullscreen.push(member);
                        changed = true;
                    }
                }
                if edges && self.maximized_edges.insert(member) {
                    changed = true;
                }
                if changed {
                    self.invalidate_sizing_geometry(member);
                }
            }
            if let Some(&member) = members.first() {
                self.hold_slot(member);
            }
        }
        newly_fullscreen
    }

    /// This also handles newly inserted tabs and stacked -> tabbed transitions.
    fn record_tabbed_sizing(&mut self, output: OutputId) -> Vec<WindowId> {
        let groups: Vec<Vec<_>> = self
            .monitors
            .get(&output)
            .into_iter()
            .flat_map(|monitor| monitor.workspaces.values())
            .flat_map(|workspace| &workspace.columns)
            .filter(|column| matches!(column.display, ColumnDisplay::Tabbed { .. }))
            .map(|column| column.tiles.iter().map(|tile| tile.window_id).collect())
            .collect();
        let mut newly_fullscreen = Vec::new();
        for members in groups {
            newly_fullscreen.extend(self.record_sizing_group(&members));
        }
        newly_fullscreen
    }

    fn apply_inherited_fullscreen(&mut self, members: Vec<WindowId>) {
        for member in members {
            if let Some(hwnd) = self.tiled_windows.get(&member).map(|window| window.hwnd) {
                crate::backend::taskbar::mark_fullscreen(hwnd, true);
            }
        }
    }

    /// Only tabs layout is about to show may require a native style change. In
    /// particular, entering/exiting fullscreen must not synchronously message every
    /// hidden sibling (which may be hung). No style work runs for overview sources.
    fn tab_style_changes(&self, output: OutputId) -> Vec<(WindowId, bool)> {
        let Some(workspace) = self.monitors.get(&output).and_then(|m| m.workspace()) else {
            return Vec::new();
        };
        workspace
            .columns
            .iter()
            .flat_map(|column| {
                column
                    .visible_tile_indices()
                    .into_iter()
                    .filter_map(move |index| {
                        let member = column.tiles[index].window_id;
                        let styled = self.tab_sizing_styles.get(&member)?;
                        let wanted = self.fullscreen_windows.contains(&member)
                            && !self.app_fullscreen.contains(&member);
                        (*styled != wanted).then_some((member, wanted))
                    })
            })
            .collect()
    }

    pub(super) fn sync_tab_sizing_styles(&mut self, output: OutputId) {
        let _transition = SizingTransition::begin();
        for (member, wanted) in self.tab_style_changes(output) {
            let Some(hwnd) = self.tiled_windows.get(&member).map(|w| w.hwnd) else {
                continue;
            };
            if wanted {
                self.tab_sizing_styles.insert(member, true);
                // A long-hidden tab's queued frame reports need their own settle
                // interval; the group's original entry may have been minutes ago.
                self.fullscreen_since.insert(member, Instant::now());
                self.style_tab_fullscreen(hwnd);
            } else {
                self.tab_sizing_styles.remove(&member);
                self.restore_tab_sizing_style(hwnd);
            }
            self.invalidate_sizing_geometry(member);
        }
    }

    /// Called before computing layout/decorations. No recursive layout, no showing,
    /// raising, styling or moving of HWNDs (including newly selected hidden tabs).
    pub(super) fn sync_tabbed_sizing(&mut self, output: OutputId) {
        let _transition = SizingTransition::begin();
        let inherited = self.record_tabbed_sizing(output);
        // Moves and minimize restoration may leave a cloned forced-width column
        // without any sizing member. Provenance belongs to the column, not an HWND.
        // A workspace's own layout block can change its view width (C2), so the width a
        // forced column returns to is resolved per workspace.
        let ids: Vec<i32> = self
            .monitors
            .get(&output)
            .map(|monitor| monitor.workspaces.keys().copied().collect())
            .unwrap_or_default();
        let views: Vec<(i32, u32)> = ids
            .into_iter()
            .map(|id| (id, self.slot_view_width(output, id).unwrap_or(50)))
            .collect();
        if let Some(monitor) = self.monitors.get_mut(&output) {
            for (id, view) in views {
                let Some(workspace) = monitor.workspaces.get_mut(&id) else {
                    continue;
                };
                for column in workspace.columns.iter_mut() {
                    if column.sizing_forced
                        && !column.tiles.iter().any(|tile| {
                            self.fullscreen_windows.contains(&tile.window_id)
                                || self.maximized_edges.contains(&tile.window_id)
                        })
                    {
                        if column.maximized {
                            column.toggle_maximized(view);
                        }
                        column.sizing_forced = false;
                        for tile in &column.tiles {
                            self.slot_forced.remove(&tile.window_id);
                        }
                    }
                }
            }
        }
        self.apply_inherited_fullscreen(inherited);
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
            if self.effective_fullscreen(*window_id) {
                modes.insert(*window_id, DrawMode::Exact);
            } else if self.is_maximized_to_edges(*window_id) {
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
                (
                    window_id,
                    state_rect(mode, slot, work_rect, work_area, monitor),
                )
            })
            .collect()
    }

    /// Tab fullscreen changes styles only. Layout owns placement/visibility, so even a
    /// selected tab that has not been shown yet is never moved or raised here.
    fn style_tab_fullscreen(&mut self, hwnd: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
            SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            SWP_NOZORDER, WS_CAPTION, WS_EX_APPWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU,
            WS_THICKFRAME,
        };
        let window_id = WindowId::new(hwnd);
        let native = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            let style = GetWindowLongW(native, GWL_STYLE) as u32;
            let ex_style = GetWindowLongW(native, GWL_EXSTYLE) as u32;
            // strip-frame may already have saved the actual originals.
            self.saved_styles.entry(window_id).or_insert(style);
            self.saved_ex_styles.entry(window_id).or_insert(ex_style);
            let stripped = style
                & !(WS_CAPTION.0
                    | WS_THICKFRAME.0
                    | WS_SYSMENU.0
                    | WS_MINIMIZEBOX.0
                    | WS_MAXIMIZEBOX.0);
            SetWindowLongW(native, GWL_STYLE, stripped as i32);
            SetWindowLongW(native, GWL_EXSTYLE, (ex_style & !WS_EX_APPWINDOW.0) as i32);
            let _ = SetWindowPos(
                native,
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | SWP_FRAMECHANGED
                    | SWP_ASYNCWINDOWPOS,
            );
        }
        crate::backend::forget_shadow_margins(hwnd);
    }

    /// Saved styles can contain WS_VISIBLE from before a tab was hidden. Restoring
    /// them verbatim would show that tab behind layout's back, including reentrantly.
    fn restore_tab_sizing_style(&self, hwnd: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
            SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            SWP_NOZORDER, WS_VISIBLE,
        };
        let window_id = WindowId::new(hwnd);
        let native = HWND(hwnd as *mut std::ffi::c_void);
        unsafe {
            if let Some(&saved) = self.saved_styles.get(&window_id) {
                let visible = GetWindowLongW(native, GWL_STYLE) as u32 & WS_VISIBLE.0;
                SetWindowLongW(
                    native,
                    GWL_STYLE,
                    ((saved & !WS_VISIBLE.0) | visible) as i32,
                );
            }
            if let Some(&saved) = self.saved_ex_styles.get(&window_id) {
                SetWindowLongW(native, GWL_EXSTYLE, saved as i32);
            }
            let _ = SetWindowPos(
                native,
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | SWP_FRAMECHANGED
                    | SWP_ASYNCWINDOWPOS,
            );
        }
        crate::backend::forget_shadow_margins(hwnd);
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

    /// The width of the view of `workspace` on `output`, which a column is made while its
    /// window is fullscreen or maximized (the width `maximize-column` gives).
    fn slot_view_width(&self, output: OutputId, workspace: i32) -> Option<u32> {
        let monitor = self.monitors.get(&output)?;
        let cfg = self.effective_config_for_workspace(output, workspace);
        Some(Self::view_width_for_monitor(monitor, cfg).max(50) as u32)
    }

    /// The window takes the whole view while it is fullscreen or maximized: its column is made
    /// as wide as the view, unless it is already (`maximize-column`).  What was done is
    /// remembered, so that [`Self::release_slot`] gives the old width back.
    pub(super) fn hold_slot(&mut self, window_id: WindowId) {
        let Some((output, workspace, column)) = self.locate_window(window_id) else {
            return;
        };
        let Some(view) = self.slot_view_width(output, workspace) else {
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
            column.sizing_forced = true;
        }
        if column.sizing_forced {
            // Each tab may later split into a column that clones pre_max_width.
            // Keep an ownership token with it, not only with the original initiator.
            if matches!(column.display, ColumnDisplay::Tabbed { .. }) {
                self.slot_forced
                    .extend(column.tiles.iter().map(|t| t.window_id));
            } else {
                self.slot_forced.insert(window_id);
            }
        } else {
            // Incoming windows do not transfer forced-width provenance into a
            // destination that was already maximized manually.
            for tile in &column.tiles {
                self.slot_forced.remove(&tile.window_id);
            }
        }
    }

    /// Release a sizing-forced width only after the last current member no longer
    /// needs it. When its owner leaves, move ownership to a surviving sizing member.
    fn release_slot(&mut self, window_id: WindowId) {
        self.release_slot_for(window_id, false);
    }

    fn release_slot_for(&mut self, window_id: WindowId, departing: bool) {
        if !departing
            && (self.fullscreen_windows.contains(&window_id)
                || self.maximized_edges.contains(&window_id))
        {
            return;
        }
        self.slot_forced.remove(&window_id);
        let Some((output, workspace, column_index)) = self.locate_window(window_id) else {
            return;
        };
        let forced = self
            .monitors
            .get(&output)
            .and_then(|m| m.workspaces.get(&workspace))
            .and_then(|w| w.columns.get(column_index))
            .is_some_and(|c| c.sizing_forced);
        if !forced {
            return;
        }
        let successor = self
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspaces.get(&workspace))
            .and_then(|workspace| workspace.columns.get(column_index))
            .and_then(|column| {
                column
                    .tiles
                    .iter()
                    .map(|tile| tile.window_id)
                    .find(|&member| {
                        member != window_id
                            && (self.fullscreen_windows.contains(&member)
                                || self.maximized_edges.contains(&member))
                    })
            });
        if let Some(successor) = successor {
            self.slot_forced.insert(successor);
            return;
        }
        let Some(view) = self.slot_view_width(output, workspace) else {
            return;
        };
        if let Some(column) = self
            .monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspaces.get_mut(&workspace))
            .and_then(|workspace| workspace.columns.get_mut(column_index))
        {
            if column.maximized {
                column.toggle_maximized(view);
            }
        }
    }

    /// niri `fullscreen-window` for one window (or its tabbed column): toggle.
    pub fn toggle_fullscreen_for(&mut self, window_id: WindowId, backend: &BackendHandle) {
        let on = !self.effective_fullscreen(window_id);
        self.set_fullscreen(window_id, on, false, backend);
    }

    /// Update the whole current tab group before calling Win32: native style changes
    /// can report sizes synchronously, and must never see a half-transitioned column.
    fn set_tabbed_fullscreen(
        &mut self,
        window_id: WindowId,
        on: bool,
        by_app: bool,
        backend: &BackendHandle,
    ) {
        let members = self.sizing_members(window_id);
        if members
            .iter()
            .all(|member| self.fullscreen_windows.contains(member) == on)
        {
            return;
        }
        let Some((output, _, _)) = self.locate_window(window_id) else {
            return;
        };
        let transition = SizingTransition::begin();
        let edges = members
            .iter()
            .any(|member| self.maximized_edges.contains(member));
        let now = Instant::now();
        for &member in &members {
            let was_fullscreen = self.fullscreen_windows.contains(&member);
            let was_app = self.app_fullscreen.contains(&member);
            if edges {
                self.maximized_edges.insert(member);
            }
            if on {
                self.tab_sizing_styles
                    .entry(member)
                    .or_insert(was_fullscreen && !was_app);
                self.fullscreen_windows.insert(member);
                if !was_fullscreen {
                    self.fullscreen_since.insert(member, now);
                }
                if by_app && member == window_id {
                    self.app_fullscreen.insert(member);
                }
            } else {
                self.fullscreen_windows.remove(&member);
                self.fullscreen_since.remove(&member);
                self.app_fullscreen.remove(&member);
                if self.tab_sizing_styles.get(&member) == Some(&false) {
                    self.tab_sizing_styles.remove(&member);
                }
            }
            self.invalidate_sizing_geometry(member);
        }
        if on {
            self.hold_slot(window_id);
        } else {
            for &member in &members {
                self.release_slot(member);
            }
        }
        for member in members {
            if let Some(hwnd) = self.tiled_windows.get(&member).map(|w| w.hwnd) {
                crate::backend::taskbar::mark_fullscreen(hwnd, on);
            }
        }
        drop(transition);
        self.reveal_focused_column(output);
        self.apply_layout_for_monitor(output, backend);
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
        if self.sizing_tabbed_column(window_id).is_some() {
            self.set_tabbed_fullscreen(window_id, on, by_app, backend);
            return;
        }
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
            if !by_app && !self.tab_sizing_styles.contains_key(&window_id) {
                // A former tab keeps its deferred native style until layout selects it.
                // Ordinary independent fullscreen retains the existing native path.
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
                match self.tab_sizing_styles.remove(&window_id) {
                    Some(true) => self.restore_tab_sizing_style(hwnd),
                    Some(false) => {} // Hidden inherited fullscreen never changed this HWND.
                    None => self.restore_window_style(hwnd),
                }
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
        {
            return;
        }
        let members = self.sizing_members(window_id);
        if members
            .iter()
            .all(|member| self.maximized_edges.contains(member) == on)
        {
            return;
        }
        let Some((output, _, _)) = self.locate_window(window_id) else {
            return;
        };
        let transition = SizingTransition::begin();
        // A member inserted since the last layout inherits fullscreen as part of the
        // same atomic transition. Do not touch its HWND until edges are recorded too.
        let inherited = self.record_sizing_group(&members);
        for &member in &members {
            if on {
                self.maximized_edges.insert(member);
            } else {
                self.maximized_edges.remove(&member);
            }
            self.invalidate_sizing_geometry(member);
        }
        if on {
            self.hold_slot(window_id);
        } else {
            for &member in &members {
                self.release_slot(member);
            }
        }
        self.apply_inherited_fullscreen(inherited);
        drop(transition);
        info!(
            "Window {} is {} maximized to the edges",
            window_id.as_isize(),
            if on { "now" } else { "no longer" }
        );
        self.reveal_focused_column(output);
        self.apply_layout_for_monitor(output, backend);
    }

    /// niri `maximize-window-to-edges` for one window: toggle.
    pub fn toggle_maximize_window_to_edges_for(
        &mut self,
        window_id: WindowId,
        backend: &BackendHandle,
    ) {
        let on = !self.is_maximized_to_edges(window_id);
        self.set_maximized_to_edges(window_id, on, backend);
    }

    /// A window that is about to leave the strip (it floats, or is minimized) is not
    /// fullscreen any more, and not maximized to the edges unless `keep_maximized` (a
    /// minimized window comes back maximized).  Its column gets its width back.  Nothing is
    /// laid out: the caller moves the window next.
    pub(super) fn leave_sizing_states(&mut self, window_id: WindowId, keep_maximized: bool) {
        let _transition = SizingTransition::begin();
        // Lifecycle callers remove membership afterwards. Record inheritance first, while
        // the departing owner can still pass its flags and width hold to its siblings.
        let inherited = if self.sizing_tabbed_column(window_id).is_some() {
            self.record_sizing_group(&self.sizing_members(window_id))
        } else {
            Vec::new()
        };
        self.release_slot_for(window_id, true);
        let was_fullscreen = self.fullscreen_windows.remove(&window_id);
        self.fullscreen_since.remove(&window_id);
        let by_app = self.app_fullscreen.remove(&window_id);
        if !keep_maximized {
            self.maximized_edges.remove(&window_id);
        }
        self.unzoom_pending.remove(&window_id);
        self.invalidate_sizing_geometry(window_id);
        let tab_styled = self.tab_sizing_styles.remove(&window_id);
        if was_fullscreen {
            if let Some(hwnd) = self.tiled_windows.get(&window_id).map(|w| w.hwnd) {
                if !by_app && tab_styled != Some(false) {
                    self.restore_tab_sizing_style(hwnd);
                }
                crate::backend::taskbar::mark_fullscreen(hwnd, false);
            }
        }
        self.apply_inherited_fullscreen(
            inherited
                .into_iter()
                .filter(|&member| member != window_id)
                .collect(),
        );
    }

    /// The window is gone: forget it without touching its dead HWND. Called before
    /// membership is removed, so the surviving tabs can inherit its sizing/width hold.
    pub(super) fn forget_sizing(&mut self, window_id: WindowId) {
        let _transition = SizingTransition::begin();
        let inherited = if self.sizing_tabbed_column(window_id).is_some() {
            self.record_sizing_group(&self.sizing_members(window_id))
        } else {
            Vec::new()
        };
        self.release_slot_for(window_id, true);
        self.fullscreen_windows.remove(&window_id);
        self.maximized_edges.remove(&window_id);
        self.app_fullscreen.remove(&window_id);
        self.tab_sizing_styles.remove(&window_id);
        self.unzoom_pending.remove(&window_id);
        self.fullscreen_since.remove(&window_id);
        self.apply_inherited_fullscreen(
            inherited
                .into_iter()
                .filter(|&member| member != window_id)
                .collect(),
        );
    }

    /// Hidden or inactive tabs report changes caused by being parked, not user requests.
    fn ignores_sizing_report(&self, window_id: WindowId) -> bool {
        if SizingTransition::in_progress() {
            return true;
        }
        if let Some(column) = self.sizing_tabbed_column(window_id) {
            let Some((output, workspace, _)) = self.locate_window(window_id) else {
                return true;
            };
            let Some(monitor) = self.monitors.get(&output) else {
                return true;
            };
            if workspace != monitor.active_workspace {
                return true;
            }
            if let ColumnDisplay::Tabbed { active_tab } = column.display {
                // Layout first reconciles the tab index with monitor focus. Use the same
                // selection even if this report arrived before hide_inactive_tabs ran.
                let selected = monitor
                    .focus_window
                    .filter(|id| column.tiles.iter().any(|tile| tile.window_id == *id))
                    .or_else(|| column.tiles.get(active_tab).map(|tile| tile.window_id));
                if selected != Some(window_id) {
                    return true;
                }
            }
        }
        self.applied_state
            .get(&window_id)
            .is_some_and(|state| !state.visible)
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
        // Style/layout feedback (including a hidden tab being parked) is not an app request.
        // A float keeps whatever its application does with it; minimized reports are noise.
        if self.floating_windows.contains(&window_id)
            || self.minimized.contains_key(&window_id)
            || self.ignores_sizing_report(window_id)
        {
            return;
        }
        let Some((output, _, _)) = self.locate_window(window_id) else {
            return;
        };
        let monitor_bounds = self.monitors.get(&output).map(|m| m.bounds);
        let (own_rect_covers, own_rect_beside_monitor) =
            match (self.applied_state.get(&window_id), monitor_bounds) {
                (Some(applied), Some(bounds)) if applied.rect.size.w > 0 => {
                    let covers = crate::backend::hooks::covers_monitor(applied.rect, bounds);
                    let monitor_sized = applied.rect.size.w == bounds.size.w
                        && applied.rect.size.h == bounds.size.h;
                    (covers, monitor_sized && !covers)
                }
                _ => (false, false),
            };
        let state = SizingState {
            fullscreen: self.effective_fullscreen(window_id),
            maximized_edges: self.is_maximized_to_edges(window_id),
            unzoom_pending: self.unzoom_pending.get(&window_id).copied(),
            own_rect_covers,
            own_rect_beside_monitor,
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
                let unchanged = self.maximized_edges.contains(&window_id) == wanted;
                self.applied_state.remove(&window_id);
                self.set_maximized_to_edges(window_id, wanted, backend);
                // A normalized interactive drop can already have the wanted state,
                // but the application's async restore still invalidates its rectangle.
                if unchanged {
                    self.apply_layout_for_monitor(output, backend);
                }
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
        let covers = self.monitors.get(&output).map_or(false, |m| {
            crate::backend::hooks::covers_monitor(frame, m.bounds)
        });
        if covers && !self.effective_fullscreen(window_id) {
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
        assert_eq!(
            state_rect(DrawMode::Exact, slot, work_rect, work_area, monitor),
            monitor
        );
        // The strip scrolled by a view: the slot is a view to the left of the view's start, and
        // so is the window (by the same distance).
        let scrolled = rect(8 - 1366, 8, 1350, 712);
        assert_eq!(
            state_rect(DrawMode::Exact, scrolled, work_rect, work_area, monitor),
            rect(-1366, 0, 1366, 768)
        );
        // Whatever row of its column the tile is in, the window covers the monitor.
        let lower = rect(8, 300, 1350, 400);
        assert_eq!(
            state_rect(DrawMode::Exact, lower, work_rect, work_area, monitor),
            monitor
        );
    }

    #[test]
    fn a_maximized_window_is_drawn_over_the_work_area() {
        let work_rect = rect(8, 8, 1350, 712);
        let work_area = rect(0, 0, 1366, 728);
        let monitor = rect(0, 0, 1366, 768);
        assert_eq!(
            state_rect(
                DrawMode::Edges,
                rect(8, 8, 1350, 712),
                work_rect,
                work_area,
                monitor
            ),
            work_area,
            "no gap and no border: the whole work area, the taskbar stays visible"
        );
        assert_eq!(
            state_rect(
                DrawMode::Edges,
                rect(1374, 8, 1350, 712),
                work_rect,
                work_area,
                monitor
            ),
            rect(1366, 0, 1366, 728),
            "the neighbour to the right is a view away"
        );
        let slot = rect(40, 40, 100, 100);
        assert_eq!(
            state_rect(DrawMode::Inset, slot, work_rect, work_area, monitor),
            slot
        );
    }

    #[test]
    fn an_application_that_maximizes_itself_gets_maximized_to_the_edges() {
        let state = SizingState::default();
        assert_eq!(
            size_reaction(true, false, state),
            SizeReaction::RestoreNatively {
                wants_maximized: true
            }
        );
        // Its button pressed again while it is maximized to the edges: back to its tile.
        let state = SizingState {
            maximized_edges: true,
            ..SizingState::default()
        };
        assert_eq!(
            size_reaction(true, false, state),
            SizeReaction::RestoreNatively {
                wants_maximized: false
            }
        );
        // A window maximized on a monitor without a taskbar covers it too: maximized wins.
        assert_eq!(
            size_reaction(true, true, SizingState::default()),
            SizeReaction::RestoreNatively {
                wants_maximized: true
            }
        );
    }

    #[test]
    fn the_toggle_is_set_when_the_native_state_is_gone() {
        let state = SizingState {
            unzoom_pending: Some(true),
            ..SizingState::default()
        };
        assert_eq!(
            size_reaction(false, false, state),
            SizeReaction::SetMaximized(true)
        );
        let state = SizingState {
            unzoom_pending: Some(false),
            maximized_edges: true,
            ..SizingState::default()
        };
        assert_eq!(
            size_reaction(false, false, state),
            SizeReaction::SetMaximized(false)
        );
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
        assert_eq!(
            size_reaction(false, true, fullscreen),
            SizeReaction::Nothing
        );
        // It stops covering the monitor: it left fullscreen...
        assert_eq!(
            size_reaction(false, false, fullscreen),
            SizeReaction::LeaveFullscreen
        );
        // ...unless it has only just been made fullscreen, when the frame going away makes noise.
        let fresh = SizingState {
            fullscreen_settled: false,
            ..fullscreen
        };
        assert_eq!(size_reaction(false, false, fresh), SizeReaction::Nothing);
    }

    #[test]
    fn a_fullscreen_window_wiri_scrolled_beside_its_monitor_has_not_left_fullscreen() {
        // Focusing a neighbour moves the fullscreen window with its slot, monitor-sized but
        // beside the monitor.  On a slow machine the echo of that move can come in as a size
        // class report: it is wiri's own move, not the application leaving fullscreen.
        let scrolled = SizingState {
            fullscreen: true,
            fullscreen_settled: true,
            own_rect_beside_monitor: true,
            ..SizingState::default()
        };
        assert_eq!(size_reaction(false, false, scrolled), SizeReaction::Nothing);
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
        assert_eq!(
            size_reaction(false, false, SizingState::default()),
            SizeReaction::Nothing
        );
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

    fn with_two_tabs() -> TilingEngine {
        let mut engine = with_three_windows();
        let monitor = engine.monitors.focused_mut().unwrap();
        let workspace = monitor.workspace_mut().unwrap();
        let second = workspace.columns.remove(1);
        workspace.columns[0].tiles.extend(second.tiles);
        workspace.columns[0].set_width(Some(640));
        workspace.columns[0].display = ColumnDisplay::Tabbed { active_tab: 0 };
        workspace.columns[0].active_window = Some(WindowId::new(1));
        monitor.set_focus(WindowId::new(1));
        engine.apply_layout_for_monitor(OutputId::from_name("M"), &be());
        engine
    }

    fn assert_tab_drawing(engine: &TilingEngine, selected: isize, mode: DrawMode) {
        let monitor = engine.monitors.get(&OutputId::from_name("M")).unwrap();
        let work_rect = Rect::new(8, 8, 1904, 1024);
        let positions =
            engine.calculate_positions(monitor.workspace().unwrap(), work_rect, &engine.config);
        let ids = [1, 2, 3].into_iter().map(WindowId::new).collect();
        let modes = engine.sizing_draw_modes(&ids);
        assert_eq!(modes.get(&WindowId::new(selected)), Some(&mode));
        assert!(!modes.contains_key(&WindowId::new(3)));
        let drawn = TilingEngine::apply_sizing_states(
            positions,
            &modes,
            work_rect,
            monitor.work_area,
            monitor.bounds,
        );
        let tabs: Vec<_> = drawn
            .iter()
            .filter(|(wid, _)| *wid == WindowId::new(1) || *wid == WindowId::new(2))
            .collect();
        assert_eq!(tabs.len(), 1, "only the selected tab is positioned");
        assert_eq!(tabs[0].0, WindowId::new(selected));
        assert_eq!(tabs[0].1.size.w, 1920);
        assert_eq!(
            tabs[0].1.size.h,
            if mode == DrawMode::Exact { 1080 } else { 1040 }
        );
        assert_eq!(tabs[0].1.loc.y, 0);
    }

    #[test]
    fn native_style_work_is_lazy_for_hidden_tabs_on_entry_and_exit() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        let output = OutputId::from_name("M");
        engine.fullscreen_windows.extend([one, two]);
        engine
            .tab_sizing_styles
            .extend([(one, false), (two, false)]);
        assert_eq!(engine.tab_style_changes(output), vec![(one, true)]);
        engine.tab_sizing_styles.insert(one, true);
        assert!(engine.tab_style_changes(output).is_empty());
        {
            let monitor = engine.monitors.get_mut(&output).unwrap();
            monitor.workspace_mut().unwrap().columns[0].select_window(two);
            monitor.set_focus(two);
        }
        assert_eq!(engine.tab_style_changes(output), vec![(two, true)]);
        engine.tab_sizing_styles.insert(two, true);
        engine.fullscreen_windows.clear();
        // Exiting does not restore the other, hidden HWND synchronously either.
        assert_eq!(engine.tab_style_changes(output), vec![(two, false)]);
        engine.tab_sizing_styles.remove(&two);
        assert!(engine.tab_style_changes(output).is_empty());
        {
            let monitor = engine.monitors.get_mut(&output).unwrap();
            monitor.workspace_mut().unwrap().columns[0].select_window(one);
            monitor.set_focus(one);
        }
        assert_eq!(engine.tab_style_changes(output), vec![(one, false)]);
    }

    #[test]
    fn expelling_either_sized_tab_preserves_independent_width_restoration() {
        for fullscreen in [false, true] {
            for initiating in [1, 2] {
                let mut engine = with_two_tabs();
                let one = WindowId::new(1);
                let two = WindowId::new(2);
                let initiating = WindowId::new(initiating);
                if fullscreen {
                    engine.set_fullscreen(initiating, true, false, &be());
                } else {
                    engine.set_maximized_to_edges(initiating, true, &be());
                }
                engine.expel_window_from_column(&be());
                assert_ne!(engine.locate_window(one), engine.locate_window(two));
                assert!(engine.slot_forced.contains(&one));
                assert!(engine.slot_forced.contains(&two));
                if fullscreen {
                    engine.set_fullscreen(two, false, false, &be());
                } else {
                    engine.set_maximized_to_edges(two, false, &be());
                }
                assert_eq!(column_of(&engine, 2).width, Some(640));
                assert!(!column_of(&engine, 2).maximized);
                assert_eq!(column_of(&engine, 1).width, Some(1904));
                if fullscreen {
                    engine.set_fullscreen(one, false, false, &be());
                } else {
                    engine.set_maximized_to_edges(one, false, &be());
                }
                assert_eq!(column_of(&engine, 1).width, Some(640));
                assert!(engine.slot_forced.is_empty());
            }
        }
    }

    #[test]
    fn expelling_a_manually_maximized_tab_does_not_create_sizing_ownership() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        engine
            .monitors
            .focused_mut()
            .unwrap()
            .workspace_mut()
            .unwrap()
            .columns[0]
            .toggle_maximized(1904);
        engine.set_maximized_to_edges(one, true, &be());
        engine.expel_window_from_column(&be());
        assert!(engine.slot_forced.is_empty());
        engine.set_maximized_to_edges(two, false, &be());
        engine.set_maximized_to_edges(one, false, &be());
        for member in [1, 2] {
            assert!(column_of(&engine, member).maximized);
            assert_eq!(column_of(&engine, member).width, Some(1904));
        }
    }

    #[test]
    fn consuming_a_forced_tab_preserves_a_manually_maximized_destination() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        let three = WindowId::new(3);
        engine.set_maximized_to_edges(one, true, &be());
        engine
            .monitors
            .focused_mut()
            .unwrap()
            .workspace_mut()
            .unwrap()
            .columns[1]
            .toggle_maximized(1904);
        // Put the manual destination on the left and consume the forced bottom tab.
        {
            let monitor = engine.monitors.focused_mut().unwrap();
            monitor.workspace_mut().unwrap().columns.swap(0, 1);
            monitor.set_focus(two);
        }
        engine.consume_or_expel_window(false, &be());
        engine.consume_or_expel_window(false, &be());
        engine.set_maximized_to_edges(two, false, &be());
        assert!(column_of(&engine, 3).maximized);
        assert!(!column_of(&engine, 3).sizing_forced);
        assert_eq!(engine.locate_window(two), engine.locate_window(three));
    }

    #[test]
    fn orphaned_forced_width_restores_but_manual_width_does_not() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        engine.set_maximized_to_edges(one, true, &be());
        // A restored/split slot can carry width provenance without live sizing flags.
        engine.maximized_edges.clear();
        engine.slot_forced.clear();
        engine.apply_layout_for_monitor(OutputId::from_name("M"), &be());
        assert_eq!(column_of(&engine, 1).width, Some(640));
        assert!(!column_of(&engine, 1).sizing_forced);
        engine
            .monitors
            .focused_mut()
            .unwrap()
            .workspace_mut()
            .unwrap()
            .columns[0]
            .toggle_maximized(1904);
        engine.apply_layout_for_monitor(OutputId::from_name("M"), &be());
        assert!(column_of(&engine, 1).maximized);
    }

    #[test]
    fn joining_a_sized_tabbed_column_inherits_both_modes_and_its_width_hold() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let three = WindowId::new(3);
        engine.toggle_maximize_window_to_edges_for(one, &be());
        engine.set_fullscreen(one, true, false, &be());
        {
            let workspace = engine
                .monitors
                .focused_mut()
                .unwrap()
                .workspace_mut()
                .unwrap();
            let joining = workspace.columns.remove(1);
            workspace.columns[0].tiles.extend(joining.tiles);
        }
        engine.apply_layout_for_monitor(OutputId::from_name("M"), &be());
        for member in [1, 2, 3].into_iter().map(WindowId::new) {
            assert!(engine.fullscreen_windows.contains(&member));
            assert!(engine.maximized_edges.contains(&member));
            assert_eq!(engine.window_sizing_mode(member), SizingMode::Fullscreen);
        }
        assert!(!engine.app_fullscreen.contains(&three));
        assert_eq!(column_of(&engine, 3).width, Some(1904));
        engine.set_fullscreen(three, false, false, &be());
        for member in [1, 2, 3].into_iter().map(WindowId::new) {
            assert!(!engine.is_fullscreen(member));
            assert!(engine.is_maximized_to_edges(member));
        }
        engine.toggle_maximize_window_to_edges_for(three, &be());
        assert_eq!(column_of(&engine, 3).width, Some(640));
        assert!(engine.slot_forced.is_empty());
    }

    #[test]
    fn closing_the_sizing_owner_preserves_the_surviving_tabs_modes_and_original_width() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        engine.toggle_maximize_window_to_edges_for(one, &be());
        engine.set_fullscreen(one, true, false, &be());
        engine.remove_window(one, &be());
        assert!(!engine.fullscreen_windows.contains(&one));
        assert!(!engine.maximized_edges.contains(&one));
        assert!(!engine.slot_forced.contains(&one));
        assert!(engine.slot_forced.contains(&two));
        assert!(engine.is_fullscreen(two));
        assert!(engine.is_maximized_to_edges(two));
        assert_eq!(column_of(&engine, 2).width, Some(1904));
        engine.set_fullscreen(two, false, false, &be());
        assert!(engine.is_maximized_to_edges(two));
        engine.toggle_maximize_window_to_edges_for(two, &be());
        assert_eq!(column_of(&engine, 2).width, Some(640));
        assert!(engine.slot_forced.is_empty());
    }

    #[test]
    fn minimizing_and_restoring_either_tab_preserves_the_survivors_sizing() {
        for departing in [1, 2] {
            let mut engine = with_two_tabs();
            let one = WindowId::new(1);
            let departing = WindowId::new(departing);
            let survivor = if departing == one {
                WindowId::new(2)
            } else {
                one
            };
            engine.toggle_maximize_window_to_edges_for(one, &be());
            engine.set_fullscreen(one, true, false, &be());
            engine.minimize_window(departing, &be());
            assert!(engine.is_minimized(departing));
            assert!(!engine.is_fullscreen(departing));
            assert!(engine.is_fullscreen(survivor));
            assert!(engine.is_maximized_to_edges(survivor));
            assert!(engine.slot_forced.contains(&survivor));
            engine.restore_window(departing, &be());
            assert!(!engine.is_minimized(departing));
            assert!(engine.is_fullscreen(departing));
            assert!(engine.is_maximized_to_edges(departing));
            assert_eq!(
                engine.locate_window(departing),
                engine.locate_window(survivor)
            );
            engine.set_fullscreen(departing, false, false, &be());
            assert!(engine.is_maximized_to_edges(survivor));
            engine.toggle_maximize_window_to_edges_for(survivor, &be());
            assert_eq!(column_of(&engine, 1).width, Some(640));
            assert!(engine.slot_forced.is_empty());
        }
    }

    #[test]
    fn hidden_tabs_and_reentrant_transitions_cannot_report_sizing_changes() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        assert!(!engine.ignores_sizing_report(one));
        assert!(engine.ignores_sizing_report(two));
        {
            let _transition = SizingTransition::begin();
            assert!(engine.ignores_sizing_report(one));
            assert!(engine.ignores_sizing_report(two));
        }
        assert!(!engine.ignores_sizing_report(one));
        engine.set_fullscreen(one, true, false, &be());
        engine.focused_column_next_tab(&be());
        assert!(engine.ignores_sizing_report(one));
        assert!(!engine.ignores_sizing_report(two));
        assert!(engine.is_fullscreen(one) && engine.is_fullscreen(two));
    }

    #[test]
    fn leaving_tabbed_sizing_does_not_release_a_manually_maximized_column() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        engine
            .monitors
            .focused_mut()
            .unwrap()
            .workspace_mut()
            .unwrap()
            .columns[0]
            .toggle_maximized(1904);
        engine.toggle_maximize_window_to_edges_for(one, &be());
        engine.set_fullscreen(one, true, false, &be());
        assert!(engine.slot_forced.is_empty());
        engine.set_fullscreen(two, false, false, &be());
        engine.toggle_maximize_window_to_edges_for(two, &be());
        assert!(column_of(&engine, 1).maximized);
        assert_eq!(column_of(&engine, 1).width, Some(1904));
        assert!(engine.slot_forced.is_empty());
        engine
            .monitors
            .focused_mut()
            .unwrap()
            .workspace_mut()
            .unwrap()
            .columns[0]
            .toggle_maximized(1904);
        assert_eq!(column_of(&engine, 1).width, Some(640));
    }

    #[test]
    fn fullscreen_is_column_wide_across_tab_switches_and_exits_from_either_tab() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        let before = engine.locate_window(one);
        engine.set_fullscreen(one, true, false, &be());
        assert!(engine.is_fullscreen(one) && engine.is_fullscreen(two));
        assert!(!engine.is_fullscreen(WindowId::new(3)));
        assert_eq!(
            engine.column_sizing_mode(&column_of(&engine, 1)),
            DrawMode::Exact
        );
        assert_tab_drawing(&engine, 1, DrawMode::Exact);
        for selected in [2, 1, 2] {
            // Indicators are suppressed in this mode; switch through the keyboard path.
            engine.focused_column_next_tab(&be());
            assert_eq!(
                column_of(&engine, 1).active_window(),
                Some(WindowId::new(selected))
            );
            assert_tab_drawing(&engine, selected, DrawMode::Exact);
            assert!(engine.is_fullscreen(one) && engine.is_fullscreen(two));
            assert_eq!(column_of(&engine, 1).width, Some(1904));
        }
        assert_eq!(engine.locate_window(one), before);
        assert!(!engine.applied_state[&one].visible);
        engine.toggle_fullscreen_for(two, &be());
        assert!(!engine.is_fullscreen(one) && !engine.is_fullscreen(two));
        assert!(!column_of(&engine, 1).maximized);
        assert_eq!(column_of(&engine, 1).width, Some(640));
        assert!(engine.slot_forced.is_empty());
        assert!(
            !engine.applied_state[&one].visible,
            "exiting must not show the hidden tab"
        );
    }

    #[test]
    fn edges_is_column_wide_across_tab_switches_and_exits_from_either_tab() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        engine.toggle_maximize_window_to_edges_for(one, &be());
        assert!(engine.is_maximized_to_edges(one) && engine.is_maximized_to_edges(two));
        assert_eq!(
            engine.column_sizing_mode(&column_of(&engine, 1)),
            DrawMode::Edges
        );
        assert_tab_drawing(&engine, 1, DrawMode::Edges);
        for selected in [2, 1, 2] {
            // Indicators are suppressed in this mode; switch through the keyboard path.
            engine.focused_column_next_tab(&be());
            assert_eq!(
                column_of(&engine, 1).active_window(),
                Some(WindowId::new(selected))
            );
            assert_tab_drawing(&engine, selected, DrawMode::Edges);
        }
        engine.toggle_maximize_window_to_edges_for(two, &be());
        assert!(!engine.is_maximized_to_edges(one) && !engine.is_maximized_to_edges(two));
        assert_eq!(column_of(&engine, 1).width, Some(640));
        assert!(!column_of(&engine, 1).maximized);
        assert!(engine.slot_forced.is_empty());
        assert!(!engine.applied_state[&one].visible);
    }

    #[test]
    fn fullscreen_takes_precedence_without_losing_the_columns_edges_state() {
        let mut engine = with_two_tabs();
        let one = WindowId::new(1);
        let two = WindowId::new(2);
        engine.set_maximized_to_edges(one, true, &be());
        engine.set_fullscreen(two, true, false, &be());
        engine.focused_column_next_tab(&be());
        assert_eq!(column_of(&engine, 1).active_window(), Some(two));
        assert_tab_drawing(&engine, 2, DrawMode::Exact);
        assert_eq!(engine.window_sizing_mode(two), SizingMode::Fullscreen);
        engine.set_fullscreen(two, false, false, &be());
        assert!(engine.is_maximized_to_edges(one) && engine.is_maximized_to_edges(two));
        assert_eq!(engine.window_sizing_mode(two), SizingMode::Maximized);
        assert_tab_drawing(&engine, 2, DrawMode::Edges);
        assert!(column_of(&engine, 1).maximized);
        engine.set_maximized_to_edges(two, false, &be());
        assert_eq!(column_of(&engine, 1).width, Some(640));
        assert!(engine.slot_forced.is_empty());
    }

    #[test]
    fn a_fullscreen_window_keeps_its_column_and_the_others_stay_where_they_are() {
        let mut engine = with_three_windows();
        let two = WindowId::new(2);
        let before = engine.locate_window(two).unwrap();
        let width_before = column_of(&engine, 2).width;
        engine.toggle_fullscreen_for(two, &be());
        assert!(engine.is_fullscreen(two));
        assert_eq!(
            engine.locate_window(two),
            Some(before),
            "the same place in the strip"
        );
        // The column is as wide as the view for as long as it is fullscreen...
        let column = column_of(&engine, 2);
        assert!(column.maximized);
        assert_eq!(column.width, Some(1920 - 16));
        // ...and nobody else was touched: no other window is fullscreen, nothing was hidden.
        assert!(!engine.is_fullscreen(WindowId::new(1)));
        assert_eq!(engine.tiled_windows().len(), 3);
        for other in [1, 3] {
            let applied = engine.applied_state.get(&WindowId::new(other));
            assert!(
                applied.map_or(true, |a| a.visible),
                "window {} is not hidden",
                other
            );
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
        let at = |hwnd: isize| {
            drawn
                .iter()
                .find(|(w, _)| *w == WindowId::new(hwnd))
                .unwrap()
                .1
        };
        // The strip is not scrolled in this test: column 2 starts at column 1's end.
        let second = at(2);
        assert_eq!((second.size.w, second.size.h), (1920, 1080));
        assert_eq!(second.loc.y, 0);
        // The third column starts after the fullscreen one's whole slot, a gap beyond.
        assert!(
            at(3).loc.x >= second.loc.x + 1904,
            "the neighbour is out of the way"
        );
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
        assert!(
            column_of(&engine, 2).maximized,
            "still maximized to the edges"
        );
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
        assert!(
            engine.app_fullscreen.contains(&two),
            "its own doing: its frame is not touched"
        );
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
    fn minimizing_a_fullscreen_window_gives_the_column_back_and_restoring_makes_it_fullscreen_again(
    ) {
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
        engine.set_window_rules(vec![crate::config::WindowRule {
            match_groups: vec![crate::config::types::MatchGroup {
                conds: vec![crate::config::types::Matcher::TitleSearch(
                    crate::config::types::RegexPat::new("^Window 5").unwrap(),
                )],
            }],
            open_maximized_to_edges: Some(true),
            ..crate::config::WindowRule::default()
        }]);
        engine.add_window(window(5), &be());
        assert!(engine.is_maximized_to_edges(WindowId::new(5)));
        engine.add_window(window(6), &be());
        assert!(!engine.is_maximized_to_edges(WindowId::new(6)));
    }
}
