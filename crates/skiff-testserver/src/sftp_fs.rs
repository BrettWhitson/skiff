//! SFTP handler serving a local directory as `/`. Paths that would leave the
//! root are refused.
//!
//! Like a POSIX server, every file reports an owner (1000 unless set) and
//! SETSTAT of permissions/owner is honoured, but only emulated: the values
//! are kept in memory per path (moved by rename, dropped by remove), never
//! applied to the real file. A mode with file type bits makes the file
//! report that type (e.g. `0o010644`, a FIFO). A file created with
//! permissions gets them. A directory whose emulated mode lacks the owner
//! write bit refuses new files. If the root holds `.ignore-temp-setstat`,
//! SETSTAT on `*.skiff-part` files succeeds but changes nothing, like a
//! server that ignores it. A SETSTAT size really truncates the file.

use std::collections::HashMap;
use std::io::{ErrorKind, SeekFrom};
use std::path::PathBuf;

use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// What an open SFTP handle refers to.
enum Open {
    /// An open local file (from OPEN).
    File(tokio::fs::File),
    /// A directory listing read in full at OPENDIR, handed out by READDIR.
    Dir(Vec<File>),
}

/// One SFTP session's state: the served root, its open handles and the
/// emulated SETSTAT values. A new one is created per `sftp` subsystem request.
pub struct SftpFs {
    /// Local directory served as `/`.
    root: PathBuf,
    /// Open handles by their wire name (`h1`, `h2`, ...).
    handles: HashMap<String, Open>,
    /// Number used for the next handle name.
    next: u64,
    /// Emulated SETSTAT values by virtual path: (permission bits, uid, gid).
    emulated: HashMap<String, Emulated>,
}

/// Emulated permission bits, uid and gid of one path.
type Emulated = (Option<u32>, Option<u32>, Option<u32>);

/// uid and gid reported when none has been set.
const DEFAULT_OWNER: u32 = 1000;

/// An `Ok` status reply for request `id`.
fn status(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

/// Like `FileAttributes::from`, but symlinks are reported as links (the
/// library's conversion reports them as regular files).
fn attrs_of(m: &std::fs::Metadata) -> FileAttributes {
    let mut a = FileAttributes::from(m);
    if m.file_type().is_symlink() {
        const LNK: u32 = 0o120000;
        a.permissions = Some((a.permissions.unwrap_or(0o777) & 0o7777) | LNK);
    }
    a
}

/// Maps a local I/O error to the closest SFTP status code.
fn io_err(e: std::io::Error) -> StatusCode {
    match e.kind() {
        ErrorKind::NotFound => StatusCode::NoSuchFile,
        ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

impl SftpFs {
    /// A handler serving `root` as `/`, with no open handles or emulated state.
    pub fn new(root: PathBuf) -> Self {
        Self { root, handles: HashMap::new(), next: 1, emulated: HashMap::new() }
    }

    /// Normalizes a client path to `/a/b` form; `..` above `/` is refused.
    fn virt(path: &str) -> Result<String, StatusCode> {
        let mut parts: Vec<&str> = vec![];
        // Both separators count, so a Windows-style `..\` cannot slip past.
        for seg in path.split(['/', '\\']) {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop().ok_or(StatusCode::PermissionDenied)?;
                }
                s => parts.push(s),
            }
        }
        Ok(format!("/{}", parts.join("/")))
    }

    /// The local path for a client path: its normalized form joined under the
    /// root, so it can never point outside it.
    fn real(&self, path: &str) -> Result<PathBuf, StatusCode> {
        let v = Self::virt(path)?;
        let mut p = self.root.clone();
        for seg in v.split('/').filter(|s| !s.is_empty()) {
            p.push(seg);
        }
        Ok(p)
    }

    /// Applies the emulated owner/permissions of `path` to `a`.
    fn patch(&self, path: &str, mut a: FileAttributes) -> FileAttributes {
        let (mode, uid, gid) = Self::virt(path).ok().and_then(|v| self.emulated.get(&v).copied()).unwrap_or_default();
        // A mode with file type bits replaces the whole value (so the file
        // reports that type); otherwise only the low 12 permission bits change.
        match mode {
            Some(mode) if mode & 0o170000 != 0 => a.permissions = Some(mode),
            Some(mode) => a.permissions = Some((a.permissions.unwrap_or(0) & !0o7777) | (mode & 0o7777)),
            None => {}
        }
        a.uid = uid.or(a.uid).or(Some(DEFAULT_OWNER));
        a.gid = gid.or(a.gid).or(Some(DEFAULT_OWNER));
        a
    }

    /// True if `path`'s parent has an emulated mode without the write bit.
    fn parent_read_only(&self, path: &str) -> bool {
        let Ok(v) = Self::virt(path) else { return false };
        let parent = v.rsplit_once('/').map(|(p, _)| if p.is_empty() { "/" } else { p }).unwrap_or("/");
        matches!(self.emulated.get(parent), Some((Some(mode), _, _)) if mode & 0o200 == 0)
    }

    /// Stores an open handle under a fresh name and returns that name.
    fn add(&mut self, h: Open) -> String {
        let name = format!("h{}", self.next);
        self.next += 1;
        self.handles.insert(name.clone(), h);
        name
    }

    /// The open file behind `handle`; unknown or directory handles fail.
    fn file(&mut self, handle: &str) -> Result<&mut tokio::fs::File, StatusCode> {
        match self.handles.get_mut(handle) {
            Some(Open::File(f)) => Ok(f),
            _ => Err(StatusCode::Failure),
        }
    }
}

impl russh_sftp::server::Handler for SftpFs {
    type Error = StatusCode;

    /// Requests without an override below are answered "operation unsupported".
    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    /// Accepts any client version and offers no extensions.
    async fn init(&mut self, _version: u32, _ext: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    /// Opens a file with the requested flags, honouring a read-only parent
    /// and recording the permissions of a newly created file.
    async fn open(&mut self, id: u32, filename: String, pflags: OpenFlags, attrs: FileAttributes) -> Result<Handle, Self::Error> {
        let path = self.real(&filename)?;
        let created = pflags.contains(OpenFlags::CREATE) && !path.exists();
        if created && self.parent_read_only(&filename) {
            return Err(StatusCode::PermissionDenied);
        }
        if created && let Some(mode) = attrs.permissions {
            self.emulated.insert(Self::virt(&filename)?, (Some(mode & 0o7777), None, None));
        }
        // Translate SFTP open flags; EXCLUDE means "create, fail if it exists".
        let mut o = tokio::fs::OpenOptions::new();
        o.read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE) || pflags.contains(OpenFlags::APPEND))
            .append(pflags.contains(OpenFlags::APPEND));
        if pflags.contains(OpenFlags::EXCLUDE) {
            o.create_new(true);
        } else {
            o.create(pflags.contains(OpenFlags::CREATE)).truncate(pflags.contains(OpenFlags::TRUNCATE));
        }
        let f = o.open(path).await.map_err(io_err)?;
        Ok(Handle { id, handle: self.add(Open::File(f)) })
    }

    /// Drops the handle; closing an unknown handle still succeeds.
    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(status(id))
    }

    /// Reads up to `len` bytes at `offset`; reading at or past the end is EOF.
    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        let f = self.file(&handle)?;
        f.seek(SeekFrom::Start(offset)).await.map_err(io_err)?;
        let mut buf = vec![0u8; len as usize];
        let n = f.read(&mut buf).await.map_err(io_err)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    /// Writes all of `data` at `offset`.
    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        let f = self.file(&handle)?;
        f.seek(SeekFrom::Start(offset)).await.map_err(io_err)?;
        f.write_all(&data).await.map_err(io_err)?;
        Ok(status(id))
    }

    /// Attributes of `path` itself (a symlink is not followed), with emulated values applied.
    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = tokio::fs::symlink_metadata(self.real(&path)?).await.map_err(io_err)?;
        Ok(Attrs { id, attrs: self.patch(&path, attrs_of(&m)) })
    }

    /// Attributes of `path`'s target (symlinks followed), with emulated values applied.
    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = tokio::fs::metadata(self.real(&path)?).await.map_err(io_err)?;
        Ok(Attrs { id, attrs: self.patch(&path, attrs_of(&m)) })
    }

    /// Attributes of an open file; emulated values are not applied here.
    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let m = self.file(&handle)?.metadata().await.map_err(io_err)?;
        Ok(Attrs { id, attrs: attrs_of(&m) })
    }

    /// Applies a size for real (truncate/extend) and records permissions and
    /// owner in the emulated table (see the module docs).
    async fn setstat(&mut self, id: u32, path: String, attrs: FileAttributes) -> Result<Status, Self::Error> {
        let real = self.real(&path)?;
        tokio::fs::symlink_metadata(&real).await.map_err(io_err)?;
        if let Some(size) = attrs.size {
            let f = tokio::fs::OpenOptions::new().write(true).open(&real).await.map_err(io_err)?;
            f.set_len(size).await.map_err(io_err)?;
        }
        // Opt-in emulation of a server that silently ignores SETSTAT on
        // skiff's temporary upload files.
        if path.ends_with(".skiff-part") && self.root.join(".ignore-temp-setstat").exists() {
            return Ok(status(id));
        }
        let e = self.emulated.entry(Self::virt(&path)?).or_default();
        if let Some(mode) = attrs.permissions {
            e.0 = Some(if mode & 0o170000 != 0 { mode } else { mode & 0o7777 });
        }
        if attrs.uid.is_some() {
            (e.1, e.2) = (attrs.uid, attrs.gid);
        }
        Ok(status(id))
    }

    /// Accepted and ignored.
    async fn fsetstat(&mut self, id: u32, _handle: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        Ok(status(id))
    }

    /// Reads the whole directory up front; entries whose metadata cannot be
    /// read are skipped. Emulated values are not applied to listings.
    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let mut rd = tokio::fs::read_dir(self.real(&path)?).await.map_err(io_err)?;
        let mut files = vec![];
        while let Some(e) = rd.next_entry().await.map_err(io_err)? {
            if let Ok(m) = e.metadata().await {
                files.push(File::new(e.file_name().to_string_lossy().to_string(), attrs_of(&m)));
            }
        }
        Ok(Handle { id, handle: self.add(Open::Dir(files)) })
    }

    /// Returns every remaining entry in one batch, then EOF on the next call.
    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir(files)) if !files.is_empty() => Ok(Name { id, files: std::mem::take(files) }),
            Some(Open::Dir(_)) => Err(StatusCode::Eof),
            _ => Err(StatusCode::Failure),
        }
    }

    /// Deletes a file or symlink and forgets its emulated values.
    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let path = self.real(&filename)?;
        if let Err(e) = tokio::fs::remove_file(&path).await {
            // On Windows a directory symlink is removed with remove_dir, like
            // `rm` removes a link on a Unix server.
            let is_link = tokio::fs::symlink_metadata(&path).await.map(|m| m.file_type().is_symlink()).unwrap_or(false);
            if !is_link {
                return Err(io_err(e));
            }
            tokio::fs::remove_dir(&path).await.map_err(io_err)?;
        }
        self.emulated.remove(&Self::virt(&filename)?);
        Ok(status(id))
    }

    /// Creates one directory; requested attributes are ignored.
    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        tokio::fs::create_dir(self.real(&path)?).await.map_err(io_err)?;
        Ok(status(id))
    }

    /// Removes an empty directory.
    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        tokio::fs::remove_dir(self.real(&path)?).await.map_err(io_err)?;
        Ok(status(id))
    }

    /// Renames without overwriting (fails if the target exists, as SFTP v3
    /// RENAME does) and moves the emulated values with the file.
    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, Self::Error> {
        let (from, to) = (self.real(&oldpath)?, self.real(&newpath)?);
        if tokio::fs::try_exists(&to).await.unwrap_or(false) {
            return Err(StatusCode::Failure);
        }
        tokio::fs::rename(from, to).await.map_err(io_err)?;
        if let Some(e) = self.emulated.remove(&Self::virt(&oldpath)?) {
            self.emulated.insert(Self::virt(&newpath)?, e);
        }
        Ok(status(id))
    }

    /// Canonicalizes lexically (no symlink resolution); `.` becomes `/`.
    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name { id, files: vec![File::dummy(Self::virt(&path)?)] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.`/`..` are resolved lexically and climbing above `/` is an error.
    #[test]
    fn virt_normalizes_and_refuses_escape() {
        assert_eq!(SftpFs::virt(".").unwrap(), "/");
        assert_eq!(SftpFs::virt("/a/./b/../c").unwrap(), "/a/c");
        assert!(SftpFs::virt("/../x").is_err());
        assert!(SftpFs::virt("a/../../x").is_err());
    }
}
