//! Saved commands (`[[snippet]]`): the list (with Add in its header) and
//! the editor (name, command lines, host, run) with a primary Save and
//! Delete in its header. Delete and discarding unsaved edits ask in a
//! dialog. Snippets are sent to the focused terminal from the command palette.

use skiff_core::termx::Snippet;
use skiff_ui::geom::Rect;
use skiff_ui::icons::Icon;
use skiff_ui::input::{Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{
    ButtonKind, ButtonOpts, EmptyState, TextInputOpts, TextOpts, ToastKind, Tone, badge, badge_width, button_width, button_with, checkbox,
    empty_state, icon_button, list_named, text, text_input,
};

use super::{
    LIST_W, PendingDiscard, Rows, delete_prompt, discard_prompt, error_line, field_ph, labeled_dropdown, list_header, row_label, scrolled,
    split_row,
};
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
    /// Asking "Delete snippet …?".
    confirm_delete: bool,
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
        self.confirm_delete = false;
        self.confirm_discard = None;
    }

    /// Opens an empty form for a new snippet and focuses its Name field.
    fn new_form(&mut self, ui: &mut Ui) {
        self.form = Some(SnippetForm::new());
        self.error = None;
        self.confirm_delete = false;
        self.confirm_discard = None;
        ui.request_focus(Id::new("settings-snip-name"));
    }

    /// Opens row `to` (`None`: a new snippet), asking first when the form
    /// has unsaved edits.
    pub fn open(&mut self, ui: &mut Ui, to: Option<usize>, app: &GuiApp) {
        if self.form.as_ref().is_some_and(|f| f.dirty) {
            self.confirm_discard = Some(PendingDiscard::new(to));
            self.confirm_delete = false;
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

/// Draws the Snippets page into `rect`: the list on the left; on the
/// right the editor or an empty state. Saves and deletes queue a notice in
/// `notices`. Returns the height used (all of it: the editor scrolls on its own).
pub fn view(ui: &mut Ui, rect: Rect, st: &mut SnippetsState, app: &mut GuiApp, notices: &mut Vec<(ToastKind, String)>) -> i32 {
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

    let cols = ui.split_h(rect, &[L::Px(LIST_W), L::Fill], tokens::SPACE_XL);
    let hh = ui.px(tokens::HEADER_H);
    let head = Rect::new(cols[0].x, cols[0].y, cols[0].w, hh);
    if list_header(ui, Id::new("settings-snip-add"), head, "Snippets", app.cfg.snippets.len()) {
        st.open(ui, None, app);
    }
    let list_r = Rect::new(
        cols[0].x,
        head.bottom() + ui.px(tokens::SPACE_XS),
        cols[0].w,
        cols[0].bottom() - head.bottom() - ui.px(tokens::SPACE_XS),
    );
    // Host-specific snippets say so (a badge, and to screen readers).
    let rows_: Vec<(String, Option<String>)> = app.cfg.snippets.iter().map(|s| (s.name.clone(), s.host.clone())).collect();
    let row_name = |i: usize| match &rows_[i].1 {
        Some(h) => format!("{}, only on {h}", rows_[i].0),
        None => rows_[i].0.clone(),
    };
    let before = st.selected;
    let list_id = Id::new("settings-snip-list");
    let pad = ui.px(tokens::SPACE_SM);
    list_named(ui, list_id, list_r, "Snippets", rows_.len(), tokens::ROW_H, &mut st.selected, row_name, |ui, r, i, _| {
        let host = rows_[i].1.as_deref();
        let bw = host.map_or(0, |h| badge_width(ui, h).min(r.w / 2));
        let tr = Rect::new(r.x + pad + ui.px(tokens::SPACE_XS), r.y, r.w - 2 * pad - bw - ui.px(tokens::SPACE_SM), r.h);
        text(ui, tr, &rows_[i].0, TextOpts::default());
        if let Some(h) = host {
            let bh = ui.px(18);
            badge(ui, Rect::new(r.right() - pad - bw, r.y + (r.h - bh) / 2, bw, bh), h, Tone::Neutral);
        }
    });
    // Clicking a snippet asks to switch (which may ask about unsaved edits first).
    if st.selected != before
        && let Some(i) = st.selected
    {
        st.selected = before; // until the switch happens
        st.open(ui, Some(i), app);
    }
    let del_key = ui.has_focus(list_id) && ui.input_mut().take_key(&Key::Delete, Modifiers::NONE);
    if del_key && st.selected.is_some() {
        st.confirm_delete = true;
    }
    dialogs(ui, st, app, notices);
    st.sync_line_ids();
    let right = cols[1];
    if st.form.is_none() {
        let e = EmptyState {
            icon: Some(Icon::Terminal),
            title: "No snippet selected",
            body: "Snippets are saved commands you send to a terminal from the command palette (type # there to list them).",
            action: Some("Add snippet"),
            ..Default::default()
        };
        if empty_state(ui, Id::new("settings-snip-empty"), Rect::new(right.x, right.y, right.w, right.h.min(ui.px(420))), &e).action {
            st.open(ui, None, app);
        }
        return rect.h;
    }
    scrolled(ui, Id::new("settings-snip-editor"), right, |ui, r| editor(ui, r, st, app, notices));
    rect.h
}

/// The delete and discard dialogs, while one is open.
fn dialogs(ui: &mut Ui, st: &mut SnippetsState, app: &mut GuiApp, notices: &mut Vec<(ToastKind, String)>) {
    let list_id = Id::new("settings-snip-list");
    if let Some(p) = &st.confirm_discard {
        let what = st.form.as_ref().and_then(|f| f.editing.clone()).unwrap_or_else(|| "the new snippet".to_string());
        let to = p.to;
        match discard_prompt(ui, "settings-snip", &what) {
            Some(true) => {
                st.form = None;
                st.confirm_discard = None;
                st.open(ui, to, app);
                if to.is_some() {
                    ui.request_focus(list_id);
                }
            }
            Some(false) => st.confirm_discard = None,
            None => {}
        }
        return;
    }
    let (true, Some(i)) = (st.confirm_delete, st.selected) else {
        st.confirm_delete = false;
        return;
    };
    let name = app.cfg.snippets[i].name.clone();
    let title = format!("Delete snippet {name}?");
    match delete_prompt(ui, Id::new("settings-snip-delete"), &title, "It is removed from config.toml. This cannot be undone.") {
        Some(true) => {
            app.cfg.snippets.retain(|s| s.name != name);
            app.save_config();
            notices.push((ToastKind::Success, format!("Deleted snippet {name}")));
            st.form = None;
            st.selected = None;
            st.confirm_delete = false;
            ui.request_focus(list_id);
        }
        Some(false) => st.confirm_delete = false,
        None => {}
    }
}

/// The snippet editor in `rect` (content coordinates): title with Delete,
/// name, command lines, host, run, and Save. Returns the height used.
fn editor(ui: &mut Ui, rect: Rect, st: &mut SnippetsState, app: &mut GuiApp, notices: &mut Vec<(ToastKind, String)>) -> i32 {
    let mut rows = Rows::new(rect);
    let Some(form) = &mut st.form else { return 0 };
    let head = rows.next(ui);
    let mut x = head.right();
    if form.editing.is_some() {
        let w = button_width(ui, "Delete", 1);
        x -= w;
        let opts = ButtonOpts { kind: ButtonKind::Danger, icon: Some(Icon::Trash), ..Default::default() };
        if button_with(ui, Id::new("settings-snip-del"), Rect::new(x, head.y, w, head.h), "Delete", opts).clicked && st.selected.is_some() {
            st.confirm_delete = true;
        }
    }
    let title = match &form.editing {
        Some(n) => format!("Edit {n}"),
        None => "New snippet".to_string(),
    };
    let tr = Rect::new(head.x, head.y, (x - head.x - ui.px(tokens::SPACE_SM)).max(0), head.h);
    text(ui, tr, &title, TextOpts { role: Some(TextRole::Title), ..Default::default() });
    rows.gap(ui, tokens::SPACE_SM);
    form.dirty |= field_ph(ui, Id::new("settings-snip-name"), rows.next(ui), "Name", &mut form.name, "e.g. tail logs");
    let mut remove = None;
    let n = form.lines.len();
    // One input per command line, each with a remove button once there are two or more.
    for (i, (line, &key)) in form.lines.iter_mut().zip(&st.line_ids).enumerate() {
        let (l, c) = split_row(ui, rows.next(ui));
        if i == 0 {
            row_label(ui, l, "Command");
        }
        let parts = ui.split_h(c, &[L::Fill, L::Px(if n > 1 { tokens::CONTROL_H } else { 0 })], if n > 1 { tokens::SPACE_XS } else { 0 });
        let name = if n == 1 { "Command".to_string() } else { format!("Command line {}", i + 1) };
        let opts =
            TextInputOpts { label: &name, placeholder: if i == 0 { "e.g. tail -f /var/log/syslog" } else { "" }, ..Default::default() };
        form.dirty |= text_input(ui, Id::new(("settings-snip-line", key)), parts[0], line, opts).changed;
        if n > 1 {
            let label = format!("Remove line {}", i + 1);
            if icon_button(ui, Id::new(("settings-snip-line-del", key)), parts[1], Icon::Close, &label, ButtonKind::Ghost).clicked {
                remove = Some(i);
            }
        }
    }
    if let Some(i) = remove {
        form.lines.remove(i);
        st.line_ids.remove(i);
        form.dirty = true;
    }
    let (_, c) = split_row(ui, rows.next(ui));
    let w = button_width(ui, "Add line", 1);
    let opts = ButtonOpts { kind: ButtonKind::Ghost, icon: Some(Icon::Plus), ..Default::default() };
    if button_with(ui, Id::new("settings-snip-line-add"), Rect::new(c.x, c.y, w, c.h), "Add line", opts).clicked {
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
    let row = rows.next(ui);
    let row = Rect::new(row.x, row.y, row.w.min(ui.px(super::LABEL_W + 280)), row.h);
    if labeled_dropdown(ui, Id::new("settings-snip-host"), row, "Host", &opts, &mut sel) {
        form.host = sel.checked_sub(1).map(|i| hosts[i].clone());
        form.dirty = true;
    }
    let (_, c) = split_row(ui, rows.next(ui));
    form.dirty |= checkbox(ui, Id::new("settings-snip-run"), c, &mut form.run, "Run it (press Enter after the command)").clicked;
    rows.gap(ui, tokens::SPACE_SM);
    let (_, c) = split_row(ui, rows.next(ui));
    if button_with(ui, Id::new("settings-snip-save"), Rect::new(c.x, c.y, ui.px(100), c.h), "Save", ButtonOpts::kind(ButtonKind::Primary))
        .clicked
    {
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
                notices.push((ToastKind::Success, format!("Saved snippet {name}")));
                if let Some(s) = app.cfg.snippets.iter().find(|s| s.name == name).cloned() {
                    st.select(&s);
                }
            }
            Err(e) => st.error = Some(e),
        }
    }
    if st.error.is_some() {
        let (_, c) = split_row(ui, rows.next(ui));
        error_line(ui, c, &st.error);
    }
    rows.used()
}
