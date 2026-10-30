//! niri e19df12 floating.rs:137-158, 964-1025. Work areas and rectangles are
//! physical Win32 pixels; action values and visibility margins are logical.
use crate::ipc::PositionChange;
use crate::utils::{Rect, WindowId};

/// Cache-unacknowledged positions belong to this floating lifetime only. A live
/// HWND read and the throttled backend cache are separate observation streams:
/// reaching a requested native frame does not mean the cache has seen it yet.
/// A cache acknowledgement removes its request prefix. Native-loop completion
/// supersedes prior requests and rebases the next cache sample; legacy writes
/// already supply their authoritative cache sample. No past geometry is kept
/// after acknowledgement as a blacklist for subsequent external moves.
///
/// Windows policy: agreement between cache and HWND at the latest requested
/// frame completes all pending requests. Rectangles have no request serial, so
/// agreement at an early occurrence of a repeated frame is indistinguishable
/// from its final occurrence. Treat that agreement as a completion barrier;
/// when the streams differ, acknowledge only the earliest matching cache frame.
#[derive(Debug, Clone)]
pub(crate) struct PositionHistory {
    submitted: Rect,
    observed: Option<Rect>,
    pending: std::collections::VecDeque<Rect>,
    rebase_after_completion: bool,
}

impl PositionHistory {
    pub(crate) fn new(rect: Rect, observed: Option<Rect>) -> Self {
        Self {
            submitted: rect,
            observed,
            pending: [rect].into(),
            rebase_after_completion: false,
        }
    }

    pub(crate) fn submit(&mut self, rect: Rect) {
        if self.submitted != rect {
            self.pending.push_back(rect);
            self.submitted = rect;
        }
    }

    pub(crate) fn authoritative(&mut self, rect: Rect, observed: Option<Rect>) {
        self.submitted = rect;
        self.observed = observed;
        self.pending.clear();
        self.rebase_after_completion = false;
    }

    pub(crate) fn native_completed(&mut self, rect: Rect) {
        self.submitted = rect;
        self.pending.clear();
        // The final frame can reach the throttled cache after a subsequent
        // request. Retire it only when that cache actually acknowledges it.
        self.pending.push_back(rect);
        self.rebase_after_completion = true;
    }

    /// Capture the backend snapshot at the native completion boundary, without
    /// adopting its possibly throttled geometry. The real native-loop caller
    /// supplies this immediately; callers without a backend defer it to reconcile.
    pub(crate) fn completion_cache(&mut self, observed: Option<Rect>) {
        if !self.rebase_after_completion {
            return;
        }
        self.rebase_after_completion = false;
        self.observed = observed;
        if observed == Some(self.submitted) {
            self.pending.clear();
        }
    }

    /// Return genuinely new observed geometry, retaining newer queued requests.
    pub(crate) fn reconcile(
        &mut self,
        home: Rect,
        observed: Option<Rect>,
        native: Option<Rect>,
    ) -> Option<Rect> {
        if home != self.submitted {
            // Topology changes can also replace a home outside submit(). Native
            // loop completion signals explicitly, including an unchanged frame.
            self.native_completed(home);
        }
        if self.rebase_after_completion {
            self.completion_cache(observed);
            return None;
        }
        if observed == Some(home) && native == Some(home) {
            // Check before cache-change suppression: the HWND may have caught
            // up while the cached rectangle stayed unchanged. Native alone is
            // not enough; the cache can still deliver an older request later.
            self.observed = observed;
            self.pending.clear();
            return None;
        }
        if self.observed == observed {
            return None;
        }
        self.observed = observed;
        let observed = observed?;
        if let Some(index) = self.pending.iter().position(|rect| *rect == observed) {
            self.pending.drain(..=index);
            return None;
        }
        if native == Some(home) {
            // This changed cache sample still trails the authoritative HWND.
            // Keep request provenance until the cache catches up as well.
            return None;
        }
        self.authoritative(observed, Some(observed));
        Some(observed)
    }
}

pub fn blocked_by_move(id: Option<WindowId>, moving: Option<WindowId>) -> bool {
    moving.is_some_and(|moving| id.is_none() || id == Some(moving))
}

fn scale_or_one(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn axis(current: f64, available: f64, change: PositionChange) -> f64 {
    match change {
        PositionChange::SetFixed(value) => value,
        PositionChange::AdjustFixed(value) => current + value,
        PositionChange::SetProportion(value) => available * (value / 100.0).clamp(0.0, 10000.0),
        PositionChange::AdjustProportion(value) => {
            available * (current / available.max(1.0) + value / 100.0).clamp(0.0, 10000.0)
        }
    }
}

/// Stored tile positions are deliberately unclamped until the tile floats.
pub fn resolve(
    rect: Rect,
    work: Rect,
    scale: f64,
    x: PositionChange,
    y: PositionChange,
) -> Option<Rect> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let scale = scale_or_one(scale);
    let x = axis(
        (f64::from(rect.loc.x) - f64::from(work.loc.x)) / scale,
        f64::from(work.size.w) / scale,
        x,
    );
    let y = axis(
        (f64::from(rect.loc.y) - f64::from(work.loc.y)) / scale,
        f64::from(work.size.h) / scale,
        y,
    );
    Some(Rect::new(
        (f64::from(work.loc.x) + x * scale).round() as i32,
        (f64::from(work.loc.y) + y * scale).round() as i32,
        rect.size.w,
        rect.size.h,
    ))
}

/// Preserve at least 1/4 of each dimension, bounded to 10..75 logical pixels.
pub fn clamp_visible(rect: Rect, work: Rect, scale: f64) -> Rect {
    let scale = scale_or_one(scale);
    let bound = |pos: i32, origin: i32, size: u32, available: u32| {
        let size = f64::from(size);
        let visible = (size / scale / 4.0).clamp(10.0, 75.0) * scale;
        let offscreen = (size - visible).max(0.0);
        let relative = f64::from(pos) - f64::from(origin);
        // max then min matches niri even for a work area smaller than the margin.
        (f64::from(origin)
            + relative
                .max(-offscreen)
                .min(f64::from(available) - size + offscreen))
        .round() as i32
    };
    Rect::new(
        bound(rect.loc.x, work.loc.x, rect.size.w, work.size.w),
        bound(rect.loc.y, work.loc.y, rect.size.h, work.size.h),
        rect.size.w,
        rect.size.h,
    )
}

/// The same fraction of another work area. niri stores a float's position as a fraction
/// of the working area (`logical_to_size_frac_in_working_area`, floating.rs:126-135, with
/// the extent floored at 1) and scales it by the work area it is added to
/// (`scale_by_working_area`, floating.rs:115-124). The size is kept as it is: niri's
/// remembered `floating_window_size` is not scaled either (floating.rs:538-541).
pub fn rescale_position(rect: Rect, from: Rect, to: Rect) -> Rect {
    if from == to {
        return rect;
    }
    let fraction = |pos: i32, origin: i32, extent: u32| {
        (f64::from(pos) - f64::from(origin)) / f64::from(extent).max(1.0)
    };
    let place = |fraction: f64, origin: i32, extent: u32| {
        (f64::from(origin) + fraction * f64::from(extent)).round() as i32
    };
    Rect::new(
        place(
            fraction(rect.loc.x, from.loc.x, from.size.w),
            to.loc.x,
            to.size.w,
        ),
        place(
            fraction(rect.loc.y, from.loc.y, from.size.h),
            to.loc.y,
            to.size.h,
        ),
        rect.size.w,
        rect.size.h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use PositionChange::*;

    #[test]
    fn parity_rescale_position_keeps_the_work_area_fraction_and_the_size() {
        // niri floating.rs:115-135: position / working area size, then back by the new area.
        let from = Rect::new(0, 0, 1920, 1040);
        let to = Rect::new(1920, 40, 960, 520);
        let rect = Rect::new(480, 260, 700, 400);
        assert_eq!(
            rescale_position(rect, from, to),
            Rect::new(1920 + 240, 40 + 130, 700, 400)
        );
        // The same area is the identity, even for a position outside it.
        assert_eq!(rescale_position(Rect::new(-90, 5, 10, 10), from, from), Rect::new(-90, 5, 10, 10));
        // A degenerate source extent is floored at one pixel (floating.rs:133-134).
        let flat = Rect::new(0, 0, 0, 0);
        assert_eq!(
            rescale_position(Rect::new(0, 0, 5, 5), flat, to),
            Rect::new(1920, 40, 5, 5)
        );
    }

    #[test]
    fn parity_float_repeated_final_ack_allows_external_return_to_intermediate() {
        let initial = Rect::new(0, 0, 800, 600);
        let repeated = Rect::new(100, 200, 800, 600);
        let intermediate = Rect::new(300, 250, 650, 480);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.submit(repeated);
        history.submit(intermediate);
        history.submit(repeated);
        assert_eq!(
            history.reconcile(repeated, Some(repeated), Some(repeated)),
            None
        );
        assert_eq!(
            history.reconcile(repeated, Some(intermediate), Some(intermediate)),
            Some(intermediate),
            "after both streams acknowledge the final repeated frame, an external move to an intermediate frame must preserve its position and size"
        );
    }

    #[test]
    fn parity_float_repeated_cache_ack_with_different_native_retains_later_requests() {
        let initial = Rect::new(0, 0, 800, 600);
        let repeated = Rect::new(100, 200, 800, 600);
        let intermediate = Rect::new(300, 250, 650, 480);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.submit(repeated);
        history.submit(intermediate);
        history.submit(repeated);
        assert_eq!(
            history.reconcile(repeated, Some(repeated), Some(intermediate)),
            None
        );
        assert_eq!(
            history.pending,
            std::collections::VecDeque::from([intermediate, repeated])
        );
        assert_eq!(
            history.reconcile(repeated, Some(intermediate), Some(intermediate)),
            None,
            "a differing native sample cannot promote an ambiguous early cache acknowledgement to the final request"
        );
        assert_eq!(
            history.pending,
            std::collections::VecDeque::from([repeated])
        );
        assert_eq!(
            history.reconcile(repeated, Some(repeated), Some(repeated)),
            None
        );
        assert!(history.pending.is_empty());
    }

    #[test]
    fn parity_float_unchanged_cache_and_latest_native_complete_repeated_requests() {
        let initial = Rect::new(0, 0, 800, 600);
        let repeated = Rect::new(100, 200, 800, 600);
        let intermediate = Rect::new(300, 250, 650, 480);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.submit(repeated);
        history.submit(intermediate);
        history.submit(repeated);
        assert_eq!(
            history.reconcile(repeated, Some(repeated), Some(intermediate)),
            None
        );
        assert_eq!(
            history.reconcile(repeated, Some(repeated), Some(repeated)),
            None
        );
        assert!(
            history.pending.is_empty(),
            "native convergence is a completion barrier even without a new cache value"
        );
        assert_eq!(
            history.reconcile(repeated, Some(intermediate), Some(intermediate)),
            Some(intermediate)
        );
    }

    #[test]
    fn parity_float_native_final_with_trailing_cache_retains_late_ack_for_new_request() {
        let initial = Rect::new(0, 0, 800, 600);
        let repeated = Rect::new(100, 200, 800, 600);
        let intermediate = Rect::new(300, 250, 650, 480);
        let newer = Rect::new(150, 200, 800, 600);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.submit(repeated);
        history.submit(intermediate);
        history.submit(repeated);
        assert_eq!(
            history.reconcile(repeated, Some(intermediate), Some(repeated)),
            None
        );
        assert_eq!(
            history.pending,
            std::collections::VecDeque::from([repeated])
        );
        history.submit(newer);
        assert_eq!(
            history.reconcile(newer, Some(repeated), Some(repeated)),
            None,
            "both streams agreeing on an older request cannot complete or replace the newer request"
        );
        assert_eq!(history.pending, std::collections::VecDeque::from([newer]));
        assert_eq!(history.reconcile(newer, Some(newer), Some(newer)), None);
        assert!(history.pending.is_empty());
        assert_eq!(
            history.reconcile(newer, Some(repeated), Some(repeated)),
            Some(repeated)
        );
    }

    #[test]
    fn parity_float_cached_completed_frame_cannot_replace_subsequent_request() {
        let initial = Rect::new(0, 0, 800, 600);
        let repeated = Rect::new(100, 200, 800, 600);
        let intermediate = Rect::new(300, 250, 650, 480);
        let newer = Rect::new(150, 200, 800, 600);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.submit(repeated);
        history.submit(intermediate);
        history.submit(repeated);
        assert_eq!(
            history.reconcile(repeated, Some(repeated), Some(repeated)),
            None
        );
        history.submit(newer);
        assert_eq!(
            history.reconcile(newer, Some(repeated), Some(repeated)),
            None
        );
        assert_eq!(history.pending, std::collections::VecDeque::from([newer]));
        assert_eq!(history.reconcile(newer, Some(newer), Some(newer)), None);
        assert_eq!(
            history.reconcile(newer, Some(intermediate), Some(intermediate)),
            Some(intermediate),
            "completion must retire intermediate history even when another request follows immediately"
        );
    }

    #[test]
    fn parity_float_acknowledgements_retire_history_and_allow_later_external_return() {
        let initial = Rect::new(0, 0, 800, 600);
        let first = Rect::new(100, 200, 800, 600);
        let second = Rect::new(110, 200, 800, 600);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.submit(first);
        history.submit(second);
        assert_eq!(history.reconcile(second, Some(first), None), None);
        assert_eq!(history.pending.len(), 1);
        assert_eq!(history.reconcile(second, Some(second), None), None);
        assert!(history.pending.is_empty());
        assert_eq!(
            history.reconcile(second, Some(first), None),
            Some(first),
            "acknowledged geometry must not become a permanent external-move blacklist"
        );
    }

    #[test]
    fn parity_float_native_acknowledgement_consumes_throttled_cache_without_replacing_home() {
        let initial = Rect::new(0, 0, 800, 600);
        let first = Rect::new(100, 200, 800, 600);
        let second = Rect::new(110, 200, 800, 600);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.submit(first);
        history.submit(second);
        assert_eq!(history.reconcile(second, Some(first), Some(second)), None);
        history.submit(Rect::new(120, 200, 800, 600));
        assert_eq!(
            history.reconcile(Rect::new(120, 200, 800, 600), Some(first), None),
            None
        );
    }

    #[test]
    fn parity_float_delayed_cache_ack_after_native_ack_preserves_newer_request() {
        let initial = Rect::new(0, 0, 800, 600);
        let first = Rect::new(100, 200, 800, 600);
        let second = Rect::new(110, 200, 800, 600);
        let third = Rect::new(120, 200, 800, 600);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.submit(first);
        history.submit(second);
        assert_eq!(history.reconcile(second, Some(first), Some(second)), None);
        history.submit(third);
        assert_eq!(
            history.reconcile(third, Some(second), Some(second)),
            None,
            "the cache catching up to an already native-acknowledged request must not replace a newer request"
        );
        assert_eq!(history.reconcile(third, Some(third), Some(third)), None);
        assert_eq!(
            history.reconcile(third, Some(second), Some(second)),
            Some(second),
            "after cache acknowledgement, an external return to an older requested frame is a new move"
        );
    }

    #[test]
    fn parity_float_external_move_after_native_completion_boundary_is_observable() {
        let initial = Rect::new(0, 0, 800, 600);
        let final_frame = Rect::new(100, 200, 800, 600);
        let intermediate = Rect::new(300, 250, 650, 480);
        let external = Rect::new(400, 350, 700, 500);
        let mut history = PositionHistory::new(initial, Some(initial));
        history.native_completed(final_frame);
        history.completion_cache(Some(intermediate));
        assert_eq!(
            history.reconcile(final_frame, Some(external), None),
            Some(external),
            "a post-completion application observation must not be consumed as the completion snapshot"
        );
    }

    #[test]
    fn parity_positions_use_full_work_area_origin_and_logical_dpi() {
        let work = Rect::new(-1600, 120, 1600, 900);
        let rect = Rect::new(-1400, 320, 400, 200);
        for (x, y, expected) in [
            (SetFixed(100.0), SetFixed(40.0), (-1450, 180)),
            (SetProportion(50.0), SetProportion(25.0), (-800, 345)),
            (AdjustFixed(-10.0), AdjustFixed(20.0), (-1415, 350)),
            (
                AdjustProportion(10.0),
                AdjustProportion(-10.0),
                (-1240, 230),
            ),
            (SetFixed(100.0), AdjustFixed(0.0), (-1450, 320)),
        ] {
            let result = resolve(rect, work, 1.5, x, y).unwrap();
            assert_eq!((result.loc.x, result.loc.y), expected);
            assert_eq!(result.size, rect.size);
        }
    }

    #[test]
    fn parity_negative_proportion_stops_at_origin_but_fixed_can_cross_it() {
        let work = Rect::new(100, 200, 1000, 800);
        let rect = Rect::new(200, 300, 400, 300);
        let proportional = resolve(
            rect,
            work,
            1.0,
            AdjustProportion(-50.0),
            SetProportion(-10.0),
        )
        .unwrap();
        assert_eq!(proportional.loc, crate::utils::Point::new(100, 200));
        let fixed = resolve(rect, work, 1.0, AdjustFixed(-500.0), AdjustFixed(-500.0)).unwrap();
        assert_eq!(
            clamp_visible(fixed, work, 1.0).loc,
            crate::utils::Point::new(-225, -25)
        );
        let huge = resolve(
            rect,
            work,
            1.0,
            SetProportion(2_000_000.0),
            AdjustFixed(0.0),
        )
        .unwrap();
        assert_eq!(huge.loc.x, 10_000_100);
    }

    #[test]
    fn parity_partial_visibility_uses_logical_margins_without_resizing() {
        let work = Rect::new(100, 200, 1000, 800);
        for (size, scale, low, high) in [
            (400, 1.0, -225, 1025),
            (400, 2.0, -200, 1000),
            (20, 1.0, 90, 1090),
            (4, 1.0, 100, 1096),
        ] {
            let left = clamp_visible(Rect::new(-20000, 200, size, size), work, scale);
            let right = clamp_visible(Rect::new(20000, 200, size, size), work, scale);
            assert_eq!(left.loc.x, low);
            assert_eq!(right.loc.x, high);
            assert_eq!(left.size.w, size);
        }
    }

    #[test]
    fn parity_move_grab_only_blocks_implicit_or_same_window_target() {
        let moving = Some(WindowId::new(42));
        assert!(blocked_by_move(None, moving));
        assert!(blocked_by_move(Some(WindowId::new(42)), moving));
        assert!(!blocked_by_move(Some(WindowId::new(43)), moving));
        assert!(!blocked_by_move(None, None));
    }

    #[test]
    fn parity_nonfinite_position_cannot_be_applied() {
        let rect = Rect::new(0, 0, 400, 300);
        assert!(resolve(rect, rect, 1.0, SetFixed(f64::NAN), AdjustFixed(0.0)).is_none());
        assert!(resolve(
            rect,
            rect,
            1.0,
            AdjustFixed(0.0),
            AdjustProportion(f64::INFINITY)
        )
        .is_none());
    }
}
