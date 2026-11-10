//! Integer geometry in physical pixels: points, sizes and rectangles used by
//! layout, hit testing, drawing and accessibility bounds.

/// A position; y grows downwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    /// A point at (`x`, `y`).
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// A width and height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

impl Size {
    /// A size of `w` by `h`.
    pub const fn new(w: i32, h: i32) -> Self {
        Self { w, h }
    }
}

/// An axis-aligned rectangle; `w`/`h` ≤ 0 means empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// A rect with top-left at (`x`, `y`).
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// A rect of size `s` at the origin.
    pub const fn from_size(s: Size) -> Self {
        Self { x: 0, y: 0, w: s.w, h: s.h }
    }

    /// One past the last column inside the rect.
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    /// One past the last row inside the rect.
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// True when the rect has no area.
    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// The middle point, rounded towards the top-left.
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2, self.y + self.h / 2)
    }

    /// True when `p` is inside; the right and bottom edges are exclusive.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }

    /// The overlap of the two rects, or `None` when they do not overlap.
    pub fn intersect(&self, o: Rect) -> Option<Rect> {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        (r > x && b > y).then(|| Rect::new(x, y, r - x, b - y))
    }

    /// Smallest rect containing both (an empty rect contributes nothing).
    pub fn union(&self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }

    /// Shrinks by `d` on every side (negative `d` grows it).
    pub fn inset(&self, d: i32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2 * d, self.h - 2 * d)
    }

    /// The same rect moved by (`dx`, `dy`).
    pub fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Intersection, union, containment edges, inset and emptiness on simple cases.
    #[test]
    fn intersect_and_union() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(b), Some(Rect::new(5, 5, 5, 5)));
        assert_eq!(a.union(b), Rect::new(0, 0, 15, 15));
        assert_eq!(a.intersect(Rect::new(20, 20, 1, 1)), None);
        assert!(a.contains(Point::new(9, 9)));
        assert!(!a.contains(Point::new(10, 0)));
        assert_eq!(a.inset(2), Rect::new(2, 2, 6, 6));
        assert!(Rect::new(0, 0, 0, 5).is_empty());
        assert!(a.inset(6).is_empty());
    }
}
