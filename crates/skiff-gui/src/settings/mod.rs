//! The settings tab: connections, appearance, keybindings, snippets, general.
//!
//! Every form saves through `GuiApp::save_config` (comments in
//! `config.toml` survive). A form the user has not edited follows the
//! config, so an outside edit shows up at once; a form with unsaved edits
//! keeps them.

use skiff_ui::geom::Rect;
use skiff_ui::layout::Size as L;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{TextInputOpts, button, dropdown_labeled, label, text_input};

use crate::app::GuiApp;

pub mod appearance;
pub mod connections;
pub mod general;
pub mod keys;
pub mod snippets;

/// Width of the label column of a form row, logical px.
pub const LABEL_W: i32 = 150;
/// Height of a form row, logical px.
pub const ROW_H: i32 = 30;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Connections,
    Appearance,
    Keybindings,
    Snippets,
    General,
}

impl Section {
    const ALL: [Section; 5] = [Section::Connections, Section::Appearance, Section::Keybindings, Section::Snippets, Section::General];

    fn label(self) -> &'static str {
        match self {
            Section::Connections => "Connections",
            Section::Appearance => "Appearance",
            Section::Keybindings => "Keybindings",
            Section::Snippets => "Snippets",
            Section::General => "General",
        }
    }
}

#[derive(Default)]
pub struct SettingsState {
    pub section: Section,
    pub conn: connections::ConnState,
    pub look: appearance::AppearanceState,
    pub keys: keys::KeysState,
    pub snippets: snippets::SnippetsState,
    pub general: general::GeneralState,
}

impl SettingsState {
    /// A keybinding is being recorded: app shortcuts must not fire.
    pub fn capturing(&self) -> bool {
        self.section == Section::Keybindings && self.keys.capturing.is_some()
    }
}

pub fn view(ui: &mut Ui, rect: Rect, app: &mut GuiApp) {
    let mut st = std::mem::take(&mut app.settings);
    let cols = ui.split_h(rect, &[L::Px(170), L::Fill], 0);
    let p = app.theme.palette;
    ui.painter().rect(cols[0], p.surface, 0.0);
    let mut y = cols[0].y + ui.px(12);
    for s in Section::ALL {
        let r = Rect::new(cols[0].x + ui.px(8), y, cols[0].w - ui.px(16), ui.px(ROW_H));
        if s == st.section {
            let bar = Rect::new(r.x - ui.px(6), r.y + ui.px(4), ui.px(3), r.h - ui.px(8));
            ui.painter().rect(bar, p.accent, 0.0);
        }
        if button(ui, Id::new(("settings-nav", s.label())), r, s.label()).clicked && s != st.section {
            st.keys.capturing = None;
            st.section = s;
        }
        y += ui.px(ROW_H + 6);
    }
    let body = cols[1].inset(ui.px(16));
    match st.section {
        Section::Connections => connections::view(ui, body, &mut st.conn, app),
        Section::Appearance => appearance::view(ui, body, &mut st.look, app),
        Section::Keybindings => keys::view(ui, body, &mut st.keys, app),
        Section::Snippets => snippets::view(ui, body, &mut st.snippets, app),
        Section::General => general::view(ui, body, &mut st.general, app),
    }
    app.settings = st;
}

/// Consecutive form rows from the top of `rect`.
pub struct Rows {
    rect: Rect,
    y: i32,
}

impl Rows {
    pub fn new(rect: Rect) -> Rows {
        Rows { rect, y: rect.y }
    }

    pub fn next(&mut self, ui: &Ui) -> Rect {
        self.next_h(ui, ROW_H)
    }

    pub fn next_h(&mut self, ui: &Ui, h: i32) -> Rect {
        let r = Rect::new(self.rect.x, self.y, self.rect.w, ui.px(h));
        self.y += ui.px(h + 6);
        r
    }
}

/// Splits a row into its label and control parts.
pub fn split_row(ui: &Ui, row: Rect) -> (Rect, Rect) {
    let lw = ui.px(LABEL_W).min(row.w);
    (Rect::new(row.x, row.y, lw, row.h), Rect::new(row.x + lw, row.y, (row.w - lw).min(ui.px(460)), row.h))
}

/// A labelled text input; true when edited this frame.
pub fn field(ui: &mut Ui, id: Id, row: Rect, text: &str, value: &mut String) -> bool {
    let (l, c) = split_row(ui, row);
    label(ui, l, text, None);
    text_input(ui, id, c, value, TextInputOpts { label: text, ..Default::default() }).changed
}

/// A labelled dropdown (named by its label for screen readers); true when changed.
pub fn labeled_dropdown(ui: &mut Ui, id: Id, row: Rect, text: &str, options: &[&str], selected: &mut usize) -> bool {
    let (l, c) = split_row(ui, row);
    label(ui, l, text, None);
    dropdown_labeled(ui, id, c, text, options, selected)
}

/// An inline error (or nothing).
pub fn error_line(ui: &mut Ui, row: Rect, err: &Option<String>) {
    if let Some(e) = err {
        let c = ui.style().palette.danger;
        label(ui, row, e, Some(c));
    }
}

/// A switch away from a form with unsaved edits (to `to`), waiting for
/// "Discard …?" to be answered.
pub struct PendingDiscard<T> {
    pub to: T,
    first: bool,
}

impl<T> PendingDiscard<T> {
    pub fn new(to: T) -> Self {
        PendingDiscard { to, first: true }
    }
}

/// Asks "Discard unsaved changes to `what`?" with Discard / Keep editing
/// (ids `<ids>-discard-yes` / `-no`). Focus starts on Keep editing, so a
/// reflexive Enter keeps the edits; Esc keeps them too. `Some(true)`:
/// discard, `Some(false)`: keep; `None` while unanswered.
pub fn discard_prompt<T>(ui: &mut Ui, rows: &mut Rows, ids: &str, what: &str, p: &mut PendingDiscard<T>) -> Option<bool> {
    label(ui, rows.next(ui), &format!("Discard unsaved changes to {what}?"), None);
    let r = rows.next(ui);
    let b = ui.split_h(Rect::new(r.x, r.y, ui.px(260), r.h), &[L::Frac(0.45), L::Frac(0.55)], 8);
    let (yes, no) = (Id::new(format!("{ids}-discard-yes")), Id::new(format!("{ids}-discard-no")));
    if std::mem::take(&mut p.first) {
        ui.request_focus(no);
    }
    let esc =
        (ui.has_focus(yes) || ui.has_focus(no)) && ui.input_mut().take_key(&skiff_ui::input::Key::Escape, skiff_ui::input::Modifiers::NONE);
    if button(ui, yes, b[0], "Discard").clicked {
        Some(true)
    } else if button(ui, no, b[1], "Keep editing").clicked || esc {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
