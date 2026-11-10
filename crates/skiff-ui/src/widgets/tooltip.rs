//! A one-line tooltip under (or above) a widget, on the overlay layer.

use crate::geom::{Point, Rect};
use crate::text::FontKind;
use crate::ui::Ui;

/// Draws `text` in a small box below `anchor` (above it when there is no
/// room), kept inside the window. Call it while the widget is hovered or
/// focused; it takes no input.
pub fn tooltip(ui: &mut Ui, anchor: Rect, text: &str) {
    let px = ui.font_px();
    let pad = ui.px(6);
    let w = ui.measure(text, FontKind::Ui, px).ceil() as i32 + 2 * pad;
    let h = ui.line_height(FontKind::Ui, px) + pad;
    let win = ui.window_rect();
    let below = anchor.bottom() + ui.px(4);
    let y = if below + h <= win.bottom() { below } else { (anchor.y - ui.px(4) - h).max(0) };
    let x = (anchor.x + anchor.w / 2 - w / 2).clamp(0, (win.right() - w).max(0));
    let r = Rect::new(x, y, w, h);
    let p = ui.style().palette;
    let radius = ui.px(4) as f32;
    let bw = ui.px(1).max(1);
    let lh = ui.line_height(FontKind::Ui, px);
    ui.overlay(|ui| {
        let mut painter = ui.painter();
        painter.rect(r, p.surface_alt, radius);
        painter.border(r, p.control, bw, radius);
        painter.text(Point::new(r.x + pad, r.y + (r.h - lh) / 2), text, FontKind::Ui, px, p.fg);
    });
}
