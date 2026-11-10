//! The find bar of a terminal tab.

use skiff_core::terminal::TerminalSession;
use skiff_core::termx::TermSearch;
use skiff_ui::a11y::Role;
use skiff_ui::geom::Rect;
use skiff_ui::input::{Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::{TextInputOpts, label, text_input};

/// An open search: the query being typed and what it found.
#[derive(Debug, Default)]
pub struct SearchState {
    pub query: String,
    pub found: TermSearch,
    /// Scroll to the current match on the next frame.
    pub scroll_due: bool,
}

/// What the bar asked for this frame.
#[derive(Debug, Default)]
pub struct BarOut {
    pub close: bool,
}

const SHIFT: Modifiers = Modifiers { ctrl: false, shift: true, alt: false, logo: false };

impl SearchState {
    /// "3 of 12", "No matches", or "" before anything was typed.
    pub fn label(&self) -> String {
        match (self.query.is_empty(), self.found.matches().len()) {
            (true, _) => String::new(),
            (false, 0) => "No matches".into(),
            (false, n) => format!("{} of {n}", self.found.current() + 1),
        }
    }

    /// Catches up with new output and the query; true if anything changed.
    /// Only reading new lines holds the session's lock.
    pub fn update(&mut self, session: &TerminalSession) -> bool {
        let mut changed = self.found.sync(session);
        if self.found.set_query(&self.query) {
            self.scroll_due = true;
            changed = true;
        }
        changed
    }

    /// Next older (`older`) or newer match, wrapping.
    pub fn step(&mut self, older: bool) {
        if self.found.step(older) {
            self.scroll_due = true;
        }
    }
}

/// F3 / Shift+F3: next older / newer match, while the terminal or its find
/// bar has focus.
pub fn step_keys(ui: &mut Ui, s: &mut SearchState) {
    if ui.input_mut().take_key(&Key::F(3), Modifiers::NONE) {
        s.step(true);
    }
    if ui.input_mut().take_key(&Key::F(3), SHIFT) {
        s.step(false);
    }
}

/// Draws the bar in `rect` (over the terminal's top-right corner) and
/// handles its keys. Returns whether the query changed and what was asked.
pub fn bar(ui: &mut Ui, id: Id, rect: Rect, s: &mut SearchState) -> (bool, BarOut) {
    let mut out = BarOut::default();
    let p = ui.style().palette;
    let radius = ui.px(6) as f32;
    ui.interact(id.with("panel"), rect, Sense::hover());
    ui.painter().rect(rect, p.surface_alt, radius);
    let bw = ui.px(1).max(1);
    ui.painter().border(rect, p.control, bw, radius);
    let parts = ui.split_h(rect.inset(ui.px(4)), &[L::Fill, L::Px(96)], 6);
    let input = id.with("input");
    if ui.has_focus(input) {
        if ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) {
            out.close = true;
        }
        if ui.input_mut().take_key(&Key::Enter, SHIFT) {
            s.step(false);
        }
    }
    let r = text_input(
        ui,
        input,
        parts[0],
        &mut s.query,
        TextInputOpts { placeholder: "Find", label: "Find in terminal", ..Default::default() },
    );
    if r.submitted {
        s.step(true);
    }
    let count = s.label();
    label(ui, parts[1], &count, Some(p.muted));
    if let Some(n) = ui.a11y_node(id.with("count"), Role::Status, &count) {
        n.live = true;
    }
    (r.changed, out)
}
