use super::cache::GlyphCache;
use super::layout::{char_index_at_x, draw_text, line_height, measure};
use super::{FontKind, Fonts};
use crate::canvas::Canvas;
use crate::color::Color;
use crate::geom::Point;

fn fonts() -> Option<Fonts> {
    match Fonts::load(None, None) {
        Ok(f) => Some(f),
        Err(e) => {
            eprintln!("skipping: no system font found ({e})");
            None
        }
    }
}

#[test]
fn measure_grows_with_text() {
    let Some(f) = fonts() else { return };
    let mut c = GlyphCache::default();
    let a = measure(&f, &mut c, FontKind::Ui, "ab", 14.0);
    let b = measure(&f, &mut c, FontKind::Ui, "abcdef", 14.0);
    assert!(a > 0.0 && b > a, "{a} {b}");
    assert_eq!(measure(&f, &mut c, FontKind::Ui, "", 14.0), 0.0);
}

#[test]
fn draw_text_marks_pixels_inside_line_box() {
    let Some(f) = fonts() else { return };
    let mut c = GlyphCache::default();
    let mut canvas = Canvas::new(200, 60);
    let origin = Point::new(10, 10);
    let adv = draw_text(&mut canvas, &f, &mut c, FontKind::Ui, "Hello", 16.0, origin, Color::WHITE);
    let lh = line_height(&f, FontKind::Ui, 16.0);
    assert!(adv > 0.0);
    let mut lit = 0;
    for y in 0..60 {
        for x in 0..200 {
            if canvas.pixel(x, y) != 0 {
                lit += 1;
                assert!(x >= origin.x && (x as f32) <= origin.x as f32 + adv + 1.0, "x {x}");
                assert!(y >= origin.y && (y as f32) <= origin.y as f32 + lh, "y {y}");
            }
        }
    }
    assert!(lit > 20, "only {lit} pixels lit");
}

#[test]
fn cache_reuses_glyphs() {
    let Some(f) = fonts() else { return };
    let mut c = GlyphCache::default();
    let mut canvas = Canvas::new(200, 40);
    draw_text(&mut canvas, &f, &mut c, FontKind::Ui, "aab", 14.0, Point::new(0, 0), Color::WHITE);
    let n = c.len();
    assert_eq!(n, 2);
    draw_text(&mut canvas, &f, &mut c, FontKind::Ui, "abba", 14.0, Point::new(0, 0), Color::WHITE);
    assert_eq!(c.len(), n);
}

#[test]
fn missing_glyph_draws_box() {
    let Some(f) = fonts() else { return };
    let mut c = GlyphCache::default();
    let s = "\u{10FFFD}";
    let w = measure(&f, &mut c, FontKind::Ui, s, 16.0);
    assert!(w > 0.0);
    let mut canvas = Canvas::new(40, 40);
    draw_text(&mut canvas, &f, &mut c, FontKind::Ui, s, 16.0, Point::new(2, 2), Color::WHITE);
    assert!(canvas.pixels.iter().any(|&p| p != 0));
    // Same width for every missing char keeps layout stable.
    assert_eq!(w, measure(&f, &mut c, FontKind::Ui, "\u{10FFFC}", 16.0));
}

#[test]
fn mono_cell_is_uniform() {
    let Some(f) = fonts() else { return };
    let mut c = GlyphCache::default();
    let i = measure(&f, &mut c, FontKind::Mono, "i", 14.0);
    let w = measure(&f, &mut c, FontKind::Mono, "W", 14.0);
    assert_eq!(i, w);
    let (cw, ch) = f.cell_size(14.0);
    assert_eq!(cw, w);
    assert!(ch > cw);
}

#[test]
fn char_index_at_x_roundtrip() {
    let Some(f) = fonts() else { return };
    let mut c = GlyphCache::default();
    let s = "héllo wörld";
    let n = s.chars().count();
    for i in 0..=n {
        let prefix: String = s.chars().take(i).collect();
        let x = measure(&f, &mut c, FontKind::Ui, &prefix, 14.0);
        assert_eq!(char_index_at_x(&f, &mut c, FontKind::Ui, s, 14.0, x), i, "at {i}");
    }
    assert_eq!(char_index_at_x(&f, &mut c, FontKind::Ui, s, 14.0, -5.0), 0);
    assert_eq!(char_index_at_x(&f, &mut c, FontKind::Ui, s, 14.0, 10_000.0), n);
}
