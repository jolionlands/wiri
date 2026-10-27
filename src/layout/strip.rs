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

    /// niri `center-visible-columns`: centre the span of columns that are *fully* in view
    /// (a column hanging off either edge is not part of it) in the view.  The offset may
    /// become negative, exactly like the centring policies: even the first or the only
    /// column can be centred, and there is no wrap-around.  Returns true when the view moved.
    ///
    /// An empty workspace, a zero-width view and a strip in which no column is fully in view
    /// (every column is wider than the view, or the view sits in a gap) change nothing.
    pub fn center_visible_columns(&mut self, default_width: i32, gap: i32, view: i32) -> bool {
        if self.columns.is_empty() || view <= 0 {
            return false;
        }
        let left = self.scroll_offset.x;
        let right = left.saturating_add(view);
        let mut span: Option<(i32, i32)> = None;
        for (index, _) in self.columns.iter().enumerate() {
            let (start, width) = self.column_x_and_width(index, default_width, gap);
            let end = start.saturating_add(width);
            if start < left || end > right {
                continue;
            }
            span = Some(match span {
                Some((first, last)) => (first, end.max(last)),
                None => (start, end),
            });
        }
        let Some((start, end)) = span else {
            return false;
        };
        let width = (end - start).max(0);
        let target = start - (view - width) / 2;
        if target == self.scroll_offset.x {
            return false;
        }
        self.scroll_offset.x = target;
        true
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

    // --- center-visible-columns ---

    #[test]
    fn center_visible_columns_centres_the_span_that_is_fully_in_view() {
        let mut ws = strip();
        // Columns at 0..400, 420..820, 840..1240 of a 1000 px view: the last one hangs off
        // the right edge, so the span is the first two (0..820) and it gets centred.
        assert!(ws.center_visible_columns(400, 20, 1000));
        assert_eq!(ws.scroll_offset.x, -90);
        // Centring again moves nothing.
        assert!(!ws.center_visible_columns(400, 20, 1000));
        // Scrolled so that the middle and the last column are fully visible: their span
        // (420..1240) is centred in the 1000 px view.
        ws.scroll_offset.x = 240;
        assert!(ws.center_visible_columns(400, 20, 1000));
        assert_eq!(ws.scroll_offset.x, 330, "span 420..1240 centred in 1000");
    }

    #[test]
    fn center_visible_columns_handles_one_column_and_empty_strips() {
        let mut one = Workspace::new();
        one.add_window_to_new_column_with_width(WindowId::new(1), Some(400));
        assert!(one.center_visible_columns(400, 20, 1000));
        assert_eq!(one.scroll_offset.x, -300);
        let mut empty = Workspace::new();
        assert!(!empty.center_visible_columns(400, 20, 1000));
        assert_eq!(empty.scroll_offset.x, 0);
        // A view narrower than every column has nothing fully visible to centre.
        let mut wide = strip();
        wide.columns.iter_mut().for_each(|c| c.set_width(Some(900)));
        assert!(!wide.center_visible_columns(400, 20, 500));
        assert_eq!(wide.scroll_offset.x, 0);
    }
}
