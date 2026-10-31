//! Native application bounds in visible physical pixels, before Wiri borders.
//!
//! Zero denotes an unconstrained axis. Freshness/backoff are Windows policies,
//! independent of pinned niri's constraint merging and fixed-height heuristic.

use super::rules::RuleLimits;
pub use crate::backend::NativeBounds;
use crate::backend::{NativeBoundsContext, NativeBoundsSample};
use crate::utils::WindowId;
use std::time::{Duration, Instant};

impl NativeBounds {
    /// Merge the native answer with the window's rule limits, before tile insets.
    pub(super) fn merge(self, rule: RuleLimits) -> RuleLimits {
        let maximum = |native: u32, rule: u32| match (native, rule) {
            (0, other) | (other, 0) => other,
            (native, rule) => native.min(rule),
        };
        RuleLimits {
            min_width: self.min_width.max(rule.min_width),
            min_height: self.min_height.max(rule.min_height),
            max_width: maximum(self.max_width, rule.max_width),
            max_height: maximum(self.max_height, rule.max_height),
        }
    }
}

pub(super) fn open_floating(limits: RuleLimits, explicit: Option<bool>, owned: bool) -> bool {
    explicit.unwrap_or(owned || (limits.min_height > 0 && limits.min_height == limits.max_height))
}

/// Fit ordinary floating placement without overriding a minimum larger than the
/// output. Oversized dimensions may cover the output; their position remains
/// bounded so moving/resizing cannot put the whole window beyond it.
pub(super) fn fit_float_rect(
    rect: crate::utils::Rect,
    work: crate::utils::Rect,
    limits: RuleLimits,
) -> crate::utils::Rect {
    let width = rect.size.w.min(work.size.w).max(limits.min_width).max(1);
    let height = rect.size.h.min(work.size.h).max(limits.min_height).max(1);
    let position = |pos: i32, origin: i32, available: u32, size: u32| {
        let delta = i64::from(available) - i64::from(size);
        i64::from(pos)
            .clamp(
                i64::from(origin) + delta.min(0),
                i64::from(origin) + delta.max(0),
            )
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    crate::utils::Rect::new(
        position(rect.loc.x, work.loc.x, work.size.w, width),
        position(rect.loc.y, work.loc.y, work.size.h, height),
        width,
        height,
    )
}

/// An opaque result capability for one managed admission and one query attempt.
#[derive(Debug, Clone, Copy)]
pub struct NativeBoundsQuery {
    pub window_id: WindowId,
    generation: u64,
    revision: u64,
    context: NativeBoundsContext,
}

#[derive(Debug)]
pub(super) struct NativeBoundsCache {
    generation: u64,
    revision: u64,
    context: Option<NativeBoundsContext>,
    pub bounds: NativeBounds,
    pub next_due: Instant,
    failures: u8,
    in_flight: bool,
}

impl NativeBoundsCache {
    const FRESH_FOR: Duration = Duration::from_secs(2);

    pub fn new(generation: u64, seed: Option<NativeBoundsSample>, now: Instant) -> Self {
        Self {
            generation,
            revision: 0,
            context: seed.map(|sample| sample.context),
            bounds: seed.map_or(NativeBounds::default(), |sample| sample.bounds),
            next_due: now
                + if seed.is_some() {
                    Self::FRESH_FOR
                } else {
                    Duration::from_secs(1)
                },
            failures: u8::from(seed.is_none()),
            in_flight: false,
        }
    }

    pub fn invalidate(&mut self, now: Instant) {
        self.revision = self.revision.wrapping_add(1);
        self.in_flight = false;
        self.next_due = now;
    }

    pub fn matches_context(&self, context: NativeBoundsContext) -> bool {
        self.context == Some(context)
    }

    pub fn begin(
        &mut self,
        window_id: WindowId,
        context: NativeBoundsContext,
        now: Instant,
    ) -> Option<NativeBoundsQuery> {
        if !self.matches_context(context) {
            self.invalidate(now);
            self.context = Some(context);
        }
        if self.in_flight || now < self.next_due {
            return None;
        }
        self.revision = self.revision.wrapping_add(1);
        self.in_flight = true;
        Some(NativeBoundsQuery {
            window_id,
            generation: self.generation,
            revision: self.revision,
            context,
        })
    }

    /// None rejects a stale/duplicate result. A failed query retains the last
    /// good value; a successful all-zero query replaces it like any other value.
    pub fn finish(
        &mut self,
        query: NativeBoundsQuery,
        answer: Option<NativeBoundsSample>,
        now: Instant,
    ) -> Option<bool> {
        if self.generation != query.generation || self.revision != query.revision || !self.in_flight
        {
            return None;
        }
        self.in_flight = false;
        if let Some(sample) = answer.filter(|sample| sample.context == query.context) {
            let changed = self.bounds != sample.bounds;
            self.bounds = sample.bounds;
            self.failures = 0;
            self.next_due = now + Self::FRESH_FOR;
            Some(changed)
        } else {
            self.failures = self.failures.saturating_add(1);
            self.next_due = now + Duration::from_secs(1 << self.failures.saturating_sub(1).min(3));
            Some(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_width_only_merge_preserves_rule_axes_and_conflicts() {
        let native = NativeBounds {
            min_width: 900,
            ..NativeBounds::default()
        };
        let rule = RuleLimits {
            min_width: 200,
            max_width: 300,
            min_height: 600,
            max_height: 200,
        };
        assert_eq!(
            native.merge(rule),
            RuleLimits {
                min_width: 900,
                max_width: 300,
                min_height: 600,
                max_height: 200,
            }
        );
        // Existing RuleLimits consumers already let a minimum win a conflict.
        assert_eq!(native.merge(rule).clamp(100, 100), (900, 600));
    }

    #[test]
    fn baseline_missing_or_smaller_native_minimum_keeps_rule_limits() {
        let rule = RuleLimits {
            min_width: 400,
            max_width: 700,
            min_height: 160,
            max_height: 500,
        };
        for min_width in [0, 200, 400] {
            assert_eq!(
                NativeBounds {
                    min_width,
                    ..NativeBounds::default()
                }
                .merge(rule),
                rule
            );
        }
    }

    // Pinned niri window/mod.rs apply_min_size/apply_max_size: minima combine
    // by max, nonzero maxima by min. Expectations below are hand-derived; they
    // exercise the same helper called by production tile-limit synchronization.
    #[test]
    fn native_bounds_parity_native_only_limits_both_axes() {
        let native = NativeBounds {
            min_width: 240,
            min_height: 180,
            max_width: 640,
            max_height: 360,
        };
        assert_eq!(
            native.merge(RuleLimits::default()),
            RuleLimits {
                min_width: 240,
                max_width: 640,
                min_height: 180,
                max_height: 360,
            }
        );
    }

    #[test]
    fn native_bounds_parity_combines_stricter_rule_and_native_axes() {
        let native = NativeBounds {
            min_width: 300,
            min_height: 260,
            max_width: 640,
            max_height: 320,
        };
        let rule = RuleLimits {
            min_width: 420,
            max_width: 900,
            min_height: 220,
            max_height: 280,
        };
        assert_eq!(
            native.merge(rule),
            RuleLimits {
                min_width: 420,
                max_width: 640,
                min_height: 260,
                max_height: 280,
            }
        );
    }

    #[test]
    fn native_bounds_parity_zero_maximum_leaves_the_other_source_effective() {
        let native = NativeBounds {
            max_width: 640,
            ..NativeBounds::default()
        };
        let rule = RuleLimits {
            max_height: 480,
            ..RuleLimits::default()
        };
        let merged = native.merge(rule);
        assert_eq!(merged.clamp(1000, 1000), (640, 480));
        assert_eq!(
            NativeBounds::default()
                .merge(RuleLimits::default())
                .clamp(1000, 1000),
            (1000, 1000)
        );
    }

    #[test]
    fn native_bounds_parity_native_minimum_wins_conflicting_rule_maximum() {
        let native = NativeBounds {
            min_width: 640,
            min_height: 420,
            ..NativeBounds::default()
        };
        let rule = RuleLimits {
            max_width: 500,
            max_height: 300,
            ..RuleLimits::default()
        };
        assert_eq!(native.merge(rule).clamp(100, 100), (640, 420));
    }

    #[test]
    fn native_bounds_parity_rule_minimum_wins_conflicting_native_maximum() {
        let native = NativeBounds {
            max_width: 320,
            max_height: 200,
            ..NativeBounds::default()
        };
        let rule = RuleLimits {
            min_width: 640,
            min_height: 400,
            ..RuleLimits::default()
        };
        assert_eq!(native.merge(rule).clamp(1000, 1000), (640, 400));
    }

    fn sample(bounds: NativeBounds) -> NativeBoundsSample {
        NativeBoundsSample {
            bounds,
            context: NativeBoundsContext {
                dpi: 96,
                process_id: 1000,
                thread_id: 7,
                ..NativeBoundsContext::default()
            },
        }
    }

    #[test]
    fn cache_success_expires_and_successful_zero_replaces_all_four_bounds() {
        let now = Instant::now();
        let initial = sample(NativeBounds {
            min_width: 400,
            min_height: 200,
            max_width: 800,
            max_height: 600,
        });
        let id = WindowId::new(0x7B00_0100);
        let mut cache = NativeBoundsCache::new(1, Some(initial), now);
        assert!(cache
            .begin(id, initial.context, now + Duration::from_millis(1999))
            .is_none());
        let due = now + Duration::from_secs(2);
        let query = cache.begin(id, initial.context, due).unwrap();
        assert!(
            cache.begin(id, initial.context, due).is_none(),
            "only one query in flight"
        );
        assert_eq!(
            cache.finish(query, Some(sample(NativeBounds::default())), due),
            Some(true)
        );
        assert_eq!(cache.bounds, NativeBounds::default());
        assert_eq!(
            cache.finish(query, Some(initial), due),
            None,
            "duplicate result cannot restore old bounds"
        );
        assert!(cache
            .begin(id, initial.context, due + Duration::from_millis(1999))
            .is_none());
        assert!(cache
            .begin(id, initial.context, due + Duration::from_secs(2))
            .is_some());
    }

    #[test]
    fn cache_timeouts_retain_last_good_and_retry_after_capped_backoff() {
        let initial = sample(NativeBounds {
            min_height: 240,
            max_width: 620,
            ..NativeBounds::default()
        });
        let id = WindowId::new(0x7B00_0101);
        let mut now = Instant::now();
        let mut cache = NativeBoundsCache::new(2, Some(initial), now);
        now += Duration::from_secs(2);
        for delay in [1, 2, 4, 8, 8, 8] {
            let query = cache.begin(id, initial.context, now).unwrap();
            assert_eq!(cache.finish(query, None, now), Some(false));
            assert_eq!(
                cache.bounds, initial.bounds,
                "failure must not clear native bounds"
            );
            assert!(cache
                .begin(
                    id,
                    initial.context,
                    now + Duration::from_millis(delay * 1000 - 1)
                )
                .is_none());
            now += Duration::from_secs(delay);
        }
        let query = cache.begin(id, initial.context, now).unwrap();
        assert_eq!(
            cache.finish(query, Some(sample(NativeBounds::default())), now),
            Some(true)
        );
        assert!(cache
            .begin(id, initial.context, now + Duration::from_millis(1999))
            .is_none());
        assert!(cache
            .begin(id, initial.context, now + Duration::from_secs(2))
            .is_some());
    }

    #[test]
    fn cache_rejects_context_changes_and_invalidation_results() {
        let now = Instant::now();
        let initial = sample(NativeBounds {
            min_width: 300,
            ..NativeBounds::default()
        });
        let id = WindowId::new(0x7B00_0102);
        for context in [
            NativeBoundsContext {
                dpi: 192,
                ..initial.context
            },
            NativeBoundsContext {
                style: 0x40000,
                ..initial.context
            },
            NativeBoundsContext {
                monitor: 2,
                ..initial.context
            },
        ] {
            let mut cache = NativeBoundsCache::new(3, Some(initial), now);
            cache.invalidate(now);
            let old = cache.begin(id, initial.context, now).unwrap();
            let current = cache.begin(id, context, now).unwrap();
            assert_eq!(
                cache.finish(old, Some(sample(NativeBounds::default())), now),
                None
            );
            assert_eq!(cache.bounds, initial.bounds);
            // An answer sampled under a different context is a failed attempt.
            assert_eq!(cache.finish(current, Some(initial), now), Some(false));
            assert_eq!(cache.bounds, initial.bounds);
            cache.invalidate(now);
            let retry = cache.begin(id, context, now).unwrap();
            let answer = NativeBoundsSample {
                context,
                bounds: NativeBounds::default(),
            };
            assert_eq!(cache.finish(retry, Some(answer), now), Some(true));
        }
    }

    #[test]
    fn cache_readmission_rejects_old_generation_without_consuming_current_query() {
        let now = Instant::now();
        let initial = sample(NativeBounds {
            min_width: 300,
            ..NativeBounds::default()
        });
        let id = WindowId::new(0x7B00_0103);
        let mut previous = NativeBoundsCache::new(4, Some(initial), now);
        previous.invalidate(now);
        let old = previous.begin(id, initial.context, now).unwrap();
        let mut readmitted = NativeBoundsCache::new(5, Some(initial), now);
        readmitted.invalidate(now);
        let current = readmitted.begin(id, initial.context, now).unwrap();
        assert_eq!(
            readmitted.finish(old, Some(sample(NativeBounds::default())), now),
            None
        );
        assert_eq!(readmitted.bounds, initial.bounds);
        assert_eq!(
            readmitted.finish(current, Some(sample(NativeBounds::default())), now),
            Some(true)
        );
    }

    #[test]
    fn opening_policy_requires_positive_effective_fixed_height_and_respects_explicit_choice() {
        let fixed = NativeBounds {
            min_height: 240,
            max_height: 240,
            ..NativeBounds::default()
        }
        .merge(RuleLimits::default());
        assert!(open_floating(fixed, None, false));
        assert!(!open_floating(fixed, Some(false), true));
        assert!(open_floating(RuleLimits::default(), Some(true), false));
        assert!(!open_floating(RuleLimits::default(), None, false));
        assert!(open_floating(RuleLimits::default(), None, true));
        let fixed_width = NativeBounds {
            min_width: 240,
            max_width: 240,
            ..NativeBounds::default()
        }
        .merge(RuleLimits::default());
        assert!(!open_floating(fixed_width, None, false));
        let from_both = NativeBounds {
            min_height: 240,
            ..NativeBounds::default()
        }
        .merge(RuleLimits {
            max_height: 240,
            ..RuleLimits::default()
        });
        assert!(open_floating(from_both, None, false));
    }

    fn engine_and_window(hwnd: isize) -> (super::super::TilingEngine, crate::backend::WindowInfo) {
        use crate::utils::{OutputId, Rect};
        let mut engine = super::super::TilingEngine::default();
        engine.register_monitor(
            OutputId::from_name("NativeBoundsTest"),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        let window = crate::backend::WindowInfo {
            hwnd,
            title: "native bounds fixture".into(),
            class_name: "TestClass".into(),
            process_id: 1000,
            bounds: Rect::new(100, 100, 800, 600),
            state: crate::backend::WindowState::Normal,
            is_visible: true,
        };
        (engine, window)
    }

    #[test]
    fn admission_seed_controls_first_floating_decision_and_first_tile_limits() {
        let backend = crate::backend::BackendHandle::default_for_test();
        for (index, explicit, should_float) in [(0, None, true), (1, Some(false), false)] {
            let (mut engine, window) = engine_and_window(0x7B00_0200 + index);
            let id = WindowId::new(window.hwnd);
            engine.set_window_rules(vec![crate::config::WindowRule {
                open_floating: explicit,
                ..crate::config::WindowRule::default()
            }]);
            let seed = sample(NativeBounds {
                min_width: 160,
                min_height: 240,
                max_width: 420,
                max_height: 240,
            });
            engine.add_window_with_native_bounds(window, &backend, false, Some(seed));
            assert_eq!(engine.is_floating(id), should_float);
            assert_eq!(
                engine.effective_window_limits(id).clamp(1000, 1000),
                (420, 240)
            );
            if should_float {
                assert_eq!(engine.float_rect_of(id).unwrap().size.h, 240);
            } else {
                let tile = engine
                    .monitors
                    .iter()
                    .flat_map(|(_, monitor)| monitor.workspaces.values())
                    .flat_map(|workspace| workspace.columns.iter())
                    .flat_map(|column| column.tiles.iter())
                    .find(|tile| tile.window_id == id)
                    .unwrap();
                let border = 2 * engine.config.border_inset() as u32;
                assert_eq!(
                    (tile.min_height, tile.max_height, tile.max_width),
                    (240 + border, 240 + border, 420 + border)
                );
            }
        }
    }

    #[test]
    fn engine_readmission_rejects_removed_windows_query() {
        let backend = crate::backend::BackendHandle::default_for_test();
        let (mut engine, window) = engine_and_window(0x7B00_0202);
        let id = WindowId::new(window.hwnd);
        let initial = sample(NativeBounds {
            min_width: 300,
            ..NativeBounds::default()
        });
        engine.add_window_with_native_bounds(window.clone(), &backend, false, Some(initial));
        engine.invalidate_native_bounds(id);
        let old = engine
            .native_bounds_query(id, initial.context, Instant::now())
            .unwrap();
        engine.remove_window(id, &backend);
        engine.add_window_with_native_bounds(window, &backend, false, Some(initial));
        engine.finish_native_bounds_query(
            old,
            Some(sample(NativeBounds::default())),
            Instant::now(),
            &backend,
        );
        assert_eq!(engine.effective_window_limits(id).min_width, 300);
    }

    #[test]
    fn engine_refresh_updates_all_tile_axes_and_clears_them_without_dpi_scaling() {
        use crate::utils::{OutputId, Rect};
        let backend = crate::backend::BackendHandle::default_for_test();
        let (mut engine, window) = engine_and_window(0x7B00_0203);
        let id = WindowId::new(window.hwnd);
        engine.sync_monitor_with_scale(
            OutputId::from_name("NativeBoundsTest"),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
            2.0,
        );
        let initial = NativeBoundsSample {
            context: NativeBoundsContext {
                dpi: 192,
                ..sample(NativeBounds::default()).context
            },
            bounds: NativeBounds::default(),
        };
        engine.add_window_with_native_bounds(window, &backend, false, Some(initial));
        let changed = NativeBoundsSample {
            context: initial.context,
            bounds: NativeBounds {
                min_width: 300,
                min_height: 200,
                max_width: 620,
                max_height: 400,
            },
        };
        let read_tile = |engine: &super::super::TilingEngine| {
            let tile = engine
                .monitors
                .iter()
                .flat_map(|(_, monitor)| monitor.workspaces.values())
                .flat_map(|workspace| workspace.columns.iter())
                .flat_map(|column| column.tiles.iter())
                .find(|tile| tile.window_id == id)
                .unwrap();
            (
                tile.min_width,
                tile.min_height,
                tile.max_width,
                tile.max_height,
            )
        };
        let border = 2 * engine.config.border_inset() as u32;
        engine.invalidate_native_bounds(id);
        let query = engine
            .native_bounds_query(id, initial.context, Instant::now())
            .unwrap();
        engine.finish_native_bounds_query(query, Some(changed), Instant::now(), &backend);
        assert_eq!(
            read_tile(&engine),
            (300 + border, 200 + border, 620 + border, 400 + border)
        );
        engine.invalidate_native_bounds(id);
        let query = engine
            .native_bounds_query(id, initial.context, Instant::now())
            .unwrap();
        engine.finish_native_bounds_query(query, Some(initial), Instant::now(), &backend);
        assert_eq!(read_tile(&engine), (0, 0, 0, 0));
    }

    #[test]
    fn native_bounds_review_opening_float_retains_minimum_larger_than_work_area() {
        let backend = crate::backend::BackendHandle::default_for_test();
        let (mut engine, window) = engine_and_window(0x7B00_0300);
        let id = WindowId::new(window.hwnd);
        engine.set_window_rules(vec![crate::config::WindowRule {
            open_floating: Some(true),
            ..crate::config::WindowRule::default()
        }]);
        let seed = sample(NativeBounds {
            min_width: 2400,
            min_height: 1400,
            ..NativeBounds::default()
        });
        engine.add_window_with_native_bounds(window, &backend, false, Some(seed));
        let rect = engine.float_rect_of(id).unwrap();
        assert_eq!((rect.size.w, rect.size.h), (2400, 1400));
    }

    #[test]
    fn native_bounds_review_refreshed_float_retains_minimum_larger_than_work_area() {
        let backend = crate::backend::BackendHandle::default_for_test();
        let (mut engine, window) = engine_and_window(0x7B00_0301);
        let id = WindowId::new(window.hwnd);
        engine.set_window_rules(vec![crate::config::WindowRule {
            open_floating: Some(true),
            ..crate::config::WindowRule::default()
        }]);
        let initial = sample(NativeBounds::default());
        engine.add_window_with_native_bounds(window, &backend, false, Some(initial));
        engine.invalidate_native_bounds(id);
        let query = engine
            .native_bounds_query(id, initial.context, Instant::now())
            .unwrap();
        let answer = sample(NativeBounds {
            min_width: 2400,
            min_height: 1400,
            ..NativeBounds::default()
        });
        engine.finish_native_bounds_query(query, Some(answer), Instant::now(), &backend);
        let rect = engine.float_rect_of(id).unwrap();
        assert_eq!((rect.size.w, rect.size.h), (2400, 1400));
    }
}
