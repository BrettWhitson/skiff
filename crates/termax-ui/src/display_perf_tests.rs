use std::sync::Arc;
use std::time::Instant;

use super::*;
use crate::text::{FontStack, GlyphBitmap, GlyphRasterizer, LineMetrics};
use crate::ui::testing::Harness;
use crate::ui::Ui;

/// Glyphs that overhang their advance box on every side, like Segoe 'j'
/// (left) or box-drawing chars (above/below the line box).
struct OverhangFont;
impl GlyphRasterizer for OverhangFont {
    fn has_glyph(&self, _: char) -> bool {
        true
    }
    fn advance(&self, _: char, _: f32) -> f32 {
        8.0
    }
    fn rasterize(&self, _: char, _: f32) -> GlyphBitmap {
        GlyphBitmap { width: 14, height: 22, xmin: -3, ymin: -7, advance: 8.0, coverage: vec![255; 14 * 22] }
    }
    fn line_metrics(&self, _: f32) -> LineMetrics {
        LineMetrics { ascent: 12.0, descent: -4.0, line_gap: 0.0 }
    }
}

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
