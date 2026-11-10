//! One file pane: location picker, path bar, actions, listing.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use termax_core::askpass::{AskKind, AskRequest};
use termax_core::fs::endpoint::Endpoint;
use termax_core::sshver::supports_askpass_require;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use termax_core::fs::path::is_safe_name;
use termax_core::fs::{FileEntry, human_size, parent};
use termax_ui::geom::{Point, Rect};
use termax_ui::input::{Key, Modifiers};
use termax_ui::layout::Size as L;
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Sense, Ui};
use termax_ui::widgets::{
    Selection, TextInputOpts, button, checkbox, dropdown, elide, label, select_list_named, text_input,
};

use super::ops::{self, Op};
use super::transfers::FileDrag;
use super::{FilesCtx, FilesEvent, Location};

const ROW_H: i32 = 24;

#[derive(Debug, Clone, PartialEq)]
pub enum PaneState {
    Idle,
    Loading,
    Connecting,
    Error(String),
}

/// An inline bar that replaces the action row until answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Confirm {
    /// (path, is_dir, name)
    Delete(Vec<(String, bool, String)>),
    Rename { from: String, input: String },
    NewFolder { input: String },
}

/// An ssh question shown inline in the pane that is connecting.
pub struct Prompt {
    /// The prompt exactly as ssh wrote it.
    pub text: String,
    pub kind: AskKind,
    input: String,
    reply: Option<oneshot::Sender<Option<String>>>,
    cancelled: CancellationToken,
    focus: bool,
}

impl Prompt {
    fn answer(&mut self, a: Option<String>) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(a);
        }
        self.input.clear();
    }
}

impl Drop for Prompt {
    fn drop(&mut self) {
        self.answer(None); // an unanswered prompt is a cancel
    }
}

pub struct Pane {
    pub loc: Location,
    pub ep: Option<Endpoint>,
    pub cwd: String,
    entries: Vec<FileEntry>,
    pub sel: Selection,
    pub state: PaneState,
    pub show_hidden: bool,
    pub path_edit: Option<String>,
    pub confirm: Option<Confirm>,
    /// Problem from the last file operation (the listing is still valid).
    pub banner: Option<String>,
    /// Bumped per listing request; older answers are dropped.
    pub seq: u64,
    /// Focus the text field of a just-opened bar next frame.
    focus_input: bool,
    /// A password / host-key question from ssh.
    pub prompt: Option<Prompt>,
    /// The connection being made; aborting it kills ssh.
    connecting: Option<tokio::task::AbortHandle>,
}

impl Drop for Pane {
    fn drop(&mut self) {
        self.abort_connect();
    }
}

/// `YYYY-MM-DD HH:MM` (UTC).
pub fn format_time(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn sort_entries(v: &mut [FileEntry]) {
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

impl Pane {
    pub fn new(loc: Location) -> Self {
        Pane {
            loc,
            ep: None,
            cwd: String::new(),
            entries: vec![],
            sel: Selection::default(),
            state: PaneState::Idle,
            show_hidden: false,
            path_edit: None,
            confirm: None,
            banner: None,
            seq: 0,
            focus_input: false,
            prompt: None,
            connecting: None,
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.loc, Location::Host(_))
    }

    /// Entries as shown (hidden files filtered), dirs first.
    pub fn visible(&self) -> Vec<&FileEntry> {
        self.entries.iter().filter(|e| self.show_hidden || !e.name.starts_with('.')).collect()
    }

    /// Replaces the listing, keeping the same files selected.
    fn reselect(&mut self, entries: Vec<FileEntry>) {
        let name_at = |vis: &[&FileEntry], i: Option<usize>| i.and_then(|i| vis.get(i)).map(|e| e.name.clone());
        let vis = self.visible();
        let names: Vec<String> = self.sel.iter().filter_map(|i| vis.get(i).map(|e| e.name.clone())).collect();
        let (anchor, cursor) = (name_at(&vis, self.sel.anchor), name_at(&vis, self.sel.cursor));
        self.entries = entries;
        let vis = self.visible();
        let index_of = |n: &String| vis.iter().position(|e| &e.name == n);
        let set = names.iter().filter_map(index_of).collect();
        let (anchor, cursor) = (anchor.as_ref().and_then(index_of), cursor.as_ref().and_then(index_of));
        self.sel.set = set;
        self.sel.anchor = anchor;
        self.sel.cursor = cursor;
    }

    /// Full paths of the selected entries, with `is_dir`.
    pub fn selected(&self) -> Vec<(String, bool, String)> {
        let vis = self.visible();
        self.sel
            .iter()
            .filter_map(|i| vis.get(i))
            .map(|e| (self.join(&e.name), e.is_dir, e.name.clone()))
            .collect()
    }

    pub fn join(&self, name: &str) -> String {
        termax_core::fs::join(self.is_remote(), &self.cwd, name)
    }

    /// Lists `path`; the pane switches to it when the listing arrives.
    pub fn navigate(&mut self, path: String, ctx: &FilesCtx, side: usize) {
        let Some(ep) = self.ep.clone() else { return };
        self.seq += 1;
        self.state = PaneState::Loading;
        ops::list(ctx.engine, ctx.tab_id, side, self.seq, ep, path);
    }

    pub fn refresh(&mut self, ctx: &FilesCtx, side: usize) {
        let cwd = self.cwd.clone();
        self.navigate(cwd, ctx, side);
    }

    pub fn go_up(&mut self, ctx: &FilesCtx, side: usize) {
        if let Some((p, _)) = parent(self.is_remote(), &self.cwd) {
            self.navigate(p, ctx, side);
        }
    }

    /// Switches this pane to another location (connects in the background).
    pub fn set_location(&mut self, loc: Location) {
        self.abort_connect();
        self.prompt = None;
        let (show_hidden, seq) = (self.show_hidden, self.seq + 1);
        *self = Pane::new(loc);
        self.show_hidden = show_hidden;
        self.seq = seq;
    }

    fn abort_connect(&mut self) {
        if let Some(h) = self.connecting.take() {
            h.abort();
        }
    }

    /// The user gave up on connecting (cancelled a prompt): stop ssh.
    pub fn cancel_connect(&mut self) {
        self.abort_connect();
        self.prompt = None;
        self.seq += 1; // ignore anything the dying attempt still sends
        self.state = PaneState::Error("connection cancelled".into());
    }

    fn start(&mut self, ctx: &FilesCtx, side: usize) {
        match &self.loc {
            Location::Local => {
                self.ep = Some(Endpoint::Local);
                self.navigate(ctx.start_dir.to_string(), ctx, side);
            }
            Location::Host(h) => {
                let can_prompt = ctx.askpass.is_some() && ctx.ssh_version.is_none_or(supports_askpass_require);
                let mut launch = h.sftp_launch(ctx.ssh_binary, !can_prompt);
                if can_prompt && let Some(b) = ctx.askpass {
                    launch.env.extend(b.env(&super::tag(ctx.tab_id, side)));
                }
                let note = ctx.ssh_version.filter(|v| !supports_askpass_require(*v)).map(|(a, b)| {
                    format!("termax needs OpenSSH 8.4 or newer for password prompts (found {a}.{b})")
                });
                self.seq += 1;
                self.state = PaneState::Connecting;
                self.connecting = Some(ops::connect(ctx.engine, ctx.tab_id, side, self.seq, launch, note));
            }
        }
    }

    /// Shows an ssh question in this pane.
    pub fn ask(&mut self, req: AskRequest) {
        if self.connecting.is_none() {
            let _ = req.reply.send(None); // not connecting (any more)
            return;
        }
        let AskRequest { prompt, kind, reply, cancelled, .. } = req;
        self.prompt = Some(Prompt { text: prompt, kind, input: String::new(), reply: Some(reply), cancelled, focus: true });
    }

    pub fn on_event(&mut self, ev: FilesEvent, ctx: &FilesCtx, side: usize) {
        match ev {
            FilesEvent::Listed { seq, cwd, result } => {
                if seq != self.seq {
                    return; // answers an older request
                }
                match result {
                    Ok(mut entries) => {
                        sort_entries(&mut entries);
                        if cwd != self.cwd {
                            self.sel.clear();
                            self.entries = entries;
                        } else {
                            // Same folder refreshed: rows may have moved, so
                            // the selection follows names, not row numbers.
                            self.reselect(entries);
                        }
                        self.cwd = cwd;
                        self.state = PaneState::Idle;
                    }
                    Err(e) => self.state = PaneState::Error(format!("{cwd}: {e}")),
                }
            }
            FilesEvent::Connected { seq, result } => {
                if seq != self.seq {
                    return;
                }
                self.prompt = None;
                self.connecting = None;
                match result {
                    Ok(conn) => {
                        let home = conn.home.clone();
                        self.ep = Some(Endpoint::Remote(Arc::new(conn)));
                        self.navigate(home, ctx, side);
                    }
                    Err(e) => self.state = PaneState::Error(e),
                }
            }
            FilesEvent::OpDone { result } => {
                if let Err(e) = result {
                    self.banner = Some(e);
                }
                self.refresh(ctx, side);
            }
        }
    }

    fn run(&mut self, op: Op, ctx: &FilesCtx, side: usize) {
        if let Some(ep) = self.ep.clone() {
            self.banner = None;
            ops::run(ctx.engine, ctx.tab_id, side, ep, op);
        }
    }

    fn ask_delete(&mut self) {
        let items = self.selected();
        if !items.is_empty() {
            self.confirm = Some(Confirm::Delete(items));
        }
    }

    fn ask_rename(&mut self) {
        if let [(path, _, name)] = self.selected().as_slice() {
            self.confirm = Some(Confirm::Rename { from: path.clone(), input: name.clone() });
            self.focus_input = true;
        }
    }
}

/// What a pane asks its tab to do.
#[derive(Default)]
pub struct PaneOut {
    /// Copy the selection to the other pane.
    pub copy: bool,
    /// Rows dragged from a pane were dropped here.
    pub dropped: Option<FileDrag>,
}

/// Draws one pane and handles its input.
pub fn view(ui: &mut Ui, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx) -> PaneOut {
    let mut out = PaneOut::default();
    let p = ui.style().palette;
    let id = Id::new(("pane", ctx.tab_id, side));
    if pane.ep.is_none() && pane.state == PaneState::Idle {
        pane.start(ctx, side);
    }
    let banner = match &pane.state {
        PaneState::Error(e) => Some(e.clone()),
        _ => pane.banner.clone(),
    };
    let rows = ui.split_v(
        rect,
        &[L::Px(30), L::Px(30), L::Px(if banner.is_some() { 30 } else { 0 }), L::Fill],
        6,
    );
    let (top, actions, banner_r, list_r) = (rows[0], rows[1], rows[2], rows[3]);

    // Row 1: location picker + path.
    let top_parts = ui.split_h(top, &[L::Px(160), L::Fill], 6);
    let mut options: Vec<&str> = vec!["Local"];
    options.extend(ctx.hosts.iter().map(|h| h.name.as_str()));
    let mut sel_loc = match &pane.loc {
        Location::Local => 0,
        Location::Host(h) => ctx.hosts.iter().position(|x| x.name == h.name).map_or(0, |i| i + 1),
    };
    if dropdown(ui, id.with("loc"), top_parts[0], &options, &mut sel_loc) {
        let loc = if sel_loc == 0 { Location::Local } else { Location::Host(ctx.hosts[sel_loc - 1].clone()) };
        pane.set_location(loc);
    }
    path_bar(ui, id, top_parts[1], pane, side, ctx);

    // Row 2: actions, or the open inline bar.
    if pane.confirm.is_some() {
        confirm_bar(ui, id, actions, pane, side, ctx);
    } else {
        let parts = ui.split_h(actions, &[L::Px(48), L::Px(72), L::Px(96), L::Px(72), L::Px(64), L::Px(80), L::Fill], 6);
        let can_up = parent(pane.is_remote(), &pane.cwd).is_some();
        if button(ui, id.with("up"), parts[0], "Up").clicked && can_up {
            pane.go_up(ctx, side);
        }
        if button(ui, id.with("refresh"), parts[1], "Refresh").clicked {
            pane.refresh(ctx, side);
        }
        if button(ui, id.with("mkdir"), parts[2], "New folder").clicked && pane.ep.is_some() {
            pane.confirm = Some(Confirm::NewFolder { input: String::new() });
            pane.focus_input = true;
        }
        if button(ui, id.with("rename"), parts[3], "Rename").clicked {
            pane.ask_rename();
        }
        if button(ui, id.with("delete"), parts[4], "Delete").clicked {
            pane.ask_delete();
        }
        let copy_label = if side == 0 { "Copy \u{2192}" } else { "\u{2190} Copy" };
        if button(ui, id.with("copy"), parts[5], copy_label).clicked {
            out.copy = true;
        }
        let hidden_r = Rect::new(parts[6].x + ui.px(8), parts[6].y, parts[6].w - ui.px(8), parts[6].h);
        if checkbox(ui, id.with("hidden"), hidden_r, &mut pane.show_hidden, "Hidden").clicked {
            pane.sel.clear();
        }
    }

    if let Some(msg) = banner {
        let radius = ui.px(4) as f32;
        ui.painter().rect(banner_r, p.danger.with_alpha(40), radius);
        let disconnected = pane.ep.is_none() && pane.is_remote();
        // A connected remote pane can still lose its connection (server
        // restart, timeout): offer to connect again next to Dismiss.
        let reconnect = pane.ep.is_some() && pane.is_remote();
        let parts = ui.split_h(banner_r, &[L::Fill, L::Px(if reconnect { 96 } else { 0 }), L::Px(80)], 6);
        label(ui, Rect::new(parts[0].x + ui.px(8), parts[0].y, parts[0].w - ui.px(8), parts[0].h), &msg, Some(p.danger));
        if reconnect && button(ui, id.with("reconnect"), parts[1], "Reconnect").clicked {
            let loc = pane.loc.clone();
            pane.set_location(loc);
        } else if disconnected {
            if button(ui, id.with("retry"), parts[2], "Retry").clicked {
                let loc = pane.loc.clone();
                pane.set_location(loc);
            }
        } else if button(ui, id.with("dismiss"), parts[2], "Dismiss").clicked {
            pane.banner = None;
            if matches!(pane.state, PaneState::Error(_)) {
                pane.state = PaneState::Idle;
                if pane.ep.is_none() {
                    pane.state = PaneState::Error("not connected".into());
                }
            }
        }
    }

    if pane.prompt.as_ref().is_some_and(|p| p.cancelled.is_cancelled()) {
        pane.prompt = None;
    }
    if pane.prompt.is_some() {
        prompt_card(ui, id, list_r, pane);
    } else {
        listing(ui, id, list_r, pane, side, ctx, &mut out);
    }
    out
}

/// The inline ssh question: prompt text, masked input (or Yes/No), OK/Cancel.
fn prompt_card(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane) {
    let p = ui.style().palette;
    let Some(pr) = pane.prompt.as_mut() else { return };
    let radius = ui.px(6) as f32;
    ui.painter().rect(rect, p.surface, radius);
    let pad = ui.px(14);
    let px = ui.font_px();
    let lh = ui.line_height(FontKind::Ui, px) + ui.px(4);
    let mut y = rect.y + pad;
    let title = format!("{} asks:", pane.loc.label());
    ui.painter().text(Point::new(rect.x + pad, y), &title, FontKind::Ui, px, p.muted);
    y += lh + ui.px(4);
    for line in pr.text.lines().filter(|l| !l.trim().is_empty()) {
        let shown = elide(ui, line.trim_end(), FontKind::Ui, px, rect.w - 2 * pad);
        ui.painter().text(Point::new(rect.x + pad, y), &shown, FontKind::Ui, px, p.fg);
        y += lh;
    }
    y += ui.px(6);
    let row = Rect::new(rect.x + pad, y, (rect.w - 2 * pad).min(ui.px(520)), ui.px(30));
    let iid = id.with("prompt-input");
    let mut answer: Option<Option<String>> = None;
    match pr.kind {
        AskKind::Secret => {
            let parts = ui.split_h(row, &[L::Fill, L::Px(70), L::Px(80)], 6);
            if pr.focus {
                ui.request_focus(iid);
                pr.focus = false;
            }
            let r = text_input(ui, iid, parts[0], &mut pr.input, TextInputOpts { password: true, placeholder: "Password", ..Default::default() });
            if button(ui, id.with("prompt-ok"), parts[1], "OK").clicked || r.submitted {
                answer = Some(Some(pr.input.clone()));
            }
            if button(ui, id.with("prompt-cancel"), parts[2], "Cancel").clicked {
                answer = Some(None);
            }
        }
        AskKind::Confirm => {
            let parts = ui.split_h(row, &[L::Px(80), L::Px(80), L::Px(80), L::Fill], 6);
            if button(ui, id.with("prompt-yes"), parts[0], "Yes").clicked {
                answer = Some(Some("yes".into()));
            }
            if button(ui, id.with("prompt-no"), parts[1], "No").clicked {
                answer = Some(Some("no".into()));
            }
            if button(ui, id.with("prompt-cancel"), parts[2], "Cancel").clicked {
                answer = Some(None);
            }
        }
    }
    match answer {
        Some(None) => pane.cancel_connect(),
        Some(a) => {
            if let Some(pr) = pane.prompt.as_mut() {
                pr.answer(a);
            }
            pane.prompt = None;
        }
        None => {}
    }
}

fn path_bar(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx) {
    let p = ui.style().palette;
    let pid = id.with("path");
    if let Some(mut text) = pane.path_edit.take() {
        if pane.focus_input {
            ui.request_focus(pid);
            pane.focus_input = false;
        }
        let r = text_input(ui, pid, rect, &mut text, TextInputOpts { password: false, placeholder: "Go to path", ..Default::default() });
        if r.submitted {
            pane.navigate(text, ctx, side);
            return;
        }
        if ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) || !ui.has_focus(pid) {
            return; // cancelled (Esc, or focus moved elsewhere)
        }
        pane.path_edit = Some(text);
        return;
    }
    // Focusable: Enter or Space (or a screen reader's Click) edits the path.
    let mut r = ui.interact(pid, rect, Sense::focusable());
    if r.focused && ui.keys_for(pid) {
        let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
        r.clicked |= enter || ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
    }
    ui.a11y_node(pid, termax_ui::a11y::Role::Button, &format!("Path {}, edit", pane.cwd));
    let radius = ui.px(4) as f32;
    ui.painter().rect(rect, if r.hovered { p.surface_alt } else { p.surface }, radius);
    let px = ui.font_px();
    let pad = ui.px(8);
    let shown = elide(ui, &pane.cwd, FontKind::Ui, px, rect.w - 2 * pad);
    let lh = ui.line_height(FontKind::Ui, px);
    let suffix = match pane.state {
        PaneState::Loading => "  \u{2026}",
        PaneState::Connecting => "  connecting\u{2026}",
        _ => "",
    };
    let at = Point::new(rect.x + pad, rect.y + (rect.h - lh) / 2);
    let w = ui.painter().text(at, &shown, FontKind::Ui, px, p.fg);
    if !suffix.is_empty() {
        ui.painter().text(Point::new(at.x + w, at.y), suffix, FontKind::Ui, px, p.muted);
    }
    if r.clicked && pane.ep.is_some() {
        pane.path_edit = Some(pane.cwd.clone());
        pane.focus_input = true;
    }
}

fn confirm_bar(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx) {
    let p = ui.style().palette;
    let Some(confirm) = pane.confirm.clone() else { return };
    let parts = ui.split_h(rect, &[L::Fill, L::Px(80), L::Px(80)], 6);
    let iid = id.with("bar-input");
    let mut done = false;
    match confirm {
        Confirm::Delete(items) => {
            let text = match items.as_slice() {
                [(_, _, name)] => format!("Delete {name}?"),
                _ => format!("Delete {} items?", items.len()),
            };
            label(ui, parts[0], &text, Some(p.danger));
            if button(ui, id.with("bar-ok"), parts[1], "Delete").clicked {
                pane.run(Op::Delete(items.into_iter().map(|(p, d, _)| (p, d)).collect()), ctx, side);
                done = true;
            }
        }
        Confirm::Rename { from, mut input } => {
            if pane.focus_input {
                ui.request_focus(iid);
                pane.focus_input = false;
            }
            let r = text_input(ui, iid, parts[0], &mut input, TextInputOpts { password: false, placeholder: "New name", ..Default::default() });
            let ok = button(ui, id.with("bar-ok"), parts[1], "Rename").clicked || r.submitted;
            if ok && is_safe_name(input.trim()) {
                let to = pane.join(input.trim());
                if to != from {
                    pane.run(Op::Rename { from: from.clone(), to }, ctx, side);
                }
                done = true;
            } else {
                pane.confirm = Some(Confirm::Rename { from, input });
            }
        }
        Confirm::NewFolder { mut input } => {
            if pane.focus_input {
                ui.request_focus(iid);
                pane.focus_input = false;
            }
            let r = text_input(ui, iid, parts[0], &mut input, TextInputOpts { password: false, placeholder: "New folder name", ..Default::default() });
            let ok = button(ui, id.with("bar-ok"), parts[1], "Create").clicked || r.submitted;
            if ok && is_safe_name(input.trim()) {
                let path = pane.join(input.trim());
                pane.run(Op::Mkdir(path), ctx, side);
                done = true;
            } else {
                pane.confirm = Some(Confirm::NewFolder { input });
            }
        }
    }
    if button(ui, id.with("bar-cancel"), parts[2], "Cancel").clicked || ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) {
        done = true;
    }
    if done {
        pane.confirm = None;
        ui.request_focus(id.with("list"));
    }
}

fn listing(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx, out: &mut PaneOut) {
    let p = ui.style().palette;
    let radius = ui.px(4) as f32;
    ui.painter().rect(rect, p.surface, radius);
    let lid = id.with("list");
    let vis: Vec<FileEntry> = pane.visible().into_iter().cloned().collect();
    let px = ui.font_px();
    let list_name = format!("{} files", pane.loc.label());
    let row_name = |i: usize| {
        let e = &vis[i];
        let kind = if e.is_dir { "folder".to_string() } else { human_size(e.size) };
        match e.modified {
            Some(t) => format!("{}, {kind}, modified {}", e.name, format_time(t)),
            None => format!("{}, {kind}", e.name),
        }
    };
    let r = select_list_named(ui, lid, rect, &list_name, vis.len(), ROW_H, &mut pane.sel, row_name, |ui, row, i, _| {
        let e = &vis[i];
        let lh = ui.line_height(FontKind::Ui, px);
        let y = row.y + (row.h - lh) / 2;
        // icon: a small folder or page shape
        let s = ui.px(12);
        let icon = Rect::new(row.x + ui.px(8), row.y + (row.h - s) / 2, s, s - ui.px(2));
        let (r2, bw) = (ui.px(2) as f32, ui.px(1).max(1));
        if e.is_dir {
            ui.painter().rect(icon, p.accent.with_alpha(200), r2);
        } else {
            ui.painter().border(icon, p.muted, bw, r2);
        }
        let time_w = ui.px(130);
        let size_w = ui.px(70);
        let name_w = (row.w - ui.px(28) - time_w - size_w - ui.px(16)).max(ui.px(20));
        let name = elide(ui, &e.name, FontKind::Ui, px, name_w);
        let at = Point::new(row.x + ui.px(28), y);
        ui.painter().text(at, &name, FontKind::Ui, px, p.fg);
        if !e.is_dir {
            let sz = human_size(e.size);
            let w = ui.measure(&sz, FontKind::Ui, px).ceil() as i32;
            let at = Point::new(row.right() - time_w - ui.px(8) - w, y);
            ui.painter().text(at, &sz, FontKind::Ui, px, p.muted);
        }
        if row.w > ui.px(360)
            && let Some(t) = e.modified
        {
            let at = Point::new(row.right() - time_w, y);
            ui.painter().text(at, &format_time(t), FontKind::Ui, px, p.muted);
        }
    });
    if r.drag_started {
        let items = pane.selected();
        let label = match items.as_slice() {
            [(_, _, name)] => name.clone(),
            many => format!("{} items", many.len()),
        };
        if !items.is_empty() {
            ui.start_drag(Box::new(FileDrag { tab: ctx.tab_id, side, items }), label);
        }
    }
    let drop = ui.drop_target::<FileDrag>(rect);
    if drop.hovered {
        let bw = ui.px(2);
        ui.painter().border(rect, p.accent, bw, radius);
    }
    if let Some(d) = drop.dropped {
        out.dropped = Some(d);
    }
    if let Some(i) = r.activated
        && let Some(e) = vis.get(i)
        && e.is_dir
    {
        let path = pane.join(&e.name);
        pane.navigate(path, ctx, side);
    }
    if ui.has_focus(lid) && ui.keys_for(lid) {
        if ui.input_mut().take_key(&Key::Backspace, Modifiers::NONE) {
            pane.go_up(ctx, side);
        }
        if ui.input_mut().take_key(&Key::F(2), Modifiers::NONE) {
            pane.ask_rename();
        }
        if ui.input_mut().take_key(&Key::Delete, Modifiers::NONE) {
            pane.ask_delete();
        }
        if ui.input_mut().take_key(&Key::F(5), Modifiers::NONE) {
            out.copy = true;
        }
    }
    if vis.is_empty() && pane.state == PaneState::Idle {
        let at = Point::new(rect.x + ui.px(12), rect.y + ui.px(10));
        ui.painter().text(at, "Empty folder", FontKind::Ui, px, p.muted);
    }
}
