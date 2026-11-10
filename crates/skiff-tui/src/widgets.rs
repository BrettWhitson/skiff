//! Small text helpers the screens share: display width, middle elision for
//! paths, wrapping by whole items, and key hints (`key label`) that drop the
//! least important ones to fit a width.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui_theme::Theme;

/// Display width of `s` in terminal cells (wide characters count double).
pub fn width(s: &str) -> usize {
    Span::raw(s).width()
}

/// `s` cut to `max` cells by replacing its middle with `ell`, keeping a
/// little more of the end (a path's file name); `s` itself when it fits.
pub fn elide_middle(s: &str, max: usize, ell: &str) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let room = max.saturating_sub(width(ell));
    if room == 0 {
        return ell.chars().take(max).collect();
    }
    let head_w = room / 2;
    let mut head = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = width(c.encode_utf8(&mut [0; 4]));
        if used + w > head_w {
            break;
        }
        head.push(c);
        used += w;
    }
    let mut tail: Vec<char> = vec![];
    let mut used_tail = 0;
    for c in s.chars().rev() {
        let w = width(c.encode_utf8(&mut [0; 4]));
        if used + used_tail + w > room {
            break;
        }
        tail.push(c);
        used_tail += w;
    }
    tail.reverse();
    format!("{head}{ell}{}", tail.into_iter().collect::<String>())
}

/// Packs `items` into lines of at most `max` cells, joined by `sep`,
/// breaking only between items (never inside one); an item wider than a
/// whole line is elided in the middle.
pub fn wrap_items(items: &[String], max: usize, sep: &str, ell: &str) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    let mut cur = String::new();
    for item in items {
        let item = elide_middle(item, max, ell);
        if !cur.is_empty() && width(&cur) + width(sep) + width(&item) > max {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str(sep);
        }
        cur.push_str(&item);
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// One `key label` hint, such as `Enter ssh`.
#[derive(Debug, Clone, PartialEq)]
pub struct Hint {
    pub key: String,
    pub label: String,
    /// Lower goes first when space runs out; `0` is never dropped.
    pub rank: u8,
}

/// A hint ranked `rank` (`0`: always shown).
pub fn hint(key: impl Into<String>, label: impl Into<String>, rank: u8) -> Hint {
    Hint { key: key.into(), label: label.into(), rank }
}

/// The space between two hints.
pub const GAP: &str = "  ";

/// Width of `hints` drawn in a row with [`GAP`] between them.
pub fn hints_width<'a>(hints: impl IntoIterator<Item = &'a Hint>) -> usize {
    let (mut w, mut n) = (0usize, 0usize);
    for h in hints {
        w += width(&h.key) + 1 + width(&h.label);
        n += 1;
    }
    w + GAP.len() * n.saturating_sub(1)
}

/// Which of `hints` to show: every rank-0 hint, then the others by rank
/// (ties in order) while `total(&shown)` stays within `budget`.
pub fn fit(hints: &[Hint], budget: usize, total: impl Fn(&[bool]) -> usize) -> Vec<bool> {
    let mut shown: Vec<bool> = hints.iter().map(|h| h.rank == 0).collect();
    let mut order: Vec<usize> = (0..hints.len()).filter(|&i| hints[i].rank > 0).collect();
    order.sort_by_key(|&i| hints[i].rank);
    for i in order {
        shown[i] = true;
        if total(&shown) > budget {
            shown[i] = false;
        }
    }
    shown
}

/// `hints` as spans: the key in the key style, the label muted.
pub fn hint_spans<'a>(t: &Theme, hints: impl IntoIterator<Item = &'a Hint>) -> Vec<Span<'static>> {
    let mut spans = vec![];
    for (i, h) in hints.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(GAP));
        }
        spans.push(Span::styled(h.key.clone(), t.key()));
        spans.push(Span::styled(format!(" {}", h.label), t.muted()));
    }
    spans
}

/// The hints that fit in `max` cells, as one line padded by a space on
/// each side (for a dialog's bottom border).
pub fn fitted_line(t: &Theme, hints: &[Hint], max: usize) -> Line<'static> {
    let shown = fit(hints, max.saturating_sub(2), |s| hints_width(hints.iter().zip(s).filter(|x| *x.1).map(|x| x.0)));
    let mut spans = vec![Span::raw(" ")];
    spans.extend(hint_spans(t, hints.iter().zip(&shown).filter(|x| *x.1).map(|x| x.0)));
    spans.push(Span::raw(" "));
    Line::from(spans).style(Style::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paths lose their middle, keep both ends and fit exactly.
    #[test]
    fn elides_the_middle() {
        assert_eq!(elide_middle("/srv/www", 20, "…"), "/srv/www");
        let e = elide_middle("/srv/www/web-prod/releases/current", 16, "…");
        assert_eq!((e.as_str(), width(&e)), ("/srv/ww…/current", 16));
        assert_eq!(elide_middle("abcdef", 1, "…"), "…");
        let lines = wrap_items(&["alpha".into(), "beta".into(), "gamma".into()], 11, ", ", "…");
        assert_eq!(lines, ["alpha, beta", "gamma"], "breaks between items, not inside one");
    }

    /// Low-ranked hints drop first; rank 0 always stays.
    #[test]
    fn hints_drop_by_rank() {
        let hs = [hint("Enter", "ssh", 1), hint("d", "delete", 5), hint("?", "help", 0)];
        let total = |s: &[bool]| hints_width(hs.iter().zip(s).filter(|x| *x.1).map(|x| x.0));
        assert_eq!(fit(&hs, 100, total), [true, true, true]);
        assert_eq!(fit(&hs, 17, total), [true, false, true]);
        assert_eq!(fit(&hs, 3, total), [false, false, true]);
    }
}
