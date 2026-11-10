//! vt100 screen → toolkit cell rows.

use termax_ui::color::Color;
use termax_ui::display::{Cell, CellAttrs, Prim};
use termax_ui::geom::{Point, Rect, Size};
use termax_ui::ui::Ui;

use crate::theme::{GuiTheme, xterm_256};

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

pub fn screen_rows(screen: &vt100::Screen, theme: &GuiTheme) -> Vec<Vec<Cell>> {
    let (rows, cols) = screen.size();
    (0..rows).map(|r| (0..cols).map(|c| convert(screen.cell(r, c), theme)).collect()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
) {
    let mut painter = ui.painter();
    painter.rect(rect, theme.term_bg, 0.0);
    painter.push_clip(rect);
    let rows = screen_rows(screen, theme);
    for (r, cells) in rows.iter().enumerate() {
        let origin = Point::new(rect.x, rect.y + r as i32 * cell.h);
        if origin.y >= rect.bottom() {
            break;
        }
        painter.prim(Prim::CellRow { origin, cell, px, cells: cells.clone() });
    }
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
