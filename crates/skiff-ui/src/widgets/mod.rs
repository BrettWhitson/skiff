//! Widgets: plain functions over `&mut Ui` that handle input, publish an
//! accessibility node and paint in one call. Each submodule holds one widget
//! family; this module re-exports their public entry points.
//!
//! Design-system components (see `style::tokens` for the sizes they
//! expect): button variants and `icon_button`, `badge` / `status_dot` /
//! `status_pill`, `section_header`, `table_header`, `stepper`,
//! `empty_state`, modal `dialog`, the `toasts` stack, context menus with
//! icons / separators / danger items, and `text` / `elide_mode` /
//! `paragraph` for type-scale text with end, middle or start elision.

mod badge;
mod button;
mod checkbox;
mod dialog;
mod dropdown;
mod empty_state;
mod header;
mod label;
mod list;
mod menu;
mod progress;
mod scroll;
mod select_list;
mod splitter;
mod stepper;
mod tabs;
mod text_input;
mod toast;
mod tooltip;

pub use badge::{Status, Tone, badge, badge_width, status_color, status_dot, status_pill, tone_colors};
pub use button::{
    ButtonKind, ButtonOpts, button, button_backgrounds, button_colors, button_width, button_with, icon_button, icon_button_tip,
};
pub use checkbox::checkbox;
pub use dialog::{Dialog, DialogButton, DialogResult, dialog, dialog_dismiss};
pub use dropdown::{dropdown, dropdown_labeled};
pub use empty_state::{EmptyState, EmptyStateResponse, empty_state};
pub use header::{Column, Sort, TableHeaderResponse, section_header, table_header};
pub use label::{
    Align, Elide, TextOpts, elide, elide_by, elide_mode, label, label_with, paragraph, paragraph_height, paragraph_line_h, paragraph_lines,
    text, wrap_by,
};
pub use list::{ListResponse, list, list_named};
pub use menu::{MenuItem, close_context_menu, context_menu, menu_holds_keyboard, menu_is_open, menu_item_rect, open_context_menu};
pub use progress::progress_bar;
pub use scroll::{ScrollState, scroll_area, scroll_into_view, scroll_to};
pub use select_list::{SelectListResponse, Selection, select_list, select_list_named};
pub use splitter::splitter_h;
pub use stepper::{StepperOpts, parse_value, step_value, stepper};
pub use tabs::{TabTitle, TabsOpts, TabsResponse, tabs, tabs_with};
pub use text_input::{TextInputOpts, TextInputResponse, text_input};
pub use toast::{Toast, ToastEvent, ToastKind, Toasts, toasts, toasts_in};
pub use tooltip::tooltip;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_containers;

#[cfg(test)]
mod tests_select;

#[cfg(test)]
mod tests_components;
