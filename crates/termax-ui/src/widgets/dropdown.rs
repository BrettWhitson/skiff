use crate::geom::{Point, Rect};
use crate::ui::{Id, Ui};

use super::button::button;
use super::menu::{MenuItem, context_menu, open_context_menu};

/// A button showing `options[*selected]` that opens a menu of all options.
/// Returns true when the selection changed.
pub fn dropdown(ui: &mut Ui, id: Id, rect: Rect, options: &[&str], selected: &mut usize) -> bool {
    let menu = id.with("menu");
    let current = options.get(*selected).copied().unwrap_or("");
    if button(ui, id, rect, &format!("{current}  \u{25BE}")).clicked {
        open_context_menu(ui, menu, Point::new(rect.x, rect.bottom()));
    }
    let items: Vec<MenuItem> = options.iter().map(|o| MenuItem { label: o, shortcut: None, enabled: true }).collect();
    match context_menu(ui, menu, &items) {
        Some(i) if i != *selected => {
            *selected = i;
            true
        }
        _ => false,
    }
}
