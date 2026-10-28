//! Pure window-policy helpers, following niri at `e19df12cf8`.
//!
//! Relevant upstream contracts ([window-rule documentation][docs]):
//! - Column display is used "any time a window goes into its own column", not just
//!   at admission; changing a rule must not flip an existing column's display.
//! - Floating position uses "logical coordinates relative to the working area".
//!   Positive offsets point inward from the chosen edges. A single edge centers the
//!   other axis. "Afterward, the window will remember its last floating position."
//! - Scroll factor is "multiplied with the scroll factor set for your input device".
//! - An unset activation rule is *not* `ignore`: the caller retains its normal policy.
//!
//! [docs]: https://github.com/YaLTeR/niri/blob/e19df12cf8/docs/wiki/Configuration%3A-Window-Rules.md
//!
//! Wiring: put `policy: RulePolicy` on both rule structs; merge only matching rules.
//! Use the placement helpers only when creating a column / first floating rectangle.
//! Publish scroll multipliers to the input hook as a small snapshot; do not lock the
//! layout engine or call Win32 from these helpers. Activation applies to application
//! requests, never to explicit user focus or to every foreground notification.

use crate::utils::{Rect, Size, WindowId};

pub const FLOATING_POSITION_LIMIT: f64 = 65_535.0;
pub const MAX_SCROLL_FACTOR: f64 = 100.0;

/// A default for a *new* column, distinct from the layout's live column display state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DefaultColumnDisplay {
    #[default]
    Normal,
    Tabbed,
}

impl DefaultColumnDisplay {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(Self::Normal),
            "tabbed" => Some(Self::Tabbed),
            _ => None,
        }
    }

    pub fn is_tabbed(self) -> bool {
        self == Self::Tabbed
    }
}

/// The eight working-area anchors accepted by niri. There is no `center` anchor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RelativeTo {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
}

impl RelativeTo {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "top-left" => Some(Self::TopLeft),
            "top-right" => Some(Self::TopRight),
            "bottom-left" => Some(Self::BottomLeft),
            "bottom-right" => Some(Self::BottomRight),
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// Logical offsets in `-65535..=65535`; both coordinates are required by the parser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatingPosition {
    pub x: f64,
    pub y: f64,
    pub relative_to: RelativeTo,
}

impl FloatingPosition {
    /// Resolve against physical working-area coordinates and an already-sized window.
    /// Only the logical offsets are multiplied by the output DPI scale. Size is unchanged.
    ///
    /// Right/bottom anchors subtract offsets; a centered axis adds them. Deliberately
    /// does not fit/clamp into the work area: negative offsets may put a window outside
    /// it. Any separate visibility-safety policy belongs to the engine. Invalid scale
    /// falls back to 1; final coordinates saturate to i32 instead of overflowing.
    pub fn rect(self, work: Rect, size: Size, scale: f64) -> Rect {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        // Signed floating-point differences also handle a window larger than the work area.
        let free_x = f64::from(work.size.w) - f64::from(size.w);
        let free_y = f64::from(work.size.h) - f64::from(size.h);
        let dx = self.x * scale;
        let dy = self.y * scale;
        let x = match self.relative_to {
            RelativeTo::TopLeft | RelativeTo::BottomLeft | RelativeTo::Left => dx,
            RelativeTo::TopRight | RelativeTo::BottomRight | RelativeTo::Right => free_x - dx,
            RelativeTo::Top | RelativeTo::Bottom => free_x / 2.0 + dx,
        };
        let y = match self.relative_to {
            RelativeTo::TopLeft | RelativeTo::TopRight | RelativeTo::Top => dy,
            RelativeTo::BottomLeft | RelativeTo::BottomRight | RelativeTo::Bottom => free_y - dy,
            RelativeTo::Left | RelativeTo::Right => free_y / 2.0 + dy,
        };
        Rect::new(
            (f64::from(work.loc.x) + x).round() as i32,
            (f64::from(work.loc.y) + y).round() as i32,
            size.w,
            size.h,
        )
    }
}

/// An override for a validated application activation request, not for user focus.
/// The absence of a rule must remain distinct from every variant of this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnXdgActivate {
    Ignore,
    SetUrgent,
    Focus,
}

impl OnXdgActivate {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ignore" => Some(Self::Ignore),
            "set-urgent" => Some(Self::SetUrgent),
            "focus" => Some(Self::Focus),
            _ => None,
        }
    }
}

/// Sparse overrides. `None` always means "not specified", including in resolved rules.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RulePolicy {
    pub default_column_display: Option<DefaultColumnDisplay>,
    pub default_floating_position: Option<FloatingPosition>,
    /// Capture on admission and retain across dynamic rule re-evaluation, including None.
    pub on_xdg_activate: Option<OnXdgActivate>,
    /// A finite multiplier in `0..=100`; zero intentionally suppresses application scroll.
    pub scroll_factor: Option<f64>,
}

impl RulePolicy {
    /// Last specified value wins. A floating-position node replaces the whole position.
    pub fn merge(&mut self, later: &Self) {
        if later.default_column_display.is_some() {
            self.default_column_display = later.default_column_display;
        }
        if later.default_floating_position.is_some() {
            self.default_floating_position = later.default_floating_position;
        }
        if later.on_xdg_activate.is_some() {
            self.on_xdg_activate = later.on_xdg_activate;
        }
        if later.scroll_factor.is_some() {
            self.scroll_factor = later.scroll_factor;
        }
    }

    /// niri's activation rule is latched at initial configure; the other three fields
    /// follow rule changes, but placement defaults affect only future transitions.
    pub fn same_dynamic_part(&self, other: &Self) -> bool {
        self.default_column_display == other.default_column_display
            && self.default_floating_position == other.default_floating_position
            && self.scroll_factor == other.scroll_factor
    }

    /// `normal` must override a layout-wide tabbed default as explicitly as `tabbed`.
    pub fn column_is_tabbed(self, layout_default: bool) -> bool {
        self.default_column_display
            .map_or(layout_default, DefaultColumnDisplay::is_tabbed)
    }

    /// With no position rule, preserve the caller's fallback exactly (including size).
    /// Call only if no remembered floating position exists.
    pub fn floating_rect(self, work: Rect, fallback: Rect, scale: f64) -> Rect {
        self.default_floating_position.map_or(fallback, |position| {
            position.rect(work, fallback.size, scale)
        })
    }

    /// Keep the platform's normal decision when unset. The caller must first validate
    /// and classify the request; Windows cannot recover Wayland serials from a WinEvent.
    pub fn activation_action(self, platform_default: OnXdgActivate) -> OnXdgActivate {
        self.on_xdg_activate.unwrap_or(platform_default)
    }

    /// Device and rule factors compose; neither replaces the other. Device factor is
    /// supplied by the input layer, already validated as finite and nonnegative.
    /// Saturate positive overflow from finite operands so accumulation can saturate
    /// wheel units instead of treating the composed factor as an invalid snapshot.
    pub fn scroll_multiplier(self, device_factor: f64) -> f64 {
        let rule_factor = self.scroll_factor.unwrap_or(1.0);
        let factor = rule_factor * device_factor;
        if factor == f64::INFINITY && rule_factor.is_finite() && device_factor.is_finite() {
            f64::MAX
        } else {
            factor
        }
    }
}

/// Fractional wheel deltas for one current recipient, with independent axes.
///
/// The hook owns this state, resets it on snapshot/config replacement, and calls it
/// only for unconsumed, non-injected application wheel events. A recipient or factor
/// change resets both remainders so another window cannot inherit a partial delta.
#[derive(Debug, Clone, Default)]
pub struct ScrollAccumulator {
    context: Option<(WindowId, f64)>,
    remainder: [f64; 2],
}

impl ScrollAccumulator {
    pub fn reset(&mut self) {
        self.context = None;
        self.remainder = [0.0; 2];
    }

    /// Scale signed wheel units (not notches). Zero output is meaningful: swallow the
    /// original event and wait for more fraction, or suppress it for a zero multiplier.
    /// Call even for factor 1 (or reset on bypass) to track recipient/factor changes.
    /// The caller handles pass-through at factor 1 and re-injection at other factors;
    /// injected events must bypass binding dispatch as well as this accumulator.
    pub fn scale(&mut self, target: WindowId, horizontal: bool, delta: i32, factor: f64) -> i32 {
        // A bad snapshot must not turn ordinary scrolling into a swallowed event.
        let factor = if factor.is_finite() && factor >= 0.0 {
            factor
        } else {
            1.0
        };
        if self.context != Some((target, factor)) {
            self.reset();
            self.context = Some((target, factor));
        }
        let axis = usize::from(horizontal);
        let total = f64::from(delta) * factor + self.remainder[axis];
        let whole = total.trunc();
        // Saturation also covers an unexpectedly huge composed device multiplier.
        let output = whole as i32;
        self.remainder[axis] = if whole >= f64::from(i32::MIN) && whole <= f64::from(i32::MAX) {
            total - whole
        } else {
            0.0
        };
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(relative_to: RelativeTo, x: f64, y: f64) -> FloatingPosition {
        FloatingPosition { x, y, relative_to }
    }

    #[test]
    fn enum_values_match_the_pinned_syntax_exactly() {
        assert_eq!(
            DefaultColumnDisplay::parse("normal"),
            Some(DefaultColumnDisplay::Normal)
        );
        assert_eq!(
            DefaultColumnDisplay::parse("tabbed"),
            Some(DefaultColumnDisplay::Tabbed)
        );
        assert_eq!(DefaultColumnDisplay::parse("stacked"), None);
        assert_eq!(OnXdgActivate::parse("ignore"), Some(OnXdgActivate::Ignore));
        assert_eq!(
            OnXdgActivate::parse("set-urgent"),
            Some(OnXdgActivate::SetUrgent)
        );
        assert_eq!(OnXdgActivate::parse("focus"), Some(OnXdgActivate::Focus));
        assert_eq!(OnXdgActivate::parse("urgent"), None);
        assert_eq!(RelativeTo::parse("center"), None);
        assert_eq!(RelativeTo::parse("TOP-LEFT"), None);
        assert_eq!(RelativeTo::default(), RelativeTo::TopLeft);
    }

    #[test]
    fn all_eight_anchors_use_inward_offsets_and_physical_work_origin() {
        let work = Rect::new(-900, 40, 1000, 800);
        let size = Size::new(200, 100);
        let cases = [
            ("top-left", RelativeTo::TopLeft, -880, 80),
            ("top-right", RelativeTo::TopRight, -120, 80),
            ("bottom-left", RelativeTo::BottomLeft, -880, 700),
            ("bottom-right", RelativeTo::BottomRight, -120, 700),
            ("top", RelativeTo::Top, -480, 80),
            ("bottom", RelativeTo::Bottom, -480, 700),
            ("left", RelativeTo::Left, -880, 430),
            ("right", RelativeTo::Right, -120, 430),
        ];
        for (name, anchor, x, y) in cases {
            assert_eq!(RelativeTo::parse(name), Some(anchor));
            assert_eq!(
                position(anchor, 10.0, 20.0).rect(work, size, 2.0),
                Rect::new(x, y, 200, 100)
            );
        }
    }

    #[test]
    fn negative_offsets_are_not_clamped_away() {
        let work = Rect::new(100, 200, 1000, 800);
        let size = Size::new(200, 100);
        assert_eq!(
            position(RelativeTo::TopLeft, -10.0, -20.0).rect(work, size, 1.0),
            Rect::new(90, 180, 200, 100)
        );
        assert_eq!(
            position(RelativeTo::BottomRight, -10.0, -20.0).rect(work, size, 1.0),
            Rect::new(910, 920, 200, 100)
        );
    }

    #[test]
    fn fractional_dpi_offsets_round_once_without_scaling_size() {
        let rect = position(RelativeTo::Top, 10.5, -2.25).rect(
            Rect::new(50, -100, 501, 601),
            Size::new(100, 200),
            1.5,
        );
        assert_eq!(rect, Rect::new(266, -103, 100, 200));
    }

    #[test]
    fn oversized_windows_and_extreme_origins_do_not_wrap() {
        let rect = position(RelativeTo::BottomRight, 0.0, 0.0).rect(
            Rect::new(-100, -100, 50, 50),
            Size::new(100, 100),
            1.0,
        );
        assert_eq!(rect, Rect::new(-150, -150, 100, 100));
        let rect = position(RelativeTo::BottomRight, -65_535.0, -65_535.0).rect(
            Rect::new(i32::MAX, i32::MAX, u32::MAX, u32::MAX),
            Size::new(1, 1),
            2.0,
        );
        assert_eq!(rect, Rect::new(i32::MAX, i32::MAX, 1, 1));
    }

    #[test]
    fn invalid_scale_uses_logical_scale_one() {
        let work = Rect::new(10, 20, 100, 100);
        let size = Size::new(10, 10);
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                position(RelativeTo::TopLeft, 2.0, 3.0).rect(work, size, scale),
                Rect::new(12, 23, 10, 10)
            );
        }
    }

    #[test]
    fn unset_policy_preserves_every_platform_default() {
        let policy = RulePolicy::default();
        assert!(!policy.column_is_tabbed(false));
        assert!(policy.column_is_tabbed(true));
        let fallback = Rect::new(-20, 30, 400, 300);
        assert_eq!(
            policy.floating_rect(Rect::new(0, 0, 1000, 800), fallback, 2.0),
            fallback
        );
        for action in [
            OnXdgActivate::Ignore,
            OnXdgActivate::SetUrgent,
            OnXdgActivate::Focus,
        ] {
            assert_eq!(policy.activation_action(action), action);
        }
        assert_eq!(policy.scroll_multiplier(1.5), 1.5);
    }

    #[test]
    fn sparse_merge_preserves_omissions_but_honors_normal_ignore_and_zero() {
        let mut policy = RulePolicy {
            default_column_display: Some(DefaultColumnDisplay::Tabbed),
            default_floating_position: Some(position(RelativeTo::Bottom, 1.0, 2.0)),
            on_xdg_activate: Some(OnXdgActivate::Focus),
            scroll_factor: Some(2.0),
        };
        let original_position = policy.default_floating_position;
        policy.merge(&RulePolicy::default());
        assert_eq!(policy.scroll_factor, Some(2.0));
        policy.merge(&RulePolicy {
            default_column_display: Some(DefaultColumnDisplay::Normal),
            on_xdg_activate: Some(OnXdgActivate::Ignore),
            scroll_factor: Some(0.0),
            ..RulePolicy::default()
        });
        assert!(!policy.column_is_tabbed(true));
        assert_eq!(policy.default_floating_position, original_position);
        assert_eq!(
            policy.activation_action(OnXdgActivate::Focus),
            OnXdgActivate::Ignore
        );
        assert_eq!(policy.scroll_multiplier(2.0), 0.0);
        policy.merge(&RulePolicy {
            default_floating_position: Some(position(RelativeTo::TopLeft, 0.0, 0.0)),
            ..RulePolicy::default()
        });
        assert_eq!(
            policy.default_floating_position,
            Some(position(RelativeTo::TopLeft, 0.0, 0.0))
        );
    }

    #[test]
    fn future_placement_and_scroll_are_dynamic_but_activation_is_latched() {
        let base = RulePolicy::default();
        let mut other = base;
        other.on_xdg_activate = Some(OnXdgActivate::Ignore);
        assert!(base.same_dynamic_part(&other));
        other.scroll_factor = Some(0.0);
        assert!(!base.same_dynamic_part(&other));
        other = base;
        other.default_column_display = Some(DefaultColumnDisplay::Normal);
        assert!(!base.same_dynamic_part(&other));
        other = base;
        other.default_floating_position = Some(position(RelativeTo::Top, 0.0, 0.0));
        assert!(!base.same_dynamic_part(&other));
    }

    #[test]
    fn scroll_composes_device_and_rule_multipliers() {
        let policy = RulePolicy {
            scroll_factor: Some(0.75),
            ..RulePolicy::default()
        };
        assert_eq!(policy.scroll_multiplier(2.0), 1.5);
        assert_eq!(policy.scroll_multiplier(0.0), 0.0);
    }

    #[test]
    fn finite_composition_overflow_saturates_instead_of_reverting_to_identity() {
        let policy = RulePolicy {
            scroll_factor: Some(2.0),
            ..RulePolicy::default()
        };
        let factor = policy.scroll_multiplier(f64::MAX);
        assert_eq!(factor, f64::MAX);
        let mut scroll = ScrollAccumulator::default();
        let target = WindowId::new(1);
        assert_eq!(scroll.scale(target, false, 0, factor), 0);
        assert_eq!(scroll.scale(target, false, 120, factor), i32::MAX);
        assert_eq!(scroll.scale(target, false, -120, factor), i32::MIN);
        assert_eq!(scroll.scale(target, false, 0, factor), 0);
        let zero = RulePolicy {
            scroll_factor: Some(0.0),
            ..RulePolicy::default()
        };
        assert_eq!(zero.scroll_multiplier(f64::MAX), 0.0);
    }

    #[test]
    fn fractional_positive_and_negative_scroll_is_not_lost() {
        for sign in [-1, 1] {
            let mut scroll = ScrollAccumulator::default();
            let target = WindowId::new(1);
            let values: Vec<_> = (0..4)
                .map(|_| scroll.scale(target, false, sign, 0.25))
                .collect();
            assert_eq!(values, vec![0, 0, 0, sign]);
            assert_eq!(scroll.scale(target, false, 120 * sign, 0.75), 90 * sign);
        }
    }

    #[test]
    fn axes_have_independent_fractions_and_direction_changes_cancel() {
        let mut scroll = ScrollAccumulator::default();
        let target = WindowId::new(1);
        assert_eq!(scroll.scale(target, false, 1, 0.5), 0);
        assert_eq!(scroll.scale(target, true, 1, 0.5), 0);
        assert_eq!(scroll.scale(target, false, -1, 0.5), 0);
        assert_eq!(scroll.scale(target, true, 1, 0.5), 1);
        assert_eq!(scroll.scale(target, false, 1, 0.5), 0);
    }

    #[test]
    fn recipient_factor_and_snapshot_changes_clear_old_fraction() {
        let mut scroll = ScrollAccumulator::default();
        let a = WindowId::new(1);
        let b = WindowId::new(2);
        assert_eq!(scroll.scale(a, false, 1, 0.5), 0);
        assert_eq!(scroll.scale(b, false, 1, 0.5), 0);
        assert_eq!(scroll.scale(b, false, 1, 0.25), 0);
        assert_eq!(scroll.scale(b, false, 1, 0.5), 0);
        scroll.reset();
        assert_eq!(scroll.scale(b, false, 1, 0.5), 0);
    }

    #[test]
    fn zero_identity_invalid_and_large_scroll_factors_are_safe() {
        let mut scroll = ScrollAccumulator::default();
        let target = WindowId::new(1);
        assert_eq!(scroll.scale(target, false, -120, 0.0), 0);
        assert_eq!(scroll.scale(target, true, -120, 1.0), -120);
        for factor in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(scroll.scale(target, false, 120, factor), 120);
        }
        assert_eq!(scroll.scale(target, false, 120, 100.0), 12_000);
        assert_eq!(scroll.scale(target, false, i32::MAX, 100.0), i32::MAX);
        assert_eq!(scroll.scale(target, false, i32::MIN, 100.0), i32::MIN);
    }
}
