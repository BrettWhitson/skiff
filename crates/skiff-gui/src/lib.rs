//! skiff desktop GUI.
//!
//! The windowed front-end of skiff, built on the in-house `skiff-ui`
//! immediate-mode toolkit. Connection logic, config, the terminal model and
//! the SFTP engine live in `skiff-core`; this crate draws them and turns user
//! input into calls on them. The `skiff-gui` binary (`main.rs`) just loads the
//! config and fonts and hands a [`app::GuiApp`] to the toolkit's event loop.
//!
//! Main modules:
//! - [`app`]: the application state (config, hosts, theme, tabs, engine) and
//!   the toolkit `App` implementation that lays out every screen.
//! - [`sidebar`]: the host list with filter, sort, status dots and menus.
//! - [`term`]: terminal tabs (system ssh in a pty, drawn on a cell grid),
//!   with selection, search and split panes.
//! - [`files`]: two-pane file-transfer tabs over local disk or SFTP.
//! - [`forwards`]: the port-forward manager tab.
//! - [`settings`]: the settings tab (connections, appearance, keys, snippets).
//! - [`switcher`]: the quick switcher overlay.
//! - [`gear`]: the Settings gear button at the end of the tab strip.
//! - [`keys`]: app actions, default chords and `[keys]` overrides.
//! - [`engine`]: the background tokio runtime and the UI wake-up hook.
//! - [`theme`]: maps a core theme onto toolkit and terminal colours.
//! - [`os`]: cached OS accessibility preferences (high contrast, animation,
//!   text size).
//! - `testing`: helpers shared by tests and examples (hidden from docs).

pub mod app;
pub mod engine;
pub mod files;
pub mod forwards;
pub mod gear;
pub mod keys;
pub mod os;
pub mod settings;
pub mod sidebar;
pub mod switcher;
pub mod term;
#[doc(hidden)]
pub mod testing;
pub mod theme;

#[cfg(test)]
mod host_tests;
