//! Mouse selection in a terminal: drag (characters), double-click (word),
//! triple-click (line). Rows are absolute (`visible row - scroll offset`),
//! so a selection stays on its text while scrolling.

use std::time::{Duration, Instant};

use skiff_ui::geom::{Point, Rect, Size};
use skiff_ui::input::{Event, MouseButton};
use skiff_ui::ui::{Id, Ui};

/// Presses on the same cell within this long count as a double / triple click.
const MULTI_CLICK: Duration = Duration::from_millis(450);

/// (absolute row, column)
pub type Pos = (i64, u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// What a selection snaps to: single cells (drag), whole words (double
/// click) or whole lines (triple click).
pub enum SelMode {
    Char,
    Word,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A selection between two absolute cells (either order), with its mode.
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

    /// Whether the absolute cell (`row`, `col`) is selected (reading-order range).
    pub fn contains(&self, row: i64, col: u16) -> bool {
        let (a, b) = self.ordered();
        (row, col) >= a && (row, col) <= b
    }
}

#[derive(Debug, Default)]
/// Per-terminal selection state across frames.
pub struct SelectState {
    /// The current selection, if any.
    pub sel: Option<TermSelection>,
    /// Raw press and current pointer cells; word/line modes expand these.
    raw: Option<(Pos, Pos)>,
    /// Mode chosen by the click count of the last press.
    mode: Option<SelMode>,
    /// The left button is down (a selection is being made).
    pub dragging: bool,
    /// The pointer left the press cell during this drag.
    moved: bool,
    /// Time and cell of the last press (for multi-click detection).
    last_click: Option<(Instant, Pos)>,
    /// Clicks in the current run: 1, 2 or 3.
    clicks: u8,
}

/// The absolute cell under `p`, clamped to the grid (`offset`: rows scrolled back).
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
            // Press: count clicks (same cell, quick) to pick char / word / line mode.
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
            // Drag: extend from the press cell; a char selection back on its start cell is none.
            Event::MouseMove(p) if st.dragging => {
                let pos = cell_at(*p, inner, cell, offset);
                if let Some((a, b)) = st.raw
                    && (pos != b || st.moved)
                {
                    st.moved |= pos != a;
                    st.raw = Some((a, pos));
                    let mode = st.mode.unwrap_or(SelMode::Char);
                    st.sel = if mode == SelMode::Char && a == pos { None } else { Some(TermSelection { start: a, end: pos, mode }) };
                    changed = true;
                }
            }
            // A single click without movement clears the selection.
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

/// Characters a double-click word includes (paths, hosts and emails stay whole).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || "_-./~:@".contains(c)
}

/// The character at an absolute cell; a space if it is off screen or empty.
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
        // Line: whole rows; word: grown to word boundaries on the visible screen.
        let (start, end) = match mode {
            SelMode::Line => ((a.0, 0), (b.0, cols.saturating_sub(1))),
            // Word: widen the start left and the end right over word characters.
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
        // First and last rows are partial; middle rows are whole.
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
