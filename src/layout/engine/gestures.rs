//! Small camera adapter: gesture math lives in input::gestures, frames in Phase 2B.
use super::*;
use crate::input::gestures::GestureContext;
use crate::layout::animation::AnimationTarget;
use crate::layout::strip::CenterFocusedColumn;

/// Snap to either visible column boundary, or its center under the configured policy.
fn snapping_points(
    widths: &[i32],
    view: f64,
    gap: f64,
    policy: CenterFocusedColumn,
    center_single: bool,
) -> Vec<(f64, usize)> {
    if widths.is_empty() {
        return vec![];
    }
    let centered = policy == CenterFocusedColumn::Always || (center_single && widths.len() == 1);
    let mut points = Vec::new();
    let mut x = 0.0;
    let total = widths.iter().sum::<i32>() as f64 + gap * widths.len().saturating_sub(1) as f64;
    let left = 0.0;
    let right = total - view;
    for (i, &w) in widths.iter().enumerate() {
        let w = w as f64;
        let center = x - (view - w).max(0.0) / 2.0;
        if centered {
            points.push((center, i));
        } else {
            let overflow = |other: Option<&i32>| {
                policy == CenterFocusedColumn::OnOverflow
                    && other.is_some_and(|o| *o as f64 + w + gap > view)
            };
            let a = if overflow(widths.get(i + 1)) {
                center
            } else {
                x
            };
            let b = if overflow(i.checked_sub(1).and_then(|i| widths.get(i))) {
                center
            } else {
                x + w - view
            };
            // As upstream, the outer endpoints are retained even when the strip is
            // narrower than the view; interior snaps must be between them.
            if i == 0 {
                points.push((left, i));
            }
            if i + 1 == widths.len() {
                points.push((right, i));
            }
            for p in [a, b] {
                if left < p && p < right {
                    points.push((p, i));
                }
            }
        }
        x += w + gap;
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    points
}

impl TilingEngine {
    pub(crate) fn gesture_begin(&mut self, point: Point) -> Option<GestureContext> {
        let output = self
            .monitors
            .iter()
            .find(|(_, m)| m.bounds.contains_point(point))
            .map(|(o, _)| *o)
            .or(self.monitors.focused_id())?;
        self.refresh_strip_widths(output);
        self.monitors.set_focused(output);
        let cfg = self.effective_config(output).clone();
        let m = self.monitors.get(&output)?;
        let ws = m.workspace()?;
        let view = Self::view_width_for_monitor(m, &cfg).max(1) as f64;
        let (explicit, mins, maxs) = Self::column_inputs(ws);
        let widths = Self::column_widths_with_limits(
            &explicit,
            &mins,
            &maxs,
            view as i32,
            cfg.column_gap,
            &cfg,
        )
        .0;
        let workspaces = m.workspace_ids();
        let c = GestureContext {
            output,
            workspace: m.active_workspace,
            view: ws.scroll_offset.x as f64,
            width: view,
            height: m.work_area.size.h.max(1) as f64,
            snaps: snapping_points(
                &widths,
                view,
                cfg.column_gap as f64,
                cfg.center_focused_column,
                cfg.always_center_single_column,
            ),
            workspace_index: workspaces.iter().position(|id| *id == m.active_workspace)?,
            workspaces,
        };
        let ids: Vec<_> = ws
            .columns
            .iter()
            .flat_map(|c| c.tiles.iter().map(|t| t.window_id))
            .collect();
        for wid in ids {
            self.animating_rects.remove(&wid);
        }
        self.animation.cancel(&AnimationTarget::ScrollX(output));
        self.animation.cancel(&AnimationTarget::WorkspaceX(output));
        self.gesture_output = Some(output);
        self.gesture_workspace_offset = None;
        Some(c)
    }

    pub(crate) fn gesture_end_idle(&mut self) {
        self.gesture_output = None;
        self.gesture_workspace_offset = None;
    }

    fn gesture_apply(&mut self, output: OutputId, backend: &BackendHandle) {
        // Input events, not a timer, pace direct manipulation. Do not ease each HWND
        // behind the finger or let a preceding 16 ms configure swallow the final point.
        if let Some(ws) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspace_mut())
        {
            for col in &mut ws.columns {
                for tile in &mut col.tiles {
                    tile.configure_throttle.last_send = None;
                }
            }
        }
        let previous = self.frame_in_progress;
        self.frame_in_progress = true;
        self.apply_layout_for_monitor(output, backend);
        self.frame_in_progress = previous;
    }
    pub(crate) fn gesture_view_update(
        &mut self,
        output: OutputId,
        workspace: i32,
        value: f64,
        backend: &BackendHandle,
    ) {
        let Some(m) = self
            .monitors
            .get_mut(&output)
            .filter(|m| m.active_workspace == workspace)
        else {
            return;
        };
        if let Some(ws) = m.workspace_mut() {
            ws.scroll_offset.x = value.round() as i32;
        }
        self.gesture_apply(output, backend);
    }
    pub(crate) fn gesture_workspace_update(
        &mut self,
        output: OutputId,
        value: f64,
        backend: &BackendHandle,
    ) {
        if self.monitors.get(&output).is_none() {
            return;
        }
        self.gesture_workspace_offset = Some((output, value));
        self.gesture_apply(output, backend);
    }
    pub(crate) fn gesture_view_end(
        &mut self,
        output: OutputId,
        workspace: i32,
        current: f64,
        target: f64,
        column: usize,
        velocity: f64,
        backend: &BackendHandle,
    ) {
        self.gesture_output = None;
        let Some(m) = self
            .monitors
            .get_mut(&output)
            .filter(|m| m.active_workspace == workspace)
        else {
            return;
        };
        let id = m
            .workspace()
            .and_then(|w| w.columns.get(column))
            .and_then(|c| {
                c.active_window
                    .or_else(|| c.tiles.first().map(|t| t.window_id))
            });
        if let Some(id) = id {
            m.set_focus(id);
        }
        self.animation.animate_with_velocity(
            AnimationTarget::ScrollX(output),
            current,
            target,
            velocity,
            self.motion.view_kind(),
        );
        let value = self
            .animation
            .get_value(&AnimationTarget::ScrollX(output))
            .unwrap_or(target);
        if let Some(ws) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspace_mut())
        {
            ws.scroll_offset.x = value.round() as i32;
        }
        self.gesture_apply(output, backend);
        if let Some(id) = id {
            self.activate_now(id, backend);
        }
    }
    pub(crate) fn gesture_workspace_end(
        &mut self,
        output: OutputId,
        workspace: i32,
        offset: f64,
        velocity: f64,
        backend: &BackendHandle,
    ) {
        self.gesture_output = None;
        self.gesture_workspace_offset = None;
        if !self
            .monitors
            .get(&output)
            .is_some_and(|m| m.workspaces.contains_key(&workspace))
        {
            return;
        }
        self.monitors.set_focused(output);
        if self
            .monitors
            .get(&output)
            .is_some_and(|m| m.active_workspace != workspace)
        {
            self.switch_workspace(workspace, backend);
        }
        self.animation.cancel(&AnimationTarget::WorkspaceX(output));
        self.animation.animate_with_velocity(
            AnimationTarget::WorkspaceX(output),
            offset,
            0.0,
            velocity,
            self.motion.workspace_kind(),
        );
        self.gesture_apply(output, backend);
    }

    /// Positive deltas reveal later columns. DnD uses niri's wider, bounded end
    /// range, not a rubber band; a stationary held cursor is paced only while active.
    pub fn gesture_scroll_by(&mut self, dx: f64, backend: &BackendHandle) {
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        self.refresh_strip_widths(output);
        let cfg = self.effective_config(output).clone();
        let Some(m) = self.monitors.get_mut(&output) else {
            return;
        };
        let view = Self::view_width_for_monitor(m, &cfg);
        if let Some(ws) = m.workspace_mut() {
            let (explicit, mins, maxs) = Self::column_inputs(ws);
            let widths = Self::column_widths_with_limits(
                &explicit,
                &mins,
                &maxs,
                view,
                cfg.column_gap,
                &cfg,
            )
            .0;
            let total =
                widths.iter().sum::<i32>() + cfg.column_gap * widths.len().saturating_sub(1) as i32;
            ws.scroll_offset.x = (ws.scroll_offset.x as f64 + dx)
                .clamp(-(view as f64), total.max(0) as f64)
                .round() as i32;
        }
        self.animation.cancel(&AnimationTarget::ScrollX(output));
        self.gesture_apply(output, backend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snaps_use_real_column_widths_and_both_edges() {
        let p = snapping_points(
            &[300, 600, 400],
            800.0,
            20.0,
            CenterFocusedColumn::Never,
            false,
        );
        assert!(p.contains(&(0.0, 0)));
        assert!(p.contains(&(320.0, 1)));
        assert!(p.contains(&(120.0, 1)));
        assert!(p.contains(&(540.0, 2)));
        assert!(p.iter().all(|p| (0.0..=540.0).contains(&p.0)));
    }
    #[test]
    fn centered_and_narrow_strip_snaps_are_finite() {
        assert_eq!(
            snapping_points(&[300], 800.0, 20.0, CenterFocusedColumn::Never, true),
            vec![(-250.0, 0)]
        );
        assert_eq!(
            snapping_points(&[], 800.0, 20.0, CenterFocusedColumn::Always, false),
            vec![]
        );
        let p = snapping_points(&[300], 800.0, 20.0, CenterFocusedColumn::Never, false);
        assert_eq!(p, vec![(-500.0, 0), (0.0, 0)]);
    }
    #[test]
    fn gesture_camera_bypasses_rect_easing_and_hands_velocity_to_spring() {
        let mut e = TilingEngine::new(Default::default());
        let out = OutputId::from_name("gesture-camera");
        e.register_monitor(out, Rect::new(0, 0, 1200, 800), Rect::new(0, 0, 1200, 760));
        for i in 1..=4 {
            e.monitors
                .get_mut(&out)
                .unwrap()
                .add_window_with_width(WindowId::new(i), Some(500));
        }
        e.configure_motion(&crate::config::AnimationsConfig {
            enabled: true,
            ..Default::default()
        });
        let b = BackendHandle::default_for_test();
        let c = e.gesture_begin(Point::new(100, 100)).unwrap();
        e.gesture_view_update(out, c.workspace, 250.0, &b);
        assert_eq!(
            e.monitors
                .get(&out)
                .unwrap()
                .workspace()
                .unwrap()
                .scroll_offset
                .x,
            250
        );
        e.gesture_view_end(out, c.workspace, 250.0, 516.0, 1, 1000.0, &b);
        e.tick_animations_with_backend(1, &b);
        assert!(
            e.monitors
                .get(&out)
                .unwrap()
                .workspace()
                .unwrap()
                .scroll_offset
                .x
                > 250
        );
        e.tick_animations_with_backend(60_000, &b);
        assert_eq!(
            e.monitors
                .get(&out)
                .unwrap()
                .workspace()
                .unwrap()
                .scroll_offset
                .x,
            516
        );
        assert!(!e.has_active_animations());
    }
}
