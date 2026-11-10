//! Mouse selection in a terminal: drag (characters), double-click (word),
//! triple-click (line). Rows are absolute (`visible row - scroll offset`),
//! so a selection stays on its text while scrolling.

use std::time::{Duration, Instant};

use termax_ui::geom::{Point, Rect, Size};
use termax_ui::input::{Event, MouseButton};
use termax_ui::ui::{Id, Ui};

const MULTI_CLICK: Duration = Duration::from_millis(450);

/// (absolute row, column)
pub type Pos = (i64, u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelMode {
    Char,
    Word,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSelection {
    pub start: Pos,
    pub end: Pos,
    pub mode: SelMode,
}

impl TermSelection {
    /// Start and end in reading order.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.start <= self.end { (self.start, self.end) } else { (self.end, self.start) }
    }

    pub fn contains(&self, row: i64, col: u16) -> bool {
        let (a, b) = self.ordered();
        (row, col) >= a && (row, col) <= b
    }
}

#[derive(Debug, Default)]
pub struct SelectState {
    pub sel: Option<TermSelection>,
    /// Raw press and current pointer cells; word/line modes expand these.
    raw: Option<(Pos, Pos)>,
    mode: Option<SelMode>,
    pub dragging: bool,
    moved: bool,
    last_click: Option<(Instant, Pos)>,
    clicks: u8,
}

fn cell_at(p: Point, inner: Rect, cell: Size, offset: usize) -> Pos {
    let col = ((p.x - inner.x).max(0) / cell.w.max(1)) as u16;
    let row = ((p.y - inner.y).max(0) / cell.h.max(1)) as i64;
    let max_col = ((inner.w / cell.w.max(1)).max(1) - 1) as u16;
    let max_row = ((inner.h / cell.h.max(1)).max(1) - 1) as i64;
    (row.min(max_row) - offset as i64, col.min(max_col))
}

/// Updates the selection from this frame's mouse events over `inner`.
/// Returns true when it changed.
pub fn handle_mouse(ui: &mut Ui, _id: Id, inner: Rect, cell: Size, st: &mut SelectState, offset: usize) -> bool {
    let mut changed = false;
    let events = ui.input().events.clone();
    for e in &events {
        match e {
            Event::MouseDown(MouseButton::Left, p) if inner.contains(*p) => {
                let pos = cell_at(*p, inner, cell, offset);
                let now = Instant::now();
                let repeat = st.last_click.is_some_and(|(t, lp)| now.duration_since(t) <= MULTI_CLICK && lp == pos);
                st.clicks = if repeat { (st.clicks + 1).min(3) } else { 1 };
                st.last_click = Some((now, pos));
                st.dragging = true;
                st.moved = false;
                st.raw = Some((pos, pos));
                st.mode = match st.clicks {
                    2 => Some(SelMode::Word),
                    3 => Some(SelMode::Line),
                    _ => Some(SelMode::Char),
                };
                if st.clicks > 1 {
                    // expanded by `resolve` once the screen is at hand
                    st.sel = Some(TermSelection { start: pos, end: pos, mode: st.mode.unwrap() });
                    changed = true;
                }
            }
            Event::MouseMove(p) if st.dragging => {
                let pos = cell_at(*p, inner, cell, offset);
                if let Some((a, b)) = st.raw
                    && (pos != b || st.moved)
                {
                    st.moved |= pos != a;
                    st.raw = Some((a, pos));
                    let mode = st.mode.unwrap_or(SelMode::Char);
                    st.sel = if mode == SelMode::Char && a == pos {
                        None
                    } else {
                        Some(TermSelection { start: a, end: pos, mode })
                    };
                    changed = true;
                }
            }
            Event::MouseUp(MouseButton::Left, _) if st.dragging => {
                st.dragging = false;
                if !st.moved && st.clicks == 1 {
                    changed |= st.sel.take().is_some();
                }
            }
            _ => {}
        }
    }
    changed
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || "_-./~:@".contains(c)
}

fn char_at(screen: &vt100::Screen, row: i64, col: u16, offset: usize) -> char {
    let vis = row + offset as i64;
    if vis < 0 {
        return ' ';
    }
    screen.cell(vis as u16, col).and_then(|c| c.contents().chars().next()).unwrap_or(' ')
}

impl SelectState {
    /// Expands word/line selections against the current screen.
    pub fn resolve(&mut self, screen: &vt100::Screen, offset: usize) {
        let (Some((a, b)), Some(mode)) = (self.raw, self.mode) else { return };
        if mode == SelMode::Char {
            return;
        }
        let cols = screen.size().1;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let (start, end) = match mode {
            SelMode::Line => ((a.0, 0), (b.0, cols.saturating_sub(1))),
            _ => {
                let mut s = a.1;
                if is_word(char_at(screen, a.0, s, offset)) {
                    while s > 0 && is_word(char_at(screen, a.0, s - 1, offset)) {
                        s -= 1;
                    }
                }
                let mut e = b.1;
                if is_word(char_at(screen, b.0, e, offset)) {
                    while e + 1 < cols && is_word(char_at(screen, b.0, e + 1, offset)) {
                        e += 1;
                    }
                }
                ((a.0, s), (b.0, e))
            }
        };
        self.sel = Some(TermSelection { start, end, mode });
    }
}

/// The selected text: rows joined with `\n`, trailing spaces trimmed.
pub fn selection_text(screen: &vt100::Screen, sel: &TermSelection, offset: usize) -> String {
    let (rows, cols) = screen.size();
    let (a, b) = sel.ordered();
    let mut lines = vec![];
    for row in a.0..=b.0 {
        let vis = row + offset as i64;
        if vis < 0 || vis >= rows as i64 {
            continue;
        }
        let c0 = if row == a.0 { a.1 } else { 0 };
        let c1 = if row == b.0 { b.1 } else { cols.saturating_sub(1) };
        let mut line = String::new();
        for col in c0..=c1.min(cols.saturating_sub(1)) {
            match screen.cell(vis as u16, col) {
                Some(c) if c.is_wide_continuation() => {}
                Some(c) if c.has_contents() => line.push_str(c.contents()),
                _ => line.push(' '),
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}
