//! The Skiff mark: a small boat whose sail is a terminal prompt `>` and
//! whose hull is the prompt's `_` cursor, on a rounded indigo tile.
//!
//! The mark is drawn from a handful of shapes in a unit square, each
//! covered by its signed distance (half a pixel of anti-aliasing), so one
//! description serves every size from 16 px to 1024 px. Small sizes use a
//! simpler, bolder variant laid out on a 16-unit grid so its edges land on
//! whole pixels. `rgba` feeds the window icon and the generated app icons;
//! `svg` writes the same geometry as `assets/icon/skiff.svg`.

/// Sizes up to this many pixels use the bold small-size variant.
pub const SMALL_MAX: u32 = 32;

/// A straight (non-premultiplied) RGBA colour, 0–1 floats.
type Rgba = [f32; 4];

/// Builds an opaque colour from `0xRRGGBB`.
const fn hex(c: u32) -> Rgba {
    [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0, 1.0]
}

/// Tile gradient, top and bottom (the app's indigo accent family).
pub const TILE_TOP: u32 = 0x5b74f2;
/// Tile gradient, bottom.
pub const TILE_BOTTOM: u32 = 0x2f43bd;
/// The sail (`>`): near-white.
pub const SAIL: u32 = 0xf7f8ff;
/// The hull (`_`): a pale tint of the accent so the two read apart.
pub const HULL: u32 = 0xb9c4ff;

/// One shape of the mark, in unit coordinates (0–1, y down).
enum Shape {
    /// A rounded rectangle.
    RRect { x: f32, y: f32, w: f32, h: f32, r: f32 },
    /// A polyline stroked with round caps and joins, `half` its half width.
    Stroke { pts: Vec<(f32, f32)>, half: f32 },
    /// A filled polygon, its corners rounded by `round`; `path`, when set,
    /// is the exact SVG path data the polygon approximates, with its grid.
    Poly { pts: Vec<(f32, f32)>, round: f32, path: Option<(f32, String)> },
}

/// How a shape is coloured.
enum Paint {
    /// One colour.
    Solid(u32),
    /// A top-to-bottom gradient across the shape's own height range.
    Vertical(u32, u32, f32, f32),
}

/// Points `(x, y)` on a `grid`-unit layout scaled to unit coordinates.
fn on_grid(grid: f32, pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    pts.iter().map(|&(x, y)| (x / grid, y / grid)).collect()
}

/// A skiff's hull on a `grid`-unit layout: a straight deck from the
/// transom (`stern`) to the raised bow tip, and a bottom that curves from
/// the bow back to the foot of the transom through `keel` (a quadratic
/// Bézier control point). The SVG path keeps the exact curve.
fn hull(grid: f32, stern: (f32, f32), bow: (f32, f32), keel: (f32, f32), foot: (f32, f32), round: f32) -> Shape {
    let mut pts = vec![stern, bow];
    const STEPS: usize = 48;
    for i in 1..STEPS {
        let t = i as f32 / STEPS as f32;
        let u = 1.0 - t;
        pts.push((u * u * bow.0 + 2.0 * u * t * keel.0 + t * t * foot.0, u * u * bow.1 + 2.0 * u * t * keel.1 + t * t * foot.1));
    }
    pts.push(foot);
    let path = format!("M{},{} L{},{} Q{},{} {},{} Z", stern.0, stern.1, bow.0, bow.1, keel.0, keel.1, foot.0, foot.1);
    Shape::Poly { pts: on_grid(grid, &pts), round: round / grid, path: Some((grid, path)) }
}

/// The large mark (48 px and up), on a 64-unit grid.
fn large() -> Vec<(Shape, Paint)> {
    const G: f32 = 64.0;
    vec![
        (
            Shape::RRect { x: 2.0 / G, y: 2.0 / G, w: 60.0 / G, h: 60.0 / G, r: 14.0 / G },
            Paint::Vertical(TILE_TOP, TILE_BOTTOM, 2.0 / G, 62.0 / G),
        ),
        (Shape::Stroke { pts: on_grid(G, &[(23.0, 12.0), (41.0, 25.0), (23.0, 38.0)]), half: 4.25 / G }, Paint::Solid(SAIL)),
        (hull(G, (12.5, 43.0), (54.0, 40.5), (45.0, 54.0), (17.0, 51.5), 1.25), Paint::Solid(HULL)),
    ]
}

/// The small mark (up to `SMALL_MAX` px), on a 16-unit grid: a bolder
/// sail and hull and a tighter tile corner, edges on whole pixels.
fn small() -> Vec<(Shape, Paint)> {
    const G: f32 = 16.0;
    vec![
        (Shape::RRect { x: 0.0, y: 0.0, w: 1.0, h: 1.0, r: 3.5 / G }, Paint::Vertical(TILE_TOP, TILE_BOTTOM, 0.0, 1.0)),
        (Shape::Stroke { pts: on_grid(G, &[(5.5, 2.5), (10.0, 6.0), (5.5, 9.5)]), half: 1.0 / G }, Paint::Solid(SAIL)),
        (
            Shape::Poly { pts: on_grid(G, &[(2.0, 10.0), (15.0, 10.0), (12.0, 13.0), (3.0, 13.0), (2.0, 12.0)]), round: 0.0, path: None },
            Paint::Solid(HULL),
        ),
    ]
}

/// Distance from `p` to the segment `a`–`b`.
fn dist_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (ex, ey) = (p.0 - (a.0 + t * dx), p.1 - (a.1 + t * dy));
    (ex * ex + ey * ey).sqrt()
}

/// Signed distance from `p` to a polygon (negative inside).
fn sd_poly(p: (f32, f32), pts: &[(f32, f32)]) -> f32 {
    let mut d = f32::MAX;
    let mut inside = false;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        d = d.min(dist_segment(p, a, b));
        // Even-odd crossing test on a horizontal ray.
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) * (b.0 - a.0) / (b.1 - a.1) {
            inside = !inside;
        }
    }
    if inside { -d } else { d }
}

/// Signed distance from `p` to `shape` (negative inside), unit coordinates.
fn sd(shape: &Shape, p: (f32, f32)) -> f32 {
    match shape {
        Shape::RRect { x, y, w, h, r } => {
            let (cx, cy) = (x + w / 2.0, y + h / 2.0);
            let qx = (p.0 - cx).abs() - (w / 2.0 - r);
            let qy = (p.1 - cy).abs() - (h / 2.0 - r);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            outside + qx.max(qy).min(0.0) - r
        }
        Shape::Stroke { pts, half } => pts.windows(2).map(|s| dist_segment(p, s[0], s[1])).fold(f32::MAX, f32::min) - half,
        Shape::Poly { pts, round, .. } => sd_poly(p, pts) - round,
    }
}

/// The colour of `paint` at height `y` (unit coordinates).
fn color_at(paint: &Paint, y: f32) -> Rgba {
    match *paint {
        Paint::Solid(c) => hex(c),
        Paint::Vertical(top, bottom, y0, y1) => {
            let t = ((y - y0) / (y1 - y0)).clamp(0.0, 1.0);
            let (a, b) = (hex(top), hex(bottom));
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, 1.0]
        }
    }
}

/// The mark at `size`×`size` pixels as straight RGBA bytes, row by row,
/// transparent outside the tile.
pub fn rgba(size: u32) -> Vec<u8> {
    let shapes = if size <= SMALL_MAX { small() } else { large() };
    let n = size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            // Pixel centre in unit coordinates.
            let p = ((px as f32 + 0.5) / n, (py as f32 + 0.5) / n);
            // Composite front to back over transparent, premultiplied.
            let mut acc = [0.0f32; 4];
            for (shape, paint) in &shapes {
                let cover = (0.5 - sd(shape, p) * n).clamp(0.0, 1.0);
                if cover <= 0.0 {
                    continue;
                }
                let c = color_at(paint, p.1);
                let a = c[3] * cover;
                for i in 0..3 {
                    acc[i] = c[i] * a + acc[i] * (1.0 - a);
                }
                acc[3] = a + acc[3] * (1.0 - a);
            }
            let a = acc[3];
            let un = |v: f32| if a > 0.0 { ((v / a).clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 };
            out.extend_from_slice(&[un(acc[0]), un(acc[1]), un(acc[2]), (a * 255.0).round() as u8]);
        }
    }
    out
}

/// The mark as an SVG document on a 1024 (or, for `small`, 16) unit
/// canvas: the same shapes `rgba` draws, for the master icon files.
pub fn svg(small: bool) -> String {
    let (shapes, n) = if small { (self::small(), 16.0) } else { (large(), 1024.0) };
    let f = |v: f32| {
        let s = format!("{:.2}", v * n);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    let col = |c: u32| format!("#{c:06x}");
    let pts = |p: &[(f32, f32)]| p.iter().map(|&(x, y)| format!("{},{}", f(x), f(y))).collect::<Vec<_>>().join(" ");
    let mut lines = vec![
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{n}\" height=\"{n}\" viewBox=\"0 0 {n} {n}\">"),
        "<title>Skiff</title>".to_string(),
    ];
    for (i, (shape, paint)) in shapes.iter().enumerate() {
        let fill = match *paint {
            Paint::Solid(c) => col(c),
            Paint::Vertical(top, bottom, y0, y1) => {
                lines.push(format!(
                    "<defs><linearGradient id=\"g{i}\" x1=\"0\" y1=\"{}\" x2=\"0\" y2=\"{}\" gradientUnits=\"userSpaceOnUse\">\
                     <stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/></linearGradient></defs>",
                    f(y0),
                    f(y1),
                    col(top),
                    col(bottom)
                ));
                format!("url(#g{i})")
            }
        };
        lines.push(match shape {
            Shape::RRect { x, y, w, h, r } => {
                format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"{fill}\"/>", f(*x), f(*y), f(*w), f(*h), f(*r))
            }
            Shape::Stroke { pts: p, half } => format!(
                "<polyline points=\"{}\" fill=\"none\" stroke=\"{fill}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
                pts(p),
                f(half * 2.0)
            ),
            // The exact curve, in its own grid units scaled up.
            Shape::Poly { path: Some((grid, d)), round, .. } => format!(
                "<path transform=\"scale({})\" d=\"{d}\" fill=\"{fill}\" stroke=\"{fill}\" stroke-width=\"{}\" stroke-linejoin=\"round\"/>",
                n / grid,
                round * 2.0 * grid
            ),
            Shape::Poly { pts: p, .. } => format!("<polygon points=\"{}\" fill=\"{fill}\"/>", pts(p)),
        });
    }
    lines.push("</svg>".to_string());
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every size is the right length, the tile is opaque in the middle
    /// and the sail is drawn in its own colour.
    #[test]
    fn mark_rasterizes_at_every_size() {
        for size in [16u32, 24, 32, 48, 64, 256] {
            let px = rgba(size);
            assert_eq!(px.len(), (size * size * 4) as usize);
            let at = |x: u32, y: u32| &px[((y * size + x) * 4) as usize..][..4];
            assert_eq!(at(size / 2, size * 7 / 8)[3], 255, "tile opaque ({size})");
            assert!(at(0, 0)[3] < 255, "rounded corner ({size})");
        }
        // The 16 px sail's vertex pixel is the sail colour.
        let px = rgba(16);
        let p = &px[((6 * 16 + 9) * 4) as usize..][..3];
        assert!(p.iter().all(|&v| v > 220), "{p:?}");
    }

    /// The committed master SVGs match the mark (rerun `gen_icons` after
    /// changing it).
    #[test]
    fn committed_svgs_are_current() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/icon/");
        for (file, small) in [("skiff.svg", false), ("skiff-small.svg", true)] {
            let on_disk = std::fs::read_to_string(format!("{dir}{file}")).expect("icon file").replace('\r', "");
            assert_eq!(on_disk, svg(small), "{file} is stale: run the gen_icons example");
        }
    }
}
