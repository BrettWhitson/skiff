//! SFTP handler serving a local directory as `/`. Paths that would leave the
//! root are refused.

use std::collections::HashMap;
use std::io::{ErrorKind, SeekFrom};
use std::path::PathBuf;

use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

enum Open {
    File(tokio::fs::File),
    Dir(Vec<File>),
}

pub struct SftpFs {
    root: PathBuf,
    handles: HashMap<String, Open>,
    next: u64,
}

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

fn io_err(e: std::io::Error) -> StatusCode {
    match e.kind() {
        ErrorKind::NotFound => StatusCode::NoSuchFile,
        ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

impl SftpFs {
    pub fn new(root: PathBuf) -> Self {
        Self { root, handles: HashMap::new(), next: 1 }
    }

    /// Normalizes a client path to `/a/b` form; `..` above `/` is refused.
    fn virt(path: &str) -> Result<String, StatusCode> {
        let mut parts: Vec<&str> = vec![];
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

    fn real(&self, path: &str) -> Result<PathBuf, StatusCode> {
        let v = Self::virt(path)?;
        let mut p = self.root.clone();
        for seg in v.split('/').filter(|s| !s.is_empty()) {
            p.push(seg);
        }
        Ok(p)
    }

    fn add(&mut self, h: Open) -> String {
        let name = format!("h{}", self.next);
        self.next += 1;
        self.handles.insert(name.clone(), h);
        name
    }

    fn file(&mut self, handle: &str) -> Result<&mut tokio::fs::File, StatusCode> {
        match self.handles.get_mut(handle) {
            Some(Open::File(f)) => Ok(f),
            _ => Err(StatusCode::Failure),
        }
    }
}

impl russh_sftp::server::Handler for SftpFs {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _version: u32, _ext: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(&mut self, id: u32, filename: String, pflags: OpenFlags, _attrs: FileAttributes) -> Result<Handle, Self::Error> {
        let path = self.real(&filename)?;
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

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(status(id))
    }

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

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        let f = self.file(&handle)?;
        f.seek(SeekFrom::Start(offset)).await.map_err(io_err)?;
        f.write_all(&data).await.map_err(io_err)?;
        Ok(status(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = tokio::fs::symlink_metadata(self.real(&path)?).await.map_err(io_err)?;
        Ok(Attrs { id, attrs: attrs_of(&m) })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = tokio::fs::metadata(self.real(&path)?).await.map_err(io_err)?;
        Ok(Attrs { id, attrs: attrs_of(&m) })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let m = self.file(&handle)?.metadata().await.map_err(io_err)?;
        Ok(Attrs { id, attrs: attrs_of(&m) })
    }

    async fn setstat(&mut self, id: u32, _path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        Ok(status(id))
    }

    async fn fsetstat(&mut self, id: u32, _handle: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        Ok(status(id))
    }

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

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir(files)) if !files.is_empty() => Ok(Name { id, files: std::mem::take(files) }),
            Some(Open::Dir(_)) => Err(StatusCode::Eof),
            _ => Err(StatusCode::Failure),
        }
    }

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
        Ok(status(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        tokio::fs::create_dir(self.real(&path)?).await.map_err(io_err)?;
        Ok(status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        tokio::fs::remove_dir(self.real(&path)?).await.map_err(io_err)?;
        Ok(status(id))
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, Self::Error> {
        let (from, to) = (self.real(&oldpath)?, self.real(&newpath)?);
        if tokio::fs::try_exists(&to).await.unwrap_or(false) {
            return Err(StatusCode::Failure);
        }
        tokio::fs::rename(from, to).await.map_err(io_err)?;
        Ok(status(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name { id, files: vec![File::dummy(Self::virt(&path)?)] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virt_normalizes_and_refuses_escape() {
        assert_eq!(SftpFs::virt(".").unwrap(), "/");
        assert_eq!(SftpFs::virt("/a/./b/../c").unwrap(), "/a/c");
        assert!(SftpFs::virt("/../x").is_err());
        assert!(SftpFs::virt("a/../../x").is_err());
    }
}
