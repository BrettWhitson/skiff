//! The Forwards tab: saved port forwards with Start / Stop / Edit / Delete
//! (Delete asks first), an inline form that saves with or without starting,
//! and ssh's password prompts for forwards that ask.

use skiff_core::askpass::{AskKind, AskRequest};
use skiff_core::forward::{Forward, ForwardHandle, ForwardKind, ForwardState};
use skiff_ui::geom::Rect;
use skiff_ui::input::{Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{TextInputOpts, button, dropdown_labeled, label, text_input};

use crate::app::GuiApp;
use crate::settings::{PendingDiscard, Rows, discard_prompt, error_line, field, split_row};

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
    /// The delete question just appeared: focus goes to Cancel.
    confirm_new: bool,
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

/// Draws the Forwards tab and applies its actions. The state is moved out of
/// the app for the frame so both can be borrowed mutably.
pub fn view(ui: &mut Ui, rect: Rect, app: &mut GuiApp) {
    let mut st = std::mem::take(&mut app.forwards);
    let p = ui.style().palette;
    let body = rect.inset(ui.px(16));
    let mut rows = Rows::new(body);
    let head = rows.next(ui);
    let hp = ui.split_h(head, &[L::Fill, L::Px(130)], 8);
    label(ui, hp[0], "Port forwards", None);
    if button(ui, Id::new("fwd-new"), hp[1], "New forward").clicked {
        let h = app.sidebar_selected().or_else(|| app.hosts.first().map(|h| h.name.clone())).unwrap_or_default();
        st.new_form(&h);
    }

    // ssh's question: hidden once ssh withdraws it, otherwise a notice
    // (Hide), a yes/no confirmation, or a password field.
    if st.prompt.as_ref().is_some_and(|p| p.cancelled.is_cancelled()) {
        st.prompt = None;
    }
    if let Some(pr) = &mut st.prompt {
        let r = rows.next(ui);
        label(ui, r, &pr.text, Some(p.warning));
        let r = rows.next(ui);
        let parts = ui.split_h(r, &[L::Fill, L::Px(80), L::Px(80)], 6);
        let mut done = None;
        if pr.kind == AskKind::Notice {
            if button(ui, Id::new("fwd-ask-hide"), parts[2], "Hide").clicked {
                done = Some(None);
            }
        } else if pr.kind == AskKind::Confirm {
            if button(ui, Id::new("fwd-ask-yes"), parts[1], "Yes").clicked {
                done = Some(Some("yes".to_string()));
            }
            if button(ui, Id::new("fwd-ask-no"), parts[2], "No").clicked {
                done = Some(None);
            }
        } else {
            let t = text_input(
                ui,
                Id::new("fwd-ask-input"),
                parts[0],
                &mut pr.input,
                TextInputOpts { password: true, placeholder: "Password", ..Default::default() },
            );
            if button(ui, Id::new("fwd-ask-ok"), parts[1], "OK").clicked || t.submitted {
                done = Some(Some(std::mem::take(&mut pr.input)));
            }
            if button(ui, Id::new("fwd-ask-cancel"), parts[2], "Cancel").clicked {
                done = Some(None);
            }
        }
        if let Some(a) = done {
            pr.answer(a);
            st.prompt = None;
        }
    }

    let mut host_names: Vec<String> = app.hosts.iter().map(|h| h.name.clone()).collect();
    let mut save = None;
    let mut cancel = false;
    // The discard question replaces the form until answered.
    if let Some(p) = &mut st.confirm_discard {
        let what = if st.form.as_ref().is_some_and(|f| f.editing.is_some()) { "this forward" } else { "the new forward" };
        match discard_prompt(ui, &mut rows, "fwd", what, p) {
            Some(true) => st.form = st.confirm_discard.take().map(|p| p.to),
            Some(false) => st.confirm_discard = None,
            None => {}
        }
    } else if let Some(form) = &mut st.form {
        // A host no longer listed stays the form's host (and a choice) until changed.
        if !form.host.is_empty() && !host_names.contains(&form.host) {
            host_names.push(form.host.clone());
        }
        let (l, c) = split_row(ui, rows.next(ui));
        label(ui, l, "Host", None);
        let opts: Vec<&str> = host_names.iter().map(String::as_str).collect();
        let mut hi = host_names.iter().position(|h| *h == form.host).unwrap_or(0);
        if dropdown_labeled(ui, Id::new("fwd-host"), c, "Host", &opts, &mut hi)
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
        let (l, c) = split_row(ui, rows.next(ui));
        label(ui, l, "Kind", None);
        let kinds: Vec<&str> = ForwardKind::ALL.iter().map(|k| k.label()).collect();
        form.dirty |= dropdown_labeled(ui, Id::new("fwd-kind"), c, "Kind", &kinds, &mut form.kind);
        let kind = ForwardKind::ALL[form.kind.min(2)];
        let (_, c) = split_row(ui, rows.next_h(ui, 40));
        label(ui, c, kind.explain(), Some(p.muted));
        form.dirty |= field(ui, Id::new("fwd-name"), rows.next(ui), "Name", &mut form.name);
        form.dirty |= field(ui, Id::new("fwd-bind"), rows.next(ui), "Bind address", &mut form.bind);
        form.dirty |= field(ui, Id::new("fwd-listen"), rows.next(ui), "Listen port", &mut form.listen);
        if kind != ForwardKind::Dynamic {
            form.dirty |= field(ui, Id::new("fwd-target"), rows.next(ui), "Target", &mut form.target);
        }
        let (_, c) = split_row(ui, rows.next(ui));
        let b = ui.split_h(c, &[L::Px(80), L::Px(140), L::Px(90)], 8);
        if button(ui, Id::new("fwd-save"), b[0], "Save").clicked {
            save = Some(false);
        }
        if button(ui, Id::new("fwd-save-start"), b[1], "Save and start").clicked {
            save = Some(true);
        }
        if button(ui, Id::new("fwd-cancel"), b[2], "Cancel").clicked {
            cancel = true;
        }
        error_line(ui, rows.next(ui), &form.error);
    }
    if let Some(start) = save {
        save_form(&mut st, app, start);
    } else if cancel {
        st.form = None;
    }

    // Delete confirmation; it vanishes if the forward was removed elsewhere.
    if let Some(f) = st.confirm_delete.clone() {
        if !app.cfg.forwards.contains(&f) {
            st.confirm_delete = None;
        } else {
            let r = rows.next(ui);
            let what = format!("Delete forward {}?", f.title());
            label(ui, r, &what, Some(p.danger));
            if let Some(n) = ui.a11y_node(Id::new("fwd-delete-confirm"), skiff_ui::a11y::Role::Dialog, &what) {
                n.live = true;
            }
            let (_, c) = split_row(ui, rows.next(ui));
            let b = ui.split_h(c, &[L::Px(90), L::Px(90)], 8);
            let (yes, no) = (Id::new("fwd-del-yes"), Id::new("fwd-del-no"));
            if std::mem::take(&mut st.confirm_new) {
                ui.request_focus(no);
            }
            let esc = (ui.has_focus(yes) || ui.has_focus(no)) && ui.input_mut().take_key(&Key::Escape, Modifiers::NONE);
            let (delete, keep) = (button(ui, yes, b[0], "Delete").clicked, button(ui, no, b[1], "Cancel").clicked);
            if delete {
                st.stop(&f);
                st.running.retain(|r| r.fwd != f);
                app.cfg.forwards.retain(|x| *x != f);
                app.save_config();
                st.confirm_delete = None;
            } else if keep || esc {
                st.confirm_delete = None;
            }
            if st.confirm_delete.is_none() {
                ui.request_focus(Id::new("fwd-new"));
            }
        }
    }

    rows.next_h(ui, 8);
    if app.cfg.forwards.is_empty() && st.form.is_none() {
        label(ui, rows.next(ui), "No saved forwards. Right-click a host and choose Port forward\u{2026}", Some(p.muted));
    }
    // One row per saved forward; the clicked action runs after the loop,
    // when `app.cfg` is no longer borrowed.
    let mut act: Option<(Forward, &str)> = None;
    for (i, f) in app.cfg.forwards.iter().enumerate() {
        let r = rows.next(ui);
        let parts = ui.split_h(r, &[L::Fill, L::Frac(0.3), L::Px(70), L::Px(60), L::Px(70)], 6);
        label(ui, parts[0], &f.title(), None);
        let state = st.state_of(f);
        let (text, color) = match &state {
            Some(ForwardState::Running) => ("running".to_string(), p.success),
            Some(ForwardState::Failed(e)) => (format!("failed: {e}"), p.danger),
            Some(s) => (s.label(), p.muted),
            None => ("stopped".to_string(), p.muted),
        };
        label(ui, parts[1], &text, Some(color));
        let live = state.as_ref().is_some_and(|s| s.is_live());
        let title = f.title();
        let run = if live { "Stop" } else { "Start" };
        if row_button(ui, Id::new(("fwd-run", i)), parts[2], run, &title) {
            act = Some((f.clone(), if live { "stop" } else { "start" }));
        }
        if row_button(ui, Id::new(("fwd-edit", i)), parts[3], "Edit", &title) {
            act = Some((f.clone(), "edit"));
        }
        if row_button(ui, Id::new(("fwd-del", i)), parts[4], "Delete", &title) {
            act = Some((f.clone(), "delete"));
        }
    }
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
            _ => {
                st.confirm_delete = Some(f);
                st.confirm_new = true;
            }
        }
    }
    app.forwards = st;
}

/// A row's button, named for screen readers by what it acts on ("Start ⇄ web 8080→…").
fn row_button(ui: &mut Ui, id: Id, rect: Rect, text: &str, what: &str) -> bool {
    let clicked = button(ui, id, rect, text).clicked;
    if let Some(n) = ui.a11y_node(id, skiff_ui::a11y::Role::Button, text) {
        n.name = format!("{text} {what}");
    }
    clicked
}
