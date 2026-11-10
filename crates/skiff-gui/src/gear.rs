//! The Settings button at the end of the tab strip: a gear drawn from
//! shapes (no icon font needed), with the shortcut in its tooltip.

use skiff_ui::a11y::Role;
use skiff_ui::geom::Rect;
use skiff_ui::input::{CursorIcon, Key, Modifiers};
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::tooltip;

/// The button's widget id.
pub fn id() -> Id {
    Id::new("settings-button")
}

/// The tooltip: "Settings (Ctrl+Shift+,)", or just "Settings" when unbound.
pub fn tip(shortcut: Option<String>) -> String {
    match shortcut {
        Some(s) => format!("Settings ({s})"),
        None => "Settings".into(),
    }
}

/// A focusable icon button named "Settings"; true when activated (click,
/// Space / Enter, or the screen reader's click).
pub fn button(ui: &mut Ui, rect: Rect, tip: &str) -> bool {
    let id = id();
    let r = ui.interact(id, rect, Sense::focusable());
    let mut clicked = r.clicked;
    if r.focused && ui.keys_for(id) {
        let space = ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
        let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
        clicked |= space || enter;
    }
    ui.a11y_node(id, Role::Button, "Settings");
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Pointer);
    }
    let p = ui.style().palette;
    let radius = ui.px(4) as f32;
    let face = rect.inset((rect.h / 8).max(1));
    let bg = if r.pressed {
        p.selection
    } else if r.hovered {
        p.surface_alt
    } else {
        p.bg
    };
    // Muted at rest, full foreground when pointed at or focused: both pass
    // the text contrast checks against `bg` / `surface_alt`.
    let fg = if r.hovered || r.focused || r.pressed { p.fg } else { p.muted };
    let c = rect.center();
    let outer = (rect.h as f32 * 0.26).max(4.0);
    let body = (outer * 0.78).round() as i32;
    let hole = (outer * 0.34).round().max(1.0) as i32;
    let tooth = (outer * 0.5).round().max(2.0) as i32;
    let mut painter = ui.painter();
    if r.hovered || r.pressed {
        painter.rect(face, bg, radius);
    }
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let (tx, ty) = (c.x + (a.cos() * outer).round() as i32, c.y + (a.sin() * outer).round() as i32);
        painter.rect(Rect::new(tx - tooth / 2, ty - tooth / 2, tooth, tooth), fg, 1.0);
    }
    painter.rect(Rect::new(c.x - body, c.y - body, 2 * body, 2 * body), fg, body as f32);
    painter.rect(Rect::new(c.x - hole, c.y - hole, 2 * hole, 2 * hole), bg, hole as f32);
    if r.hovered || r.focused {
        tooltip(ui, rect, tip);
    }
    clicked
}
