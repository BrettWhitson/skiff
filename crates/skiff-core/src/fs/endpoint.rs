//! One side of a file browser or transfer: the local disk or an SFTP session.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use tokio::io::{AsyncRead, AsyncWrite};

use super::connect::RemoteConn;
use super::path::{self, FileEntry};

/// What a path is, links not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link.
    Symlink,
    /// A FIFO, device, socket and the like.
    Other,
}

/// What a transfer needs to know about an existing destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteMeta {
    /// The path itself (a symlink is not followed).
    pub kind: FileKind,
    /// For a symlink, what it points to; `None` if nothing (a broken link).
    pub target: Option<FileKind>,
    /// Owner user and group ids, where the file system reports them (Unix, SFTP).
    pub uid: Option<u32>,
    /// Group id, where the file system reports it (Unix, SFTP).
    pub gid: Option<u32>,
    /// Permission bits (`0o7777`), where the file system has them.
    pub mode: Option<u32>,
    /// The Windows read-only attribute (always false elsewhere).
    pub readonly: bool,
}

/// Maps a local file type to a `FileKind`.
fn local_kind(ft: std::fs::FileType) -> FileKind {
    if ft.is_symlink() {
        FileKind::Symlink
    } else if ft.is_dir() {
        FileKind::Dir
    } else if ft.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    }
}

/// A server that sends no permissions is taken to mean a regular file.
/// Maps SFTP metadata to a `FileKind`.
fn remote_kind(m: &russh_sftp::client::fs::Metadata) -> FileKind {
    use russh_sftp::protocol::FileType;
    if m.permissions.is_none() {
        return FileKind::File;
    }
    match m.file_type() {
        FileType::Dir => FileKind::Dir,
        FileType::File => FileKind::File,
        FileType::Symlink => FileKind::Symlink,
        FileType::Other => FileKind::Other,
    }
}

/// Whether an SFTP error is "no such file".
fn no_such_file(e: &russh_sftp::client::error::Error) -> bool {
    matches!(e, russh_sftp::client::error::Error::Status(s) if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile)
}

/// Where files live: this computer or a remote SFTP connection.
#[derive(Clone)]
pub enum Endpoint {
    /// The local file system.
    Local,
    /// A remote file system over a shared SFTP connection.
    Remote(Arc<RemoteConn>),
}

impl Endpoint {
    /// Whether this is an SFTP endpoint.
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

    /// Joins `name` onto `base` using this side's path rules.
    pub fn join(&self, base: &str, name: &str) -> String {
        path::join(self.is_remote(), base, name)
    }

    /// Directory entries (without `.`/`..`), unsorted.
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        match self {
            // Local listing does blocking I/O, so it runs off the async threads.
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
                    out.push(FileEntry { name, is_dir, is_symlink: ft.is_symlink(), size: meta.len(), modified: meta.modified().ok() });
                }
                Ok(out)
            }
        }
    }

    /// Basic info about `p` (links followed); `None` if nothing exists at `p`.
    /// `None` if nothing exists at `p`.
    pub async fn stat(&self, p: &str) -> Result<Option<FileEntry>> {
        let name = path::parent(self.is_remote(), p).map(|(_, n)| n).unwrap_or_else(|| p.to_string());
        match self {
            Endpoint::Local => match std::fs::metadata(p) {
                Ok(m) => Ok(Some(FileEntry { name, is_dir: m.is_dir(), is_symlink: false, size: m.len(), modified: m.modified().ok() })),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            Endpoint::Remote(c) => {
                if !c.sftp.try_exists(p).await? {
                    return Ok(None);
                }
                let m = c.sftp.metadata(p).await?;
                Ok(Some(FileEntry { name, is_dir: m.file_type().is_dir(), is_symlink: false, size: m.len(), modified: m.modified().ok() }))
            }
        }
    }

    /// Creates one directory (parents must exist).
    pub async fn mkdir(&self, p: &str) -> Result<()> {
        match self {
            Endpoint::Local => tokio::fs::create_dir(p).await.with_context(|| format!("mkdir {p}")),
            Endpoint::Remote(c) => c.sftp.create_dir(p).await.with_context(|| format!("mkdir {p}")),
        }
    }

    /// Renames `from` to `to` on this side.
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        match self {
            Endpoint::Local => tokio::fs::rename(from, to).await.with_context(|| format!("rename {from}")),
            Endpoint::Remote(c) => c.sftp.rename(from, to).await.with_context(|| format!("rename {from}")),
        }
    }

    /// Moves file `from` over `to`, replacing it if it exists. Local renames
    /// replace atomically. SFTP v3 servers (OpenSSH) refuse to rename onto an
    /// existing file, and russh-sftp 3.0 keeps its raw session (the only way to
    /// send `posix-rename@openssh.com`) private, so a remote replace falls back
    /// to remove-then-rename: if the connection drops in between, `to` is gone
    /// but the complete new copy remains at `from`.
    pub async fn replace(&self, from: &str, to: &str) -> Result<()> {
        match self {
            Endpoint::Local => self.rename(from, to).await,
            Endpoint::Remote(c) => {
                // Try a plain rename first; servers that allow overwriting succeed here.
                if c.sftp.rename(from, to).await.is_ok() {
                    return Ok(());
                }
                if c.sftp.try_exists(to).await.unwrap_or(false) {
                    c.sftp.remove_file(to).await.with_context(|| format!("replacing {to}"))?;
                }
                self.rename(from, to).await
            }
        }
    }

    /// Removes a file, or a directory (with `recursive`, including its contents).
    /// A symlink is removed itself; its target is never touched.
    pub async fn remove(&self, p: &str, recursive: bool) -> Result<()> {
        let entry = self.stat(p).await?.ok_or_else(|| anyhow!("{p} does not exist"))?;
        match self {
            Endpoint::Local => {
                // Local: check the path itself (not its target) to see if it is a link.
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
                // Remote: a file or a link is removed directly; a directory needs care.
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

    /// Type, owner and permissions of `p` itself (links not followed);
    /// `None` if nothing is there.
    pub async fn write_meta(&self, p: &str) -> Result<Option<WriteMeta>> {
        match self {
            Endpoint::Local => {
                let m = match tokio::fs::symlink_metadata(p).await {
                    Ok(m) => m,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(e).with_context(|| format!("stat {p}")),
                };
                let kind = local_kind(m.file_type());
                let target = match kind {
                    FileKind::Symlink => match tokio::fs::metadata(p).await {
                        Ok(t) => Some(local_kind(t.file_type())),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                        Err(e) => return Err(e).with_context(|| format!("stat {p}")),
                    },
                    _ => None,
                };
                // Owner and mode come from Unix metadata; elsewhere they are unknown.
                #[cfg(unix)]
                let (uid, gid, mode) = {
                    use std::os::unix::fs::MetadataExt;
                    (Some(m.uid()), Some(m.gid()), Some(m.mode() & 0o7777))
                };
                #[cfg(not(unix))]
                let (uid, gid, mode) = (None, None, None);
                let readonly = cfg!(windows) && kind == FileKind::File && m.permissions().readonly();
                Ok(Some(WriteMeta { kind, target, uid, gid, mode, readonly }))
            }
            Endpoint::Remote(c) => {
                let m = match c.sftp.symlink_metadata(p).await {
                    Ok(m) => m,
                    Err(e) if no_such_file(&e) => return Ok(None),
                    Err(e) => return Err(e).with_context(|| format!("stat {p}")),
                };
                let kind = remote_kind(&m);
                let target = match kind {
                    FileKind::Symlink => match c.sftp.metadata(p).await {
                        Ok(t) => Some(remote_kind(&t)),
                        Err(e) if no_such_file(&e) => None,
                        Err(e) => return Err(e).with_context(|| format!("stat {p}")),
                    },
                    _ => None,
                };
                let mode = m.permissions.map(|m| m & 0o7777);
                Ok(Some(WriteMeta { kind, target, uid: m.uid, gid: m.gid, mode, readonly: false }))
            }
        }
    }

    /// Sets the permission bits of `p`. A no-op on Windows, where new files
    /// already get the directory's inherited ACL.
    pub async fn set_mode(&self, p: &str, mode: u32) -> Result<()> {
        match self {
            Endpoint::Local => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    tokio::fs::set_permissions(p, std::fs::Permissions::from_mode(mode & 0o7777))
                        .await
                        .with_context(|| format!("chmod {p}"))?;
                }
                #[cfg(not(unix))]
                let _ = (p, mode);
                Ok(())
            }
            Endpoint::Remote(c) => {
                let attrs = russh_sftp::client::fs::Metadata { permissions: Some(mode & 0o7777), ..Default::default() };
                c.sftp.set_metadata(p, attrs).await.with_context(|| format!("chmod {p}"))
            }
        }
    }

    /// Sets the group of `p` (SFTP sends the owner `uid` along, unchanged). A
    /// no-op on Windows.
    pub async fn set_group(&self, p: &str, uid: u32, gid: u32) -> Result<()> {
        match self {
            Endpoint::Local => {
                let _ = uid; // a local chown keeps the owner
                #[cfg(unix)]
                std::os::unix::fs::chown(p, None, Some(gid)).with_context(|| format!("chgrp {p}"))?;
                #[cfg(not(unix))]
                let _ = (p, gid);
                Ok(())
            }
            Endpoint::Remote(c) => {
                let attrs = russh_sftp::client::fs::Metadata { uid: Some(uid), gid: Some(gid), ..Default::default() };
                c.sftp.set_metadata(p, attrs).await.with_context(|| format!("chgrp {p}"))
            }
        }
    }

    /// Truncates (or extends) file `p` to `len` bytes.
    pub async fn set_len(&self, p: &str, len: u64) -> Result<()> {
        match self {
            Endpoint::Local => {
                let f = tokio::fs::OpenOptions::new().write(true).open(p).await.with_context(|| format!("open {p}"))?;
                f.set_len(len).await.with_context(|| format!("truncate {p}"))
            }
            Endpoint::Remote(c) => {
                let attrs = russh_sftp::client::fs::Metadata { size: Some(len), ..Default::default() };
                c.sftp.set_metadata(p, attrs).await.with_context(|| format!("truncate {p}"))
            }
        }
    }

    /// Opens file `p` for reading.
    pub async fn open_read(&self, p: &str) -> Result<Box<dyn AsyncRead + Send + Unpin>> {
        Ok(match self {
            Endpoint::Local => Box::new(tokio::fs::File::open(p).await.with_context(|| format!("open {p}"))?),
            Endpoint::Remote(c) => Box::new(c.sftp.open(p).await.with_context(|| format!("open {p}"))?),
        })
    }

    /// Creates file `p`, failing if anything (even a symlink) is there.
    /// `private` makes it readable by the owner only (`0600`; Unix and SFTP).
    pub async fn create_new(&self, p: &str, private: bool) -> Result<Box<dyn AsyncWrite + Send + Unpin>> {
        Ok(match self {
            Endpoint::Local => {
                let mut o = tokio::fs::OpenOptions::new();
                o.write(true).create_new(true);
                #[cfg(unix)]
                {
                    if private {
                        o.mode(0o600);
                    }
                }
                #[cfg(not(unix))]
                let _ = private;
                Box::new(o.open(p).await.with_context(|| format!("create {p}"))?)
            }
            Endpoint::Remote(c) => {
                use russh_sftp::protocol::OpenFlags;
                let attrs = russh_sftp::client::fs::Metadata { permissions: private.then_some(0o600), ..Default::default() };
                // EXCLUDE makes the server fail if the path already exists.
                let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE;
                Box::new(c.sftp.open_with_flags_and_attributes(p, flags, attrs).await.with_context(|| format!("create {p}"))?)
            }
        })
    }

    /// Opens existing file `p` for writing from the start, without truncating it.
    pub async fn open_write(&self, p: &str) -> Result<Box<dyn AsyncWrite + Send + Unpin>> {
        Ok(match self {
            Endpoint::Local => Box::new(tokio::fs::OpenOptions::new().write(true).open(p).await.with_context(|| format!("open {p}"))?),
            Endpoint::Remote(c) => {
                let flags = russh_sftp::protocol::OpenFlags::WRITE;
                Box::new(c.sftp.open_with_flags(p, flags).await.with_context(|| format!("open {p}"))?)
            }
        })
    }
}
