use crate::geom::Rect;
use crate::input::CursorIcon;
use crate::ui::{Id, Response, Sense, Ui};

use super::button::keyboard_activate;
use super::label::label;

/// A checkbox with a label; clicking either (or Space when focused) toggles.
pub fn checkbox(ui: &mut Ui, id: Id, rect: Rect, value: &mut bool, text: &str) -> Response {
    let mut r = ui.interact(id, rect, Sense::focusable());
    keyboard_activate(ui, &mut r);
    if r.clicked {
        *value = !*value;
    }
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Pointer);
    }
    let p = ui.style().palette;
    let side = ui.px(16).min(rect.h).max(0);
    let bx = Rect::new(rect.x, rect.y + (rect.h - side) / 2, side, side);
    let radius = ui.px(3) as f32;
    let bw = ui.px(1).max(1);
    {
        let mut painter = ui.painter();
        if *value {
            painter.rect(bx, p.accent, radius);
            painter.rect(bx.inset(side / 4), p.accent_fg, radius / 2.0);
        } else {
            painter.rect(bx, p.surface, radius);
        }
        painter.border(bx, if r.focused || r.hovered { p.accent } else { p.border }, bw, radius);
    }
    let gap = ui.px(ui.style().spacing);
    let text_rect = Rect::new(bx.right() + gap, rect.y, rect.right() - bx.right() - gap, rect.h);
    label(ui, text_rect, text, None);
    r
}
