//! One file pane of a files tab, drawn as a card: location picker, Up /
//! Refresh and a breadcrumb path bar; a grouped toolbar (or an inline name
//! bar for Rename / New folder); an error banner; a sortable table of the
//! folder (Name, Size, Modified) with type-ahead select; and a footer with
//! item, selection and size counts. Delete asks in a modal dialog. While
//! connecting, an inline ssh prompt replaces the table.

use std::cmp::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use skiff_core::askpass::{AskKind, AskRequest};
use skiff_core::fs::endpoint::Endpoint;
use skiff_core::fs::path::is_safe_name;
use skiff_core::fs::{FileEntry, human_size, parent};
use skiff_core::sshver::supports_askpass_require;
use skiff_ui::a11y::{Role, SortDir};
use skiff_ui::geom::{Point, Rect};
use skiff_ui::icons::Icon;
use skiff_ui::input::{CursorIcon, Event, Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::text::FontKind;
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::{
    Align, ButtonKind, ButtonOpts, Column, Dialog, DialogButton, DialogResult, Elide, EmptyState, Selection, Sort, TextInputOpts, TextOpts,
    Tone, button, button_width, button_with, dialog, dropdown_labeled, elide, empty_state, icon_button, scroll_into_view,
    select_list_named, table_header, text, text_input, tone_colors,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::ops::{self, Op};
use super::transfers::FileDrag;
use super::{FilesCtx, FilesEvent, Location};

/// Column indices of the listing's table header.
pub const COL_NAME: usize = 0;
/// The Size column.
pub const COL_SIZE: usize = 1;
/// The Modified column.
pub const COL_MODIFIED: usize = 2;

/// Letters typed within this long of each other extend one type-ahead search.
pub const TYPE_AHEAD_RESET: Duration = Duration::from_millis(1000);

/// Height of the pane footer, logical px.
const FOOTER_H: i32 = 26;

/// What a pane is doing right now.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneState {
    /// Nothing pending (connects on next draw if there is no endpoint yet).
    Idle,
    /// A listing request is in flight.
    Loading,
    /// The SFTP connection is being made.
    Connecting,
    /// Connecting or listing failed; shown in the banner.
    Error(String),
}

/// A question open over the pane: a delete dialog, or an inline name bar
/// that replaces the toolbar until answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Confirm {
    /// (path, is_dir, name)
    Delete(Vec<(String, bool, String)>),
    /// Rename the entry at full path `from`; `input` is the new name being typed.
    Rename { from: String, input: String },
    /// Create a folder; `input` is the name being typed.
    NewFolder { input: String },
}

/// An ssh question shown inline in the pane that is connecting.
pub struct Prompt {
    /// The prompt exactly as ssh wrote it.
    pub text: String,
    /// Password, yes/no, or a notice with nothing to answer.
    pub kind: AskKind,
    /// What the user has typed so far.
    input: String,
    /// Where the answer goes (`None` = cancel); taken once answered.
    reply: Option<oneshot::Sender<Option<String>>>,
    /// Set when ssh withdraws the question.
    cancelled: CancellationToken,
    /// Focus the input on the next frame.
    focus: bool,
}

impl Prompt {
    /// Sends `a` to ssh (once; later calls do nothing) and wipes the typed input.
    fn answer(&mut self, a: Option<String>) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(a);
        }
        self.input.clear();
    }
}

impl Drop for Prompt {
    /// Makes sure ssh is never left waiting for an answer.
    fn drop(&mut self) {
        self.answer(None); // an unanswered prompt is a cancel
    }
}

/// One side of a files tab.
pub struct Pane {
    /// Where this pane browses.
    pub loc: Location,
    /// The open connection (or Local); `None` until connected.
    pub ep: Option<Endpoint>,
    /// The folder currently listed.
    pub cwd: String,
    /// The listing of `cwd`, in `sort` order (folders first).
    entries: Vec<FileEntry>,
    /// Selected rows, as indices into `visible()`.
    pub sel: Selection,
    pub state: PaneState,
    /// Show dot-files.
    pub show_hidden: bool,
    /// The column and direction the listing is sorted by.
    pub sort: Option<Sort>,
    /// The path being typed while the path bar is in edit mode.
    pub path_edit: Option<String>,
    /// The open delete dialog or name bar.
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
    /// Letters typed for type-ahead select, and when the last one came.
    typed: String,
    typed_at: Option<Instant>,
}

impl Drop for Pane {
    /// Kills a connection attempt still running for this pane.
    fn drop(&mut self) {
        self.abort_connect();
    }
}

/// Formats `t` as `YYYY-MM-DD HH:MM` (UTC); times before 1970 show as the epoch.
pub fn format_time(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days: days since 1970-01-01 to (y, m, d),
    // using 400-year eras that start on 0000-03-01.
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

/// The listing's default order: by name, A to Z.
pub const NAME_ASC: Sort = Sort { column: COL_NAME, dir: SortDir::Ascending };

/// Sorts folders first, then by the `sort` column (name ignoring case
/// breaks ties; folders compare by name when sorting by size).
pub fn sort_entries(v: &mut [FileEntry], sort: Option<Sort>) {
    let s = sort.unwrap_or(NAME_ASC);
    let by_name = |a: &FileEntry, b: &FileEntry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    v.sort_by(|a, b| {
        let key = match s.column {
            COL_SIZE if !(a.is_dir && b.is_dir) => a.size.cmp(&b.size),
            COL_MODIFIED => a.modified.cmp(&b.modified),
            _ => Ordering::Equal,
        };
        let o = key.then_with(|| by_name(a, b));
        let o = if s.dir == SortDir::Descending { o.reverse() } else { o };
        b.is_dir.cmp(&a.is_dir).then(o)
    });
}

/// The breadcrumb segments of `cwd`: (label, full path) from the root
/// down. Remote paths split on `/`; local ones follow the OS's path rules.
pub fn crumbs(remote: bool, cwd: &str) -> Vec<(String, String)> {
    if cwd.is_empty() {
        return vec![];
    }
    if remote {
        let mut out = vec![];
        let mut path = String::new();
        if cwd.starts_with('/') {
            out.push(("/".to_string(), "/".to_string()));
            path.push('/');
        }
        for part in cwd.split('/').filter(|s| !s.is_empty()) {
            if !path.is_empty() && !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(part);
            out.push((part.to_string(), path.clone()));
        }
        return out;
    }
    let mut out: Vec<(String, String)> = std::path::Path::new(cwd)
        .ancestors()
        .filter(|a| !a.as_os_str().is_empty())
        .map(|a| {
            // The root keeps its own spelling ("C:\" shows as "C:", "/" as "/").
            let label = match a.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => {
                    let s = a.display().to_string();
                    let t = s.trim_end_matches(['\\', '/']);
                    if t.is_empty() { s } else { t.to_string() }
                }
            };
            (label, a.display().to_string())
        })
        .collect();
    out.reverse();
    out
}

/// The footer line: "N items", plus "· K selected · size of the selected
/// files" when something is selected, or "· size of all files" otherwise.
pub fn footer_text(pane: &Pane) -> String {
    let vis = pane.visible();
    let n = vis.len();
    let mut s = format!("{n} item{}", if n == 1 { "" } else { "s" });
    let files_size = |it: &mut dyn Iterator<Item = &&FileEntry>| it.filter(|e| !e.is_dir).map(|e| e.size).sum::<u64>();
    if pane.sel.is_empty() {
        let total = files_size(&mut vis.iter());
        if total > 0 {
            s.push_str(&format!(" \u{00B7} {}", human_size(total)));
        }
    } else {
        let picked: Vec<&&FileEntry> = pane.sel.iter().filter_map(|i| vis.get(i)).collect();
        s.push_str(&format!(" \u{00B7} {} selected", picked.len()));
        let total = files_size(&mut picked.into_iter());
        if total > 0 {
            s.push_str(&format!(" \u{00B7} {}", human_size(total)));
        }
    }
    s
}

impl Pane {
    /// An unconnected pane at `loc`; it connects the first time it is drawn.
    pub fn new(loc: Location) -> Self {
        Pane {
            loc,
            ep: None,
            cwd: String::new(),
            entries: vec![],
            sel: Selection::default(),
            state: PaneState::Idle,
            show_hidden: false,
            sort: Some(NAME_ASC),
            path_edit: None,
            confirm: None,
            banner: None,
            seq: 0,
            focus_input: false,
            prompt: None,
            connecting: None,
            typed: String::new(),
            typed_at: None,
        }
    }

    /// True for a host pane (remote path rules apply), false for Local.
    pub fn is_remote(&self) -> bool {
        matches!(self.loc, Location::Host(_))
    }

    /// Entries as shown (hidden files filtered), in sort order.
    pub fn visible(&self) -> Vec<&FileEntry> {
        self.entries.iter().filter(|e| self.show_hidden || !e.name.starts_with('.')).collect()
    }

    /// Replaces the listing, keeping the same files selected.
    fn reselect(&mut self, entries: Vec<FileEntry>) {
        // Remember the selected, anchor and cursor rows by name...
        let name_at = |vis: &[&FileEntry], i: Option<usize>| i.and_then(|i| vis.get(i)).map(|e| e.name.clone());
        let vis = self.visible();
        let names: Vec<String> = self.sel.iter().filter_map(|i| vis.get(i).map(|e| e.name.clone())).collect();
        let (anchor, cursor) = (name_at(&vis, self.sel.anchor), name_at(&vis, self.sel.cursor));
        self.entries = entries;
        // ...then map those names to rows of the new listing (gone ones drop out).
        let vis = self.visible();
        let index_of = |n: &String| vis.iter().position(|e| &e.name == n);
        let set = names.iter().filter_map(index_of).collect();
        let (anchor, cursor) = (anchor.as_ref().and_then(index_of), cursor.as_ref().and_then(index_of));
        self.sel.set = set;
        self.sel.anchor = anchor;
        self.sel.cursor = cursor;
    }

    /// Sorts the listing by `sort`; the same files stay selected.
    pub fn set_sort(&mut self, sort: Option<Sort>) {
        self.sort = sort;
        let mut e = self.entries.clone();
        sort_entries(&mut e, self.sort);
        self.reselect(e);
    }

    /// Type-ahead: adds `text` to the letters typed (starting over after
    /// `TYPE_AHEAD_RESET`) and selects the next visible entry whose name
    /// starts with them, ignoring case. Repeating one letter cycles through
    /// the entries starting with it. Returns the selected row.
    pub fn type_ahead(&mut self, text: &str, now: Instant) -> Option<usize> {
        if self.typed_at.is_none_or(|t| now.saturating_duration_since(t) > TYPE_AHEAD_RESET) {
            self.typed.clear();
        }
        self.typed_at = Some(now);
        self.typed.push_str(&text.to_lowercase());
        let vis = self.visible();
        let n = vis.len();
        if n == 0 {
            return None;
        }
        let mut chars = self.typed.chars();
        let first = chars.next()?;
        // "bbb" cycles through the b's; "bin" narrows to names starting "bin".
        let cycling = chars.all(|c| c == first);
        let needle = if cycling { first.to_string() } else { self.typed.clone() };
        let start = match (self.sel.cursor, cycling) {
            (Some(c), true) => c + 1,
            (Some(c), false) => c,
            (None, _) => 0,
        };
        let hit = (0..n).map(|k| (start + k) % n).find(|&i| vis[i].name.to_lowercase().starts_with(&needle))?;
        self.sel.select_only(hit);
        Some(hit)
    }

    /// The selected entries as (full path, is_dir, name).
    pub fn selected(&self) -> Vec<(String, bool, String)> {
        let vis = self.visible();
        self.sel.iter().filter_map(|i| vis.get(i)).map(|e| (self.join(&e.name), e.is_dir, e.name.clone())).collect()
    }

    /// Full path of `name` inside the current folder, using this pane's path style.
    pub fn join(&self, name: &str) -> String {
        skiff_core::fs::join(self.is_remote(), &self.cwd, name)
    }

    /// Lists `path`; the pane switches to it when the listing arrives.
    pub fn navigate(&mut self, path: String, ctx: &FilesCtx, side: usize) {
        let Some(ep) = self.ep.clone() else { return };
        self.seq += 1;
        self.state = PaneState::Loading;
        ops::list(ctx.engine, ctx.tab_id, side, self.seq, ep, path);
    }

    /// Lists the current folder again.
    pub fn refresh(&mut self, ctx: &FilesCtx, side: usize) {
        let cwd = self.cwd.clone();
        self.navigate(cwd, ctx, side);
    }

    /// Goes to the parent folder; does nothing at the root.
    pub fn go_up(&mut self, ctx: &FilesCtx, side: usize) {
        if let Some((p, _)) = parent(self.is_remote(), &self.cwd) {
            self.navigate(p, ctx, side);
        }
    }

    /// Switches this pane to another location (connects in the background).
    /// Resets everything but "show hidden" and the sort; `seq` keeps rising
    /// so answers for the old location are ignored.
    pub fn set_location(&mut self, loc: Location) {
        self.abort_connect();
        self.prompt = None;
        let (show_hidden, sort, seq) = (self.show_hidden, self.sort, self.seq + 1);
        *self = Pane::new(loc);
        self.show_hidden = show_hidden;
        self.sort = sort;
        self.seq = seq;
    }

    /// Aborts a running connection attempt (which kills its ssh process).
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

    /// Opens the pane's location: Local lists the start folder at once, a
    /// host starts an SFTP connection in the background.
    fn start(&mut self, ctx: &FilesCtx, side: usize) {
        match &self.loc {
            Location::Local => {
                self.ep = Some(Endpoint::Local);
                self.navigate(ctx.start_dir.to_string(), ctx, side);
            }
            Location::Host(h) => {
                // Prompts go through askpass only when the broker exists and
                // ssh supports SSH_ASKPASS_REQUIRE; otherwise run ssh in batch
                // mode and explain why if the version is too old.
                let can_prompt = ctx.askpass.is_some() && ctx.ssh_version.is_none_or(supports_askpass_require);
                let mut launch = h.sftp_launch(ctx.ssh_binary, !can_prompt);
                if can_prompt && let Some(b) = ctx.askpass {
                    launch.env.extend(b.env(&super::tag(ctx.tab_id, side)));
                }
                let note = ctx
                    .ssh_version
                    .filter(|v| !supports_askpass_require(*v))
                    .map(|(a, b)| format!("Skiff needs OpenSSH 8.4 or newer for password prompts (found {a}.{b})"));
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

    /// Applies a background result: a listing, a finished connection, or a
    /// finished file operation (which refreshes the listing).
    pub fn on_event(&mut self, ev: FilesEvent, ctx: &FilesCtx, side: usize) {
        match ev {
            FilesEvent::Listed { seq, cwd, result } => {
                if seq != self.seq {
                    return; // answers an older request
                }
                match result {
                    Ok(mut entries) => {
                        sort_entries(&mut entries, self.sort);
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
                    // The folder's name, not its whole path: the reason must fit.
                    Err(e) => {
                        let name = crumbs(self.is_remote(), &cwd).pop().map_or(cwd, |c| c.0);
                        self.state = PaneState::Error(format!("Can't open {name}: {e}"));
                    }
                }
            }
            FilesEvent::Connected { seq, result } => {
                if seq != self.seq {
                    return;
                }
                self.prompt = None;
                self.connecting = None;
                match result {
                    // Connected: start in the remote home folder.
                    Ok(conn) => {
                        let home = conn.home.clone();
                        self.ep = Some(Endpoint::Remote(Arc::new(*conn)));
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

    /// Starts `op` in the background (if connected), clearing an old banner.
    fn run(&mut self, op: Op, ctx: &FilesCtx, side: usize) {
        if let Some(ep) = self.ep.clone() {
            self.banner = None;
            ops::run(ctx.engine, ctx.tab_id, side, ep, op);
        }
    }

    /// Opens the delete dialog for the selection (if any).
    fn ask_delete(&mut self) {
        let items = self.selected();
        if !items.is_empty() {
            self.confirm = Some(Confirm::Delete(items));
        }
    }

    /// Opens the rename bar, prefilled with the name, when exactly one entry is selected.
    fn ask_rename(&mut self) {
        if let [(path, _, name)] = self.selected().as_slice() {
            self.confirm = Some(Confirm::Rename { from: path.clone(), input: name.clone() });
            self.focus_input = true;
        }
    }

    /// Opens the New folder bar (when connected).
    fn ask_new_folder(&mut self) {
        if self.ep.is_some() {
            self.confirm = Some(Confirm::NewFolder { input: String::new() });
            self.focus_input = true;
        }
    }

    /// Turns the path bar into a text field holding the current folder.
    fn edit_path(&mut self) {
        if self.ep.is_some() {
            self.path_edit = Some(self.cwd.clone());
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

/// Draws one pane as a card and handles its input.
pub fn view(ui: &mut Ui, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx) -> PaneOut {
    let mut out = PaneOut::default();
    let p = ui.style().palette;
    let id = Id::new(("pane", ctx.tab_id, side));
    // Not connected and nothing pending: connect now.
    if pane.ep.is_none() && pane.state == PaneState::Idle {
        pane.start(ctx, side);
    }
    // An error state wins over an operation banner.
    let banner = match &pane.state {
        PaneState::Error(e) => Some(e.clone()),
        _ => pane.banner.clone(),
    };
    let radius = tokens::RADIUS_MD * ui.scale();
    let bw = ui.px(1).max(1);
    {
        let mut painter = ui.painter();
        painter.rect(rect, p.surface, radius);
        painter.border(rect, p.border, bw, radius);
    }
    let pad = ui.px(tokens::SPACE_SM);
    let ctl = ui.px(tokens::CONTROL_H);
    let inner_x = rect.x + pad;
    let inner_w = rect.w - 2 * pad;
    let mut y = rect.y + pad;
    let mut next_row = |h: i32| {
        let r = Rect::new(inner_x, y, inner_w, h);
        y += h + pad;
        r
    };
    let nav = next_row(ctl);
    let tools = next_row(ctl);
    let banner_r = banner.as_ref().map(|_| next_row(ctl));
    let table = Rect::new(rect.x + bw, y, rect.w - 2 * bw, rect.bottom() - bw - y);

    // Row 1: location picker, Up, Refresh, breadcrumb path.
    let parts = ui.split_h(nav, &[L::Px(150), L::Px(tokens::CONTROL_H), L::Px(tokens::CONTROL_H), L::Fill], tokens::SPACE_XS);
    location_picker(ui, id, parts[0], pane, side, ctx);
    let can_up = parent(pane.is_remote(), &pane.cwd).is_some();
    if icon_button(ui, id.with("up"), parts[1], Icon::ChevronUp, "Up one folder", ButtonKind::Ghost).clicked && can_up {
        pane.go_up(ctx, side);
    }
    if icon_button(ui, id.with("refresh"), parts[2], Icon::Refresh, "Refresh", ButtonKind::Ghost).clicked {
        pane.refresh(ctx, side);
    }
    path_bar(ui, id, parts[3], pane, side, ctx);

    // Row 2: the toolbar, or the open name bar.
    if matches!(pane.confirm, Some(Confirm::Rename { .. } | Confirm::NewFolder { .. })) {
        name_bar(ui, id, tools, pane, side, ctx);
    } else {
        out.copy |= toolbar(ui, id, tools, pane, side);
    }

    // The error banner, with Reconnect / Retry / Dismiss.
    if let (Some(msg), Some(r)) = (banner, banner_r) {
        error_banner(ui, id, r, &msg, pane);
    }

    // Rest: the ssh prompt while one is open (dropped if ssh withdrew it),
    // else the listing.
    if pane.prompt.as_ref().is_some_and(|p| p.cancelled.is_cancelled()) {
        pane.prompt = None;
    }
    if pane.prompt.is_some() {
        prompt_card(ui, id, table.inset(pad), pane);
    } else {
        listing(ui, id, table, pane, side, ctx, &mut out);
    }
    if let Some(Confirm::Delete(items)) = pane.confirm.clone() {
        delete_dialog(ui, id, pane, items, side, ctx);
    }
    out
}

/// The location dropdown: Local, every host, and a host no longer listed
/// (still shown by name). Picking one reconnects the pane there.
fn location_picker(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx) {
    let mut options: Vec<&str> = vec!["Local"];
    options.extend(ctx.hosts.iter().map(|h| h.name.as_str()));
    // A host renamed or removed meanwhile stays shown (not as "Local").
    let gone = match &pane.loc {
        Location::Host(h) if !ctx.hosts.iter().any(|x| x.name == h.name) => Some(h.name.clone()),
        _ => None,
    };
    options.extend(gone.as_deref());
    let mut sel_loc = match &pane.loc {
        Location::Local => 0,
        Location::Host(h) => ctx.hosts.iter().position(|x| x.name == h.name).map_or(options.len() - 1, |i| i + 1),
    };
    let what = if side == 0 { "Left pane location" } else { "Right pane location" };
    if dropdown_labeled(ui, id.with("loc"), rect, what, &options, &mut sel_loc) {
        // Index 0 is Local, 1.. are hosts, and a trailing extra is the gone host.
        let loc = match ctx.hosts.get(sel_loc.wrapping_sub(1)) {
            _ if sel_loc == 0 => Location::Local,
            Some(h) => Location::Host(Box::new(h.clone())),
            None => pane.loc.clone(), // the host no longer listed: connect again
        };
        pane.set_location(loc);
    }
}

/// How much of the toolbar keeps its labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// Every button labelled.
    Full,
    /// Copy and the hidden-files toggle labelled, the rest icons.
    Mid,
    /// Icons only (named, with tooltips).
    Icons,
}

/// The toolbar: Copy (primary) | New folder, Rename | Delete (danger), and
/// the "Hidden files" toggle at the right end. Buttons that need a
/// selection are disabled without one. Narrower panes drop labels, the
/// secondary actions first (icon buttons are named and have tooltips).
/// Returns true when Copy was pressed.
fn toolbar(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane, side: usize) -> bool {
    let p = ui.style().palette;
    let n = pane.sel.len();
    let connected = pane.ep.is_some();
    let (copy_name, copy_icon) =
        if side == 0 { ("Copy to the right pane", Icon::ChevronRight) } else { ("Copy to the left pane", Icon::ChevronLeft) };
    let hidden_name = "Show hidden files";
    let hidden_label = "Hidden files";
    let hidden_icon = if pane.show_hidden { Icon::Eye } else { Icon::EyeOff };
    let items: [(&str, &str, Icon, ButtonKind, bool); 4] = [
        ("Copy", copy_name, copy_icon, ButtonKind::Primary, n > 0),
        ("New folder", "New folder", Icon::Plus, ButtonKind::Ghost, connected),
        ("Rename", "Rename", Icon::Edit, ButtonKind::Ghost, n == 1),
        ("Delete", "Delete", Icon::Trash, ButtonKind::Danger, n > 0),
    ];
    let (gap, group_gap) = (ui.px(tokens::SPACE_XS), ui.px(tokens::SPACE_SM));
    let full: Vec<i32> = items.iter().map(|it| button_width(ui, it.0, 1)).collect();
    let hidden_full = button_width(ui, hidden_label, 1);
    let gaps = 2 * group_gap + gap + group_gap;
    let fit = if full.iter().sum::<i32>() + hidden_full + gaps <= rect.w {
        Fit::Full
    } else if full[0] + 3 * rect.h + hidden_full + gaps <= rect.w {
        Fit::Mid
    } else {
        Fit::Icons
    };
    let labelled = |i: usize| fit == Fit::Full || (fit == Fit::Mid && i == 0);
    let widths: Vec<i32> = (0..4).map(|i| if labelled(i) { full[i] } else { rect.h }).collect();
    let mut x = rect.x;
    let mut copy = false;
    for (i, ((label, name, icon, kind, enabled), w)) in items.into_iter().zip(&widths).enumerate() {
        let r = Rect::new(x, rect.y, *w, rect.h);
        let bid = id.with(("tool", i));
        let compact = !labelled(i);
        let clicked = if compact {
            // Icon-only: the primary fill only while Copy can act.
            let k = if i == 0 && enabled { ButtonKind::Primary } else { ButtonKind::Ghost };
            icon_button(ui, bid, r, icon, name, k).clicked
        } else {
            // Copy's arrow points at the other pane: after the label on the left, before it on the right.
            let (lead, trail) = if i == 0 && side == 0 { (None, Some(icon)) } else { (Some(icon), None) };
            let opts = ButtonOpts { kind, icon: lead, trailing: trail, enabled, tooltip: None, a11y_name: Some(name) };
            button_with(ui, bid, r, label, opts).clicked
        };
        if compact && !enabled {
            // Icon buttons have no disabled look of their own: say so to screen readers.
            if let Some(nd) = ui.a11y_node(bid, Role::Button, name) {
                nd.disabled = true;
            }
        }
        if clicked && enabled {
            match i {
                0 => copy = true,
                1 => pane.ask_new_folder(),
                2 => pane.ask_rename(),
                _ => pane.ask_delete(),
            }
        }
        x += w + if i == 0 || i == 2 { group_gap } else { gap };
    }
    // The hidden-files toggle: a ghost button that stays filled while on.
    let compact = fit == Fit::Icons;
    let hw = if compact { rect.h } else { hidden_full };
    let hr = Rect::new((rect.right() - hw).max(x), rect.y, hw, rect.h);
    let hid = id.with("hidden");
    if pane.show_hidden {
        let radius = ui.style().radius * ui.scale();
        ui.painter().rect(hr, p.selection, radius);
    }
    let clicked = if compact {
        icon_button(ui, hid, hr, hidden_icon, hidden_name, ButtonKind::Ghost).clicked
    } else {
        let opts = ButtonOpts { kind: ButtonKind::Ghost, icon: Some(hidden_icon), a11y_name: Some(hidden_name), ..Default::default() };
        button_with(ui, hid, hr, hidden_label, opts).clicked
    };
    if let Some(nd) = ui.a11y_node(hid, Role::CheckBox, hidden_name) {
        nd.role = Role::CheckBox;
        nd.checked = Some(pane.show_hidden);
    }
    if clicked {
        pane.show_hidden = !pane.show_hidden;
        pane.sel.clear();
    }
    copy
}

/// The error banner: a danger-tinted strip with the message and Reconnect,
/// Retry or Dismiss.
fn error_banner(ui: &mut Ui, id: Id, rect: Rect, msg: &str, pane: &mut Pane) {
    let p = ui.style().palette;
    let (bg, fg) = tone_colors(&p, Tone::Danger);
    let radius = tokens::RADIUS_SM * ui.scale();
    ui.painter().rect(rect, bg, radius);
    let disconnected = pane.ep.is_none() && pane.is_remote();
    // A connected remote pane can still lose its connection (server
    // restart, timeout): offer to connect again next to Dismiss.
    let reconnect = pane.ep.is_some() && pane.is_remote();
    let pad = ui.px(tokens::SPACE_SM);
    let s = ui.px(tokens::ICON);
    ui.painter().icon(Rect::new(rect.x + pad, rect.y + (rect.h - s) / 2, s, s), Icon::Error, fg);
    let body = Rect::new(rect.x + 2 * pad + s, rect.y, rect.w - 2 * pad - s, rect.h);
    let parts = ui.split_h(body, &[L::Fill, L::Px(if reconnect { 96 } else { 0 }), L::Px(80)], tokens::SPACE_XS);
    text(ui, parts[0], msg, TextOpts { color: Some(fg), ..Default::default() });
    if let Some(n) = ui.a11y_node(id.with("banner"), Role::Status, msg) {
        n.live = true;
        n.bounds = rect;
    }
    let ghost = ButtonOpts::kind(ButtonKind::Ghost);
    if reconnect && button_with(ui, id.with("reconnect"), parts[1], "Reconnect", ghost).clicked {
        let loc = pane.loc.clone();
        pane.set_location(loc);
    } else if disconnected {
        if button_with(ui, id.with("retry"), parts[2], "Retry", ghost).clicked {
            let loc = pane.loc.clone();
            pane.set_location(loc);
        }
    } else if button_with(ui, id.with("dismiss"), parts[2], "Dismiss", ghost).clicked {
        // Clearing an error leaves an unconnected pane in an error state,
        // so it does not silently reconnect on the next frame.
        pane.banner = None;
        if matches!(pane.state, PaneState::Error(_)) {
            pane.state = PaneState::Idle;
            if pane.ep.is_none() {
                pane.state = PaneState::Error("not connected".into());
            }
        }
    }
}

/// The inline ssh question: prompt text, masked input (or Yes/No), OK/Cancel.
fn prompt_card(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane) {
    let p = ui.style().palette;
    let Some(pr) = pane.prompt.as_mut() else { return };
    let pad = ui.px(tokens::SPACE_LG);
    let px = ui.font_px();
    let lh = ui.line_height(FontKind::Ui, px) + ui.px(4);
    let mut y = rect.y + pad;
    // Title, then the prompt text line by line (blank lines skipped).
    let title = format!("{} asks:", pane.loc.label());
    text(ui, Rect::new(rect.x + pad, y, rect.w - 2 * pad, lh), &title, TextOpts { role: Some(TextRole::Title), ..Default::default() });
    y += lh + ui.px(tokens::SPACE_SM);
    for line in pr.text.lines().filter(|l| !l.trim().is_empty()) {
        let shown = elide(ui, line.trim_end(), FontKind::Ui, px, rect.w - 2 * pad);
        ui.painter().text(Point::new(rect.x + pad, y), &shown, FontKind::Ui, px, p.muted);
        y += lh;
    }
    y += ui.px(tokens::SPACE_SM);
    let row = Rect::new(rect.x + pad, y, (rect.w - 2 * pad).min(ui.px(520)), ui.px(tokens::CONTROL_H));
    let iid = id.with("prompt-input");
    let primary = ButtonOpts::kind(ButtonKind::Primary);
    // `Some(None)` = cancel (stops connecting), `Some(Some(a))` = answer `a`.
    let mut answer: Option<Option<String>> = None;
    match pr.kind {
        AskKind::Secret => {
            let parts = ui.split_h(row, &[L::Fill, L::Px(70), L::Px(80)], tokens::SPACE_SM);
            if pr.focus {
                ui.request_focus(iid);
                pr.focus = false;
            }
            let r = text_input(
                ui,
                iid,
                parts[0],
                &mut pr.input,
                TextInputOpts { password: true, placeholder: "Password", ..Default::default() },
            );
            if button_with(ui, id.with("prompt-ok"), parts[1], "OK", primary).clicked || r.submitted {
                answer = Some(Some(pr.input.clone()));
            }
            if button(ui, id.with("prompt-cancel"), parts[2], "Cancel").clicked {
                answer = Some(None);
            }
        }
        AskKind::Notice => {
            // Nothing to answer ("touch your security key"); it closes by
            // itself when ssh is done, or can be hidden.
            let parts = ui.split_h(row, &[L::Px(80), L::Fill], tokens::SPACE_SM);
            if button(ui, id.with("prompt-hide"), parts[0], "Hide").clicked {
                answer = Some(Some(String::new()));
            }
        }
        AskKind::Confirm => {
            let parts = ui.split_h(row, &[L::Px(80), L::Px(80), L::Px(80), L::Fill], tokens::SPACE_SM);
            if button_with(ui, id.with("prompt-yes"), parts[0], "Yes", primary).clicked {
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

/// The current folder as a breadcrumb: each segment is clickable and goes
/// to that folder. Clicking the last segment or the empty space, or Enter /
/// Space while the bar has focus, turns it into a text field where Enter
/// goes to the typed path. Segments that do not fit collapse into "…".
fn path_bar(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx) {
    let p = ui.style().palette;
    let pid = id.with("path");
    // Edit mode: the field stays open only while it keeps focus.
    if let Some(mut text) = pane.path_edit.take() {
        if pane.focus_input {
            ui.request_focus(pid);
            pane.focus_input = false;
        }
        let r =
            text_input(ui, pid, rect, &mut text, TextInputOpts { placeholder: "Go to path", label: "Go to path", ..Default::default() });
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
    ui.a11y_node(pid, Role::Button, &format!("Path {}, edit", pane.cwd));
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Text);
    }
    let radius = ui.style().radius * ui.scale();
    let bw = ui.px(1).max(1);
    {
        let mut painter = ui.painter();
        painter.rect(rect, p.bg, radius);
        painter.border(rect, if r.hovered { p.control } else { p.border }, bw, radius);
    }
    let (font, px) = ui.text_style(TextRole::Body);
    let pad = ui.px(tokens::SPACE_XS);
    let chev = ui.px(12);
    let seg_pad = ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
    // Right end: what the pane is busy with.
    let busy = match pane.state {
        PaneState::Loading => "Loading\u{2026}",
        PaneState::Connecting => "Connecting\u{2026}",
        _ => "",
    };
    let busy_w = if busy.is_empty() { 0 } else { ui.measure(busy, font, px).ceil() as i32 + 2 * seg_pad };
    let room = rect.w - 2 * pad - busy_w;
    let all = crumbs(pane.is_remote(), &pane.cwd);
    let widths: Vec<i32> = all.iter().map(|(l, _)| ui.measure(l, font, px).ceil() as i32 + 2 * seg_pad).collect();
    // Keep the root and as many trailing segments as fit; the rest become "…".
    let ell_w = ui.measure("\u{2026}", font, px).ceil() as i32 + 2 * seg_pad;
    let mut shown: Vec<Option<usize>> = (0..all.len()).map(Some).collect();
    let total =
        |s: &[Option<usize>]| -> i32 { s.iter().map(|i| i.map_or(ell_w, |i| widths[i])).sum::<i32>() + chev * (s.len() as i32 - 1).max(0) };
    let n = all.len();
    if total(&shown) > room && n > 2 {
        // Root, "…", then the last `k` segments (at least one stays hidden).
        let layout = |k: usize| -> Vec<Option<usize>> { [Some(0), None].into_iter().chain((n - k..n).map(Some)).collect() };
        let mut keep = 1;
        while keep < n - 2 && total(&layout(keep + 1)) <= room {
            keep += 1;
        }
        shown = layout(keep);
    }
    let lh = ui.line_height(font, px);
    let ty = rect.y + (rect.h - lh) / 2;
    let mut x = rect.x + pad;
    let inner = Rect::new(rect.x + pad, rect.y + bw, room.max(0), rect.h - 2 * bw);
    let mut go: Option<String> = None;
    let mut edit = r.clicked;
    ui.with_clip(inner, |ui| {
        let last = shown.len().saturating_sub(1);
        for (n, seg) in shown.iter().enumerate() {
            if n > 0 {
                ui.painter().icon(Rect::new(x, rect.y + (rect.h - chev) / 2, chev, chev), Icon::ChevronRight, p.muted);
                x += chev;
            }
            let (label, path, w) = match seg {
                Some(i) => (all[*i].0.as_str(), Some(all[*i].1.clone()), widths[*i]),
                None => ("\u{2026}", None, ell_w),
            };
            let seg_r = Rect::new(x, rect.y + ui.px(3), w, rect.h - ui.px(6));
            let sid = id.with(("crumb", n));
            let sr = ui.interact(sid, seg_r, Sense::click());
            let name = match &path {
                Some(pth) if n == last => format!("Current folder {pth}, edit"),
                Some(pth) => format!("Go to {pth}"),
                None => "Hidden folders, edit path".to_string(),
            };
            ui.a11y_node(sid, Role::Button, &name);
            if sr.hovered {
                ui.set_cursor_icon(CursorIcon::Pointer);
                let rr = ui.style().radius * ui.scale();
                ui.painter().rect(seg_r, p.hover, rr);
            }
            let color = if n == last || sr.hovered { p.fg } else { p.muted };
            let f = if n == last { FontKind::UiSemibold } else { font };
            ui.painter().text(Point::new(x + seg_pad, ty), label, f, px, color);
            if sr.clicked {
                match path {
                    Some(pth) if n != last => go = Some(pth),
                    _ => edit = true,
                }
            }
            x += w;
        }
    });
    if !busy.is_empty() {
        ui.painter().text(Point::new(rect.right() - pad - busy_w + seg_pad, ty), busy, font, px, p.muted);
    }
    if let Some(pth) = go {
        pane.navigate(pth, ctx, side);
    } else if edit {
        pane.edit_path();
    }
}

/// Why `name` can't be a file name.
fn invalid_name(name: &str) -> String {
    if name.is_empty() { "Type a name first".into() } else { format!("{name} is not a valid name (no / \\ or :)") }
}

/// The inline name bar for Rename / New folder: a field with the primary
/// action and Cancel. Bad names keep the bar open and show a banner.
fn name_bar(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx) {
    let Some(confirm) = pane.confirm.clone() else { return };
    let parts = ui.split_h(rect, &[L::Fill, L::Px(84), L::Px(76)], tokens::SPACE_XS);
    let iid = id.with("bar-input");
    let mut done = false;
    if pane.focus_input {
        ui.request_focus(iid);
        pane.focus_input = false;
    }
    let primary = ButtonOpts::kind(ButtonKind::Primary);
    match confirm {
        Confirm::Rename { from, mut input } => {
            let opts = TextInputOpts { placeholder: "New name", label: "New name", ..Default::default() };
            let r = text_input(ui, iid, parts[0], &mut input, opts);
            let ok = button_with(ui, id.with("bar-ok"), parts[1], "Rename", primary).clicked || r.submitted;
            if ok && !is_safe_name(input.trim()) {
                pane.banner = Some(invalid_name(input.trim()));
            }
            if ok && is_safe_name(input.trim()) {
                let to = pane.join(input.trim());
                // An unchanged name just closes the bar.
                if to != from {
                    pane.run(Op::Rename { from: from.clone(), to }, ctx, side);
                }
                done = true;
            } else {
                // Still open: store the edited text back for next frame.
                pane.confirm = Some(Confirm::Rename { from, input });
            }
        }
        Confirm::NewFolder { mut input } => {
            let opts = TextInputOpts { placeholder: "New folder name", label: "New folder name", ..Default::default() };
            let r = text_input(ui, iid, parts[0], &mut input, opts);
            let ok = button_with(ui, id.with("bar-ok"), parts[1], "Create", primary).clicked || r.submitted;
            if ok && !is_safe_name(input.trim()) {
                pane.banner = Some(invalid_name(input.trim()));
            }
            if ok && is_safe_name(input.trim()) {
                let path = pane.join(input.trim());
                pane.run(Op::Mkdir(path), ctx, side);
                done = true;
            } else {
                pane.confirm = Some(Confirm::NewFolder { input });
            }
        }
        Confirm::Delete(_) => return,
    }
    let cancel = id.with("bar-cancel");
    // Esc belongs to the bar only while focus is in it (or on its list):
    // the other pane may have a bar open too.
    let mine = [iid, id.with("bar-ok"), cancel, id.with("list")].into_iter().any(|i| ui.has_focus(i));
    let ghost = ButtonOpts::kind(ButtonKind::Ghost);
    if button_with(ui, cancel, parts[2], "Cancel", ghost).clicked || (mine && ui.input_mut().take_key(&Key::Escape, Modifiers::NONE)) {
        done = true;
    }
    if done {
        pane.banner = None; // an invalid-name notice goes with its bar
        pane.confirm = None;
        ui.request_focus(id.with("list"));
    }
}

/// The delete question as a modal dialog (focus starts on Cancel).
fn delete_dialog(ui: &mut Ui, id: Id, pane: &mut Pane, items: Vec<(String, bool, String)>, side: usize, ctx: &FilesCtx) {
    let title = match items.as_slice() {
        [(_, _, name)] => format!("Delete {name}?"),
        _ => format!("Delete {} items?", items.len()),
    };
    let folders = items.iter().any(|(_, d, _)| *d);
    // Where: the folder's name and the location (the full path is in the breadcrumb).
    let folder = crumbs(pane.is_remote(), &pane.cwd).pop().map_or(pane.cwd.clone(), |c| c.0);
    let place = format!("{folder} on {}", pane.loc.label());
    let body = if folders {
        format!("Deletes from {place}, folders with everything in them. This cannot be undone.")
    } else {
        format!("Deletes from {place}. This cannot be undone.")
    };
    let buttons =
        [DialogButton { label: "Cancel", kind: ButtonKind::Secondary }, DialogButton { label: "Delete", kind: ButtonKind::Danger }];
    let d = Dialog { title: &title, body: &body, buttons: &buttons, initial: 0, icon: Some(Icon::Warning), ..Default::default() };
    match dialog(ui, id.with("delete-dialog"), &d) {
        Some(DialogResult::Button(1)) => {
            pane.run(Op::Delete(items.into_iter().map(|(p, d, _)| (p, d)).collect()), ctx, side);
            pane.confirm = None;
        }
        Some(_) => pane.confirm = None,
        None => {}
    }
}

/// The folder table: a sortable header (Name, Size, Modified), one row per
/// entry with a folder / file / link icon, and the footer. Rows are a drag
/// source and drop target for copies; Enter/double-click opens folders;
/// typing letters selects by name; Backspace, F2, Delete, F5 and Ctrl+L go
/// up, rename, delete, copy and edit the path.
fn listing(ui: &mut Ui, id: Id, rect: Rect, pane: &mut Pane, side: usize, ctx: &FilesCtx, out: &mut PaneOut) {
    let p = ui.style().palette;
    let lid = id.with("list");
    let bw = ui.px(1).max(1);
    let pad = ui.px(tokens::SPACE_SM);
    let hh = ui.px(tokens::HEADER_H);
    let fh = ui.px(FOOTER_H);
    let header = Rect::new(rect.x, rect.y, rect.w, hh);
    let footer = Rect::new(rect.x, rect.bottom() - fh, rect.w, fh);
    let list_r = Rect::new(rect.x, header.bottom(), rect.w, footer.y - header.bottom());
    // Header: a top rule, then the sortable columns (Modified only when there is room).
    ui.painter().rect(Rect::new(rect.x, rect.y, rect.w, bw), p.border, 0.0);
    let wide = rect.w > ui.px(380);
    let mut columns = vec![
        Column { title: "Name", width: L::Fill, sortable: true, align: Align::Start },
        Column { title: "Size", width: L::Px(84), sortable: true, align: Align::End },
    ];
    if wide {
        columns.push(Column { title: "Modified", width: L::Px(136), sortable: true, align: Align::Start });
    }
    let mut sort = pane.sort;
    let head = table_header(ui, id.with("header"), Rect::new(header.x + pad, header.y, header.w - 2 * pad, header.h), &columns, &mut sort);
    if head.changed {
        pane.set_sort(sort);
    }
    let cols = head.columns;

    // Type-ahead and the list's own shortcuts, before the rows are drawn.
    let rh = ui.px(tokens::ROW_H);
    if ui.has_focus(lid) && ui.keys_for(lid) {
        let typed: String = ui
            .input()
            .events
            .iter()
            .filter_map(|e| if let Event::Text(t) = e { Some(t.as_str()) } else { None })
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        let typed = if pane.typed_at.is_none_or(|t| ui.now().saturating_duration_since(t) > TYPE_AHEAD_RESET) {
            typed.trim_start().to_string() // a leading space starts nothing
        } else {
            typed
        };
        if !typed.is_empty()
            && let Some(i) = pane.type_ahead(&typed, ui.now())
        {
            let top = i as i32 * rh;
            scroll_into_view(ui, lid.with("scroll"), top, top + rh, list_r.h);
        }
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
        let ctrl_l = Modifiers { ctrl: true, ..Modifiers::NONE };
        if ui.input_mut().take_key(&Key::Char('l'), ctrl_l) {
            pane.edit_path();
        }
    }

    let vis: Vec<FileEntry> = pane.visible().into_iter().cloned().collect();
    let list_name = format!("{} files", pane.loc.label());
    // Screen-reader text for row `i`.
    let row_name = |i: usize| {
        let e = &vis[i];
        let kind = match (e.is_dir, e.is_symlink) {
            (true, true) => "link to a folder".to_string(),
            (true, false) => "folder".to_string(),
            (false, true) => format!("link, {}", human_size(e.size)),
            (false, false) => human_size(e.size),
        };
        match e.modified {
            Some(t) => format!("{}, {kind}, modified {}", e.name, format_time(t)),
            None => format!("{}, {kind}", e.name),
        }
    };
    let icon_px = ui.px(tokens::ICON);
    let muted = TextOpts { color: Some(p.muted), ..Default::default() };
    let r = select_list_named(ui, lid, list_r, &list_name, vis.len(), tokens::ROW_H, &mut pane.sel, row_name, |ui, row, i, _| {
        let e = &vis[i];
        let c0 = cols[COL_NAME];
        let icon = if e.is_symlink {
            Icon::Link
        } else if e.is_dir {
            Icon::Folder
        } else {
            Icon::File
        };
        let ic = if e.is_dir { p.accent } else { p.muted };
        let ix = c0.x + ui.px(tokens::SPACE_SM);
        ui.painter().icon(Rect::new(ix, row.y + (row.h - icon_px) / 2, icon_px, icon_px), icon, ic);
        let nx = ix + icon_px + ui.px(tokens::SPACE_SM);
        let name_r = Rect::new(nx, row.y, (c0.right() - nx - pad).max(0), row.h);
        text(ui, name_r, &e.name, TextOpts { elide: Elide::Middle, ..Default::default() });
        let c1 = cols[COL_SIZE];
        let size = if e.is_dir { "\u{2014}".to_string() } else { human_size(e.size) };
        text(ui, Rect::new(c1.x, row.y, c1.w - pad, row.h), &size, TextOpts { align: Align::End, ..muted });
        if let Some(c2) = cols.get(COL_MODIFIED)
            && let Some(t) = e.modified
        {
            text(ui, Rect::new(c2.x + pad, row.y, c2.w - pad, row.h), &format_time(t), muted);
        }
    });
    // Dragging the selection out; dropping rows from elsewhere onto this list.
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
        let w = ui.px(2);
        let tint = p.accent.with_alpha(26);
        ui.painter().rect(list_r, tint, 0.0);
        let rr = tokens::RADIUS_SM * ui.scale();
        ui.painter().border(list_r, p.accent, w, rr);
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
    if vis.is_empty() && pane.state == PaneState::Idle {
        let e = EmptyState {
            icon: Some(Icon::Folder),
            title: "This folder is empty",
            body: "Drop files here to copy them in.",
            ..Default::default()
        };
        empty_state(ui, id.with("empty"), list_r, &e);
    }
    // Footer: counts and sizes.
    ui.painter().rect(Rect::new(footer.x, footer.y, footer.w, bw), p.border, 0.0);
    let ft = footer_text(pane);
    text(ui, Rect::new(footer.x + pad, footer.y, footer.w - 2 * pad, footer.h), &ft, TextOpts { role: Some(TextRole::Caption), ..muted });
}
