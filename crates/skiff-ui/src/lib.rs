//! skiff-ui: a small GUI toolkit written from scratch.
//!
//! Only the bottom layers come from crates — `winit` (window + input),
//! `softbuffer` (putting pixels on screen) and `fontdue` (glyph outlines,
//! behind our own trait). Drawing, text layout, damage tracking, layout,
//! focus and every widget are implemented here. `skiff-gui` builds the
//! Skiff desktop app on top of it; the toolkit itself knows nothing about
//! SSH.
//!
//! # Immediate mode
//!
//! There is no retained widget tree. Every frame the app calls
//! `ui::Ui::run`, and widgets are plain functions over `&mut Ui` that lay
//! themselves out, ask `Ui::interact` what the user did to their
//! rectangle, and push drawing commands. Anything that must survive the
//! frame (focus, the pressed widget, per-widget memory, the drag in
//! progress) lives in `ui::UiState`, which the app keeps; widgets are
//! identified across frames by a hashed `ui::Id`. Hit-testing uses the
//! rectangles registered during the previous frame, so the topmost widget
//! (overlays included) gets the click.
//!
//! # Frame loop
//!
//! 1. `shell` turns winit events into toolkit `input::Event`s and queues
//!    them until the next redraw.
//! 2. On redraw it calls `shell::App::frame` with a `shell::FrameCtx`
//!    holding the events, window size, DPI scale, fonts and an empty
//!    `display::DisplayList`.
//! 3. The app (usually through `Ui::run` and `widgets`) fills the list with
//!    `display::Prim`s: rects, borders, text, terminal cell rows and clips.
//! 4. `display::TileRenderer` hashes what reaches each 64px tile and
//!    repaints only tiles that changed, onto a CPU `canvas::Canvas`, using
//!    several threads for big repaints.
//! 5. The shell copies the damaged areas to the window, applies the app's
//!    requests (title, cursor, quit, font swap) and sleeps until the next
//!    event or the requested repaint time.
//!
//! The same frame code runs headless: `ui::testing::Harness` and
//! `shell::render_offscreen` drive frames against an in-memory canvas,
//! clipboard and deterministic test font, so widgets and apps are tested
//! without a window.
//!
//! # Accessibility
//!
//! While a screen reader is listening, each frame also builds an
//! accessibility tree (`a11y`): widgets add a node with role, name and
//! state as they draw, with bounds taken from their hit rectangle. The
//! shell converts the tree to AccessKit updates (`a11y_kit`), and
//! AccessKit actions (click, focus, set value...) come back as ordinary
//! `input::Event::A11y` events handled in the next frame. When nobody is
//! listening the tree is not built at all. Keyboard access follows the
//! same model: Tab cycles focusable widgets, F6 moves between focus
//! regions, and a focus ring shows while focus is moved by keyboard.
//!
//! # Modules
//!
//! - `ui`: the per-frame context, input, focus, overlays, drag-and-drop.
//! - `widgets`: buttons, inputs, lists, menus, tabs, tooltips, scroll areas...
//! - `display`, `canvas`: drawing commands and the tiled CPU renderer.
//! - `icons`: the in-house vector icon set.
//! - `style`: design tokens (spacing, radii, heights, type scale, motion)
//!   and palettes.
//! - `text`: fonts, fallbacks, glyph cache, single-line layout.
//! - `shell`: the winit / softbuffer window and event loop.
//! - `a11y`, `a11y_kit`: accessibility tree and AccessKit bridge.
//! - `geom`, `layout`, `input`, `color`, `style`, `clipboard`, `dnd`:
//!   shared building blocks.

pub mod a11y;
pub mod a11y_kit;
pub mod canvas;
pub mod clipboard;
pub mod color;
#[doc(hidden)]
pub mod demo;
pub mod display;
pub mod dnd;
pub mod geom;
pub mod icons;
pub mod input;
pub mod layout;
pub mod shell;
pub mod style;

pub mod text;
pub mod ui;
pub mod widgets;

#[cfg(test)]
mod a11y_tests;
