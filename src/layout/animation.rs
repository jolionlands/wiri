//! Event-driven animation timelines shared by scroll, workspace, move and resize.
mod timeline;
use crate::utils::WindowId;
use std::collections::HashMap;
#[cfg(test)]
use timeline::apply_easing;
pub use timeline::{Animation, AnimationKind, Easing, RectAnimation, Spring};

/// Type of animated property
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnimationTarget {
    /// Scroll offset X for a monitor's workspace
    ScrollX(OutputId),
    /// Window X position
    WindowX(WindowId),
    /// Window Y position
    WindowY(WindowId),
    /// Window width
    WindowW(WindowId),
    /// Window height
    WindowH(WindowId),
    /// Opacity for a window
    Opacity(WindowId),
    /// Per-monitor workspace X offset (workspace-transition slide).  The
    /// engine reads the current value via `AnimationManager::get_value(...)`
    /// while a workspace switch is in flight and adds it to the column
    /// positions for every window on the active workspace.
    ///
    /// Sign convention: positive offsets push the incoming workspace's
    /// columns from the right, negative offsets from the left, matching
    /// niri's `workspace-switch` animation direction.
    WorkspaceX(OutputId),
}

use crate::utils::OutputId;

/// Animation manager that tracks all active animations
pub struct AnimationManager {
    animations: HashMap<AnimationTarget, Animation>,
    enabled: bool,
    default_duration_ms: u32,
    default_easing: Easing,
    default_kind: AnimationKind,
    slowdown: f64,
    wake: Option<std::sync::Arc<tokio::sync::Notify>>,
}

impl AnimationManager {
    pub fn new(enabled: bool, duration_ms: u32, easing: Easing) -> Self {
        Self {
            animations: HashMap::new(),
            enabled,
            default_duration_ms: duration_ms,
            default_easing: easing,
            default_kind: AnimationKind::Easing {
                duration_ms,
                curve: easing,
            },
            slowdown: 1.0,
            wake: None,
        }
    }

    /// Create with animations disabled (instant transitions)
    pub fn disabled() -> Self {
        Self::new(false, 0, Easing::None)
    }

    /// Start or retarget from the currently displayed value, preserving spring velocity.
    pub fn animate(&mut self, target: AnimationTarget, start: f64, end: f64) {
        self.animate_with_kind(target, start, end, self.default_kind);
    }

    pub fn animate_with_kind(
        &mut self,
        target: AnimationTarget,
        start: f64,
        end: f64,
        kind: AnimationKind,
    ) {
        let kind = if self.enabled {
            kind
        } else {
            AnimationKind::Off
        };
        if let Some(anim) = self.animations.get_mut(&target) {
            anim.retarget(end, kind, self.slowdown);
        } else {
            self.animations.insert(
                target,
                Animation::with_kind(start, end, kind, self.slowdown),
            );
        }
        if let Some(wake) = &self.wake {
            wake.notify_one();
        }
    }

    /// Start a new animation with custom duration (legacy easing API).
    pub fn animate_with_duration(
        &mut self,
        target: AnimationTarget,
        start: f64,
        end: f64,
        duration_ms: u32,
    ) {
        self.animate_with_kind(
            target,
            start,
            end,
            AnimationKind::Easing {
                duration_ms,
                curve: self.default_easing,
            },
        );
    }

    pub fn set_notifier(&mut self, wake: std::sync::Arc<tokio::sync::Notify>) {
        self.wake = Some(wake);
    }
    pub fn set_kind(&mut self, kind: AnimationKind) {
        self.default_kind = kind;
    }
    pub fn set_slowdown(&mut self, slowdown: f64) {
        self.slowdown = slowdown;
    }
    pub fn cancel(&mut self, target: &AnimationTarget) {
        self.animations.remove(target);
    }

    /// Advance all animations by delta_ms and return values that changed
    pub fn tick(&mut self, delta_ms: u32) -> Vec<(AnimationTarget, f64)> {
        let mut results = Vec::new();
        let mut finished = Vec::new();

        for (target, anim) in &mut self.animations {
            let current = anim.tick(delta_ms);
            // Always deliver the terminal value, including an instant/off transition.
            results.push((target.clone(), current));
            if anim.is_finished() {
                finished.push(target.clone());
            }
        }

        // Clean up finished animations
        for target in &finished {
            self.animations.remove(target);
        }

        results
    }

    /// Check whether a tick is pending, including terminal delivery for instant/off
    /// transitions. Completed timelines remain active until tick delivers their value.
    pub fn has_active(&self) -> bool {
        !self.animations.is_empty()
    }

    /// Get the current value of an animation target, or None if not animating
    pub fn get_value(&self, target: &AnimationTarget) -> Option<f64> {
        self.animations.get(target).map(|a| a.current())
    }

    /// Cancel all animations
    pub fn cancel_all(&mut self) {
        self.animations.clear();
    }

    /// Start a workspace-switch slide on the given monitor.
    ///
    /// `from_offset_px` is the starting X offset (typically the width of the
    /// outgoing workspace, e.g. `+work_rect.width` when sliding in from the
    /// right or `-work_rect.width` when sliding in from the left).
    /// `to_offset_px` is the resting offset, almost always `0.0`.
    /// `duration_ms` overrides the manager's default duration when non-zero;
    /// pass `0` to use the manager default.
    ///
    /// When animations are disabled or the manager-default duration is also 0
    /// the animation completes instantly (no slide, layout jumps).
    ///
    /// Engine wiring: the consumer reads `get_value(&AnimationTarget::WorkspaceX(oid))`
    /// during layout calculations and adds the returned f64 to every column's
    /// X position on the active workspace.  See the "Request to Agent E" note
    /// in CHANGELOG.md.
    pub fn start_workspace_slide(
        &mut self,
        output: OutputId,
        from_offset_px: f64,
        to_offset_px: f64,
        duration_ms: u32,
    ) {
        let target = AnimationTarget::WorkspaceX(output);
        // A different incoming workspace starts at its own edge, not the old slide.
        self.cancel(&target);
        let kind = if duration_ms == 0 {
            self.default_kind
        } else {
            AnimationKind::Easing {
                duration_ms,
                curve: self.default_easing,
            }
        };
        self.animate_with_kind(target, from_offset_px, to_offset_px, kind);
    }

    /// Read the current workspace-slide offset for a monitor, in pixels.
    /// Returns `0.0` when no slide is in flight (i.e. the workspace is at rest).
    pub fn workspace_slide_offset(&self, output: OutputId) -> f64 {
        self.animations
            .get(&AnimationTarget::WorkspaceX(output))
            .map(|a| a.current())
            .unwrap_or(0.0)
    }

    /// Update animation settings
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Update the legacy easing duration without replacing a spring/off default.
    pub fn set_duration(&mut self, duration_ms: u32) {
        self.default_duration_ms = duration_ms;
        if let AnimationKind::Easing { curve, .. } = self.default_kind {
            self.default_kind = AnimationKind::Easing { duration_ms, curve };
        }
    }

    /// Update the legacy easing curve without replacing a spring/off default.
    pub fn set_easing(&mut self, easing: Easing) {
        self.default_easing = easing;
        if let AnimationKind::Easing { duration_ms, .. } = self.default_kind {
            self.default_kind = AnimationKind::Easing {
                duration_ms,
                curve: easing,
            };
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for AnimationManager {
    fn default() -> Self {
        Self::new(false, 200, Easing::CubicOut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_animation_linear() {
        let mut anim = Animation::new(0.0, 100.0, 100, Easing::Linear);
        assert_eq!(anim.tick(50), 50.0);
        assert_eq!(anim.tick(50), 100.0);
        assert!(anim.is_finished());
    }

    #[test]
    fn test_animation_ease_out() {
        let mut anim = Animation::new(0.0, 100.0, 100, Easing::EaseOut);
        let mid = anim.tick(50);
        // Ease-out: should be past 50% at the midpoint
        assert!(mid > 50.0 && mid < 100.0);
        anim.tick(50);
        assert!(anim.is_finished());
    }

    #[test]
    fn test_animation_instant() {
        let mut anim = Animation::instant(42.0);
        assert_eq!(anim.tick(0), 42.0);
        assert!(anim.is_finished());
    }

    #[test]
    fn test_animation_zero_duration() {
        let mut anim = Animation::new(0.0, 100.0, 0, Easing::Linear);
        assert_eq!(anim.tick(0), 100.0);
        assert!(anim.is_finished());
    }

    #[test]
    fn test_animation_clamp() {
        let mut anim = Animation::new(0.0, 100.0, 100, Easing::Linear);
        // Tick past the end — should clamp to end value
        assert_eq!(anim.tick(200), 100.0);
        assert!(anim.is_finished());
    }

    #[test]
    fn test_animation_manager() {
        let mut mgr = AnimationManager::new(true, 200, Easing::CubicOut);
        let target = AnimationTarget::ScrollX(OutputId::from_name("test"));
        mgr.animate(target.clone(), 0.0, 500.0);
        assert!(mgr.has_active());

        let results = mgr.tick(100);
        assert!(!results.is_empty());
        assert!(mgr.has_active());

        let results = mgr.tick(100);
        assert!(!results.is_empty());

        // Animation should be done after 200ms total
        assert!(!mgr.has_active());
    }

    #[test]
    fn test_animation_manager_disabled() {
        let mut mgr = AnimationManager::disabled();
        let target = AnimationTarget::ScrollX(OutputId::from_name("test"));
        mgr.animate(target.clone(), 0.0, 500.0);
        // Disabled transitions remain active until their terminal value is delivered.
        assert!(mgr.has_active());
        let results = mgr.tick(0);
        assert_eq!(results, vec![(target, 500.0)]);
        assert!(!mgr.has_active());
        assert!(mgr.tick(0).is_empty());
    }

    #[test]
    fn test_pending_terminal_values_are_delivered_by_active_loop() {
        let target = AnimationTarget::ScrollX(OutputId::from_name("pending-terminal"));
        for (start, kind) in [
            (0.0, AnimationKind::Off),
            (
                0.0,
                AnimationKind::Easing {
                    duration_ms: 0,
                    curve: Easing::Linear,
                },
            ),
            (
                0.0,
                AnimationKind::Easing {
                    duration_ms: 100,
                    curve: Easing::None,
                },
            ),
            (
                100.0,
                AnimationKind::Easing {
                    duration_ms: 100,
                    curve: Easing::Linear,
                },
            ),
        ] {
            let mut mgr = AnimationManager::new(true, 100, Easing::Linear);
            assert!(!mgr.has_active());
            mgr.animate_with_kind(target.clone(), start, 100.0, kind);

            let mut results = Vec::new();
            let mut ticks = 0;
            while mgr.has_active() {
                assert_eq!(ticks, 0, "terminal delivery should require only one tick");
                results.extend(mgr.tick(0));
                ticks += 1;
            }
            assert_eq!(ticks, 1);
            assert_eq!(results, vec![(target.clone(), 100.0)]);
            assert!(mgr.tick(0).is_empty());
        }
    }

    #[test]
    fn test_legacy_setters_update_default_timeline() {
        let mut mgr = AnimationManager::new(true, 200, Easing::Linear);
        let target = AnimationTarget::ScrollX(OutputId::from_name("legacy-settings"));

        mgr.set_duration(100);
        mgr.animate(target.clone(), 0.0, 100.0);
        assert_eq!(mgr.tick(50), vec![(target.clone(), 50.0)]);
        assert_eq!(mgr.tick(50), vec![(target.clone(), 100.0)]);

        mgr.set_easing(Easing::EaseIn);
        mgr.animate(target.clone(), 0.0, 100.0);
        assert_eq!(mgr.tick(50), vec![(target.clone(), 12.5)]);
        assert_eq!(mgr.tick(50), vec![(target.clone(), 100.0)]);

        mgr.set_duration(0);
        mgr.animate(target.clone(), 0.0, 100.0);
        assert!(mgr.has_active());
        assert_eq!(mgr.tick(0), vec![(target, 100.0)]);
        assert!(!mgr.has_active());
    }

    #[test]
    fn test_legacy_setters_preserve_other_kind_settings() {
        let mut mgr = AnimationManager::new(true, 200, Easing::Linear);
        mgr.set_kind(AnimationKind::Easing {
            duration_ms: 300,
            curve: Easing::QuadOut,
        });
        mgr.set_duration(100);
        assert_eq!(
            mgr.default_kind,
            AnimationKind::Easing {
                duration_ms: 100,
                curve: Easing::QuadOut
            }
        );

        mgr.set_kind(AnimationKind::Easing {
            duration_ms: 300,
            curve: Easing::QuadOut,
        });
        mgr.set_easing(Easing::EaseIn);
        assert_eq!(
            mgr.default_kind,
            AnimationKind::Easing {
                duration_ms: 300,
                curve: Easing::EaseIn
            }
        );

        for kind in [AnimationKind::Off, AnimationKind::Spring(Spring::default())] {
            mgr.set_kind(kind);
            mgr.set_duration(75);
            mgr.set_easing(Easing::Linear);
            assert_eq!(mgr.default_kind, kind);
        }
    }

    #[test]
    fn test_easing_functions() {
        // None should jump to 1.0
        assert_eq!(apply_easing(0.5, Easing::None), 1.0);
        // Linear should pass through
        assert_eq!(apply_easing(0.5, Easing::Linear), 0.5);
        // Ease-in should be < 0.5 at t=0.5
        assert!(apply_easing(0.5, Easing::EaseIn) < 0.5);
        // Ease-out should be > 0.5 at t=0.5
        assert!(apply_easing(0.5, Easing::EaseOut) > 0.5);
        // All should start at 0
        assert_eq!(apply_easing(0.0, Easing::Linear), 0.0);
        // All should end at 1
        assert_eq!(apply_easing(1.0, Easing::Linear), 1.0);
    }

    #[test]
    fn test_animation_reverse() {
        let mut anim = Animation::new(100.0, 0.0, 100, Easing::Linear);
        assert_eq!(anim.tick(50), 50.0);
        assert_eq!(anim.tick(50), 0.0);
    }

    #[test]
    fn test_animation_small_tick() {
        let mut anim = Animation::new(0.0, 1000.0, 10000, Easing::Linear);
        let val = anim.tick(1);
        assert_eq!(val, 0.1); // 1/10000 * 1000
    }

    #[test]
    fn test_animation_manager_replace() {
        let mut mgr = AnimationManager::new(true, 200, Easing::CubicOut);
        let target = AnimationTarget::ScrollX(OutputId::from_name("test"));
        mgr.animate(target.clone(), 0.0, 500.0);
        // Replace with new animation for same target
        mgr.animate(target.clone(), 500.0, 1000.0);
        // Should have only one animation (replaced)
        assert!(mgr.has_active());
    }

    #[test]
    fn test_animation_manager_multiple_targets() {
        let mut mgr = AnimationManager::new(true, 200, Easing::Linear);
        let t1 = AnimationTarget::ScrollX(OutputId::from_name("m1"));
        let t2 = AnimationTarget::ScrollX(OutputId::from_name("m2"));
        mgr.animate(t1, 0.0, 500.0);
        mgr.animate(t2, 0.0, 300.0);
        assert!(mgr.has_active());
        let results = mgr.tick(100);
        // Both should tick
        assert!(results.len() >= 2);
    }

    #[test]
    fn test_easing_cubic_out() {
        // CubicOut at t=0 should be 0
        assert_eq!(apply_easing(0.0, Easing::CubicOut), 0.0);
        // At t=1 should be 1
        let val = apply_easing(1.0, Easing::CubicOut);
        assert!((val - 1.0).abs() < 0.01);
        // At t=0.5 should be > 0.5
        assert!(apply_easing(0.5, Easing::CubicOut) > 0.5);
    }

    #[test]
    fn test_easing_ease_in_out() {
        // At midpoint should be 0.5
        let val = apply_easing(0.5, Easing::EaseInOut);
        assert!((val - 0.5).abs() < 0.01);
        // At start/end
        assert_eq!(apply_easing(0.0, Easing::EaseInOut), 0.0);
        let end = apply_easing(1.0, Easing::EaseInOut);
        assert!((end - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_animation_target_equality() {
        let t1 = AnimationTarget::ScrollX(OutputId::from_name("m1"));
        let t2 = AnimationTarget::ScrollX(OutputId::from_name("m1"));
        assert_eq!(t1, t2);
        let t3 = AnimationTarget::ScrollX(OutputId::from_name("m2"));
        assert_ne!(t1, t3);
    }

    // ── workspace-slide animation tests ───────────────────────────────────────

    #[test]
    fn test_start_workspace_slide_moves_from_offset_to_zero() {
        let mut mgr = AnimationManager::new(true, 200, Easing::Linear);
        let oid = OutputId::from_name("ws-slide-1");
        // Slide in from +1920 px (incoming workspace pushed off to the right).
        mgr.start_workspace_slide(oid, 1920.0, 0.0, 200);
        assert!(
            mgr.has_active(),
            "slide animation must be active immediately"
        );
        assert!(
            (mgr.workspace_slide_offset(oid) - 1920.0).abs() < f64::EPSILON,
            "offset at t=0 should be the starting offset"
        );

        // Halfway through (Linear) — should be exactly +960 px.
        mgr.tick(100);
        let mid = mgr.workspace_slide_offset(oid);
        assert!(
            (mid - 960.0).abs() < 1.0,
            "linear at t=0.5 expected ≈960, got {}",
            mid
        );

        // Finish out the animation.
        mgr.tick(200);
        assert!(!mgr.has_active(), "animation should be done");
        assert_eq!(
            mgr.workspace_slide_offset(oid),
            0.0,
            "post-animation offset returns 0 (no entry)"
        );
    }

    #[test]
    fn test_start_workspace_slide_negative_direction() {
        let mut mgr = AnimationManager::new(true, 100, Easing::Linear);
        let oid = OutputId::from_name("ws-slide-2");
        // Slide in from -1920 px (incoming workspace pushed off to the left).
        mgr.start_workspace_slide(oid, -1920.0, 0.0, 0); // 0 → use mgr default 100
        mgr.tick(50);
        let mid = mgr.workspace_slide_offset(oid);
        assert!(
            mid < 0.0 && mid > -1920.0,
            "halfway through a left-slide the offset is negative and between -1920 and 0, got {}",
            mid
        );
    }

    #[test]
    fn test_start_workspace_slide_when_disabled_is_instant() {
        let mut mgr = AnimationManager::disabled();
        let oid = OutputId::from_name("ws-slide-3");
        mgr.start_workspace_slide(oid, 1920.0, 0.0, 200);
        // Disabled manager → instant transition (no active animation after tick).
        let _ = mgr.tick(0);
        assert!(!mgr.has_active(), "disabled manager: slide is instant");
        assert_eq!(mgr.workspace_slide_offset(oid), 0.0);
    }

    #[test]
    fn test_workspace_slide_offset_with_no_active_returns_zero() {
        let mgr = AnimationManager::new(true, 200, Easing::Linear);
        let oid = OutputId::from_name("ws-slide-4");
        assert_eq!(mgr.workspace_slide_offset(oid), 0.0);
    }

    #[test]
    fn test_start_workspace_slide_zero_duration_is_instant() {
        let mut mgr = AnimationManager::new(true, 0, Easing::Linear);
        let oid = OutputId::from_name("ws-slide-5");
        // Both override and default = 0 → instant.
        mgr.start_workspace_slide(oid, 1000.0, 0.0, 0);
        let _ = mgr.tick(0);
        assert!(
            !mgr.has_active(),
            "zero-duration slide must complete instantly"
        );
    }
}
