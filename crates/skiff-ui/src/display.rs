//! Display list and a tiled renderer that only repaints tiles whose
//! contents changed since the previous frame.
//!
//! Widgets push `Prim`s into a `DisplayList` each frame; `TileRenderer`
//! hashes what reaches each tile and repaints only tiles whose hash changed.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::canvas::Canvas;
use crate::color::Color;
use crate::geom::{Point, Rect, Size};
use crate::icons::Icon;
use crate::text::cache::GlyphCache;
use crate::text::layout::draw_text;
use crate::text::{FontKind, Fonts};

/// Text attributes of one terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellAttrs {
    /// Drawn twice, one pixel apart (synthetic bold).
    pub bold: bool,
    /// Glyph rows sheared to the right (synthetic italic).
    pub italic: bool,
    /// A line under the glyph, below the baseline.
    pub underline: bool,
    /// A line through the glyph.
    pub strike: bool,
    /// First half of a double-width char; the next cell is `'\0'` and skipped.
    pub wide: bool,
}

/// One monospace grid cell (colours already resolved, inverse applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    /// The character shown; `' '` (or NUL) draws only the background.
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: CellAttrs,
}

/// Italic slant: horizontal shift per glyph row, in pixels.
const ITALIC_SHEAR: f32 = 0.2;

/// One drawing command, in physical pixels.
/// `Rect` is a fill with corners rounded by `radius`; `Border` is a
/// `width`-pixel outline drawn inside `rect`.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Rect {
        rect: Rect,
        color: Color,
        radius: f32,
    },
    Border {
        rect: Rect,
        color: Color,
        width: i32,
        radius: f32,
    },
    /// `bounds` is the text's line box, computed by whoever pushes it
    /// (the renderer needs it for tiling without re-measuring).
    Text {
        origin: Point,
        text: String,
        font: FontKind,
        px: f32,
        color: Color,
        bounds: Rect,
    },
    /// One row of a monospace grid (a terminal line): per-cell background,
    /// then glyphs at `origin.x + col * cell.w` in the mono font.
    CellRow {
        origin: Point,
        cell: Size,
        px: f32,
        cells: Vec<Cell>,
    },
    /// A vector icon filling the square `rect` (its width is the size).
    Icon {
        rect: Rect,
        icon: Icon,
        color: Color,
    },
    /// A soft drop shadow cast by the opaque rounded rect `rect`, moved
    /// down by `dy` and blurred by `blur` pixels. Only the part outside
    /// `rect` is painted (the caster covers the rest), so draw the caster
    /// right after it.
    Shadow {
        rect: Rect,
        radius: f32,
        dy: i32,
        blur: i32,
        color: Color,
    },
    /// Clips the following prims to this rect (nested clips intersect).
    PushClip(Rect),
    /// Ends the innermost `PushClip`.
    PopClip,
}

impl Prim {
    /// The area this primitive can touch; `None` for clip commands.
    pub fn bounds(&self) -> Option<Rect> {
        match self {
            Prim::Rect { rect, .. } | Prim::Border { rect, .. } | Prim::Icon { rect, .. } => Some(*rect),
            Prim::Shadow { rect, dy, blur, .. } => Some(rect.translate(0, *dy).inset(-(*blur * 3 + 1) / 2)),
            Prim::Text { bounds, .. } => Some(*bounds),
            Prim::CellRow { origin, cell, cells, .. } => {
                // Glyphs (box drawing, italics) may overhang the row a little.
                let w = cell.w * cells.len() as i32;
                let (ox, oy) = (cell.w, cell.h / 4);
                Some(Rect::new(origin.x - ox, origin.y - oy, w + 2 * ox, cell.h + 2 * oy))
            }
            Prim::PushClip(_) | Prim::PopClip => None,
        }
    }

    /// Feeds everything that affects this prim's pixels into the tile hash.
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
            Prim::CellRow { origin, cell, px, cells } => {
                origin.hash(h);
                cell.hash(h);
                px.to_bits().hash(h);
                cells.hash(h);
            }
            Prim::Icon { rect, icon, color } => {
                rect.hash(h);
                icon.hash(h);
                color.hash(h);
            }
            Prim::Shadow { rect, radius, dy, blur, color } => {
                rect.hash(h);
                radius.to_bits().hash(h);
                dy.hash(h);
                blur.hash(h);
                color.hash(h);
            }
            Prim::PushClip(r) => r.hash(h),
            Prim::PopClip => {}
        }
    }
}

/// One frame's drawing commands, in paint order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayList {
    pub prims: Vec<Prim>,
}

impl DisplayList {
    /// Appends a primitive.
    pub fn push(&mut self, p: Prim) {
        self.prims.push(p);
    }

    /// Removes all primitives (start of a frame).
    pub fn clear(&mut self) {
        self.prims.clear();
    }
}

/// Draws one non-clip prim on `canvas` within its current clip.
fn draw_prim(p: &Prim, canvas: &mut Canvas, fonts: &Fonts, cache: &mut GlyphCache) {
    match p {
        Prim::Rect { rect, color, radius } => canvas.fill_rounded_rect(*rect, *radius, *color),
        Prim::Border { rect, color, width, radius } => canvas.stroke_rect(*rect, *width, *radius, *color),
        Prim::Text { origin, text, font, px, color, .. } => {
            draw_text(canvas, fonts, cache, *font, text, *px, *origin, *color);
        }
        Prim::CellRow { origin, cell, px, cells } => draw_cells(canvas, fonts, cache, *origin, *cell, *px, cells),
        Prim::Icon { rect, icon, color } => {
            let size = rect.w.min(rect.h);
            if size > 0 {
                let mask = cache.icon(*icon, size);
                canvas.blend_mask(rect.x, rect.y, size, size, mask, *color);
            }
        }
        Prim::Shadow { rect, radius, dy, blur, color } => canvas.shadow(*rect, *radius, *dy, *blur, *color),
        Prim::PushClip(_) | Prim::PopClip => {}
    }
}

/// Columns each cell starts at, skipping wide-char continuations.
fn cell_spans(cells: &[Cell]) -> impl Iterator<Item = (usize, &Cell, i32)> {
    let mut skip = false;
    cells.iter().enumerate().filter_map(move |(i, c)| {
        if skip {
            skip = false;
            return None;
        }
        skip = c.attrs.wide;
        Some((i, c, if c.attrs.wide { 2 } else { 1 }))
    })
}

/// Draws a `CellRow`: cell backgrounds, then glyphs (bold, italic) and
/// underline / strike lines, skipping cells outside the clip.
fn draw_cells(canvas: &mut Canvas, fonts: &Fonts, cache: &mut GlyphCache, origin: Point, cell: Size, px: f32, cells: &[Cell]) {
    let clip = canvas.clip();
    // A cell (with one cell of slack for overhanging glyphs) is drawn only if it
    // can reach the clip.
    let visible = |i: usize, span: i32| {
        let x = origin.x + i as i32 * cell.w;
        x <= clip.right() + cell.w && x + span * cell.w + cell.w >= clip.x
    };
    // Backgrounds first, so a glyph overhanging into the next cell survives.
    for (i, c, span) in cell_spans(cells) {
        if c.bg.a > 0 && visible(i, span) {
            let r = Rect::new(origin.x + i as i32 * cell.w, origin.y, span * cell.w, cell.h);
            canvas.fill_rect(r, c.bg);
        }
    }
    // Baseline: ascent plus half the line gap below the row top; decoration
    // lines are 1px per 14px of font size.
    let m = fonts.stack(FontKind::Mono).line_metrics(px);
    let baseline = origin.y + (m.ascent + m.line_gap / 2.0).round() as i32;
    let line = ((px / 14.0).round() as i32).max(1);
    for (i, c, span) in cell_spans(cells) {
        if !visible(i, span) {
            continue;
        }
        let x0 = origin.x + i as i32 * cell.w;
        // Spaces and NUL have no glyph to draw.
        if c.ch != ' ' && c.ch != '\0' {
            let g = cache.get(fonts, FontKind::Mono, c.ch, px);
            if g.width > 0 && g.height > 0 {
                let (gw, gh) = (g.width as i32, g.height as i32);
                let x = x0 + g.xmin;
                let y = baseline - g.ymin - gh;
                let passes = if c.attrs.bold { 2 } else { 1 };
                for dx in 0..passes {
                    if c.attrs.italic {
                        canvas.blend_mask_sheared(x + dx, y, gw, gh, &g.coverage, c.fg, ITALIC_SHEAR);
                    } else {
                        canvas.blend_mask(x + dx, y, gw, gh, &g.coverage, c.fg);
                    }
                }
            }
        }
        let w = span * cell.w;
        if c.attrs.underline {
            let y = (baseline + line).min(origin.y + cell.h - line);
            canvas.fill_rect(Rect::new(x0, y, w, line), c.fg);
        }
        if c.attrs.strike {
            let y = baseline - (m.ascent * 0.3).round() as i32;
            canvas.fill_rect(Rect::new(x0, y, w, line), c.fg);
        }
    }
}

/// Index of the last opaque, square-cornered rect that covers all of `r`
/// (after its clips): nothing drawn before it can show inside `r`.
pub(crate) fn cover_start(list: &DisplayList, r: Rect) -> Option<usize> {
    let mut clips: Vec<Rect> = vec![];
    let mut found = None;
    for (i, p) in list.prims.iter().enumerate() {
        // Track the effective clip so a covering rect is judged by what it really paints.
        match p {
            Prim::PushClip(c) => {
                let c = clips.last().map_or(Some(*c), |top| top.intersect(*c)).unwrap_or_default();
                clips.push(c);
            }
            Prim::PopClip => {
                clips.pop();
            }
            Prim::Rect { rect, color, radius } if color.a == 255 && *radius < 0.5 => {
                let eff = match clips.last() {
                    Some(c) => rect.intersect(*c),
                    None => Some(*rect),
                };
                if eff.is_some_and(|e| e.intersect(r) == Some(r)) {
                    found = Some(i);
                }
            }
            _ => {}
        }
    }
    found
}

/// Replays `list` onto `canvas` inside the canvas's current clip.
fn replay(list: &DisplayList, canvas: &mut Canvas, fonts: &Fonts, cache: &mut GlyphCache) {
    replay_from(list, canvas, fonts, cache, 0);
}

/// Like `replay`, drawing only prims from index `start` (earlier clips
/// still apply).
fn replay_from(list: &DisplayList, canvas: &mut Canvas, fonts: &Fonts, cache: &mut GlyphCache, start: usize) {
    let mut depth = 0usize;
    for (i, p) in list.prims.iter().enumerate() {
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
                if i >= start && p.bounds().and_then(|b| b.intersect(canvas.clip())).is_some() {
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

/// Renders a display list onto a canvas, repainting only the tiles whose
/// contents changed since the last frame.
pub struct TileRenderer {
    /// Tile edge length in pixels (64 by default).
    pub tile: i32,
    /// Last frame's per-tile hashes; empty forces a full repaint.
    prev: Vec<u64>,
    /// Canvas size the hashes belong to.
    size: (i32, i32),
    /// Per-thread band canvases and glyph caches, kept between frames.
    bands: Vec<(Canvas, GlyphCache)>,
    /// Whether the last `render` used several threads.
    parallel: bool,
}

impl Default for TileRenderer {
    /// 64px tiles, nothing rendered yet (first frame repaints everything).
    fn default() -> Self {
        Self { tile: 64, prev: vec![], size: (-1, -1), bands: vec![], parallel: false }
    }
}

/// Damage (in pixels) from which painting is split across threads.
const PARALLEL_MIN_AREA: i64 = 1 << 19;
/// At most this many bands (threads) per frame.
const MAX_BANDS: usize = 8;
/// Bands are at least this many rows tall.
const MIN_BAND_ROWS: i32 = 64;

/// Paints `r` from scratch, skipping whatever an opaque rect hides.
fn paint_rect(list: &DisplayList, canvas: &mut Canvas, fonts: &Fonts, cache: &mut GlyphCache, r: Rect, background: Color) {
    canvas.push_clip(r);
    // If an opaque rect covers the whole of `r`, start from it; otherwise
    // clear to the background first.
    let start = cover_start(list, r);
    if start.is_none() {
        canvas.fill_rect(r, background.with_alpha(255));
    }
    replay_from(list, canvas, fonts, cache, start.unwrap_or(0));
    canvas.pop_clip();
}

/// Width of the pieces a band's damage is painted in: narrow enough that
/// a panel's own background covers most pieces (no overdraw beneath it).
const PIECE_W: i32 = 256;

/// Mixes one prim hash into a tile hash; order-sensitive.
#[inline]
fn combine(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x100_0000_01b3).rotate_left(29)
}

impl TileRenderer {
    /// Forces a full repaint next frame.
    pub fn invalidate(&mut self) {
        self.prev.clear();
    }

    /// Drops the band threads' glyph caches (the fonts changed).
    pub fn clear_glyphs(&mut self) {
        for (_, cache) in &mut self.bands {
            cache.clear();
        }
    }

    /// Glyphs cached by the band threads.
    pub fn band_glyphs(&self) -> usize {
        self.bands.iter().map(|(_, c)| c.len()).sum()
    }

    /// The last `render` painted on several threads (tests watch this).
    pub fn last_was_parallel(&self) -> bool {
        self.parallel
    }

    /// Paints `damage` in horizontal bands, one thread per band, each with
    /// its own canvas and glyph cache; the bands are then copied back.
    fn paint_parallel(&mut self, list: &DisplayList, canvas: &mut Canvas, fonts: &Fonts, damage: &[Rect], background: Color) {
        let (w, h) = (canvas.width, canvas.height);
        // Band count: one per thread, but no band shorter than MIN_BAND_ROWS.
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_BANDS);
        let n = threads.min((h / MIN_BAND_ROWS).max(1) as usize).max(1);
        let band_h = (h + n as i32 - 1) / n as i32;
        while self.bands.len() < n {
            self.bands.push((Canvas::new(0, 0), GlyphCache::default()));
        }
        // Each band thread writes only its own slice of the canvas's rows.
        let rows = canvas.pixels.chunks_mut((w * band_h) as usize);
        std::thread::scope(|s| {
            for (i, ((band, cache), dst)) in self.bands.iter_mut().zip(rows).enumerate() {
                let y0 = i as i32 * band_h;
                band.set_band(w, band_h.min(h - y0), y0);
                s.spawn(move || {
                    let bounds = band.bounds();
                    // Paint each damaged rect in PIECE_W-wide pieces, then copy those rows
                    // into the shared canvas.
                    for r in damage.iter().filter_map(|d| d.intersect(bounds)) {
                        let mut x = r.x;
                        while x < r.right() {
                            let piece = Rect::new(x, r.y, PIECE_W.min(r.right() - x), r.h);
                            paint_rect(list, band, fonts, cache, piece, background);
                            x += PIECE_W;
                        }
                        for y in r.y..r.bottom() {
                            let at = ((y - y0) * w + r.x) as usize;
                            dst[at..at + r.w as usize].copy_from_slice(&band.pixels[at..at + r.w as usize]);
                        }
                    }
                });
            }
        });
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

        // A new size (or invalidate) makes every tile dirty.
        let full = self.size != (w, h) || self.prev.len() != n;
        // Merge dirty tiles into rects (row runs, stacked when aligned),
        // then paint each rect in one pass: a line of text crossing many
        // dirty tiles is walked once, not once per tile.
        let mut damage: Vec<Rect> = vec![];
        for ty in 0..rows {
            let mut run: Option<(i32, i32)> = None; // (first col, last col)
            for tx in 0..=cols {
                let dirty = tx < cols && {
                    let i = (ty * cols + tx) as usize;
                    full || self.prev[i] != hashes[i]
                };
                if dirty {
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
        // Large damage on a tall enough canvas, with more than one CPU: paint in bands.
        let area: i64 = damage.iter().map(|r| r.w as i64 * r.h as i64).sum();
        self.parallel =
            area >= PARALLEL_MIN_AREA && h >= 2 * MIN_BAND_ROWS && std::thread::available_parallelism().is_ok_and(|n| n.get() > 1);
        if self.parallel {
            self.paint_parallel(list, canvas, fonts, &damage, background);
        } else {
            for r in &damage {
                paint_rect(list, canvas, fonts, cache, *r, background);
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
        /// Every char has a glyph.
        fn has_glyph(&self, _: char) -> bool {
            true
        }
        /// Fixed 8px advance.
        fn advance(&self, _: char, _: f32) -> f32 {
            8.0
        }
        /// A 6x10 block at coverage 200.
        fn rasterize(&self, _: char, _: f32) -> GlyphBitmap {
            GlyphBitmap { width: 6, height: 10, xmin: 1, ymin: 0, advance: 8.0, coverage: vec![200; 60] }
        }
        /// Ascent 12, descent -4, no gap.
        fn line_metrics(&self, _: f32) -> LineMetrics {
            LineMetrics { ascent: 12.0, descent: -4.0, line_gap: 0.0 }
        }
    }

    /// Two `BlockFont` stacks for UI and mono.
    fn fonts() -> Fonts {
        let f: Arc<dyn GlyphRasterizer> = Arc::new(BlockFont);
        Fonts::new(FontStack::new(f.clone()), FontStack::new(f))
    }

    /// A red square shifted right by `dx` plus a fixed rounded green square.
    fn scene(dx: i32) -> DisplayList {
        let mut l = DisplayList::default();
        l.push(Prim::Rect { rect: Rect::new(10 + dx, 10, 30, 30), color: Color::hex(0xff0000), radius: 0.0 });
        l.push(Prim::Rect { rect: Rect::new(150, 150, 20, 20), color: Color::hex(0x00ff00), radius: 6.0 });
        l
    }

    const BG: Color = Color::rgb(1, 2, 3);

    /// Total pixel area of `rects`.
    fn area(rects: &[Rect]) -> i32 {
        rects.iter().map(|r| r.w * r.h).sum()
    }

    /// The first frame repaints the whole canvas.
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

    /// Rendering the same list twice reports no damage the second time.
    #[test]
    fn identical_frame_damages_nothing() {
        let (f, mut cache) = (fonts(), GlyphCache::default());
        let mut canvas = Canvas::new(200, 200);
        let mut r = TileRenderer::default();
        r.render(&scene(0), &mut canvas, &f, &mut cache, BG);
        assert!(r.render(&scene(0), &mut canvas, &f, &mut cache, BG).is_empty());
    }

    /// Moving one rect a few pixels repaints only the tile it lives in.
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

    /// Wrapping the same prims in a clip changes their tiles' hashes.
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

    /// Incremental tiled rendering ends up pixel-identical to a full render.
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

    /// A zero-size canvas renders nothing; growing again repaints everything.
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

#[cfg(test)]
#[path = "display_cells_tests.rs"]
mod cells_tests;
#[cfg(test)]
#[path = "display_perf_tests.rs"]
mod perf_tests;
