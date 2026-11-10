//! Terminal tabs: system ssh in a pty, rendered on our cell grid.

use std::time::{Duration, Instant};

use termax_core::hosts::HostEntry;
use termax_core::terminal::TerminalSession;
use termax_ui::geom::{Point, Rect, Size};
use termax_ui::input::{CursorIcon, Event, Key, Modifiers};
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Sense, Ui};
use termax_ui::widgets::{MenuItem, context_menu, label, open_context_menu};

use crate::engine::WakeFn;
use crate::keys::{Action, Keymap};
use crate::theme::GuiTheme;

use self::render::{CursorDraw, draw_screen};
use self::select::{SelectState, handle_mouse, selection_text};

pub mod input;
pub mod render;
pub mod select;

const BLINK: Duration = Duration::from_millis(530);
/// Logical pixels of wheel movement per scrolled line (a notch = 48 → 3 lines).
const WHEEL_PX_PER_LINE: f32 = 16.0;
const MIN_FONT: f32 = 8.0;
const MAX_FONT: f32 = 40.0;

pub struct TermTab {
    pub host: HostEntry,
    program: String,
    args: Vec<String>,
    scrollback: usize,
    pub session: Option<TerminalSession>,
    pub error: Option<String>,
    /// (rows, cols) last sent to the pty.
    pub grid: (u16, u16),
    /// How many times the pty was resized (tests watch this).
    pub resizes: usize,
    /// Terminal font size in logical px; 0 = the style's mono size.
    pub font_px: f32,
    /// Cell size (physical px) of the last frame.
    cell: Size,
    pub select: SelectState,
    blink_from: Instant,
    wheel_rest: f32,
}

/// Rows × columns that fit `rect` (at least 2 × 10, as the pty needs).
pub fn grid_for(rect: Rect, cell: Size) -> (u16, u16) {
    let rows = if cell.h > 0 { rect.h / cell.h } else { 0 };
    let cols = if cell.w > 0 { rect.w / cell.w } else { 0 };
    (rows.clamp(2, u16::MAX as i32) as u16, cols.clamp(10, u16::MAX as i32) as u16)
}

impl TermTab {
    pub fn new(host: HostEntry, ssh_binary: &str, scrollback: usize) -> Self {
        let args = host.shell_args();
        Self::with_command(host, ssh_binary.to_string(), args, scrollback)
    }

    /// A tab that runs `program args` (tests point this at the local fixture).
    pub fn with_command(host: HostEntry, program: String, args: Vec<String>, scrollback: usize) -> Self {
        TermTab {
            host,
            program,
            args,
            scrollback,
            session: None,
            error: None,
            grid: (0, 0),
            resizes: 0,
            font_px: 0.0,
            cell: Size::new(8, 16),
            select: SelectState::default(),
            blink_from: Instant::now(),
            wheel_rest: 0.0,
        }
    }

    /// Cell width and height of the last frame, in physical pixels.
    pub fn cell_size(&self) -> (i32, i32) {
        (self.cell.w, self.cell.h)
    }

    /// Status-bar text.
    pub fn status(&mut self) -> String {
        let state = match (&mut self.session, &self.error) {
            (_, Some(e)) => e.clone(),
            (Some(s), _) => {
                if s.is_alive() {
                    "connected".into()
                } else {
                    "session ended".into()
                }
            }
            (None, _) => "starting".into(),
        };
        format!("{}: {state}", self.host.name)
    }

    fn ensure_session(&mut self, grid: (u16, u16), wake: Option<&WakeFn>) {
        if self.session.is_some() || self.error.is_some() {
            return;
        }
        match TerminalSession::spawn(&self.program, &self.args, grid.0, grid.1, self.scrollback) {
            Ok(mut s) => {
                if let Some(w) = wake {
                    s.set_notify(w.clone());
                }
                self.grid = grid;
                self.session = Some(s);
            }
            Err(e) => self.error = Some(format!("could not start {}: {e:#}", self.program)),
        }
    }

    fn reset_blink(&mut self) {
        self.blink_from = Instant::now();
    }

    fn selected_text(&self) -> Option<String> {
        let sel = self.select.sel.as_ref()?;
        let p = self.session.as_ref()?.parser.lock().ok()?;
        let t = selection_text(p.screen(), sel, p.screen().scrollback());
        (!t.is_empty()).then_some(t)
    }

    fn zoom(&mut self, a: Action, default_px: f32) {
        let cur = if self.font_px > 0.0 { self.font_px } else { default_px };
        self.font_px = match a {
            Action::ZoomIn => (cur + 1.0).min(MAX_FONT),
            Action::ZoomOut => (cur - 1.0).max(MIN_FONT),
            _ => 0.0,
        };
    }
}

/// The terminal view: spawns the session once its size is known, keeps the
/// pty sized to the view, handles keys, mouse selection, clipboard, wheel
/// scrollback and zoom (`actions` are app shortcuts meant for this view).
#[allow(clippy::too_many_arguments)]
pub fn view(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    tab: &mut TermTab,
    theme: &GuiTheme,
    keymap: &Keymap,
    wake: Option<WakeFn>,
    actions: &mut Vec<Action>,
) {
    let r = ui.interact(id, rect, Sense { click: true, drag: true, focus: true });
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Text);
    }
    if r.pressed {
        ui.request_focus(id);
    }
    let focused = ui.has_focus(id);
    if focused {
        ui.capture_keyboard(id); // Tab and Esc belong to the shell
        // F6 / Shift+F6 still leave the terminal (no keyboard trap);
        // Ctrl+Shift+F6 sends F6 to the shell instead.
        let f6 = ui.input().events.iter().find_map(|e| match e {
            Event::KeyDown { key: Key::F(6), mods, .. } if !mods.ctrl && !mods.alt && !mods.logo => Some(!mods.shift),
            _ => None,
        });
        if let Some(forward) = f6 {
            ui.cycle_region(forward);
        }
    }
    let scale = ui.scale();
    let default_px = ui.mono_px() / scale;
    for a in actions.drain(..) {
        match a {
            Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => tab.zoom(a, default_px),
            Action::Copy => {
                if let Some(t) = tab.selected_text() {
                    ui.clipboard().set(&t);
                }
            }
            Action::Paste => {
                if let (Some(t), Some(s)) = (ui.clipboard().get(), tab.session.as_mut()) {
                    s.paste(&t);
                }
            }
            _ => {}
        }
    }
    let px = if tab.font_px > 0.0 { tab.font_px * scale } else { ui.mono_px() };
    let (cw, ch) = ui.fonts().cell_size(px);
    let cell = Size::new((cw.round() as i32).max(1), (ch.ceil() as i32).max(1));
    tab.cell = cell;
    let pad = ui.px(6);
    let inner = Rect::new(rect.x + pad, rect.y + pad, (rect.w - 2 * pad).max(0), (rect.h - 2 * pad).max(0));
    let grid = grid_for(inner, cell);

    tab.ensure_session(grid, wake.as_ref());
    if let Some(e) = tab.error.clone() {
        ui.painter().rect(rect, theme.term_bg, 0.0);
        label(ui, Rect::new(inner.x, inner.y, inner.w, cell.h + ui.px(4)), &e, Some(theme.palette.danger));
        return;
    }
    let Some(session) = tab.session.as_mut() else { return };
    // Too small for even the minimum grid (minimized, collapsed): keep the
    // pty's size, since shrinking it would cut the screen's text.
    let hidden = inner.w < cell.w * 10 || inner.h < cell.h * 2;
    if grid != tab.grid && !hidden {
        session.resize(grid.0, grid.1);
        tab.grid = grid;
        tab.resizes += 1;
    }

    // Wheel: scrollback (or arrow keys in full-screen apps like less/vim).
    let (_, dy) = ui.input().wheel();
    if dy != 0.0 && inner.contains(ui.input().mouse) {
        tab.wheel_rest += dy / (WHEEL_PX_PER_LINE * scale);
        let lines = tab.wheel_rest.trunc() as i32;
        tab.wheel_rest -= lines as f32;
        if lines != 0 {
            let alt = session.parser.lock().map(|p| p.screen().alternate_screen()).unwrap_or(false);
            if alt {
                let key = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
                for _ in 0..lines.unsigned_abs() {
                    session.write(key);
                }
            } else {
                session.scroll(lines);
            }
        }
    }

    let alive = session.is_alive();
    if focused && ui.keys_for(id) {
        let events = ui.input().events.clone();
        if !alive {
            if ui.input_mut().take_key(&Key::Enter, Modifiers::NONE) {
                if let Err(e) = session.respawn() {
                    tab.error = Some(format!("reconnect failed: {e:#}"));
                }
                tab.reset_blink();
            }
        } else {
            let bytes = input::route(&events, keymap, session.application_cursor());
            if !bytes.is_empty() {
                session.write(&bytes); // also snaps the view back to live output
                tab.reset_blink();
            }
        }
    }

    let Some(session) = tab.session.as_mut() else { return };
    let parser = session.parser.clone();
    let offset = parser.lock().map(|p| p.screen().scrollback()).unwrap_or(0);
    if handle_mouse(ui, id, inner, cell, &mut tab.select, offset)
        && let Ok(p) = parser.lock()
    {
        tab.select.resolve(p.screen(), offset);
    }

    // Right-click menu: Copy / Paste.
    let menu = id.with("menu");
    if r.right_clicked {
        open_context_menu(ui, menu, ui.input().mouse);
    }
    let has_sel = tab.select.sel.is_some();
    let items = [
        MenuItem { label: "Copy", shortcut: None, enabled: has_sel },
        MenuItem { label: "Paste", shortcut: None, enabled: true },
    ];
    match context_menu(ui, menu, &items) {
        Some(0) => actions.push(Action::Copy),
        Some(1) => actions.push(Action::Paste),
        _ => {}
    }

    let cursor = if !focused {
        Some(CursorDraw::Hollow)
    } else {
        let el = tab.blink_from.elapsed().as_millis();
        let left = BLINK.as_millis() - el % BLINK.as_millis();
        ui.request_repaint_in(Duration::from_millis(left as u64));
        ((el / BLINK.as_millis()) % 2 == 0).then_some(CursorDraw::Block)
    };
    ui.painter().rect(rect, theme.term_bg, 0.0);
    if let Ok(mut p) = parser.lock() {
        draw_screen(ui, inner, p.screen(), theme, cell, px, cursor, tab.select.sel.as_ref());
        if offset > 0 {
            draw_scrollbar(ui, rect, &mut p, offset, theme);
        }
    }
    if !alive {
        let msg = "[session ended \u{2014} press Enter to reconnect]";
        let lh = ui.line_height(FontKind::Mono, px);
        let y = inner.bottom() - lh - ui.px(4);
        let w = ui.measure(msg, FontKind::Mono, px).ceil() as i32 + ui.px(16);
        let bar = Rect::new(inner.x, y - ui.px(4), w.min(inner.w), lh + ui.px(8));
        let (radius, at) = (ui.px(4) as f32, Point::new(bar.x + ui.px(8), y));
        ui.painter().rect(bar, theme.palette.surface_alt, radius);
        ui.painter().text(at, msg, FontKind::Mono, px, theme.palette.warning);
    }
}

/// A thin scrollbar, shown while scrolled back into history.
fn draw_scrollbar(ui: &mut Ui, rect: Rect, p: &mut vt100::Parser, offset: usize, theme: &GuiTheme) {
    // vt100 clamps the offset to the history length: probe it.
    p.screen_mut().set_scrollback(usize::MAX);
    let history = p.screen().scrollback();
    p.screen_mut().set_scrollback(offset);
    let rows = p.screen().size().0 as usize;
    let total = (history + rows).max(1);
    let w = ui.px(4).max(2);
    let track = Rect::new(rect.right() - w - ui.px(2), rect.y + ui.px(2), w, rect.h - ui.px(4));
    let h = ((track.h as usize * rows / total) as i32).max(ui.px(20)).min(track.h);
    let top_line = history - offset; // first visible line, counted from the oldest
    let y = track.y + ((track.h - h) as usize * top_line / history.max(1)) as i32;
    let radius = w as f32 / 2.0;
    ui.painter().rect(Rect::new(track.x, y, w, h), theme.palette.muted.with_alpha(160), radius);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_select;
