//! The Forwards page: a heading with New forward, then a table of saved
//! port forwards (name, host, type, route, a status pill) with a primary
//! Start / Stop per row and an overflow menu for Edit and Delete (Delete
//! asks in a dialog); an empty state when there are none. The add/edit
//! form opens as a card above the table and saves with or without
//! starting; ssh's password prompts for forwards show above it too.

use skiff_core::askpass::{AskKind, AskRequest};
use skiff_core::forward::{Forward, ForwardHandle, ForwardKind, ForwardState};
use skiff_ui::a11y::Role;
use skiff_ui::geom::{Point, Rect};
use skiff_ui::icons::Icon;
use skiff_ui::layout::Size as L;
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{
    Align, ButtonKind, ButtonOpts, Column, EmptyState, MenuItem, Status, TextInputOpts, TextOpts, Tone, badge, badge_width, button_width,
    button_with, context_menu, dropdown_labeled, empty_state, icon_button, open_context_menu, paragraph, paragraph_height, status_pill,
    table_header, text, text_input,
};

use crate::app::GuiApp;
use crate::settings::{PendingDiscard, Rows, card, delete_prompt, discard_prompt, error_line, field_ph, row_label, scrolled, split_row};

/// Askpass tags of forwards start with this (file panes use `<tab>:<side>`).
pub const TAG_PREFIX: &str = "fwd:";

/// A forward started this session (running, starting, or failed).
pub struct Running {
    /// The forward as it was when started.
    pub fwd: Forward,
    /// Live state of its ssh process; dropping it stops ssh.
    pub handle: ForwardHandle,
    /// Askpass tag (`fwd:<n>`) routing ssh's prompts back to this forward.
    pub tag: String,
    /// Already counted as a connection in the host's history (done once,
    /// when it first reaches Running).
    pub recorded: bool,
}

/// The inline add/edit form's fields, as typed.
#[derive(Default)]
pub struct FwdForm {
    /// The saved forward being edited (found again by value when saving,
    /// so a reload meanwhile can't make the form overwrite another one).
    pub editing: Option<Forward>,
    /// Host name.
    pub host: String,
    /// Index into `ForwardKind::ALL` (local, remote, dynamic).
    pub kind: usize,
    /// Optional display name.
    pub name: String,
    /// Optional bind address.
    pub bind: String,
    /// Listen port, validated on save.
    pub listen: String,
    /// `host:port` to forward to (ignored for dynamic forwards).
    pub target: String,
    /// Validation or save error shown under the buttons.
    pub error: Option<String>,
    /// Edited since it was opened.
    pub dirty: bool,
}

/// A question ssh asked for a running forward (password, yes/no, notice).
pub struct FwdPrompt {
    /// Tag of the forward that asked.
    pub tag: String,
    /// ssh's prompt text.
    pub text: String,
    /// Which answer widgets to show.
    pub kind: AskKind,
    /// The password typed so far.
    pub input: String,
    /// Where the answer goes; taken on first answer.
    reply: Option<tokio::sync::oneshot::Sender<Option<String>>>,
    /// ssh withdrew the question (e.g. a security key was touched).
    cancelled: tokio_util::sync::CancellationToken,
}

impl FwdPrompt {
    /// Sends the answer (`None` = declined) once; later calls do nothing.
    fn answer(&mut self, a: Option<String>) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(a);
        }
    }
}

impl Drop for FwdPrompt {
    /// A prompt closed without an answer declines, so ssh never hangs.
    fn drop(&mut self) {
        self.answer(None);
    }
}

/// Everything the Forwards tab keeps between frames (owned by the app even
/// while the tab is closed, so forwards keep running).
#[derive(Default)]
pub struct ForwardsState {
    /// Forwards started this session, oldest first.
    pub running: Vec<Running>,
    /// The open add/edit form.
    pub form: Option<FwdForm>,
    /// The ssh question being shown, if any (one at a time).
    pub prompt: Option<FwdPrompt>,
    /// The saved forward waiting for "Delete?" to be confirmed.
    pub confirm_delete: Option<Forward>,
    /// Another form waiting for "Discard unsaved changes?" to be answered.
    confirm_discard: Option<PendingDiscard<FwdForm>>,
    /// Counter behind `new_tag`.
    next_tag: u64,
}

impl ForwardsState {
    /// State of the most recent run of `f`; `None` if never started (or pruned).
    pub fn state_of(&self, f: &Forward) -> Option<ForwardState> {
        self.running.iter().rev().find(|r| &r.fwd == f).map(|r| r.handle.state())
    }

    /// Shows ssh's question, replacing (and so declining) any earlier one.
    pub fn ask(&mut self, req: AskRequest) {
        self.prompt = Some(FwdPrompt {
            tag: req.tag,
            text: req.prompt,
            kind: req.kind,
            input: String::new(),
            reply: Some(req.reply),
            cancelled: req.cancelled,
        });
    }

    /// Stops every run of `f` and drops its pending prompt, if any.
    pub fn stop(&mut self, f: &Forward) {
        for r in self.running.iter().filter(|r| &r.fwd == f) {
            r.handle.stop();
        }
        if self.prompt.as_ref().is_some_and(|p| self.running.iter().any(|r| &r.fwd == f && r.tag == p.tag)) {
            self.prompt = None;
        }
    }

    /// Stops all forwards (app shutdown).
    pub fn stop_all(&mut self) {
        self.prompt = None;
        self.running.clear(); // dropping a handle stops its ssh
    }

    /// Forgets forwards that were stopped, and failed ones no longer saved
    /// (a failed saved forward stays, to show why).
    pub fn prune(&mut self, saved: &[Forward]) {
        self.running.retain(|r| match r.handle.state() {
            ForwardState::Stopped => false,
            ForwardState::Failed(_) => saved.contains(&r.fwd),
            _ => true,
        });
    }

    /// A fresh askpass tag for a forward about to start.
    pub fn new_tag(&mut self) -> String {
        self.next_tag += 1;
        format!("{TAG_PREFIX}{}", self.next_tag)
    }

    /// The form for a new forward on host `host`.
    pub fn new_form(&mut self, host: &str) {
        self.open_form(FwdForm { host: host.to_string(), target: "localhost:".into(), ..Default::default() });
    }

    /// Shows `form`, asking first when the open one has unsaved edits.
    pub fn open_form(&mut self, form: FwdForm) {
        if self.form.as_ref().is_some_and(|f| f.dirty) {
            self.confirm_discard = Some(PendingDiscard::new(form));
        } else {
            self.form = Some(form);
            self.confirm_discard = None;
        }
    }
}

/// Validates the form into a [`Forward`]: port 1-65535, blank optional
/// fields become `None`, then the forward's own checks.
fn form_to_forward(f: &FwdForm) -> Result<Forward, String> {
    let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    let listen = f.listen.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or("Listen port must be 1\u{2013}65535")?;
    let kind = ForwardKind::ALL[f.kind.min(2)];
    let fwd = Forward {
        name: opt(&f.name),
        host: f.host.clone(),
        kind,
        bind: opt(&f.bind),
        listen,
        target: if kind == ForwardKind::Dynamic { None } else { opt(&f.target) },
    };
    fwd.validate()?;
    Ok(fwd)
}

/// Saves the form's forward; starts it when `start`, or when it replaces
/// one that was running.
fn save_form(st: &mut ForwardsState, app: &mut GuiApp, start: bool) {
    let Some(form) = &mut st.form else { return };
    let f = match form_to_forward(form) {
        Ok(f) => f,
        Err(e) => {
            form.error = Some(e);
            return;
        }
    };
    let old = form.editing.clone();
    let at = old.as_ref().and_then(|o| app.cfg.forwards.iter().position(|x| x == o));
    if old.is_some() && at.is_none() {
        // Deleted (or changed) elsewhere while the form was open.
        form.editing = None;
        form.error = Some("This forward was deleted; Save again to add it back".into());
        return;
    }
    if app.cfg.forwards.iter().enumerate().any(|(i, x)| x == &f && Some(i) != at) {
        form.error = Some("This forward is already saved".into());
        return;
    }
    // Replace in place when editing, else append.
    let mut was_live = false;
    match at {
        Some(i) => {
            let o = app.cfg.forwards[i].clone();
            was_live = st.state_of(&o).is_some_and(|s| s.is_live());
            if o != f {
                st.stop(&o);
            }
            app.cfg.forwards[i] = f.clone();
        }
        None => app.cfg.forwards.push(f.clone()),
    }
    app.save_config();
    st.form = None;
    // An unchanged forward that is already running is left alone.
    let unchanged_and_live = old.as_ref() == Some(&f) && was_live;
    if (start || was_live) && !unchanged_and_live {
        app.start_forward(st, &f);
    }
}

/// What a row's status pill shows, from the forward's last run.
pub fn status_of(state: Option<&ForwardState>) -> (Status, &'static str) {
    match state {
        Some(ForwardState::Running) => (Status::Ok, "Running"),
        Some(ForwardState::Starting) => (Status::Connecting, "Starting"),
        Some(ForwardState::Failed(_)) => (Status::Error, "Failed"),
        Some(ForwardState::Stopped) | None => (Status::Idle, "Stopped"),
    }
}

/// Where a forward listens and where it leads, e.g.
/// "localhost:15432 → 127.0.0.1:5432" or "localhost:1080 · SOCKS proxy".
pub fn route(f: &Forward) -> String {
    let side = if f.kind == ForwardKind::Remote { f.host.as_str() } else { "localhost" };
    let bind = f.bind.as_deref().filter(|b| !b.is_empty()).unwrap_or(side);
    match (f.kind, f.target.as_deref()) {
        (ForwardKind::Dynamic, _) => format!("{bind}:{} \u{00B7} SOCKS proxy", f.listen),
        (_, Some(t)) => format!("{bind}:{} \u{2192} {t}", f.listen),
        (_, None) => format!("{bind}:{}", f.listen),
    }
}

/// The short type name shown in the table's badge.
fn kind_badge(k: ForwardKind) -> &'static str {
    match k {
        ForwardKind::Local => "Local",
        ForwardKind::Remote => "Remote",
        ForwardKind::Dynamic => "SOCKS",
    }
}

/// The overflow menu's items, in order (Edit, a separator, Delete).
pub const MENU: [&str; 3] = ["Edit", "", "Delete"];

/// Height of a table row, logical px.
const ROW: i32 = 40;
/// Extra height of a row that shows why its forward failed, logical px.
const FAIL_LINE: i32 = 20;

/// Draws the Forwards page and applies its actions. The state is moved out
/// of the app for the frame so both can be borrowed mutably.
pub fn view(ui: &mut Ui, rect: Rect, app: &mut GuiApp) {
    let mut st = std::mem::take(&mut app.forwards);
    let pad = ui.px(tokens::SPACE_XL);
    let area = Rect::new(rect.x + pad, rect.y + ui.px(tokens::SPACE_LG), rect.w - 2 * pad, rect.h - ui.px(tokens::SPACE_LG));
    let body = Rect::new(area.x, area.y, area.w.min(ui.px(1120)), area.h);
    // Heading, a one-line summary, and New forward.
    let head = Rect::new(body.x, body.y, body.w, ui.px(40));
    let nw = button_width(ui, "New forward", 1);
    let opts = ButtonOpts { kind: ButtonKind::Primary, icon: Some(Icon::Plus), ..Default::default() };
    let br = Rect::new(head.right() - nw, head.y + (head.h - ui.px(tokens::CONTROL_H)) / 2, nw, ui.px(tokens::CONTROL_H));
    // With nothing saved the empty state's button is the one primary action.
    let empty = app.cfg.forwards.is_empty() && st.form.is_none();
    if !empty && button_with(ui, Id::new("fwd-new"), br, "New forward", opts).clicked {
        let h = app.sidebar_selected().or_else(|| app.hosts.first().map(|h| h.name.clone())).unwrap_or_default();
        st.new_form(&h);
    }
    let tw = text(
        ui,
        Rect::new(head.x, head.y, br.x - head.x, head.h),
        "Port forwards",
        TextOpts { role: Some(TextRole::Heading), ..Default::default() },
    );
    if let Some(n) = ui.a11y_node(Id::new("fwd-title"), Role::Heading, "Port forwards") {
        n.bounds = Rect::new(head.x, head.y, tw, head.h);
    }
    let live = st.running.iter().filter(|r| r.handle.state() == ForwardState::Running).count();
    let saved = app.cfg.forwards.len();
    if saved > 0 {
        let muted = ui.style().palette.muted;
        let sum = format!("{saved} saved \u{00B7} {live} running");
        let x = head.x + tw + ui.px(tokens::SPACE_MD);
        text(
            ui,
            Rect::new(x, head.y + ui.px(2), (br.x - x).max(0), head.h),
            &sum,
            TextOpts { role: Some(TextRole::Caption), color: Some(muted), ..Default::default() },
        );
    }
    let rest = Rect::new(body.x, head.bottom() + ui.px(tokens::SPACE_MD), body.w, body.bottom() - head.bottom() - ui.px(tokens::SPACE_MD));
    let mut act: Option<(Forward, &'static str)> = None;
    scrolled(ui, Id::new("fwd-page"), rest, |ui, r| content(ui, r, &mut st, app, &mut act));
    dialogs(ui, &mut st, app);
    if let Some((f, what)) = act {
        match what {
            "start" => app.start_forward(&mut st, &f),
            "stop" => st.stop(&f),
            "edit" => {
                st.open_form(FwdForm {
                    editing: Some(f.clone()),
                    host: f.host.clone(),
                    kind: ForwardKind::ALL.iter().position(|k| *k == f.kind).unwrap_or(0),
                    name: f.name.clone().unwrap_or_default(),
                    bind: f.bind.clone().unwrap_or_default(),
                    listen: f.listen.to_string(),
                    target: f.target.clone().unwrap_or_default(),
                    error: None,
                    dirty: false,
                });
            }
            _ => st.confirm_delete = Some(f),
        }
    }
    app.forwards = st;
}

/// The page below the heading (content coordinates): ssh's question, the
/// form, then the table or the empty state. Row actions go to `act`.
/// Returns the height used.
fn content(ui: &mut Ui, rect: Rect, st: &mut ForwardsState, app: &mut GuiApp, act: &mut Option<(Forward, &'static str)>) -> i32 {
    let mut y = rect.y;
    y += prompt_card(ui, Rect::new(rect.x, y, rect.w, 0), st);
    y += form_card(ui, Rect::new(rect.x, y, rect.w.min(ui.px(720)), 0), st, app);
    if app.cfg.forwards.is_empty() {
        if st.form.is_none() {
            let e = EmptyState {
                icon: Some(Icon::Forward),
                title: "No port forwards yet",
                body: "Reach a service behind one of your hosts through a local port, or open a SOCKS proxy through it.",
                action: Some("Add a forward"),
                hint: Some("Or right-click a host and choose Port forward\u{2026}"),
                ..Default::default()
            };
            let r = Rect::new(rect.x, y, rect.w, (rect.bottom() - y).max(ui.px(320)));
            if empty_state(ui, Id::new("fwd-empty"), r, &e).action {
                let h = app.sidebar_selected().or_else(|| app.hosts.first().map(|h| h.name.clone())).unwrap_or_default();
                st.new_form(&h);
            }
            return r.bottom() - rect.y;
        }
        return y - rect.y;
    }
    y + table(ui, Rect::new(rect.x, y, rect.w, 0), st, app, act) - rect.y
}

/// ssh's question for a running forward, as a card: a notice (Hide), a
/// yes/no confirmation, or a password field. Returns the height used.
fn prompt_card(ui: &mut Ui, at: Rect, st: &mut ForwardsState) -> i32 {
    // Hidden once ssh withdraws it.
    if st.prompt.as_ref().is_some_and(|p| p.cancelled.is_cancelled()) {
        st.prompt = None;
    }
    let Some(pr) = &mut st.prompt else { return 0 };
    let p = ui.style().palette;
    let pad = ui.px(tokens::SPACE_LG);
    let w = at.w.min(ui.px(720));
    let tw = w - 2 * pad - ui.px(tokens::ICON + tokens::SPACE_SM);
    let th = paragraph_height(ui, &pr.text, TextRole::Body, tw);
    let ctl = ui.px(tokens::CONTROL_H);
    let h = pad + th + ui.px(tokens::SPACE_MD) + ctl + pad;
    let c = Rect::new(at.x, at.y, w, h);
    card(ui, c);
    let s = ui.px(tokens::ICON);
    ui.painter().icon(Rect::new(c.x + pad, c.y + pad, s, s), Icon::Key, p.accent);
    let tx = c.x + pad + s + ui.px(tokens::SPACE_SM);
    paragraph(ui, Rect::new(tx, c.y + pad, tw, th), &pr.text, TextRole::Body, p.fg, Align::Start);
    let row = Rect::new(tx, c.y + pad + th + ui.px(tokens::SPACE_MD), tw, ctl);
    let parts = ui.split_h(row, &[L::Fill, L::Px(80), L::Px(80)], tokens::SPACE_SM);
    let primary = ButtonOpts::kind(ButtonKind::Primary);
    let mut done = None;
    if pr.kind == AskKind::Notice {
        if button_with(ui, Id::new("fwd-ask-hide"), parts[2], "Hide", ButtonOpts::default()).clicked {
            done = Some(None);
        }
    } else if pr.kind == AskKind::Confirm {
        if button_with(ui, Id::new("fwd-ask-yes"), parts[1], "Yes", primary).clicked {
            done = Some(Some("yes".to_string()));
        }
        if button_with(ui, Id::new("fwd-ask-no"), parts[2], "No", ButtonOpts::default()).clicked {
            done = Some(None);
        }
    } else {
        let t = text_input(
            ui,
            Id::new("fwd-ask-input"),
            parts[0],
            &mut pr.input,
            TextInputOpts { password: true, placeholder: "Password", label: "Password", ..Default::default() },
        );
        if button_with(ui, Id::new("fwd-ask-ok"), parts[1], "OK", primary).clicked || t.submitted {
            done = Some(Some(std::mem::take(&mut pr.input)));
        }
        if button_with(ui, Id::new("fwd-ask-cancel"), parts[2], "Cancel", ButtonOpts::kind(ButtonKind::Ghost)).clicked {
            done = Some(None);
        }
    }
    if let Some(a) = done {
        pr.answer(a);
        st.prompt = None;
    }
    h + ui.px(tokens::SPACE_LG)
}

/// The add/edit form as a card: host, kind (with what it does), name,
/// bind address, listen port, target; Save (primary), Save and start,
/// Cancel. Returns the height used.
fn form_card(ui: &mut Ui, at: Rect, st: &mut ForwardsState, app: &mut GuiApp) -> i32 {
    let Some(form) = &mut st.form else { return 0 };
    let p = ui.style().palette;
    let pad = ui.px(tokens::SPACE_LG);
    let kind = ForwardKind::ALL[form.kind.min(2)];
    let inner_w = at.w - 2 * pad;
    let explain_h = paragraph_height(ui, kind.explain(), TextRole::Caption, (inner_w - ui.px(crate::settings::LABEL_W)).max(1));
    let fields = if kind == ForwardKind::Dynamic { 5 } else { 6 };
    let step = |h: i32| ui.px(h + tokens::SPACE_SM);
    let mut h = 2 * pad + step(28) + fields * step(tokens::CONTROL_H) + explain_h + ui.px(tokens::SPACE_SM) + step(tokens::CONTROL_H);
    if form.error.is_some() {
        h += step(tokens::CONTROL_H);
    }
    let c = Rect::new(at.x, at.y, at.w, h);
    card(ui, c);
    let mut rows = Rows::new(c.inset(pad));
    let title = if form.editing.is_some() { "Edit forward" } else { "New forward" };
    text(ui, rows.next_h(ui, 28), title, TextOpts { role: Some(TextRole::Title), ..Default::default() });
    let mut host_names: Vec<String> = app.hosts.iter().map(|h| h.name.clone()).collect();
    // A host no longer listed stays the form's host (and a choice) until changed.
    if !form.host.is_empty() && !host_names.contains(&form.host) {
        host_names.push(form.host.clone());
    }
    let (l, c2) = split_row(ui, rows.next(ui));
    row_label(ui, l, "Host");
    let opts: Vec<&str> = host_names.iter().map(String::as_str).collect();
    let mut hi = host_names.iter().position(|h| *h == form.host).unwrap_or(0);
    if dropdown_labeled(ui, Id::new("fwd-host"), c2, "Host", &opts, &mut hi)
        && let Some(h) = host_names.get(hi)
    {
        form.host = h.clone();
        form.dirty = true;
    }
    // A new form with no host takes the dropdown's first choice.
    if form.host.is_empty()
        && let Some(h) = host_names.get(hi)
    {
        form.host = h.clone();
    }
    let (l, c2) = split_row(ui, rows.next(ui));
    row_label(ui, l, "Kind");
    let kinds: Vec<&str> = ForwardKind::ALL.iter().map(|k| k.label()).collect();
    form.dirty |= dropdown_labeled(ui, Id::new("fwd-kind"), c2, "Kind", &kinds, &mut form.kind);
    let kind = ForwardKind::ALL[form.kind.min(2)];
    let (_, c2) = split_row(ui, rows.next_h(ui, 0));
    let er = Rect::new(c2.x, c2.y - ui.px(tokens::SPACE_XS), c2.w, explain_h);
    paragraph(ui, er, kind.explain(), TextRole::Caption, p.muted, Align::Start);
    rows.gap(ui, (explain_h as f32 / ui.scale()) as i32);
    form.dirty |= field_ph(ui, Id::new("fwd-name"), rows.next(ui), "Name", &mut form.name, "optional, e.g. Postgres");
    form.dirty |= field_ph(ui, Id::new("fwd-bind"), rows.next(ui), "Bind address", &mut form.bind, "localhost");
    form.dirty |= field_ph(ui, Id::new("fwd-listen"), rows.next(ui), "Listen port", &mut form.listen, "e.g. 8080");
    if kind != ForwardKind::Dynamic {
        form.dirty |= field_ph(ui, Id::new("fwd-target"), rows.next(ui), "Target", &mut form.target, "host:port, e.g. localhost:5432");
    }
    rows.gap(ui, tokens::SPACE_SM);
    let (_, c2) = split_row(ui, rows.next(ui));
    let w1 = button_width(ui, "Save", 0).max(ui.px(80));
    let w2 = button_width(ui, "Save and start", 0);
    let w3 = button_width(ui, "Cancel", 0);
    let gap = ui.px(tokens::SPACE_SM);
    let mut save = None;
    let mut cancel = false;
    if button_with(ui, Id::new("fwd-save"), Rect::new(c2.x, c2.y, w1, c2.h), "Save", ButtonOpts::kind(ButtonKind::Primary)).clicked {
        save = Some(false);
    }
    if button_with(ui, Id::new("fwd-save-start"), Rect::new(c2.x + w1 + gap, c2.y, w2, c2.h), "Save and start", ButtonOpts::default())
        .clicked
    {
        save = Some(true);
    }
    let ghost = ButtonOpts::kind(ButtonKind::Ghost);
    if button_with(ui, Id::new("fwd-cancel"), Rect::new(c2.x + w1 + w2 + 2 * gap, c2.y, w3, c2.h), "Cancel", ghost).clicked {
        cancel = true;
    }
    if form.error.is_some() {
        let (_, c2) = split_row(ui, rows.next(ui));
        error_line(ui, c2, &form.error);
    }
    if let Some(start) = save {
        save_form(st, app, start);
    } else if cancel {
        st.form = None;
    }
    h + ui.px(tokens::SPACE_LG)
}

/// The table of saved forwards: header, then one row each (failed ones
/// with their reason under them). Row actions go to `act`. Returns the
/// height used.
fn table(ui: &mut Ui, at: Rect, st: &mut ForwardsState, app: &mut GuiApp, act: &mut Option<(Forward, &'static str)>) -> i32 {
    let p = ui.style().palette;
    let wide = at.w > ui.px(760);
    let mut columns = vec![
        Column { title: "Name", width: L::Fill, sortable: false, align: Align::Start },
        Column { title: "Host", width: L::Px(140), sortable: false, align: Align::Start },
        Column { title: "Type", width: L::Px(84), sortable: false, align: Align::Start },
    ];
    if wide {
        columns.push(Column { title: "Route", width: L::Fill, sortable: false, align: Align::Start });
    }
    columns.push(Column { title: "Status", width: L::Px(112), sortable: false, align: Align::Start });
    columns.push(Column { title: "", width: L::Px(124), sortable: false, align: Align::End });
    let hh = ui.px(tokens::HEADER_H);
    let mut none = None;
    let cols = table_header(ui, Id::new("fwd-table"), Rect::new(at.x, at.y, at.w, hh), &columns, &mut none).columns;
    let (status_col, action_col) = (cols.len() - 2, cols.len() - 1);
    let pad = ui.px(tokens::SPACE_SM);
    let bw = ui.px(1).max(1);
    let mut y = at.y + hh;
    let cell = |c: Rect, y: i32, h: i32| Rect::new(c.x + pad, y, (c.w - 2 * pad).max(0), h);
    for (i, f) in app.cfg.forwards.iter().enumerate() {
        let state = st.state_of(f);
        let failed = match &state {
            Some(ForwardState::Failed(e)) => Some(e.clone()),
            _ => None,
        };
        let rh = ui.px(ROW);
        let full = rh + if failed.is_some() { ui.px(FAIL_LINE) } else { 0 };
        let title = f.title();
        // Name (and route below it when there is no Route column).
        let name = f.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| kind_badge(f.kind).to_string() + " " + &f.listen.to_string());
        let strong = TextOpts { role: Some(TextRole::Strong), ..Default::default() };
        text(ui, cell(cols[0], y, rh), &name, strong);
        text(ui, cell(cols[1], y, rh), &f.host, TextOpts { color: Some(p.muted), ..Default::default() });
        let kb = kind_badge(f.kind);
        let kw = badge_width(ui, kb);
        let bh = ui.px(20);
        badge(
            ui,
            Rect::new(cols[2].x + pad, y + (rh - bh) / 2, kw, bh),
            kb,
            if f.kind == ForwardKind::Dynamic { Tone::Info } else { Tone::Neutral },
        );
        if wide {
            text(ui, cell(cols[3], y, rh), &route(f), TextOpts { color: Some(p.muted), ..Default::default() });
        }
        let (status, label) = status_of(state.as_ref());
        let pr = cell(cols[status_col], y + (rh - bh) / 2, bh);
        status_pill(ui, Id::new(("fwd-status", i)), pr, status, label);
        // Actions: Start / Stop, then the overflow menu.
        let live = state.as_ref().is_some_and(|s| s.is_live());
        let ac = cols[action_col];
        let ctl = ui.px(tokens::CONTROL_H);
        let by = y + (rh - ctl) / 2;
        let more = Rect::new(ac.right() - ctl, by, ctl, ctl);
        let run = if live { "Stop" } else { "Start" };
        let rw = button_width(ui, run, 0).max(ui.px(72));
        let rr = Rect::new(more.x - pad / 2 - rw, by, rw, ctl);
        let name_run = format!("{run} {title}");
        let kind = if live { ButtonKind::Secondary } else { ButtonKind::Primary };
        let opts = ButtonOpts { kind, a11y_name: Some(&name_run), ..Default::default() };
        if button_with(ui, Id::new(("fwd-run", i)), rr, run, opts).clicked {
            *act = Some((f.clone(), if live { "stop" } else { "start" }));
        }
        let mid = Id::new(("fwd-more", i));
        let more_name = format!("More actions for {title}");
        if icon_button(ui, mid, more, Icon::Ellipsis, &more_name, ButtonKind::Ghost).clicked {
            open_context_menu(ui, mid.with("menu"), Point::new(more.right() - ui.px(180), more.bottom() + ui.px(tokens::SPACE_XS)));
        }
        let items = [MenuItem::new(MENU[0]).icon(Icon::Edit), MenuItem::SEPARATOR, MenuItem::new(MENU[2]).icon(Icon::Trash).danger()];
        match context_menu(ui, mid.with("menu"), &items) {
            Some(0) => *act = Some((f.clone(), "edit")),
            Some(2) => *act = Some((f.clone(), "delete")),
            _ => {}
        }
        if let Some(e) = failed {
            let er = Rect::new(cols[0].x + pad, y + rh - ui.px(8), ac.x - cols[0].x - pad, ui.px(FAIL_LINE));
            text(ui, er, &e, TextOpts { role: Some(TextRole::Caption), color: Some(p.danger), ..Default::default() });
        }
        y += full;
        ui.painter().rect(Rect::new(at.x, y - bw, at.w, bw), p.border, 0.0);
    }
    y - at.y
}

/// The delete dialog, while one is open; it closes if the forward was
/// removed elsewhere meanwhile. Then the discard dialog.
fn dialogs(ui: &mut Ui, st: &mut ForwardsState, app: &mut GuiApp) {
    if let Some(f) = st.confirm_delete.clone() {
        if !app.cfg.forwards.contains(&f) {
            st.confirm_delete = None;
        } else {
            let title = format!("Delete forward {}?", f.label());
            let running = st.state_of(&f).is_some_and(|s| s.is_live());
            let body =
                format!("{} is removed from config.toml{}. This cannot be undone.", route(&f), if running { " and stopped" } else { "" });
            match delete_prompt(ui, Id::new("fwd-delete"), &title, &body) {
                Some(true) => {
                    st.stop(&f);
                    st.running.retain(|r| r.fwd != f);
                    app.cfg.forwards.retain(|x| *x != f);
                    app.save_config();
                    st.confirm_delete = None;
                }
                Some(false) => st.confirm_delete = None,
                None => {}
            }
        }
    }
    if st.confirm_discard.is_some() {
        let what = if st.form.as_ref().is_some_and(|f| f.editing.is_some()) { "this forward" } else { "the new forward" };
        match discard_prompt(ui, "fwd", what) {
            Some(true) => st.form = st.confirm_discard.take().map(|p| p.to),
            Some(false) => st.confirm_discard = None,
            None => {}
        }
    }
}
