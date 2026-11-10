//! Dual-pane SFTP browser for the TUI: a local pane and one remote pane.
//! All SSH/SFTP work goes through `termax_core::fs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::app::Msg;
use termax_core::askpass::Broker;
use termax_core::fs::connect::{RemoteConn, SshLaunch, connect_sftp};
use termax_core::fs::endpoint::Endpoint;
use termax_core::fs::transfer::{self, OverwritePolicy, TransferEvent, TransferHandle, TransferRequest, TransferSummary};
pub use termax_core::fs::{self, FileEntry, human_size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

impl Side {
    pub fn other(self) -> Self {
        match self {
            Side::Local => Side::Remote,
            Side::Remote => Side::Local,
        }
    }
}

#[derive(Debug, Default)]
pub struct Pane {
    pub path: String,
    pub offset: usize,
    pub entries: Vec<FileEntry>,
    pub selected: usize,
    pub loading: bool,
    pub error: Option<String>,
}

impl Pane {
    pub fn current(&self) -> Option<&FileEntry> {
        self.entries.get(self.selected)
    }

    pub fn set_entries(&mut self, path: String, mut entries: Vec<FileEntry>, show_hidden: bool, keep: Option<&str>) {
        if !show_hidden {
            entries.retain(|e| !e.name.starts_with('.'));
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        entries.insert(0, FileEntry { name: "..".into(), is_dir: true, is_symlink: false, size: 0, modified: None });
        let same_dir = self.path == path;
        let prev = keep.map(str::to_string).or_else(|| same_dir.then(|| self.current().map(|e| e.name.clone())).flatten());
        self.path = path;
        self.entries = entries;
        self.selected = prev.and_then(|n| self.entries.iter().position(|e| e.name == n)).unwrap_or(0);
        self.loading = false;
        self.error = None;
    }

    pub fn move_by(&mut self, delta: i64) {
        if self.entries.is_empty() {
            return;
        }
        let max = self.entries.len() as i64 - 1;
        self.selected = (self.selected as i64 + delta).clamp(0, max) as usize;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferState {
    Running,
    Done,
    Cancelled,
    Failed(String),
}

pub struct Transfer {
    pub id: u64,
    pub label: String,
    pub done: u64,
    pub total: Option<u64>,
    /// Files left alone because they existed (or weren't safe to copy).
    pub skipped: u64,
    pub state: TransferState,
    handle: TransferHandle,
}

pub enum Status {
    Connecting,
    Connected,
    Failed(String),
}

pub struct SftpTab {
    pub tab_id: u64,
    pub host_name: String,
    pub status: Status,
    pub conn: Option<Arc<RemoteConn>>,
    pub local: Pane,
    pub remote: Pane,
    pub focus: Side,
    pub transfers: Vec<Transfer>,
    pub show_hidden: bool,
    launch: Option<SshLaunch>,
    /// Relays ssh's password and host-key questions (when termax can prompt).
    askpass: Option<Arc<Broker>>,
    /// Connection attempts so far; each gets its own askpass tag.
    attempt: u64,
    next_transfer: u64,
    rt: Handle,
    tx: UnboundedSender<Msg>,
}

impl SftpTab {
    pub fn new(tab_id: u64, host_name: String, local_dir: PathBuf, show_hidden: bool, rt: Handle, tx: UnboundedSender<Msg>) -> Self {
        let mut tab = Self {
            tab_id,
            host_name,
            status: Status::Connecting,
            conn: None,
            local: Pane::default(),
            remote: Pane { loading: true, ..Default::default() },
            focus: Side::Remote,
            transfers: vec![],
            show_hidden,
            launch: None,
            askpass: None,
            attempt: 0,
            next_transfer: 1,
            rt,
            tx,
        };
        tab.list_local(local_dir.display().to_string(), None);
        tab
    }

    /// ssh's questions while connecting go to `broker` (as `Msg::Ask`).
    pub fn set_askpass(&mut self, broker: Option<Arc<Broker>>) {
        self.askpass = broker;
    }

    /// The askpass tag of the current connection attempt.
    pub fn ask_tag(&self) -> String {
        format!("sftp:{}:{}", self.tab_id, self.attempt)
    }

    /// Withdraws ssh's open questions for the current attempt.
    fn retire_questions(&self) {
        if let Some(b) = &self.askpass {
            b.retire(&self.ask_tag());
        }
    }

    /// Connects in the background; the result arrives as `Msg::SftpConnected`.
    pub fn connect(&mut self, launch: SshLaunch) {
        self.launch = Some(launch.clone());
        self.status = Status::Connecting;
        self.conn = None;
        self.remote = Pane { loading: true, ..Default::default() };
        self.retire_questions();
        self.attempt += 1;
        let mut launch = launch;
        if let Some(b) = &self.askpass {
            launch.env.extend(b.env(&self.ask_tag()));
        }
        let (tx, id) = (self.tx.clone(), self.tab_id);
        self.rt.spawn(async move {
            let result = connect_sftp(launch).await.map(Arc::new);
            let _ = tx.send(Msg::SftpConnected { tab: id, result });
        });
    }

    pub fn reconnect(&mut self) {
        if let Some(launch) = self.launch.clone() {
            self.connect(launch);
        }
    }

    pub fn on_connected(&mut self, result: Result<Arc<RemoteConn>, String>) {
        match result {
            Ok(conn) => {
                let home = conn.home.clone();
                self.conn = Some(conn);
                self.status = Status::Connected;
                self.list_remote(home, None);
            }
            Err(e) => {
                // ssh gave up: a question still on screen is moot.
                self.retire_questions();
                self.remote.loading = false;
                self.status = Status::Failed(e);
            }
        }
    }

    fn endpoint(&self, side: Side) -> Option<Endpoint> {
        match side {
            Side::Local => Some(Endpoint::Local),
            Side::Remote => self.conn.clone().map(Endpoint::Remote),
        }
    }

    pub fn pane(&self, side: Side) -> &Pane {
        match side {
            Side::Local => &self.local,
            Side::Remote => &self.remote,
        }
    }

    pub fn pane_mut(&mut self, side: Side) -> &mut Pane {
        match side {
            Side::Local => &mut self.local,
            Side::Remote => &mut self.remote,
        }
    }

    pub fn list_local(&mut self, path: String, keep: Option<String>) {
        match fs::path::list_local(Path::new(&path)) {
            Ok(entries) => {
                let path = std::fs::canonicalize(&path).map(|p| clean_local(&p)).unwrap_or(path);
                self.local.set_entries(path, entries, self.show_hidden, keep.as_deref());
            }
            Err(e) => self.local.error = Some(format!("{path}: {e}")),
        }
    }

    pub fn list_remote(&mut self, path: String, keep: Option<String>) {
        let Some(ep) = self.endpoint(Side::Remote) else { return };
        self.remote.loading = true;
        let (tx, id) = (self.tx.clone(), self.tab_id);
        self.rt.spawn(async move {
            let result = ep.list(&path).await.map(|e| (path, e, keep)).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::RemoteListing { tab: id, result });
        });
    }

    pub fn on_remote_listing(&mut self, result: Result<(String, Vec<FileEntry>, Option<String>), String>) {
        match result {
            Ok((path, entries, keep)) => self.remote.set_entries(path, entries, self.show_hidden, keep.as_deref()),
            Err(e) => {
                self.remote.loading = false;
                self.remote.error = Some(e);
            }
        }
    }

    pub fn refresh(&mut self, side: Side) {
        let path = self.pane(side).path.clone();
        match side {
            Side::Local => self.list_local(path, None),
            Side::Remote => self.list_remote(path, None),
        }
    }

    pub fn navigate(&mut self, side: Side, path: String, keep: Option<String>) {
        match side {
            Side::Local => self.list_local(path, keep),
            Side::Remote => self.list_remote(path, keep),
        }
    }

    /// Enter the selected directory (or go up for `..`).
    pub fn open_selected(&mut self) {
        let side = self.focus;
        let Some(entry) = self.pane(side).current().cloned() else { return };
        if entry.name == ".." {
            self.go_up();
        } else if entry.is_dir || entry.is_symlink {
            let path = join(side, &self.pane(side).path, &entry.name);
            self.navigate(side, path, None);
        }
    }

    pub fn go_up(&mut self) {
        let side = self.focus;
        let cur = self.pane(side).path.clone();
        if let Some((parent, name)) = parent(side, &cur) {
            self.navigate(side, parent, Some(name));
        }
    }

    pub fn selected_path(&self) -> Option<(String, FileEntry)> {
        let pane = self.pane(self.focus);
        let e = pane.current()?.clone();
        (e.name != "..").then(|| (join(self.focus, &pane.path, &e.name), e))
    }

    /// Copies the selected entry into the directory shown in the other pane.
    /// For each file that already exists there the user is asked
    /// (`Msg::TransferAsk`).
    pub fn copy_selected(&mut self) -> Result<(), String> {
        let (Some(src), Some(dst)) = (self.endpoint(self.focus), self.endpoint(self.focus.other())) else {
            return Err("not connected".into());
        };
        let Some((src_path, entry)) = self.selected_path() else { return Err("nothing selected".into()) };
        let id = self.next_transfer;
        self.next_transfer += 1;
        let upload = self.focus == Side::Local;
        let req = TransferRequest { id, src, src_path, dst, dst_dir: self.pane(self.focus.other()).path.clone() };
        let (events_tx, mut events) = unbounded_channel();
        let handle = transfer::start(&self.rt, req, OverwritePolicy::Ask, events_tx);
        self.transfers.push(Transfer {
            id,
            label: format!("{} {}", if upload { "↑" } else { "↓" }, entry.name),
            done: 0,
            total: (!entry.is_dir).then_some(entry.size),
            skipped: 0,
            state: TransferState::Running,
            handle,
        });
        let (tx, tab) = (self.tx.clone(), self.tab_id);
        self.rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                match ev {
                    TransferEvent::Progress { id, done, .. } => {
                        let _ = tx.send(Msg::TransferProgress { tab, id, done });
                    }
                    TransferEvent::Done { id, result } => {
                        let _ = tx.send(Msg::TransferDone { tab, id, result });
                        break;
                    }
                    TransferEvent::NeedsDecision { id, path, reply } => {
                        let _ = tx.send(Msg::TransferAsk { tab, id, path, reply });
                    }
                }
            }
        });
        Ok(())
    }

    pub fn on_transfer_progress(&mut self, id: u64, done: u64) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.done = done;
        }
    }

    pub fn on_transfer_done(&mut self, id: u64, result: Result<TransferSummary, String>) {
        let mut target = None;
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.state = match result {
                Ok(s) => {
                    t.skipped = s.skipped;
                    if s.cancelled { TransferState::Cancelled } else { TransferState::Done }
                }
                Err(e) => TransferState::Failed(e),
            };
            target = Some(if t.label.starts_with('↑') { Side::Remote } else { Side::Local });
        }
        if let Some(side) = target {
            self.refresh(side);
        }
    }

    /// Cancels transfer `id` if it is still running.
    pub fn cancel_transfer(&mut self, id: u64) {
        if let Some(t) = self.transfers.iter().find(|t| t.id == id && t.state == TransferState::Running) {
            t.handle.cancel();
        }
    }

    /// Cancels the newest running transfer (the top of the list); returns its label.
    pub fn cancel_newest_transfer(&mut self) -> Option<String> {
        let t = self.transfers.iter().rev().find(|t| t.state == TransferState::Running)?;
        t.handle.cancel();
        Some(t.label.clone())
    }

    pub fn clear_finished_transfers(&mut self) {
        self.transfers.retain(|t| t.state == TransferState::Running);
    }

    /// Runs a filesystem operation on the focused side and refreshes it afterwards.
    pub fn file_op(&mut self, op: FileOp) {
        let side = self.focus;
        let Some(ep) = self.endpoint(side) else { return };
        let base = self.pane(side).path.clone();
        let (tx, tab) = (self.tx.clone(), self.tab_id);
        let remote = side == Side::Remote;
        let task = async move {
            let j = |n: &str| fs::join(remote, &base, n);
            let r = match &op {
                FileOp::Mkdir(n) => ep.mkdir(&j(n)).await,
                FileOp::Rename { from, to } => ep.rename(&j(from), &j(to)).await,
                FileOp::Delete { name, is_dir } => ep.remove(&j(name), *is_dir).await,
            };
            (r.map_err(|e| format!("{e:#}")), op.keep_name())
        };
        match side {
            Side::Local => {
                let (r, keep) = self.rt.block_on(task);
                if let Err(e) = r {
                    self.local.error = Some(e);
                }
                let path = self.local.path.clone();
                self.list_local(path, keep);
            }
            Side::Remote => {
                self.rt.spawn(async move {
                    let (result, keep) = task.await;
                    let _ = tx.send(Msg::RemoteOpDone { tab, result, keep });
                });
            }
        }
    }

    pub fn on_remote_op(&mut self, result: Result<(), String>, keep: Option<String>) {
        if let Err(e) = result {
            self.remote.error = Some(e);
        }
        let path = self.remote.path.clone();
        self.list_remote(path, keep);
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refresh(Side::Local);
        self.refresh(Side::Remote);
    }
}

impl Drop for SftpTab {
    /// Closing the tab stops its transfers and withdraws ssh's questions.
    fn drop(&mut self) {
        for t in self.transfers.iter().filter(|t| t.state == TransferState::Running) {
            t.handle.cancel();
        }
        self.retire_questions();
    }
}

#[derive(Debug, Clone)]
pub enum FileOp {
    Mkdir(String),
    Rename { from: String, to: String },
    Delete { name: String, is_dir: bool },
}

impl FileOp {
    fn keep_name(&self) -> Option<String> {
        match self {
            FileOp::Mkdir(n) => Some(n.clone()),
            FileOp::Rename { to, .. } => Some(to.clone()),
            FileOp::Delete { .. } => None,
        }
    }
}

/// Strips Windows' `\\?\` prefix from canonicalized paths.
fn clean_local(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

pub fn join(side: Side, base: &str, name: &str) -> String {
    fs::join(side == Side::Remote, base, name)
}

pub fn parent(side: Side, path: &str) -> Option<(String, String)> {
    fs::parent(side == Side::Remote, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_sorting_and_hidden() {
        let mut p = Pane::default();
        let f = |n: &str, d: bool| FileEntry { name: n.into(), is_dir: d, is_symlink: false, size: 0, modified: None };
        p.set_entries("/x".into(), vec![f("b", false), f(".hid", false), f("A", true), f("a", false)], false, None);
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "A", "a", "b"]);
    }
}
