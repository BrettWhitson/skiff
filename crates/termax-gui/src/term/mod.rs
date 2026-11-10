//! Terminal tabs: system ssh in a pty, rendered on our cell grid.

use std::time::{Duration, Instant};

use termax_core::hosts::HostEntry;
use termax_core::terminal::TerminalSession;
use termax_ui::geom::{Point, Rect, Size};
use termax_ui::input::{CursorIcon, Key, Modifiers};
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Sense, Ui};
use termax_ui::widgets::label;

use crate::engine::WakeFn;
use crate::keys::Keymap;
use crate::theme::GuiTheme;

use self::render::{CursorDraw, draw_screen};

pub mod input;
pub mod render;

const BLINK: Duration = Duration::from_millis(530);

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
    /// Terminal font size; 0 = the style's mono size.
    pub font_px: f32,
    blink_from: Instant,
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
            blink_from: Instant::now(),
        }
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
}

/// The terminal view: spawns the session once its size is known, keeps the
/// pty sized to the view, sends keys and draws the screen.
pub fn view(ui: &mut Ui, id: Id, rect: Rect, tab: &mut TermTab, theme: &GuiTheme, keymap: &Keymap, wake: Option<WakeFn>) {
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
    }
    let px = if tab.font_px > 0.0 { tab.font_px * ui.scale() } else { ui.mono_px() };
    let (cw, ch) = ui.fonts().cell_size(px);
    let cell = Size::new((cw.round() as i32).max(1), (ch.ceil() as i32).max(1));
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
    if grid != tab.grid {
        session.resize(grid.0, grid.1);
        tab.grid = grid;
        tab.resizes += 1;
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
                session.write(&bytes);
                tab.reset_blink();
            }
        }
    }

    let cursor = if !focused {
        Some(CursorDraw::Hollow)
    } else {
        let phase = tab.blink_from.elapsed().as_millis() / BLINK.as_millis();
        let left = BLINK.as_millis() - tab.blink_from.elapsed().as_millis() % BLINK.as_millis();
        ui.request_repaint_in(Duration::from_millis(left as u64));
        (phase % 2 == 0).then_some(CursorDraw::Block)
    };
    let Some(session) = tab.session.as_mut() else { return };
    let parser = session.parser.clone();
    ui.painter().rect(rect, theme.term_bg, 0.0);
    if let Ok(p) = parser.lock() {
        draw_screen(ui, inner, p.screen(), theme, cell, px, cursor);
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

#[cfg(test)]
mod tests;
