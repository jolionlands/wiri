//! What the overlay layer is asked to draw, as plain data: one frame (shadow, border band and
//! focus ring) per decorated window, one tab indicator per tabbed column and one insert hint.
//!
//! A [`DecorSpec`] knows its own bounding rectangle and how to paint itself into a surface, so
//! the Win32 side only has to keep a layered window of that size where the rectangle is and
//! call [`DecorSpec::render`].  The engine builds the specs (`layout/engine/decor.rs`).

use super::paint::{
    draw_frame, draw_shadow, fill, fill_relative, shadow_reach, FramePaint, Gradient, Paint, RRect,
    Rgba, Surface,
};
use crate::config::{GradientStyle, PaintStyle, TabPosition};
use crate::utils::{Point, Rect, WindowId};

/// The `output` key of the specs that hooks set (the insert hint): they do not belong to the
/// layout pass of any monitor.
pub const TRANSIENT_OUTPUT: u64 = u64::MAX;

/// Which kind of decoration a window of the layer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DecorKind {
    Frame,
    Tabs,
    Hint,
    /// niri `fullscreen-backdrop-color`: a filled rectangle behind a fullscreen window.
    Backdrop,
}

/// Names a decoration across updates: the layer keeps one window per id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DecorId {
    pub kind: DecorKind,
    /// The window handle for a frame, the handle of the column's first window for tabs, 0 for
    /// the hint.
    pub key: isize,
}

/// A box shadow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowSpec {
    pub softness: f32,
    pub spread: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub color: Rgba,
    pub draw_behind_window: bool,
}

/// The shadow, border band and focus ring of one window.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSpec {
    /// The tile: the window and the border band around it, in screen pixels.
    pub tile: Rect,
    /// The width of the border band; the window is the tile shrunk by this on every side.
    pub band: i32,
    /// The legacy uniform radius of the window's own corners, used when `corners` is `None`.
    pub radius: f32,
    /// Top-left, top-right, bottom-right and bottom-left window radii, overriding `radius`.
    pub corners: Option<[f32; 4]>,
    pub border: Option<Paint>,
    pub ring_width: i32,
    pub ring: Option<Paint>,
    pub shadow: Option<ShadowSpec>,
    /// The workspace view, for gradients that are relative to it.
    pub view: Rect,
}

fn expand(rect: Rect, by: i32) -> Rect {
    let w = (rect.size.w as i64 + 2 * by as i64).max(0) as u32;
    let h = (rect.size.h as i64 + 2 * by as i64).max(0) as u32;
    Rect::new(rect.loc.x - by, rect.loc.y - by, w, h)
}

fn translated(rect: Rect, dx: i32, dy: i32) -> Rect {
    Rect::new(rect.loc.x + dx, rect.loc.y + dy, rect.size.w, rect.size.h)
}

impl FrameSpec {
    /// The window itself.
    pub fn window(&self) -> Rect {
        expand(self.tile, -self.band.max(0))
    }

    /// How far beyond the tile something is drawn.
    pub fn extent(&self) -> i32 {
        let ring = if self.ring.is_some() {
            self.ring_width.max(0)
        } else {
            0
        };
        let shadow = self.shadow.as_ref().map_or(0, |s| {
            shadow_reach(s.softness, s.spread, s.offset_x, s.offset_y) + 1
        });
        ring.max(shadow) + 2
    }

    /// The rectangle of the layered window that shows this frame.
    pub fn bounds(&self) -> Rect {
        expand(self.tile, self.extent())
    }

    pub fn render(&self, surf: &mut Surface, origin: (i32, i32)) {
        let band = self.band.max(0) as f32;
        let corners = self.corners.unwrap_or([self.radius; 4]);
        let tile_corners = corners.map(|r| if r > 0.0 { r + band } else { 0.0 });
        let tile = RRect::of_corners(self.tile, origin, tile_corners);
        let window = RRect::of_corners(self.window(), origin, corners);
        let view = RRect::of(self.view, origin, 0.0);
        if let Some(sh) = &self.shadow {
            let shape = tile.outset(sh.spread).moved(sh.offset_x, sh.offset_y);
            draw_shadow(
                surf,
                &shape,
                &tile,
                sh.softness,
                sh.color,
                sh.draw_behind_window,
            );
        }
        let ring = match (&self.ring, self.ring_width) {
            (Some(paint), w) if w > 0 => Some((w as f32, paint)),
            _ => None,
        };
        let border = if self.band > 0 {
            self.border.as_ref()
        } else {
            None
        };
        if ring.is_some() || border.is_some() {
            draw_frame(
                surf,
                &FramePaint {
                    tile,
                    window,
                    ring,
                    border,
                    view,
                },
            );
        }
    }

    /// Move by `(dx, dy)`.  The view only matters to a gradient that is relative to it; for
    /// anything else it is forgotten, so that frames in different places compare equal.
    fn translate(&mut self, dx: i32, dy: i32) {
        let uses_view = self.uses_view();
        self.tile = translated(self.tile, dx, dy);
        self.view = if uses_view {
            translated(self.view, dx, dy)
        } else {
            Rect::default()
        };
    }

    fn uses_view(&self) -> bool {
        paint_uses_view(self.border.as_ref()) || paint_uses_view(self.ring.as_ref())
    }
}

fn paint_uses_view(p: Option<&Paint>) -> bool {
    matches!(p, Some(Paint::Gradient(g)) if g.relative_to_view)
}

/// The tab indicator of one tabbed column.
#[derive(Debug, Clone, PartialEq)]
pub struct TabsSpec {
    /// The column's tile area: the indicator sits beside it.
    pub area: Rect,
    pub position: TabPosition,
    /// Thickness of the indicator.
    pub width: i32,
    /// Raw gap to the area; its sign (including negative zero) survives the one-pixel minimum.
    /// Negative gaps put the indicator over the window.
    pub gap: f64,
    /// Raw configured gap: even a positive value that rounds to zero keeps independent caps.
    pub gaps_between: f64,
    /// Requested total proportion, clamped to 0..=2 before pixel rounding. Keep the config's
    /// f64 precision; a cast before clamping can overflow or change a half-pixel decision.
    /// Each tab and the indicator's thickness are at least one pixel, even at zero length/width.
    pub length: f64,
    pub corner_radius: f32,
    /// One paint per tab, already dimmed for the tabs that are not active.
    pub tabs: Vec<Paint>,
    /// One target per paint, in exactly the same order. A mismatched list is not clickable.
    pub windows: Vec<WindowId>,
    pub view: Rect,
}

/// A snapshot of just the input geometry, independent of the native window/worker state.
/// Like upstream hit(), the whole tab rectangle is clickable, including rounded cutouts.
pub(super) struct TabHitRegion {
    window: WindowId,
    rect: Rect,
}

impl TabHitRegion {
    pub(super) fn hit_test(&self, point: Point) -> Option<WindowId> {
        self.rect.contains_point(point).then_some(self.window)
    }
}

impl TabsSpec {
    /// Where each tab is, in screen pixels (niri's scale=1, progress=1 layout). Equal tab
    /// lengths are computed with at least a one-pixel gap, then the final gaps are rounded
    /// without that minimum. Rounding left over goes to the first tabs, and the resulting
    /// whole indicator is centred along the side of the area.
    pub fn tab_rects(&self) -> Vec<Rect> {
        let n = self.tabs.len();
        if n == 0
            || !self.length.is_finite()
            || !self.gap.is_finite()
            || !self.gaps_between.is_finite()
        {
            return Vec::new();
        }
        let horizontal_side = matches!(self.position, TabPosition::Top | TabPosition::Bottom);
        let side = if horizontal_side {
            f64::from(self.area.size.w)
        } else {
            f64::from(self.area.size.h)
        };
        let raw_gaps = self.gaps_between.max(0.0);
        let reserved_gaps = raw_gaps.round().max(1.0);
        let wanted = (side * self.length.clamp(0.0, 2.0)).round();
        let shortest = n as f64 * (1.0 + reserved_gaps) - reserved_gaps;
        let length = wanted.max(shortest);
        let per_tab = (length + reserved_gaps) / n as f64 - reserved_gaps;

        // Upstream computes tab lengths before animating gaps. Even at progress=1, a raw
        // gap below 0.5 loses the reserved pixel here: the final span can be n-1 shorter.
        let gaps = raw_gaps.round();
        let length = n as f64 * (per_tab + gaps) - gaps;
        let per_tab = per_tab.floor().max(1.0);
        let floored_length = n as f64 * (per_tab + gaps) - gaps;
        let mut ones_left = (length - floored_length).round() as usize;
        let start = ((side - length) / 2.0).round() as i32;
        let per_tab = per_tab as i32;
        let gaps = gaps as i32;
        let thickness = self.width.max(1);
        let gap = self.gap.abs().round().max(1.0).copysign(self.gap) as i32;
        let across = match self.position {
            TabPosition::Left | TabPosition::Top => -gap - thickness,
            TabPosition::Right => self.area.size.w as i32 + gap,
            TabPosition::Bottom => self.area.size.h as i32 + gap,
        };
        let mut along = start;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let mut len = per_tab;
            if ones_left > 0 {
                len += 1;
                ones_left -= 1;
            }
            let rect = if horizontal_side {
                Rect::new(
                    self.area.loc.x + along,
                    self.area.loc.y + across,
                    len as u32,
                    thickness as u32,
                )
            } else {
                Rect::new(
                    self.area.loc.x + across,
                    self.area.loc.y + along,
                    thickness as u32,
                    len as u32,
                )
            };
            out.push(rect);
            along += len + gaps;
        }
        out
    }

    /// The radius of each corner (top-left, top-right, bottom-right, bottom-left) of tab `i`
    /// of `n`: with no gap between tabs only the ends of the whole indicator are round.
    pub fn corner_radii(&self, i: usize, n: usize) -> [f32; 4] {
        let r = self.corner_radius.max(0.0);
        if self.gaps_between <= 0.0 && n > 1 {
            let vertical_side = matches!(self.position, TabPosition::Left | TabPosition::Right);
            if i == 0 {
                return if vertical_side {
                    [r, r, 0.0, 0.0]
                } else {
                    [r, 0.0, 0.0, r]
                };
            }
            if i == n - 1 {
                return if vertical_side {
                    [0.0, 0.0, r, r]
                } else {
                    [0.0, r, r, 0.0]
                };
            }
            return [0.0; 4];
        }
        [r; 4]
    }

    /// The rectangle of the layered window that shows the indicator.
    pub fn bounds(&self) -> Rect {
        let rects = self.tab_rects();
        let mut it = rects.iter();
        let first = match it.next() {
            Some(r) => *r,
            None => return Rect::new(self.area.loc.x, self.area.loc.y, 0, 0),
        };
        let (mut l, mut t, mut r, mut b) =
            (first.loc.x, first.loc.y, first.right(), first.bottom());
        for rect in it {
            l = l.min(rect.loc.x);
            t = t.min(rect.loc.y);
            r = r.max(rect.right());
            b = b.max(rect.bottom());
        }
        // A little room for the anti-aliased edge.
        Rect::new(l - 2, t - 2, (r - l + 4) as u32, (b - t + 4) as u32)
    }

    /// The clicked target, in screen pixels. The whole tab rectangle is an input region,
    /// including transparent rounded cutouts, but not gaps or the bounds' 2px AA margin.
    /// Incomplete target lists fail closed rather than use an ambiguous paint-to-window mapping.
    pub fn hit_test(&self, point: Point) -> Option<WindowId> {
        self.hit_regions().iter().find_map(|r| r.hit_test(point))
    }

    pub(super) fn hit_regions(&self) -> Vec<TabHitRegion> {
        if self.windows.len() != self.tabs.len() {
            return Vec::new();
        }
        self.tab_rects()
            .into_iter()
            .zip(self.windows.iter().copied())
            .filter(|(_, window)| window.as_isize() != 0)
            .map(|(rect, window)| TabHitRegion { window, rect })
            .collect()
    }

    /// Fit the painted corner radii, including the shared end caps of touching tabs.
    fn tab_shape(&self, rect: Rect, origin: (i32, i32), i: usize) -> RRect {
        let mut rr = RRect::of(rect, origin, 0.0);
        rr.r = self.corner_radii(i, self.tabs.len());
        let [tl, tr, br, bl] = rr.r;
        // CSS fitting scales all corners together. A shared cap can use the entire tab
        // length, rather than half of it, because the corners at the seam are square.
        let scale = 1.0_f32
            .min(rr.w / (tl + tr))
            .min(rr.w / (bl + br))
            .min(rr.h / (tl + bl))
            .min(rr.h / (tr + br));
        rr.r = rr.r.map(|r| r * scale);
        rr
    }

    pub fn render(&self, surf: &mut Surface, origin: (i32, i32)) {
        let view = RRect::of(self.view, origin, 0.0);
        let window = RRect::of(self.area, origin, 0.0);
        for (i, (rect, paint)) in self
            .tab_rects()
            .into_iter()
            .zip(self.tabs.iter())
            .enumerate()
        {
            let rr = self.tab_shape(rect, origin, i);
            fill_relative(surf, &rr, paint, &window, &view);
        }
    }

    fn translate(&mut self, dx: i32, dy: i32) {
        let uses_view = self.uses_view();
        self.area = translated(self.area, dx, dy);
        self.view = if uses_view {
            translated(self.view, dx, dy)
        } else {
            Rect::default()
        };
    }

    fn uses_view(&self) -> bool {
        self.tabs.iter().any(|p| paint_uses_view(Some(p)))
    }
}

/// The translucent rectangle that shows where a window dropped now would go.
#[derive(Debug, Clone, PartialEq)]
pub struct HintSpec {
    pub rect: Rect,
    pub radius: f32,
    pub paint: Paint,
    pub view: Rect,
}

/// niri `fullscreen-backdrop-color`: the colour the layer fills the tiling view with while a
/// fullscreen window is up, so that the rim a fullscreen window can leave (gaps, the taskbar
/// area, DPI rounding) is not the desktop showing through.
#[derive(Debug, Clone, PartialEq)]
pub struct BackdropSpec {
    pub rect: Rect,
    pub paint: Paint,
}

impl BackdropSpec {
    pub fn new(rect: Rect, color: Rgba) -> Self {
        Self {
            rect,
            paint: Paint::Solid(color),
        }
    }

    pub fn bounds(&self) -> Rect {
        self.rect
    }

    pub fn render(&self, surf: &mut Surface, origin: (i32, i32)) {
        let rr = RRect::of(self.rect, origin, 0.0);
        let view = RRect::of(self.rect, origin, 0.0);
        fill(surf, &rr, &self.paint, &view);
    }
}

impl HintSpec {
    pub fn bounds(&self) -> Rect {
        expand(self.rect, 2)
    }

    pub fn render(&self, surf: &mut Surface, origin: (i32, i32)) {
        let rr = RRect::of(self.rect, origin, self.radius);
        let view = RRect::of(self.view, origin, 0.0);
        fill(surf, &rr, &self.paint, &view);
    }

    fn translate(&mut self, dx: i32, dy: i32) {
        let uses_view = self.uses_view();
        self.rect = translated(self.rect, dx, dy);
        self.view = if uses_view {
            translated(self.view, dx, dy)
        } else {
            Rect::default()
        };
    }

    fn uses_view(&self) -> bool {
        paint_uses_view(Some(&self.paint))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecorContent {
    Frame(FrameSpec),
    Tabs(TabsSpec),
    Hint(HintSpec),
    Backdrop(BackdropSpec),
}

/// One decoration: what to draw, and the window it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct DecorSpec {
    pub id: DecorId,
    /// The window whose z-order this decoration follows: frames sit below it, tabs above.
    /// Zero means above every ordinary window.
    pub anchor: isize,
    pub content: DecorContent,
}

impl DecorSpec {
    pub fn frame(window: isize, frame: FrameSpec) -> Self {
        Self {
            id: DecorId {
                kind: DecorKind::Frame,
                key: window,
            },
            anchor: window,
            content: DecorContent::Frame(frame),
        }
    }

    pub fn tabs(key: isize, anchor: isize, tabs: TabsSpec) -> Self {
        Self {
            id: DecorId {
                kind: DecorKind::Tabs,
                key,
            },
            anchor,
            content: DecorContent::Tabs(tabs),
        }
    }

    pub fn hint(hint: HintSpec) -> Self {
        Self {
            id: DecorId {
                kind: DecorKind::Hint,
                key: 0,
            },
            anchor: 0,
            content: DecorContent::Hint(hint),
        }
    }

    /// The fullscreen backdrop of an output: one per monitor, keyed by the output, stacked
    /// above every ordinary window (`anchor` 0) so it sits behind the fullscreen window.
    pub fn backdrop(output: u64, backdrop: BackdropSpec) -> Self {
        Self {
            id: DecorId {
                kind: DecorKind::Backdrop,
                key: output as isize,
            },
            anchor: 0,
            content: DecorContent::Backdrop(backdrop),
        }
    }

    /// The rectangle of the layered window, in screen pixels.
    pub fn bounds(&self) -> Rect {
        match &self.content {
            DecorContent::Frame(f) => f.bounds(),
            DecorContent::Tabs(t) => t.bounds(),
            DecorContent::Hint(h) => h.bounds(),
            DecorContent::Backdrop(b) => b.bounds(),
        }
    }

    /// Paint into `surf`, whose top-left pixel is the screen pixel `origin` (the top-left of
    /// [`Self::bounds`]).
    pub fn render(&self, surf: &mut Surface, origin: (i32, i32)) {
        match &self.content {
            DecorContent::Frame(f) => f.render(surf, origin),
            DecorContent::Tabs(t) => t.render(surf, origin),
            DecorContent::Hint(h) => h.render(surf, origin),
            DecorContent::Backdrop(b) => b.render(surf, origin),
        }
    }

    /// The same decoration moved so that its bounds start at the origin: two specs that are
    /// equal like this look the same wherever they are.
    pub fn at_origin(&self) -> DecorSpec {
        let b = self.bounds();
        let mut out = self.clone();
        match &mut out.content {
            DecorContent::Frame(f) => f.translate(-b.loc.x, -b.loc.y),
            DecorContent::Tabs(t) => t.translate(-b.loc.x, -b.loc.y),
            DecorContent::Hint(h) => h.translate(-b.loc.x, -b.loc.y),
            // The backdrop is the output's rectangle, so it is compared where it is.
            DecorContent::Backdrop(_) => {}
        }
        out
    }

    /// True when the look depends on where the decoration is (a gradient relative to the
    /// workspace view).
    pub fn depends_on_position(&self) -> bool {
        match &self.content {
            DecorContent::Frame(f) => f.uses_view(),
            DecorContent::Tabs(t) => t.uses_view(),
            DecorContent::Hint(h) => h.uses_view(),
            DecorContent::Backdrop(_) => false,
        }
    }

    /// True when `other` looks the same as `self`, wherever each of them is.
    pub fn looks_like(&self, other: &DecorSpec) -> bool {
        if self.id != other.id || self.at_origin().content != other.at_origin().content {
            return false;
        }
        !self.depends_on_position() || self.bounds().loc == other.bounds().loc
    }
}

/// The paint a configured colour or gradient stands for.  `accent` is the Windows accent
/// colour, used for the colour name `accent`.  `None` when the colour cannot be read.
pub fn paint_of(style: &PaintStyle, accent: Option<[u8; 4]>) -> Option<Paint> {
    match style {
        PaintStyle::Color(text) => colour_of(text, accent).map(Paint::Solid),
        PaintStyle::Gradient(g) => gradient_of(g, accent).map(Paint::Gradient),
    }
}

fn colour_of(text: &str, accent: Option<[u8; 4]>) -> Option<Rgba> {
    match text.trim() {
        "accent" | "windows-accent" | "system-accent" => accent.map(Rgba::from_array),
        other => crate::config::parse_css_color(other).map(Rgba::from_array),
    }
}

fn gradient_of(g: &GradientStyle, accent: Option<[u8; 4]>) -> Option<Gradient> {
    Some(Gradient {
        from: colour_of(&g.from, accent)?,
        to: colour_of(&g.to, accent)?,
        angle: g.angle as f32,
        relative_to_view: g.relative_to_view,
        space: g.space,
        hue: g.hue,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::effects::{GradientHue, GradientSpace};

    const RED: Paint = Paint::Solid(Rgba::new(255, 0, 0, 255));
    const BLUE: Paint = Paint::Solid(Rgba::new(0, 0, 255, 255));
    const TAB_SIDES: [TabPosition; 4] = [
        TabPosition::Left,
        TabPosition::Right,
        TabPosition::Top,
        TabPosition::Bottom,
    ];

    fn frame(tile: Rect) -> FrameSpec {
        FrameSpec {
            tile,
            band: 4,
            radius: 8.0,
            corners: None,
            border: Some(BLUE),
            ring_width: 3,
            ring: Some(RED),
            shadow: None,
            view: Rect::new(0, 0, 1366, 768),
        }
    }

    #[test]
    fn the_window_is_the_tile_without_its_border_band() {
        let f = frame(Rect::new(100, 50, 400, 300));
        assert_eq!(f.window(), Rect::new(104, 54, 392, 292));
    }

    #[test]
    fn a_frame_reaches_as_far_as_its_ring_and_the_edge_of_the_antialiasing() {
        let f = frame(Rect::new(100, 50, 400, 300));
        assert_eq!(f.extent(), 3 + 2);
        assert_eq!(f.bounds(), Rect::new(95, 45, 410, 310));
    }

    #[test]
    fn a_shadow_makes_the_frame_reach_further() {
        let mut f = frame(Rect::new(100, 50, 400, 300));
        f.shadow = Some(ShadowSpec {
            softness: 30.0,
            spread: 5.0,
            offset_x: 0.0,
            offset_y: 5.0,
            color: Rgba::new(0, 0, 0, 119),
            draw_behind_window: false,
        });
        assert_eq!(f.extent(), 55 + 1 + 2);
    }

    #[test]
    fn a_frame_without_a_ring_does_not_reach_beyond_its_tile() {
        let mut f = frame(Rect::new(0, 0, 100, 100));
        f.ring = None;
        assert_eq!(f.extent(), 2);
    }

    #[test]
    fn rendering_a_frame_paints_the_ring_outside_the_tile_and_the_band_inside() {
        let f = FrameSpec {
            radius: 0.0,
            ..frame(Rect::new(100, 50, 60, 40))
        };
        let b = f.bounds();
        let (w, h) = (b.size.w as i32, b.size.h as i32);
        let mut px = vec![0u32; (w * h) as usize];
        let mut surf = Surface::new(w, h, &mut px);
        f.render(&mut surf, (b.loc.x, b.loc.y));
        // Left of the tile, in the ring: red.  Just inside the tile, in the band: blue.
        let ring_x = 100 - 2 - b.loc.x;
        let band_x = 102 - b.loc.x;
        let mid_y = 70 - b.loc.y;
        assert_eq!(surf.get(ring_x, mid_y), [255, 0, 0, 255]);
        assert_eq!(surf.get(band_x, mid_y), [0, 0, 255, 255]);
        // In the window: nothing.
        assert_eq!(surf.get(130 - b.loc.x, mid_y), [0, 0, 0, 0]);
    }

    #[test]
    fn explicit_frame_corners_override_the_legacy_radius_and_none_keeps_it() {
        let mut f = frame(Rect::new(20, 20, 80, 80));
        let render = |f: &FrameSpec| {
            let mut pixels = vec![0; 120 * 120];
            f.render(&mut Surface::new(120, 120, &mut pixels), (0, 0));
            pixels
        };
        let legacy = render(&f);
        f.corners = Some([f.radius; 4]);
        assert_eq!(render(&f), legacy);
        f.corners = Some([0.0; 4]);
        let square = render(&f);
        assert_ne!(square, legacy);
        f.corners = None;
        f.radius = 0.0;
        assert_eq!(render(&f), square);
    }

    #[test]
    fn frame_corners_reach_the_border_ring_and_shadow_in_clockwise_order() {
        let mut f = frame(Rect::new(20, 20, 80, 80));
        f.corners = Some([16.0, 0.0, 8.0, 4.0]);
        let mut pixels = vec![0; 120 * 120];
        let mut surf = Surface::new(120, 120, &mut pixels);
        f.render(&mut surf, (0, 0));
        assert_eq!(
            surf.get(24, 24),
            [255, 0, 0, 255],
            "large top-left cut is ring"
        );
        assert_eq!(surf.get(95, 24), [0; 4], "top-right window stays square");
        assert_eq!(
            surf.get(95, 95),
            [0, 0, 255, 255],
            "bottom-right cut is border"
        );
        assert!(
            surf.get(25, 94)[3] < 30,
            "bottom-left has the smaller radius"
        );
        assert_eq!(surf.get(102, 17), [255, 0, 0, 255], "square ring corner");
        assert_eq!(surf.get(17, 17), [0; 4], "round ring corner");

        f.ring = None;
        f.border = None;
        f.shadow = Some(ShadowSpec {
            softness: 0.0,
            spread: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
            color: Rgba::new(0, 0, 0, 200),
            draw_behind_window: true,
        });
        surf.clear();
        f.render(&mut surf, (0, 0));
        assert_eq!(surf.get(20, 20), [0; 4], "shadow's top-left is round");
        assert_eq!(
            surf.get(99, 20),
            [0, 0, 0, 200],
            "shadow's top-right is square"
        );
    }

    #[test]
    fn two_frames_that_differ_only_in_place_look_alike_and_a_colour_makes_them_differ() {
        let a = DecorSpec::frame(7, frame(Rect::new(100, 50, 400, 300)));
        let b = DecorSpec::frame(7, frame(Rect::new(600, 80, 400, 300)));
        assert!(a.looks_like(&b));
        let mut other = frame(Rect::new(600, 80, 400, 300));
        other.ring = Some(BLUE);
        assert!(!a.looks_like(&DecorSpec::frame(7, other)));
        let bigger = frame(Rect::new(100, 50, 401, 300));
        assert!(!a.looks_like(&DecorSpec::frame(7, bigger)));
    }

    #[test]
    fn a_gradient_relative_to_the_view_looks_different_in_another_place() {
        let g = Paint::Gradient(Gradient {
            from: Rgba::new(255, 0, 0, 255),
            to: Rgba::new(0, 0, 255, 255),
            angle: 90.0,
            relative_to_view: true,
            space: GradientSpace::Srgb,
            hue: GradientHue::Shorter,
        });
        let mut f1 = frame(Rect::new(100, 50, 400, 300));
        f1.ring = Some(g);
        let mut f2 = frame(Rect::new(600, 50, 400, 300));
        f2.ring = Some(g);
        let a = DecorSpec::frame(7, f1);
        let b = DecorSpec::frame(7, f2);
        assert!(a.depends_on_position());
        assert!(!a.looks_like(&b));
    }

    fn tabs(position: TabPosition, n: usize) -> TabsSpec {
        TabsSpec {
            area: Rect::new(200, 100, 400, 600),
            position,
            width: 4,
            gap: 5.0,
            gaps_between: 0.0,
            length: 0.5,
            corner_radius: 3.0,
            tabs: vec![RED; n],
            windows: (1..=n).map(|i| WindowId::new(i as isize)).collect(),
            view: Rect::new(0, 0, 1366, 768),
        }
    }

    #[test]
    fn zero_width_and_length_keep_one_pixel_tabs_on_every_side() {
        for position in TAB_SIDES {
            for length in [0.0, -1.0] {
                let mut t = tabs(position, 3);
                t.width = 0;
                t.length = length;
                t.gaps_between = 2.0;
                let rects = t.tab_rects();
                assert_eq!(rects.len(), 3);
                assert!(rects.iter().all(|r| r.size.w == 1 && r.size.h == 1));
                let b = t.bounds();
                let (w, h) = (b.size.w as i32, b.size.h as i32);
                let mut px = vec![0u32; (w * h) as usize];
                let mut surf = Surface::new(w, h, &mut px);
                t.render(&mut surf, (b.loc.x, b.loc.y));
                for (i, rect) in rects.iter().enumerate() {
                    assert_eq!(t.hit_test(rect.loc), Some(t.windows[i]));
                    assert_eq!(
                        surf.get(rect.loc.x - b.loc.x, rect.loc.y - b.loc.y),
                        [255, 0, 0, 255]
                    );
                }
            }
        }
    }

    #[test]
    fn length_two_extends_equally_with_zero_gap_reservations_removed() {
        for position in TAB_SIDES {
            let mut t = tabs(position, 3);
            t.length = 2.0;
            let horizontal = matches!(position, TabPosition::Top | TabPosition::Bottom);
            let (start, side) = if horizontal {
                (t.area.loc.x, t.area.size.w as i32)
            } else {
                (t.area.loc.y, t.area.size.h as i32)
            };
            let rects = t.tab_rects();
            let first = rects.first().unwrap();
            let last = rects.last().unwrap();
            assert_eq!(
                if horizontal { first.loc.x } else { first.loc.y },
                start - side / 2 + 1
            );
            assert_eq!(
                if horizontal {
                    last.right()
                } else {
                    last.bottom()
                },
                start + side + side / 2 - 1
            );
            // Clamping occurs in f64, even for finite values that would overflow f32.
            for length in [3.0, f64::MAX] {
                t.length = length;
                assert_eq!(t.tab_rects(), rects);
            }
        }
    }

    #[test]
    fn length_keeps_f64_precision_until_after_clamping_and_pixel_rounding() {
        for position in TAB_SIDES {
            let mut t = tabs(position, 1);
            t.area = Rect::new(200, 100, 101, 101);
            for (length, span, start) in [
                (-f64::MAX, 1, 50),
                (0.5 - f64::EPSILON, 50, 26),
                (0.5, 51, 25),
                (0.5 + f64::EPSILON, 51, 25),
                (f64::MAX, 202, -51),
            ] {
                t.length = length;
                let rects = t.tab_rects();
                assert_eq!(rects.len(), 1);
                let rect = rects[0];
                if matches!(position, TabPosition::Left | TabPosition::Right) {
                    assert_eq!((rect.loc.y, rect.size.h), (100 + start, span));
                } else {
                    assert_eq!((rect.loc.x, rect.size.w), (200 + start, span));
                }
            }
        }
    }

    #[test]
    fn external_gaps_keep_their_sign_and_one_pixel_minimum_on_every_side() {
        for position in TAB_SIDES {
            for (raw, expected) in [
                (0.0, 1),
                (-0.0, -1),
                (0.25, 1),
                (-0.25, -1),
                (0.5, 1),
                (-0.5, -1),
                (1.5, 2),
                (-1.5, -2),
            ] {
                let mut t = tabs(position, 3);
                t.gap = raw;
                for rect in t.tab_rects() {
                    let actual = match position {
                        TabPosition::Left => t.area.loc.x - rect.right(),
                        TabPosition::Right => rect.loc.x - t.area.right(),
                        TabPosition::Top => t.area.loc.y - rect.bottom(),
                        TabPosition::Bottom => rect.loc.y - t.area.bottom(),
                    };
                    assert_eq!(actual, expected, "{position:?}, {raw:?}");
                }
            }
        }
    }

    #[test]
    fn nonfinite_tab_geometry_never_paints_or_accepts_input() {
        for position in TAB_SIDES {
            for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                for field in 0..3 {
                    let mut t = tabs(position, 3);
                    match field {
                        0 => t.length = bad,
                        1 => t.gap = bad,
                        _ => t.gaps_between = bad,
                    }
                    assert!(t.tab_rects().is_empty());
                    assert_eq!(t.bounds(), Rect::new(200, 100, 0, 0));
                    assert!(t.hit_regions().is_empty());
                    assert_eq!(t.hit_test(t.area.center()), None);
                    let mut px = [0u32; 16];
                    let mut surf = Surface::new(4, 4, &mut px);
                    t.render(&mut surf, (200, 100));
                    assert!(surf.px.iter().all(|&pixel| pixel == 0));
                }
            }
        }
    }

    #[test]
    fn negative_gap_overlaps_the_window_but_negative_tab_gaps_only_touch() {
        for position in TAB_SIDES {
            let mut t = tabs(position, 3);
            t.gap = -3.0;
            t.gaps_between = -5.0;
            let rects = t.tab_rects();
            match position {
                TabPosition::Left => assert_eq!(rects[0].right(), t.area.loc.x + 3),
                TabPosition::Right => assert_eq!(rects[0].loc.x, t.area.right() - 3),
                TabPosition::Top => assert_eq!(rects[0].bottom(), t.area.loc.y + 3),
                TabPosition::Bottom => assert_eq!(rects[0].loc.y, t.area.bottom() - 3),
            }
            t.gaps_between = 0.0;
            assert_eq!(t.tab_rects(), rects);
        }
    }

    #[test]
    fn tabs_on_the_left_run_down_the_middle_of_the_column() {
        let rects = tabs(TabPosition::Left, 3).tab_rects();
        assert_eq!(rects.len(), 3);
        // 300 requested pixels minus two reserved gaps: 298 px, centred at y = 400.
        assert_eq!(rects[0].loc.y, 251);
        assert_eq!(rects[2].bottom(), 549);
        // 5 px gap and 4 px width to the left of x = 200.
        assert_eq!(rects[0].loc.x, 200 - 5 - 4);
        assert_eq!(rects[0].size.w, 4);
        // They touch (no gaps between tabs) and share the length.
        assert_eq!(rects[0].bottom(), rects[1].loc.y);
        assert_eq!(rects[1].bottom(), rects[2].loc.y);
        assert_eq!(
            rects.iter().map(|r| r.size.h).collect::<Vec<_>>(),
            vec![100, 99, 99]
        );
    }

    #[test]
    fn tabs_on_the_right_sit_beyond_the_right_edge() {
        let rects = tabs(TabPosition::Right, 2).tab_rects();
        assert_eq!(rects[0].loc.x, 200 + 400 + 5);
    }

    #[test]
    fn tabs_on_top_run_along_the_width_and_on_the_bottom_sit_below() {
        let top = tabs(TabPosition::Top, 2).tab_rects();
        assert_eq!(top[0].loc.y, 100 - 5 - 4);
        assert_eq!(top[0].size.h, 4);
        // The final zero gap removes one reserved pixel: round((400 - 199) / 2).
        assert_eq!(top[0].loc.x, 301);
        let bottom = tabs(TabPosition::Bottom, 2).tab_rects();
        assert_eq!(bottom[0].loc.y, 100 + 600 + 5);
    }

    #[test]
    fn the_rounding_left_over_goes_to_the_first_tabs() {
        let mut t = tabs(TabPosition::Left, 4);
        t.area = Rect::new(0, 0, 100, 202);
        t.length = 0.5; // 101 requested pixels, minus three reserved gaps = 98.
        let rects = t.tab_rects();
        let heights: Vec<u32> = rects.iter().map(|r| r.size.h).collect();
        assert_eq!(heights.iter().sum::<u32>(), 98);
        assert_eq!(heights, vec![25, 25, 24, 24]);
        assert_eq!(rects[0].loc.y, 52);
    }

    #[test]
    fn gaps_between_tabs_are_kept_and_make_every_tab_round() {
        let mut t = tabs(TabPosition::Left, 3);
        t.gaps_between = 2.0;
        let rects = t.tab_rects();
        assert_eq!(rects[1].loc.y - rects[0].bottom(), 2);
        assert_eq!(t.corner_radii(1, 3), [3.0; 4]);
        // Without gaps only the ends are round.
        let t = tabs(TabPosition::Left, 3);
        assert_eq!(t.corner_radii(0, 3), [3.0, 3.0, 0.0, 0.0]);
        assert_eq!(t.corner_radii(1, 3), [0.0; 4]);
        assert_eq!(t.corner_radii(2, 3), [0.0, 0.0, 3.0, 3.0]);
    }

    #[test]
    fn static_tabs_match_both_gap_rounding_stages_on_every_side() {
        // (raw gap, final pixel gap, tab lengths at proportion=1, start, shortest start).
        // The column side is 101, and there are four tabs. Positive subpixel gaps keep
        // independent caps even when their final pixel gap is zero.
        let cases = [
            (0.0, 0, [25, 25, 24, 24], 2, 49),
            (f64::MIN_POSITIVE, 0, [25, 25, 24, 24], 2, 49),
            (0.25, 0, [25, 25, 24, 24], 2, 49),
            (0.49999999999999994, 0, [25, 25, 24, 24], 2, 49),
            (0.5, 1, [25, 25, 24, 24], 0, 47),
            (0.75, 1, [25, 25, 24, 24], 0, 47),
            (1.0, 1, [25, 25, 24, 24], 0, 47),
            (1.4999999999999998, 1, [25, 25, 24, 24], 0, 47),
            (1.5, 2, [24, 24, 24, 23], 0, 46),
            (2.5, 3, [23, 23, 23, 23], 0, 44),
            (3.5, 4, [23, 22, 22, 22], 0, 43),
            (65534.5, 65535, [1, 1, 1, 1], -98254, -98254),
            (65535.0, 65535, [1, 1, 1, 1], -98254, -98254),
        ];
        for position in TAB_SIDES {
            for (raw_gap, gap, lengths, start, shortest_start) in cases {
                let mut t = tabs(position, 4);
                t.area = Rect::new(200, 100, 101, 101);
                t.gaps_between = raw_gap;
                assert_eq!(
                    t.corner_radii(1, 4),
                    if raw_gap == 0.0 { [0.0; 4] } else { [3.0; 4] }
                );
                for (length, lengths, start) in
                    [(0.0, [1; 4], shortest_start), (1.0, lengths, start)]
                {
                    t.length = length;
                    let mut along = start;
                    let expected: Vec<_> = lengths
                        .iter()
                        .map(|&len| {
                            let rect = match position {
                                TabPosition::Left => Rect::new(191, 100 + along, 4, len),
                                TabPosition::Right => Rect::new(306, 100 + along, 4, len),
                                TabPosition::Top => Rect::new(200 + along, 91, len, 4),
                                TabPosition::Bottom => Rect::new(200 + along, 206, len, 4),
                            };
                            along += len as i32 + gap;
                            rect
                        })
                        .collect();
                    assert_eq!(t.tab_rects(), expected, "{position:?}, {raw_gap}, {length}");
                }
            }
        }
    }

    #[test]
    fn touching_tabs_share_fitted_caps_on_every_side() {
        for position in TAB_SIDES {
            let mut t = tabs(position, 3);
            t.width = 20;
            t.corner_radius = 100.0;
            let vertical = matches!(position, TabPosition::Left | TabPosition::Right);
            let expected = if vertical {
                [[10.0, 10.0, 0.0, 0.0], [0.0; 4], [0.0, 0.0, 10.0, 10.0]]
            } else {
                [[10.0, 0.0, 0.0, 10.0], [0.0; 4], [0.0, 10.0, 10.0, 0.0]]
            };
            let rects = t.tab_rects();
            for (i, rect) in rects.iter().enumerate() {
                assert_eq!(t.tab_shape(*rect, (0, 0), i).r, expected[i]);
                assert_eq!(t.hit_test(rect.loc), Some(t.windows[i]));
            }
            for pair in rects.windows(2) {
                let seam = if vertical {
                    Point::new(pair[0].loc.x, pair[0].bottom())
                } else {
                    Point::new(pair[0].right(), pair[0].loc.y)
                };
                let next = rects.iter().position(|r| r.loc == pair[1].loc).unwrap();
                assert_eq!(t.hit_test(seam), Some(t.windows[next]));
            }
        }
    }

    #[test]
    fn short_touching_tabs_fit_and_paint_caps_past_their_midpoints() {
        for position in TAB_SIDES {
            let mut t = tabs(position, 3);
            t.area = Rect::new(200, 100, 20, 20);
            t.width = 20;
            t.length = 1.0;
            t.corner_radius = 100.0;
            let vertical = matches!(position, TabPosition::Left | TabPosition::Right);
            let expected = if vertical {
                [[6.0, 6.0, 0.0, 0.0], [0.0; 4], [0.0, 0.0, 6.0, 6.0]]
            } else {
                [[6.0, 0.0, 0.0, 6.0], [0.0; 4], [0.0, 6.0, 6.0, 0.0]]
            };
            let rects = t.tab_rects();
            let b = t.bounds();
            let (w, h) = (b.size.w as i32, b.size.h as i32);
            let mut px = vec![0u32; (w * h) as usize];
            let mut surf = Surface::new(w, h, &mut px);
            t.render(&mut surf, (b.loc.x, b.loc.y));
            for (i, rect) in rects.iter().enumerate() {
                assert_eq!(t.tab_shape(*rect, (0, 0), i).r, expected[i]);
            }
            // At these pixels the end-cap arc crosses the tab's centre line. A radius
            // chosen by rectangle quadrant would incorrectly make them fully opaque.
            let alpha = ((0.5 - (5.5_f32.hypot(2.5) - 6.0)) * 255.0).round() as i32;
            for (i, along) in [(0, 3), (2, 2)] {
                let rect = rects[i];
                let point = if vertical {
                    Point::new(rect.loc.x, rect.loc.y + along)
                } else {
                    Point::new(rect.loc.x + along, rect.loc.y)
                };
                let actual = surf.get(point.x - b.loc.x, point.y - b.loc.y);
                assert!((i32::from(actual[3]) - alpha).abs() <= 1);
                assert_eq!(t.hit_test(point), Some(t.windows[i]));
                let cutout = if i == 0 {
                    rect.loc
                } else if vertical {
                    Point::new(rect.loc.x, rect.bottom() - 1)
                } else {
                    Point::new(rect.right() - 1, rect.loc.y)
                };
                assert_eq!(surf.get(cutout.x - b.loc.x, cutout.y - b.loc.y)[3], 0);
                assert_eq!(t.hit_test(cutout), Some(t.windows[i]));
            }
            let seam = rects[1].loc;
            assert_eq!(surf.get(seam.x - b.loc.x, seam.y - b.loc.y)[3], 255);
        }
    }

    #[test]
    fn a_single_tab_is_round_all_over() {
        let t = tabs(TabPosition::Left, 1);
        assert_eq!(t.corner_radii(0, 1), [3.0; 4]);
    }

    #[test]
    fn many_tabs_never_get_shorter_than_one_pixel() {
        let mut t = tabs(TabPosition::Left, 10);
        t.area = Rect::new(0, 0, 100, 4);
        let rects = t.tab_rects();
        assert_eq!(rects.len(), 10);
        assert!(rects.iter().all(|r| r.size.h >= 1));
    }

    #[test]
    fn the_bounds_of_the_indicator_cover_every_tab() {
        let t = tabs(TabPosition::Left, 3);
        let b = t.bounds();
        for r in t.tab_rects() {
            assert!(r.loc.x >= b.loc.x && r.right() <= b.right());
            assert!(r.loc.y >= b.loc.y && r.bottom() <= b.bottom());
        }
    }

    #[test]
    fn hits_include_rounded_cutouts_but_exclude_gaps_edges_and_aa_margin() {
        for position in TAB_SIDES {
            let mut t = tabs(position, 3);
            t.width = 20;
            t.corner_radius = 10.0;
            t.gaps_between = 6.0;
            let rects = t.tab_rects();
            for (i, rect) in rects.iter().enumerate() {
                let shape = t.tab_shape(*rect, (0, 0), i);
                assert_eq!(
                    shape.coverage(rect.loc.x as f32 + 0.5, rect.loc.y as f32 + 0.5),
                    0.0
                );
                for point in [
                    rect.loc,
                    Point::new(rect.right() - 1, rect.loc.y),
                    Point::new(rect.loc.x, rect.bottom() - 1),
                    Point::new(rect.right() - 1, rect.bottom() - 1),
                    rect.center(),
                ] {
                    assert_eq!(t.hit_test(point), Some(t.windows[i]));
                }
                assert_eq!(t.hit_test(Point::new(rect.right(), rect.center().y)), None);
                assert_eq!(t.hit_test(Point::new(rect.center().x, rect.bottom())), None);
            }
            let gap = if matches!(position, TabPosition::Top | TabPosition::Bottom) {
                Point::new(rects[0].right() + 3, rects[0].center().y)
            } else {
                Point::new(rects[0].center().x, rects[0].bottom() + 3)
            };
            assert_eq!(t.hit_test(gap), None);
            let b = t.bounds();
            for inset in 0..2 {
                for point in [
                    Point::new(b.loc.x + inset, b.center().y),
                    Point::new(b.right() - 1 - inset, b.center().y),
                    Point::new(b.center().x, b.loc.y + inset),
                    Point::new(b.center().x, b.bottom() - 1 - inset),
                ] {
                    assert_eq!(t.hit_test(point), None);
                }
            }
        }
    }

    #[test]
    fn missing_extra_and_null_targets_never_select_the_wrong_window() {
        let mut t = tabs(TabPosition::Left, 2);
        let rects = t.tab_rects();
        let targets = t.windows.clone();
        t.windows.pop();
        assert!(rects.iter().all(|r| t.hit_test(r.center()).is_none()));
        t.windows = targets;
        t.windows.push(WindowId::new(3));
        assert!(rects.iter().all(|r| t.hit_test(r.center()).is_none()));
        t.windows.pop();
        t.windows[0] = WindowId::new(0);
        assert_eq!(t.hit_test(rects[0].center()), None);
        assert_eq!(t.hit_test(rects[1].center()), Some(t.windows[1]));
    }

    #[test]
    fn rendering_tabs_paints_each_tab() {
        let mut t = tabs(TabPosition::Left, 2);
        t.corner_radius = 0.0;
        t.tabs = vec![RED, BLUE];
        let b = t.bounds();
        let (w, h) = (b.size.w as i32, b.size.h as i32);
        let mut px = vec![0u32; (w * h) as usize];
        let mut surf = Surface::new(w, h, &mut px);
        t.render(&mut surf, (b.loc.x, b.loc.y));
        let rects = t.tab_rects();
        let c0 = rects[0].center();
        let c1 = rects[1].center();
        assert_eq!(surf.get(c0.x - b.loc.x, c0.y - b.loc.y), [255, 0, 0, 255]);
        assert_eq!(surf.get(c1.x - b.loc.x, c1.y - b.loc.y), [0, 0, 255, 255]);
    }

    #[test]
    fn tab_gradients_use_the_window_or_view_without_restarting_per_tab() {
        for position in TAB_SIDES {
            for relative_to_view in [false, true] {
                let mut t = tabs(position, 2);
                let horizontal = matches!(position, TabPosition::Top | TabPosition::Bottom);
                t.corner_radius = 0.0;
                t.tabs = vec![
                    Paint::Gradient(Gradient {
                        from: Rgba::new(0, 0, 0, 255),
                        to: Rgba::new(255, 255, 255, 255),
                        angle: if horizontal { 90.0 } else { 180.0 },
                        relative_to_view,
                        space: GradientSpace::Srgb,
                        hue: GradientHue::default(),
                    });
                    2
                ];
                let b = t.bounds();
                let (w, h) = (b.size.w as i32, b.size.h as i32);
                let mut px = vec![0u32; (w * h) as usize];
                let mut surf = Surface::new(w, h, &mut px);
                t.render(&mut surf, (b.loc.x, b.loc.y));
                let source = if relative_to_view { t.view } else { t.area };
                for rect in t.tab_rects() {
                    let point = rect.center();
                    let (coordinate, start, length) = if horizontal {
                        (point.x, source.loc.x, source.size.w)
                    } else {
                        (point.y, source.loc.y, source.size.h)
                    };
                    let fraction = (coordinate as f32 + 0.5 - start as f32) / length as f32;
                    let expected = (fraction.clamp(0.0, 1.0) * 255.0).round() as i32;
                    let actual = surf.get(point.x - b.loc.x, point.y - b.loc.y);
                    assert!((i32::from(actual[0]) - expected).abs() <= 1);
                    assert_eq!(actual[0], actual[1]);
                    assert_eq!(actual[1], actual[2]);
                    assert_eq!(actual[3], 255);
                }
            }
        }
    }

    #[test]
    fn the_hint_is_a_filled_rectangle_a_little_larger_than_itself_in_bounds() {
        let hint = HintSpec {
            rect: Rect::new(10, 10, 100, 50),
            radius: 0.0,
            paint: Paint::Solid(Rgba::new(127, 200, 255, 128)),
            view: Rect::new(0, 0, 500, 500),
        };
        let spec = DecorSpec::hint(hint);
        let b = spec.bounds();
        assert_eq!(b, Rect::new(8, 8, 104, 54));
        let (w, h) = (b.size.w as i32, b.size.h as i32);
        let mut px = vec![0u32; (w * h) as usize];
        let mut surf = Surface::new(w, h, &mut px);
        spec.render(&mut surf, (b.loc.x, b.loc.y));
        assert_eq!(surf.get(50 - b.loc.x, 30 - b.loc.y)[3], 128);
        assert_eq!(surf.get(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn paints_come_from_colour_text_and_the_accent_name_uses_the_system_colour() {
        let solid = paint_of(&PaintStyle::color("#7fc8ff"), None).unwrap();
        assert_eq!(solid, Paint::Solid(Rgba::new(127, 200, 255, 255)));
        assert_eq!(paint_of(&PaintStyle::color("accent"), None), None);
        assert_eq!(
            paint_of(&PaintStyle::color("accent"), Some([1, 2, 3, 255])),
            Some(Paint::Solid(Rgba::new(1, 2, 3, 255)))
        );
        assert_eq!(paint_of(&PaintStyle::color("nonsense"), None), None);
        let g = GradientStyle {
            from: "#ff0000".to_string(),
            to: "blue".to_string(),
            angle: 45.0,
            relative_to_view: true,
            space: GradientSpace::Oklch,
            hue: GradientHue::Longer,
        };
        match paint_of(&PaintStyle::Gradient(g), None) {
            Some(Paint::Gradient(grad)) => {
                assert_eq!(grad.to, Rgba::new(0, 0, 255, 255));
                assert_eq!(grad.angle, 45.0);
                assert!(grad.relative_to_view);
                assert_eq!(grad.space, GradientSpace::Oklch);
                assert_eq!(grad.hue, GradientHue::Longer);
            }
            other => panic!("expected a gradient, got {:?}", other),
        }
    }
}
