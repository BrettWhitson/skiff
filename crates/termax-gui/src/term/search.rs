//! The find bar of a terminal tab.

use termax_core::termx::{Match, find, screen_lines};
use termax_ui::a11y::Role;
use termax_ui::geom::Rect;
use termax_ui::input::{Key, Modifiers};
use termax_ui::layout::Size as L;
use termax_ui::ui::{Id, Sense, Ui};
use termax_ui::widgets::{TextInputOpts, label, text_input};

/// An open search: the query, the lines it ran over and where it is.
#[derive(Debug, Default)]
pub struct SearchState {
    pub query: String,
    history: usize,
    pub matches: Vec<Match>,
    /// Index into `matches`.
    pub current: usize,
    /// Output generation the lines were read at.
    generation: Option<u64>,
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
        match (self.query.is_empty(), self.matches.len()) {
            (true, _) => String::new(),
            (false, 0) => "No matches".into(),
            (false, n) => format!("{} of {n}", self.current + 1),
        }
    }

    /// Re-runs the search when the query or the output changed; true if it did.
    pub fn update(&mut self, parser: &mut vt100::Parser, generation: u64, query_changed: bool) -> bool {
        if !query_changed && self.generation == Some(generation) {
            return false;
        }
        let (lines, history) = screen_lines(parser);
        self.history = history;
        self.generation = Some(generation);
        let was = self.matches.get(self.current).copied();
        self.matches = find(&lines, &self.query);
        self.current = match (query_changed, was) {
            // A new query starts at the newest match.
            (true, _) | (false, None) => self.matches.len().saturating_sub(1),
            // New output: stay on the same text (lines only move up with scroll).
            (false, Some(m)) => self.matches.iter().position(|x| *x == m).unwrap_or(self.matches.len().saturating_sub(1)),
        };
        if query_changed {
            self.scroll_due = true;
        }
        true
    }

    /// Next older (`older`) or newer match, wrapping.
    pub fn step(&mut self, older: bool) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        self.current = if older { (self.current + n - 1) % n } else { (self.current + 1) % n };
        self.scroll_due = true;
    }

    /// The scrollback offset that shows the current match.
    pub fn offset(&self) -> Option<usize> {
        let m = self.matches.get(self.current)?;
        Some(termax_core::termx::scroll_offset_for(m.line, self.history, 0))
    }

    /// Visible matches as (screen row, col, len, is_current), for a view
    /// scrolled to `offset` showing `rows` rows.
    pub fn marks(&self, offset: usize, rows: usize) -> Vec<(usize, usize, usize, bool)> {
        let top = self.history.saturating_sub(offset);
        self.matches
            .iter()
            .enumerate()
            .filter(|(_, m)| m.line >= top && m.line < top + rows)
            .map(|(i, m)| (m.line - top, m.col, m.len, i == self.current))
            .collect()
    }
}

/// Takes F3 / Shift+F3 (next older / newer match) wherever focus is in the tab.
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
    let r = text_input(ui, input, parts[0], &mut s.query, TextInputOpts { placeholder: "Find", label: "Find in terminal", ..Default::default() });
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
