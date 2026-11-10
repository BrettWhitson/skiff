//! Text tests: measuring, drawing, glyph caching, tofu, monospace cells,
//! caret hit-testing, lazy fallbacks and font scanning. Tests that need
//! installed fonts skip themselves when there are none.

use super::cache::GlyphCache;
use super::layout::{char_index_at_x, draw_text, line_height, measure};
use super::{BlockFont, FontKind, FontStack, Fonts, GlyphBitmap, GlyphRasterizer, LazyFont, LineMetrics, TOFU};
use crate::canvas::Canvas;
use crate::color::Color;
use crate::geom::Point;
use std::sync::Arc;

/// The system fonts, or `None` (test skipped) when none are installed.
fn fonts() -> Option<Fonts> {
    match Fonts::load(None, None) {
        Ok(f) => Some(f),
        Err(e) => {
            eprintln!("skipping: no system font found ({e})");
            None
        }
    }
}

/// Longer text measures wider; empty text measures zero.
#[test]
fn measure_grows_with_text() {
    let Some(f) = fonts() else { return };
    let mut c = GlyphCache::default();
    let a = measure(&f, &mut c, FontKind::Ui, "ab", 14.0);
    let b = measure(&f, &mut c, FontKind::Ui, "abcdef", 14.0);
    assert!(a > 0.0 && b > a, "{a} {b}");
    assert_eq!(measure(&f, &mut c, FontKind::Ui, "", 14.0), 0.0);
}

/// Drawn text only lights pixels inside its advance and line box.
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

/// Drawing repeated chars rasterizes each distinct glyph once.
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

/// A char no font has draws a visible box, the same width for every such char.
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

/// Monospace glyphs share one advance, which is the cell width.
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

/// Measuring a prefix and hit-testing at that x returns the prefix length;
/// out-of-range x clamps to the ends.
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

/// A lazy fallback loads only when a char needs it, only once, and a
/// fallback that fails to load leaves the char as tofu.
#[test]
fn fallback_fonts_load_on_first_use() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let loads = Arc::new(AtomicUsize::new(0));
    let n = loads.clone();
    let lazy = LazyFont::new(Box::new(move || {
        n.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(BlockFont) as Arc<dyn GlyphRasterizer>)
    }));
    let mut stack = FontStack::new(Arc::new(NoMissingGlyphs));
    stack.fallbacks.push(Arc::new(lazy));
    assert_eq!(stack.resolve('a'), 0);
    assert_eq!(loads.load(Ordering::SeqCst), 0, "primary had it: nothing loaded");
    assert_eq!(stack.resolve('\u{2603}'), 1);
    assert_eq!(stack.resolve('\u{2604}'), 1);
    assert_eq!(loads.load(Ordering::SeqCst), 1, "loaded once");
    assert!(stack.rasterize(1, '\u{2603}', 14.0).width > 0);

    let broken = LazyFont::new(Box::new(|| anyhow::bail!("no such file")));
    let mut stack = FontStack::new(Arc::new(NoMissingGlyphs));
    stack.fallbacks.push(Arc::new(broken));
    assert_eq!(stack.resolve('\u{2603}'), TOFU);
}

/// Has ASCII only.
struct NoMissingGlyphs;
impl GlyphRasterizer for NoMissingGlyphs {
    /// ASCII only.
    fn has_glyph(&self, c: char) -> bool {
        c.is_ascii()
    }
    /// Same as `BlockFont`.
    fn advance(&self, c: char, px: f32) -> f32 {
        BlockFont.advance(c, px)
    }
    /// Same as `BlockFont`.
    fn rasterize(&self, c: char, px: f32) -> GlyphBitmap {
        BlockFont.rasterize(c, px)
    }
    /// Same as `BlockFont`.
    fn line_metrics(&self, px: f32) -> LineMetrics {
        BlockFont.line_metrics(px)
    }
}

/// Font scanning recurses into subfolders, skips junk files, sorts by name
/// and can filter to monospace fonts.
#[test]
fn scan_fonts_lists_monospace() {
    use super::system::{candidates, scan_fonts};
    let first = |k| candidates(k).into_iter().find(|p| p.exists() && p.extension().is_some_and(|e| e == "ttf"));
    let (Some(mono), Some(ui)) = (first(FontKind::Mono), first(FontKind::Ui)) else {
        eprintln!("no system .ttf fonts; skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("skiff-fonts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Fonts in subfolders count (Linux keeps them in per-family folders).
    std::fs::create_dir_all(dir.join("family")).unwrap();
    std::fs::copy(&mono, dir.join("family").join("mono.ttf")).unwrap();
    std::fs::copy(&ui, dir.join("ui.ttf")).unwrap();
    std::fs::write(dir.join("junk.ttf"), b"not a font at all, just bytes").unwrap();
    let dirs = [dir.clone()];
    let only_mono = scan_fonts(&dirs, true);
    let all = scan_fonts(&dirs, false);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(only_mono.len(), 1, "{only_mono:?}");
    assert!(only_mono[0].1.ends_with("mono.ttf"));
    assert!(!only_mono[0].0.is_empty() && !only_mono[0].0.ends_with(".ttf"), "a font name, not a file name: {only_mono:?}");
    assert_eq!(all.len(), 2, "junk skipped: {all:?}");
    assert!(all.windows(2).all(|w| w[0].0 <= w[1].0), "sorted by name");
}
