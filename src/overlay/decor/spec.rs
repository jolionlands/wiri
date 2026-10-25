//! What the overlay layer is asked to draw, as plain data: one frame (shadow, border band and
//! focus ring) per decorated window, one tab indicator per tabbed column and one insert hint.
//!
//! A [`DecorSpec`] knows its own bounding rectangle and how to paint itself into a surface, so
//! the Win32 side only has to keep a layered window of that size where the rectangle is and
//! call [`DecorSpec::render`].  The engine builds the specs (`layout/engine/decor.rs`).

use super::paint::{
    draw_frame, draw_shadow, fill, shadow_reach, FramePaint, Gradient, Paint, RRect, Rgba, Surface,
};
use crate::config::{GradientStyle, PaintStyle, TabPosition};
use crate::utils::Rect;

/// The `output` key of the specs that hooks set (the insert hint): they do not belong to the
/// layout pass of any monitor.
pub const TRANSIENT_OUTPUT: u64 = u64::MAX;

/// Which kind of decoration a window of the layer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DecorKind {
    Frame,
    Tabs,
    Hint,
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
    /// The radius of the window's own corners.
    pub radius: f32,
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
        let tile_radius = if self.radius > 0.0 {
            self.radius + band
        } else {
            0.0
        };
        let tile = RRect::of(self.tile, origin, tile_radius);
        let window = RRect::of(self.window(), origin, self.radius);
        let view = RRect::of(self.view, origin, 0.0);
        if let Some(sh) = &self.shadow {
            let shape = tile.outset(sh.spread).moved(sh.offset_x, sh.offset_y);
            draw_shadow(surf, &shape, &tile, sh.softness, sh.color, sh.draw_behind_window);
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
    /// Between the indicator and the area; negative puts it over the window.
    pub gap: i32,
    pub gaps_between: i32,
    /// All tabs together, as a proportion of the side of the area they run along.
    pub length: f32,
    pub corner_radius: f32,
    /// One paint per tab, already dimmed for the tabs that are not active.
    pub tabs: Vec<Paint>,
    pub view: Rect,
}

impl TabsSpec {
    /// Where each tab is, in screen pixels (niri's layout: centred along the side of the
    /// area, `length` of it in all, the tabs of equal length with the rounding left over
    /// given to the first ones).
    pub fn tab_rects(&self) -> Vec<Rect> {
        let n = self.tabs.len() as i32;
        if n == 0 {
            return Vec::new();
        }
        let horizontal_side = matches!(self.position, TabPosition::Top | TabPosition::Bottom);
        let side = if horizontal_side {
            self.area.size.w as i32
        } else {
            self.area.size.h as i32
        };
        let gaps = self.gaps_between.max(0);
        let wanted = (side as f32 * self.length.clamp(0.0, 2.0)).round() as i32;
        let shortest = n * (1 + gaps) - gaps;
        let length = wanted.max(shortest);
        let per_tab = ((length + gaps) / n - gaps).max(1);
        let mut ones_left = (length - (n * (per_tab + gaps) - gaps)).max(0);
        let start = ((side - length) as f32 / 2.0).round() as i32;
        let thickness = self.width.max(1);
        let across = match self.position {
            TabPosition::Left | TabPosition::Top => -self.gap - thickness,
            TabPosition::Right => self.area.size.w as i32 + self.gap,
            TabPosition::Bottom => self.area.size.h as i32 + self.gap,
        };
        let mut along = start;
        let mut out = Vec::with_capacity(n as usize);
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
        if self.gaps_between == 0 && n > 1 {
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
        let (mut l, mut t, mut r, mut b) = (first.loc.x, first.loc.y, first.right(), first.bottom());
        for rect in it {
            l = l.min(rect.loc.x);
            t = t.min(rect.loc.y);
            r = r.max(rect.right());
            b = b.max(rect.bottom());
        }
        // A little room for the anti-aliased edge.
        Rect::new(l - 2, t - 2, (r - l + 4) as u32, (b - t + 4) as u32)
    }

    pub fn render(&self, surf: &mut Surface, origin: (i32, i32)) {
        let view = RRect::of(self.view, origin, 0.0);
        let n = self.tabs.len();
        for (i, (rect, paint)) in self.tab_rects().into_iter().zip(self.tabs.iter()).enumerate() {
            let mut rr = RRect::of(rect, origin, 0.0);
            rr.r = self.corner_radii(i, n);
            let max = (rr.w.min(rr.h) / 2.0).max(0.0);
            for r in rr.r.iter_mut() {
                *r = r.min(max);
            }
            fill(surf, &rr, paint, &view);
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
}

/// One decoration: what to draw, and the window it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct DecorSpec {
    pub id: DecorId,
    /// The window the decoration is stacked directly below (it follows that window's place in
    /// the z-order); 0: above every ordinary window.
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

    /// The rectangle of the layered window, in screen pixels.
    pub fn bounds(&self) -> Rect {
        match &self.content {
            DecorContent::Frame(f) => f.bounds(),
            DecorContent::Tabs(t) => t.bounds(),
            DecorContent::Hint(h) => h.bounds(),
        }
    }

    /// Paint into `surf`, whose top-left pixel is the screen pixel `origin` (the top-left of
    /// [`Self::bounds`]).
    pub fn render(&self, surf: &mut Surface, origin: (i32, i32)) {
        match &self.content {
            DecorContent::Frame(f) => f.render(surf, origin),
            DecorContent::Tabs(t) => t.render(surf, origin),
            DecorContent::Hint(h) => h.render(surf, origin),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Paint = Paint::Solid(Rgba::new(255, 0, 0, 255));
    const BLUE: Paint = Paint::Solid(Rgba::new(0, 0, 255, 255));

    fn frame(tile: Rect) -> FrameSpec {
        FrameSpec {
            tile,
            band: 4,
            radius: 8.0,
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
            gap: 5,
            gaps_between: 0,
            length: 0.5,
            corner_radius: 3.0,
            tabs: vec![RED; n],
            view: Rect::new(0, 0, 1366, 768),
        }
    }

    #[test]
    fn tabs_on_the_left_run_down_the_middle_of_the_column() {
        let rects = tabs(TabPosition::Left, 3).tab_rects();
        assert_eq!(rects.len(), 3);
        // 600 px high column, half of it in all: 300 px, from y = 100 + 150.
        assert_eq!(rects[0].loc.y, 250);
        assert_eq!(rects[2].bottom(), 550);
        // 5 px gap and 4 px width to the left of x = 200.
        assert_eq!(rects[0].loc.x, 200 - 5 - 4);
        assert_eq!(rects[0].size.w, 4);
        // They touch (no gaps between tabs) and share the length.
        assert_eq!(rects[0].bottom(), rects[1].loc.y);
        assert_eq!(rects[1].bottom(), rects[2].loc.y);
        assert!(rects.iter().all(|r| r.size.h == 100));
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
        assert_eq!(top[0].loc.x, 200 + (400 - 200) / 2);
        let bottom = tabs(TabPosition::Bottom, 2).tab_rects();
        assert_eq!(bottom[0].loc.y, 100 + 600 + 5);
    }

    #[test]
    fn the_rounding_left_over_goes_to_the_first_tabs() {
        let mut t = tabs(TabPosition::Left, 4);
        t.area = Rect::new(0, 0, 100, 202);
        t.length = 0.5; // 101 px in all
        let heights: Vec<u32> = t.tab_rects().iter().map(|r| r.size.h).collect();
        assert_eq!(heights.iter().sum::<u32>(), 101);
        assert_eq!(heights, vec![26, 25, 25, 25]);
    }

    #[test]
    fn gaps_between_tabs_are_kept_and_make_every_tab_round() {
        let mut t = tabs(TabPosition::Left, 3);
        t.gaps_between = 2;
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
            space: crate::config::GradientSpace::Srgb,
        };
        match paint_of(&PaintStyle::Gradient(g), None) {
            Some(Paint::Gradient(grad)) => {
                assert_eq!(grad.to, Rgba::new(0, 0, 255, 255));
                assert_eq!(grad.angle, 45.0);
                assert!(grad.relative_to_view);
            }
            other => panic!("expected a gradient, got {:?}", other),
        }
    }
}
