//! termax desktop GUI.

pub mod app;
pub mod engine;
pub mod files;
pub mod os;
pub mod forwards;
pub mod keys;
pub mod settings;
pub mod sidebar;
pub mod switcher;
pub mod term;
#[doc(hidden)]
pub mod testing;
pub mod theme;

#[cfg(test)]
mod host_tests;
