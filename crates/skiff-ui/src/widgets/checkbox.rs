//! The checkbox widget: a square toggle with a label beside it.

use crate::a11y::Role;
use crate::geom::Rect;
use crate::input::CursorIcon;
use crate::ui::{Id, Response, Sense, Ui};

use super::button::keyboard_activate;
use super::label::label;

/// A checkbox with a label; clicking either (or Space when focused) toggles.
pub fn checkbox(ui: &mut Ui, id: Id, rect: Rect, value: &mut bool, text: &str) -> Response {
    let mut r = ui.interact(id, rect, Sense::focusable());
    keyboard_activate(ui, id, &mut r);
    if r.clicked {
        *value = !*value;
    }
    if let Some(n) = ui.a11y_node(id, Role::CheckBox, text) {
        n.checked = Some(*value);
    }
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Pointer);
    }
    let p = ui.style().palette;
    // A 16px box centred vertically at the left; the label takes the rest of the row.
    let side = ui.px(16).min(rect.h).max(0);
    let bx = Rect::new(rect.x, rect.y + (rect.h - side) / 2, side, side);
    let radius = ui.px(3) as f32;
    let bw = ui.px(1).max(1);
    {
        let mut painter = ui.painter();
        // Checked: accent-filled box with a smaller inner square as the mark.
        if *value {
            painter.rect(bx, p.accent, radius);
            painter.rect(bx.inset(side / 4), p.accent_fg, radius / 2.0);
        } else {
            painter.rect(bx, p.surface, radius);
        }
        painter.border(bx, if r.focused || r.hovered { p.accent } else { p.control }, bw, radius);
    }
    let gap = ui.px(ui.style().spacing);
    let text_rect = Rect::new(bx.right() + gap, rect.y, rect.right() - bx.right() - gap, rect.h);
    label(ui, text_rect, text, None);
    r
}
