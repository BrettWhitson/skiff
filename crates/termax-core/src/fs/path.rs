//! File-system model shared by front-ends: entries and path helpers for local
//! and remote (SFTP, always `/`-separated) locations.

use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Joins `name` onto `base`. Remote paths always use `/`.
pub fn join(remote: bool, base: &str, name: &str) -> String {
    if !remote {
        return Path::new(base).join(name).display().to_string();
    }
    if name.starts_with('/') {
        name.to_string()
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Returns `(parent, current_dir_name)`, or `None` at the root.
pub fn parent(remote: bool, path: &str) -> Option<(String, String)> {
    if !remote {
        let p = Path::new(path);
        let name = p.file_name()?.to_string_lossy().to_string();
        return Some((p.parent()?.display().to_string(), name));
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let (parent, name) = trimmed.rsplit_once('/')?;
    let parent = if parent.is_empty() { "/" } else { parent };
    Some((parent.to_string(), name.to_string()))
}

/// Lists a local directory (without `.`/`..`), unsorted.
pub fn list_local(dir: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        let meta = entry.metadata().ok();
        out.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: ft.is_dir() || (ft.is_symlink() && entry.path().is_dir()),
            is_symlink: ft.is_symlink(),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            modified: meta.and_then(|m| m.modified().ok()),
        });
    }
    Ok(out)
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
        assert_eq!(join(true, "/", "etc"), "/etc");
        assert_eq!(join(true, "/home/u", "x"), "/home/u/x");
        assert_eq!(parent(true, "/home/u"), Some(("/home".into(), "u".into())));
        assert_eq!(parent(true, "/home"), Some(("/".into(), "home".into())));
        assert_eq!(parent(true, "/"), None);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(2048), "2.0K");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0M");
    }
}
