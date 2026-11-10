//! Saved commands (`[[snippet]]`): list, form, Add / Delete / Save.
//! Snippets are sent to the focused terminal from the quick switcher.

use skiff_core::termx::Snippet;
use skiff_ui::geom::Rect;
use skiff_ui::input::{Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{TextInputOpts, button, checkbox, label, list_named, text_input};

use super::{LABEL_W, PendingDiscard, Rows, discard_prompt, error_line, field, labeled_dropdown, split_row};
use crate::app::GuiApp;

/// The form's text, as typed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnippetForm {
    /// Name of the snippet being edited (as saved); `None` for a new one.
    pub editing: Option<String>,
    /// The snippet's name, as typed.
    pub name: String,
    /// The command, one input per line.
    pub lines: Vec<String>,
    /// Only for this host; `None` = every host.
    pub host: Option<String>,
    /// Press Enter after sending the command.
    pub run: bool,
    /// Edited by the user since it was opened or saved (a new, untouched
    /// form is not: leaving it loses nothing).
    pub dirty: bool,
}

impl SnippetForm {
    /// An empty form for a new snippet, with one empty command line.
    fn new() -> SnippetForm {
        SnippetForm { lines: vec![String::new()], ..Default::default() }
    }

    /// The form for saved snippet `s`, one input per command line.
    fn of(s: &Snippet) -> SnippetForm {
        SnippetForm {
            editing: Some(s.name.clone()),
            name: s.name.clone(),
            lines: if s.command.is_empty() { vec![String::new()] } else { s.command.lines().map(String::from).collect() },
            host: s.host.clone(),
            run: s.run,
            dirty: false,
        }
    }

    /// The snippet to save, or why it can't be. `all` are the saved ones.
    pub fn validate(&self, all: &[Snippet]) -> Result<Snippet, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("A snippet needs a name".into());
        }
        if all.iter().any(|s| s.name == name && self.editing.as_deref() != Some(name)) {
            return Err(format!("A snippet named {name} already exists"));
        }
        let mut lines: Vec<&str> = self.lines.iter().map(String::as_str).collect();
        // Trailing blank lines are dropped; a command of only blank lines is empty.
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        if lines.is_empty() {
            return Err("The command is empty".into());
        }
        Ok(Snippet { name: name.to_string(), command: lines.join("\n"), host: self.host.clone(), run: self.run })
    }
}

/// The Snippets page's state.
#[derive(Default)]
pub struct SnippetsState {
    /// The open form, if any.
    pub form: Option<SnippetForm>,
    /// Why the last save failed.
    pub error: Option<String>,
    /// Asking "Delete snippet …?"; the first frame moves focus to Cancel.
    confirm_delete: Option<bool>,
    /// Another row (or Add, `None`) was picked while the form had unsaved
    /// edits: asking "Discard …?" before switching.
    confirm_discard: Option<PendingDiscard<Option<usize>>>,
    /// The list's highlighted row (follows the open snippet).
    selected: Option<usize>,
    /// A stable id per command line input (so focus and cursor stay with a
    /// line when one above it is removed), parallel to `form.lines`.
    line_ids: Vec<u64>,
    /// Last line id handed out.
    next_line_id: u64,
}

impl SnippetsState {
    /// Opens saved snippet `s` in the form, dropping any error or question.
    fn select(&mut self, s: &Snippet) {
        self.form = Some(SnippetForm::of(s));
        self.error = None;
        self.confirm_delete = None;
        self.confirm_discard = None;
    }

    /// Opens an empty form for a new snippet and focuses its Name field.
    fn new_form(&mut self, ui: &mut Ui) {
        self.form = Some(SnippetForm::new());
        self.error = None;
        self.confirm_delete = None;
        self.confirm_discard = None;
        ui.request_focus(Id::new("settings-snip-name"));
    }

    /// Switches to row `to` (`None`: a new snippet), asking first when the
    /// form has unsaved edits.
    fn switch(&mut self, ui: &mut Ui, to: Option<usize>, app: &GuiApp) {
        if self.form.as_ref().is_some_and(|f| f.dirty) {
            self.confirm_discard = Some(PendingDiscard::new(to));
            self.confirm_delete = None;
            return;
        }
        match to {
            Some(i) => match app.cfg.snippets.get(i).cloned() {
                Some(s) => self.select(&s),
                None => self.confirm_discard = None,
            },
            None => self.new_form(ui),
        }
    }

    /// A fresh id for a command line input.
    fn new_line_id(&mut self) -> u64 {
        self.next_line_id += 1;
        self.next_line_id
    }

    /// Keeps `line_ids` as long as the form's lines (a replaced form with
    /// another line count gets fresh ids).
    fn sync_line_ids(&mut self) {
        let n = self.form.as_ref().map_or(0, |f| f.lines.len());
        if self.line_ids.len() != n {
            self.line_ids.clear();
            for _ in 0..n {
                let id = self.new_line_id();
                self.line_ids.push(id);
            }
        }
    }
}

/// Draws the Snippets page: list with Add / Delete on the left; on the right
/// a discard or delete question, or the form with Save.
pub fn view(ui: &mut Ui, rect: Rect, st: &mut SnippetsState, app: &mut GuiApp) {
    // An untouched form follows the config (outside edits).
    if let Some(f) = &st.form
        && !f.dirty
        && let Some(name) = f.editing.clone()
    {
        match app.cfg.snippets.iter().find(|s| s.name == name) {
            Some(s) if SnippetForm::of(s) != *f => st.form = Some(SnippetForm::of(s)),
            Some(_) => {}
            None => st.form = None,
        }
    }
    let editing = st.form.as_ref().and_then(|f| f.editing.clone());
    st.selected = editing.as_ref().and_then(|n| app.cfg.snippets.iter().position(|s| &s.name == n));

    let cols = ui.split_h(rect, &[L::Px(250), L::Fill], 16);
    let left = ui.split_v(cols[0], &[L::Fill, L::Px(30)], 8);
    // Host-specific snippets say so to screen readers.
    let names: Vec<String> = app.cfg.snippets.iter().map(|s| s.name.clone()).collect();
    let row_name = |i: usize| {
        let s = &app.cfg.snippets[i];
        match &s.host {
            Some(h) => format!("{}, only on {h}", s.name),
            None => s.name.clone(),
        }
    };
    let before = st.selected;
    let list_id = Id::new("settings-snip-list");
    list_named(ui, list_id, left[0], "Snippets", names.len(), 28, &mut st.selected, row_name, |ui, r, i, _| {
        let r = Rect::new(r.x + ui.px(8), r.y, r.w - ui.px(16), r.h);
        label(ui, r, &names[i], None);
    });
    // Clicking a snippet asks to switch (which may ask about unsaved edits first).
    if st.selected != before
        && let Some(i) = st.selected
    {
        st.selected = before; // until the switch happens
        st.switch(ui, Some(i), app);
    }
    let btns = ui.split_h(left[1], &[L::Frac(0.5), L::Frac(0.5)], 4);
    if button(ui, Id::new("settings-snip-add"), btns[0], "Add").clicked {
        st.switch(ui, None, app);
    }
    let del_key = ui.has_focus(list_id) && ui.input_mut().take_key(&Key::Delete, Modifiers::NONE);
    if (button(ui, Id::new("settings-snip-del"), btns[1], "Delete").clicked || del_key) && st.selected.is_some() {
        st.confirm_delete = Some(true);
    }

    // The right side shows, in order of priority: the discard question, the
    // delete question, or the form.
    let mut rows = Rows::new(cols[1]);
    if let Some(p) = &mut st.confirm_discard {
        let what = st.form.as_ref().and_then(|f| f.editing.clone()).unwrap_or_else(|| "the new snippet".to_string());
        match discard_prompt(ui, &mut rows, "settings-snip", &what, p) {
            Some(true) => {
                let to = p.to;
                st.form = None;
                st.switch(ui, to, app);
                if to.is_some() {
                    ui.request_focus(list_id);
                }
            }
            Some(false) => {
                st.confirm_discard = None;
                ui.request_focus(list_id);
            }
            None => {}
        }
        return;
    }
    if let (Some(first), Some(i)) = (st.confirm_delete, st.selected) {
        let name = app.cfg.snippets[i].name.clone();
        label(ui, rows.next(ui), &format!("Delete snippet {name}?"), None);
        let r = rows.next(ui);
        let b = ui.split_h(Rect::new(r.x, r.y, ui.px(240), r.h), &[L::Frac(0.5), L::Frac(0.5)], 8);
        let (yes, no) = (Id::new("settings-snip-del-yes"), Id::new("settings-snip-del-no"));
        if first {
            ui.request_focus(no); // a reflexive Enter keeps it
            st.confirm_delete = Some(false);
        }
        let esc = (ui.has_focus(yes) || ui.has_focus(no)) && ui.input_mut().take_key(&Key::Escape, Modifiers::NONE);
        if button(ui, yes, b[0], "Yes, delete").clicked {
            app.cfg.snippets.retain(|s| s.name != name);
            app.save_config();
            st.form = None;
            st.selected = None;
            st.confirm_delete = None;
            ui.request_focus(list_id);
        } else if button(ui, no, b[1], "Cancel").clicked || esc {
            st.confirm_delete = None;
            ui.request_focus(list_id);
        }
        return;
    }
    st.confirm_delete = None;
    st.sync_line_ids();
    let muted = ui.style().palette.muted;
    let Some(form) = &mut st.form else {
        label(ui, rows.next(ui), "Select a snippet, or Add one. Run them from the quick switcher (Ctrl+Shift+K).", Some(muted));
        error_line(ui, rows.next(ui), &st.error);
        return;
    };
    label(ui, rows.next(ui), if form.editing.is_some() { "Edit snippet" } else { "New snippet" }, None);
    form.dirty |= field(ui, Id::new("settings-snip-name"), rows.next(ui), "Name", &mut form.name);
    let mut remove = None;
    let n = form.lines.len();
    // One input per command line, each with a remove button once there are two or more.
    for (i, (line, &key)) in form.lines.iter_mut().zip(&st.line_ids).enumerate() {
        let (l, c) = split_row(ui, rows.next(ui));
        if i == 0 {
            label(ui, l, "Command", None);
        }
        let parts = ui.split_h(c, &[L::Fill, L::Px(30)], 4);
        let name = if n == 1 { "Command".to_string() } else { format!("Command line {}", i + 1) };
        let opts =
            TextInputOpts { label: &name, placeholder: if i == 0 { "e.g. tail -f /var/log/syslog" } else { "" }, ..Default::default() };
        form.dirty |= text_input(ui, Id::new(("settings-snip-line", key)), parts[0], line, opts).changed;
        let del = Id::new(("settings-snip-line-del", key));
        if n > 1 {
            if button(ui, del, parts[1], "\u{2715}").clicked {
                remove = Some(i);
            }
            if let Some(node) = ui.a11y_node(del, skiff_ui::a11y::Role::Button, "") {
                node.name = format!("Remove line {}", i + 1);
            }
        }
    }
    if let Some(i) = remove {
        form.lines.remove(i);
        st.line_ids.remove(i);
        form.dirty = true;
    }
    let (_, c) = split_row(ui, rows.next(ui));
    if button(ui, Id::new("settings-snip-line-add"), Rect::new(c.x, c.y, ui.px(130), c.h), "+ Add line").clicked {
        form.lines.push(String::new());
        form.dirty = true;
        st.next_line_id += 1;
        st.line_ids.push(st.next_line_id);
        ui.request_focus(Id::new(("settings-snip-line", st.next_line_id)));
    }
    // Host: every host, or one (a host no longer listed stays choosable).
    let mut hosts: Vec<String> = app.hosts.iter().map(|h| h.name.clone()).collect();
    if let Some(h) = &form.host
        && !hosts.contains(h)
    {
        hosts.push(h.clone());
    }
    let mut opts = vec!["Every host"];
    opts.extend(hosts.iter().map(String::as_str));
    let mut sel = form.host.as_ref().and_then(|h| hosts.iter().position(|x| x == h)).map_or(0, |i| i + 1);
    if labeled_dropdown(ui, Id::new("settings-snip-host"), rows.next(ui), "Host", &opts, &mut sel) {
        form.host = sel.checked_sub(1).map(|i| hosts[i].clone());
        form.dirty = true;
    }
    let (_, c) = split_row(ui, rows.next(ui));
    form.dirty |= checkbox(ui, Id::new("settings-snip-run"), c, &mut form.run, "Run it (press Enter after the command)").clicked;
    let r = rows.next(ui);
    if button(ui, Id::new("settings-snip-save"), Rect::new(r.x + ui.px(LABEL_W), r.y, ui.px(100), r.h), "Save").clicked {
        // Replace the snippet being edited (if still there) or add a new one, then
        // reopen it from the saved config.
        match form.validate(&app.cfg.snippets) {
            Ok(s) => {
                let name = s.name.clone();
                match form.editing.as_ref().and_then(|n| app.cfg.snippets.iter().position(|x| &x.name == n)) {
                    Some(i) => app.cfg.snippets[i] = s,
                    None => app.cfg.snippets.push(s),
                }
                app.save_config();
                if let Some(s) = app.cfg.snippets.iter().find(|s| s.name == name).cloned() {
                    st.select(&s);
                }
            }
            Err(e) => st.error = Some(e),
        }
    }
    error_line(ui, rows.next(ui), &st.error);
}
