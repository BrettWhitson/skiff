//! Compatibility with the project's former name, termax: the old config
//! variable and a one-time copy of the old data directory.

use std::path::{Path, PathBuf};

/// The config path variable from before the rename, honoured after
/// `SKIFF_CONFIG` for one release.
pub const LEGACY_CONFIG_ENV: &str = "TERMAX_CONFIG";

/// The data directory name from before the rename.
const LEGACY_NAME: &str = "termax";

/// Where the old name kept `config.toml`, `history.toml`, `state.toml` and
/// `backups/` (`%APPDATA%\termax\config`, `~/.config/termax`, ...).
pub fn legacy_config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", LEGACY_NAME).map(|d| d.config_dir().to_path_buf())
}

/// Run once at startup, before the config is loaded: when the config is in
/// its default place, that directory doesn't exist yet and the old one does,
/// copies the old one over. Returns a message to show the user once (the new
/// directory exists afterwards, so it is never repeated).
pub fn migrate_legacy(config_path: &Path) -> Option<String> {
    // SKIFF_CONFIG or TERMAX_CONFIG set (and non-empty) means the user picked the
    // location, so there is no default-place directory to fill.
    if [crate::config::CONFIG_ENV, LEGACY_CONFIG_ENV].iter().any(|v| std::env::var_os(v).is_some_and(|p| !p.is_empty())) {
        return None; // an explicit config path: nothing to move
    }
    let new = config_path.parent().filter(|p| !p.as_os_str().is_empty())?;
    let old = legacy_config_dir()?;
    match copy_legacy(&old, new) {
        Ok(true) => Some(format!("copied your settings from {} to {}; the old folder was left as it was", old.display(), new.display())),
        Ok(false) => None,
        Err(e) => Some(format!("could not copy your settings from {}: {e}", old.display())),
    }
}

/// Copies the directory `old` to `new` when `new` doesn't exist and `old`
/// does; `old` is never changed. The copy is made beside `new` and renamed
/// into place, so a failed or interrupted copy leaves no half-filled `new`,
/// and nothing already at `new` is overwritten. `Ok(true)` when it copied.
pub fn copy_legacy(old: &Path, new: &Path) -> std::io::Result<bool> {
    if new.symlink_metadata().is_ok() || !old.is_dir() {
        return Ok(false);
    }
    // Copy into a hidden per-process staging directory beside `new`, then rename
    // it into place in one step.
    let parent = new.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = new.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let staging = parent.join(format!(".{name}.migrating-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let done = copy_tree(old, &staging).and_then(|()| {
        if new.symlink_metadata().is_ok() {
            return Ok(false); // another instance got there first
        }
        std::fs::rename(&staging, new).map(|()| true)
    });
    // Clean up the staging copy on failure or when another instance won the race.
    if done.as_ref().map_or(true, |copied| !copied) {
        let _ = std::fs::remove_dir_all(&staging);
    }
    done
}

/// Copies a directory tree into the new directory `to`, keeping permissions
/// (private directories stay private while they are filled) and, on Unix,
/// symlinks as symlinks.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    // Create the target private first; its real permissions are applied after
    // the contents are copied.
    let meta = std::fs::metadata(from)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(to)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        #[cfg(unix)]
        {
            if kind.is_symlink() {
                std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
                continue;
            }
        }
        // A symlink (non-Unix here) is followed: copy what it points to.
        let is_dir = if kind.is_symlink() { src.is_dir() } else { kind.is_dir() };
        if is_dir {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    std::fs::set_permissions(to, meta.permissions())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a fake `termax` data directory with a config, history and a backup.
    fn old_dir(root: &Path) -> PathBuf {
        let old = root.join("termax");
        std::fs::create_dir_all(old.join("backups")).unwrap();
        std::fs::write(old.join("config.toml"), "theme = \"x\"\n").unwrap();
        std::fs::write(old.join("history.toml"), "h").unwrap();
        std::fs::write(old.join("backups").join("config-1-20260101-000000.bak"), "b").unwrap();
        old
    }

    /// Copies everything once, leaves the old directory intact, leaves no staging
    /// directory behind, and does nothing on a second run.
    #[test]
    fn copies_old_data_once_and_leaves_it_intact() {
        let d = tempfile::tempdir().unwrap();
        let old = old_dir(d.path());
        let new = d.path().join("apps").join("skiff");
        assert!(copy_legacy(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("config.toml")).unwrap(), "theme = \"x\"\n");
        assert_eq!(std::fs::read_to_string(new.join("backups").join("config-1-20260101-000000.bak")).unwrap(), "b");
        assert!(old.join("config.toml").exists() && old.join("backups").join("config-1-20260101-000000.bak").exists());
        // No staging directory left behind.
        assert_eq!(std::fs::read_dir(new.parent().unwrap()).unwrap().count(), 1);
        // Second run: the new directory exists, nothing is copied again.
        std::fs::write(old.join("config.toml"), "changed").unwrap();
        assert!(!copy_legacy(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("config.toml")).unwrap(), "theme = \"x\"\n");
    }

    /// An existing new directory is never overwritten, and a missing old
    /// directory copies nothing.
    #[test]
    fn never_touches_an_existing_new_dir_or_a_missing_old_one() {
        let d = tempfile::tempdir().unwrap();
        let old = old_dir(d.path());
        let new = d.path().join("skiff");
        std::fs::create_dir(&new).unwrap();
        std::fs::write(new.join("config.toml"), "mine").unwrap();
        assert!(!copy_legacy(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("config.toml")).unwrap(), "mine");
        assert!(!new.join("history.toml").exists());
        assert!(!copy_legacy(&d.path().join("missing"), &d.path().join("other")).unwrap());
        assert!(!d.path().join("other").exists());
    }

    /// A 0700 directory is still 0700 after the copy.
    #[cfg(unix)]
    #[test]
    fn keeps_private_dirs_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let old = old_dir(d.path());
        std::fs::set_permissions(old.join("backups"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let new = d.path().join("skiff");
        assert!(copy_legacy(&old, &new).unwrap());
        assert_eq!(std::fs::metadata(new.join("backups")).unwrap().permissions().mode() & 0o777, 0o700);
    }
}
