//! Animation system for smooth scrolling and focus transitions.
//!
//! Uses tick-based interpolation with configurable duration and easing.
//! Animations are driven by the main event loop's periodic tick.

use std::collections::HashMap;
use crate::utils::WindowId;

/// Easing function type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    /// No animation — instant
    None,
    /// Linear interpolation
    Linear,
    /// Ease-in (slow start, fast end)
    EaseIn,
    /// Ease-out (fast start, slow end)
    EaseOut,
    /// Ease-in-out (slow start and end)
    EaseInOut,
    /// Cubic bezier approximation (ease-out-cubic)
    CubicOut,
}

impl Default for Easing {
    fn default() -> Self { Self::CubicOut }
}

/// A single animation that interpolates a value from start to end
#[derive(Debug, Clone)]
pub struct Animation {
    /// Start value
    pub start: f64,
    /// End value
    pub end: f64,
    /// Duration in milliseconds
    pub duration_ms: u32,
    /// Elapsed time in milliseconds
    pub elapsed_ms: u32,
    /// Easing function
    pub easing: Easing,
    /// Whether this animation is finished
    pub finished: bool,
}

impl Animation {
    pub fn new(start: f64, end: f64, duration_ms: u32, easing: Easing) -> Self {
        Self {
            start,
            end,
            duration_ms,
            elapsed_ms: 0,
            easing,
            finished: false,
        }
    }

    /// Create an instant (no-animation) transition
    pub fn instant(end: f64) -> Self {
        Self {
            start: end,
            end,
            duration_ms: 0,
            elapsed_ms: 0,
            easing: Easing::None,
            finished: true,
        }
    }

    /// Advance the animation by `delta_ms` milliseconds and return the current value
    pub fn tick(&mut self, delta_ms: u32) -> f64 {
        if self.finished || self.duration_ms == 0 {
            self.finished = true;
            return self.end;
        }

        self.elapsed_ms = (self.elapsed_ms + delta_ms).min(self.duration_ms);
        let t = self.elapsed_ms as f64 / self.duration_ms as f64;
        let eased_t = apply_easing(t, self.easing);

        if self.elapsed_ms >= self.duration_ms {
            self.finished = true;
            self.end
        } else {
            self.start + (self.end - self.start) * eased_t
        }
    }

    /// Get the current value without advancing
    pub fn current(&self) -> f64 {
        if self.finished || self.duration_ms == 0 {
            return self.end;
        }
        let t = self.elapsed_ms as f64 / self.duration_ms as f64;
        let eased_t = apply_easing(t, self.easing);
        self.start + (self.end - self.start) * eased_t
    }

    /// Check if this animation is complete
    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

/// Apply an easing function to a linear t value [0..1]
fn apply_easing(t: f64, easing: Easing) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::None => 1.0, // Jump to end immediately
        Easing::Linear => t,
        Easing::EaseIn => t * t * t,
        Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
        Easing::EaseInOut => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
            }
        }
        Easing::CubicOut => 1.0 - (1.0 - t).powi(3),
    }
}

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
}

use crate::utils::OutputId;

/// Animation manager that tracks all active animations
pub struct AnimationManager {
    animations: HashMap<AnimationTarget, Animation>,
    enabled: bool,
    default_duration_ms: u32,
    default_easing: Easing,
}

impl AnimationManager {
    pub fn new(enabled: bool, duration_ms: u32, easing: Easing) -> Self {
        Self {
            animations: HashMap::new(),
            enabled,
            default_duration_ms: duration_ms,
            default_easing: easing,
        }
    }

    /// Create with animations disabled (instant transitions)
    pub fn disabled() -> Self {
        Self::new(false, 0, Easing::None)
    }

    /// Start a new animation, replacing any existing one for this target
    pub fn animate(&mut self, target: AnimationTarget, start: f64, end: f64) {
        if !self.enabled || start == end {
            // No animation needed — store an instant transition
            self.animations.insert(target, Animation::instant(end));
            return;
        }
        self.animations.insert(target, Animation::new(
            start, end, self.default_duration_ms, self.default_easing,
        ));
    }

    /// Start a new animation with custom duration
    pub fn animate_with_duration(&mut self, target: AnimationTarget, start: f64, end: f64, duration_ms: u32) {
        if !self.enabled || start == end {
            self.animations.insert(target, Animation::instant(end));
            return;
        }
        self.animations.insert(target, Animation::new(
            start, end, duration_ms, self.default_easing,
        ));
    }

    /// Advance all animations by delta_ms and return values that changed
    pub fn tick(&mut self, delta_ms: u32) -> Vec<(AnimationTarget, f64)> {
        let mut results = Vec::new();
        let mut finished = Vec::new();

        for (target, anim) in &mut self.animations {
            let prev = anim.current();
            let current = anim.tick(delta_ms);
            if !anim.is_finished() || (prev - current).abs() > f64::EPSILON {
                results.push((target.clone(), current));
            }
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

    /// Check if any animations are active
    pub fn has_active(&self) -> bool {
        self.animations.values().any(|a| !a.finished)
    }

    /// Get the current value of an animation target, or None if not animating
    pub fn get_value(&self, target: &AnimationTarget) -> Option<f64> {
        self.animations.get(target).map(|a| a.current())
    }

    /// Cancel all animations
    pub fn cancel_all(&mut self) {
        self.animations.clear();
    }

    /// Update animation settings
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_duration(&mut self, duration_ms: u32) {
        self.default_duration_ms = duration_ms;
    }

    pub fn set_easing(&mut self, easing: Easing) {
        self.default_easing = easing;
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
        // Should be instant since disabled
        let results = mgr.tick(0);
        assert!(results.is_empty()); // instant transition already cleaned up
        assert!(!mgr.has_active());
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

}
