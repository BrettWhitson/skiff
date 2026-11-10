//! termax-ui: a small GUI toolkit written from scratch.
//!
//! Only the bottom layers come from crates — `winit` (window + input),
//! `softbuffer` (putting pixels on screen) and `fontdue` (glyph outlines,
//! behind our own trait). Drawing, text layout, damage tracking, layout,
//! focus and every widget are implemented here.

pub mod a11y;
pub mod canvas;
pub mod clipboard;
pub mod color;
#[doc(hidden)]
pub mod demo;
pub mod display;
pub mod dnd;
pub mod geom;
pub mod input;
pub mod layout;
pub mod shell;
pub mod style;

pub mod text;
pub mod ui;
pub mod widgets;

#[cfg(test)]
mod a11y_tests;
