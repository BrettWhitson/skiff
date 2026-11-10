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

use super::endpoint::{Endpoint, FileKind, WriteMeta};
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

/// Refuses destinations a copy must not write to or replace.
fn check_destination(d: &str, m: &WriteMeta) -> Result<()> {
    match (m.kind, m.target) {
        (FileKind::File, _) | (FileKind::Symlink, Some(FileKind::File)) => {}
        (FileKind::Symlink, None) => bail!("{d} is a broken symlink"),
        (FileKind::Dir, _) | (FileKind::Symlink, Some(FileKind::Dir)) => bail!("{d} is a directory"),
        _ => bail!("{d} is not a regular file"),
    }
    if m.readonly {
        bail!("{d}: the destination is read-only");
    }
    Ok(())
}

/// Gives the finished temp file `tmp` the destination's group, then its
/// permission bits (a group change can clear set-id bits), and reads them
/// back. False if they didn't take (some servers ignore SETSTAT).
async fn adopt_meta(dst: &Endpoint, tmp: &str, dest: &WriteMeta, mine: &WriteMeta) -> bool {
    if let (Some(gid), Some(uid)) = (dest.gid, mine.uid)
        && mine.gid != Some(gid)
        && dst.set_group(tmp, uid, gid).await.is_err()
    {
        return false;
    }
    if let Some(mode) = dest.mode
        && dst.set_mode(tmp, mode).await.is_err()
    {
        return false;
    }
    match dst.write_meta(tmp).await {
        Ok(Some(now)) => (dest.mode.is_none() || now.mode == dest.mode) && (dest.gid.is_none() || now.gid == dest.gid),
        _ => false,
    }
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
                        self.summary.skipped += 1;
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
            match self.copy_file(src, &s, dst, &d).await? {
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

    /// Copies `s` to `d`. A new file is written into a hidden sibling (see
    /// [`part_path`]) and moved into place once complete. Overwriting a
    /// regular file works the same way, but the temp file is created private
    /// (`0600`) and only after the data is written gets the old file's group
    /// and permission bits (in that order, so set-id bits survive), read back
    /// to check they took. A cancelled or failed copy never touches the
    /// existing file; only the temp file is removed.
    ///
    /// The destination is instead rewritten in place when replacing it would
    /// lose something: it is a symlink (written through), it belongs to
    /// another user, its group or mode can't be given to the temp file, or the
    /// directory doesn't allow new files. In place, the file is overwritten
    /// from the start and cut to the new length only at the end, so a
    /// cancelled or failed copy leaves new data followed by the rest of the
    /// old.
    ///
    /// Refused before anything is written: a destination that isn't a regular
    /// file or a symlink to one, a broken symlink, a symlink owned by another
    /// user, and (Windows) a read-only file.
    async fn copy_file(&mut self, src: &Endpoint, s: &str, dst: &Endpoint, d: &str) -> Result<Copied> {
        self.current = d.to_string();
        let dest = dst.write_meta(d).await?;
        if let Some(m) = &dest {
            check_destination(d, m)?;
        }
        let reader = src.open_read(s).await?;
        let tmp = part_path(dst, d, self.id);
        let Some(dest) = dest else {
            let writer = dst.create_new(&tmp, false).await?;
            return self.finish_via_temp(reader, writer, dst, &tmp, d).await;
        };
        let writer = match dst.create_new(&tmp, true).await {
            Ok(w) => w,
            Err(e) if is_permission_denied(&e) => {
                if dest.kind == FileKind::Symlink && dest.uid.is_some() {
                    return Err(e.context(format!("{d} is a symlink and its owner can't be checked")));
                }
                return self.write_in_place(reader, dst, d).await;
            }
            Err(e) => return Err(e),
        };
        let mine = dst.write_meta(&tmp).await.ok().flatten();
        let foreign = matches!((dest.uid, mine.and_then(|m| m.uid)), (Some(a), Some(b)) if a != b);
        let Some(mine) = mine.filter(|_| dest.kind != FileKind::Symlink && !foreign) else {
            drop(writer);
            let _ = dst.remove(&tmp, false).await;
            if dest.kind == FileKind::Symlink && foreign {
                bail!("{d} is a symlink owned by another user; not writing through it");
            }
            return self.write_in_place(reader, dst, d).await;
        };
        let start = (self.done, self.summary.bytes);
        match self.stream(reader, writer).await {
            Ok(Copied::Done) => {}
            other => {
                let _ = dst.remove(&tmp, false).await;
                return other;
            }
        }
        if !adopt_meta(dst, &tmp, &dest, &mine).await {
            let _ = dst.remove(&tmp, false).await;
            (self.done, self.summary.bytes) = start;
            let reader = src.open_read(s).await?;
            return self.write_in_place(reader, dst, d).await;
        }
        self.replace_with(dst, &tmp, d).await
    }

    async fn finish_via_temp(
        &mut self,
        reader: Box<dyn AsyncRead + Send + Unpin>,
        writer: Box<dyn AsyncWrite + Send + Unpin>,
        dst: &Endpoint,
        tmp: &str,
        d: &str,
    ) -> Result<Copied> {
        match self.stream(reader, writer).await {
            Ok(Copied::Done) => self.replace_with(dst, tmp, d).await,
            other => {
                let _ = dst.remove(tmp, false).await;
                other
            }
        }
    }

    /// Moves the finished `tmp` over `d`.
    async fn replace_with(&mut self, dst: &Endpoint, tmp: &str, d: &str) -> Result<Copied> {
        if let Err(e) = dst.replace(tmp, d).await {
            // Keep the finished copy if the original is already gone.
            if matches!(dst.stat(d).await, Ok(Some(_))) {
                let _ = dst.remove(tmp, false).await;
                return Err(e);
            }
            return Err(e.context(format!("the new copy was left at {tmp}")));
        }
        Ok(Copied::Done)
    }

    /// Overwrites existing `d` from the start, then cuts it to the new length.
    async fn write_in_place(&mut self, reader: Box<dyn AsyncRead + Send + Unpin>, dst: &Endpoint, d: &str) -> Result<Copied> {
        let writer = dst.open_write(d).await?;
        let before = self.summary.bytes;
        let copied = self.stream(reader, writer).await?;
        if let Copied::Done = copied {
            dst.set_len(d, self.summary.bytes - before).await?;
        }
        Ok(copied)
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
