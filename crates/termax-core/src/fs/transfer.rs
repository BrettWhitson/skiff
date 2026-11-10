//! Copies a file or directory tree between any two endpoints. Data streams
//! from source to destination in chunks; a remote→remote copy is relayed
//! through this machine without touching the local disk.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::endpoint::{Endpoint, WriteMeta};
use super::path::is_safe_name;

/// True if `dst` is `src` itself or a path inside it.
fn is_same_or_inside(ep: &Endpoint, dst: &str, src: &str) -> bool {
    if ep.is_remote() {
        let (d, s) = (dst.trim_end_matches('/'), src.trim_end_matches('/'));
        return d == s || d.starts_with(&format!("{s}/")) || s.is_empty();
    }
    let canon = |p: &str| {
        let p = std::path::Path::new(p);
        // dst usually doesn't exist yet: canonicalize its parent.
        match (p.parent(), p.file_name()) {
            (Some(parent), Some(name)) => std::fs::canonicalize(parent).map(|c| c.join(name)),
            _ => std::fs::canonicalize(p),
        }
        .unwrap_or_else(|_| p.to_path_buf())
    };
    canon(dst).starts_with(canon(src))
}

/// Suffix of the temporary files transfers write; such files are never copied.
const PART_SUFFIX: &str = ".termax-part";

/// The temporary sibling a file is written to before it replaces `d`:
/// `.<name>.<pid>-<job>-<n>.termax-part`, unique per copy so concurrent
/// transfers to one file never share it, with `<name>` shortened so the whole
/// name stays within the usual 255-byte limit.
fn part_path(ep: &Endpoint, d: &str, job: u64) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tag = format!(".{}-{job}-{}{PART_SUFFIX}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let (dir, name) = super::path::parent(ep.is_remote(), d).unwrap_or_default();
    let mut end = name.len().min(255 - 1 - tag.len());
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let file = format!(".{}{tag}", &name[..end]);
    if dir.is_empty() { file } else { ep.join(&dir, &file) }
}

/// True if `e` (anywhere in its chain) is a local or SFTP permission error.
fn is_permission_denied(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
            || matches!(c.downcast_ref::<SftpError>(), Some(SftpError::Status(s)) if s.status_code == StatusCode::PermissionDenied)
    })
}

const CHUNK: usize = 256 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

pub struct TransferRequest {
    pub id: u64,
    pub src: Endpoint,
    /// The file or directory to copy.
    pub src_path: String,
    pub dst: Endpoint,
    /// The directory to copy it into.
    pub dst_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwritePolicy {
    Ask,
    Overwrite,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwriteDecision {
    Overwrite,
    Skip,
    OverwriteAll,
    SkipAll,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferSummary {
    pub files: u64,
    pub bytes: u64,
    pub skipped: u64,
    pub cancelled: bool,
}

pub enum TransferEvent {
    /// `current` is the file being copied; `total` is known for single files.
    Progress {
        id: u64,
        done: u64,
        total: Option<u64>,
        current: String,
    },
    /// The destination file exists; answer on `reply` (dropping it cancels).
    NeedsDecision {
        id: u64,
        path: String,
        reply: oneshot::Sender<OverwriteDecision>,
    },
    Done {
        id: u64,
        result: Result<TransferSummary, String>,
    },
}

#[derive(Clone)]
pub struct TransferHandle {
    token: CancellationToken,
}

impl TransferHandle {
    pub fn cancel(&self) {
        self.token.cancel();
    }
}

pub fn start(rt: &Handle, req: TransferRequest, policy: OverwritePolicy, events: UnboundedSender<TransferEvent>) -> TransferHandle {
    let token = CancellationToken::new();
    let id = req.id;
    let mut job = Job {
        id,
        policy,
        events: events.clone(),
        token: token.clone(),
        summary: TransferSummary::default(),
        done: 0,
        total: None,
        last_emit: Instant::now() - PROGRESS_EVERY,
        current: String::new(),
    };
    rt.spawn(async move {
        let result = job.run(req).await.map_err(|e| format!("{e:#}"));
        let _ = events.send(TransferEvent::Done { id, result });
    });
    TransferHandle { token }
}

struct Job {
    id: u64,
    policy: OverwritePolicy,
    events: UnboundedSender<TransferEvent>,
    token: CancellationToken,
    summary: TransferSummary,
    done: u64,
    total: Option<u64>,
    last_emit: Instant,
    current: String,
}

enum Copied {
    Done,
    Cancelled,
}

impl Job {
    async fn run(&mut self, req: TransferRequest) -> Result<TransferSummary> {
        let (src, dst) = (&req.src, &req.dst);
        let root = src.stat(&req.src_path).await?.ok_or_else(|| anyhow!("{} does not exist", req.src_path))?;
        if !root.is_dir {
            self.total = Some(root.size);
        }
        let root_dst = dst.join(&req.dst_dir, &root.name);
        if src.same_as(dst) && is_same_or_inside(dst, &root_dst, &req.src_path) {
            bail!("cannot copy {} onto or into itself", req.src_path);
        }
        // (source path, is_dir, destination path)
        let mut stack = vec![(req.src_path.clone(), root.is_dir, root_dst)];
        while let Some((s, is_dir, d)) = stack.pop() {
            if self.token.is_cancelled() {
                self.summary.cancelled = true;
                break;
            }
            let existing = dst.stat(&d).await?;
            if is_dir {
                match existing {
                    Some(e) if !e.is_dir => bail!("{d} exists and is not a directory"),
                    Some(_) => {}
                    None => dst.mkdir(&d).await?,
                }
                let mut children = src.list(&s).await?;
                children.sort_by(|a, b| b.name.cmp(&a.name)); // popped in name order
                for c in children {
                    // Links to directories are not followed (no loops, no surprise
                    // copies of whole trees), and names that aren't a single
                    // ordinary path component are never written.
                    if (c.is_symlink && c.is_dir) || !is_safe_name(&c.name) {
                        self.summary.skipped += 1;
                        continue;
                    }
                    // Leftovers of interrupted transfers are not real content.
                    if !c.is_dir && c.name.ends_with(PART_SUFFIX) {
                        continue;
                    }
                    let child_dst = dst.join(&d, &c.name);
                    stack.push((src.join(&s, &c.name), c.is_dir, child_dst));
                }
                continue;
            }
            if let Some(e) = &existing {
                if e.is_dir {
                    bail!("{d} is a directory");
                }
                if !self.should_overwrite(&d).await? {
                    self.summary.skipped += 1;
                    // Cancelled while asking: say so even when this was the last file.
                    if self.token.is_cancelled() {
                        self.summary.cancelled = true;
                        break;
                    }
                    continue;
                }
            }
            let exists = existing.is_some();
            match self.copy_file(src, &s, dst, &d, exists).await? {
                Copied::Done => self.summary.files += 1,
                Copied::Cancelled => {
                    self.summary.cancelled = true;
                    break;
                }
            }
        }
        self.emit(true);
        Ok(self.summary.clone())
    }

    async fn should_overwrite(&mut self, path: &str) -> Result<bool> {
        match self.policy {
            OverwritePolicy::Overwrite => return Ok(true),
            OverwritePolicy::Skip => return Ok(false),
            OverwritePolicy::Ask => {}
        }
        let (tx, rx) = oneshot::channel();
        let _ = self.events.send(TransferEvent::NeedsDecision { id: self.id, path: path.to_string(), reply: tx });
        let decision = tokio::select! {
            d = rx => d.map_err(|_| anyhow!("overwrite question was dismissed"))?,
            _ = self.token.cancelled() => return Ok(false),
        };
        Ok(match decision {
            OverwriteDecision::Overwrite => true,
            OverwriteDecision::Skip => false,
            OverwriteDecision::OverwriteAll => {
                self.policy = OverwritePolicy::Overwrite;
                true
            }
            OverwriteDecision::SkipAll => {
                self.policy = OverwritePolicy::Skip;
                false
            }
        })
    }

    /// Normally writes into a hidden sibling (see [`part_path`]) and moves it
    /// over `d` only once complete, so a cancelled or failed copy never touches
    /// an existing destination file; only the partial temp file is removed. An
    /// existing destination's permission bits are copied onto the temp first.
    ///
    /// An existing `d` is instead rewritten in place (truncated, then written)
    /// when replacing it would lose something: it is a symlink, it belongs to
    /// another user, its permissions can't be copied, or the directory doesn't
    /// allow new files. In that mode a cancelled or failed copy leaves the
    /// partially written `d` behind.
    async fn copy_file(&mut self, src: &Endpoint, s: &str, dst: &Endpoint, d: &str, exists: bool) -> Result<Copied> {
        self.current = d.to_string();
        let reader = src.open_read(s).await?;
        let dest = if exists { Some(dst.write_meta(d).await?) } else { None };
        if dest.is_some_and(|m| m.is_symlink) {
            return self.stream(reader, dst.create(d).await?).await;
        }
        let tmp = part_path(dst, d, self.id);
        let writer = match dst.create(&tmp).await {
            Ok(w) => w,
            Err(e) if exists && is_permission_denied(&e) => return self.stream(reader, dst.create(d).await?).await,
            Err(e) => return Err(e),
        };
        if let Some(dest) = dest
            && !self.adopt_meta(dst, &tmp, dest).await
        {
            drop(writer);
            let _ = dst.remove(&tmp, false).await;
            return self.stream(reader, dst.create(d).await?).await;
        }
        let result = self.stream(reader, writer).await;
        match result {
            Ok(Copied::Done) => {
                if let Err(e) = dst.replace(&tmp, d).await {
                    // Keep the finished copy if the original is already gone.
                    if matches!(dst.stat(d).await, Ok(Some(_))) {
                        let _ = dst.remove(&tmp, false).await;
                        return Err(e);
                    }
                    return Err(e.context(format!("the new copy was left at {tmp}")));
                }
                Ok(Copied::Done)
            }
            other => {
                let _ = dst.remove(&tmp, false).await;
                other
            }
        }
    }

    /// Gives the fresh temp file `tmp` the destination's permission bits. False
    /// if the replacement would not keep the destination's owner or mode.
    async fn adopt_meta(&self, dst: &Endpoint, tmp: &str, dest: WriteMeta) -> bool {
        let Ok(mine) = dst.write_meta(tmp).await else { return false };
        if matches!((dest.uid, mine.uid), (Some(a), Some(b)) if a != b) {
            return false;
        }
        match dest.mode {
            Some(mode) if mine.mode != Some(mode) => dst.set_mode(tmp, mode).await.is_ok(),
            _ => true,
        }
    }

    async fn stream(
        &mut self,
        mut reader: Box<dyn AsyncRead + Send + Unpin>,
        mut writer: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> Result<Copied> {
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = tokio::select! {
                r = reader.read(&mut buf) => r?,
                _ = self.token.cancelled() => return Ok(Copied::Cancelled),
            };
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n]).await?;
            self.done += n as u64;
            self.summary.bytes += n as u64;
            self.emit(false);
        }
        writer.shutdown().await?;
        Ok(Copied::Done)
    }

    fn emit(&mut self, force: bool) {
        if !force && self.last_emit.elapsed() < PROGRESS_EVERY {
            return;
        }
        self.last_emit = Instant::now();
        let _ =
            self.events.send(TransferEvent::Progress { id: self.id, done: self.done, total: self.total, current: self.current.clone() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_paths_are_unique_hidden_and_bounded() {
        let ep = Endpoint::Local;
        let d = std::env::temp_dir().join("x.txt").display().to_string();
        let (a, b) = (part_path(&ep, &d, 7), part_path(&ep, &d, 7));
        assert_ne!(a, b, "concurrent copies to one file must not share a temp");
        let long = std::env::temp_dir().join("é".repeat(127)).display().to_string();
        let name = super::super::path::parent(false, &part_path(&ep, &long, u64::MAX)).unwrap().1;
        assert!(name.len() <= 255 && name.starts_with('.') && name.ends_with(".termax-part"), "{name}");
    }
}
