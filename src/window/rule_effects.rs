//! Sparse decoration rules. Unset keys never replace the layout (or an earlier rule).
//! A colour replaces a gradient and vice versa, as in niri's appearance::MergeWith.
use crate::config::{Effects, PaintStyle, RingStyle, ShadowStyle, TabIndicatorStyle};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RingOverride {
    pub off: Option<bool>,
    /// Logical pixels (niri `FloatOrInt<0, 65535>`); fractional values are kept.
    pub width: Option<f64>,
    pub active: Option<PaintStyle>,
    pub inactive: Option<PaintStyle>,
    pub urgent: Option<PaintStyle>,
}

macro_rules! merge_fields {
    ($target:expr, $part:expr, $($field:ident),+ $(,)?) => {
        $(if let Some(value) = &$part.$field { $target.$field = Some(value.clone()); })+
    };
}
macro_rules! apply_fields {
    ($target:expr, $part:expr, $($field:ident),+ $(,)?) => {
        $(if let Some(value) = &$part.$field { $target.$field = value.clone(); })+
    };
}

impl RingOverride {
    pub fn merge(&mut self, part: &Self) {
        merge_fields!(self, part, off, width, active, inactive, urgent);
    }
    pub fn apply(&self, target: &mut RingStyle) {
        apply_fields!(target, self, off, width, active, inactive, urgent);
    }
    /// The border band in PHYSICAL pixels on an output of `scale`.
    ///
    /// Honour layout-off unless explicitly enabled; enabling inherits its saved width. The
    /// rule's own logical width, and the saved width of an off layout border, are projected
    /// through the output's scale exactly as niri does (tile.rs:244,252-257:
    /// `round_logical_in_physical_max1` on the merged border config). `legacy_width` is the
    /// layout's own band, already projected for the output.
    pub fn inset(&self, base: &RingStyle, legacy_width: i32, scale: f64) -> i32 {
        if self.off.unwrap_or(base.off) {
            return 0;
        }
        self.width.map_or_else(
            || {
                if base.off {
                    crate::layout::metrics::ring_width_px(base.width, scale)
                } else {
                    legacy_width.max(0)
                }
            },
            |w| crate::layout::metrics::ring_width_px(w, scale),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShadowOverride {
    pub on: Option<bool>,
    pub softness: Option<f64>,
    pub spread: Option<f64>,
    pub offset_x: Option<f64>,
    pub offset_y: Option<f64>,
    pub draw_behind_window: Option<bool>,
    pub color: Option<String>,
    pub inactive_color: Option<String>,
}
impl ShadowOverride {
    pub fn merge(&mut self, part: &Self) {
        merge_fields!(
            self,
            part,
            on,
            softness,
            spread,
            offset_x,
            offset_y,
            draw_behind_window,
            color,
            inactive_color
        );
    }
    pub fn apply(&self, target: &mut ShadowStyle) {
        apply_fields!(
            target,
            self,
            on,
            softness,
            spread,
            offset_x,
            offset_y,
            draw_behind_window,
            color
        );
        if let Some(value) = &self.inactive_color {
            target.inactive_color = Some(value.clone());
        }
    }
}

/// niri's TabIndicatorRule overrides paints per tab, not column-wide geometry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabOverride {
    pub active: Option<PaintStyle>,
    pub inactive: Option<PaintStyle>,
    pub urgent: Option<PaintStyle>,
}
impl TabOverride {
    pub fn merge(&mut self, part: &Self) {
        merge_fields!(self, part, active, inactive, urgent);
    }
    pub fn apply(&self, target: &mut TabIndicatorStyle) {
        merge_fields!(target, self, active, inactive, urgent);
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleEffects {
    pub focus_ring: RingOverride,
    pub border: RingOverride,
    pub shadow: ShadowOverride,
    pub tab_indicator: TabOverride,
    /// CSS order: top-left, top-right, bottom-right, bottom-left, in logical px.
    pub geometry_corner_radius: Option<[f32; 4]>,
    pub clip_to_geometry: Option<bool>,
}
impl RuleEffects {
    pub fn merge(&mut self, part: &Self) {
        self.focus_ring.merge(&part.focus_ring);
        self.border.merge(&part.border);
        self.shadow.merge(&part.shadow);
        self.tab_indicator.merge(&part.tab_indicator);
        merge_fields!(self, part, geometry_corner_radius, clip_to_geometry);
    }
    pub fn apply_to(&self, base: &Effects) -> Effects {
        let mut out = base.clone();
        self.focus_ring.apply(&mut out.focus_ring);
        self.border.apply(&mut out.border);
        self.shadow.apply(&mut out.shadow);
        self.tab_indicator.apply(&mut out.tab_indicator);
        out
    }
    /// Disable DWM's own rounding while the exact four-corner HWND region is active.
    /// Geometry alone affects overlays and leaves the application's corners unchanged.
    pub fn dwm_radius(&self) -> Option<u32> {
        (self.clip_to_geometry == Some(true)).then_some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_empty_block_is_not_a_reset() {
        let mut base = Effects::default();
        base.border.width = 11.0;
        assert_eq!(RuleEffects::default().apply_to(&base), base);
    }
    #[test]
    fn later_keys_merge_and_unset_keys_survive() {
        let mut first = RuleEffects::default();
        first.border.width = Some(9.0);
        first.border.inactive = Some(PaintStyle::color("red"));
        let mut second = RuleEffects::default();
        second.border.active = Some(PaintStyle::color("lime"));
        first.merge(&second);
        let result = first.apply_to(&Effects::default());
        assert_eq!(result.border.width, 9.0);
        assert_eq!(result.border.inactive, PaintStyle::color("red"));
        assert_eq!(result.border.active, PaintStyle::color("lime"));
        assert_eq!(result.shadow, Effects::default().shadow);
    }
    #[test]
    fn explicit_false_and_zero_override_previous_values() {
        let mut first = RuleEffects::default();
        first.border.off = Some(true);
        first.border.width = Some(6.0);
        let mut second = RuleEffects::default();
        second.border.off = Some(false);
        second.border.width = Some(0.0);
        first.merge(&second);
        let result = first.apply_to(&Effects::default());
        assert!(!result.border.off);
        assert_eq!(result.border.width, 0.0);
    }
    #[test]
    fn enabling_an_off_border_restores_its_width_and_width_alone_does_not_enable() {
        let mut base = RingStyle::border_default();
        base.off = true;
        base.width = 7.0;
        let mut rule = RingOverride::default();
        rule.width = Some(11.0);
        assert_eq!(rule.inset(&base, 0, 1.0), 0);
        rule.width = None;
        rule.off = Some(false);
        assert_eq!(rule.inset(&base, 0, 1.0), 7);
        rule.width = Some(11.0);
        assert_eq!(rule.inset(&base, 0, 1.0), 11);
    }
    #[test]
    fn geometry_alone_does_not_clip_the_foreign_window() {
        let mut rule = RuleEffects::default();
        rule.geometry_corner_radius = Some([8.0; 4]);
        assert_eq!(rule.dwm_radius(), None);
        rule.clip_to_geometry = Some(true);
        assert_eq!(rule.dwm_radius(), Some(0));
        rule.geometry_corner_radius = Some([4.0, 8.0, 16.0, 0.0]);
        assert_eq!(rule.dwm_radius(), Some(0));
    }
    #[test]
    fn shadow_partial_preserves_offset_and_tab_partial_preserves_paints() {
        let mut base = Effects::default();
        base.shadow.offset_y = 17.0;
        base.tab_indicator.active = Some(PaintStyle::color("blue"));
        let mut rule = RuleEffects::default();
        rule.shadow.on = Some(true);
        rule.tab_indicator.urgent = Some(PaintStyle::color("red"));
        let result = rule.apply_to(&base);
        assert!(result.shadow.on);
        assert_eq!(result.shadow.offset_y, 17.0);
        assert_eq!(result.tab_indicator.active, base.tab_indicator.active);
        assert_eq!(result.tab_indicator.urgent, rule.tab_indicator.urgent);
    }
}
