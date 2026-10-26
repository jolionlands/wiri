//! Pure viewport policy. No frame clock: focus changes alone reveal columns.
use super::workspace::Workspace;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CenterFocusedColumn {
    #[default]
    Never,
    Always,
    OnOverflow,
}

impl Workspace {
    pub fn reveal_with_policy(
        &mut self,
        column: usize,
        default_width: i32,
        gap: i32,
        view: i32,
        policy: CenterFocusedColumn,
        center_single: bool,
    ) -> bool {
        if column >= self.columns.len() || view <= 0 {
            return false;
        }
        let (start, width) = self.column_x_and_width(column, default_width, gap);
        let before = self.scroll_offset.x;
        let overflows = start < before || start + width > before + view;
        let center = (center_single && self.columns.len() == 1)
            || policy == CenterFocusedColumn::Always
            || (policy == CenterFocusedColumn::OnOverflow && overflows);
        if center {
            // Negative offsets deliberately leave leading space: even the first
            // or only column can be centered.
            self.scroll_offset.x = start - (view - width) / 2;
        } else if policy == CenterFocusedColumn::Never {
            self.reveal_column(column, default_width, gap, view);
            self.fit_scroll(default_width, gap, view);
        }
        before != self.scroll_offset.x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::WindowId;

    fn strip() -> Workspace {
        let mut ws = Workspace::new();
        for i in 1..=3 {
            ws.add_window_to_new_column_with_width(WindowId::new(i), Some(400));
        }
        ws
    }

    #[test]
    fn never_only_scrolls_as_far_as_needed() {
        let mut ws = strip();
        assert!(!ws.reveal_with_policy(1, 400, 20, 1000, CenterFocusedColumn::Never, false));
        assert!(ws.reveal_with_policy(2, 400, 20, 1000, CenterFocusedColumn::Never, false));
        assert_eq!(ws.scroll_offset.x, 240);
        ws.reveal_with_policy(0, 400, 20, 1000, CenterFocusedColumn::Never, false);
        assert_eq!(ws.scroll_offset.x, 0);
    }

    #[test]
    fn always_centers_first_middle_and_last() {
        let mut ws = strip();
        for (col, offset) in [(0, -300), (1, 120), (2, 540)] {
            ws.reveal_with_policy(col, 400, 20, 1000, CenterFocusedColumn::Always, false);
            assert_eq!(ws.scroll_offset.x, offset);
        }
    }

    #[test]
    fn on_overflow_does_not_move_an_already_visible_column() {
        let mut ws = strip();
        ws.reveal_with_policy(1, 400, 20, 1000, CenterFocusedColumn::OnOverflow, false);
        assert_eq!(ws.scroll_offset.x, 0);
        ws.reveal_with_policy(2, 400, 20, 1000, CenterFocusedColumn::OnOverflow, false);
        assert_eq!(ws.scroll_offset.x, 540);
        assert!(!ws.reveal_with_policy(2, 400, 20, 1000, CenterFocusedColumn::OnOverflow, false));
    }

    #[test]
    fn single_column_can_center_in_never_mode() {
        let mut ws = Workspace::new();
        ws.add_window_to_new_column_with_width(WindowId::new(1), Some(400));
        ws.reveal_with_policy(0, 400, 20, 1000, CenterFocusedColumn::Never, true);
        assert_eq!(ws.scroll_offset.x, -300);
        ws.reveal_with_policy(0, 400, 20, 1000, CenterFocusedColumn::Never, false);
        assert_eq!(ws.scroll_offset.x, 0);
        ws.columns.clear();
        assert!(!ws.reveal_with_policy(0, 400, 20, 1000, CenterFocusedColumn::Always, true));
    }
}
