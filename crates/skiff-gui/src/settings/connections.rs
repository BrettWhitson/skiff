//! Hosts from both sources (skiff connections and ssh config files): list,
//! form, Add / Duplicate / Delete / Save. ssh config hosts are edited in
//! place (with a backup); hosts that can't be edited safely say why.

use skiff_core::hostform::{self, Dest, Editing, HostForm};
use skiff_core::hosts::Source;
use skiff_core::sshconfig::edit::{ConfigSet, display_path};
use skiff_ui::geom::Rect;
use skiff_ui::layout::Size as L;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{TextInputOpts, button, checkbox, dropdown_labeled, label, list_named, text_input};

use super::{PendingDiscard, Rows, discard_prompt, error_line, field, split_row};
use crate::app::GuiApp;

pub struct Form {
    pub form: HostForm,
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

#[derive(Default)]
pub struct ConnState {
    pub form: Option<Form>,
    /// A host that can't be edited here: (name, why).
    pub readonly: Option<(String, String)>,
    pub error: Option<String>,
    pub confirm_delete: bool,
    /// A switch to another host (`None`: a new one) waiting for "Discard
    /// unsaved changes?".
    confirm_discard: Option<PendingDiscard<Option<String>>>,
    /// The list's highlighted row.
    selected: Option<usize>,
}

impl ConnState {
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

    pub fn new_form(&mut self, app: &mut GuiApp) {
        let set = app.load_set();
        let dest = if set.docs.is_empty() { Dest::Skiff } else { Dest::SshConfig(0) };
        self.load(HostForm::new_host(dest), set, None, app, false);
        self.selected = None;
    }
}

pub fn view(ui: &mut Ui, rect: Rect, st: &mut ConnState, app: &mut GuiApp) {
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
    if let Some(name) = st.form.as_ref().and_then(|f| f.editing.clone()).or_else(|| st.readonly.as_ref().map(|r| r.0.clone())) {
        st.selected = app.hosts.iter().position(|h| h.name == name);
    }

    let cols = ui.split_h(rect, &[L::Px(250), L::Fill], 16);
    let left = ui.split_v(cols[0], &[L::Fill, L::Px(30)], 8);
    let names: Vec<(String, bool)> = app.hosts.iter().map(|h| (h.name.clone(), h.source == Source::SshConfig)).collect();
    let before = st.selected;
    let muted = ui.style().palette.muted;
    let row_name = |i: usize| if names[i].1 { format!("{}, ssh config", names[i].0) } else { names[i].0.clone() };
    list_named(ui, Id::new("settings-conn-list"), left[0], "Connections", names.len(), 28, &mut st.selected, row_name, |ui, r, i, _| {
        let r = Rect::new(r.x + ui.px(8), r.y, r.w - ui.px(16), r.h);
        let w = label(ui, r, &names[i].0, None);
        if names[i].1 && r.w - w > ui.px(70) {
            let tag = Rect::new(r.right() - ui.px(60), r.y, ui.px(60), r.h);
            label(ui, tag, "ssh", Some(muted));
        }
    });
    if st.selected != before
        && let Some(i) = st.selected
    {
        st.selected = before; // until the switch happens
        st.switch(Some(names[i].0.clone()), app);
    }
    let btns = ui.split_h(left[1], &[L::Frac(0.28), L::Frac(0.44), L::Frac(0.28)], 4);
    if button(ui, Id::new("settings-conn-add"), btns[0], "Add").clicked {
        st.switch(None, app);
    }
    let current = st.form.as_ref().and_then(|f| f.editing.clone()).and_then(|n| app.hosts.iter().position(|h| h.name == n));
    if button(ui, Id::new("settings-conn-dup"), btns[1], "Duplicate").clicked
        && let Some(i) = current
    {
        match app.duplicate_host(&app.hosts[i].name.clone()) {
            Ok(name) => st.switch(Some(name), app),
            Err(e) => st.error = Some(e),
        }
    }
    if button(ui, Id::new("settings-conn-del"), btns[2], "Delete").clicked && current.is_some() {
        st.confirm_delete = true;
    }

    let mut rows = Rows::new(cols[1]);
    if let Some(p) = &mut st.confirm_discard {
        let what = st.form.as_ref().and_then(|f| f.editing.clone()).unwrap_or_else(|| "the new host".to_string());
        match discard_prompt(ui, &mut rows, "settings-conn", &what, p) {
            Some(true) => {
                let to = p.to.clone();
                st.form = None;
                st.switch(to, app);
            }
            Some(false) => st.confirm_discard = None,
            None => {}
        }
        if st.confirm_discard.is_none() {
            ui.request_focus(Id::new("settings-conn-list"));
        }
        return;
    }
    if st.confirm_delete
        && let Some(i) = current
    {
        let h = &app.hosts[i];
        let what = match &h.source_file {
            Some(f) => format!("Delete {} from {}? A backup is kept.", h.name, display_path(f)),
            None => format!("Delete {}?", h.name),
        };
        label(ui, rows.next(ui), &what, None);
        let r = rows.next(ui);
        let b = ui.split_h(Rect::new(r.x, r.y, ui.px(220), r.h), &[L::Frac(0.5), L::Frac(0.5)], 8);
        if button(ui, Id::new("settings-conn-del-yes"), b[0], "Delete").clicked {
            if let Err(e) = app.delete_host(&app.hosts[i].name.clone()) {
                st.error = Some(e);
            }
            st.form = None;
            st.selected = None;
            st.confirm_delete = false;
        } else if button(ui, Id::new("settings-conn-del-no"), b[1], "Cancel").clicked {
            st.confirm_delete = false;
        }
        return;
    }
    if let Some((name, why)) = &st.readonly {
        label(ui, rows.next(ui), name, None);
        label(ui, rows.next_h(ui, 44), why, Some(ui.style().palette.warning));
        label(ui, rows.next(ui), "Skiff leaves this host as it is; edit the file in a text editor.", Some(muted));
        return;
    }
    let Some(form) = &mut st.form else {
        label(ui, rows.next(ui), "Select a host, or Add one.", Some(muted));
        error_line(ui, rows.next(ui), &st.error);
        return;
    };
    let title = if form.editing.is_some() { "Edit host" } else { "New host" };
    label(ui, rows.next(ui), title, None);
    if form.form.can_change_dest() && !form.set.docs.is_empty() {
        let (l, c) = split_row(ui, rows.next(ui));
        label(ui, l, "Save to", None);
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
    for fl in form.form.fields() {
        if field(ui, Id::new(("settings-conn-field", fl as usize)), rows.next(ui), fl.label(), form.form.get_mut(fl)) {
            form.dirty = true;
        }
    }
    let (_, c) = split_row(ui, rows.next(ui));
    if checkbox(ui, Id::new("settings-conn-hidden"), c, &mut form.form.hidden, "Hidden (left out of host lists)").clicked {
        form.dirty = true;
    }
    if matches!(form.form.dest, Dest::SshConfig(_)) {
        let mut remove = None;
        for (i, line) in form.other.iter_mut().enumerate() {
            let (l, c) = split_row(ui, rows.next(ui));
            if i == 0 {
                label(ui, l, "Other options", None);
            }
            let parts = ui.split_h(c, &[L::Fill, L::Px(30)], 4);
            let opts = TextInputOpts { password: false, placeholder: "e.g. ForwardAgent yes", ..Default::default() };
            form.dirty |= text_input(ui, Id::new(("settings-conn-other", i)), parts[0], line, opts).changed;
            if button(ui, Id::new(("settings-conn-other-del", i)), parts[1], "\u{2715}").clicked {
                remove = Some(i);
            }
        }
        if let Some(i) = remove {
            form.other.remove(i);
            form.dirty = true;
        }
        let (l, c) = split_row(ui, rows.next(ui));
        if form.other.is_empty() {
            label(ui, l, "Other options", None);
        }
        if button(ui, Id::new("settings-conn-other-add"), Rect::new(c.x, c.y, ui.px(130), c.h), "+ Add option").clicked {
            form.other.push(String::new());
            form.dirty = true;
        }
    }
    let r = rows.next(ui);
    if button(ui, Id::new("settings-conn-save"), Rect::new(r.x + ui.px(super::LABEL_W), r.y, ui.px(100), r.h), "Save").clicked {
        match save(form, app) {
            Ok(name) => st.edit_host(&name, app),
            Err(e) => st.error = Some(e),
        }
    }
    error_line(ui, rows.next(ui), &st.error);
}

fn save(form: &mut Form, app: &mut GuiApp) -> Result<String, String> {
    form.form.other = form.other.join("\n");
    // Where the edited connection is now; one deleted meanwhile is saved as new.
    if let (Editing::Custom(_), Some(name)) = (&form.form.editing, &form.editing) {
        form.form.editing = match app.cfg.connections.iter().position(|c| &c.name == name) {
            Some(i) => Editing::Custom(i),
            None => Editing::New,
        };
    }
    let taken = hostform::taken_names(&app.hosts);
    let old = form.form.original_name(&app.cfg);
    // Edit a copy: if the write fails, the form can be saved again as is.
    let mut set = form.set.clone();
    let saved = form.form.save(&mut app.cfg, &mut set, &taken)?;
    let name = saved.name().to_string();
    app.apply_saved(saved, &mut set)?;
    if let Some(old) = old {
        app.after_rename(&old, &name);
    }
    form.set = set;
    form.dirty = false;
    form.editing = Some(name.clone());
    Ok(name)
}
