//! Dropdowns: a button showing the current choice that opens a context
//! menu of all choices.

use crate::a11y::Role;
use crate::geom::{Point, Rect};
use crate::icons::Icon;
use crate::ui::{Id, Ui};

use super::button::{ButtonOpts, button_with};
use super::menu::{MenuItem, context_menu, open_context_menu};

/// A button showing `options[*selected]` that opens a menu of all options.
/// Returns true when the selection changed.
pub fn dropdown(ui: &mut Ui, id: Id, rect: Rect, options: &[&str], selected: &mut usize) -> bool {
    dropdown_labeled(ui, id, rect, "", options, selected)
}

/// `dropdown` that screen readers announce as a combo box named `label`
/// (the form field it sets), with the current option as its value.
pub fn dropdown_labeled(ui: &mut Ui, id: Id, rect: Rect, label: &str, options: &[&str], selected: &mut usize) -> bool {
    let menu = id.with("menu");
    let current = options.get(*selected).copied().unwrap_or("");
    // The button shows the current option with a down chevron.
    let opts = ButtonOpts { trailing: Some(Icon::ChevronDown), ..Default::default() };
    if button_with(ui, id, rect, current, opts).clicked {
        open_context_menu(ui, menu, Point::new(rect.x, rect.bottom() + ui.px(2)));
    }
    if !label.is_empty()
        && let Some(n) = ui.a11y_node(id, Role::ComboBox, label)
    {
        // The button's own node (the first one for an id wins), renamed.
        n.role = Role::ComboBox;
        n.name = label.to_string();
        n.value = Some(current.to_string());
    }
    let items: Vec<MenuItem> = options.iter().map(|o| MenuItem::new(o)).collect();
    match context_menu(ui, menu, &items) {
        Some(i) if i != *selected => {
            *selected = i;
            true
        }
        _ => false,
    }
}
