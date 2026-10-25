//! Tab selection is focus, not just a different rectangle. Inactive tabs use the same
//! crash-safe hiding path as inactive workspaces and never enter the positioning batch.
use super::{BackendHandle, TilingEngine};
use crate::layout::workspace::ColumnDisplay;
use crate::utils::OutputId;

pub(super) fn reserve_indicator(
    mut rect: crate::utils::Rect,
    column: &crate::layout::Column,
    style: &crate::config::TabIndicatorStyle,
    zoom: f64,
) -> crate::utils::Rect {
    use crate::config::TabPosition;
    if !matches!(column.display, ColumnDisplay::Tabbed { .. })
        || style.off
        || !style.place_within_column
        || (style.hide_when_single_tab && column.tiles.len() < 2)
    {
        return rect;
    }
    let space = ((style.width + style.gap).max(0.0) * zoom).round() as u32;
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
