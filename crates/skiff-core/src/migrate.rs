//! Compatibility with the project's former name, termax: the old config
//! variable and a one-time copy of the old data directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::Config;

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

/// What the one-time copy of the old data directory did at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Migration {
    /// Nothing to copy: an explicit config path, no old directory, already
    /// done, or another skiff instance copied it at the same moment.
    None,
    /// Copied; the message to show the user once.
    Copied(String),
    /// The copy failed and left nothing at the new place; the message to
    /// show. The next start tries again.
    Failed(String),
}

impl Migration {
    /// The message to show the user, if there is one.
    pub fn note(&self) -> Option<&str> {
        match self {
            Migration::None => None,
            Migration::Copied(m) | Migration::Failed(m) => Some(m),
        }
    }
}

/// How the copy of the old directory ended (see [`copy_legacy`]).
#[derive(Debug, PartialEq, Eq)]
pub enum CopyOutcome {
    /// `new` already existed or `old` didn't: nothing copied.
    NotNeeded,
    /// Copied; `skipped_links` are directory links that could not be
    /// recreated and were left out instead of being followed.
    Copied { skipped_links: Vec<PathBuf> },
    /// Another instance (the TUI and the GUI starting together) put `new`
    /// in place first; our copy was discarded.
    ByOther,
}

/// New data directories that must not be created this run: their copy from
/// the old name failed, and creating them would stop the next start retrying.
static HELD: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Keeps `dir` from being created for the rest of this run (see [`ensure_dir`]).
pub fn hold_dir(dir: &Path) {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).push(dir.to_path_buf());
}

/// Whether `dir` is, or is inside, a held directory that doesn't exist yet.
pub fn is_held(dir: &Path) -> bool {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|h| dir.starts_with(h) && !h.exists())
}

/// Creates `dir` and its parents like `create_dir_all`, unless it lies in a
/// held directory: then nothing is created and an error explains why.
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if is_held(dir) {
        return Err(std::io::Error::other(HELD_REASON));
    }
    std::fs::create_dir_all(dir)
}

/// Why a write into a held directory was refused.
pub const HELD_REASON: &str =
    "not saved: your settings from termax couldn't be copied yet, so nothing is written to the new folder until a restart copies them";

/// Staging directories older than this are left over from a crashed run.
const STALE_AFTER: Duration = Duration::from_secs(300);

/// How often the final rename is tried: on Windows an antivirus scanner can
/// briefly lock the freshly copied files.
const RENAME_ATTEMPTS: u32 = if cfg!(windows) { 5 } else { 1 };

/// Run once at startup, before the config is loaded: when the config is in
/// its default place, that directory doesn't exist yet and the old one does,
/// copies the old one over. Its note is shown once (the new directory
/// exists afterwards, so it is never repeated).
pub fn migrate_legacy(config_path: &Path) -> Migration {
    // SKIFF_CONFIG or TERMAX_CONFIG set (and non-empty) means the user picked the
    // location, so there is no default-place directory to fill.
    if [crate::config::CONFIG_ENV, LEGACY_CONFIG_ENV].iter().any(|v| std::env::var_os(v).is_some_and(|p| !p.is_empty())) {
        return Migration::None; // an explicit config path: nothing to move
    }
    let (Some(new), Some(old)) = (config_path.parent().filter(|p| !p.as_os_str().is_empty()), legacy_config_dir()) else {
        return Migration::None;
    };
    match copy_legacy(&old, new) {
        Ok(CopyOutcome::Copied { skipped_links }) => {
            let mut note = format!("copied your settings from {} to {}; the old folder was left as it was", old.display(), new.display());
            if !skipped_links.is_empty() {
                let list: Vec<String> = skipped_links.iter().map(|p| p.display().to_string()).collect();
                note.push_str(&format!(" (folder links not copied: {})", list.join(", ")));
            }
            Migration::Copied(note)
        }
        Ok(CopyOutcome::NotNeeded | CopyOutcome::ByOther) => Migration::None,
        Err(e) => Migration::Failed(format!(
            "could not copy your settings from {}: {e}; skiff will try again next start. Until then this session saves nothing \
             (history, window state, settings and ssh config edits are kept in memory or refused)",
            old.display()
        )),
    }
}

/// Runs [`migrate_legacy`] and loads the config. After a failed copy the
/// defaults are used and no fresh config is written, so the next start retries.
pub fn load_config(config_path: &Path) -> anyhow::Result<(Config, Migration)> {
    let migration = migrate_legacy(config_path);
    Ok((load_after(config_path, &migration)?, migration))
}

/// Loads the config after `migration`. After a failed copy it writes no
/// template and holds the config directory (see [`hold_dir`]) so that no
/// history, state, backup or settings write creates it and stops the retry.
fn load_after(config_path: &Path, migration: &Migration) -> anyhow::Result<Config> {
    match migration {
        Migration::Failed(_) => {
            if let Some(dir) = config_path.parent().filter(|d| !d.as_os_str().is_empty()) {
                hold_dir(dir);
            }
            Config::load_or_default(config_path)
        }
        _ => Config::load_or_create(config_path),
    }
}

/// Copies the directory `old` to `new` when `new` doesn't exist and `old`
/// does; `old` is never changed. The copy is made beside `new` and renamed
/// into place, so a failed or interrupted copy leaves no half-filled `new`,
/// and nothing already at `new` is overwritten.
pub fn copy_legacy(old: &Path, new: &Path) -> std::io::Result<CopyOutcome> {
    copy_legacy_with(old, new, &mut |from, to| std::fs::rename(from, to))
}

/// [`copy_legacy`] with the final rename supplied (tests make it fail).
fn copy_legacy_with(old: &Path, new: &Path, rename: &mut dyn FnMut(&Path, &Path) -> std::io::Result<()>) -> std::io::Result<CopyOutcome> {
    if new.symlink_metadata().is_ok() || !old.is_dir() {
        return Ok(CopyOutcome::NotNeeded);
    }
    // Copy into a hidden per-process staging directory beside `new`, then rename
    // it into place in one step.
    let parent = new.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = new.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    clean_stale(parent, &name, STALE_AFTER);
    let staging = parent.join(format!(".{name}.migrating-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let mut skipped_links = Vec::new();
    let done = copy_tree(old, &staging, &mut skipped_links).and_then(|()| {
        if new.symlink_metadata().is_ok() {
            return Ok(CopyOutcome::ByOther); // another instance got there first
        }
        match rename_retrying(&staging, new, RENAME_ATTEMPTS, rename) {
            Ok(()) => Ok(CopyOutcome::Copied { skipped_links }),
            // The rename lost a race with another instance: its copy is in place.
            Err(_) if new.symlink_metadata().is_ok() => Ok(CopyOutcome::ByOther),
            Err(e) => Err(e),
        }
    });
    // Clean up the staging copy on failure or when another instance won the race.
    if !matches!(done, Ok(CopyOutcome::Copied { .. })) {
        let _ = std::fs::remove_dir_all(&staging);
    }
    done
}

/// Renames `from` to `to`, trying up to `attempts` times with a short,
/// doubling pause; stops early once `to` exists (another instance won).
fn rename_retrying(
    from: &Path,
    to: &Path,
    attempts: u32,
    rename: &mut dyn FnMut(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt + 1 >= attempts || to.symlink_metadata().is_ok() => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(50 << attempt)),
        }
        attempt += 1;
    }
}

/// Removes `.<name>.migrating-*` staging directories in `parent` that are
/// at least `max_age` old (left by a crashed run); fresh ones may belong
/// to another instance copying right now and are kept.
fn clean_stale(parent: &Path, name: &str, max_age: Duration) {
    let prefix = format!(".{name}.migrating-");
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        let stale = entry.file_name().to_string_lossy().starts_with(&prefix)
            && entry.file_type().is_ok_and(|t| t.is_dir())
            && entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().unwrap_or_default() >= max_age);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Copies a directory tree into the new directory `to`, keeping permissions
/// (private directories stay private while they are filled) and, on Unix,
/// symlinks as symlinks. Elsewhere a link to a directory (symlink or
/// junction) is recreated as a link if possible, else left out and added to
/// `skipped_links`; it is never followed, so a link loop can't recurse.
// On Unix `skipped_links` is only passed down (links are always recreated).
#[cfg_attr(unix, allow(clippy::only_used_in_recursion))]
fn copy_tree(from: &Path, to: &Path, skipped_links: &mut Vec<PathBuf>) -> std::io::Result<()> {
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
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
            continue;
        }
        #[cfg(not(unix))]
        if kind.is_symlink() && src.is_dir() {
            if !copy_dir_link(&src, &dst) {
                skipped_links.push(src);
            }
            continue;
        }
        // A file symlink (non-Unix here) is followed: copy what it points to.
        if kind.is_dir() {
            copy_tree(&src, &dst, skipped_links)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    std::fs::set_permissions(to, meta.permissions())
}

/// Recreates the directory link `src` at `dst` pointing where it points
/// (Windows needs Developer Mode or admin for that); false when it can't.
#[cfg(not(unix))]
fn copy_dir_link(src: &Path, dst: &Path) -> bool {
    #[cfg(windows)]
    return std::fs::read_link(src).and_then(|target| std::os::windows::fs::symlink_dir(target, dst)).is_ok();
    #[cfg(not(windows))]
    {
        let _ = (src, dst);
        false
    }
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
        assert!(matches!(copy_legacy(&old, &new).unwrap(), CopyOutcome::Copied { .. }));
        assert_eq!(std::fs::read_to_string(new.join("config.toml")).unwrap(), "theme = \"x\"\n");
        assert_eq!(std::fs::read_to_string(new.join("backups").join("config-1-20260101-000000.bak")).unwrap(), "b");
        assert!(old.join("config.toml").exists() && old.join("backups").join("config-1-20260101-000000.bak").exists());
        // No staging directory left behind.
        assert_eq!(std::fs::read_dir(new.parent().unwrap()).unwrap().count(), 1);
        // Second run: the new directory exists, nothing is copied again.
        std::fs::write(old.join("config.toml"), "changed").unwrap();
        assert_eq!(CopyOutcome::NotNeeded, copy_legacy(&old, &new).unwrap());
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
        assert_eq!(CopyOutcome::NotNeeded, copy_legacy(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("config.toml")).unwrap(), "mine");
        assert!(!new.join("history.toml").exists());
        assert_eq!(CopyOutcome::NotNeeded, copy_legacy(&d.path().join("missing"), &d.path().join("other")).unwrap());
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
        assert!(matches!(copy_legacy(&old, &new).unwrap(), CopyOutcome::Copied { .. }));
        assert_eq!(std::fs::metadata(new.join("backups")).unwrap().permissions().mode() & 0o777, 0o700);
    }

    /// A failed copy loads the defaults without writing a config, and the
    /// session's history, state, settings and backup writes don't create the
    /// new directory either, so the next start retries.
    #[test]
    fn failed_migration_writes_no_fresh_config() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("skiff").join("config.toml");
        let cfg = load_after(&path, &Migration::Failed("x".into())).unwrap();
        let mut hist = crate::history::History::default();
        hist.record("web", crate::history::ConnKind::Terminal, 1);
        assert!(hist.save(&crate::history::History::path_for(&path)).is_err());
        assert!(crate::state::UiState::default().save(&crate::state::UiState::path_for(&path)).is_err());
        let mut changed = cfg.clone();
        changed.settings.scrollback += 1;
        assert!(changed.save(&cfg, &path).unwrap_err().to_string().contains("not saved"));
        assert!(ensure_dir(&crate::sshconfig::edit::backups_dir(&path)).is_err());
        Config::load_or_create(&path).unwrap(); // a reload
        assert!(!path.parent().unwrap().exists());
        // Other directories are unaffected.
        ensure_dir(&d.path().join("elsewhere")).unwrap();
        let path = d.path().join("ok").join("config.toml");
        load_after(&path, &Migration::None).unwrap();
        assert!(path.exists());
    }

    /// A rename that fails while copying is retried, then succeeds.
    #[test]
    fn retries_a_failed_rename() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("a"), d.path().join("b"));
        std::fs::create_dir(&from).unwrap();
        let mut calls = 0;
        let mut rename = |f: &Path, t: &Path| {
            calls += 1;
            if calls < 3 { Err(std::io::Error::other("locked")) } else { std::fs::rename(f, t) }
        };
        rename_retrying(&from, &to, 3, &mut rename).unwrap();
        assert!(to.is_dir());
    }

    /// When the rename fails because another instance just put the new
    /// directory in place, that counts as done: no error, our staging copy
    /// is removed and theirs is kept.
    #[test]
    fn losing_the_rename_race_is_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        let old = old_dir(d.path());
        let new = d.path().join("apps").join("skiff");
        let mut rename = |_: &Path, t: &Path| {
            std::fs::create_dir(t)?;
            std::fs::write(t.join("config.toml"), "theirs")?;
            Err(std::io::Error::other("target exists"))
        };
        assert_eq!(copy_legacy_with(&old, &new, &mut rename).unwrap(), CopyOutcome::ByOther);
        assert_eq!(std::fs::read_to_string(new.join("config.toml")).unwrap(), "theirs");
        assert_eq!(std::fs::read_dir(new.parent().unwrap()).unwrap().count(), 1);
    }

    /// Old staging directories are removed; other entries are kept.
    #[test]
    fn cleans_stale_staging_dirs() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".skiff.migrating-999").join("backups")).unwrap();
        std::fs::create_dir(d.path().join("other")).unwrap();
        clean_stale(d.path(), "skiff", STALE_AFTER);
        assert!(d.path().join(".skiff.migrating-999").exists(), "a fresh one may be another instance's");
        clean_stale(d.path(), "skiff", Duration::ZERO);
        assert!(!d.path().join(".skiff.migrating-999").exists());
        assert!(d.path().join("other").exists());
    }

    /// A junction looping back to the old directory is not followed: the copy
    /// finishes, with the link recreated or listed as skipped.
    #[cfg(windows)]
    #[test]
    fn does_not_follow_junctions() {
        let d = tempfile::tempdir().unwrap();
        let old = old_dir(d.path());
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(old.join("loop"))
            .arg(&old)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "mklink /J failed");
        let new = d.path().join("skiff");
        let CopyOutcome::Copied { skipped_links } = copy_legacy(&old, &new).unwrap() else { panic!("not copied") };
        assert!(skipped_links == vec![old.join("loop")] || new.join("loop").symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(new.join("config.toml")).unwrap(), "theme = \"x\"\n");
    }
}
