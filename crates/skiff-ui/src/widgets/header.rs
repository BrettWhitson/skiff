//! Section headers (small-caps captions with an optional count) and table
//! headers with sortable columns.

use crate::a11y::{Role, SortDir};
use crate::geom::{Point, Rect};
use crate::icons::Icon;
use crate::input::CursorIcon;
use crate::layout;
use crate::style::{TextRole, tokens};
use crate::text::FontKind;
use crate::ui::{Id, Sense, Ui};

use super::button::keyboard_activate;
use super::label::{Align, Elide, elide_mode};

/// A section title in `rect`: an upper-case semibold caption in muted text,
/// followed by `count` when given. Returns the space right of the title
/// (for header actions such as an "Add" icon button).
pub fn section_header(ui: &mut Ui, rect: Rect, title: &str, count: Option<usize>) -> Rect {
    let p = ui.style().palette;
    let px = ui.style().text_px(TextRole::Caption) * ui.scale();
    let upper = title.to_uppercase();
    let lh = ui.line_height(FontKind::UiSemibold, px);
    let y = rect.y + (rect.h - lh) / 2;
    let shown = elide_mode(ui, &upper, FontKind::UiSemibold, px, rect.w, Elide::Middle);
    let mut x = rect.x + ui.painter().text(Point::new(rect.x, y), &shown, FontKind::UiSemibold, px, p.muted);
    if let Some(n) = count {
        x += ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
        x += ui.painter().text(Point::new(x, y), &n.to_string(), FontKind::Ui, px, p.muted);
    }
    if ui.a11y_on() {
        let name = match count {
            Some(n) => format!("{title}, {n}"),
            None => title.to_owned(),
        };
        if let Some(node) = ui.a11y_node(Id::new(("section", title)), Role::Heading, &name) {
            node.bounds = Rect::new(rect.x, rect.y, x - rect.x, rect.h);
        }
    }
    let gap = ui.px(tokens::SPACE_SM);
    Rect::new(x + gap, rect.y, (rect.right() - x - gap).max(0), rect.h)
}

/// One column of a `table_header`.
#[derive(Debug, Clone, Copy)]
pub struct Column<'a> {
    pub title: &'a str,
    /// Width, as for `Ui::split_h` (logical px, fraction or fill).
    pub width: layout::Size,
    /// Clicking the title sorts by this column.
    pub sortable: bool,
    /// Where the title (and the column's cells) sit.
    pub align: Align,
}

impl<'a> Column<'a> {
    /// A sortable, start-aligned column.
    pub const fn new(title: &'a str, width: layout::Size) -> Self {
        Column { title, width, sortable: true, align: Align::Start }
    }
}

/// The column a table is sorted by and in which direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub column: usize,
    pub dir: SortDir,
}

impl Sort {
    /// The sort after activating `column`'s header: the same column flips
    /// direction, another column sorts ascending.
    pub fn toggle(cur: Option<Sort>, column: usize) -> Sort {
        match cur {
            Some(s) if s.column == column => {
                let dir = if s.dir == SortDir::Ascending { SortDir::Descending } else { SortDir::Ascending };
                Sort { column, dir }
            }
            _ => Sort { column, dir: SortDir::Ascending },
        }
    }
}

/// What a table header reported this frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableHeaderResponse {
    /// Each column's rect (use them to lay out the rows' cells).
    pub columns: Vec<Rect>,
    /// `sort` changed this frame.
    pub changed: bool,
}

/// A table's header row in `rect`: caption-size titles with a bottom
/// border; sortable titles are focusable buttons (click, Space or Enter
/// toggle via `Sort::toggle`) showing a chevron on the sorted column.
pub fn table_header(ui: &mut Ui, id: Id, rect: Rect, columns: &[Column], sort: &mut Option<Sort>) -> TableHeaderResponse {
    let sizes: Vec<layout::Size> = columns.iter().map(|c| c.width).collect();
    let rects = ui.split_h(rect, &sizes, 0);
    let p = ui.style().palette;
    let px = ui.style().text_px(TextRole::Caption) * ui.scale();
    let lh = ui.line_height(FontKind::UiSemibold, px);
    let pad = ui.px(tokens::SPACE_SM);
    let icon = ui.px(12).max(8);
    let bw = ui.px(1).max(1);
    let mut out = TableHeaderResponse { columns: rects.clone(), changed: false };
    ui.painter().rect(Rect::new(rect.x, rect.bottom() - bw, rect.w, bw), p.border, 0.0);
    for (i, (c, r)) in columns.iter().zip(&rects).enumerate() {
        let cid = id.with(("col", i));
        let sorted = sort.filter(|s| s.column == i).map(|s| s.dir);
        let mut hovered = false;
        if c.sortable {
            let mut resp = ui.interact(cid, *r, Sense::focusable());
            keyboard_activate(ui, cid, &mut resp);
            if resp.clicked {
                *sort = Some(Sort::toggle(*sort, i));
                out.changed = true;
            }
            hovered = resp.hovered;
            if hovered {
                ui.set_cursor_icon(CursorIcon::Pointer);
                let radius = tokens::RADIUS_SM * ui.scale();
                ui.painter().rect(*r, p.hover, radius);
            }
        }
        let role = if c.sortable { Role::ColumnHeader } else { Role::Label };
        if let Some(n) = ui.a11y_node(cid, role, c.title) {
            n.bounds = *r;
            n.sort = sort.filter(|s| s.column == i).map(|s| s.dir);
        }
        let color = if sorted.is_some() || hovered { p.fg } else { p.muted };
        let room = r.w - 2 * pad - if sorted.is_some() { icon + bw * 4 } else { 0 };
        let shown = elide_mode(ui, c.title, FontKind::UiSemibold, px, room, Elide::End);
        let tw = ui.measure(&shown, FontKind::UiSemibold, px).ceil() as i32;
        let group = tw + if sorted.is_some() { icon + bw * 4 } else { 0 };
        let x = match c.align {
            Align::Start => r.x + pad,
            Align::Center => r.x + (r.w - group) / 2,
            Align::End => r.right() - pad - group,
        };
        let mut painter = ui.painter();
        painter.text(Point::new(x, r.y + (r.h - lh) / 2), &shown, FontKind::UiSemibold, px, color);
        if let Some(dir) = sorted {
            let glyph = if dir == SortDir::Ascending { Icon::ChevronUp } else { Icon::ChevronDown };
            painter.icon(Rect::new(x + tw + bw * 4, r.y + (r.h - icon) / 2, icon, icon), glyph, p.fg);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Activating the sorted column flips its direction; another column
    /// starts ascending.
    #[test]
    fn sort_toggles() {
        let asc = |c| Sort { column: c, dir: SortDir::Ascending };
        let s = Sort::toggle(None, 1);
        assert_eq!(s, asc(1));
        let s = Sort::toggle(Some(s), 1);
        assert_eq!(s.dir, SortDir::Descending);
        assert_eq!(Sort::toggle(Some(s), 1), asc(1));
        assert_eq!(Sort::toggle(Some(s), 2), asc(2));
    }
}
