//! Deterministic scalar timelines. No clocks, tasks, HWNDs or frame-rate assumptions.
use crate::utils::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Easing {
    None,
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    #[default]
    CubicOut,
    QuadOut,
    ExpoOut,
    CubicBezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
}

impl Easing {
    /// The eased position of `t` (0 to 1); Bezier y controls may overshoot. The layout timeline uses
    /// it for its own animations; the overlay's recent-windows fade reuses it so that the
    /// configured curve means the same thing everywhere.
    pub fn apply(self, t: f64) -> f64 {
        apply_easing(t, self)
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.to_ascii_lowercase().replace('_', "-").as_str() {
            "none" | "off" => Self::None,
            "linear" => Self::Linear,
            "ease-in" | "ease-in-cubic" => Self::EaseIn,
            "ease-out" | "ease-out-cubic" | "cubic-out" | "cubic-bezier" => Self::CubicOut,
            "ease-in-out" | "ease-in-out-cubic" => Self::EaseInOut,
            "ease-out-quad" => Self::QuadOut,
            "ease-out-expo" => Self::ExpoOut,
            _ => return None,
        })
    }
}

pub(super) fn apply_easing(t: f64, easing: Easing) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::None => 1.0,
        Easing::Linear => t,
        Easing::EaseIn => t.powi(3),
        Easing::EaseOut | Easing::CubicOut => 1.0 - (1.0 - t).powi(3),
        Easing::QuadOut => 1.0 - (1.0 - t).powi(2),
        Easing::ExpoOut if t >= 1.0 => 1.0,
        Easing::ExpoOut => 1.0 - 2.0_f64.powf(-10.0 * t),
        Easing::EaseInOut if t < 0.5 => 4.0 * t.powi(3),
        Easing::EaseInOut => 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0,
        Easing::CubicBezier { x1, y1, x2, y2 } => cubic_bezier(t, x1, y1, x2, y2),
    }
}

/// Invert monotonic x, then sample y. Fixed bisection remains stable at flat
/// derivatives, including x1=1, x2=0. The bounds and 31 iterations match niri
/// e19df12's animation/bezier.rs; y deliberately remains unclamped.
fn cubic_bezier(x: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    if x <= f64::EPSILON {
        return 0.0;
    }
    if x >= 1.0 - f64::EPSILON {
        return 1.0;
    }
    let sample = |t: f64, p1: f64, p2: f64| {
        let omt = 1.0 - t;
        3.0 * omt * omt * t * p1 + 3.0 * omt * t * t * p2 + t * t * t
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..=30 {
        let t = (low + high) / 2.0;
        if x < sample(t, x1, x2) {
            high = t;
        } else {
            low = t;
        }
    }
    sample((low + high) / 2.0, y1, y2)
}

/// Unit-mass damped oscillator, matching niri's named spring parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub damping_ratio: f64,
    pub stiffness: f64,
    pub epsilon: f64,
}

impl Default for Spring {
    fn default() -> Self {
        Self {
            damping_ratio: 1.0,
            stiffness: 1000.0,
            epsilon: 0.0001,
        }
    }
}

impl Spring {
    fn valid(self) -> bool {
        self.damping_ratio.is_finite()
            && (0.0..=100.0).contains(&self.damping_ratio)
            && self.stiffness.is_finite()
            && (0.000001..=1_000_000_000.0).contains(&self.stiffness)
            && self.epsilon.is_finite()
            && (0.000000000001..=1.0).contains(&self.epsilon)
    }

    /// Analytic position (relative to target) and velocity. Stable even for large ticks:
    /// overdamping uses two decaying roots, never exp(-a*t)*cosh(b*t).
    fn sample(self, x: f64, v: f64, t: f64) -> (f64, f64) {
        let w = self.stiffness.sqrt();
        let z = self.damping_ratio;
        if (z - 1.0).abs() < 1e-6 {
            let b = v + w * x;
            let decay = (-w * t).exp();
            ((x + b * t) * decay, (v - w * b * t) * decay)
        } else if z < 1.0 {
            let a = z * w;
            let b = w * (1.0 - z * z).sqrt();
            let c = (v + a * x) / b;
            let (sin, cos) = (b * t).sin_cos();
            let decay = (-a * t).exp();
            let pos = x * cos + c * sin;
            (decay * pos, decay * (-a * pos - x * b * sin + c * b * cos))
        } else {
            let root = (z * z - 1.0).sqrt();
            // -w*(z-root), written without cancellation at large damping.
            let r1 = -w / (z + root);
            let r2 = -w * (z + root);
            let c1 = (v - r2 * x) / (r1 - r2);
            let c2 = x - c1;
            let a = c1 * (r1 * t).exp();
            let b = c2 * (r2 * t).exp();
            (a + b, r1 * a + r2 * b)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationKind {
    Off,
    Spring(Spring),
    Easing { duration_ms: u32, curve: Easing },
}

impl AnimationKind {
    pub fn is_off(self) -> bool {
        matches!(
            self,
            Self::Off
                | Self::Easing { duration_ms: 0, .. }
                | Self::Easing {
                    curve: Easing::None,
                    ..
                }
        )
    }
}

#[derive(Debug, Clone)]
pub struct Animation {
    pub start: f64,
    pub end: f64,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
    pub easing: Easing,
    pub finished: bool,
    kind: AnimationKind,
    slowdown: f64,
    elapsed_s: f64,
    initial_velocity: f64,
    value: f64,
    velocity: f64,
}

impl Animation {
    pub fn new(start: f64, end: f64, duration_ms: u32, easing: Easing) -> Self {
        Self::with_kind(
            start,
            end,
            AnimationKind::Easing {
                duration_ms,
                curve: easing,
            },
            1.0,
        )
    }

    pub fn instant(end: f64) -> Self {
        Self::with_kind(end, end, AnimationKind::Off, 1.0)
    }

    pub fn with_kind(start: f64, end: f64, kind: AnimationKind, slowdown: f64) -> Self {
        let end = if end.is_finite() { end } else { 0.0 };
        let start = if start.is_finite() { start } else { end };
        let invalid = matches!(kind, AnimationKind::Spring(s) if !s.valid());
        let finished =
            start == end || kind.is_off() || invalid || !slowdown.is_finite() || slowdown <= 0.0;
        let (duration_ms, easing) = match kind {
            AnimationKind::Easing { duration_ms, curve } => (duration_ms, curve),
            _ => (0, Easing::None),
        };
        Self {
            start,
            end,
            duration_ms,
            elapsed_ms: 0,
            easing,
            finished,
            kind,
            slowdown,
            elapsed_s: 0.0,
            initial_velocity: 0.0,
            value: if finished { end } else { start },
            velocity: 0.0,
        }
    }

    /// Gesture hand-off starts with the measured physical velocity, even when
    /// the nearest snap equals the current position. Off/easing policies retain
    /// their usual semantics; a spring owns deceleration and termination.
    pub fn with_velocity(
        start: f64,
        end: f64,
        velocity: f64,
        kind: AnimationKind,
        slowdown: f64,
    ) -> Self {
        let mut animation = Self::with_kind(start, end, kind, slowdown);
        if matches!(kind, AnimationKind::Spring(s) if s.valid())
            && slowdown.is_finite()
            && slowdown > 0.0
            && velocity.is_finite()
            && (start != end || velocity != 0.0)
        {
            animation.finished = false;
            animation.initial_velocity = velocity;
            animation.velocity = velocity;
            animation.value = animation.start;
        }
        animation
    }

    /// Retarget from the displayed value, retaining spring momentum. Easing keeps
    /// positional continuity (its prescribed curve restarts with its own derivative).
    pub fn retarget(&mut self, end: f64, kind: AnimationKind, slowdown: f64) {
        let value = self.current();
        let velocity = self.velocity;
        *self = Self::with_kind(value, end, kind, slowdown);
        if matches!(kind, AnimationKind::Spring(s) if s.valid())
            && slowdown.is_finite()
            && slowdown > 0.0
            && velocity.is_finite()
            && (value != self.end || velocity != 0.0)
        {
            // Even when retargeting exactly at the displayed position, a moving
            // spring must decelerate rather than silently dropping its momentum.
            self.finished = false;
            self.initial_velocity = velocity;
            self.velocity = velocity;
        }
    }

    pub fn tick(&mut self, delta_ms: u32) -> f64 {
        if self.finished {
            return self.end;
        }
        self.elapsed_ms = self.elapsed_ms.saturating_add(delta_ms);
        // Divide in stages so a finite slowdown cannot overflow the denominator.
        self.elapsed_s += delta_ms as f64 / 1000.0 / self.slowdown;
        match self.kind {
            AnimationKind::Off => self.finished = true,
            AnimationKind::Easing { duration_ms, curve } => {
                let t = self.elapsed_s * 1000.0 / duration_ms as f64;
                self.value = self.start + (self.end - self.start) * apply_easing(t, curve);
                self.velocity = 0.0;
                self.finished = t >= 1.0;
            }
            AnimationKind::Spring(spring) => {
                let (x, v) =
                    spring.sample(self.start - self.end, self.initial_velocity, self.elapsed_s);
                self.value = self.end + x;
                self.velocity = v;
                let scale = (self.start - self.end).abs().max(1.0);
                self.finished = x.abs() <= spring.epsilon * scale
                    && v.abs() <= spring.epsilon * scale * spring.stiffness.sqrt();
                // Undamped/near-undamped user settings must not create an endless
                // rendering task. This cap is animation time, so slowdown still applies.
                self.finished |= self.elapsed_s >= 60.0;
            }
        }
        if !self.value.is_finite() || !self.velocity.is_finite() {
            self.finished = true;
        }
        if self.finished {
            self.value = self.end;
            self.velocity = 0.0;
        }
        self.value
    }

    pub fn current(&self) -> f64 {
        self.value
    }

    /// Rebase coordinates without restarting elapsed time or changing velocity.
    pub(super) fn translate(&mut self, delta: f64) {
        self.start += delta;
        self.end += delta;
        self.value += delta;
    }

    pub fn velocity(&self) -> f64 {
        self.velocity
    }
    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

/// All four HWND coordinates use identical tick deltas, with separate movement/resize kinds.
/// Sizes clamp to a valid Win32 dimension even when an underdamped spring overshoots.
#[derive(Debug, Clone)]
pub struct RectAnimation {
    pub from: Rect,
    pub target: Rect,
    axes: [Animation; 4],
}

impl RectAnimation {
    /// Animate movement and size with the same kind.
    pub fn new(from: Rect, target: Rect, kind: AnimationKind, slowdown: f64) -> Self {
        Self::new_kinds(from, target, kind, kind, slowdown)
    }

    /// Axes 0/1 (x/y) use movement; axes 2/3 (width/height) use resize.
    /// Off axes are at their targets immediately, including before the first tick.
    pub fn new_kinds(
        from: Rect,
        target: Rect,
        movement: AnimationKind,
        resize: AnimationKind,
        slowdown: f64,
    ) -> Self {
        let a = Self::values(from);
        let b = Self::values(target);
        Self {
            from,
            target,
            axes: std::array::from_fn(|i| {
                let kind = if i < 2 { movement } else { resize };
                Animation::with_kind(a[i], b[i], kind, slowdown)
            }),
        }
    }

    fn values(r: Rect) -> [f64; 4] {
        [
            r.loc.x as f64,
            r.loc.y as f64,
            r.size.w as f64,
            r.size.h as f64,
        ]
    }

    pub fn retarget(&mut self, target: Rect, kind: AnimationKind, slowdown: f64) {
        self.retarget_kinds(target, kind, kind, slowdown);
    }

    /// Retarget movement and size independently, snapping Off axes immediately.
    pub fn retarget_kinds(
        &mut self,
        target: Rect,
        movement: AnimationKind,
        resize: AnimationKind,
        slowdown: f64,
    ) {
        self.from = self.current();
        self.target = target;
        for (i, (axis, end)) in self.axes.iter_mut().zip(Self::values(target)).enumerate() {
            let kind = if i < 2 { movement } else { resize };
            axis.retarget(end, kind, slowdown);
        }
    }

    /// Move an existing timeline with the camera without restarting its easing/spring
    /// or touching its resize axes. The target and displayed position share one space.
    pub fn translate(&mut self, dx: i32, dy: i32) {
        for (axis, delta) in self.axes[..2].iter_mut().zip([dx, dy]) {
            let delta = delta as f64;
            axis.start += delta;
            axis.end += delta;
            axis.value += delta;
        }
        self.from.loc.x = self.from.loc.x.saturating_add(dx);
        self.from.loc.y = self.from.loc.y.saturating_add(dy);
        self.target.loc.x = self.target.loc.x.saturating_add(dx);
        self.target.loc.y = self.target.loc.y.saturating_add(dy);
    }

    pub fn current(&self) -> Rect {
        Rect::new(
            self.axes[0].current().round() as i32,
            self.axes[1].current().round() as i32,
            self.axes[2].current().round().clamp(1.0, i32::MAX as f64) as u32,
            self.axes[3].current().round().clamp(1.0, i32::MAX as f64) as u32,
        )
    }

    pub fn tick(&mut self, delta_ms: u32) -> Rect {
        for axis in &mut self.axes {
            axis.tick(delta_ms);
        }
        self.current()
    }

    pub fn is_finished(&self) -> bool {
        self.axes.iter().all(Animation::is_finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gesture_seed_moves_with_measured_velocity_even_at_its_snap() {
        let kind = AnimationKind::Spring(Spring::default());
        let mut moving = Animation::with_velocity(300.0, 300.0, 900.0, kind, 1.0);
        assert_eq!(moving.velocity(), 900.0);
        assert!(!moving.is_finished());
        assert!(moving.tick(1) > 300.0);
        assert_eq!(moving.tick(60_000), 300.0);
        assert!(moving.is_finished());
        let off = Animation::with_velocity(0.0, 100.0, 900.0, AnimationKind::Off, 1.0);
        assert_eq!(off.current(), 100.0);
        assert!(off.is_finished());
    }

    #[test]
    fn analytic_springs_are_frame_rate_independent_and_settle() {
        for damping_ratio in [0.3, 1.0, 3.0] {
            let kind = AnimationKind::Spring(Spring {
                damping_ratio,
                ..Spring::default()
            });
            let mut one = Animation::with_kind(0.0, 1000.0, kind, 1.0);
            let mut many = one.clone();
            one.tick(160);
            for _ in 0..10 {
                many.tick(16);
            }
            assert!((one.current() - many.current()).abs() < 1e-8);
            assert!(one.current().is_finite());
            one.tick(60_000);
            assert!(one.is_finished());
            assert_eq!(one.current(), 1000.0);
        }
    }

    #[test]
    fn underdamping_overshoots_but_critical_damping_does_not() {
        let mut under = Animation::with_kind(
            0.0,
            1.0,
            AnimationKind::Spring(Spring {
                damping_ratio: 0.3,
                ..Spring::default()
            }),
            1.0,
        );
        let mut critical =
            Animation::with_kind(0.0, 1.0, AnimationKind::Spring(Spring::default()), 1.0);
        assert!(under.tick(100) > 1.0);
        assert!(critical.tick(100) < 1.0);
    }

    #[test]
    fn spring_retarget_preserves_position_and_velocity() {
        let kind = AnimationKind::Spring(Spring::default());
        let mut a = Animation::with_kind(0.0, 100.0, kind, 1.0);
        a.tick(70);
        let (pos, velocity) = (a.current(), a.velocity());
        a.retarget(-100.0, kind, 1.0);
        assert_eq!(a.current(), pos);
        assert_eq!(a.velocity(), velocity);
        a.tick(1);
        assert!(a.current() > pos, "retarget retains outgoing momentum");
    }

    #[test]
    fn spring_retarget_at_current_position_keeps_momentum() {
        let kind = AnimationKind::Spring(Spring::default());
        let mut a = Animation::with_kind(0.0, 100.0, kind, 1.0);
        a.tick(70);
        let (pos, velocity) = (a.current(), a.velocity());
        assert!(velocity > 0.0);
        a.retarget(pos, kind, 1.0);
        assert_eq!(a.current(), pos);
        assert_eq!(a.velocity(), velocity);
        assert!(!a.is_finished());
        assert!(a.tick(1) > pos);
        assert_eq!(a.tick(60_000), pos);
        assert!(a.is_finished());
    }

    #[test]
    fn extreme_finite_slowdowns_make_progress_or_finish_safely() {
        let easing = AnimationKind::Easing {
            duration_ms: 100,
            curve: Easing::Linear,
        };
        let mut slow = Animation::with_kind(0.0, 100.0, easing, f64::MAX);
        let value = slow.tick(100);
        assert!(value.is_finite() && value > 0.0);
        assert!(!slow.is_finished());

        for kind in [easing, AnimationKind::Spring(Spring::default())] {
            let mut fast = Animation::with_kind(0.0, 100.0, kind, f64::from_bits(1));
            assert_eq!(fast.tick(1), 100.0);
            assert!(fast.is_finished());
            assert_eq!(fast.velocity(), 0.0);
        }
    }

    #[test]
    fn slowdown_zero_off_and_non_finite_inputs_are_safe() {
        let kind = AnimationKind::Easing {
            duration_ms: 200,
            curve: Easing::Linear,
        };
        let mut slow = Animation::with_kind(0.0, 100.0, kind, 2.0);
        assert_eq!(slow.tick(200), 50.0);
        assert_eq!(slow.tick(200), 100.0);
        for factor in [0.0, f64::NAN, f64::INFINITY] {
            let a = Animation::with_kind(0.0, 100.0, kind, factor);
            assert!(a.is_finished());
            assert_eq!(a.current(), 100.0);
        }
        assert!(Animation::with_kind(0.0, 100.0, AnimationKind::Off, 1.0).is_finished());
        assert!(Animation::with_kind(f64::NAN, f64::NAN, kind, 1.0)
            .current()
            .is_finite());
    }

    #[test]
    fn epsilon_checks_velocity_not_only_a_target_crossing() {
        let mut a = Animation::with_kind(
            0.0,
            1.0,
            AnimationKind::Spring(Spring {
                damping_ratio: 0.0,
                ..Spring::default()
            }),
            1.0,
        );
        a.tick(50);
        assert!(!a.is_finished());
        a.tick(60_000);
        assert!(a.is_finished(), "undamped springs have a finite safety cap");
    }

    #[test]
    fn rect_retarget_and_size_interpolation_share_the_timeline() {
        let kind = AnimationKind::Easing {
            duration_ms: 100,
            curve: Easing::Linear,
        };
        let mut rect = RectAnimation::new(
            Rect::new(-100, 0, 100, 100),
            Rect::new(100, 100, 300, 300),
            kind,
            1.0,
        );
        assert_eq!(rect.tick(50), Rect::new(0, 50, 200, 200));
        let mid = rect.current();
        rect.retarget(Rect::new(200, 0, 100, 100), kind, 1.0);
        assert_eq!(rect.current(), mid);
        assert_eq!(rect.tick(100), rect.target);
        assert!(rect.is_finished());
    }

    #[test]
    fn camera_translation_preserves_resize_progress_and_spring_momentum() {
        let kind = AnimationKind::Spring(Spring::default());
        let mut rect = RectAnimation::new(
            Rect::new(100, 20, 200, 300),
            Rect::new(300, 80, 650, 700),
            kind,
            1.0,
        );
        let before = rect.tick(50);
        let velocities: Vec<_> = rect.axes.iter().map(Animation::velocity).collect();
        let elapsed: Vec<_> = rect.axes.iter().map(|a| a.elapsed_ms).collect();
        rect.translate(-40, 12);
        assert_eq!(
            rect.current(),
            Rect::new(
                before.loc.x - 40,
                before.loc.y + 12,
                before.size.w,
                before.size.h
            )
        );
        assert_eq!(rect.target, Rect::new(260, 92, 650, 700));
        assert_eq!(
            rect.axes
                .iter()
                .map(Animation::velocity)
                .collect::<Vec<_>>(),
            velocities
        );
        assert_eq!(
            rect.axes.iter().map(|a| a.elapsed_ms).collect::<Vec<_>>(),
            elapsed
        );
        assert_eq!(rect.tick(60_000), rect.target);
    }

    #[test]
    fn rect_finished_ticks_clamp_invalid_target_dimensions() {
        let target = Rect::new(10, 20, 0, u32::MAX);
        let expected = Rect::new(10, 20, 1, i32::MAX as u32);
        for kind in [
            AnimationKind::Off,
            AnimationKind::Easing {
                duration_ms: 100,
                curve: Easing::Linear,
            },
        ] {
            let mut rect = RectAnimation::new(Rect::new(0, 0, 100, 100), target, kind, 1.0);
            assert_eq!(rect.tick(100), expected);
            assert!(rect.is_finished());
            assert_eq!(rect.current(), expected);
            assert_eq!(rect.tick(0), expected);
        }
    }

    #[test]
    fn rect_initial_instant_axes_leave_other_axes_animating() {
        let from = Rect::new(-100, -50, 100, 200);
        let target = Rect::new(100, 50, 300, 400);
        let animated = AnimationKind::Easing {
            duration_ms: 100,
            curve: Easing::Linear,
        };
        for instant in [
            AnimationKind::Off,
            AnimationKind::Easing {
                duration_ms: 0,
                curve: Easing::Linear,
            },
            AnimationKind::Easing {
                duration_ms: 100,
                curve: Easing::None,
            },
        ] {
            let mut rect = RectAnimation::new_kinds(from, target, instant, animated, 1.0);
            assert_eq!(rect.current(), Rect::new(100, 50, 100, 200));
            assert!(!rect.is_finished());
            assert_eq!(rect.tick(50), Rect::new(100, 50, 200, 300));
            assert_eq!(rect.tick(50), target);
            assert!(rect.is_finished());

            let mut rect = RectAnimation::new_kinds(from, target, animated, instant, 1.0);
            assert_eq!(rect.current(), Rect::new(-100, -50, 300, 400));
            assert!(!rect.is_finished());
            assert_eq!(rect.tick(50), Rect::new(0, 0, 300, 400));
            assert_eq!(rect.tick(50), target);
            assert!(rect.is_finished());
        }
    }

    #[test]
    fn rect_retarget_kinds_can_disable_each_group_independently() {
        let kind = AnimationKind::Easing {
            duration_ms: 100,
            curve: Easing::Linear,
        };
        let mut rect = RectAnimation::new(
            Rect::new(-100, -50, 100, 100),
            Rect::new(100, 50, 300, 300),
            kind,
            1.0,
        );
        let mid = rect.tick(50);
        rect.retarget_kinds(Rect::new(200, 100, 400, 400), AnimationKind::Off, kind, 1.0);
        assert_eq!(rect.from, mid);
        assert_eq!(rect.current(), Rect::new(200, 100, 200, 200));
        assert!(!rect.is_finished());
        assert_eq!(rect.tick(50), Rect::new(200, 100, 300, 300));

        let target = Rect::new(400, 200, 600, 600);
        rect.retarget_kinds(target, kind, AnimationKind::Off, 1.0);
        assert_eq!(rect.current(), Rect::new(200, 100, 600, 600));
        assert!(!rect.is_finished());
        assert_eq!(rect.tick(50), Rect::new(300, 150, 600, 600));
        assert_eq!(rect.tick(50), target);
        assert!(rect.is_finished());
    }

    #[test]
    fn niri_curve_endpoints_and_names() {
        for curve in [
            Easing::Linear,
            Easing::QuadOut,
            Easing::CubicOut,
            Easing::ExpoOut,
        ] {
            assert_eq!(apply_easing(0.0, curve), 0.0);
            assert_eq!(apply_easing(1.0, curve), 1.0);
        }
        assert_eq!(Easing::parse("ease-out-expo"), Some(Easing::ExpoOut));
        assert_eq!(Easing::parse("ease-out-quad"), Some(Easing::QuadOut));
        assert_eq!(Easing::parse("typo"), None);
    }

    #[test]
    fn parity_bezier_inverts_x_instead_of_sampling_at_time() {
        let curve = Easing::CubicBezier {
            x1: 0.0,
            y1: 0.0,
            x2: 0.0,
            y2: 1.0,
        };
        // x(t)=t^3: x=.125 means parameter .5 and y=.5, not y(.125).
        assert!((curve.apply(0.125) - 0.5).abs() < 0.000001);
        assert_eq!(curve.apply(-1.0), 0.0);
        assert_eq!(curve.apply(0.0), 0.0);
        assert_eq!(curve.apply(1.0), 1.0);
        assert_eq!(curve.apply(2.0), 1.0);
    }

    #[test]
    fn parity_bezier_flat_slopes_remain_finite_and_accurate() {
        for (x1, x2, x, expected) in [
            (0.0, 0.0, 0.125, 0.5),
            (1.0, 1.0, 0.875, 0.5),
            (1.0, 0.0, 0.4375, 0.15625),
            (1.0, 0.0, 0.5, 0.5),
        ] {
            let curve = Easing::CubicBezier {
                x1,
                y1: 0.0,
                x2,
                y2: 1.0,
            };
            let actual = curve.apply(x);
            assert!((actual - expected).abs() < 0.00001, "{curve:?}: {actual}");
        }
    }

    #[test]
    fn parity_bezier_identity_and_overshoot_are_not_clamped() {
        let identity = Easing::CubicBezier {
            x1: 0.25,
            y1: 0.25,
            x2: 0.75,
            y2: 0.75,
        };
        for t in [0.0, 0.01, 0.25, 0.5, 0.99, 1.0] {
            assert!((identity.apply(t) - t).abs() < 0.000001);
        }
        for (y, expected) in [(2.0, 1.625), (-2.0, -1.375)] {
            let curve = Easing::CubicBezier {
                x1: 0.0,
                y1: y,
                x2: 1.0,
                y2: y,
            };
            assert!((curve.apply(0.5) - expected).abs() < 0.000001);
            let mut animation = Animation::new(0.0, 100.0, 1000, curve);
            assert!((animation.tick(500) - 100.0 * expected).abs() < 0.0001);
            assert!(!animation.is_finished());
            assert_eq!(animation.tick(500), 100.0);
            assert!(animation.is_finished());
        }
    }

    #[test]
    fn parity_bezier_and_existing_curves_honor_slowdown() {
        for (curve, expected) in [
            (
                Easing::CubicBezier {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 0.0,
                    y2: 1.0,
                },
                50.0,
            ),
            (Easing::Linear, 12.5),
            (Easing::CubicOut, 33.0078125),
            (Easing::QuadOut, 23.4375),
        ] {
            let mut animation = Animation::with_kind(
                0.0,
                100.0,
                AnimationKind::Easing {
                    duration_ms: 1000,
                    curve,
                },
                2.0,
            );
            assert!(
                (animation.tick(250) - expected).abs() < 0.000001,
                "{curve:?}"
            );
            assert_eq!(animation.tick(1750), 100.0);
            assert!(animation.is_finished());
        }
    }
}
