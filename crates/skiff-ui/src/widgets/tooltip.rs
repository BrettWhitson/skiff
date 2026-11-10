//! A one-line tooltip under (or above) a widget, on the overlay layer.

use crate::geom::{Point, Rect};
use crate::style::{TextRole, tokens};
use crate::ui::Ui;

/// Draws `text` in a small box below `anchor` (above it when there is no
/// room), kept inside the window. Call it while the widget is hovered or
/// focused; it takes no input.
pub fn tooltip(ui: &mut Ui, anchor: Rect, text: &str) {
    let (font, px) = ui.text_style(TextRole::Caption);
    let (padx, pady) = (ui.px(tokens::SPACE_SM), ui.px(tokens::SPACE_XS));
    let lh = ui.line_height(font, px);
    let w = ui.measure(text, font, px).ceil() as i32 + 2 * padx;
    let h = lh + 2 * pady;
    let win = ui.window_rect();
    // Prefer 6px below the anchor, flipping above it when that would leave
    // the window; center on the anchor horizontally, clamped to the window.
    let off = ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
    let below = anchor.bottom() + off;
    let y = if below + h <= win.bottom() { below } else { (anchor.y - off - h).max(0) };
    let x = (anchor.x + anchor.w / 2 - w / 2).clamp(0, (win.right() - w).max(0));
    let r = Rect::new(x, y, w, h);
    let p = ui.style().palette;
    let radius = tokens::RADIUS_SM * ui.scale();
    let bw = ui.px(1).max(1);
    ui.overlay(|ui| {
        let mut painter = ui.painter();
        painter.rect(r, p.overlay, radius);
        painter.border(r, p.control, bw, radius);
        painter.text(Point::new(r.x + padx, r.y + pady), text, font, px, p.fg);
    });
}
