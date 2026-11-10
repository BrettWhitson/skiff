//! The Forwards tab: saved port forwards with Start / Stop / Edit / Delete,
//! an inline form, and ssh's password prompts for forwards that ask.

use termax_core::askpass::{AskKind, AskRequest};
use termax_core::forward::{Forward, ForwardHandle, ForwardKind, ForwardState};
use termax_ui::geom::Rect;
use termax_ui::layout::Size as L;
use termax_ui::ui::{Id, Ui};
use termax_ui::widgets::{TextInputOpts, button, dropdown, label, text_input};

use crate::app::GuiApp;
use crate::settings::{Rows, error_line, field, split_row};

/// Askpass tags of forwards start with this (file panes use `<tab>:<side>`).
pub const TAG_PREFIX: &str = "fwd:";

pub struct Running {
    pub fwd: Forward,
    pub handle: ForwardHandle,
    pub tag: String,
    pub recorded: bool,
}

#[derive(Default)]
pub struct FwdForm {
    /// Index into `Config::forwards` when editing.
    pub editing: Option<usize>,
    pub host: usize,
    pub kind: usize,
    pub name: String,
    pub bind: String,
    pub listen: String,
    pub target: String,
    pub error: Option<String>,
}

pub struct FwdPrompt {
    pub tag: String,
    pub text: String,
    pub kind: AskKind,
    pub input: String,
    reply: Option<tokio::sync::oneshot::Sender<Option<String>>>,
}

impl FwdPrompt {
    fn answer(&mut self, a: Option<String>) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(a);
        }
    }
}

impl Drop for FwdPrompt {
    fn drop(&mut self) {
        self.answer(None);
    }
}

#[derive(Default)]
pub struct ForwardsState {
    pub running: Vec<Running>,
    pub form: Option<FwdForm>,
    pub prompt: Option<FwdPrompt>,
    next_tag: u64,
}

impl ForwardsState {
    pub fn state_of(&self, f: &Forward) -> Option<ForwardState> {
        self.running.iter().rev().find(|r| &r.fwd == f).map(|r| r.handle.state())
    }

    pub fn ask(&mut self, req: AskRequest) {
        self.prompt = Some(FwdPrompt { tag: req.tag, text: req.prompt, kind: req.kind, input: String::new(), reply: Some(req.reply) });
    }

    pub fn stop(&mut self, f: &Forward) {
        for r in self.running.iter().filter(|r| &r.fwd == f) {
            r.handle.stop();
        }
        if self.prompt.as_ref().is_some_and(|p| self.running.iter().any(|r| &r.fwd == f && r.tag == p.tag)) {
            self.prompt = None;
        }
    }

    pub fn stop_all(&mut self) {
        self.prompt = None;
        self.running.clear(); // dropping a handle stops its ssh
    }

    pub fn new_tag(&mut self) -> String {
        self.next_tag += 1;
        format!("{TAG_PREFIX}{}", self.next_tag)
    }

    /// The form for a new forward on host `host` (index into the host list).
    pub fn new_form(&mut self, host: usize) {
        self.form = Some(FwdForm { host, target: "localhost:".into(), ..Default::default() });
    }
}

fn form_to_forward(f: &FwdForm, hosts: &[String]) -> Result<Forward, String> {
    let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    let listen = f.listen.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or("Listen port must be 1\u{2013}65535")?;
    let kind = ForwardKind::ALL[f.kind.min(2)];
    let fwd = Forward {
        name: opt(&f.name),
        host: hosts.get(f.host).cloned().unwrap_or_default(),
        kind,
        bind: opt(&f.bind),
        listen,
        target: if kind == ForwardKind::Dynamic { None } else { opt(&f.target) },
    };
    fwd.validate()?;
    Ok(fwd)
}

pub fn view(ui: &mut Ui, rect: Rect, app: &mut GuiApp) {
    let mut st = std::mem::take(&mut app.forwards);
    let p = ui.style().palette;
    let body = rect.inset(ui.px(16));
    let mut rows = Rows::new(body);
    let head = rows.next(ui);
    let hp = ui.split_h(head, &[L::Fill, L::Px(130)], 8);
    label(ui, hp[0], "Port forwards", None);
    if button(ui, Id::new("fwd-new"), hp[1], "New forward").clicked {
        let h = app.sidebar_selected().unwrap_or(0);
        st.new_form(h);
    }

    if let Some(pr) = &mut st.prompt {
        let r = rows.next(ui);
        label(ui, r, &pr.text, Some(p.warning));
        let r = rows.next(ui);
        let parts = ui.split_h(r, &[L::Fill, L::Px(80), L::Px(80)], 6);
        let mut done = None;
        if pr.kind == AskKind::Confirm {
            if button(ui, Id::new("fwd-ask-yes"), parts[1], "Yes").clicked {
                done = Some(Some("yes".to_string()));
            }
            if button(ui, Id::new("fwd-ask-no"), parts[2], "No").clicked {
                done = Some(None);
            }
        } else {
            let t = text_input(ui, Id::new("fwd-ask-input"), parts[0], &mut pr.input, TextInputOpts { password: true, placeholder: "Password", ..Default::default() });
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

    let host_names: Vec<String> = app.hosts.iter().map(|h| h.name.clone()).collect();
    if let Some(form) = &mut st.form {
        let mut save = false;
        let mut cancel = false;
        let (l, c) = split_row(ui, rows.next(ui));
        label(ui, l, "Host", None);
        let opts: Vec<&str> = host_names.iter().map(String::as_str).collect();
        dropdown(ui, Id::new("fwd-host"), c, &opts, &mut form.host);
        let (l, c) = split_row(ui, rows.next(ui));
        label(ui, l, "Kind", None);
        let kinds: Vec<&str> = ForwardKind::ALL.iter().map(|k| k.label()).collect();
        dropdown(ui, Id::new("fwd-kind"), c, &kinds, &mut form.kind);
        let kind = ForwardKind::ALL[form.kind.min(2)];
        let (_, c) = split_row(ui, rows.next_h(ui, 40));
        label(ui, c, kind.explain(), Some(p.muted));
        field(ui, Id::new("fwd-name"), rows.next(ui), "Name", &mut form.name);
        field(ui, Id::new("fwd-bind"), rows.next(ui), "Bind address", &mut form.bind);
        field(ui, Id::new("fwd-listen"), rows.next(ui), "Listen port", &mut form.listen);
        if kind != ForwardKind::Dynamic {
            field(ui, Id::new("fwd-target"), rows.next(ui), "Target", &mut form.target);
        }
        let (_, c) = split_row(ui, rows.next(ui));
        let b = ui.split_h(c, &[L::Px(140), L::Px(90)], 8);
        if button(ui, Id::new("fwd-save"), b[0], "Save and start").clicked {
            save = true;
        }
        if button(ui, Id::new("fwd-cancel"), b[1], "Cancel").clicked {
            cancel = true;
        }
        error_line(ui, rows.next(ui), &form.error);
        if save {
            match form_to_forward(form, &host_names) {
                Err(e) => form.error = Some(e),
                Ok(f) => {
                    match form.editing {
                        Some(i) if i < app.cfg.forwards.len() => {
                            let old = app.cfg.forwards[i].clone();
                            st.stop(&old);
                            app.cfg.forwards[i] = f.clone();
                        }
                        _ => app.cfg.forwards.push(f.clone()),
                    }
                    app.save_config();
                    st.form = None;
                    app.start_forward(&mut st, &f);
                }
            }
        } else if cancel {
            st.form = None;
        }
    }

    rows.next_h(ui, 8);
    if app.cfg.forwards.is_empty() && st.form.is_none() {
        label(ui, rows.next(ui), "No saved forwards. Right-click a host and choose Port forward\u{2026}", Some(p.muted));
    }
    let mut act: Option<(usize, &str)> = None;
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
        if button(ui, Id::new(("fwd-run", i)), parts[2], if live { "Stop" } else { "Start" }).clicked {
            act = Some((i, if live { "stop" } else { "start" }));
        }
        if button(ui, Id::new(("fwd-edit", i)), parts[3], "Edit").clicked {
            act = Some((i, "edit"));
        }
        if button(ui, Id::new(("fwd-del", i)), parts[4], "Delete").clicked {
            act = Some((i, "delete"));
        }
    }
    if let Some((i, what)) = act {
        let f = app.cfg.forwards[i].clone();
        match what {
            "start" => app.start_forward(&mut st, &f),
            "stop" => st.stop(&f),
            "edit" => {
                let host = host_names.iter().position(|h| *h == f.host).unwrap_or(0);
                st.form = Some(FwdForm {
                    editing: Some(i),
                    host,
                    kind: ForwardKind::ALL.iter().position(|k| *k == f.kind).unwrap_or(0),
                    name: f.name.clone().unwrap_or_default(),
                    bind: f.bind.clone().unwrap_or_default(),
                    listen: f.listen.to_string(),
                    target: f.target.clone().unwrap_or_default(),
                    error: None,
                });
            }
            _ => {
                st.stop(&f);
                app.cfg.forwards.remove(i);
                app.save_config();
            }
        }
    }
    app.forwards = st;
}
