//! Widgets: plain functions over `&mut Ui`.

mod button;
mod checkbox;
mod label;
mod text_input;

pub use button::button;
pub use checkbox::checkbox;
pub use label::{elide, label, label_with};
pub use text_input::{TextInputOpts, TextInputResponse, text_input};

#[cfg(test)]
mod tests;
