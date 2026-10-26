//! Output-local, inclusive corner geometry, matching niri's screenshot UI.
//! No HWNDs, clocks, or desktop state: drag/Space-drag can be replayed in tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug)]
pub struct Selection {
    width: i32,
    height: i32,
    a: (i32, i32),
    b: (i32, i32),
    last: (i32, i32),
    pub dragging: bool,
    space: bool,
}

impl Selection {
    pub fn new(width: i32, height: i32, remembered: Option<Region>) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let region = remembered
            .filter(|r| {
                r.x >= 0
                    && r.y >= 0
                    && r.width > 0
                    && r.height > 0
                    && r.x.saturating_add(r.width) <= width
                    && r.y.saturating_add(r.height) <= height
            })
            .unwrap_or(Region {
                x: width / 4,
                y: height / 4,
                width: (width / 2).max(1),
                height: (height / 2).max(1),
            });
        Self {
            width,
            height,
            a: (region.x, region.y),
            b: (region.x + region.width - 1, region.y + region.height - 1),
            last: (0, 0),
            dragging: false,
            space: false,
        }
    }

    fn clamp(&self, p: (i32, i32)) -> (i32, i32) {
        (p.0.clamp(0, self.width - 1), p.1.clamp(0, self.height - 1))
    }

    pub fn region(&self) -> Region {
        Region {
            x: self.a.0.min(self.b.0),
            y: self.a.1.min(self.b.1),
            width: (self.a.0 - self.b.0).abs() + 1,
            height: (self.a.1 - self.b.1).abs() + 1,
        }
    }

    pub fn begin(&mut self, point: (i32, i32)) {
        self.dragging = true;
        self.last = point;
        if !self.space {
            self.a = self.clamp(point);
            self.b = self.a;
        }
    }

    pub fn motion(&mut self, point: (i32, i32)) {
        if !self.dragging {
            return;
        }
        if self.space {
            self.translate(
                point.0.saturating_sub(self.last.0),
                point.1.saturating_sub(self.last.1),
            );
        } else {
            self.b = self.clamp(point);
        }
        self.last = point;
    }

    pub fn end(&mut self) {
        self.dragging = false;
        // Like niri, a click draws a small region, not a zero-sized image.
        if self.a == self.b {
            let x = (self.a.0 - 16).max(0);
            let y = (self.a.1 - 16).max(0);
            self.a = (x, y);
            self.b = ((x + 31).min(self.width - 1), (y + 31).min(self.height - 1));
        }
    }

    pub fn space(&mut self, down: bool) {
        self.space = down;
    }

    pub fn translate(&mut self, dx: i32, dy: i32) {
        let r = self.region();
        let dx = dx.clamp(-r.x, self.width - r.x - r.width);
        let dy = dy.clamp(-r.y, self.height - r.y - r.height);
        self.a.0 += dx;
        self.b.0 += dx;
        self.a.1 += dy;
        self.b.1 += dy;
    }

    pub fn resize(&mut self, dw: i32, dh: i32) {
        let r = self.region();
        self.a = (r.x, r.y);
        self.b = (
            r.x + r.width.saturating_add(dw).clamp(1, self.width - r.x) - 1,
            r.y + r.height.saturating_add(dh).clamp(1, self.height - r.y) - 1,
        );
    }
}

pub fn crop(
    image: &crate::screenshot::Image,
    region: Region,
) -> anyhow::Result<crate::screenshot::Image> {
    anyhow::ensure!(
        region.x >= 0 && region.y >= 0 && region.width > 0 && region.height > 0,
        "invalid screenshot selection"
    );
    anyhow::ensure!(
        region.x.saturating_add(region.width) <= image.width as i32
            && region.y.saturating_add(region.height) <= image.height as i32,
        "screenshot selection crosses output bounds"
    );
    let stride = image.width as usize * 4;
    anyhow::ensure!(
        image.pixels.len() == stride * image.height as usize,
        "invalid screenshot buffer"
    );
    let row_len = region.width as usize * 4;
    let mut pixels = Vec::with_capacity(row_len * region.height as usize);
    for y in region.y..region.y + region.height {
        let offset = y as usize * stride + region.x as usize * 4;
        pixels.extend_from_slice(&image.pixels[offset..offset + row_len]);
    }
    Ok(crate::screenshot::Image {
        width: region.width as u32,
        height: region.height as u32,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_selection_is_the_middle_half_and_remembered_if_it_fits() {
        let s = Selection::new(800, 600, None);
        assert_eq!(
            s.region(),
            Region {
                x: 200,
                y: 150,
                width: 400,
                height: 300
            }
        );
        assert_eq!(
            Selection::new(800, 600, Some(s.region())).region(),
            s.region()
        );
        assert_eq!(
            Selection::new(100, 100, Some(s.region())).region().width,
            50
        );
    }
    #[test]
    fn backwards_drag_includes_both_pixels_and_cannot_span_outputs() {
        let mut s = Selection::new(200, 100, None);
        s.begin((199, 99));
        s.motion((-100, -100));
        s.end();
        assert_eq!(
            s.region(),
            Region {
                x: 0,
                y: 0,
                width: 200,
                height: 100
            }
        );
    }
    #[test]
    fn space_during_drag_moves_both_corners_then_release_resumes_resize() {
        let mut s = Selection::new(800, 600, None);
        s.begin((100, 100));
        s.motion((199, 199));
        s.space(true);
        s.motion((249, 229));
        assert_eq!(
            s.region(),
            Region {
                x: 150,
                y: 130,
                width: 100,
                height: 100
            }
        );
        s.space(false);
        s.motion((299, 279));
        s.end();
        assert_eq!(
            s.region(),
            Region {
                x: 150,
                y: 130,
                width: 150,
                height: 150
            }
        );
    }
    #[test]
    fn space_drag_existing_region_preserves_size_and_clamps_all_edges() {
        let mut s = Selection::new(800, 600, None);
        s.space(true);
        s.begin((400, 300));
        s.motion((2000, -2000));
        s.end();
        assert_eq!(
            s.region(),
            Region {
                x: 400,
                y: 0,
                width: 400,
                height: 300
            }
        );
        s.translate(-2000, 2000);
        assert_eq!(
            s.region(),
            Region {
                x: 0,
                y: 300,
                width: 400,
                height: 300
            }
        );
    }
    #[test]
    fn keyboard_resize_and_click_remain_nonempty() {
        let mut s = Selection::new(20, 20, None);
        s.resize(-100, -100);
        assert_eq!(s.region().width, 1);
        s.begin((19, 19));
        s.end();
        assert!(s.region().width > 0 && s.region().x + s.region().width <= 20);
    }
    #[test]
    fn crop_is_tightly_packed_bgra_not_overlay_pixels() {
        let image = crate::screenshot::Image {
            width: 3,
            height: 2,
            pixels: (0..24).collect(),
        };
        let result = crop(
            &image,
            Region {
                x: 1,
                y: 0,
                width: 2,
                height: 2,
            },
        )
        .unwrap();
        assert_eq!(
            result.pixels,
            [4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 19, 20, 21, 22, 23]
        );
        assert!(crop(
            &image,
            Region {
                x: 2,
                y: 0,
                width: 2,
                height: 1
            }
        )
        .is_err());
        assert!(crop(
            &image,
            Region {
                x: 0,
                y: 0,
                width: 0,
                height: 1
            }
        )
        .is_err());
    }
}
