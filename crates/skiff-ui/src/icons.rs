//! In-house vector icons: each icon is a few strokes, arcs and dots on a
//! 16-unit grid, rasterized with anti-aliasing into a coverage mask at the
//! size it is drawn (so icons stay sharp at any zoom). Masks are cached per
//! (icon, size) in the `GlyphCache` and blended like glyphs; widgets draw
//! them with `Painter::icon`, which pushes a `display::Prim::Icon`.
//!
//! Coverage comes from distance fields: a stroke covers a pixel by how far
//! its centre lies within half the stroke width of the shape (plus half a
//! pixel of anti-aliasing); shapes are combined with `max`.

use crate::style::tokens;

/// The icon set. Directional icons name the way they point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Host,
    Folder,
    File,
    Link,
    Terminal,
    Transfer,
    Forward,
    Settings,
    Search,
    Plus,
    Minus,
    Close,
    ChevronUp,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Ellipsis,
    /// Two panes side by side.
    SplitH,
    /// Two panes stacked.
    SplitV,
    Upload,
    Download,
    Refresh,
    Key,
    Warning,
    Error,
    Info,
    Check,
    Pin,
    Eye,
    EyeOff,
    Trash,
    Copy,
    Edit,
}

impl Icon {
    /// Every icon, for showcases and tests.
    pub const ALL: [Icon; 33] = [
        Icon::Host,
        Icon::Folder,
        Icon::File,
        Icon::Link,
        Icon::Terminal,
        Icon::Transfer,
        Icon::Forward,
        Icon::Settings,
        Icon::Search,
        Icon::Plus,
        Icon::Minus,
        Icon::Close,
        Icon::ChevronUp,
        Icon::ChevronDown,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::Ellipsis,
        Icon::SplitH,
        Icon::SplitV,
        Icon::Upload,
        Icon::Download,
        Icon::Refresh,
        Icon::Key,
        Icon::Warning,
        Icon::Error,
        Icon::Info,
        Icon::Check,
        Icon::Pin,
        Icon::Eye,
        Icon::EyeOff,
        Icon::Trash,
        Icon::Copy,
        Icon::Edit,
    ];
}

/// One drawing step on the 16-unit grid (y grows downwards; angles in
/// degrees, clockwise from +x).
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// A stroked segment with round caps.
    Line(f32, f32, f32, f32),
    /// A stroked arc: centre, radius, start and end angle (start < end).
    Arc(f32, f32, f32, f32, f32),
    /// A stroked circle: centre and radius.
    Circle(f32, f32, f32),
    /// A filled disc: centre and radius.
    Disc(f32, f32, f32),
    /// A stroked rounded rectangle: x, y, w, h, corner radius.
    RRect(f32, f32, f32, f32, f32),
}

use Shape::{Arc, Circle, Disc, Line, RRect};

/// Segments joining consecutive points.
fn poly(pts: &[(f32, f32)]) -> Vec<Shape> {
    pts.windows(2).map(|w| Line(w[0].0, w[0].1, w[1].0, w[1].1)).collect()
}

/// The shapes `icon` is drawn with.
fn shapes(icon: Icon) -> Vec<Shape> {
    let tray = poly(&[(2.5, 10.5), (2.5, 13.5), (13.5, 13.5), (13.5, 10.5)]);
    match icon {
        Icon::Host => {
            vec![RRect(2.0, 2.5, 12.0, 4.75, 1.5), RRect(2.0, 8.75, 12.0, 4.75, 1.5), Disc(4.75, 4.9, 0.9), Disc(4.75, 11.1, 0.9)]
        }
        Icon::Folder => poly(&[(2.0, 4.0), (6.0, 4.0), (7.5, 5.5), (14.0, 5.5), (14.0, 13.0), (2.0, 13.0), (2.0, 4.0)]),
        Icon::File => {
            let mut v = poly(&[(3.5, 2.0), (9.5, 2.0), (12.5, 5.0), (12.5, 14.0), (3.5, 14.0), (3.5, 2.0)]);
            v.extend(poly(&[(9.5, 2.0), (9.5, 5.0), (12.5, 5.0)]));
            v
        }
        Icon::Link => vec![
            Arc(5.0, 8.0, 3.0, 90.0, 270.0),
            Line(5.0, 5.0, 6.5, 5.0),
            Line(5.0, 11.0, 6.5, 11.0),
            Arc(11.0, 8.0, 3.0, -90.0, 90.0),
            Line(9.5, 5.0, 11.0, 5.0),
            Line(9.5, 11.0, 11.0, 11.0),
            Line(5.5, 8.0, 10.5, 8.0),
        ],
        Icon::Terminal => {
            let mut v = vec![RRect(2.0, 2.75, 12.0, 10.5, 2.0), Line(8.5, 10.5, 11.5, 10.5)];
            v.extend(poly(&[(4.5, 6.0), (6.5, 8.0), (4.5, 10.0)]));
            v
        }
        Icon::Transfer => {
            let mut v = vec![Line(5.0, 13.0, 5.0, 3.0), Line(11.0, 3.0, 11.0, 13.0)];
            v.extend(poly(&[(2.5, 5.5), (5.0, 3.0), (7.5, 5.5)]));
            v.extend(poly(&[(8.5, 10.5), (11.0, 13.0), (13.5, 10.5)]));
            v
        }
        Icon::Forward => {
            let mut v = vec![Circle(3.75, 8.0, 1.75), Line(6.0, 8.0, 13.5, 8.0)];
            v.extend(poly(&[(10.5, 5.0), (13.5, 8.0), (10.5, 11.0)]));
            v
        }
        Icon::Settings => {
            // Eight square-ish teeth around a hub: an outline alternating
            // between the tooth tips (r 6.25) and the valleys (r 4.5).
            let at = |r: f32, deg: f32| (8.0 + r * deg.to_radians().cos(), 8.0 + r * deg.to_radians().sin());
            let mut pts = vec![];
            for i in 0..8 {
                let a = i as f32 * 45.0;
                pts.extend([at(4.5, a - 15.0), at(6.25, a - 9.0), at(6.25, a + 9.0), at(4.5, a + 15.0)]);
            }
            pts.push(pts[0]);
            let mut v = poly(&pts);
            v.push(Circle(8.0, 8.0, 2.0));
            v
        }
        Icon::Search => vec![Circle(7.0, 7.0, 4.25), Line(10.2, 10.2, 13.5, 13.5)],
        Icon::Plus => vec![Line(8.0, 3.0, 8.0, 13.0), Line(3.0, 8.0, 13.0, 8.0)],
        Icon::Minus => vec![Line(3.0, 8.0, 13.0, 8.0)],
        Icon::Close => vec![Line(4.0, 4.0, 12.0, 12.0), Line(12.0, 4.0, 4.0, 12.0)],
        Icon::ChevronUp => poly(&[(4.0, 10.0), (8.0, 6.0), (12.0, 10.0)]),
        Icon::ChevronDown => poly(&[(4.0, 6.0), (8.0, 10.0), (12.0, 6.0)]),
        Icon::ChevronLeft => poly(&[(10.0, 4.0), (6.0, 8.0), (10.0, 12.0)]),
        Icon::ChevronRight => poly(&[(6.0, 4.0), (10.0, 8.0), (6.0, 12.0)]),
        Icon::Ellipsis => vec![Disc(3.5, 8.0, 1.25), Disc(8.0, 8.0, 1.25), Disc(12.5, 8.0, 1.25)],
        Icon::SplitH => vec![RRect(2.0, 2.75, 12.0, 10.5, 1.5), Line(8.0, 3.0, 8.0, 13.0)],
        Icon::SplitV => vec![RRect(2.0, 2.75, 12.0, 10.5, 1.5), Line(2.5, 8.0, 13.5, 8.0)],
        Icon::Upload => {
            let mut v = tray;
            v.push(Line(8.0, 10.0, 8.0, 2.5));
            v.extend(poly(&[(4.75, 5.75), (8.0, 2.5), (11.25, 5.75)]));
            v
        }
        Icon::Download => {
            let mut v = tray;
            v.push(Line(8.0, 2.5, 8.0, 10.0));
            v.extend(poly(&[(4.75, 6.75), (8.0, 10.0), (11.25, 6.75)]));
            v
        }
        Icon::Refresh => vec![Arc(8.0, 8.0, 5.0, 40.0, 320.0), Line(11.83, 4.79, 9.1, 4.55), Line(11.83, 4.79, 12.05, 2.05)],
        Icon::Key => vec![Circle(5.0, 11.0, 2.75), Line(7.0, 9.0, 13.5, 2.5), Line(11.0, 5.0, 12.75, 6.75), Line(9.25, 6.75, 10.5, 8.0)],
        Icon::Warning => {
            let mut v = poly(&[(8.0, 2.25), (14.25, 13.25), (1.75, 13.25), (8.0, 2.25)]);
            v.extend([Line(8.0, 6.25, 8.0, 9.0), Disc(8.0, 11.1, 0.9)]);
            v
        }
        Icon::Error => vec![Circle(8.0, 8.0, 6.0), Line(5.75, 5.75, 10.25, 10.25), Line(10.25, 5.75, 5.75, 10.25)],
        Icon::Info => vec![Circle(8.0, 8.0, 6.0), Line(8.0, 7.5, 8.0, 11.0), Disc(8.0, 5.0, 0.9)],
        Icon::Check => poly(&[(3.0, 8.5), (6.5, 12.0), (13.0, 4.5)]),
        Icon::Pin => {
            let mut v = vec![Line(5.0, 2.5, 11.0, 2.5), Line(4.0, 10.0, 12.0, 10.0), Line(8.0, 10.0, 8.0, 14.0)];
            v.extend(poly(&[(6.5, 2.5), (6.5, 6.5), (4.0, 10.0)]));
            v.extend(poly(&[(9.5, 2.5), (9.5, 6.5), (12.0, 10.0)]));
            v
        }
        Icon::Eye => vec![Arc(8.0, 10.5, 6.5, 202.6, 337.4), Arc(8.0, 5.5, 6.5, 22.6, 157.4), Circle(8.0, 8.0, 2.0)],
        Icon::EyeOff => {
            vec![Arc(8.0, 10.5, 6.5, 202.6, 337.4), Arc(8.0, 5.5, 6.5, 22.6, 157.4), Circle(8.0, 8.0, 2.0), Line(2.5, 2.5, 13.5, 13.5)]
        }
        Icon::Trash => {
            let mut v = vec![Line(2.5, 4.0, 13.5, 4.0), Line(6.5, 7.0, 6.5, 11.0), Line(9.5, 7.0, 9.5, 11.0)];
            v.extend(poly(&[(6.0, 4.0), (6.0, 2.25), (10.0, 2.25), (10.0, 4.0)]));
            v.extend(poly(&[(3.75, 4.0), (4.5, 13.75), (11.5, 13.75), (12.25, 4.0)]));
            v
        }
        Icon::Copy => {
            let mut v = vec![RRect(5.5, 5.5, 8.5, 8.5, 1.5)];
            v.extend(poly(&[(2.5, 10.5), (2.5, 2.5), (10.5, 2.5)]));
            v
        }
        Icon::Edit => {
            let mut v = poly(&[(3.0, 13.0), (3.5, 10.25), (10.75, 3.0), (13.0, 5.25), (5.75, 12.5), (3.0, 13.0)]);
            v.push(Line(9.25, 4.5, 11.5, 6.75));
            v
        }
    }
}

/// Distance from `p` to segment `a`–`b`.
fn dist_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
}

/// Distance from `p` to the boundary of a rounded rectangle.
fn dist_rrect(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let qx = (px - cx).abs() - (w / 2.0 - r);
    let qy = (py - cy).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    (outside + qx.max(qy).min(0.0) - r).abs()
}

/// Coverage (0..=1) of the pixel centred at grid point `p` by `s`, where
/// `unit` is pixels per grid unit and `half` half the stroke in pixels.
fn coverage(s: &Shape, px: f32, py: f32, unit: f32, half: f32) -> f32 {
    let stroke = |d: f32| (half + 0.5 - d * unit).clamp(0.0, 1.0);
    match *s {
        Line(ax, ay, bx, by) => stroke(dist_segment(px, py, ax, ay, bx, by)),
        Circle(cx, cy, r) => stroke((((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r).abs()),
        Disc(cx, cy, r) => ((r - ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()) * unit + 0.5).clamp(0.0, 1.0),
        RRect(x, y, w, h, r) => stroke(dist_rrect(px, py, x, y, w, h, r)),
        Arc(cx, cy, r, a0, a1) => {
            let ang = (py - cy).atan2(px - cx).to_degrees();
            // Bring the angle into [a0, a0 + 360).
            let a = a0 + (ang - a0).rem_euclid(360.0);
            let d = if a <= a1 {
                (((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r).abs()
            } else {
                let end = |deg: f32| {
                    let t = deg.to_radians();
                    ((px - cx - r * t.cos()).powi(2) + (py - cy - r * t.sin()).powi(2)).sqrt()
                };
                end(a0).min(end(a1))
            };
            stroke(d)
        }
    }
}

/// `icon` rasterized into a `size`×`size` coverage mask (row-major, one
/// byte per pixel). Strokes are `tokens::ICON_STROKE` at 16 px and scale
/// with the size, never thinner than one pixel.
pub fn rasterize(icon: Icon, size: i32) -> Vec<u8> {
    let size = size.max(1);
    let unit = size as f32 / tokens::ICON as f32;
    let half = (tokens::ICON_STROKE * unit).max(1.0) / 2.0;
    let shapes = shapes(icon);
    let mut out = vec![0u8; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let (gx, gy) = ((x as f32 + 0.5) / unit, (y as f32 + 0.5) / unit);
            let c = shapes.iter().map(|s| coverage(s, gx, gy, unit, half)).fold(0.0f32, f32::max);
            out[(y * size + x) as usize] = (c * 255.0).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon draws a reasonable amount of ink at 16 and 32 px, keeps
    /// a clear margin at the edges, and no two icons rasterize the same.
    #[test]
    fn icons_rasterize_sanely() {
        let mut seen = std::collections::HashMap::new();
        for icon in Icon::ALL {
            for size in [16, 32] {
                let m = rasterize(icon, size);
                let ink: u32 = m.iter().map(|&c| c as u32).sum::<u32>() / 255;
                let area = (size * size) as u32;
                assert!(ink > area / 30 && ink < area / 2, "{icon:?}@{size}: ink {ink} of {area}");
                // The outermost ring of pixels stays almost empty.
                let s = size as usize;
                let edge: u32 = (0..s).flat_map(|i| [m[i], m[(s - 1) * s + i], m[i * s], m[i * s + s - 1]]).map(|c| c as u32).sum();
                assert!(edge < 255 * 4, "{icon:?}@{size}: edge ink {edge}");
            }
            assert!(seen.insert(rasterize(icon, 16), icon).is_none(), "{icon:?} duplicates another icon");
        }
    }

    /// Symmetric icons come out mirror-symmetric, and strokes are
    /// anti-aliased (partial coverage exists).
    #[test]
    fn icons_are_symmetric_and_antialiased() {
        let m = rasterize(Icon::Plus, 16);
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(m[y * 16 + x], m[y * 16 + 15 - x], "plus not symmetric at {x},{y}");
            }
        }
        let c = rasterize(Icon::Search, 16);
        assert!(c.iter().any(|&v| v > 0 && v < 200), "no partial coverage");
    }
}
