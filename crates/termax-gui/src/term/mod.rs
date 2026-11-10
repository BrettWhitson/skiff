//! Terminal tabs: system ssh in a pty, rendered on our cell grid.

use std::time::{Duration, Instant};

use termax_core::hosts::HostEntry;
use termax_core::terminal::TerminalSession;
use termax_ui::a11y::Role;
use termax_ui::geom::{Point, Rect, Size};
use termax_ui::input::{CursorIcon, Event, Key, Modifiers};
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Sense, Ui};
use termax_ui::layout::Size as L;
use termax_ui::widgets::{MenuItem, button, context_menu, label, open_context_menu};

use crate::engine::WakeFn;
use crate::keys::{Action, Keymap};
use crate::theme::GuiTheme;

use self::render::{CursorDraw, draw_screen};
use self::select::{SelectState, handle_mouse, selection_text};

pub mod announce;
pub mod input;
pub mod render;
pub mod mouse;
pub mod search;
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
    /// The cursor blinks (`gui.cursor_blink` and the OS animation setting); set by the app.
    pub blink: bool,
    /// Which output screen readers hear (`gui.announce_output`); set by the app.
    pub announce: termax_core::config::Announce,
    announcer: announce::Announcer,
    /// The screen text the tree last showed.
    last_screen: String,
    /// The latest announcement (a live region's text).
    live: String,
    /// Take the screen as read without announcing it (tab shown again,
    /// or a screen reader just connected).
    resync: bool,
    /// The open find bar.
    pub search: Option<search::SearchState>,
    /// Ask before a multi-line paste (`settings.confirm_multiline_paste`); set by the app.
    pub confirm_paste: bool,
    /// A paste waiting for a yes: the text and its line count.
    pub pending_paste: Option<(String, usize)>,
    mouse: mouse::MouseState,
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
            blink: true,
            announce: termax_core::config::Announce::All,
            announcer: announce::Announcer::new(Instant::now()),
            last_screen: String::new(),
            live: String::new(),
            resync: false,
            search: None,
            confirm_paste: true,
            pending_paste: None,
            mouse: mouse::MouseState::default(),
        }
    }

    /// Cell width and height of the last frame, in physical pixels.
    /// Sends (`yes`) or drops the paste waiting for confirmation.
    pub fn paste_answer(&mut self, yes: bool) {
        if let Some((text, _)) = self.pending_paste.take()
            && yes
            && let Some(s) = self.session.as_mut()
        {
            s.paste(&text);
        }
    }

    /// The tab came to the front: its screen is taken as read, so screen
    /// readers hear only output from here on.
    pub fn shown(&mut self) {
        self.resync = true;
    }

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
                    let bracketed = s.parser.lock().map(|p| p.screen().bracketed_paste()).unwrap_or(false);
                    match termax_core::termx::paste_lines(&t, bracketed, tab.confirm_paste) {
                        Some(n) => tab.pending_paste = Some((t, n)),
                        None => s.paste(&t),
                    }
                }
            }
            Action::Find => {
                tab.search.get_or_insert_default();
                ui.request_focus(id.with("find").with("input"));
            }
            _ => {}
        }
    }
    // After the actions: Find moves focus to the find bar this frame.
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
        let name = format!("Terminal: {}", tab.host.name);
        if let Some(n) = ui.a11y_node(id, Role::Terminal, &name) {
            n.value = Some(e);
        }
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

    // A program that turned mouse mode on gets the mouse; Shift keeps it
    // for selecting (as in xterm), and a selection already started finishes.
    let (mouse_mode, mouse_enc) =
        session.parser.lock().map(|p| (p.screen().mouse_protocol_mode(), p.screen().mouse_protocol_encoding())).unwrap_or_default();
    let report_mouse = mouse_mode != vt100::MouseProtocolMode::None && !ui.input().mods.shift && !tab.select.dragging;

    // Wheel: scrollback (or arrow keys in full-screen apps like less/vim).
    let (_, dy) = ui.input().wheel();
    if dy != 0.0 && inner.contains(ui.input().mouse) {
        tab.wheel_rest += dy / (WHEEL_PX_PER_LINE * scale);
        let lines = tab.wheel_rest.trunc() as i32;
        tab.wheel_rest -= lines as f32;
        if lines != 0 {
            let alt = session.parser.lock().map(|p| p.screen().alternate_screen()).unwrap_or(false);
            if report_mouse {
                let ev = if lines > 0 { termax_core::termx::MouseEv::WheelUp } else { termax_core::termx::MouseEv::WheelDown };
                let (c, r) = mouse::cell_of(ui.input().mouse, inner, cell);
                let m = ui.input().mods;
                if let Some(b) = termax_core::termx::encode_mouse(mouse_mode, mouse_enc, ev, c, r, m.shift, m.alt, m.ctrl) {
                    for _ in 0..lines.unsigned_abs() {
                        session.write(&b);
                    }
                }
            } else if alt {
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
    if let Some(s) = tab.search.as_mut() {
        search::step_keys(ui, s);
    }
    if focused && ui.keys_for(id) {
        let events = ui.input().events.clone();
        if !alive {
            if ui.input_mut().take_key(&Key::Enter, Modifiers::NONE) {
                if let Err(e) = session.respawn() {
                    tab.error = Some(format!("reconnect failed: {e:#}"));
                }
                tab.reset_blink();
            }
        } else if tab.pending_paste.is_some() {
            // The paste prompt owns Enter / Esc; other keys wait.
            if ui.input_mut().take_key(&Key::Enter, Modifiers::NONE) {
                tab.paste_answer(true);
            } else if ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) {
                tab.paste_answer(false);
            }
        } else {
            let bytes = input::route(&events, keymap, session.application_cursor());
            if !bytes.is_empty() {
                session.write(&bytes); // also snaps the view back to live output
                tab.reset_blink();
                tab.announcer.typed(&ui.input().text());
            }
        }
    }

    let name = format!("Terminal: {}", tab.host.name);
    let Some(session) = tab.session.as_mut() else {
        if let Some(n) = ui.a11y_node(id, Role::Terminal, &name) {
            n.value = tab.error.clone();
        }
        return;
    };
    let parser = session.parser.clone();
    if !ui.a11y_on() {
        tab.resync = true;
    } else {
        let text = parser.lock().map(|p| p.screen().contents()).unwrap_or_default();
        let speak = match tab.announce {
            termax_core::config::Announce::All => true,
            termax_core::config::Announce::Focused => focused,
            termax_core::config::Announce::Off => false,
        };
        let quiet = std::mem::take(&mut tab.resync);
        if speak && !quiet && text != tab.last_screen {
            tab.announcer.push_output(&announce::new_output(&tab.last_screen, &text));
        }
        tab.last_screen.clone_from(&text);
        if let Some(s) = tab.announcer.take(Instant::now()) {
            tab.live = s;
        }
        if tab.announcer.has_pending() {
            ui.request_repaint_in(announce::EVERY);
        }
        if let Some(n) = ui.a11y_node(id, Role::Terminal, &name) {
            n.value = Some(text);
        }
        if let Some(n) = ui.a11y_node(id.with("live"), Role::Status, &tab.live) {
            n.live = true;
        }
    }
    let offset = parser.lock().map(|p| p.screen().scrollback()).unwrap_or(0);
    if report_mouse {
        let events = ui.input().events.clone();
        let mods = ui.input().mods;
        for b in mouse::reports(&events, inner, cell, mouse_mode, mouse_enc, mods, &mut tab.mouse) {
            if let Some(s) = tab.session.as_mut() {
                s.write(&b);
            }
        }
    } else if handle_mouse(ui, id, inner, cell, &mut tab.select, offset)
        && let Ok(p) = parser.lock()
    {
        tab.select.resolve(p.screen(), offset);
    }

    // Right-click menu: Copy / Paste.
    let menu = id.with("menu");
    if r.right_clicked && !report_mouse {
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
    } else if !tab.blink {
        Some(CursorDraw::Block)
    } else {
        let el = tab.blink_from.elapsed().as_millis();
        let left = BLINK.as_millis() - el % BLINK.as_millis();
        ui.request_repaint_in(Duration::from_millis(left as u64));
        ((el / BLINK.as_millis()) % 2 == 0).then_some(CursorDraw::Block)
    };
    ui.painter().rect(rect, theme.term_bg, 0.0);
    if let Ok(mut p) = parser.lock() {
        let rows = p.screen().size().0 as usize;
        let marks = tab.search.as_ref().map(|s| s.marks(offset, rows)).unwrap_or_default();
        draw_screen(ui, inner, p.screen(), theme, cell, px, cursor, tab.select.sel.as_ref(), &marks);
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
    find_bar(ui, id, inner, tab, &parser);
    paste_bar(ui, id, inner, tab);
}

/// "Paste n lines into host?" along the bottom of the terminal.
fn paste_bar(ui: &mut Ui, id: Id, inner: Rect, tab: &mut TermTab) {
    let Some((_, n)) = tab.pending_paste.as_ref() else { return };
    let msg = format!("Paste {n} lines into {}?", tab.host.name);
    let h = ui.px(36);
    let rect = Rect::new(inner.x, inner.bottom() - h, inner.w, h);
    let mut answer = None;
    ui.a11y_group(id.with("paste"), Role::Dialog, &msg, |ui| {
        let p = ui.style().palette;
        ui.interact(id.with("paste-panel"), rect, Sense::hover());
        let (radius, bw) = (ui.px(6) as f32, ui.px(1).max(1));
        ui.painter().rect(rect, p.surface_alt, radius);
        ui.painter().border(rect, p.warning, bw, radius);
        let parts = ui.split_h(rect.inset(ui.px(4)), &[L::Fill, L::Px(80), L::Px(80)], 6);
        label(ui, parts[0], &msg, Some(p.fg));
        if button(ui, id.with("paste-yes"), parts[1], "Paste").clicked {
            answer = Some(true);
        }
        if button(ui, id.with("paste-no"), parts[2], "Cancel").clicked {
            answer = Some(false);
        }
    });
    if let Some(yes) = answer {
        tab.paste_answer(yes);
        ui.request_focus(id);
    }
}

/// The find bar over the top-right of the terminal, while search is open:
/// re-runs the search as the query or output changes and scrolls to the
/// current match.
fn find_bar(ui: &mut Ui, id: Id, inner: Rect, tab: &mut TermTab, parser: &std::sync::Arc<std::sync::Mutex<vt100::Parser>>) {
    let Some(s) = tab.search.as_mut() else { return };
    let w = ui.px(360).min(inner.w);
    let h = ui.px(32);
    let rect = Rect::new(inner.right() - w, inner.y, w, h);
    let (changed, out) = search::bar(ui, id.with("find"), rect, s);
    let generation = tab.session.as_ref().map_or(0, |s| s.generation());
    let Ok(mut p) = parser.lock() else { return };
    if out.close {
        tab.search = None;
        p.screen_mut().set_scrollback(0);
        ui.request_focus(id);
        return;
    }
    s.update(&mut p, generation, changed);
    if std::mem::take(&mut s.scroll_due) {
        if let Some(off) = s.offset() {
            // Keep the match a few rows below the top when there is room.
            let rows = p.screen().size().0 as usize;
            p.screen_mut().set_scrollback(off.saturating_sub(rows / 3));
        }
        ui.request_repaint_in(Duration::ZERO);
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
