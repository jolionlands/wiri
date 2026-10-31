//! Tab selection is focus, not just a different rectangle. Inactive tabs use the same
//! crash-safe hiding path as inactive workspaces and never enter the positioning batch.
use super::{BackendHandle, DrawMode, TilingEngine};
use crate::layout::workspace::ColumnDisplay;
use crate::utils::{OutputId, WindowId};

/// The pixels a placed tab indicator takes from its column (niri `tab_indicator.rs:323-335`,
/// `extra_size`): the logical width and the gap, each rounded through the output scale on its
/// own, then their sum, never below zero. `zoom` shrinks the strip in the overview, where the
/// same physical pixels are scaled down again.
pub(super) fn indicator_space(
    style: &crate::config::TabIndicatorStyle,
    zoom: f64,
    scale: f64,
) -> u32 {
    let factor = zoom * crate::layout::metrics::valid_scale(scale);
    ((style.width * factor).round() + (style.gap * factor).round()).max(0.0) as u32
}

pub(super) fn reserve_indicator(
    mut rect: crate::utils::Rect,
    column: &crate::layout::Column,
    style: &crate::config::TabIndicatorStyle,
    zoom: f64,
    scale: f64,
    sizing: DrawMode,
) -> crate::utils::Rect {
    use crate::config::TabPosition;
    if !matches!(column.display, ColumnDisplay::Tabbed { .. })
        || sizing != DrawMode::Inset
        || style.off
        || !style.place_within_column
        || (style.hide_when_single_tab && column.tiles.len() < 2)
    {
        return rect;
    }
    // A negative gap can cancel all of the reserved space, but never add it.
    let space = indicator_space(style, zoom, scale);
    match style.position {
        TabPosition::Left => {
            let n = space.min(rect.size.w.saturating_sub(1));
            rect.loc.x += n as i32;
            rect.size.w -= n;
        }
        TabPosition::Right => rect.size.w = rect.size.w.saturating_sub(space).max(1),
        TabPosition::Top => {
            let n = space.min(rect.size.h.saturating_sub(1));
            rect.loc.y += n as i32;
            rect.size.h -= n;
        }
        TabPosition::Bottom => rect.size.h = rect.size.h.saturating_sub(space).max(1),
    }
    rect
}

impl TilingEngine {
    pub(super) fn hide_inactive_tabs_for_monitor(
        &mut self,
        output: OutputId,
        backend: &BackendHandle,
    ) {
        let mut hidden = Vec::new();
        if let Some(monitor) = self.monitors.get_mut(&output) {
            let focus = monitor.focus_window;
            if let Some(ws) = monitor.workspace_mut() {
                for col in &mut ws.columns {
                    if let Some(wid) = focus {
                        col.select_window(wid);
                    }
                    col.clamp_active_tab();
                    if let ColumnDisplay::Tabbed { active_tab } = col.display {
                        hidden.extend(
                            col.tiles
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| *i != active_tab)
                                .map(|(_, t)| t.window_id),
                        );
                    }
                }
            }
        }
        for wid in hidden {
            if self.applied_state.get(&wid).map_or(true, |s| s.visible) {
                let _ = backend.show_window(wid.as_isize(), false);
                self.note_hidden(wid);
            }
        }
    }

    pub fn toggle_tabbed_for_focused_column(&mut self, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        if let Some(monitor) = self.monitors.get_mut(&output) {
            let focus = monitor.focus_window;
            let index = monitor.focus_column;
            if let Some(column) = monitor
                .workspace_mut()
                .and_then(|ws| ws.columns.get_mut(index?))
            {
                match column.display {
                    ColumnDisplay::Tabbed { .. } => column.switch_to_stacked(),
                    ColumnDisplay::Stacked => {
                        column.switch_to_tabbed();
                        if let Some(wid) = focus {
                            column.select_window(wid);
                        }
                    }
                }
            }
        }
        self.apply_layout_for_monitor(output, backend);
    }

    /// A decor click is delivered on the existing session UI thread, after its input
    /// handler has released the native overlay's state. Revalidate here: layout, reload,
    /// workspace changes and window destruction can all overtake a queued click.
    pub fn select_tab_window(&mut self, target: WindowId, backend: &BackendHandle) -> bool {
        if self.is_overview() || self.effective_zoom() < 1.0 {
            return false;
        }
        let Some((output, workspace, column)) = self.locate_window(target) else {
            return false;
        };
        let Some(monitor) = self.monitors.get(&output) else {
            return false;
        };
        if monitor.active_workspace != workspace {
            return false;
        }
        let Some(col) = monitor.workspace().and_then(|ws| ws.columns.get(column)) else {
            return false;
        };
        let style = &self.effective_config(output).effects.tab_indicator;
        if !matches!(col.display, ColumnDisplay::Tabbed { .. })
            || style.off
            || (style.hide_when_single_tab && col.tiles.len() < 2)
            || self.column_sizing_mode(col) != DrawMode::Inset
        {
            return false;
        }
        self.monitors.set_focused(output);
        if let Some(monitor) = self.monitors.get_mut(&output) {
            monitor.set_focus(target);
        }
        self.reveal_focused_column(output);
        // The layout must unhide the selected HWND before activation: Windows refuses
        // activation of a hidden tab. It also updates the old/new indicators atomically.
        self.apply_layout_for_monitor(output, backend);
        // This is an explicit user click, even when the model selection has not
        // changed: an unmanaged foreground HWND does not invalidate last_activated.
        self.activate_now(target, backend);
        true
    }

    fn cycle_tab(&mut self, forward: bool, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let selected = self.monitors.get_mut(&output).and_then(|monitor| {
            let index = monitor.focus_column?;
            let column = monitor.workspace_mut()?.columns.get_mut(index)?;
            if !matches!(column.display, ColumnDisplay::Tabbed { .. }) {
                return None;
            }
            if forward {
                column.next_tab();
            } else {
                column.prev_tab();
            }
            let wid = column.active_window()?;
            monitor.set_focus(wid);
            Some(wid)
        });
        // Show the selected tab before activation; Windows rejects activation of hidden HWNDs.
        self.apply_layout_for_monitor(output, backend);
        if let Some(wid) = selected {
            self.activate_now(wid, backend);
        }
    }

    pub fn focused_column_next_tab(&mut self, backend: &BackendHandle) {
        self.cycle_tab(true, backend);
    }

    pub fn focused_column_prev_tab(&mut self, backend: &BackendHandle) {
        self.cycle_tab(false, backend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Column, Monitor, Tile};
    use crate::utils::{Rect, WindowId};

    fn wid(n: isize) -> WindowId {
        WindowId::new(n)
    }
    fn column() -> Column {
        let mut c = Column::new();
        for n in 1..=3 {
            c.add_tile(Tile::new(wid(n)));
        }
        c.switch_to_tabbed();
        c
    }
    fn monitor() -> Monitor {
        let mut m = Monitor::new(
            OutputId::from_name("tabs-test"),
            Rect::new(0, 0, 1366, 768),
            Rect::new(0, 0, 1366, 728),
        );
        m.workspace_mut().unwrap().columns.push(column());
        m.set_focus(wid(2));
        m
    }

    #[test]
    fn indicator_reservation_follows_each_side_without_moving_the_column_slot() {
        use crate::config::{TabIndicatorStyle, TabPosition};
        let col = column();
        let area = Rect::new(100, 200, 400, 600);
        let mut style = TabIndicatorStyle {
            width: 7.0,
            gap: 3.0,
            place_within_column: true,
            ..TabIndicatorStyle::default()
        };
        for (position, expected) in [
            (TabPosition::Left, Rect::new(110, 200, 390, 600)),
            (TabPosition::Right, Rect::new(100, 200, 390, 600)),
            (TabPosition::Top, Rect::new(100, 210, 400, 590)),
            (TabPosition::Bottom, Rect::new(100, 200, 400, 590)),
        ] {
            style.position = position;
            assert_eq!(
                reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
                expected
            );
        }
    }

    #[test]
    fn fullscreen_and_edges_remove_indicator_reservation_but_maximize_column_does_not() {
        let mut col = column();
        let mut style = crate::config::TabIndicatorStyle {
            place_within_column: true,
            ..crate::config::TabIndicatorStyle::default()
        };
        let area = Rect::new(100, 200, 400, 600);
        for mode in [DrawMode::Exact, DrawMode::Edges] {
            assert_eq!(reserve_indicator(area, &col, &style, 1.0, 1.0, mode), area);
        }
        // maximize-column changes the slot width, not its DrawMode.
        col.maximized = true;
        assert_eq!(
            reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
            Rect::new(109, 200, 391, 600)
        );
        style.off = true;
        assert_eq!(
            reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
            area
        );
        style.off = false;
        style.place_within_column = false;
        assert_eq!(
            reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
            area
        );
    }

    #[test]
    fn indicator_space_rounds_dimensions_separately_and_never_expands_a_tile() {
        let mut col = column();
        let mut style = crate::config::TabIndicatorStyle {
            place_within_column: true,
            width: 4.6,
            gap: -0.6,
            ..crate::config::TabIndicatorStyle::default()
        };
        let area = Rect::new(0, 0, 400, 600);
        assert_eq!(
            reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
            Rect::new(4, 0, 396, 600)
        );
        style.gap = -65535.0;
        assert_eq!(
            reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
            area
        );
        style.gap = 5.0;
        style.width = 0.0;
        style.length = 0.0;
        // Rendering has a one-physical-pixel minimum; reservation intentionally uses raw width.
        assert_eq!(
            reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
            Rect::new(5, 0, 395, 600)
        );
        style.hide_when_single_tab = true;
        col.tiles.truncate(1);
        assert_eq!(
            reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
            area
        );
        style.hide_when_single_tab = false;
        col.display = ColumnDisplay::Stacked;
        assert_eq!(
            reserve_indicator(area, &col, &style, 1.0, 1.0, DrawMode::Inset),
            area
        );
    }

    #[test]
    fn indicator_reservation_projects_each_dimension_through_the_output_scale() {
        // niri tab_indicator.rs:323-335 rounds the width and the gap through the output
        // scale, each on its own, and reserves their sum.
        let col = column();
        let style = crate::config::TabIndicatorStyle {
            place_within_column: true,
            width: 7.0,
            gap: 3.0,
            ..crate::config::TabIndicatorStyle::default()
        };
        let area = Rect::new(0, 0, 400, 600);
        for (scale, reserve) in [(1.0, 10), (1.25, 13), (1.5, 16), (2.0, 20)] {
            assert_eq!(
                reserve_indicator(area, &col, &style, 1.0, scale, DrawMode::Inset),
                Rect::new(reserve, 0, 400 - reserve as u32, 600),
                "width 7 and gap 3 at scale {scale}"
            );
        }
        // The overview zoom still shrinks what is reserved for the strip it is drawn on.
        assert_eq!(
            reserve_indicator(area, &col, &style, 0.5, 1.0, DrawMode::Inset),
            Rect::new(6, 0, 394, 600)
        );
    }

    #[test]
    fn focus_and_visible_tab_move_together_without_wrapping() {
        let mut m = monitor();
        m.focus_down();
        assert_eq!(m.focus_window, Some(wid(3)));
        assert_eq!(
            m.workspace().unwrap().columns[0].active_window(),
            Some(wid(3))
        );
        m.focus_down();
        assert_eq!(m.focus_window, Some(wid(3)));
        m.focus_up();
        assert_eq!(
            m.workspace().unwrap().columns[0].active_window(),
            Some(wid(2))
        );
    }

    #[test]
    fn removing_tab_before_selection_preserves_window() {
        let mut c = column();
        c.select_window(wid(3));
        c.remove_tile(wid(1));
        assert_eq!(c.active_window(), Some(wid(3)));
        c.remove_tile(wid(3));
        assert_eq!(c.active_window(), Some(wid(2)));
        c.remove_tile(wid(2));
        assert_eq!(c.active_window(), None);
    }

    #[test]
    fn horizontal_focus_returns_to_the_active_tab() {
        let mut m = monitor();
        let mut other = Column::new();
        other.add_tile(Tile::new(wid(4)));
        m.workspace_mut().unwrap().columns.push(other);
        m.focus_right();
        assert_eq!(m.focus_window, Some(wid(4)));
        m.focus_left();
        assert_eq!(m.focus_window, Some(wid(2)));
    }

    #[test]
    fn toggle_preserves_focus_and_hides_only_other_tabs() {
        let mut e = TilingEngine::new(Default::default());
        let mut m = monitor();
        m.workspace_mut().unwrap().columns[0].switch_to_stacked();
        e.monitors.insert(OutputId::from_name("tabs-test"), m);
        e.monitors.set_focused(OutputId::from_name("tabs-test"));
        let backend = BackendHandle::default_for_test();
        e.toggle_tabbed_for_focused_column(&backend);
        assert_eq!(
            e.monitors
                .get(&OutputId::from_name("tabs-test"))
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .active_window(),
            Some(wid(2))
        );
        assert!(!e.applied_state[&wid(1)].visible);
        assert!(!e.applied_state[&wid(3)].visible);
        e.focused_column_next_tab(&backend);
        assert_eq!(
            e.monitors
                .get(&OutputId::from_name("tabs-test"))
                .unwrap()
                .focus_window,
            Some(wid(3))
        );
        e.focused_column_next_tab(&backend);
        assert_eq!(
            e.monitors
                .get(&OutputId::from_name("tabs-test"))
                .unwrap()
                .focus_window,
            Some(wid(1))
        );
        e.focused_column_prev_tab(&backend);
        assert_eq!(
            e.monitors
                .get(&OutputId::from_name("tabs-test"))
                .unwrap()
                .focus_window,
            Some(wid(3))
        );
    }
}
