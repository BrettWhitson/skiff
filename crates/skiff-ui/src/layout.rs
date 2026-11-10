//! Splitting rectangles into rows and columns: the layout primitive behind
//! `Ui::split_h` / `Ui::split_v`.

use crate::geom::Rect;

/// How much of the main axis a part takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Size {
    /// Exact pixels (physical here; `Ui::split_*` scales logical values).
    Px(i32),
    /// Fraction of the space left after gaps.
    Frac(f32),
    /// An equal share of whatever remains.
    Fill,
}

/// Start offsets and lengths along one axis. Edges are rounded from
/// cumulative positions, so parts never leave gaps or overlap.
fn spans(start: i32, len: i32, sizes: &[Size], gap: i32) -> Vec<(i32, i32)> {
    let n = sizes.len();
    if n == 0 {
        return vec![];
    }
    // Space for the parts themselves once the gaps are taken out.
    let avail = (len - gap * (n as i32 - 1)).max(0) as f32;
    // Pixel and fraction parts are claimed first; `Fill` parts split the rest.
    let fixed: f32 = sizes
        .iter()
        .map(|s| match s {
            Size::Px(p) => (*p).max(0) as f32,
            Size::Frac(f) => f.max(0.0) * avail,
            Size::Fill => 0.0,
        })
        .sum();
    let fills = sizes.iter().filter(|s| matches!(s, Size::Fill)).count();
    let share = if fills > 0 { (avail - fixed).max(0.0) / fills as f32 } else { 0.0 };
    let end = start + len.max(0);
    // Walk the parts accumulating float lengths; each part spans the rounded
    // difference of consecutive cumulative positions, clamped to `avail` and the
    // outer end so oversized fixed parts shrink instead of overflowing.
    let mut cum = 0.0f32;
    sizes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let want = match s {
                Size::Px(p) => (*p).max(0) as f32,
                Size::Frac(f) => f.max(0.0) * avail,
                Size::Fill => share,
            };
            let a = cum.round().min(avail) as i32;
            cum += want;
            let b = cum.round().min(avail) as i32;
            let x = (start + a + gap * i as i32).min(end);
            (x, (b - a).min(end - x).max(0))
        })
        .collect()
}

/// Columns, left to right.
pub fn split_h(r: Rect, sizes: &[Size], gap: i32) -> Vec<Rect> {
    spans(r.x, r.w, sizes, gap).into_iter().map(|(x, w)| Rect::new(x, r.y, w, r.h)).collect()
}

/// Rows, top to bottom.
pub fn split_v(r: Rect, sizes: &[Size], gap: i32) -> Vec<Rect> {
    spans(r.y, r.h, sizes, gap).into_iter().map(|(y, h)| Rect::new(r.x, y, r.w, h)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that `rects` cover `outer` edge to edge on the x axis with exactly
    /// `gap` between neighbours, keeping `outer`'s y and height.
    fn check_contiguous(rects: &[Rect], outer: Rect, gap: i32) {
        assert_eq!(rects.first().unwrap().x, outer.x);
        assert_eq!(rects.last().unwrap().right(), outer.right());
        for w in rects.windows(2) {
            assert_eq!(w[0].right() + gap, w[1].x, "{rects:?}");
        }
        for r in rects {
            assert!(r.w >= 0);
            assert_eq!((r.y, r.h), (outer.y, outer.h));
        }
    }

    /// Mixed sizes cover the whole width at any size, keep fixed parts exact, and
    /// split `Fill` evenly (to within a pixel).
    #[test]
    fn split_fills_exactly() {
        for w in [0, 1, 7, 100, 101, 333, 1919] {
            let outer = Rect::new(5, 3, w, 10);
            let r = split_h(outer, &[Size::Fill, Size::Fill, Size::Fill], 0);
            check_contiguous(&r, outer, 0);
            let r = split_h(outer, &[Size::Px(20), Size::Frac(0.3), Size::Fill], 0);
            check_contiguous(&r, outer, 0);
        }
        let outer = Rect::new(0, 0, 101, 10);
        let r = split_h(outer, &[Size::Fill, Size::Px(30), Size::Fill], 4);
        check_contiguous(&r, outer, 4);
        assert_eq!(r[1].w, 30);
        assert!((r[0].w - r[2].w).abs() <= 1);
        let v = split_v(Rect::new(0, 0, 10, 99), &[Size::Frac(0.5), Size::Fill], 0);
        assert_eq!((v[0].h, v[1].y, v[1].bottom()), (50, 50, 99));
    }

    /// Fixed parts larger than the space are clipped instead of spilling past the edge.
    #[test]
    fn oversized_fixed_parts_never_overflow() {
        let outer = Rect::new(0, 0, 50, 10);
        let r = split_h(outer, &[Size::Px(40), Size::Px(40), Size::Fill], 0);
        assert!(r.iter().all(|r| r.w >= 0 && r.right() <= outer.right()), "{r:?}");
    }
}
