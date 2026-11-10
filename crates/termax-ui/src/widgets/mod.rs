//! Widgets: plain functions over `&mut Ui`.

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

pub use button::{button, button_backgrounds};
pub use checkbox::checkbox;
pub use dropdown::dropdown;
pub use label::{elide, label, label_with};
pub use list::{ListResponse, list, list_named};
pub use menu::{MenuItem, close_context_menu, context_menu, menu_is_open, open_context_menu};
pub use progress::progress_bar;
pub use select_list::{SelectListResponse, Selection, select_list, select_list_named};
pub use scroll::{ScrollState, scroll_area, scroll_into_view, scroll_to};
pub use splitter::splitter_h;
pub use tabs::{TabTitle, TabsResponse, tabs};
pub use text_input::{TextInputOpts, TextInputResponse, text_input};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_containers;

#[cfg(test)]
mod tests_select;
