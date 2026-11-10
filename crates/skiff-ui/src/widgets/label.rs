use crate::color::Color;
use crate::geom::{Point, Rect};
use crate::text::FontKind;
use crate::ui::Ui;

/// `text` shortened with "…" so it fits in `max_w` pixels.
pub fn elide(ui: &mut Ui, text: &str, font: FontKind, px: f32, max_w: i32) -> String {
    if ui.measure(text, font, px) <= max_w as f32 {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    // Binary search for the longest prefix that fits with the ellipsis.
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().chain(['…'].iter()).collect();
        if ui.measure(&s, font, px) <= max_w as f32 {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 && ui.measure("…", font, px) > max_w as f32 {
        return String::new();
    }
    chars[..lo].iter().chain(['…'].iter()).collect()
}

/// Draws text with `font`, vertically centred in `rect`, elided to fit.
pub fn label_with(ui: &mut Ui, rect: Rect, text: &str, font: FontKind, color: Color) -> i32 {
    let px = if font == FontKind::Mono { ui.mono_px() } else { ui.font_px() };
    let shown = elide(ui, text, font, px, rect.w);
    let lh = ui.line_height(font, px);
    let pos = Point::new(rect.x, rect.y + (rect.h - lh) / 2);
    ui.painter().text(pos, &shown, font, px, color)
}

/// A single line of UI text, vertically centred in `rect` and elided
/// with "…" when too wide. `color` defaults to the palette foreground.
pub fn label(ui: &mut Ui, rect: Rect, text: &str, color: Option<Color>) -> i32 {
    let c = color.unwrap_or(ui.style().palette.fg);
    label_with(ui, rect, text, FontKind::Ui, c)
}
