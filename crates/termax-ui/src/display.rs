//! Display list and a tiled renderer that only repaints tiles whose
//! contents changed since the previous frame.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::canvas::Canvas;
use crate::color::Color;
use crate::geom::{Point, Rect};
use crate::text::cache::GlyphCache;
use crate::text::layout::draw_text;
use crate::text::{FontKind, Fonts};

/// One drawing command, in physical pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Rect { rect: Rect, color: Color, radius: f32 },
    Border { rect: Rect, color: Color, width: i32, radius: f32 },
    /// `bounds` is the text's line box, computed by whoever pushes it
    /// (the renderer needs it for tiling without re-measuring).
    Text { origin: Point, text: String, font: FontKind, px: f32, color: Color, bounds: Rect },
    PushClip(Rect),
    PopClip,
}

impl Prim {
    /// The area this primitive can touch; `None` for clip commands.
    pub fn bounds(&self) -> Option<Rect> {
        match self {
            Prim::Rect { rect, .. } | Prim::Border { rect, .. } => Some(*rect),
            Prim::Text { bounds, .. } => Some(*bounds),
            Prim::PushClip(_) | Prim::PopClip => None,
        }
    }

    fn hash_into(&self, h: &mut impl Hasher) {
        std::mem::discriminant(self).hash(h);
        match self {
            Prim::Rect { rect, color, radius } => {
                rect.hash(h);
                color.hash(h);
                radius.to_bits().hash(h);
            }
            Prim::Border { rect, color, width, radius } => {
                rect.hash(h);
                color.hash(h);
                width.hash(h);
                radius.to_bits().hash(h);
            }
            Prim::Text { origin, text, font, px, color, bounds } => {
                origin.hash(h);
                text.hash(h);
                font.hash(h);
                px.to_bits().hash(h);
                color.hash(h);
                bounds.hash(h);
            }
            Prim::PushClip(r) => r.hash(h),
            Prim::PopClip => {}
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayList {
    pub prims: Vec<Prim>,
}

impl DisplayList {
    pub fn push(&mut self, p: Prim) {
        self.prims.push(p);
    }

    pub fn clear(&mut self) {
        self.prims.clear();
    }
}

fn draw_prim(p: &Prim, canvas: &mut Canvas, fonts: &Fonts, cache: &mut GlyphCache) {
    match p {
        Prim::Rect { rect, color, radius } => canvas.fill_rounded_rect(*rect, *radius, *color),
        Prim::Border { rect, color, width, radius } => canvas.stroke_rect(*rect, *width, *radius, *color),
        Prim::Text { origin, text, font, px, color, .. } => {
            draw_text(canvas, fonts, cache, *font, text, *px, *origin, *color);
        }
        Prim::PushClip(_) | Prim::PopClip => {}
    }
}

/// Replays `list` onto `canvas` inside the canvas's current clip.
fn replay(list: &DisplayList, canvas: &mut Canvas, fonts: &Fonts, cache: &mut GlyphCache) {
    let mut depth = 0usize;
    for p in &list.prims {
        match p {
            Prim::PushClip(r) => {
                canvas.push_clip(*r);
                depth += 1;
            }
            Prim::PopClip => {
                if depth > 0 {
                    canvas.pop_clip();
                    depth -= 1;
                }
            }
            _ => {
                if p.bounds().and_then(|b| b.intersect(canvas.clip())).is_some() {
                    draw_prim(p, canvas, fonts, cache);
                }
            }
        }
    }
    for _ in 0..depth {
        canvas.pop_clip();
    }
}

/// Paints the whole list without tiling (reference path, and for tests).
pub fn render_full(list: &DisplayList, canvas: &mut Canvas, fonts: &Fonts, cache: &mut GlyphCache, background: Color) {
    canvas.clear(background);
    replay(list, canvas, fonts, cache);
}

pub struct TileRenderer {
    pub tile: i32,
    prev: Vec<u64>,
    size: (i32, i32),
}

impl Default for TileRenderer {
    fn default() -> Self {
        Self { tile: 64, prev: vec![], size: (-1, -1) }
    }
}

#[inline]
fn combine(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x100_0000_01b3).rotate_left(29)
}

impl TileRenderer {
    /// Forces a full repaint next frame.
    pub fn invalidate(&mut self) {
        self.prev.clear();
    }

    /// Renders `list`, repainting only changed tiles. Returns damage rects.
    pub fn render(
        &mut self,
        list: &DisplayList,
        canvas: &mut Canvas,
        fonts: &Fonts,
        cache: &mut GlyphCache,
        background: Color,
    ) -> Vec<Rect> {
        let (w, h, t) = (canvas.width, canvas.height, self.tile.max(1));
        if w <= 0 || h <= 0 {
            self.size = (w, h);
            self.prev.clear();
            return vec![];
        }
        let cols = (w + t - 1) / t;
        let rows = (h + t - 1) / t;
        let n = (cols * rows) as usize;

        // Per-tile content hash: background, then every (clip, prim) pair
        // that reaches the tile, in order.
        let mut seed = DefaultHasher::new();
        background.hash(&mut seed);
        let mut hashes = vec![seed.finish(); n];
        let mut clips = vec![canvas.bounds()];
        for p in &list.prims {
            match p {
                Prim::PushClip(r) => {
                    let c = clips.last().unwrap().intersect(*r).unwrap_or_default();
                    clips.push(c);
                }
                Prim::PopClip => {
                    if clips.len() > 1 {
                        clips.pop();
                    }
                }
                _ => {
                    let clip = *clips.last().unwrap();
                    let Some(eb) = p.bounds().and_then(|b| b.intersect(clip)) else { continue };
                    let mut ph = DefaultHasher::new();
                    clip.hash(&mut ph);
                    p.hash_into(&mut ph);
                    let v = ph.finish();
                    for ty in eb.y / t..=(eb.bottom() - 1) / t {
                        for tx in eb.x / t..=(eb.right() - 1) / t {
                            let i = (ty * cols + tx) as usize;
                            hashes[i] = combine(hashes[i], v);
                        }
                    }
                }
            }
        }

        let full = self.size != (w, h) || self.prev.len() != n;
        let mut damage: Vec<Rect> = vec![];
        for ty in 0..rows {
            let mut run: Option<(i32, i32)> = None; // (first col, last col)
            for tx in 0..=cols {
                let dirty = tx < cols && {
                    let i = (ty * cols + tx) as usize;
                    full || self.prev[i] != hashes[i]
                };
                if dirty {
                    let tr = Rect::new(tx * t, ty * t, t, t).intersect(canvas.bounds()).unwrap();
                    canvas.push_clip(tr);
                    canvas.fill_rect(tr, background.with_alpha(255));
                    replay(list, canvas, fonts, cache);
                    canvas.pop_clip();
                    run = Some((run.map_or(tx, |r| r.0), tx));
                } else if let Some((a, b)) = run.take() {
                    let r = Rect::new(a * t, ty * t, ((b + 1) * t).min(w) - a * t, t.min(h - ty * t));
                    match damage.iter_mut().find(|d| d.x == r.x && d.w == r.w && d.bottom() == r.y) {
                        Some(d) => d.h += r.h,
                        None => damage.push(r),
                    }
                }
            }
        }
        self.prev = hashes;
        self.size = (w, h);
        damage
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::text::{FontStack, GlyphBitmap, GlyphRasterizer, LineMetrics};

    /// A deterministic stand-in font: every glyph is a filled 6×10 block.
    struct BlockFont;
    impl GlyphRasterizer for BlockFont {
        fn has_glyph(&self, _: char) -> bool {
            true
        }
        fn advance(&self, _: char, _: f32) -> f32 {
            8.0
        }
        fn rasterize(&self, _: char, _: f32) -> GlyphBitmap {
            GlyphBitmap { width: 6, height: 10, xmin: 1, ymin: 0, advance: 8.0, coverage: vec![200; 60] }
        }
        fn line_metrics(&self, _: f32) -> LineMetrics {
            LineMetrics { ascent: 12.0, descent: -4.0, line_gap: 0.0 }
        }
    }

    fn fonts() -> Fonts {
        let f: Arc<dyn GlyphRasterizer> = Arc::new(BlockFont);
        Fonts::new(FontStack::new(f.clone()), FontStack::new(f))
    }

    fn scene(dx: i32) -> DisplayList {
        let mut l = DisplayList::default();
        l.push(Prim::Rect { rect: Rect::new(10 + dx, 10, 30, 30), color: Color::hex(0xff0000), radius: 0.0 });
        l.push(Prim::Rect { rect: Rect::new(150, 150, 20, 20), color: Color::hex(0x00ff00), radius: 6.0 });
        l
    }

    const BG: Color = Color::rgb(1, 2, 3);

    fn area(rects: &[Rect]) -> i32 {
        rects.iter().map(|r| r.w * r.h).sum()
    }

    #[test]
    fn first_frame_damages_everything() {
        let (f, mut cache) = (fonts(), GlyphCache::default());
        let mut canvas = Canvas::new(200, 200);
        let mut r = TileRenderer::default();
        let dmg = r.render(&scene(0), &mut canvas, &f, &mut cache, BG);
        assert_eq!(area(&dmg), 200 * 200);
        assert_eq!(canvas.pixel(0, 0), BG.to_u32());
        assert_eq!(canvas.pixel(20, 20), 0xff0000);
    }

    #[test]
    fn identical_frame_damages_nothing() {
        let (f, mut cache) = (fonts(), GlyphCache::default());
        let mut canvas = Canvas::new(200, 200);
        let mut r = TileRenderer::default();
        r.render(&scene(0), &mut canvas, &f, &mut cache, BG);
        assert!(r.render(&scene(0), &mut canvas, &f, &mut cache, BG).is_empty());
    }

    #[test]
    fn moving_one_rect_damages_only_its_tiles() {
        let (f, mut cache) = (fonts(), GlyphCache::default());
        let mut canvas = Canvas::new(200, 200);
        let mut r = TileRenderer::default();
        r.render(&scene(0), &mut canvas, &f, &mut cache, BG);
        let dmg = r.render(&scene(5), &mut canvas, &f, &mut cache, BG);
        assert_eq!(dmg, vec![Rect::new(0, 0, 64, 64)]);
        assert_eq!(canvas.pixel(12, 20), BG.to_u32());
        assert_eq!(canvas.pixel(42, 20), 0xff0000);
    }

    #[test]
    fn clip_affects_tile_hash() {
        let (f, mut cache) = (fonts(), GlyphCache::default());
        let mut canvas = Canvas::new(200, 200);
        let mut r = TileRenderer::default();
        r.render(&scene(0), &mut canvas, &f, &mut cache, BG);
        let mut clipped = DisplayList::default();
        clipped.push(Prim::PushClip(Rect::new(0, 0, 20, 200)));
        clipped.prims.extend(scene(0).prims);
        clipped.push(Prim::PopClip);
        let dmg = r.render(&clipped, &mut canvas, &f, &mut cache, BG);
        assert!(dmg.contains(&Rect::new(0, 0, 64, 64)), "{dmg:?}");
        assert_eq!(canvas.pixel(25, 20), BG.to_u32());
        assert_eq!(canvas.pixel(15, 20), 0xff0000);
    }

    #[test]
    fn rendered_tiles_match_full_render() {
        let (f, mut cache) = (fonts(), GlyphCache::default());
        let mut list = DisplayList::default();
        list.push(Prim::Rect { rect: Rect::new(-5, 50, 300, 40), color: Color::rgba(0, 0, 255, 128), radius: 9.0 });
        list.push(Prim::Border { rect: Rect::new(30, 30, 140, 100), color: Color::WHITE, width: 2, radius: 7.0 });
        list.push(Prim::PushClip(Rect::new(40, 0, 100, 200)));
        let text = "clipped text across tiles".to_string();
        list.push(Prim::Text {
            origin: Point::new(20, 60),
            bounds: Rect::new(20, 60, 8 * text.chars().count() as i32, 16),
            text,
            font: FontKind::Ui,
            px: 14.0,
            color: Color::hex(0xffff00),
        });
        list.push(Prim::PopClip);

        let mut tiled = Canvas::new(190, 170);
        let mut r = TileRenderer::default();
        r.render(&scene(0), &mut tiled, &f, &mut cache, BG);
        r.render(&list, &mut tiled, &f, &mut cache, BG);

        let mut full = Canvas::new(190, 170);
        render_full(&list, &mut full, &f, &mut cache, BG);
        assert!(tiled.pixels == full.pixels, "tiled output differs from full render");
    }

    #[test]
    fn resize_and_zero_size() {
        let (f, mut cache) = (fonts(), GlyphCache::default());
        let mut canvas = Canvas::new(100, 100);
        let mut r = TileRenderer::default();
        r.render(&scene(0), &mut canvas, &f, &mut cache, BG);
        canvas.resize(0, 0);
        assert!(r.render(&scene(0), &mut canvas, &f, &mut cache, BG).is_empty());
        canvas.resize(130, 70);
        let dmg = r.render(&scene(0), &mut canvas, &f, &mut cache, BG);
        assert_eq!(area(&dmg), 130 * 70);
    }
}
