//! Tests for `Prim::CellRow` painting (terminal rows): per-cell backgrounds,
//! wide chars, bold / italic / underline, and per-row damage.

use super::*;
use crate::geom::Size;
use crate::text::test_fonts;

const BG: Color = Color::rgb(0, 0, 0);

/// A cell with default attributes and hex colors.
fn cell(ch: char, fg: u32, bg: u32) -> Cell {
    Cell { ch, fg: Color::hex(fg), bg: Color::hex(bg), attrs: CellAttrs::default() }
}

/// A row of 8x16 cells at height `y`, 14px font.
fn row_prim(y: i32, cells: Vec<Cell>) -> Prim {
    Prim::CellRow { origin: Point::new(0, y), cell: Size::new(8, 16), px: 14.0, cells }
}

/// Renders `list` in one full pass on a `w`x`h` black canvas.
fn paint(list: &DisplayList, w: i32, h: i32) -> Canvas {
    let mut c = Canvas::new(w, h);
    render_full(list, &mut c, &test_fonts(), &mut GlyphCache::default(), BG);
    c
}

/// Columns (x) holding glyph ink in rows y0..y1.
fn ink_cols(c: &Canvas, y0: i32, y1: i32) -> Vec<i32> {
    (0..c.width).filter(|&x| (y0..y1).any(|y| is_fgish(c.pixel(x, y)))).collect()
}

/// Whether pixel `p` is bright in all channels (glyph ink, not a background).
fn is_fgish(p: u32) -> bool {
    // test glyph colour is white blended at coverage 200
    (p >> 16) & 0xff > 0x80 && (p >> 8) & 0xff > 0x80 && p & 0xff > 0x80
}

/// Each cell paints its own background, and a glyph's ink stays inside its cell.
#[test]
fn cell_row_paints_bg_and_glyph_per_cell() {
    let mut l = DisplayList::default();
    l.push(row_prim(0, vec![cell(' ', 0xffffff, 0xff0000), cell('A', 0xffffff, 0x00ff00), cell(' ', 0xffffff, 0x0000ff)]));
    let c = paint(&l, 40, 16);
    assert_eq!(c.pixel(4, 14), 0xff0000);
    assert_eq!(c.pixel(20, 14), 0x0000ff);
    // 'A' (6x10 block at xmin 1) lands only inside cell 1 (x 8..16)
    let cols = ink_cols(&c, 0, 16);
    assert!(!cols.is_empty());
    assert!(cols.iter().all(|&x| (8..16).contains(&x)), "{cols:?}");
    // bg of the glyph cell is visible below the glyph
    assert_eq!(c.pixel(8, 15), 0x00ff00);
}

/// A wide char's background spans two columns and its continuation cell is skipped.
#[test]
fn cell_row_wide_char_spans_two_cells() {
    let mut wide = cell('日', 0xffffff, 0x00ff00);
    wide.attrs.wide = true;
    let cont = cell('\0', 0xffffff, 0xff0000); // continuation: skipped entirely
    let mut l = DisplayList::default();
    l.push(row_prim(0, vec![wide, cont, cell('x', 0xffffff, 0x0000ff)]));
    let c = paint(&l, 40, 16);
    assert_eq!(c.pixel(12, 15), 0x00ff00, "wide cell bg covers both columns");
    assert_eq!(c.pixel(20, 15), 0x0000ff, "next char starts at column 2");
}

/// Changing one cell in a 24-row grid repaints only tiles around that row.
#[test]
fn changing_one_row_damages_only_its_tiles() {
    let fonts = test_fonts();
    let mut cache = GlyphCache::default();
    let mut canvas = Canvas::new(640, 24 * 16);
    let mut r = TileRenderer::default();
    let scene = |changed: bool| {
        let mut l = DisplayList::default();
        for y in 0..24 {
            let ch = if changed && y == 5 { 'Z' } else { 'a' };
            l.push(row_prim(y * 16, (0..80).map(|_| cell(ch, 0xffffff, 0x101010)).collect()));
        }
        l
    };
    r.render(&scene(false), &mut canvas, &fonts, &mut cache, BG);
    // BlockFont draws every char the same, so change a colour instead
    let mut l = scene(false);
    if let Prim::CellRow { cells, .. } = &mut l.prims[5] {
        cells[3].fg = Color::hex(0xff00ff);
    }
    let damage = r.render(&l, &mut canvas, &fonts, &mut cache, BG);
    assert!(!damage.is_empty());
    for d in &damage {
        assert!(d.y <= 5 * 16 && d.bottom() >= 6 * 16 && d.h <= 128, "{d:?}");
    }
}

/// Bold widens the ink by a pixel; italic shifts upper rows to the right.
#[test]
fn bold_and_italic_change_ink() {
    let draw = |attrs: CellAttrs| {
        let mut l = DisplayList::default();
        l.push(row_prim(0, vec![Cell { ch: 'A', fg: Color::WHITE, bg: BG, attrs }]));
        paint(&l, 16, 16)
    };
    let span = |c: &Canvas, y: i32| {
        let xs: Vec<i32> = (0..c.width).filter(|&x| c.pixel(x, y) != 0).collect();
        (xs.first().copied(), xs.last().copied())
    };
    let regular = draw(CellAttrs::default());
    let bold = draw(CellAttrs { bold: true, ..Default::default() });
    let italic = draw(CellAttrs { italic: true, ..Default::default() });
    // glyph rows: baseline = 12, height 10 → y 2..12
    let (r0, r1) = span(&regular, 6);
    let (b0, b1) = span(&bold, 6);
    assert_eq!(b0, r0);
    assert_eq!(b1.unwrap(), r1.unwrap() + 1, "bold widens by one pixel");
    let (it_top, _) = span(&italic, 2);
    let (it_bot, _) = span(&italic, 11);
    assert!(it_top.unwrap() > it_bot.unwrap(), "italic top row shifted right");
}

/// Underline draws a full-width line in the lower part of the cell.
#[test]
fn underline_draws_below_glyph() {
    let mut l = DisplayList::default();
    l.push(row_prim(0, vec![Cell { ch: ' ', fg: Color::WHITE, bg: BG, attrs: CellAttrs { underline: true, ..Default::default() } }]));
    let c = paint(&l, 8, 16);
    assert!((12..16).any(|y| (0..8).all(|x| c.pixel(x, y) == 0xffffff)));
}
