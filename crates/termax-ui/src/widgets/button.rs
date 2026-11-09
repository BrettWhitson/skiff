use crate::geom::{Point, Rect};
use crate::input::{CursorIcon, Key, Modifiers};
use crate::text::FontKind;
use crate::ui::{Id, Response, Sense, Ui};

use super::label::elide;

/// Activates a focused widget on Space or Enter (consuming the key).
pub(crate) fn keyboard_activate(ui: &mut Ui, r: &mut Response) {
    if r.focused {
        let space = ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
        let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
        if space || enter {
            r.clicked = true;
        }
    }
}

/// A push button with a centred label. Focusable; Space/Enter activate it.
pub fn button(ui: &mut Ui, id: Id, rect: Rect, text: &str) -> Response {
    let mut r = ui.interact(id, rect, Sense::focusable());
    keyboard_activate(ui, &mut r);
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Pointer);
    }
    let p = ui.style().palette;
    let radius = ui.style().radius * ui.scale();
    let bg = if r.pressed {
        p.surface_alt.mix(p.accent, 0.35)
    } else if r.hovered {
        p.surface_alt.mix(p.fg, 0.08)
    } else {
        p.surface_alt
    };
    let border = if r.focused { p.accent } else { p.border };
    let bw = ui.px(1).max(1);
    let px = ui.font_px();
    let pad = ui.px(ui.style().padding);
    let shown = elide(ui, text, FontKind::Ui, px, rect.w - 2 * pad);
    let w = ui.measure(&shown, FontKind::Ui, px).ceil() as i32;
    let lh = ui.line_height(FontKind::Ui, px);
    let mut painter = ui.painter();
    painter.rect(rect, bg, radius);
    painter.border(rect, border, bw, radius);
    let pos = Point::new(rect.x + (rect.w - w) / 2, rect.y + (rect.h - lh) / 2);
    painter.text(pos, &shown, FontKind::Ui, px, p.fg);
    r
}
