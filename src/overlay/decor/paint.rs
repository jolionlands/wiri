//! A small software rasterizer for the decorations: anti-aliased rounded rectangles, rings,
//! gaussian shadows and CSS-style linear gradients, drawn into a premultiplied BGRA surface.
//!
//! No Win32 here: the layer hands in a slice (a DIB section's memory) and gets it back drawn,
//! and the tests look at pixels.  A pixel is a `u32` laid out `0xAARRGGBB` (the bytes B, G, R, A
//! in memory, which is what `UpdateLayeredWindow` with `AC_SRC_ALPHA` reads), with the colour
//! channels already multiplied by the alpha.
//!
//! Only the pixels that can change are visited: the inside of a window is left alone (the
//! window covers it), and the straight stretches of a ring are filled row by row.

use crate::utils::Rect;

/// A colour with straight (not premultiplied) 8-bit channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_array(c: [u8; 4]) -> Self {
        Self::new(c[0], c[1], c[2], c[3])
    }

    /// The colour as premultiplied floats `[r, g, b, a]`, each 0 to 1.
    fn premultiplied(self) -> [f32; 4] {
        let a = self.a as f32 / 255.0;
        [
            self.r as f32 / 255.0 * a,
            self.g as f32 / 255.0 * a,
            self.b as f32 / 255.0 * a,
            a,
        ]
    }

    /// The same colour with its opacity multiplied by `factor`.
    pub fn faded(self, factor: f32) -> Self {
        Self {
            a: ((self.a as f32) * factor.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }
}

/// CSS `linear-gradient(angle, from, to)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gradient {
    pub from: Rgba,
    pub to: Rgba,
    /// Degrees: 0 points up, 90 right, 180 down.
    pub angle: f32,
    /// Measured against the workspace view instead of the shape that is painted.
    pub relative_to_view: bool,
}

/// What a shape is filled with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Solid(Rgba),
    Gradient(Gradient),
}

impl Paint {
    /// The paint with its opacity multiplied by `factor` (an inactive tab is dimmed).
    pub fn faded(self, factor: f32) -> Paint {
        match self {
            Paint::Solid(c) => Paint::Solid(c.faded(factor)),
            Paint::Gradient(g) => Paint::Gradient(Gradient {
                from: g.from.faded(factor),
                to: g.to.faded(factor),
                ..g
            }),
        }
    }

    /// True when the paint draws nothing.
    pub fn is_invisible(&self) -> bool {
        match self {
            Paint::Solid(c) => c.a == 0,
            Paint::Gradient(g) => g.from.a == 0 && g.to.a == 0,
        }
    }

    /// The premultiplied colour at the pixel centre `(x, y)`, in the coordinates of the
    /// surface.  `shape` is the rectangle the gradient is measured against when it is
    /// relative to the shape, `view` the one it is measured against otherwise.
    fn sample(&self, x: f32, y: f32, shape: &RRect, view: &RRect) -> [f32; 4] {
        match self {
            Paint::Solid(c) => c.premultiplied(),
            Paint::Gradient(g) => {
                let area = if g.relative_to_view { view } else { shape };
                let t = gradient_position(g.angle, area, x, y);
                let a = g.from.premultiplied();
                let b = g.to.premultiplied();
                [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                    a[3] + (b[3] - a[3]) * t,
                ]
            }
        }
    }
}

/// Where `(x, y)` falls along the gradient line of a CSS `linear-gradient(angle, ...)` over
/// `area`, 0 at the start and 1 at the end (clamped).
pub fn gradient_position(angle_deg: f32, area: &RRect, x: f32, y: f32) -> f32 {
    let a = angle_deg.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    // The gradient line is as long as the projection of the box on its direction.
    let length = (area.w * dx).abs() + (area.h * dy).abs();
    if length <= f32::EPSILON {
        return 0.0;
    }
    let cx = area.x + area.w / 2.0;
    let cy = area.y + area.h / 2.0;
    (((x - cx) * dx + (y - cy) * dy) / length + 0.5).clamp(0.0, 1.0)
}

/// A rectangle with a radius for each corner, in surface coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Top-left, top-right, bottom-right, bottom-left.
    pub r: [f32; 4],
}

impl RRect {
    /// `rect` (screen pixels) as a rectangle of the surface whose top-left corner is at
    /// `origin` on the screen, with the same radius at every corner.
    pub fn of(rect: Rect, origin: (i32, i32), radius: f32) -> Self {
        Self::of_corners(rect, origin, [radius; 4])
    }

    /// As [`Self::of`], with top-left, top-right, bottom-right and bottom-left radii.
    pub fn of_corners(rect: Rect, origin: (i32, i32), radii: [f32; 4]) -> Self {
        let mut rr = RRect {
            x: (rect.loc.x - origin.0) as f32,
            y: (rect.loc.y - origin.1) as f32,
            w: rect.size.w as f32,
            h: rect.size.h as f32,
            r: radii,
        };
        rr.fit();
        rr
    }

    /// CSS corner-overlap reduction, as in niri's `CornerRadius::fit_to`: scale all radii
    /// together until the two on each edge fit. A single corner may exceed half a side.
    fn fit(&mut self) {
        for r in &mut self.r {
            *r = if r.is_finite() { r.max(0.0) } else { 0.0 };
        }
        let [tl, tr, br, bl] = self.r;
        let mut scale = 1.0f32;
        for (side, sum) in [
            (self.w, tl + tr),
            (self.w, bl + br),
            (self.h, tl + bl),
            (self.h, tr + br),
        ] {
            if sum > 0.0 {
                scale = scale.min(side.max(0.0) / sum);
            }
        }
        self.r = self.r.map(|r| r * scale);
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Grown by `d` on every side (shrunk when negative); a corner that is round stays round
    /// by the same amount, a square one stays square.
    pub fn outset(&self, d: f32) -> RRect {
        let mut out = RRect {
            x: self.x - d,
            y: self.y - d,
            w: (self.w + 2.0 * d).max(0.0),
            h: (self.h + 2.0 * d).max(0.0),
            r: self.r.map(|r| if r > 0.0 { (r + d).max(0.0) } else { 0.0 }),
        };
        out.fit();
        out
    }

    /// Moved by `(dx, dy)`.
    pub fn moved(&self, dx: f32, dy: f32) -> RRect {
        RRect {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }

    /// Signed distance from the point to the outline: negative inside, positive outside.
    pub fn sdf(&self, px: f32, py: f32) -> f32 {
        let x = px - self.x;
        let y = py - self.y;
        let ax = (-x).max(x - self.w);
        let ay = (-y).max(y - self.h);
        let mut distance = ax.max(0.0).hypot(ay.max(0.0)) + ax.max(ay).min(0.0);
        // Test the corner squares, not the rectangle's quadrants: unequal CSS radii can
        // cross its centre. Opposite corner squares can overlap, so consider all four.
        for (r, dx, dy) in [
            (self.r[0], x, y),
            (self.r[1], self.w - x, y),
            (self.r[2], self.w - x, self.h - y),
            (self.r[3], x, self.h - y),
        ] {
            if r > 0.0 && dx < r && dy < r {
                distance = distance.max((dx - r).hypot(dy - r) - r);
            }
        }
        distance
    }

    /// How much of the pixel centred on `(px, py)` the shape covers, 0 to 1.
    pub fn coverage(&self, px: f32, py: f32) -> f32 {
        (0.5 - self.sdf(px, py)).clamp(0.0, 1.0)
    }

    /// The largest corner radius.
    fn max_radius(&self) -> f32 {
        self.r.iter().cloned().fold(0.0, f32::max)
    }
}

/// A BGRA premultiplied surface.
pub struct Surface<'a> {
    pub w: i32,
    pub h: i32,
    pub px: &'a mut [u32],
}

impl<'a> Surface<'a> {
    pub fn new(w: i32, h: i32, px: &'a mut [u32]) -> Self {
        debug_assert!(px.len() >= (w.max(0) as usize) * (h.max(0) as usize));
        Self { w, h, px }
    }

    pub fn clear(&mut self) {
        let n = (self.w.max(0) as usize) * (self.h.max(0) as usize);
        for p in self.px[..n].iter_mut() {
            *p = 0;
        }
    }

    fn index(&self, x: i32, y: i32) -> usize {
        (y as usize) * (self.w as usize) + x as usize
    }

    /// Composite the premultiplied colour `src` over the pixel.
    fn over(&mut self, x: i32, y: i32, src: [f32; 4]) {
        if src[3] <= 0.0 {
            return;
        }
        let i = self.index(x, y);
        let d = self.px[i];
        let inv = 1.0 - src[3];
        let da = ((d >> 24) & 0xff) as f32 / 255.0;
        let dr = ((d >> 16) & 0xff) as f32 / 255.0;
        let dg = ((d >> 8) & 0xff) as f32 / 255.0;
        let db = (d & 0xff) as f32 / 255.0;
        let a = (src[3] + da * inv).clamp(0.0, 1.0);
        let r = (src[0] + dr * inv).clamp(0.0, a);
        let g = (src[1] + dg * inv).clamp(0.0, a);
        let b = (src[2] + db * inv).clamp(0.0, a);
        let ai = (a * 255.0).round() as u32;
        // Rounding must not push a colour channel above the alpha: the compositor would
        // brighten the pixel.
        let ri = ((r * 255.0).round() as u32).min(ai);
        let gi = ((g * 255.0).round() as u32).min(ai);
        let bi = ((b * 255.0).round() as u32).min(ai);
        self.px[i] = (ai << 24) | (ri << 16) | (gi << 8) | bi;
    }

    /// The pixel as `[r, g, b, a]` (premultiplied, 0 to 255): for the tests and the checks of
    /// the layer.
    pub fn get(&self, x: i32, y: i32) -> [u8; 4] {
        let p = self.px[self.index(x, y)];
        [
            ((p >> 16) & 0xff) as u8,
            ((p >> 8) & 0xff) as u8,
            (p & 0xff) as u8,
            ((p >> 24) & 0xff) as u8,
        ]
    }
}

/// `erfc(x)`, to about 1.5e-7 (Abramowitz and Stegun 7.1.26).
pub fn erfc(x: f32) -> f32 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t
        * (0.254_829_6
            + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    let e = poly * (-z * z).exp();
    if x >= 0.0 {
        e
    } else {
        2.0 - e
    }
}

/// How far from the edge of a shape its shadow can be seen.
pub fn shadow_reach(softness: f32, spread: f32, offset_x: f32, offset_y: f32) -> i32 {
    let sigma = softness.max(0.0) / 2.0;
    (3.0 * sigma + spread.max(0.0) + offset_x.abs().max(offset_y.abs())).ceil() as i32
}

/// Paint `rr` (the shape of the ring's inner edge is `inner`) and everything between: a
/// filled shape when `inner` is `None`.  The visited area is clipped to the surface.
fn visit_rows(surf: &Surface, outer: &RRect, skip: Option<&RRect>, mut f: impl FnMut(i32, i32)) {
    let margin = 1.0;
    let y0 = ((outer.y - margin).floor() as i32).max(0);
    let y1 = ((outer.bottom() + margin).ceil() as i32).min(surf.h);
    let x0 = ((outer.x - margin).floor() as i32).max(0);
    let x1 = ((outer.right() + margin).ceil() as i32).min(surf.w);
    for y in y0..y1 {
        let yc = y as f32 + 0.5;
        // The straight stretch of the skipped shape: its inside is fully covered there, so
        // the pixels between its edges need no visit.
        let (skip_from, skip_to) = match skip {
            Some(s) => {
                let r = s.max_radius();
                if yc >= s.y + r && yc <= s.bottom() - r {
                    ((s.x + 1.0).ceil() as i32, (s.right() - 1.0).floor() as i32)
                } else {
                    (i32::MAX, i32::MIN)
                }
            }
            None => (i32::MAX, i32::MIN),
        };
        let mut x = x0;
        while x < x1 {
            if x >= skip_from && x < skip_to {
                x = skip_to;
                continue;
            }
            f(x, y);
            x += 1;
        }
    }
}

/// What a frame is made of.
pub struct FramePaint<'a> {
    /// The tile: the window and the border around it.
    pub tile: RRect,
    /// The window itself (the inside of the border band).
    pub window: RRect,
    /// The ring around the tile (`None`: no ring).
    pub ring: Option<(f32, &'a Paint)>,
    /// The border band (`None`: no border).
    pub border: Option<&'a Paint>,
    /// The workspace view, for gradients relative to it.
    pub view: RRect,
}

/// Draw the border band and the focus ring.  The band is the area between `window` and
/// `tile`, the ring the area between `tile` and `tile` grown by the ring width; both end in
/// the same pixel where they meet, so there is no seam.
pub fn draw_frame(surf: &mut Surface, f: &FramePaint) {
    let ring_w = f.ring.map_or(0.0, |(w, _)| w);
    let outer = f.tile.outset(ring_w);
    if outer.w <= 0.0 || outer.h <= 0.0 {
        return;
    }
    let mut cells: Vec<(i32, i32, [f32; 4])> = Vec::new();
    visit_rows(surf, &outer, Some(&f.window), |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let cov_out = outer.coverage(px, py);
        if cov_out <= 0.0 {
            return;
        }
        let cov_tile = f.tile.coverage(px, py);
        let cov_win = f.window.coverage(px, py);
        let mut color = [0.0f32; 4];
        if let Some((_, ring)) = f.ring {
            let share = (cov_out - cov_tile).max(0.0);
            if share > 0.0 {
                let c = ring.sample(px, py, &outer, &f.view);
                for i in 0..4 {
                    color[i] += c[i] * share;
                }
            }
        }
        if let Some(border) = f.border {
            let share = (cov_tile - cov_win).max(0.0);
            if share > 0.0 {
                let c = border.sample(px, py, &f.tile, &f.view);
                for i in 0..4 {
                    color[i] += c[i] * share;
                }
            }
        }
        if color[3] > 0.0 {
            cells.push((x, y, color));
        }
    });
    for (x, y, c) in cells {
        surf.over(x, y, c);
    }
}

/// A box shadow like CSS: the box (`shape`, already grown by the spread and moved by the
/// offset) blurred by a gaussian whose sigma is half the softness.  `hole` is cut out so that
/// the shadow shows around the window only, unless `behind_window`.
pub fn draw_shadow(
    surf: &mut Surface,
    shape: &RRect,
    hole: &RRect,
    softness: f32,
    color: Rgba,
    behind_window: bool,
) {
    let sigma = softness.max(0.0) / 2.0;
    let reach = (3.0 * sigma).ceil() + 1.0;
    let area = shape.outset(reach);
    let src = color.premultiplied();
    let skip = if behind_window { None } else { Some(hole) };
    let mut cells: Vec<(i32, i32, [f32; 4])> = Vec::new();
    visit_rows(surf, &area, skip, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let d = shape.sdf(px, py);
        let mut a = if sigma < 0.05 {
            (0.5 - d).clamp(0.0, 1.0)
        } else {
            0.5 * erfc(d / (sigma * std::f32::consts::SQRT_2))
        };
        if !behind_window {
            a *= 1.0 - hole.coverage(px, py);
        }
        if a > 0.002 {
            cells.push((x, y, [src[0] * a, src[1] * a, src[2] * a, src[3] * a]));
        }
    });
    for (x, y, c) in cells {
        surf.over(x, y, c);
    }
}

/// Fill a rounded shape with a paint.
pub fn fill(surf: &mut Surface, shape: &RRect, paint: &Paint, view: &RRect) {
    if paint.is_invisible() {
        return;
    }
    let mut cells: Vec<(i32, i32, [f32; 4])> = Vec::new();
    visit_rows(surf, shape, None, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let cov = shape.coverage(px, py);
        if cov > 0.0 {
            let c = paint.sample(px, py, shape, view);
            cells.push((x, y, [c[0] * cov, c[1] * cov, c[2] * cov, c[3] * cov]));
        }
    });
    for (x, y, c) in cells {
        surf.over(x, y, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba = Rgba::new(255, 0, 0, 255);
    const BLUE: Rgba = Rgba::new(0, 0, 255, 255);

    fn canvas(w: i32, h: i32) -> Vec<u32> {
        vec![0u32; (w * h) as usize]
    }

    #[test]
    fn a_rounded_rectangle_has_signed_distance_zero_on_its_edge() {
        let rr = RRect::of(Rect::new(10, 10, 100, 60), (0, 0), 12.0);
        // Middle of the left edge.
        assert!(rr.sdf(10.0, 40.0).abs() < 1e-4);
        // Inside by 5 px.
        assert!((rr.sdf(15.0, 40.0) + 5.0).abs() < 1e-4);
        // Outside by 3 px.
        assert!((rr.sdf(7.0, 40.0) - 3.0).abs() < 1e-4);
        // The corner: the point at the square corner is outside the round one.
        let corner = rr.sdf(10.0, 10.0);
        assert!(corner > 4.0 && corner < 6.0, "corner distance {}", corner);
    }

    #[test]
    fn a_radius_is_never_more_than_half_the_shorter_side() {
        let rr = RRect::of(Rect::new(0, 0, 40, 20), (0, 0), 100.0);
        assert_eq!(rr.r, [10.0; 4]);
    }

    #[test]
    fn growing_a_round_shape_keeps_its_corners_concentric_and_a_square_one_square() {
        let round = RRect::of(Rect::new(20, 20, 100, 100), (0, 0), 8.0).outset(4.0);
        assert_eq!(round.r, [12.0; 4]);
        assert_eq!(
            (round.x, round.y, round.w, round.h),
            (16.0, 16.0, 108.0, 108.0)
        );
        let square = RRect::of(Rect::new(20, 20, 100, 100), (0, 0), 0.0).outset(4.0);
        assert_eq!(square.r, [0.0; 4]);
    }

    #[test]
    fn unequal_corners_are_fitted_proportionally_without_rounding_square_ones() {
        let rr = RRect::of_corners(Rect::new(10, 20, 100, 60), (5, 10), [80.0, 40.0, 0.0, 20.0]);
        // The left edge is limiting: (80 + 20) * 0.6 = 60.
        assert_eq!((rr.x, rr.y), (5.0, 10.0));
        for (actual, expected) in rr.r.into_iter().zip([48.0, 24.0, 0.0, 12.0]) {
            assert!((actual - expected).abs() < 1e-5);
        }
        assert_eq!(rr.outset(4.0).r, [52.0, 28.0, 0.0, 16.0]);
        assert_eq!(rr.outset(-30.0).r, [0.0; 4]);
    }

    #[test]
    fn a_large_single_corner_is_not_clamped_to_half_a_side() {
        let rr = RRect::of_corners(Rect::new(0, 0, 100, 20), (0, 0), [20.0, 0.0, 0.0, 0.0]);
        assert_eq!(rr.r, [20.0, 0.0, 0.0, 0.0]);
        // This arc crosses the vertical centre of the rectangle.
        assert!((rr.sdf(0.0, 20.0)).abs() < 1e-5);
        assert!((rr.sdf(5.0, 15.0) - (15.0f32.hypot(5.0) - 20.0)).abs() < 1e-5);
        assert_eq!(rr.coverage(99.5, 0.5), 1.0, "top-right stays square");
    }

    #[test]
    fn overlapping_opposite_corner_squares_use_both_arcs() {
        let rr = RRect::of_corners(Rect::new(0, 0, 100, 100), (0, 0), [80.0, 0.0, 80.0, 0.0]);
        assert!(rr.sdf(21.0, 21.0) > 0.0);
        assert!(rr.sdf(79.0, 79.0) > 0.0);
        assert!(rr.sdf(50.0, 50.0) < 0.0);
        assert_eq!(rr.coverage(99.5, 0.5), 1.0);
        assert_eq!(rr.coverage(0.5, 99.5), 1.0);
    }

    #[test]
    fn erfc_matches_known_values() {
        assert!((erfc(0.0) - 1.0).abs() < 1e-6);
        assert!((erfc(1.0) - 0.157_299_2).abs() < 1e-5);
        assert!((erfc(-1.0) - 1.842_700_8).abs() < 1e-5);
        assert!(erfc(4.0) < 1e-6);
    }

    #[test]
    fn a_ring_is_painted_outside_the_window_and_the_inside_is_left_alone() {
        let (w, h) = (80, 60);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let window = RRect::of(Rect::new(10, 10, 60, 40), (0, 0), 0.0);
        let frame = FramePaint {
            tile: window,
            window,
            ring: Some((4.0, &Paint::Solid(RED))),
            border: None,
            view: window,
        };
        draw_frame(&mut surf, &frame);
        // The middle of the ring, left of the window: solid red.
        assert_eq!(surf.get(7, 30), [255, 0, 0, 255]);
        // Inside the window: untouched.
        assert_eq!(surf.get(40, 30), [0, 0, 0, 0]);
        // Well outside the ring: untouched.
        assert_eq!(surf.get(2, 30), [0, 0, 0, 0]);
        assert_eq!(surf.get(40, 3), [0, 0, 0, 0]);
        // Above the window: the ring's thickness is 4 px.
        assert_eq!(surf.get(40, 6), [255, 0, 0, 255]);
        assert_eq!(surf.get(40, 5), [0, 0, 0, 0]);
    }

    #[test]
    fn the_border_band_fills_the_space_between_the_window_and_the_tile() {
        let (w, h) = (80, 60);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let tile = RRect::of(Rect::new(10, 10, 60, 40), (0, 0), 0.0);
        let window = tile.outset(-4.0);
        let frame = FramePaint {
            tile,
            window,
            ring: None,
            border: Some(&Paint::Solid(BLUE)),
            view: tile,
        };
        draw_frame(&mut surf, &frame);
        assert_eq!(surf.get(11, 30), [0, 0, 255, 255]);
        assert_eq!(surf.get(13, 30), [0, 0, 255, 255]);
        assert_eq!(surf.get(14, 30), [0, 0, 0, 0], "the window starts here");
        assert_eq!(surf.get(9, 30), [0, 0, 0, 0], "outside the tile");
    }

    #[test]
    fn a_ring_and_a_border_meet_without_a_seam() {
        let (w, h) = (90, 70);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let tile = RRect::of(Rect::new(15, 15, 60, 40), (0, 0), 10.0);
        let window = tile.outset(-4.0);
        let frame = FramePaint {
            tile,
            window,
            ring: Some((3.0, &Paint::Solid(RED))),
            border: Some(&Paint::Solid(BLUE)),
            view: tile,
        };
        draw_frame(&mut surf, &frame);
        // Along a straight edge every pixel from the ring's outer edge to the window is opaque.
        for x in 12..19 {
            assert_eq!(surf.get(x, 35)[3], 255, "pixel {} should be opaque", x);
        }
        // Red outside the tile edge (x < 15), blue inside it.
        assert_eq!(surf.get(13, 35), [255, 0, 0, 255]);
        assert_eq!(surf.get(16, 35), [0, 0, 255, 255]);
    }

    #[test]
    fn no_pixel_has_a_colour_above_its_alpha() {
        let (w, h) = (120, 90);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let tile = RRect::of(Rect::new(20, 20, 80, 50), (0, 0), 9.0);
        let window = tile.outset(-4.0);
        let ring = Paint::Gradient(Gradient {
            from: Rgba::new(255, 200, 100, 200),
            to: Rgba::new(10, 20, 255, 90),
            angle: 45.0,
            relative_to_view: false,
        });
        let frame = FramePaint {
            tile,
            window,
            ring: Some((5.0, &ring)),
            border: Some(&Paint::Solid(Rgba::new(250, 250, 250, 128))),
            view: tile,
        };
        draw_frame(&mut surf, &frame);
        let shape = tile.moved(0.0, 4.0);
        draw_shadow(
            &mut surf,
            &shape,
            &tile,
            10.0,
            Rgba::new(0, 0, 0, 120),
            false,
        );
        for y in 0..h {
            for x in 0..w {
                let p = surf.get(x, y);
                assert!(
                    p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3],
                    "({}, {}) = {:?}",
                    x,
                    y,
                    p
                );
            }
        }
    }

    #[test]
    fn a_round_window_leaves_the_corner_of_the_ring_partly_covered() {
        let (w, h) = (100, 100);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let window = RRect::of(Rect::new(20, 20, 60, 60), (0, 0), 12.0);
        let frame = FramePaint {
            tile: window,
            window,
            ring: Some((4.0, &Paint::Solid(RED))),
            border: None,
            view: window,
        };
        draw_frame(&mut surf, &frame);
        // On the diagonal at the top-left corner: the ring fills the space between the round
        // window and the (round) outer edge, so the cut corner of the window is red.
        let on_ring = surf.get(21, 21);
        assert_eq!(
            on_ring,
            [255, 0, 0, 255],
            "the cut corner of the window is ring"
        );
        let outer_corner = surf.get(16, 16);
        assert_eq!(outer_corner[3], 0, "the outer corner is rounded off");
        let mid_edge = surf.get(17, 50);
        assert_eq!(mid_edge, [255, 0, 0, 255]);
    }

    #[test]
    fn a_shadow_is_darkest_next_to_the_window_and_fades_away() {
        let (w, h) = (160, 140);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let window = RRect::of(Rect::new(40, 40, 80, 60), (0, 0), 0.0);
        draw_shadow(
            &mut surf,
            &window,
            &window,
            20.0,
            Rgba::new(0, 0, 0, 200),
            false,
        );
        let near = surf.get(38, 70)[3];
        let mid = surf.get(30, 70)[3];
        let far = surf.get(10, 70)[3];
        assert!(near > mid && mid > far, "{} > {} > {}", near, mid, far);
        // Half the shadow's opacity sits on the edge of the box.
        assert!((near as i32 - 100).abs() < 20, "next to the edge: {}", near);
        // The window itself is not shadowed.
        assert_eq!(surf.get(80, 70), [0, 0, 0, 0]);
    }

    #[test]
    fn a_shadow_can_be_drawn_behind_the_window_too() {
        let (w, h) = (160, 140);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let window = RRect::of(Rect::new(40, 40, 80, 60), (0, 0), 0.0);
        draw_shadow(
            &mut surf,
            &window,
            &window,
            20.0,
            Rgba::new(0, 0, 0, 200),
            true,
        );
        assert!(surf.get(80, 70)[3] > 150);
    }

    #[test]
    fn gradients_follow_the_css_angle() {
        let area = RRect::of(Rect::new(0, 0, 100, 100), (0, 0), 0.0);
        // 180 degrees (the default) runs from top to bottom.
        assert!(gradient_position(180.0, &area, 50.0, 1.0) < 0.02);
        assert!(gradient_position(180.0, &area, 50.0, 99.0) > 0.98);
        assert!((gradient_position(180.0, &area, 10.0, 50.0) - 0.5).abs() < 0.01);
        // 90 degrees runs left to right.
        assert!(gradient_position(90.0, &area, 1.0, 50.0) < 0.02);
        assert!(gradient_position(90.0, &area, 99.0, 50.0) > 0.98);
        // 45 degrees runs towards the top right corner.
        assert!(gradient_position(45.0, &area, 0.0, 100.0) < 0.01);
        assert!(gradient_position(45.0, &area, 100.0, 0.0) > 0.99);
    }

    #[test]
    fn a_gradient_ring_changes_colour_along_its_length() {
        let (w, h) = (140, 60);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let window = RRect::of(Rect::new(10, 10, 120, 40), (0, 0), 0.0);
        let ring = Paint::Gradient(Gradient {
            from: RED,
            to: BLUE,
            angle: 90.0,
            relative_to_view: false,
        });
        let frame = FramePaint {
            tile: window,
            window,
            ring: Some((4.0, &ring)),
            border: None,
            view: window,
        };
        draw_frame(&mut surf, &frame);
        let left = surf.get(20, 7);
        let right = surf.get(120, 7);
        assert!(left[0] > left[2], "red end: {:?}", left);
        assert!(right[2] > right[0], "blue end: {:?}", right);
    }

    #[test]
    fn a_gradient_relative_to_the_view_ignores_the_shape_it_paints() {
        let view = RRect::of(Rect::new(0, 0, 200, 100), (0, 0), 0.0);
        let g = Paint::Gradient(Gradient {
            from: RED,
            to: BLUE,
            angle: 90.0,
            relative_to_view: true,
        });
        let small = RRect::of(Rect::new(150, 10, 30, 30), (0, 0), 0.0);
        let c = g.sample(165.0, 25.0, &small, &view);
        // 165 of 200 along the view: mostly blue.
        assert!(c[2] > c[0], "{:?}", c);
    }

    #[test]
    fn fill_paints_the_inside_of_a_round_shape_and_not_its_cut_corners() {
        let (w, h) = (60, 40);
        let mut px = canvas(w, h);
        let mut surf = Surface::new(w, h, &mut px);
        let rr = RRect::of(Rect::new(5, 5, 50, 30), (0, 0), 10.0);
        let hint = Paint::Solid(Rgba::new(127, 200, 255, 128));
        fill(&mut surf, &rr, &hint, &rr);
        let mid = surf.get(30, 20);
        assert_eq!(mid[3], 128);
        assert_eq!(surf.get(5, 5)[3], 0, "the corner is cut off");
    }

    #[test]
    fn faded_paint_has_less_opacity() {
        let p = Paint::Solid(Rgba::new(255, 0, 0, 200)).faded(0.5);
        assert_eq!(p, Paint::Solid(Rgba::new(255, 0, 0, 100)));
        assert!(Paint::Solid(Rgba::TRANSPARENT).is_invisible());
        assert!(!p.is_invisible());
    }

    #[test]
    fn the_shadow_reaches_three_sigma_plus_spread_and_offset() {
        assert_eq!(shadow_reach(30.0, 5.0, 0.0, 5.0), 55);
        assert_eq!(shadow_reach(0.0, 0.0, 0.0, 0.0), 0);
        assert_eq!(shadow_reach(10.0, -3.0, -2.0, 1.0), 17);
    }
}
