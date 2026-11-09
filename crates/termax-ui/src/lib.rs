//! termax-ui: a small GUI toolkit written from scratch.
//!
//! Only the bottom layers come from crates — `winit` (window + input),
//! `softbuffer` (putting pixels on screen) and `fontdue` (glyph outlines,
//! behind our own trait). Drawing, text layout, damage tracking, layout,
//! focus and every widget are implemented here.

pub mod canvas;
pub mod clipboard;
pub mod color;
pub mod display;
pub mod geom;
pub mod input;
pub mod shell;

pub mod text;
