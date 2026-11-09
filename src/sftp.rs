//! Dual-pane SFTP browser. The SFTP protocol runs over `ssh -s <host> sftp`,
//! so authentication, ProxyJump, agents etc. all come from OpenSSH.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::Msg;

const CHUNK: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

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
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Transfer {
    pub id: u64,
    pub label: String,
    pub done: u64,
    pub total: Option<u64>,
    pub state: TransferState,
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
    pub session: Option<Arc<SftpSession>>,
    pub local: Pane,
    pub remote: Pane,
    pub focus: Side,
    pub transfers: Vec<Transfer>,
    pub show_hidden: bool,
    launch: Option<(String, Vec<String>, Option<String>)>,
    child: Option<Child>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_transfer: u64,
    rt: Handle,
    tx: UnboundedSender<Msg>,
}

impl SftpTab {
    pub fn new(
        tab_id: u64,
        host_name: String,
        local_dir: PathBuf,
        show_hidden: bool,
        rt: Handle,
        tx: UnboundedSender<Msg>,
    ) -> Self {
        let mut tab = Self {
            tab_id,
            host_name,
            status: Status::Connecting,
            session: None,
            local: Pane::default(),
            remote: Pane { loading: true, ..Default::default() },
            focus: Side::Remote,
            transfers: vec![],
            show_hidden,
            launch: None,
            child: None,
            stderr: Arc::new(Mutex::new(vec![])),
            next_transfer: 1,
            rt,
            tx,
        };
        tab.list_local(local_dir.display().to_string(), None);
        tab
    }

    /// Spawns `ssh ... -s host sftp` and performs the SFTP handshake in the background.
    pub fn connect(&mut self, program: &str, args: &[String], start_dir: Option<String>) {
        self.launch = Some((program.to_string(), args.to_vec(), start_dir.clone()));
        if let Some(mut old) = self.child.take() {
            let _ = old.start_kill();
        }
        self.status = Status::Connecting;
        self.session = None;
        self.remote = Pane { loading: true, ..Default::default() };
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: keep ssh from touching our console.
            cmd.creation_flags(0x0800_0000);
        }
        let _guard = self.rt.enter();
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.status = Status::Failed(format!("starting `{program}`: {e}"));
                return;
            }
        };
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        self.child = Some(child);

        let log = self.stderr.clone();
        log.lock().unwrap().clear();
        self.rt.spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut l = log.lock().unwrap();
                l.push(line);
                if l.len() > 20 {
                    l.remove(0);
                }
            }
        });

        let (tx, id, log) = (self.tx.clone(), self.tab_id, self.stderr.clone());
        self.rt.spawn(async move {
            let stream = tokio::io::join(stdout, stdin);
            let result = async {
                let sftp = SftpSession::new(stream).await?;
                let home = match start_dir {
                    Some(d) => sftp.canonicalize(d).await?,
                    None => sftp.canonicalize(".").await?,
                };
                Ok::<_, anyhow::Error>((Arc::new(sftp), home))
            }
            .await;
            let result = match result {
                Ok(v) => Ok(v),
                Err(e) => {
                    // Give ssh a moment to flush its error to stderr.
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    let lines = log.lock().unwrap().join("\n");
                    Err(if lines.trim().is_empty() { e.to_string() } else { lines })
                }
            };
            let _ = tx.send(Msg::SftpConnected { tab: id, result });
        });
    }

    pub fn reconnect(&mut self) {
        if let Some((program, args, dir)) = self.launch.clone() {
            self.connect(&program, &args, dir);
        }
    }

    pub fn on_connected(&mut self, result: Result<(Arc<SftpSession>, String), String>) {
        match result {
            Ok((sftp, home)) => {
                self.session = Some(sftp);
                self.status = Status::Connected;
                self.list_remote(home, None);
            }
            Err(e) => {
                self.remote.loading = false;
                self.status = Status::Failed(e);
            }
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
        match read_local_dir(Path::new(&path)) {
            Ok(entries) => {
                let path = std::fs::canonicalize(&path).map(|p| clean_local(&p)).unwrap_or(path);
                self.local.set_entries(path, entries, self.show_hidden, keep.as_deref());
            }
            Err(e) => self.local.error = Some(format!("{path}: {e}")),
        }
    }

    pub fn list_remote(&mut self, path: String, keep: Option<String>) {
        let Some(sftp) = self.session.clone() else { return };
        self.remote.loading = true;
        let (tx, id) = (self.tx.clone(), self.tab_id);
        self.rt.spawn(async move {
            let result = read_remote_dir(&sftp, &path).await.map(|e| (path, e, keep)).map_err(|e| e.to_string());
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

    /// Copies the selected entry to the directory shown in the other pane.
    pub fn copy_selected(&mut self) -> Result<(), String> {
        let Some(sftp) = self.session.clone() else { return Err("not connected".into()) };
        let Some((src, entry)) = self.selected_path() else { return Err("nothing selected".into()) };
        let dst = join(self.focus.other(), &self.pane(self.focus.other()).path, &entry.name);
        let id = self.next_transfer;
        self.next_transfer += 1;
        let upload = self.focus == Side::Local;
        let label = format!("{} {}", if upload { "↑" } else { "↓" }, entry.name);
        self.transfers.push(Transfer {
            id,
            label,
            done: 0,
            total: (!entry.is_dir).then_some(entry.size),
            state: TransferState::Running,
        });
        let (tx, tab) = (self.tx.clone(), self.tab_id);
        self.rt.spawn(async move {
            let progress = Progress { tx: tx.clone(), tab, id, done: 0 };
            let result = if upload {
                upload_path(sftp, PathBuf::from(src), dst, progress).await
            } else {
                download_path(sftp, src, PathBuf::from(dst), progress).await
            };
            let _ = tx.send(Msg::TransferDone { tab, id, result: result.map_err(|e| format!("{e:#}")) });
        });
        Ok(())
    }

    pub fn on_transfer_progress(&mut self, id: u64, done: u64) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.done = done;
        }
    }

    pub fn on_transfer_done(&mut self, id: u64, result: Result<(), String>) {
        let mut target = None;
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.state = match result {
                Ok(()) => TransferState::Done,
                Err(e) => TransferState::Failed(e),
            };
            target = Some(if t.label.starts_with('↑') { Side::Remote } else { Side::Local });
        }
        if let Some(side) = target {
            self.refresh(side);
        }
    }

    pub fn clear_finished_transfers(&mut self) {
        self.transfers.retain(|t| t.state == TransferState::Running);
    }

    /// Runs a filesystem operation on the focused side and refreshes it afterwards.
    pub fn file_op(&mut self, op: FileOp) {
        let side = self.focus;
        let base = self.pane(side).path.clone();
        match side {
            Side::Local => {
                let r = local_op(&base, &op);
                if let Err(e) = r {
                    self.local.error = Some(e.to_string());
                }
                self.list_local(base, op.keep_name());
            }
            Side::Remote => {
                let Some(sftp) = self.session.clone() else { return };
                let (tx, tab) = (self.tx.clone(), self.tab_id);
                self.rt.spawn(async move {
                    let result = remote_op(&sftp, &base, &op).await.map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Msg::RemoteOpDone { tab, result, keep: op.keep_name() });
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
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
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

fn local_op(base: &str, op: &FileOp) -> Result<()> {
    let base = Path::new(base);
    match op {
        FileOp::Mkdir(n) => std::fs::create_dir(base.join(n))?,
        FileOp::Rename { from, to } => std::fs::rename(base.join(from), base.join(to))?,
        FileOp::Delete { name, is_dir: true } => std::fs::remove_dir_all(base.join(name))?,
        FileOp::Delete { name, is_dir: false } => std::fs::remove_file(base.join(name))?,
    }
    Ok(())
}

async fn remote_op(sftp: &SftpSession, base: &str, op: &FileOp) -> Result<()> {
    let j = |n: &str| join(Side::Remote, base, n);
    match op {
        FileOp::Mkdir(n) => sftp.create_dir(j(n)).await?,
        FileOp::Rename { from, to } => sftp.rename(j(from), j(to)).await?,
        FileOp::Delete { name, is_dir: true } => remove_remote_dir(sftp, j(name)).await?,
        FileOp::Delete { name, is_dir: false } => sftp.remove_file(j(name)).await?,
    }
    Ok(())
}

async fn remove_remote_dir(sftp: &SftpSession, path: String) -> Result<()> {
    // Iterative depth-first delete: collect dirs, delete files on the way, dirs in reverse.
    let mut stack = vec![path];
    let mut dirs = vec![];
    while let Some(dir) = stack.pop() {
        for e in read_remote_dir(sftp, &dir).await? {
            let p = join(Side::Remote, &dir, &e.name);
            if e.is_dir {
                stack.push(p);
            } else {
                sftp.remove_file(p).await?;
            }
        }
        dirs.push(dir);
    }
    for d in dirs.into_iter().rev() {
        sftp.remove_dir(d).await?;
    }
    Ok(())
}

fn read_local_dir(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut out = vec![];
    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        let meta = entry.metadata().ok();
        let is_dir = ft.is_dir() || (ft.is_symlink() && entry.path().is_dir());
        out.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir,
            is_symlink: ft.is_symlink(),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            modified: meta.and_then(|m| m.modified().ok()),
        });
    }
    Ok(out)
}

async fn read_remote_dir(sftp: &SftpSession, path: &str) -> Result<Vec<FileEntry>> {
    let mut out = vec![];
    for e in sftp.read_dir(path).await? {
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let meta = e.metadata();
        let ft = e.file_type();
        let mut is_dir = ft.is_dir();
        if ft.is_symlink() {
            // Follow the link to decide if it can be entered.
            if let Ok(m) = sftp.metadata(join(Side::Remote, path, &name)).await {
                is_dir = m.file_type().is_dir();
            }
        }
        out.push(FileEntry { name, is_dir, is_symlink: ft.is_symlink(), size: meta.len(), modified: meta.modified().ok() });
    }
    Ok(out)
}

struct Progress {
    tx: UnboundedSender<Msg>,
    tab: u64,
    id: u64,
    done: u64,
}

impl Progress {
    fn add(&mut self, n: u64) {
        self.done += n;
        let _ = self.tx.send(Msg::TransferProgress { tab: self.tab, id: self.id, done: self.done });
    }
}

async fn upload_path(sftp: Arc<SftpSession>, src: PathBuf, dst: String, mut p: Progress) -> Result<()> {
    let mut stack = vec![(src, dst)];
    while let Some((src, dst)) = stack.pop() {
        if src.is_dir() {
            if !sftp.try_exists(dst.clone()).await.unwrap_or(false) {
                sftp.create_dir(dst.clone()).await.with_context(|| format!("mkdir {dst}"))?;
            }
            for e in std::fs::read_dir(&src)? {
                let e = e?;
                let name = e.file_name().to_string_lossy().to_string();
                stack.push((e.path(), join(Side::Remote, &dst, &name)));
            }
        } else {
            let mut input = tokio::fs::File::open(&src).await.with_context(|| format!("open {}", src.display()))?;
            let mut output = sftp.create(dst.clone()).await.with_context(|| format!("create {dst}"))?;
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = input.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n]).await?;
                p.add(n as u64);
            }
            output.shutdown().await?;
        }
    }
    Ok(())
}

async fn download_path(sftp: Arc<SftpSession>, src: String, dst: PathBuf, mut p: Progress) -> Result<()> {
    let mut stack = vec![(src, dst)];
    while let Some((src, dst)) = stack.pop() {
        let meta = sftp.metadata(src.clone()).await.with_context(|| format!("stat {src}"))?;
        if meta.file_type().is_dir() {
            std::fs::create_dir_all(&dst)?;
            for e in read_remote_dir(&sftp, &src).await? {
                stack.push((join(Side::Remote, &src, &e.name), dst.join(&e.name)));
            }
        } else {
            let mut input = sftp.open(src.clone()).await.with_context(|| format!("open {src}"))?;
            let mut output =
                tokio::fs::File::create(&dst).await.with_context(|| format!("create {}", dst.display()))?;
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = input.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n]).await?;
                p.add(n as u64);
            }
            output.flush().await?;
        }
    }
    Ok(())
}

/// Strips Windows' `\\?\` prefix from canonicalized paths.
fn clean_local(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

pub fn join(side: Side, base: &str, name: &str) -> String {
    match side {
        Side::Local => Path::new(base).join(name).display().to_string(),
        Side::Remote => {
            if name.starts_with('/') {
                name.to_string()
            } else if base.ends_with('/') {
                format!("{base}{name}")
            } else {
                format!("{base}/{name}")
            }
        }
    }
}

/// Returns `(parent, current_dir_name)`, or `None` at the root.
pub fn parent(side: Side, path: &str) -> Option<(String, String)> {
    match side {
        Side::Local => {
            let p = Path::new(path);
            let name = p.file_name()?.to_string_lossy().to_string();
            Some((p.parent()?.display().to_string(), name))
        }
        Side::Remote => {
            let trimmed = path.trim_end_matches('/');
            if trimmed.is_empty() {
                return None;
            }
            let (parent, name) = trimmed.rsplit_once('/')?;
            let parent = if parent.is_empty() { "/" } else { parent };
            Some((parent.to_string(), name.to_string()))
        }
    }
}

pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n}B") } else { format!("{v:.1}{}", UNITS[u]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths() {
        assert_eq!(join(Side::Remote, "/", "etc"), "/etc");
        assert_eq!(join(Side::Remote, "/home/u", "x"), "/home/u/x");
        assert_eq!(parent(Side::Remote, "/home/u"), Some(("/home".into(), "u".into())));
        assert_eq!(parent(Side::Remote, "/home"), Some(("/".into(), "home".into())));
        assert_eq!(parent(Side::Remote, "/"), None);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(2048), "2.0K");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0M");
    }

    #[test]
    fn pane_sorting_and_hidden() {
        let mut p = Pane::default();
        let f = |n: &str, d: bool| FileEntry { name: n.into(), is_dir: d, is_symlink: false, size: 0, modified: None };
        p.set_entries("/x".into(), vec![f("b", false), f(".hid", false), f("A", true), f("a", false)], false, None);
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "A", "a", "b"]);
    }
}
