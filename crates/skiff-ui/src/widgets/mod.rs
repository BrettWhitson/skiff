//! Widgets: plain functions over `&mut Ui` that handle input, publish an
//! accessibility node and paint in one call. Each submodule holds one widget
//! family; this module re-exports their public entry points.

mod button;
mod checkbox;
mod dropdown;
mod label;
mod list;
mod menu;
mod progress;
mod scroll;
mod select_list;
mod splitter;
mod tabs;
mod text_input;
mod tooltip;

pub use button::{button, button_backgrounds};
pub use checkbox::checkbox;
pub use dropdown::{dropdown, dropdown_labeled};
pub use label::{elide, label, label_with};
pub use list::{ListResponse, list, list_named};
pub use menu::{MenuItem, close_context_menu, context_menu, menu_is_open, open_context_menu};
pub use progress::progress_bar;
pub use scroll::{ScrollState, scroll_area, scroll_into_view, scroll_to};
pub use select_list::{SelectListResponse, Selection, select_list, select_list_named};
pub use splitter::splitter_h;
pub use tabs::{TabTitle, TabsResponse, tabs};
pub use text_input::{TextInputOpts, TextInputResponse, text_input};
pub use tooltip::tooltip;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_containers;

#[cfg(test)]
mod tests_select;
