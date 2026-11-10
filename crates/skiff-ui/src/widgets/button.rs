//! Push buttons, and the keyboard activation (Space/Enter) shared with
//! other clickable widgets.

use crate::a11y::Role;
use crate::color::Color;
use crate::geom::{Point, Rect};
use crate::input::{CursorIcon, Key, Modifiers};
use crate::style::Palette;
use crate::text::FontKind;
use crate::ui::{Id, Response, Sense, Ui};

use super::label::elide;

/// Activates a focused widget on Space or Enter (consuming the key).
pub(crate) fn keyboard_activate(ui: &mut Ui, id: Id, r: &mut Response) {
    if r.focused && ui.keys_for(id) {
        let space = ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
        let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
        if space || enter {
            r.clicked = true;
        }
    }
}

/// A button's fill at rest, hovered and pressed (its label is `fg`).
pub fn button_backgrounds(p: &Palette) -> [(&'static str, Color); 3] {
    [("rest", p.surface_alt), ("hover", p.surface_alt.mix(p.fg, 0.08)), ("pressed", p.selection)]
}

/// A push button with a centred label. Focusable; Space/Enter activate it.
pub fn button(ui: &mut Ui, id: Id, rect: Rect, text: &str) -> Response {
    let mut r = ui.interact(id, rect, Sense::focusable());
    keyboard_activate(ui, id, &mut r);
    ui.a11y_node(id, Role::Button, text);
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Pointer);
    }
    let p = ui.style().palette;
    let radius = ui.style().radius * ui.scale();
    let [(_, rest), (_, hover), (_, pressed)] = button_backgrounds(&p);
    // Pressed beats hovered beats resting; the focused state shows on the border.
    let bg = if r.pressed {
        pressed
    } else if r.hovered {
        hover
    } else {
        rest
    };
    let border = if r.focused { p.accent } else { p.control };
    let bw = ui.px(1).max(1);
    let px = ui.font_px();
    let pad = ui.px(ui.style().padding);
    // Elide the label to the inner width, then centre it in both directions.
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
