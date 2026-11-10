//! Copies a file or directory tree between any two endpoints. Data streams
//! from source to destination in chunks; a remote→remote copy is relayed
//! through this machine without touching the local disk.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::endpoint::Endpoint;
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
                    let child_dst = dst.join(&d, &c.name);
                    stack.push((src.join(&s, &c.name), c.is_dir, child_dst));
                }
                continue;
            }
            if let Some(e) = existing {
                if e.is_dir {
                    bail!("{d} is a directory");
                }
                if !self.should_overwrite(&d).await? {
                    self.summary.skipped += 1;
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

    async fn copy_file(&mut self, src: &Endpoint, s: &str, dst: &Endpoint, d: &str) -> Result<Copied> {
        self.current = d.to_string();
        let mut reader = src.open_read(s).await?;
        let mut writer = dst.create(d).await?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = tokio::select! {
                r = reader.read(&mut buf) => r?,
                _ = self.token.cancelled() => {
                    drop(writer);
                    let _ = dst.remove(d, false).await;
                    return Ok(Copied::Cancelled);
                }
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
