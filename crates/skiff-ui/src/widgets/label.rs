//! Text labels: single lines elided to fit (at the end, middle or start),
//! wrapped paragraphs, and the type-scale aware `text` helper. Every
//! widget that draws text into a fixed width elides through here.

use crate::color::Color;
use crate::geom::{Point, Rect};
use crate::style::{TextRole, tokens};
use crate::text::FontKind;
use crate::ui::Ui;

/// Where text that does not fit is shortened with "…".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Elide {
    /// `long name…` (titles, labels).
    #[default]
    End,
    /// `~/projects/…/main.rs`: keeps the start and the last path segment.
    Middle,
    /// `…/src/main.rs`: keeps the end (deep paths in narrow places).
    Start,
}

/// Horizontal placement of a line in its rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// The characters kept from the end in `Middle` mode when `k` characters
/// fit: half, or more to keep the last path segment (from its separator)
/// whole when it fits.
fn tail_len(chars: &[char], k: usize) -> usize {
    let seg = chars.iter().rposition(|&c| c == '/' || c == '\\').map_or(0, |i| chars.len() - i);
    k.div_ceil(2).max(seg.min(k))
}

/// `text` shortened with "…" (at the end, middle or start per `mode`) so
/// that `measure` of the result is at most `max_w`. Empty when not even
/// the ellipsis fits. Pure, so it can be tested without fonts.
pub fn elide_by(text: &str, max_w: f32, mode: Elide, mut measure: impl FnMut(&str) -> f32) -> String {
    if measure(text) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    // The string keeping `k` of the original chars.
    let build = |k: usize| -> String {
        match mode {
            Elide::End => chars[..k].iter().chain(['…'].iter()).collect(),
            Elide::Start => ['…'].iter().chain(chars[n - k..].iter()).collect(),
            Elide::Middle => {
                let tail = tail_len(&chars, k);
                chars[..k - tail].iter().chain(['…'].iter()).chain(chars[n - tail..].iter()).collect()
            }
        }
    };
    // Binary search for the most chars that fit with the ellipsis.
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&build(mid)) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 && measure("…") > max_w {
        return String::new();
    }
    build(lo)
}

/// `text` shortened at the end with "…" so it fits in `max_w` pixels.
pub fn elide(ui: &mut Ui, text: &str, font: FontKind, px: f32, max_w: i32) -> String {
    elide_mode(ui, text, font, px, max_w, Elide::End)
}

/// `text` shortened per `mode` so it fits in `max_w` pixels.
pub fn elide_mode(ui: &mut Ui, text: &str, font: FontKind, px: f32, max_w: i32, mode: Elide) -> String {
    elide_by(text, max_w as f32, mode, |s| ui.measure(s, font, px))
}

/// Word-wraps `text` into lines no wider than `max_w` (by `measure`);
/// newlines start new lines and words too long for a line are split.
pub fn wrap_by(text: &str, max_w: f32, mut measure: impl FnMut(&str) -> f32) -> Vec<String> {
    let mut lines = vec![];
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            let joined = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if measure(&joined) <= max_w {
                line = joined;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word wider than the line: break it by characters.
            for c in word.chars() {
                line.push(c);
                if measure(&line) > max_w && line.chars().count() > 1 {
                    line.pop();
                    lines.push(std::mem::replace(&mut line, c.to_string()));
                }
            }
        }
        lines.push(line);
    }
    lines
}

/// Options for `text`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextOpts {
    /// Type-scale role (size and weight); `Body` by default.
    pub role: Option<TextRole>,
    /// Colour; the palette foreground by default.
    pub color: Option<Color>,
    /// Where to shorten text that does not fit.
    pub elide: Elide,
    /// Horizontal placement.
    pub align: Align,
}

/// One line of text in a type-scale role, vertically centred in `rect`,
/// placed per `opts.align` and elided per `opts.elide`. Returns the drawn
/// width in pixels.
pub fn text(ui: &mut Ui, rect: Rect, text: &str, opts: TextOpts) -> i32 {
    let (font, px) = ui.text_style(opts.role.unwrap_or(TextRole::Body));
    let color = opts.color.unwrap_or(ui.style().palette.fg);
    let shown = elide_mode(ui, text, font, px, rect.w, opts.elide);
    let w = ui.measure(&shown, font, px).ceil() as i32;
    let lh = ui.line_height(font, px);
    let x = match opts.align {
        Align::Start => rect.x,
        Align::Center => rect.x + (rect.w - w) / 2,
        Align::End => rect.right() - w,
    };
    ui.painter().text(Point::new(x, rect.y + (rect.h - lh) / 2), &shown, font, px, color)
}

/// Height of one wrapped line of `role` (its size times `tokens::LINE_HEIGHT`).
pub fn paragraph_line_h(ui: &Ui, role: TextRole) -> i32 {
    let (font, px) = ui.text_style(role);
    ((px * tokens::LINE_HEIGHT).ceil() as i32).max(ui.line_height(font, px))
}

/// The lines `text` wraps into at width `w` in `role`.
pub fn paragraph_lines(ui: &mut Ui, text: &str, role: TextRole, w: i32) -> Vec<String> {
    let (font, px) = ui.text_style(role);
    wrap_by(text, w as f32, |s| ui.measure(s, font, px))
}

/// Height `text` takes wrapped at width `w` in `role`.
pub fn paragraph_height(ui: &mut Ui, text: &str, role: TextRole, w: i32) -> i32 {
    paragraph_lines(ui, text, role, w).len() as i32 * paragraph_line_h(ui, role)
}

/// Draws `text` word-wrapped to `rect.w` from the top of `rect` (lines
/// below `rect` are not drawn), each line placed per `align`. Returns the
/// height used.
pub fn paragraph(ui: &mut Ui, rect: Rect, text: &str, role: TextRole, color: Color, align: Align) -> i32 {
    let lines = paragraph_lines(ui, text, role, rect.w);
    let lh = paragraph_line_h(ui, role);
    let (font, px) = ui.text_style(role);
    let glyph_h = ui.line_height(font, px);
    let mut y = rect.y;
    for line in &lines {
        if y + lh > rect.bottom() + lh / 2 {
            break;
        }
        let w = ui.measure(line, font, px).ceil() as i32;
        let x = match align {
            Align::Start => rect.x,
            Align::Center => rect.x + (rect.w - w) / 2,
            Align::End => rect.right() - w,
        };
        ui.painter().text(Point::new(x, y + (lh - glyph_h) / 2), line, font, px, color);
        y += lh;
    }
    y - rect.y
}

/// Draws text with `font`, vertically centred in `rect`, elided to fit.
/// Mono text uses the terminal font size. Returns the drawn width.
pub fn label_with(ui: &mut Ui, rect: Rect, text: &str, font: FontKind, color: Color) -> i32 {
    let px = if font == FontKind::Mono { ui.mono_px() } else { ui.font_px() };
    let shown = elide(ui, text, font, px, rect.w);
    let lh = ui.line_height(font, px);
    let pos = Point::new(rect.x, rect.y + (rect.h - lh) / 2);
    ui.painter().text(pos, &shown, font, px, color)
}

/// A single line of UI text, vertically centred in `rect` and elided
/// with "…" when too wide. `color` defaults to the palette foreground.
/// Returns the drawn width in pixels.
pub fn label(ui: &mut Ui, rect: Rect, text: &str, color: Option<Color>) -> i32 {
    let c = color.unwrap_or(ui.style().palette.fg);
    label_with(ui, rect, text, FontKind::Ui, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Width of `s` at one unit per char.
    fn w(s: &str) -> f32 {
        s.chars().count() as f32
    }

    /// End, start and middle elision fit the width; middle keeps the last
    /// path segment whole when it can, and splits evenly when it can't.
    #[test]
    fn elision_modes() {
        let p = "/home/user/projects/skiff/src/main.rs";
        assert_eq!(elide_by(p, 100.0, Elide::Middle, w), p);
        assert_eq!(elide_by(p, 12.0, Elide::End, w), "/home/user/…");
        assert_eq!(elide_by(p, 12.0, Elide::Start, w), "…src/main.rs");
        assert_eq!(elide_by(p, 16.0, Elide::Middle, w), "/home/u…/main.rs");
        assert_eq!(elide_by("abcdefghij", 5.0, Elide::Middle, w), "ab…ij");
        // The segment alone is wider than the room: the end wins.
        assert_eq!(elide_by("/a/verylongfilename.txt", 8.0, Elide::Middle, w), "…ame.txt");
        for mode in [Elide::End, Elide::Middle, Elide::Start] {
            assert_eq!(elide_by(p, 0.5, mode, w), "");
            assert_eq!(elide_by(p, 1.0, mode, w), "…");
        }
    }

    /// Wrapping breaks between words, keeps newlines, and splits words
    /// longer than a line.
    #[test]
    fn wrapping() {
        assert_eq!(wrap_by("the quick brown fox", 9.0, w), ["the quick", "brown fox"]);
        assert_eq!(wrap_by("a\nb c", 9.0, w), ["a", "b c"]);
        assert_eq!(wrap_by("abcdefgh xy", 3.0, w), ["abc", "def", "gh", "xy"]);
        assert_eq!(wrap_by("", 5.0, w), [""]);
    }
}
