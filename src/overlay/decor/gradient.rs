//! Linear-gradient coordinates and colour interpolation, matching niri `e19df12cf8`.
//!
//! The reference is `src/render_helpers/shaders/border.frag`: linear-light conversion
//! deliberately uses gamma 2.2 (not the IEC sRGB transfer function). Rectangular spaces
//! interpolate premultiplied components; OKLCH premultiplies only lightness and chroma,
//! leaving hue independent of alpha. RGB is clipped to the display gamut before encoding
//! into the overlay's premultiplied sRGB surface.

use crate::config::effects::{GradientHue, GradientSpace};

use super::paint::{RRect, Rgba};

/// CSS `linear-gradient(angle, from, to)` with niri's interpolation options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gradient {
    pub from: Rgba,
    pub to: Rgba,
    /// Degrees: 0 points up, 90 right, 180 down.
    pub angle: f32,
    /// Measured against the workspace view instead of the local reference rectangle.
    pub relative_to_view: bool,
    pub space: GradientSpace,
    pub hue: GradientHue,
}

impl Gradient {
    /// Premultiplied sRGB `[r, g, b, a]` at a clamped position along the gradient.
    pub fn color_at(&self, position: f32) -> [f32; 4] {
        GradientColors::new(self).sample(position)
    }
}

/// Where `(x, y)` falls along the CSS gradient line over `area`, clamped to 0..=1.
/// The line is the box diagonal's projection onto the gradient direction, as in niri's
/// `BorderRenderElement`. Shape and workspace-view gradients use the same coordinates.
pub fn gradient_position(angle_deg: f32, area: &RRect, x: f32, y: f32) -> f32 {
    if !angle_deg.is_finite() {
        return 0.0;
    }
    let angle = angle_deg.rem_euclid(360.0).to_radians();
    let (dx, cos) = angle.sin_cos();
    let dy = -cos;
    let length = (area.w * dx).abs() + (area.h * dy).abs();
    if !length.is_finite() || length <= f32::EPSILON {
        return 0.0;
    }
    let cx = area.x + area.w / 2.0;
    let cy = area.y + area.h / 2.0;
    unit(((x - cx) * dx + (y - cy) * dy) / length + 0.5)
}

/// Endpoint conversions are prepared once per paint, not once per pixel.
pub(super) struct GradientColors {
    from: [f32; 4],
    to: [f32; 4],
    space: GradientSpace,
    hue_delta: f32,
}

impl GradientColors {
    pub(super) fn new(gradient: &Gradient) -> Self {
        let convert = |color: Rgba| {
            let mut rgb = [color.r, color.g, color.b].map(|v| v as f32 / 255.0);
            if gradient.space != GradientSpace::Srgb {
                rgb = rgb.map(|v| v.powf(2.2));
            }
            if matches!(gradient.space, GradientSpace::Oklab | GradientSpace::Oklch) {
                rgb = linear_to_oklab(rgb);
            }
            if gradient.space == GradientSpace::Oklch {
                rgb = lab_to_lch(rgb);
            }
            let alpha = color.a as f32 / 255.0;
            let count = if gradient.space == GradientSpace::Oklch {
                2
            } else {
                3
            };
            for component in &mut rgb[..count] {
                *component *= alpha;
            }
            [rgb[0], rgb[1], rgb[2], alpha]
        };
        let from = convert(gradient.from);
        let to = convert(gradient.to);
        Self {
            from,
            to,
            space: gradient.space,
            hue_delta: hue_delta(from[2], to[2], gradient.hue),
        }
    }

    pub(super) fn sample(&self, position: f32) -> [f32; 4] {
        let t = unit(position);
        let mut mixed = [0.0; 4];
        for (i, component) in mixed.iter_mut().enumerate() {
            *component = self.from[i] + (self.to[i] - self.from[i]) * t;
        }
        if self.space == GradientSpace::Srgb {
            return mixed;
        }
        let alpha = mixed[3];
        if alpha <= 0.0 {
            return [0.0; 4];
        }
        let count = if self.space == GradientSpace::Oklch {
            2
        } else {
            3
        };
        for component in &mut mixed[..count] {
            *component /= alpha;
        }
        let mut rgb = [mixed[0], mixed[1], mixed[2]];
        if self.space == GradientSpace::Oklch {
            rgb[2] = (self.from[2] + self.hue_delta * t).rem_euclid(360.0);
            rgb = lch_to_lab(rgb);
        }
        if matches!(self.space, GradientSpace::Oklab | GradientSpace::Oklch) {
            rgb = oklab_to_linear(rgb);
        }
        // Clipping also avoids taking a fractional power of negative out-of-gamut RGB.
        rgb = rgb.map(|v| v.clamp(0.0, 1.0).powf(1.0 / 2.2) * alpha);
        [rgb[0], rgb[1], rgb[2], alpha]
    }
}

fn unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// niri's tie-breaking is intentional: at 180 degrees shorter takes the direct path and
/// longer the wrapped path; equal hues make a negative full turn for longer/decreasing.
fn hue_delta(from: f32, to: f32, hue: GradientHue) -> f32 {
    let direct = to - from;
    let distance = direct.abs();
    let wrapped = if from <= to {
        direct - 360.0
    } else {
        direct + 360.0
    };
    let wrap = match hue {
        GradientHue::Shorter => distance > 360.0 - distance,
        GradientHue::Longer => distance <= 360.0 - distance,
        GradientHue::Increasing => from > to,
        GradientHue::Decreasing => from <= to,
    };
    if wrap {
        wrapped
    } else {
        direct
    }
}

fn linear_to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn oklab_to_linear([l, a, b]: [f32; 3]) -> [f32; 3] {
    let ll = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (l - 0.105_561_35 * a - 0.063_854_17 * b).powi(3);
    let s = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * ll - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * ll + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * ll - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

fn lab_to_lch([l, a, b]: [f32; 3]) -> [f32; 3] {
    let hue = b.atan2(a).to_degrees();
    [l, a.hypot(b), if hue <= 0.0 { hue + 360.0 } else { hue }]
}

fn lch_to_lab([l, c, h]: [f32; 3]) -> [f32; 3] {
    let (sin, cos) = h.to_radians().sin_cos();
    [l, c * cos, c * sin]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::Rect;

    const SPACES: [GradientSpace; 4] = [
        GradientSpace::Srgb,
        GradientSpace::SrgbLinear,
        GradientSpace::Oklab,
        GradientSpace::Oklch,
    ];
    const HUES: [GradientHue; 4] = [
        GradientHue::Shorter,
        GradientHue::Longer,
        GradientHue::Increasing,
        GradientHue::Decreasing,
    ];

    fn gradient(space: GradientSpace) -> Gradient {
        Gradient {
            from: Rgba::new(255, 0, 0, 255),
            to: Rgba::new(0, 0, 255, 255),
            angle: 180.0,
            relative_to_view: false,
            space,
            hue: GradientHue::Shorter,
        }
    }

    fn close<const N: usize>(actual: [f32; N], expected: [f32; N], tolerance: f32) {
        for (a, e) in actual.into_iter().zip(expected) {
            assert!((a - e).abs() < tolerance, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn positions_use_css_angles_on_a_non_square_offset_area() {
        let area = RRect::of(Rect::new(-200, 40, 200, 100), (0, 0), 0.0);
        for (angle, start, end) in [
            (0.0, (-100.0, 140.0), (-100.0, 40.0)),
            (90.0, (-200.0, 90.0), (0.0, 90.0)),
            (180.0, (-100.0, 40.0), (-100.0, 140.0)),
            (270.0, (0.0, 90.0), (-200.0, 90.0)),
            (45.0, (-200.0, 140.0), (0.0, 40.0)),
        ] {
            close(
                [gradient_position(angle, &area, start.0, start.1)],
                [0.0],
                1e-6,
            );
            close([gradient_position(angle, &area, end.0, end.1)], [1.0], 1e-6);
            close([gradient_position(angle, &area, -100.0, 90.0)], [0.5], 1e-6);
            close(
                [gradient_position(angle + 720.0, &area, -150.0, 60.0)],
                [gradient_position(angle - 360.0, &area, -150.0, 60.0)],
                1e-6,
            );
        }
        // At 45 degrees the width contributes twice the height to the line length.
        close(
            [gradient_position(45.0, &area, -200.0, 40.0)],
            [1.0 / 3.0],
            1e-6,
        );
        assert_eq!(gradient_position(90.0, &area, -500.0, 90.0), 0.0);
        assert_eq!(gradient_position(90.0, &area, 500.0, 90.0), 1.0);
        let empty = RRect::of(Rect::default(), (0, 0), 0.0);
        assert_eq!(gradient_position(45.0, &empty, 0.0, 0.0), 0.0);
        assert_eq!(gradient_position(f32::NAN, &area, 0.0, 0.0), 0.0);
    }

    #[test]
    fn srgb_linear_matches_the_pinned_gamma_22_not_iec_srgb() {
        let mut g = gradient(GradientSpace::Srgb);
        g.from = Rgba::new(0, 0, 0, 255);
        g.to = Rgba::new(255, 255, 255, 255);
        close(g.color_at(0.5), [0.5, 0.5, 0.5, 1.0], 1e-6);
        g.space = GradientSpace::SrgbLinear;
        close(
            g.color_at(0.5),
            [0.729_740_1, 0.729_740_1, 0.729_740_1, 1.0],
            1e-6,
        );
    }

    #[test]
    fn oklab_conversion_matches_primary_reference_values_and_round_trips() {
        for (rgb, lab) in [
            ([1.0, 0.0, 0.0], [0.627_955_4, 0.224_863_1, 0.125_846_3]),
            ([0.0, 1.0, 0.0], [0.866_439_6, -0.233_887_6, 0.179_498_5]),
            ([0.0, 0.0, 1.0], [0.452_013_7, -0.032_457, -0.311_528_1]),
            ([1.0, 1.0, 1.0], [1.0, 0.0, 0.0]),
        ] {
            close(linear_to_oklab(rgb), lab, 1e-6);
            close(oklab_to_linear(linear_to_oklab(rgb)), rgb, 2e-6);
        }
        close(
            gradient(GradientSpace::Oklab).color_at(0.5),
            [0.545_6, 0.328_9, 0.630_6, 1.0],
            0.002,
        );
    }

    #[test]
    fn all_hue_paths_cross_the_seam_in_the_requested_direction() {
        for (from, to, expected) in [
            (350.0, 10.0, [20.0, -340.0, 20.0, -340.0]),
            (10.0, 350.0, [-20.0, 340.0, 340.0, -20.0]),
            (30.0, 210.0, [180.0, -180.0, 180.0, -180.0]),
            (210.0, 30.0, [-180.0, 180.0, 180.0, -180.0]),
            (120.0, 120.0, [0.0, -360.0, 0.0, -360.0]),
        ] {
            for (hue, delta) in HUES.into_iter().zip(expected) {
                assert_eq!(hue_delta(from, to, hue), delta);
            }
        }
    }

    #[test]
    fn oklch_hue_paths_produce_distinct_visible_colours() {
        let mut g = gradient(GradientSpace::Oklch);
        let shorter = g.color_at(0.5);
        assert!(shorter[0] > shorter[1] && shorter[2] > shorter[1]);
        g.hue = GradientHue::Decreasing;
        close(g.color_at(0.5), shorter, 1e-6);
        g.hue = GradientHue::Longer;
        let longer = g.color_at(0.5);
        assert!(longer[1] > longer[0] && longer[1] > longer[2]);
        g.hue = GradientHue::Increasing;
        close(g.color_at(0.5), longer, 1e-6);
    }

    #[test]
    fn rectangular_spaces_premultiply_before_interpolating() {
        for space in [
            GradientSpace::Srgb,
            GradientSpace::SrgbLinear,
            GradientSpace::Oklab,
        ] {
            let mut g = gradient(space);
            g.from.a = 0;
            close(g.color_at(0.5), [0.0, 0.0, 0.5, 0.5], 0.001);
            g.to.a = 0;
            assert_eq!(g.color_at(0.5), [0.0; 4]);
        }
    }

    #[test]
    fn oklch_hue_is_not_alpha_weighted_like_lightness_and_chroma() {
        let mut g = gradient(GradientSpace::Oklch);
        g.from.a = 0;
        let mixed = g.color_at(0.5);
        assert_eq!(mixed[3], 0.5);
        assert!(mixed[0] > 0.1 && mixed[2] > 0.1, "{mixed:?}");
        g.to.a = 0;
        assert_eq!(g.color_at(0.5), [0.0; 4]);
    }

    #[test]
    fn every_space_and_hue_preserves_endpoints_and_valid_premultiplication() {
        for space in SPACES {
            for hue in HUES {
                let g = Gradient {
                    from: Rgba::new(250, 20, 90, 128),
                    to: Rgba::new(5, 230, 250, 192),
                    hue,
                    ..gradient(space)
                };
                for (t, color) in [(0.0, g.from), (1.0, g.to)] {
                    let alpha = color.a as f32 / 255.0;
                    close(
                        g.color_at(t),
                        [
                            color.r as f32 / 255.0 * alpha,
                            color.g as f32 / 255.0 * alpha,
                            color.b as f32 / 255.0 * alpha,
                            alpha,
                        ],
                        0.002,
                    );
                }
                for t in [-1.0, 0.0, 0.2, 0.5, 0.9, 1.0, 2.0, f32::NAN] {
                    let color = g.color_at(t);
                    assert!(color[3].is_finite() && (0.0..=1.0).contains(&color[3]));
                    for component in &color[..3] {
                        assert!(component.is_finite() && (0.0..=color[3]).contains(component));
                    }
                }
            }
        }
    }
}
