use std::ops::{Add, Sub, Mul};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub loc: Point,
    pub size: Size,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            loc: Point { x, y },
            size: Size { w, h },
        }
    }

    pub fn right(self) -> i32 {
        self.loc.x + self.size.w as i32
    }

    pub fn bottom(self) -> i32 {
        self.loc.y + self.size.h as i32
    }

    pub fn center(self) -> Point {
        Point {
            x: self.loc.x + (self.size.w as i32) / 2,
            y: self.loc.y + (self.size.h as i32) / 2,
        }
    }

    pub fn contains_point(self, point: Point) -> bool {
        point.x >= self.loc.x
            && point.x < self.right()
            && point.y >= self.loc.y
            && point.y < self.bottom()
    }

    pub fn intersects(self, other: Rect) -> bool {
        self.loc.x < other.right()
            && self.right() > other.loc.x
            && self.loc.y < other.bottom()
            && self.bottom() > other.loc.y
    }
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

impl Size {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }
}

impl Add<Point> for Point {
    type Output = Point;

    fn add(self, other: Point) -> Point {
        Point::new(self.x + other.x, self.y + other.y)
    }
}

impl Sub<Point> for Point {
    type Output = Point;

    fn sub(self, other: Point) -> Point {
        Point::new(self.x - other.x, self.y - other.y)
    }
}

impl Mul<f32> for Point {
    type Output = Point;

    fn mul(self, scalar: f32) -> Point {
        Point::new(
            (self.x as f32 * scalar) as i32,
            (self.y as f32 * scalar) as i32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_new() {
        let r = Rect::new(10, 20, 800, 600);
        assert_eq!(r.loc.x, 10);
        assert_eq!(r.loc.y, 20);
        assert_eq!(r.size.w, 800);
        assert_eq!(r.size.h, 600);
    }

    #[test]
    fn test_rect_right_bottom() {
        let r = Rect::new(100, 200, 500, 400);
        assert_eq!(r.right(), 600);
        assert_eq!(r.bottom(), 600);
    }

    #[test]
    fn test_rect_center() {
        let r = Rect::new(0, 0, 800, 600);
        let c = r.center();
        assert_eq!(c.x, 400);
        assert_eq!(c.y, 300);
    }

    #[test]
    fn test_rect_center_odd_size() {
        let r = Rect::new(0, 0, 801, 601);
        let c = r.center();
        assert_eq!(c.x, 400);
        assert_eq!(c.y, 300);
    }

    #[test]
    fn test_contains_point_interior() {
        let r = Rect::new(0, 0, 100, 100);
        assert!(r.contains_point(Point::new(50, 50)));
        assert!(r.contains_point(Point::new(0, 0)));
        assert!(r.contains_point(Point::new(99, 99)));
    }

    #[test]
    fn test_contains_point_exterior() {
        let r = Rect::new(0, 0, 100, 100);
        assert!(!r.contains_point(Point::new(100, 50)));
        assert!(!r.contains_point(Point::new(50, 100)));
        assert!(!r.contains_point(Point::new(-1, 50)));
        assert!(!r.contains_point(Point::new(50, -1)));
    }

    #[test]
    fn test_intersects_overlapping() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 50, 100, 100);
        assert!(a.intersects(b));
        assert!(b.intersects(a));
    }

    #[test]
    fn test_intersects_non_overlapping() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(200, 200, 100, 100);
        assert!(!a.intersects(b));
    }

    #[test]
    fn test_intersects_touching() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(100, 0, 100, 100);
        assert!(!a.intersects(b));
    }

    #[test]
    fn test_intersects_contained() {
        let a = Rect::new(0, 0, 200, 200);
        let b = Rect::new(50, 50, 50, 50);
        assert!(a.intersects(b));
        assert!(b.intersects(a));
    }

    #[test]
    fn test_point_add_sub() {
        let a = Point::new(10, 20);
        let b = Point::new(5, 10);
        assert_eq!(a + b, Point::new(15, 30));
        assert_eq!(a - b, Point::new(5, 10));
    }

    #[test]
    fn test_point_mul_scalar() {
        let p = Point::new(100, 200);
        let scaled = p * 0.5;
        assert_eq!(scaled, Point::new(50, 100));
    }

    #[test]
    fn test_point_mul_zero() {
        let p = Point::new(100, 200);
        let scaled = p * 0.0;
        assert_eq!(scaled, Point::new(0, 0));
    }

    #[test]
    fn test_rect_default() {
        let r = Rect::default();
        assert_eq!(r.loc.x, 0);
        assert_eq!(r.loc.y, 0);
        assert_eq!(r.size.w, 0);
        assert_eq!(r.size.h, 0);
    }

    #[test]
    fn test_size_new() {
        let s = Size::new(1920, 1080);
        assert_eq!(s.w, 1920);
        assert_eq!(s.h, 1080);
    }
}
