//! Hosts from both sources (skiff connections and ssh config files): the
//! host list (with Add in its header) and the host editor, grouped into
//! Basic and Advanced, with a primary Save and Duplicate / Delete in its
//! header. ssh config hosts are edited in place (with a backup); hosts
//! that can't be edited safely say why. Delete and discarding unsaved
//! edits ask in a dialog.

use skiff_core::hostform::{self, Dest, Editing, Field, HostForm};
use skiff_core::hosts::Source;
use skiff_core::sshconfig::edit::{ConfigSet, display_path};
use skiff_ui::geom::Rect;
use skiff_ui::icons::Icon;
use skiff_ui::layout::Size as L;
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{
    ButtonKind, ButtonOpts, EmptyState, TextInputOpts, TextOpts, ToastKind, Tone, badge, badge_width, button_width, button_with, checkbox,
    dropdown_labeled, empty_state, empty_state_height, icon_button, list_named, paragraph, paragraph_height, text, text_input,
};

use super::{PendingDiscard, Rows, card, delete_prompt, discard_prompt, error_line, field_ph, list_header, row_label, scrolled, split_row};
use crate::app::GuiApp;

/// The host form being edited.
pub struct Form {
    /// The editable host fields.
    pub form: HostForm,
    /// The ssh config files as loaded when the form opened.
    pub set: ConfigSet,
    /// Name of the host being edited (as saved); `None` for a new one.
    /// Looked up on use: the list can change under an open form.
    pub editing: Option<String>,
    /// "Other options", one per line.
    pub other: Vec<String>,
    /// Edited by the user since it was opened or saved (a new, untouched
    /// form is not: leaving it loses nothing).
    pub dirty: bool,
    /// `GuiApp::hosts_gen` the form was loaded at.
    generation: u64,
}

/// The Connections page's state.
#[derive(Default)]
pub struct ConnState {
    /// The open form, if any.
    pub form: Option<Form>,
    /// A host that can't be edited here: (name, why).
    pub readonly: Option<(String, String)>,
    /// Why the last save, duplicate or delete failed.
    pub error: Option<String>,
    /// The delete question is showing.
    pub confirm_delete: bool,
    /// A switch to another host (`None`: a new one) waiting for "Discard
    /// unsaved changes?".
    confirm_discard: Option<PendingDiscard<Option<String>>>,
    /// The list's highlighted row.
    selected: Option<usize>,
}

impl ConnState {
    /// Opens `form` in the editor and clears any read-only notice, error or pending question.
    fn load(&mut self, form: HostForm, set: ConfigSet, editing: Option<String>, app: &GuiApp, dirty: bool) {
        let other = form.other.lines().map(String::from).collect();
        self.form = Some(Form { form, set, editing, other, dirty, generation: app.hosts_gen });
        self.readonly = None;
        self.error = None;
        self.confirm_delete = false;
        self.confirm_discard = None;
    }

    /// Opens host `to` (`None`: a new host), asking first when the form
    /// has unsaved edits.
    pub fn switch(&mut self, to: Option<String>, app: &mut GuiApp) {
        if self.form.as_ref().is_some_and(|f| f.dirty) {
            self.confirm_discard = Some(PendingDiscard::new(to));
            self.confirm_delete = false;
            return;
        }
        self.confirm_discard = None;
        match to {
            Some(name) => self.edit_host(&name, app),
            None => self.new_form(app),
        }
    }

    /// Opens host `name` for editing (or shows why it can't be).
    pub fn edit_host(&mut self, name: &str, app: &mut GuiApp) {
        self.selected = app.hosts.iter().position(|h| h.name == name);
        let Some(h) = app.hosts.iter().find(|h| h.name == name).cloned() else {
            self.form = None;
            return;
        };
        let set = app.load_set();
        match HostForm::for_entry(&h, &app.cfg, &set) {
            Ok(form) => self.load(form, set, Some(name.to_string()), app, false),
            Err(why) => {
                self.form = None;
                self.readonly = Some((name.to_string(), why));
                self.confirm_delete = false;
            }
        }
    }

    /// Opens an empty form for a new host, saving to the first ssh config file
    /// if there is one, else to the skiff config.
    pub fn new_form(&mut self, app: &mut GuiApp) {
        let set = app.load_set();
        let dest = if set.docs.is_empty() { Dest::Skiff } else { Dest::SshConfig(0) };
        self.load(HostForm::new_host(dest), set, None, app, false);
        self.selected = None;
    }
}

/// What a field's input shows while empty.
fn placeholder(f: Field) -> &'static str {
    match f {
        Field::Name => "e.g. web-prod",
        Field::User => "your login name",
        Field::Port => "22",
        Field::Identity => "~/.ssh/id_ed25519",
        f => f.hint(),
    }
}

/// Draws the Connections page into `rect`: the host list on the left; on
/// the right the editor, a read-only notice, or an empty state. Saves and
/// deletes queue a notice in `notices`. Returns the height used (all of it:
/// the editor scrolls on its own).
pub fn view(ui: &mut Ui, rect: Rect, st: &mut ConnState, app: &mut GuiApp, notices: &mut Vec<(ToastKind, String)>) -> i32 {
    // An untouched form follows the host list (outside edits, sidebar
    // changes); an untouched new one is just reloaded.
    if let Some(f) = &st.form
        && !f.dirty
        && f.generation != app.hosts_gen
    {
        match f.editing.clone() {
            Some(name) => st.edit_host(&name, app),
            None => st.new_form(app),
        }
    }
    // Keep the list highlight on the open (or read-only) host as the list changes.
    if let Some(name) = st.form.as_ref().and_then(|f| f.editing.clone()).or_else(|| st.readonly.as_ref().map(|r| r.0.clone())) {
        st.selected = app.hosts.iter().position(|h| h.name == name);
    }
    let p = ui.style().palette;
    let (list_col, editor_col) = super::list_and_editor(ui, rect, app.hosts.len());
    let cols = [list_col, editor_col];
    let hh = ui.px(tokens::HEADER_H);
    let head = Rect::new(cols[0].x, cols[0].y, cols[0].w, hh);
    if list_header(ui, Id::new("settings-conn-add"), head, "Hosts", app.hosts.len()) {
        st.switch(None, app);
    }
    let list_r = Rect::new(
        cols[0].x,
        head.bottom() + ui.px(tokens::SPACE_XS),
        cols[0].w,
        cols[0].bottom() - head.bottom() - ui.px(tokens::SPACE_XS),
    );
    let names: Vec<(String, bool)> = app.hosts.iter().map(|h| (h.name.clone(), h.source == Source::SshConfig)).collect();
    let before = st.selected;
    // ssh-config hosts carry an "ssh" badge (and say so to screen readers).
    let row_name = |i: usize| if names[i].1 { format!("{}, ssh config", names[i].0) } else { names[i].0.clone() };
    let pad = ui.px(tokens::SPACE_SM);
    list_named(
        ui,
        Id::new("settings-conn-list"),
        list_r,
        "Connections",
        names.len(),
        tokens::ROW_H,
        &mut st.selected,
        row_name,
        |ui, r, i, _| {
            let bw = if names[i].1 { badge_width(ui, "ssh") } else { 0 };
            let tr = Rect::new(r.x + pad + ui.px(tokens::SPACE_XS), r.y, r.w - 2 * pad - bw - ui.px(tokens::SPACE_SM), r.h);
            text(ui, tr, &names[i].0, TextOpts::default());
            if bw > 0 {
                let bh = ui.px(18);
                badge(ui, Rect::new(r.right() - pad - bw, r.y + (r.h - bh) / 2, bw, bh), "ssh", Tone::Neutral);
            }
        },
    );
    // Clicking a host asks to switch (which may ask about unsaved edits first).
    if st.selected != before
        && let Some(i) = st.selected
    {
        st.selected = before; // until the switch happens
        st.switch(Some(names[i].0.clone()), app);
    }

    let right = cols[1];
    let current = st.form.as_ref().and_then(|f| f.editing.clone()).and_then(|n| app.hosts.iter().position(|h| h.name == n));
    dialogs(ui, st, app, current, notices);
    if let Some((name, why)) = &st.readonly {
        let body = "Skiff leaves this host as it is; edit the file in a text editor.";
        let w = right.w - 2 * ui.px(tokens::SPACE_LG);
        let h = ui.px(tokens::SPACE_LG) * 2
            + ui.px(28)
            + paragraph_height(ui, why, TextRole::Body, w)
            + ui.px(tokens::SPACE_SM)
            + paragraph_height(ui, body, TextRole::Body, w);
        let c = Rect::new(right.x, right.y, right.w, h);
        card(ui, c);
        let inner = c.inset(ui.px(tokens::SPACE_LG));
        let s = ui.px(tokens::ICON);
        let iy = inner.y + (ui.px(28) - s) / 2;
        ui.painter().icon(Rect::new(inner.x, iy, s, s), Icon::Warning, p.warning);
        let tr = Rect::new(inner.x + s + pad, inner.y, inner.w - s - pad, ui.px(28));
        text(ui, tr, name, TextOpts { role: Some(TextRole::Title), ..Default::default() });
        let mut y = inner.y + ui.px(28);
        y += paragraph(ui, Rect::new(inner.x, y, inner.w, inner.bottom() - y), why, TextRole::Body, p.fg, skiff_ui::widgets::Align::Start);
        y += ui.px(tokens::SPACE_SM);
        paragraph(ui, Rect::new(inner.x, y, inner.w, inner.bottom() - y), body, TextRole::Body, p.muted, skiff_ui::widgets::Align::Start);
        return rect.h;
    }
    if st.form.is_none() {
        let e = EmptyState {
            icon: Some(Icon::Host),
            title: "No host selected",
            body: "Pick a host in the list to edit it, or add a new one.",
            action: Some("Add host"),
            ..Default::default()
        };
        // At least as tall as it needs (stacked under the list it may be short).
        let eh = right.h.min(ui.px(420)).max(empty_state_height(ui, right.w, &e));
        if empty_state(ui, Id::new("settings-conn-empty"), Rect::new(right.x, right.y, right.w, eh), &e).action {
            st.switch(None, app);
        }
        if st.form.is_none() {
            error_line(ui, Rect::new(right.x, right.y, right.w, ui.px(tokens::CONTROL_H)), &st.error);
        }
        return rect.h;
    }
    scrolled(ui, Id::new("settings-conn-editor"), right, |ui, r| editor(ui, r, st, app, current, notices));
    rect.h
}

/// The delete and discard dialogs, while one is open.
fn dialogs(ui: &mut Ui, st: &mut ConnState, app: &mut GuiApp, current: Option<usize>, notices: &mut Vec<(ToastKind, String)>) {
    if let Some(p) = &st.confirm_discard {
        let what = st.form.as_ref().and_then(|f| f.editing.clone()).unwrap_or_else(|| "the new host".to_string());
        let to = p.to.clone();
        match discard_prompt(ui, "settings-conn", &what) {
            Some(true) => {
                st.form = None;
                st.confirm_discard = None;
                st.switch(to, app);
            }
            Some(false) => st.confirm_discard = None,
            None => {}
        }
        return;
    }
    if !st.confirm_delete {
        return;
    }
    let Some(i) = current else {
        st.confirm_delete = false;
        return;
    };
    let h = &app.hosts[i];
    let title = format!("Delete {}?", h.name);
    let body = match &h.source_file {
        Some(f) => format!("It is removed from {}. A backup of the file is kept.", display_path(f)),
        None => "It is removed from Skiff's config, with its saved forwards left as they are.".to_string(),
    };
    match delete_prompt(ui, Id::new("settings-conn-delete"), &title, &body) {
        Some(true) => {
            let name = app.hosts[i].name.clone();
            match app.delete_host(&name) {
                Ok(()) => notices.push((ToastKind::Success, format!("Deleted {name}"))),
                Err(e) => st.error = Some(e),
            }
            st.form = None;
            st.selected = None;
            st.confirm_delete = false;
        }
        Some(false) => st.confirm_delete = false,
        None => {}
    }
}

/// The host editor in `rect` (content coordinates): title with Duplicate
/// and Delete, the Basic and Advanced groups, and Save. Returns the height used.
fn editor(
    ui: &mut Ui,
    rect: Rect,
    st: &mut ConnState,
    app: &mut GuiApp,
    current: Option<usize>,
    notices: &mut Vec<(ToastKind, String)>,
) -> i32 {
    let Some(form) = &mut st.form else { return 0 };
    let mut rows = Rows::new(rect);
    let head = rows.next(ui);
    let title = match &form.editing {
        Some(n) => format!("Edit {n}"),
        None => "New host".to_string(),
    };
    let mut x = head.right();
    let mut dup = false;
    let mut del = false;
    if form.editing.is_some() {
        let w = button_width(ui, "Delete", 1);
        x -= w;
        let opts = ButtonOpts { kind: ButtonKind::Danger, icon: Some(Icon::Trash), ..Default::default() };
        del = button_with(ui, Id::new("settings-conn-del"), Rect::new(x, head.y, w, head.h), "Delete", opts).clicked;
        let w = button_width(ui, "Duplicate", 1);
        x -= w + ui.px(tokens::SPACE_SM);
        let opts = ButtonOpts { kind: ButtonKind::Ghost, icon: Some(Icon::Copy), ..Default::default() };
        dup = button_with(ui, Id::new("settings-conn-dup"), Rect::new(x, head.y, w, head.h), "Duplicate", opts).clicked;
    }
    let tr = Rect::new(head.x, head.y, (x - head.x - ui.px(tokens::SPACE_SM)).max(0), head.h);
    text(ui, tr, &title, TextOpts { role: Some(TextRole::Title), ..Default::default() });

    rows.section(ui, "Basic");
    // "Save to": skiff config or one of the ssh config files.
    if form.form.can_change_dest() && !form.set.docs.is_empty() {
        let (l, c) = split_row(ui, rows.next(ui));
        row_label(ui, l, "Save to");
        let mut opts: Vec<String> = vec!["Skiff config".into()];
        opts.extend(form.set.docs.iter().map(|d| display_path(&d.path)));
        let o: Vec<&str> = opts.iter().map(String::as_str).collect();
        let mut sel = match form.form.dest {
            Dest::Skiff => 0,
            Dest::SshConfig(i) => i + 1,
        };
        if dropdown_labeled(ui, Id::new("settings-conn-dest"), c, "Save to", &o, &mut sel) {
            form.form.dest = if sel == 0 { Dest::Skiff } else { Dest::SshConfig(sel - 1) };
            form.dirty = true;
        }
    }
    let basic = [Field::Name, Field::Host, Field::User, Field::Port];
    let fields = form.form.fields();
    for fl in fields.iter().copied().filter(|f| basic.contains(f)) {
        let id = Id::new(("settings-conn-field", fl as usize));
        form.dirty |= field_ph(ui, id, rows.next(ui), fl.label(), form.form.get_mut(fl), placeholder(fl));
    }
    rows.section(ui, "Advanced");
    for fl in fields.iter().copied().filter(|f| !basic.contains(f)) {
        let id = Id::new(("settings-conn-field", fl as usize));
        form.dirty |= field_ph(ui, id, rows.next(ui), fl.label(), form.form.get_mut(fl), placeholder(fl));
    }
    let (_, c) = split_row(ui, rows.next(ui));
    if checkbox(ui, Id::new("settings-conn-hidden"), c, &mut form.form.hidden, "Hidden (left out of host lists)").clicked {
        form.dirty = true;
    }
    // Free-form "Other options" lines, only for ssh config hosts.
    if matches!(form.form.dest, Dest::SshConfig(_)) {
        let mut remove = None;
        for (i, line) in form.other.iter_mut().enumerate() {
            let (l, c) = split_row(ui, rows.next(ui));
            if i == 0 {
                row_label(ui, l, "Other options");
            }
            let parts = ui.split_h(c, &[L::Fill, L::Px(tokens::CONTROL_H)], tokens::SPACE_XS);
            let opts = TextInputOpts { placeholder: "e.g. ForwardAgent yes", label: "Other option", ..Default::default() };
            form.dirty |= text_input(ui, Id::new(("settings-conn-other", i)), parts[0], line, opts).changed;
            let name = format!("Remove option {}", i + 1);
            if icon_button(ui, Id::new(("settings-conn-other-del", i)), parts[1], Icon::Close, &name, ButtonKind::Ghost).clicked {
                remove = Some(i);
            }
        }
        if let Some(i) = remove {
            form.other.remove(i);
            form.dirty = true;
        }
        let (l, c) = split_row(ui, rows.next(ui));
        if form.other.is_empty() {
            row_label(ui, l, "Other options");
        }
        let w = button_width(ui, "Add option", 1);
        let opts = ButtonOpts { kind: ButtonKind::Ghost, icon: Some(Icon::Plus), ..Default::default() };
        if button_with(ui, Id::new("settings-conn-other-add"), Rect::new(c.x, c.y, w, c.h), "Add option", opts).clicked {
            form.other.push(String::new());
            form.dirty = true;
        }
    }
    rows.gap(ui, tokens::SPACE_SM);
    let (_, c) = split_row(ui, rows.next(ui));
    let save =
        button_with(ui, Id::new("settings-conn-save"), Rect::new(c.x, c.y, ui.px(100), c.h), "Save", ButtonOpts::kind(ButtonKind::Primary))
            .clicked;
    if save {
        match save_form(form, app) {
            Ok(name) => {
                notices.push((ToastKind::Success, format!("Saved {name}")));
                st.edit_host(&name, app);
            }
            Err(e) => st.error = Some(e),
        }
    }
    if st.error.is_some() {
        let (_, c) = split_row(ui, rows.next(ui));
        error_line(ui, c, &st.error);
    }
    if dup && let Some(i) = current {
        match app.duplicate_host(&app.hosts[i].name.clone()) {
            Ok(name) => st.switch(Some(name), app),
            Err(e) => st.error = Some(e),
        }
    }
    if del && current.is_some() {
        st.confirm_delete = true;
    }
    rows.used()
}

/// Saves the form to the skiff config or its ssh config file and applies it;
/// returns the saved name. On error nothing is lost and it can be saved again.
fn save_form(form: &mut Form, app: &mut GuiApp) -> Result<String, String> {
    form.form.other = form.other.join("\n");
    // Where the edited connection is now; one deleted meanwhile is saved as new.
    if let (Editing::Custom(_), Some(name)) = (&form.form.editing, &form.editing) {
        form.form.editing = match app.cfg.connections.iter().position(|c| &c.name == name) {
            Some(i) => Editing::Custom(i),
            None => Editing::New,
        };
    }
    // Names already in use, and the name before this edit (to detect a rename).
    let taken = hostform::taken_names(&app.hosts);
    let old = form.form.original_name(&app.cfg);
    // Edit a copy: if the write fails, the form can be saved again as is.
    let mut set = form.set.clone();
    let saved = form.form.save(&mut app.cfg, &mut set, &taken)?;
    let name = saved.name().to_string();
    app.apply_saved(saved, &mut set)?;
    if let Some(old) = old {
        // A rename moves the host's forwards, history and status to the new name.
        app.after_rename(&old, &name);
    }
    form.set = set;
    form.dirty = false;
    form.editing = Some(name.clone());
    Ok(name)
}
