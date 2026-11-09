//! Single-line text measuring, drawing and hit-testing.

use super::cache::GlyphCache;
use super::{FontKind, Fonts};
use crate::canvas::Canvas;
use crate::color::Color;
use crate::geom::Point;

/// Line-box height in whole pixels.
pub fn line_height(fonts: &Fonts, kind: FontKind, px: f32) -> f32 {
    fonts.stack(kind).line_metrics(px).height().ceil()
}

pub fn measure(fonts: &Fonts, cache: &mut GlyphCache, kind: FontKind, text: &str, px: f32) -> f32 {
    text.chars().map(|c| cache.get(fonts, kind, c, px).advance).sum()
}

/// Draws `text` with its line box's top-left at `origin`; returns the advance.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    canvas: &mut Canvas,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    kind: FontKind,
    text: &str,
    px: f32,
    origin: Point,
    color: Color,
) -> f32 {
    let m = fonts.stack(kind).line_metrics(px);
    let baseline = origin.y + (m.ascent + m.line_gap / 2.0).round() as i32;
    let mut pen = 0.0f32;
    for c in text.chars() {
        let g = cache.get(fonts, kind, c, px);
        if g.width > 0 && g.height > 0 {
            let x = origin.x + pen.round() as i32 + g.xmin;
            let y = baseline - g.ymin - g.height as i32;
            canvas.blend_mask(x, y, g.width as i32, g.height as i32, &g.coverage, color);
        }
        pen += g.advance;
    }
    pen
}

/// The caret index (in chars) nearest to `x`, measured from the text start.
pub fn char_index_at_x(fonts: &Fonts, cache: &mut GlyphCache, kind: FontKind, text: &str, px: f32, x: f32) -> usize {
    let mut pen = 0.0f32;
    for (i, c) in text.chars().enumerate() {
        let adv = cache.get(fonts, kind, c, px).advance;
        if x < pen + adv / 2.0 {
            return i;
        }
        pen += adv;
    }
    text.chars().count()
}
