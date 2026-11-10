//! File-system model shared by front-ends: entries and path helpers for local
//! and remote (SFTP, always `/`-separated) locations.

use std::path::Path;
use std::time::SystemTime;

/// One item in a directory listing.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// File name only, not a path.
    pub name: String,
    /// A directory, or a symlink that points to one.
    pub is_dir: bool,
    /// The entry itself is a symlink.
    pub is_symlink: bool,
    /// Size in bytes (0 if unknown).
    pub size: u64,
    /// Last modification time, when known.
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

/// True if `name` is a single, ordinary path component on every platform.
/// Names from a remote listing that fail this (`..`, `a/b`, `a\b`, `C:x`,
/// NUL) are refused so a download can never write outside its folder.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
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

/// A short size with a binary unit, e.g. `512B` or `2.0K`.
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

    /// Remote join and parent handle `/` and the root.
    #[test]
    fn remote_paths() {
        assert_eq!(join(true, "/", "etc"), "/etc");
        assert_eq!(join(true, "/home/u", "x"), "/home/u/x");
        assert_eq!(parent(true, "/home/u"), Some(("/home".into(), "u".into())));
        assert_eq!(parent(true, "/home"), Some(("/".into(), "home".into())));
        assert_eq!(parent(true, "/"), None);
    }

    /// Traversal, separators, drive prefixes and NUL are rejected as child names.
    #[test]
    fn unsafe_child_names_rejected() {
        assert!(is_safe_name("ok.txt"));
        assert!(is_safe_name("my file ü"));
        assert!(is_safe_name(".hidden"));
        for bad in ["", ".", "..", "a/b", "a\\b", "C:x", "..\\..\\x", "nul\0"] {
            assert!(!is_safe_name(bad), "{bad:?}");
        }
    }

    /// Sizes format with one decimal above bytes.
    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(2048), "2.0K");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0M");
    }
}
