//! One side of a file browser or transfer: the local disk or an SFTP session.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use tokio::io::{AsyncRead, AsyncWrite};

use super::connect::RemoteConn;
use super::path::{self, FileEntry};

#[derive(Clone)]
pub enum Endpoint {
    Local,
    Remote(Arc<RemoteConn>),
}

impl Endpoint {
    pub fn is_remote(&self) -> bool {
        matches!(self, Endpoint::Remote(_))
    }

    /// True if both refer to the same file system (local, or one SFTP session).
    pub fn same_as(&self, other: &Endpoint) -> bool {
        match (self, other) {
            (Endpoint::Local, Endpoint::Local) => true,
            (Endpoint::Remote(a), Endpoint::Remote(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub fn join(&self, base: &str, name: &str) -> String {
        path::join(self.is_remote(), base, name)
    }

    /// Directory entries (without `.`/`..`), unsorted.
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        match self {
            Endpoint::Local => {
                let dir = dir.to_string();
                tokio::task::spawn_blocking(move || path::list_local(std::path::Path::new(&dir)))
                    .await?
                    .with_context(|| "listing directory")
            }
            Endpoint::Remote(c) => {
                let mut out = vec![];
                for e in c.sftp.read_dir(dir).await.with_context(|| format!("listing {dir}"))? {
                    let name = e.file_name();
                    if name == "." || name == ".." {
                        continue;
                    }
                    let meta = e.metadata();
                    let ft = e.file_type();
                    let mut is_dir = ft.is_dir();
                    if ft.is_symlink() {
                        // Follow links so they can be entered.
                        if let Ok(m) = c.sftp.metadata(path::join(true, dir, &name)).await {
                            is_dir = m.file_type().is_dir();
                        }
                    }
                    out.push(FileEntry {
                        name,
                        is_dir,
                        is_symlink: ft.is_symlink(),
                        size: meta.len(),
                        modified: meta.modified().ok(),
                    });
                }
                Ok(out)
            }
        }
    }

    /// `None` if nothing exists at `p`.
    pub async fn stat(&self, p: &str) -> Result<Option<FileEntry>> {
        let name = path::parent(self.is_remote(), p).map(|(_, n)| n).unwrap_or_else(|| p.to_string());
        match self {
            Endpoint::Local => match std::fs::metadata(p) {
                Ok(m) => Ok(Some(FileEntry {
                    name,
                    is_dir: m.is_dir(),
                    is_symlink: false,
                    size: m.len(),
                    modified: m.modified().ok(),
                })),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            Endpoint::Remote(c) => {
                if !c.sftp.try_exists(p).await? {
                    return Ok(None);
                }
                let m = c.sftp.metadata(p).await?;
                Ok(Some(FileEntry {
                    name,
                    is_dir: m.file_type().is_dir(),
                    is_symlink: false,
                    size: m.len(),
                    modified: m.modified().ok(),
                }))
            }
        }
    }

    pub async fn mkdir(&self, p: &str) -> Result<()> {
        match self {
            Endpoint::Local => tokio::fs::create_dir(p).await.with_context(|| format!("mkdir {p}")),
            Endpoint::Remote(c) => c.sftp.create_dir(p).await.with_context(|| format!("mkdir {p}")),
        }
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        match self {
            Endpoint::Local => tokio::fs::rename(from, to).await.with_context(|| format!("rename {from}")),
            Endpoint::Remote(c) => c.sftp.rename(from, to).await.with_context(|| format!("rename {from}")),
        }
    }

    /// Removes a file, or a directory (with `recursive`, including its contents).
    /// A symlink is removed itself; its target is never touched.
    pub async fn remove(&self, p: &str, recursive: bool) -> Result<()> {
        let entry = self.stat(p).await?.ok_or_else(|| anyhow!("{p} does not exist"))?;
        match self {
            Endpoint::Local => {
                let link = std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false);
                if link {
                    // A directory link on Windows must be removed with remove_dir.
                    if tokio::fs::remove_file(p).await.is_err() {
                        tokio::fs::remove_dir(p).await?;
                    }
                } else if entry.is_dir && recursive {
                    tokio::fs::remove_dir_all(p).await?
                } else if entry.is_dir {
                    tokio::fs::remove_dir(p).await?
                } else {
                    tokio::fs::remove_file(p).await?
                }
            }
            Endpoint::Remote(c) => {
                let link = c.sftp.symlink_metadata(p).await?.file_type().is_symlink();
                if !entry.is_dir || link {
                    return Ok(c.sftp.remove_file(p).await?);
                }
                if !recursive {
                    return Ok(c.sftp.remove_dir(p).await?);
                }
                // Depth-first: files on the way down, directories in reverse.
                let mut stack = vec![p.to_string()];
                let mut dirs = vec![];
                while let Some(dir) = stack.pop() {
                    for e in self.list(&dir).await? {
                        let child = path::join(true, &dir, &e.name);
                        if e.is_dir && !e.is_symlink {
                            stack.push(child);
                        } else {
                            c.sftp.remove_file(child).await?;
                        }
                    }
                    dirs.push(dir);
                }
                for d in dirs.into_iter().rev() {
                    c.sftp.remove_dir(d).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn open_read(&self, p: &str) -> Result<Box<dyn AsyncRead + Send + Unpin>> {
        Ok(match self {
            Endpoint::Local => Box::new(tokio::fs::File::open(p).await.with_context(|| format!("open {p}"))?),
            Endpoint::Remote(c) => Box::new(c.sftp.open(p).await.with_context(|| format!("open {p}"))?),
        })
    }

    /// Creates (or truncates) a file for writing.
    pub async fn create(&self, p: &str) -> Result<Box<dyn AsyncWrite + Send + Unpin>> {
        Ok(match self {
            Endpoint::Local => Box::new(tokio::fs::File::create(p).await.with_context(|| format!("create {p}"))?),
            Endpoint::Remote(c) => Box::new(c.sftp.create(p).await.with_context(|| format!("create {p}"))?),
        })
    }
}
