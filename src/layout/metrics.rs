//! Projection of canonical logical spacing onto one output's physical pixels.
//!
//! niri e19df12 rounds nonzero gaps to at least one physical pixel, then keeps
//! fractional strut extents through working-area and proportional-size resolution.
//!
//! The same file projects the appearance metrics (border width, focus-ring width and the
//! shadow's softness, spread and offsets). Every one of them is configured, and carried by
//! window rules, in LOGICAL pixels; the engine turns them into physical pixels once, for the
//! output the window is on, so a window that moves to an output with another scale is
//! decorated again with that output's pixels.

use crate::config::{Edges, InsetValue, LayoutConfig};
use crate::utils::{Point, Rect};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PhysicalSpacing {
    pub inner_gap_px: i32,
    pub outer_gap_px: i32,
    pub struts_px: Edges<f64>,
}

/// A working area with a pixel-aligned origin and unquantized physical extents.
///
/// Size consumers must resolve proportions before rounding the final window size.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PhysicalWorkArea {
    pub loc: Point,
    pub width: f64,
    pub height: f64,
}

impl From<Rect> for PhysicalWorkArea {
    fn from(rect: Rect) -> Self {
        Self {
            loc: rect.loc,
            width: f64::from(rect.size.w),
            height: f64::from(rect.size.h),
        }
    }
}

/// A usable output scale: a non-finite, zero or negative one is treated as 96 DPI.
pub fn valid_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// A logical `border` or `focus-ring` width in whole physical pixels.
///
/// niri's `Tile::update_config` replaces the merged (layout plus window rule) width with
/// `round_logical_in_physical_max1(scale, width)` (layout/tile.rs:244,252-257;
/// utils/mod.rs:185-191): zero stays zero, any other width is at least one physical pixel,
/// and the product with the scale is rounded to the nearest pixel (halves away from zero).
/// Both the tile shrink of the border (tile.rs:818-840) and the ring's outward extent
/// (focus_ring.rs:69-70) use that rounded width.
pub fn ring_width_px(logical: f64, scale: f64) -> i32 {
    // NaN and negative widths cannot be written in niri (range 0..=65535); treat them as off.
    if logical.is_nan() || logical <= 0.0 {
        return 0;
    }
    (logical * valid_scale(scale)).max(1.0).round() as i32
}

/// A signed logical gap (the tab indicator's `gap`) in whole physical pixels.
///
/// niri projects it as `round_logical_in_physical_max1(scale, gap.abs()).copysign(gap)`
/// (layout/tab_indicator.rs:104-105): zero keeps its sign, any other value is at least one
/// physical pixel and rounded to the nearest, halves away from zero. A non-finite value is
/// handed on unchanged, so the tab geometry still refuses to lay it out.
pub fn signed_gap_px(logical: f64, scale: f64) -> f64 {
    if logical == 0.0 || !logical.is_finite() {
        return logical;
    }
    (logical.abs() * valid_scale(scale))
        .max(1.0)
        .round()
        .copysign(logical)
}

/// The shadow geometry of one tile in physical pixels, as niri's `Shadow` derives it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PhysicalShadow {
    /// The CSS-style blur diameter: the gaussian's sigma is half of it. niri keeps sigma
    /// unrounded and multiplies by the scale in the shader (shadow.rs:43-45,119).
    pub softness_px: f64,
    /// Signed spread. niri ceils its magnitude to a physical pixel (shadow.rs:50-53).
    pub spread_px: f64,
    /// Offsets, each ceiled to a physical pixel (shadow.rs:48-49).
    pub offset_x_px: f64,
    pub offset_y_px: f64,
}

/// Project the logical shadow metrics onto an output (niri layout/shadow.rs:43-61).
///
/// niri's `ceil` is `(logical * scale).ceil() / scale`, so in physical pixels the offsets
/// are `ceil(offset * scale)` and the spread is `ceil(|spread| * scale)` with the sign of
/// the configured spread; the softness is not rounded.
pub fn project_shadow(
    softness: f64,
    spread: f64,
    offset_x: f64,
    offset_y: f64,
    scale: f64,
) -> PhysicalShadow {
    let scale = valid_scale(scale);
    let finite = |value: f64| if value.is_finite() { value } else { 0.0 };
    let (softness, spread, offset_x, offset_y) = (
        finite(softness).max(0.0),
        finite(spread),
        finite(offset_x),
        finite(offset_y),
    );
    PhysicalShadow {
        softness_px: softness * scale,
        spread_px: (spread.abs() * scale).ceil().copysign(spread),
        offset_x_px: (offset_x * scale).ceil(),
        offset_y_px: (offset_y * scale).ceil(),
    }
}

/// Resolve units exactly once after global, output and workspace overrides merge.
pub fn project_spacing(config: &LayoutConfig, scale: f64) -> PhysicalSpacing {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let gap = |value: f64| {
        if value == 0.0 {
            0
        } else {
            (value * scale).max(1.0).round() as i32
        }
    };
    let inset = |value: InsetValue| match value {
        InsetValue::Logical(value) => value * scale,
        InsetValue::Physical(value) => f64::from(value),
    };
    PhysicalSpacing {
        inner_gap_px: gap(config.inner_gaps),
        outer_gap_px: gap(config.outer_gaps),
        struts_px: Edges {
            left: inset(config.struts.left),
            right: inset(config.struts.right),
            top: inset(config.struts.top),
            bottom: inset(config.struts.bottom),
        },
    }
}

/// Apply signed struts and the outer gap, snapping only the absolute origin.
///
/// As in niri's `compute_working_area`, any positive origin snap is subtracted
/// from the extent without allowing a negative size. Fractional trailing edges
/// remain available to the eventual window-size consumer.
pub fn spacing_work_rect(work_px: Rect, spacing: &PhysicalSpacing) -> PhysicalWorkArea {
    let outer = f64::from(spacing.outer_gap_px);
    let x = f64::from(work_px.loc.x) + spacing.struts_px.left + outer;
    let y = f64::from(work_px.loc.y) + spacing.struts_px.top + outer;
    let width = (f64::from(work_px.size.w)
        - spacing.struts_px.left
        - spacing.struts_px.right
        - 2.0 * outer)
        .max(0.0);
    let height = (f64::from(work_px.size.h)
        - spacing.struts_px.top
        - spacing.struts_px.bottom
        - 2.0 * outer)
        .max(0.0);
    let snapped_x = x.ceil();
    let snapped_y = y.ceil();
    PhysicalWorkArea {
        loc: Point::new(snapped_x as i32, snapped_y as i32),
        width: width - (snapped_x - x).min(width),
        height: height - (snapped_y - y).min(height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// niri projects a tab indicator's `gap` with the sign kept and a one-pixel minimum
    /// (layout/tab_indicator.rs:104-105).
    #[test]
    fn parity_signed_gap_projection_keeps_its_sign_and_one_pixel_minimum() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            for logical in [0.0, -0.0, 0.2, -0.2, 0.5, 1.0, 1.5, 2.5, -2.5, 5.0, 12.25] {
                let px = signed_gap_px(logical, scale);
                let reference = if logical == 0.0 {
                    logical
                } else {
                    (logical.abs() * scale).max(1.0).round().copysign(logical)
                };
                assert_eq!(px, reference, "gap {logical} at scale {scale}");
                assert_eq!(
                    px.is_sign_negative(),
                    logical.is_sign_negative(),
                    "the sign survives: {logical} at scale {scale}"
                );
            }
        }
        // Hand-checked pins: halves go away from zero, a non-zero gap is a pixel at least.
        assert_eq!(signed_gap_px(5.0, 2.0), 10.0);
        assert_eq!(signed_gap_px(-0.2, 1.0), -1.0);
        assert_eq!(signed_gap_px(-2.5, 1.5), -4.0);
        assert_eq!(signed_gap_px(0.0, 2.0), 0.0);
        assert!(signed_gap_px(-0.0, 2.0).is_sign_negative());
        // A broken scale is 96 DPI; a non-finite gap is handed on so no geometry uses it.
        assert_eq!(signed_gap_px(5.0, 0.0), 5.0);
        assert!(signed_gap_px(f64::NAN, 2.0).is_nan());
        assert_eq!(signed_gap_px(f64::INFINITY, 2.0), f64::INFINITY);
    }

    #[test]
    fn parity_fractional_spacing_projection_preserves_units_and_minimum_gap() {
        let mut config = LayoutConfig::default_config();
        config.inner_gaps = 2.5;
        config.outer_gaps = 0.1;
        config.struts = Edges {
            left: InsetValue::Logical(-4.5),
            right: InsetValue::Logical(0.25),
            top: InsetValue::Physical(10),
            bottom: InsetValue::Physical(0),
        };
        let projected = project_spacing(&config, 1.5);
        assert_eq!((projected.inner_gap_px, projected.outer_gap_px), (4, 1));
        assert_eq!(
            projected.struts_px,
            Edges {
                left: -6.75,
                right: 0.375,
                top: 10.0,
                bottom: 0.0,
            }
        );
        config.inner_gaps = 0.0;
        config.outer_gaps = 0.0;
        let projected = project_spacing(&config, 1.5);
        assert_eq!((projected.inner_gap_px, projected.outer_gap_px), (0, 0));
    }

    #[test]
    fn parity_fractional_spacing_invalid_scale_uses_one() {
        let mut config = LayoutConfig::default_config();
        config.inner_gaps = 2.5;
        config.struts.left = InsetValue::Logical(-4.5);
        let expected = project_spacing(&config, 1.0);
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(project_spacing(&config, scale), expected);
        }
    }

    #[test]
    fn parity_fractional_spacing_work_area_retains_right_and_bottom_fractions() {
        let mut config = LayoutConfig::default_config();
        config.inner_gaps = 0.0;
        config.outer_gaps = 0.0;
        config.struts = Edges {
            left: InsetValue::Logical(-4.5),
            right: InsetValue::Logical(0.25),
            top: InsetValue::Logical(-2.5),
            bottom: InsetValue::Logical(0.75),
        };
        let work = spacing_work_rect(
            Rect::new(-300, -100, 900, 600),
            &project_spacing(&config, 1.0),
        );
        assert_eq!(work.loc, Point::new(-304, -102));
        assert_eq!((work.width, work.height), (903.75, 601.25));
        assert_eq!((work.width * 0.8).floor(), 723.0);
        let work = spacing_work_rect(
            Rect::new(-300, -100, 900, 600),
            &project_spacing(&config, 1.5),
        );
        assert_eq!(work.loc, Point::new(-306, -103));
        assert_eq!((work.width, work.height), (905.625, 601.875));
    }

    #[test]
    fn parity_fractional_spacing_exhausted_area_stays_nonnegative() {
        let spacing = PhysicalSpacing {
            struts_px: Edges {
                left: 0.5,
                right: 5.0,
                top: 0.75,
                bottom: 5.0,
            },
            ..PhysicalSpacing::default()
        };
        let work = spacing_work_rect(Rect::new(-3, -4, 2, 2), &spacing);
        assert_eq!(work.loc, Point::new(-2, -3));
        assert_eq!((work.width, work.height), (0.0, 0.0));
    }
}
