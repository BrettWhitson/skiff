//! Tile-renderer tests that need bigger scenes: overhanging glyphs at tile
//! seams, full-repaint cost, multi-threaded bands and the opaque-cover skip.

use std::sync::Arc;
use std::time::Instant;

use super::*;
use crate::text::{FontStack, GlyphBitmap, GlyphRasterizer, LineMetrics};
use crate::ui::Ui;
use crate::ui::testing::Harness;

/// Glyphs that overhang their advance box on every side, like Segoe 'j'
/// (left) or box-drawing chars (above/below the line box).
struct OverhangFont;
impl GlyphRasterizer for OverhangFont {
    /// Every char has a glyph.
    fn has_glyph(&self, _: char) -> bool {
        true
    }
    /// Fixed 8px advance.
    fn advance(&self, _: char, _: f32) -> f32 {
        8.0
    }
    /// A solid 14x22 block that overhangs the 8px advance on all sides.
    fn rasterize(&self, _: char, _: f32) -> GlyphBitmap {
        GlyphBitmap { width: 14, height: 22, xmin: -3, ymin: -7, advance: 8.0, coverage: vec![255; 14 * 22] }
    }
    /// Fixed 12 / -4 ascent / descent, no gap.
    fn line_metrics(&self, _: f32) -> LineMetrics {
        LineMetrics { ascent: 12.0, descent: -4.0, line_gap: 0.0 }
    }
}

/// Glyphs that overhang into neighbouring tiles are repainted there, so
/// the tiled result equals a full render.
#[test]
fn overhanging_glyphs_repaint_across_tile_seams() {
    let f: Arc<dyn GlyphRasterizer> = Arc::new(OverhangFont);
    let fonts = Fonts::new(FontStack::new(f.clone()), FontStack::new(f));
    let mut h = Harness::with_fonts(200, 140, 1.0, fonts);
    let bg = h.style.palette.bg;
    let mut cache = GlyphCache::default();
    let mut tiled = Canvas::new(200, 140);
    let mut r = TileRenderer::default();

    h.frame(vec![], &mut |_ui: &mut Ui| {});
    r.render(h.list(), &mut tiled, &h.fonts, &mut cache, bg);
    // Text whose line box starts just right of / below a tile seam.
    h.frame(vec![], &mut |ui: &mut Ui| {
        ui.painter().text(Point::new(66, 66), "ab", FontKind::Mono, 14.0, Color::WHITE);
    });
    r.render(h.list(), &mut tiled, &h.fonts, &mut cache, bg);
    let mut full = Canvas::new(200, 140);
    render_full(h.list(), &mut full, &h.fonts, &mut cache, bg);
    let diff = tiled.pixels.iter().zip(&full.pixels).filter(|(a, b)| a != b).count();
    assert_eq!(diff, 0, "{diff} pixels differ between tiled and full render");
}

/// A screen full of monospace text, all tiles dirty (a scrolling terminal).
/// With every tile dirty, tiled rendering costs at most about twice a
/// single full pass (skipped without system fonts).
#[test]
fn tiled_full_repaint_costs_about_one_pass() {
    let Ok(fonts) = Fonts::load(None, None) else {
        eprintln!("skipping: no system fonts");
        return;
    };
    let (w, h) = (1920, 1080);
    let mut cache = GlyphCache::default();
    let px = 16.0;
    let lh = crate::text::layout::line_height(&fonts, FontKind::Mono, px) as i32;
    let mut list = DisplayList::default();
    let line: String = "fn main() { let x = 42; println!(\"{x}\"); } // │ ┼ █ ".chars().cycle().take(240).collect();
    let lw = crate::text::layout::measure(&fonts, &mut cache, FontKind::Mono, &line, px).ceil() as i32;
    for row in 0..h / lh {
        let origin = Point::new(4, row * lh);
        list.push(Prim::Text {
            origin,
            text: line.clone(),
            font: FontKind::Mono,
            px,
            color: Color::WHITE,
            bounds: Rect::new(origin.x, origin.y, lw, lh),
        });
    }
    let mut canvas = Canvas::new(w, h);
    render_full(&list, &mut canvas, &fonts, &mut cache, Color::BLACK); // warm the glyph cache

    let t = Instant::now();
    render_full(&list, &mut canvas, &fonts, &mut cache, Color::BLACK);
    let single = t.elapsed().as_secs_f64() * 1000.0;
    let mut r = TileRenderer::default();
    let t = Instant::now();
    r.render(&list, &mut canvas, &fonts, &mut cache, Color::BLACK);
    let tiled = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!("1080p text screen: single pass {single:.1} ms, tiled (all dirty) {tiled:.1} ms");
    assert!(tiled < single * 2.0 + 2.0, "tiled {tiled:.1} ms vs single {single:.1} ms");
}

/// Many cell rows across a large canvas, plus overlapping UI prims.
fn busy_list(w: i32, h: i32) -> DisplayList {
    let mut list = DisplayList::default();
    list.push(Prim::Rect { rect: Rect::new(0, 0, w, h), color: Color::rgb(20, 20, 30), radius: 0.0 });
    list.push(Prim::PushClip(Rect::new(10, 10, w - 20, h - 20)));
    let cell = Size::new(8, 12);
    for row in 0..(h / cell.h) {
        let cells: Vec<Cell> = (0..(w / cell.w))
            .map(|i| Cell {
                ch: char::from(b'a' + ((i + row) % 26) as u8),
                fg: Color::rgb((i * 7 % 255) as u8, 200, 100),
                bg: if (i + row) % 9 == 0 { Color::rgb(60, 0, 0) } else { Color::TRANSPARENT },
                attrs: CellAttrs { bold: row % 3 == 0, italic: row % 4 == 0, underline: i % 11 == 0, ..Default::default() },
            })
            .collect();
        list.push(Prim::CellRow { origin: Point::new(0, row * cell.h), cell, px: 12.0, cells });
    }
    list.push(Prim::PopClip);
    list.push(Prim::Rect { rect: Rect::new(w / 3, h / 3, w / 3, h / 3), color: Color::rgba(0, 0, 255, 128), radius: 6.0 });
    list.push(Prim::Border { rect: Rect::new(w / 4, h / 5, w / 2, h / 2), color: Color::WHITE, width: 2, radius: 4.0 });
    list
}

/// Banded multi-threaded painting matches a single-pass render, and a
/// small later change is painted on one thread.
#[test]
fn parallel_bands_match_sequential_render() {
    let f: Arc<dyn GlyphRasterizer> = Arc::new(OverhangFont);
    let fonts = Fonts::new(FontStack::new(f.clone()), FontStack::new(f));
    let (w, h) = (1003, 777); // bands that do not divide evenly
    let list = busy_list(w, h);
    let bg = Color::rgb(1, 2, 3);
    let mut full = Canvas::new(w, h);
    render_full(&list, &mut full, &fonts, &mut GlyphCache::default(), bg);
    let mut tiled = Canvas::new(w, h);
    let mut r = TileRenderer::default();
    let damage = r.render(&list, &mut tiled, &fonts, &mut GlyphCache::default(), bg);
    assert!(r.last_was_parallel(), "a full repaint this large uses the bands");
    assert_eq!(damage.iter().map(|d| d.w as i64 * d.h as i64).sum::<i64>(), w as i64 * h as i64);
    let diff = tiled.pixels.iter().zip(&full.pixels).filter(|(a, b)| a != b).count();
    assert_eq!(diff, 0, "{diff} pixels differ between banded and full render");
    // A later small change repaints sequentially and still matches.
    let mut list2 = busy_list(w, h);
    list2.push(Prim::Rect { rect: Rect::new(5, 5, 20, 20), color: Color::WHITE, radius: 0.0 });
    r.render(&list2, &mut tiled, &fonts, &mut GlyphCache::default(), bg);
    assert!(!r.last_was_parallel());
    render_full(&list2, &mut full, &fonts, &mut GlyphCache::default(), bg);
    assert_eq!(tiled.pixels, full.pixels);
}

/// A band canvas takes picture coordinates and clips writes to its rows.
#[test]
fn band_canvas_uses_global_coordinates() {
    let mut c = Canvas::band(10, 4, 20);
    assert_eq!(c.bounds(), Rect::new(0, 20, 10, 4));
    c.fill_rect(Rect::new(2, 18, 3, 4), Color::WHITE); // rows 18..22: only 20, 21 land
    assert_eq!(c.pixel(2, 20) & 0xffffff, 0xffffff);
    assert_eq!(c.pixel(2, 22) & 0xffffff, 0);
    assert_eq!(c.pixels.iter().filter(|&&p| p != 0).count(), 6);
}

/// Same advance as `OverhangFont`, smaller glyphs.
struct SmallFont;
impl GlyphRasterizer for SmallFont {
    /// Every char has a glyph.
    fn has_glyph(&self, _: char) -> bool {
        true
    }
    /// Fixed 8px advance.
    fn advance(&self, _: char, _: f32) -> f32 {
        8.0
    }
    /// A 4x6 block.
    fn rasterize(&self, _: char, _: f32) -> GlyphBitmap {
        GlyphBitmap { width: 4, height: 6, xmin: 1, ymin: 0, advance: 8.0, coverage: vec![200; 24] }
    }
    /// Same metrics as `OverhangFont`.
    fn line_metrics(&self, _: f32) -> LineMetrics {
        LineMetrics { ascent: 12.0, descent: -4.0, line_gap: 0.0 }
    }
}

/// Band glyph caches survive `invalidate` and are dropped by `clear_glyphs`,
/// after which a render with new fonts matches a full render.
#[test]
fn band_glyph_caches_survive_repaints_until_fonts_change() {
    let big: Arc<dyn GlyphRasterizer> = Arc::new(OverhangFont);
    let small: Arc<dyn GlyphRasterizer> = Arc::new(SmallFont);
    let a = Fonts::new(FontStack::new(big.clone()), FontStack::new(big));
    let b = Fonts::new(FontStack::new(small.clone()), FontStack::new(small));
    let (w, h) = (1003, 777);
    let list = busy_list(w, h);
    let bg = Color::rgb(1, 2, 3);
    let mut tiled = Canvas::new(w, h);
    let mut r = TileRenderer::default();
    r.render(&list, &mut tiled, &a, &mut GlyphCache::default(), bg);
    assert!(r.band_glyphs() > 0);
    r.invalidate(); // a resize or OS redraw: glyphs stay valid
    assert!(r.band_glyphs() > 0);
    r.clear_glyphs(); // new fonts
    assert_eq!(r.band_glyphs(), 0);
    r.render(&list, &mut tiled, &b, &mut GlyphCache::default(), bg);
    let mut full = Canvas::new(w, h);
    render_full(&list, &mut full, &b, &mut GlyphCache::default(), bg);
    assert_eq!(tiled.pixels, full.pixels);
}

/// `cover_start` finds the last opaque square rect covering the area,
/// taking clips into account and ignoring translucent or rounded rects.
#[test]
fn opaque_cover_hides_earlier_prims() {
    let mut l = DisplayList::default();
    l.push(Prim::Rect { rect: Rect::new(0, 0, 100, 100), color: Color::rgb(255, 0, 0), radius: 0.0 });
    l.push(Prim::PushClip(Rect::new(0, 0, 50, 100)));
    l.push(Prim::Rect { rect: Rect::new(0, 0, 100, 100), color: Color::rgb(0, 0, 255), radius: 0.0 }); // clipped to the left half
    l.push(Prim::PopClip);
    l.push(Prim::Rect { rect: Rect::new(0, 0, 100, 100), color: Color::rgba(0, 255, 0, 100), radius: 0.0 }); // translucent
    l.push(Prim::Rect { rect: Rect::new(0, 0, 100, 100), color: Color::rgb(9, 9, 9), radius: 8.0 }); // rounded
    assert_eq!(cover_start(&l, Rect::new(0, 0, 40, 40)), Some(2), "left piece: the clipped blue rect covers it");
    assert_eq!(cover_start(&l, Rect::new(40, 0, 40, 40)), Some(0), "straddles the clip: only the red one covers");
    assert_eq!(cover_start(&l, Rect::new(0, 0, 200, 40)), None, "nothing covers beyond the rects");
}
