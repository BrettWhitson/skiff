//! vt100 screen → toolkit cell rows: colour mapping, selection and
//! search highlights, and the cursor, painted as `CellRow` primitives.

use skiff_ui::color::Color;
use skiff_ui::display::{Cell, CellAttrs, Prim};
use skiff_ui::geom::{Point, Rect, Size};
use skiff_ui::ui::Ui;

use crate::theme::{GuiTheme, xterm_256};

use super::select::TermSelection;

/// A vt100 colour in the theme's palette (16 ANSI from the theme, the rest
/// xterm-256 or RGB); `None` for the default colour.
fn color(c: vt100::Color, theme: &GuiTheme, bold: bool) -> Option<Color> {
    match c {
        vt100::Color::Default => None,
        // Bold brightens the 8 basic colours (xterm's default behaviour).
        vt100::Color::Idx(i) if i < 8 && bold => Some(theme.ansi[i as usize + 8]),
        vt100::Color::Idx(i) if i < 16 => Some(theme.ansi[i as usize]),
        vt100::Color::Idx(i) => Some(xterm_256(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::rgb(r, g, b)),
    }
}

/// One `Cell` per screen position. A default background is transparent
/// (the view paints the terminal background once underneath).
pub fn convert(cell: Option<&vt100::Cell>, theme: &GuiTheme) -> Cell {
    let Some(c) = cell else {
        return Cell { ch: ' ', fg: theme.term_fg, bg: Color::TRANSPARENT, attrs: CellAttrs::default() };
    };
    let ch = if c.is_wide_continuation() { '\0' } else { c.contents().chars().next().unwrap_or(' ') };
    let mut fg = color(c.fgcolor(), theme, c.bold()).unwrap_or(theme.term_fg);
    let mut bg = color(c.bgcolor(), theme, false);
    // Inverse swaps fg and bg (a default bg becomes the terminal background);
    // dim mixes the fg 40% towards the bg.
    if c.inverse() {
        let new_bg = fg;
        fg = bg.unwrap_or(theme.term_bg);
        bg = Some(new_bg);
    }
    if c.dim() {
        fg = fg.mix(bg.unwrap_or(theme.term_bg), 0.4);
    }
    Cell {
        ch,
        fg,
        bg: bg.unwrap_or(Color::TRANSPARENT),
        attrs: CellAttrs { bold: c.bold(), italic: c.italic(), underline: c.underline(), strike: false, wide: c.is_wide() },
    }
}

/// The visible screen as rows of toolkit cells.
pub fn screen_rows(screen: &vt100::Screen, theme: &GuiTheme) -> Vec<Vec<Cell>> {
    let (rows, cols) = screen.size();
    (0..rows).map(|r| (0..cols).map(|c| convert(screen.cell(r, c), theme)).collect()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// How the cursor is drawn this frame.
pub enum CursorDraw {
    /// Focused and in the visible blink phase.
    Block,
    /// Unfocused: an outline.
    Hollow,
}

/// Paints `screen` into `rect` (top-left aligned), one `CellRow` per line,
/// and the cursor at `cursor` when given.
#[allow(clippy::too_many_arguments)]
pub fn draw_screen(
    ui: &mut Ui,
    rect: Rect,
    screen: &vt100::Screen,
    theme: &GuiTheme,
    cell: Size,
    px: f32,
    cursor: Option<CursorDraw>,
    sel: Option<&TermSelection>,
    marks: &[(usize, usize, usize, bool)],
) {
    let selection = ui.style().palette.selection;
    let offset = screen.scrollback() as i64;
    let mut painter = ui.painter();
    painter.rect(rect, theme.term_bg, 0.0);
    painter.push_clip(rect);
    let mut rows = screen_rows(screen, theme);
    // Selection: cells inside it get the selection background (rows are
    // shifted by the scrollback offset into selection coordinates).
    if let Some(sel) = sel {
        for (r, cells) in rows.iter_mut().enumerate() {
            for (c, cell) in cells.iter_mut().enumerate() {
                if sel.contains(r as i64 - offset, c as u16) {
                    cell.bg = selection;
                }
            }
        }
    }
    // Search hits: all on the selection colour, the current one on the accent.
    for &(r, col, len, current) in marks {
        let Some(cells) = rows.get_mut(r) else { continue };
        for cell in cells.iter_mut().skip(col).take(len) {
            if current {
                cell.bg = theme.palette.accent;
                cell.fg = theme.term_bg;
            } else {
                cell.bg = selection;
            }
        }
    }
    // One `CellRow` primitive per line, stopping at the rect's bottom.
    for (r, cells) in rows.iter().enumerate() {
        let origin = Point::new(rect.x, rect.y + r as i32 * cell.h);
        if origin.y >= rect.bottom() {
            break;
        }
        painter.prim(Prim::CellRow { origin, cell, px, cells: cells.clone() });
    }
    // The cursor only on the live screen (not scrolled back) and when shown;
    // a wide character's cursor covers both cells.
    if let Some(kind) = cursor
        && !screen.hide_cursor()
        && screen.scrollback() == 0
    {
        let (row, col) = screen.cursor_position();
        let wide = rows.get(row as usize).and_then(|r| r.get(col as usize)).is_some_and(|c| c.attrs.wide);
        let w = if wide { 2 * cell.w } else { cell.w };
        let r = Rect::new(rect.x + col as i32 * cell.w, rect.y + row as i32 * cell.h, w, cell.h);
        match kind {
            CursorDraw::Block => {
                painter.rect(r, theme.term_fg.with_alpha(150), 0.0);
            }
            CursorDraw::Hollow => painter.border(r, theme.term_fg.with_alpha(180), 1, 0.0),
        }
    }
    painter.pop_clip();
}
