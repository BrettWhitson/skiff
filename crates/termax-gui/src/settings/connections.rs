//! Custom connections: list, form, Add / Duplicate / Delete / Save.

use termax_core::config::{CONNECTION_LABELS, Connection};
use termax_ui::geom::Rect;
use termax_ui::layout::Size as L;
use termax_ui::ui::{Id, Ui};
use termax_ui::widgets::{button, label, list};

use super::{Rows, error_line, field};
use crate::app::GuiApp;

pub struct Form {
    /// Index into `cfg.connections`; `None` for a new connection.
    pub editing: Option<usize>,
    pub fields: [String; 9],
    pub dirty: bool,
}

#[derive(Default)]
pub struct ConnState {
    pub form: Option<Form>,
    pub error: Option<String>,
    pub confirm_delete: bool,
    /// The list's highlighted row.
    selected: Option<usize>,
}

impl ConnState {
    pub fn edit(&mut self, i: usize, app: &GuiApp) {
        if let Some(c) = app.cfg.connections.get(i) {
            self.form = Some(Form { editing: Some(i), fields: c.to_fields(), dirty: false });
            self.selected = Some(i);
        }
        self.error = None;
        self.confirm_delete = false;
    }

    pub fn new_form(&mut self) {
        self.form = Some(Form { editing: None, fields: Connection::default().to_fields(), dirty: true });
        self.selected = None;
        self.error = None;
        self.confirm_delete = false;
    }
}

pub fn view(ui: &mut Ui, rect: Rect, st: &mut ConnState, app: &mut GuiApp) {
    // An untouched form follows the config (outside edits, sidebar changes).
    if let Some(f) = &mut st.form
        && !f.dirty
    {
        match f.editing.and_then(|i| app.cfg.connections.get(i)) {
            Some(c) => f.fields = c.to_fields(),
            None => st.form = None,
        }
    }

    let cols = ui.split_h(rect, &[L::Px(220), L::Fill], 16);
    let left = ui.split_v(cols[0], &[L::Fill, L::Px(30)], 8);
    let names: Vec<String> = app.cfg.connections.iter().map(|c| c.name.clone()).collect();
    let before = st.selected;
    list(ui, Id::new("settings-conn-list"), left[0], names.len(), 28, &mut st.selected, |ui, r, i, _| {
        let r = Rect::new(r.x + ui.px(8), r.y, r.w - ui.px(8), r.h);
        label(ui, r, &names[i], None);
    });
    if st.selected != before
        && let Some(i) = st.selected
    {
        st.edit(i, app);
    }
    let btns = ui.split_h(left[1], &[L::Frac(0.3), L::Frac(0.4), L::Frac(0.3)], 4);
    if button(ui, Id::new("settings-conn-add"), btns[0], "Add").clicked {
        st.new_form();
    }
    let editing = st.form.as_ref().and_then(|f| f.editing);
    if button(ui, Id::new("settings-conn-dup"), btns[1], "Duplicate").clicked
        && let Some(i) = editing
    {
        let mut c = app.cfg.connections[i].clone();
        c.name = app.unique_name(&c.name);
        app.cfg.connections.push(c);
        app.save_config();
        let n = app.cfg.connections.len() - 1;
        st.edit(n, app);
    }
    if button(ui, Id::new("settings-conn-del"), btns[2], "Delete").clicked && editing.is_some() {
        st.confirm_delete = true;
    }

    let mut rows = Rows::new(cols[1]);
    if st.confirm_delete
        && let Some(i) = editing
    {
        let name = app.cfg.connections.get(i).map(|c| c.name.clone()).unwrap_or_default();
        label(ui, rows.next(ui), &format!("Delete {name}?"), None);
        let r = rows.next(ui);
        let b = ui.split_h(Rect::new(r.x, r.y, ui.px(220), r.h), &[L::Frac(0.5), L::Frac(0.5)], 8);
        if button(ui, Id::new("settings-conn-del-yes"), b[0], "Delete").clicked {
            app.cfg.connections.remove(i);
            app.save_config();
            st.form = None;
            st.selected = None;
            st.confirm_delete = false;
        } else if button(ui, Id::new("settings-conn-del-no"), b[1], "Cancel").clicked {
            st.confirm_delete = false;
        }
        return;
    }
    let Some(form) = &mut st.form else {
        let muted = ui.style().palette.muted;
        label(ui, rows.next(ui), "Select a connection, or Add one.", Some(muted));
        return;
    };
    let title = if form.editing.is_some() { "Edit connection" } else { "New connection" };
    label(ui, rows.next(ui), title, None);
    for (i, l) in CONNECTION_LABELS.iter().enumerate() {
        if field(ui, Id::new(("settings-conn-field", i)), rows.next(ui), l, &mut form.fields[i]) {
            form.dirty = true;
        }
    }
    let r = rows.next(ui);
    if button(ui, Id::new("settings-conn-save"), Rect::new(r.x + ui.px(super::LABEL_W), r.y, ui.px(100), r.h), "Save").clicked {
        st.error = save(form, app).err();
        if st.error.is_none() {
            st.selected = form.editing;
        }
    }
    error_line(ui, rows.next(ui), &st.error);
}

fn save(form: &mut Form, app: &mut GuiApp) -> Result<(), String> {
    let c = Connection::from_fields(&form.fields)?;
    let clash = app.cfg.connections.iter().enumerate().any(|(j, o)| Some(j) != form.editing && o.name == c.name);
    if clash {
        return Err(format!("A connection named {} already exists", c.name));
    }
    match form.editing {
        Some(i) if i < app.cfg.connections.len() => app.cfg.connections[i] = c,
        _ => {
            app.cfg.connections.push(c);
            form.editing = Some(app.cfg.connections.len() - 1);
        }
    }
    form.dirty = false;
    app.save_config();
    Ok(())
}
