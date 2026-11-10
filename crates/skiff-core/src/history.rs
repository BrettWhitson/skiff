//! Connection history (`history.toml` next to `config.toml`): when each host
//! was last connected, how often, and how. Drives "Recent" sorting and the
//! switcher's recent hosts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hosts::HostEntry;
use crate::state::SortMode;

/// How a host was last used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnKind {
    /// An interactive terminal tab.
    #[default]
    Terminal,
    /// The file browser (SFTP).
    Files,
    /// A port forward.
    Forward,
    /// A one-off command or snippet run on the host.
    Command,
}

/// History for one host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HostHistory {
    /// Unix seconds (UTC).
    pub last: u64,
    /// Number of connections made.
    pub count: u32,
    /// How the most recent connection was made.
    pub kind: ConnKind,
}

/// Everything in `history.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct History {
    /// Per-host history keyed by host name.
    pub hosts: BTreeMap<String, HostHistory>,
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl History {
    /// `history.toml` next to the config file.
    pub fn path_for(config_path: &Path) -> PathBuf {
        config_path.with_file_name("history.toml")
    }

    /// Missing or unreadable history is empty.
    pub fn load(path: &Path) -> History {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    /// Writes the history atomically (a temp file, then a rename), so a
    /// reader never sees a half-written file. Prefer [`History::update`],
    /// which keeps other processes' changes.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        crate::migrate::ensure_dir(dir)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // The temp name is hidden and unique per process so concurrent writers
        // don't clobber each other's temp file.
        let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, toml::to_string(self)?)?;
        let mut tries = 0;
        // Rename over the old file; retried briefly on Windows, where replacing a
        // file another process has open can fail transiently.
        loop {
            match std::fs::rename(&tmp, path) {
                Ok(()) => return Ok(()),
                // Windows: another skiff may be replacing it at this moment.
                Err(_) if cfg!(windows) && tries < 10 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e.into());
                }
            }
        }
    }

    /// Notes a connection to `name` at time `now` (Unix seconds).
    pub fn record(&mut self, name: &str, kind: ConnKind, now: u64) {
        let h = self.hosts.entry(name.to_string()).or_default();
        h.last = now;
        h.count = h.count.saturating_add(1);
        h.kind = kind;
    }

    /// Moves host `old`'s history to `new` (after a rename), merging with
    /// any history `new` already has.
    pub fn rename(&mut self, old: &str, new: &str) {
        // When both have history, add the counts and keep whichever was used last.
        let Some(o) = self.hosts.remove(old) else { return };
        match self.hosts.get_mut(new) {
            Some(n) => {
                n.count = n.count.saturating_add(o.count);
                if o.last > n.last {
                    n.last = o.last;
                    n.kind = o.kind;
                }
            }
            None => {
                self.hosts.insert(new.to_string(), o);
            }
        }
    }

    /// Applies `change` to the history on disk and to `self`: re-reads the
    /// file first, so changes another skiff made meanwhile are kept, then
    /// replaces it atomically. `self` ends up with the merged history even
    /// if the write fails.
    pub fn update(&mut self, path: &Path, change: impl FnOnce(&mut History)) -> anyhow::Result<()> {
        // A file that can't be read or parsed right now (or is gone) must not
        // wipe what this process knows.
        let mut fresh = std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_else(|| self.clone());
        change(&mut fresh);
        let res = fresh.save(path);
        *self = fresh;
        res
    }

    /// Up to `n` host names, most recent first.
    pub fn recent(&self, n: usize) -> Vec<String> {
        let mut v: Vec<(&String, &HostHistory)> = self.hosts.iter().collect();
        v.sort_by(|a, b| b.1.last.cmp(&a.1.last).then_with(|| a.0.cmp(b.0)));
        v.into_iter().take(n).map(|(k, _)| k.clone()).collect()
    }

    /// Forgets all hosts.
    pub fn clear(&mut self) {
        self.hosts.clear();
    }

    /// "5 min ago" style text for when `name` was last connected.
    pub fn ago(&self, name: &str, now: u64) -> Option<String> {
        let secs = now.saturating_sub(self.hosts.get(name)?.last);
        // Ranges are half-open: under a minute is "just now", and so on.
        Some(match secs {
            0..60 => "just now".into(),
            60..3600 => format!("{} min ago", secs / 60),
            3600..86_400 => format!("{} h ago", secs / 3600),
            _ => format!("{} d ago", secs / 86_400),
        })
    }
}

/// Sorts host indices `idx` for display. Name order ignores case; Recent puts
/// connected hosts newest first, then the rest by name.
/// Sorts host indices `idx` for display.
pub fn order(hosts: &[HostEntry], idx: &mut [usize], mode: SortMode, h: &History) {
    let name = |i: usize| hosts[i].name.to_lowercase();
    match mode {
        SortMode::Name => idx.sort_by_cached_key(|&i| name(i)),
        SortMode::Recent => idx.sort_by_cached_key(|&i| {
            let last = h.hosts.get(&hosts[i].name).map(|x| x.last);
            (last.is_none(), std::cmp::Reverse(last.unwrap_or(0)), name(i))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Connection};

    /// Builds a host list containing only config connections with these names.
    fn hosts(names: &[&str]) -> Vec<HostEntry> {
        let mut cfg = Config::default();
        cfg.settings.load_ssh_config = false;
        cfg.connections = names.iter().map(|n| Connection { name: n.to_string(), host: "h".into(), ..Default::default() }).collect();
        crate::hosts::collect(&cfg).0
    }

    /// Recording updates count, time and kind; `recent` is newest first and
    /// `clear` empties it.
    #[test]
    fn record_and_recent() {
        let mut h = History::default();
        h.record("a", ConnKind::Terminal, 10);
        h.record("b", ConnKind::Files, 20);
        h.record("a", ConnKind::Command, 30);
        assert_eq!(h.recent(5), ["a", "b"]);
        assert_eq!(h.recent(1), ["a"]);
        assert_eq!(h.hosts["a"].count, 2);
        assert_eq!(h.hosts["a"].kind, ConnKind::Command);
        h.clear();
        assert!(h.recent(5).is_empty());
    }

    /// Recent order puts connected hosts first (newest first), then the rest by
    /// name; Name order is case-insensitive.
    #[test]
    fn order_recent_then_name() {
        let list = hosts(&["delta", "Alpha", "charlie", "bravo"]);
        let mut h = History::default();
        h.record("charlie", ConnKind::Terminal, 5);
        h.record("delta", ConnKind::Terminal, 9);
        let mut idx: Vec<usize> = (0..4).collect();
        order(&list, &mut idx, SortMode::Recent, &h);
        let names: Vec<&str> = idx.iter().map(|&i| list[i].name.as_str()).collect();
        assert_eq!(names, ["delta", "charlie", "Alpha", "bravo"]);
        order(&list, &mut idx, SortMode::Name, &h);
        let names: Vec<&str> = idx.iter().map(|&i| list[i].name.as_str()).collect();
        assert_eq!(names, ["Alpha", "bravo", "charlie", "delta"]);
    }

    /// `ago` picks minutes, hours or days and returns `None` for unknown hosts.
    #[test]
    fn ago_strings() {
        let mut h = History::default();
        h.record("a", ConnKind::Terminal, 1_000_000);
        let ago = |d: u64| h.ago("a", 1_000_000 + d).unwrap();
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(300), "5 min ago");
        assert_eq!(ago(3 * 3600 + 5), "3 h ago");
        assert_eq!(ago(2 * 86_400), "2 d ago");
        assert_eq!(h.ago("zz", 0), None);
    }

    /// A missing file loads empty, and a saved history loads back unchanged.
    #[test]
    fn load_missing_is_empty_and_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let p = History::path_for(&d.path().join("config.toml"));
        assert_eq!(History::load(&p), History::default());
        let mut h = History::default();
        h.record("web", ConnKind::Forward, 42);
        h.save(&p).unwrap();
        assert_eq!(History::load(&p), h);
    }

    /// Two writers interleaving `update` keep each other's entries, and a rename
    /// merges counts and keeps the latest time.
    #[test]
    fn update_keeps_other_processes_changes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("history.toml");
        let (mut one, mut two) = (History::default(), History::default());
        one.update(&p, |h| h.record("a", ConnKind::Terminal, 10)).unwrap();
        two.update(&p, |h| h.record("b", ConnKind::Files, 20)).unwrap();
        one.update(&p, |h| h.record("a", ConnKind::Terminal, 30)).unwrap();
        assert_eq!(one.recent(5), ["a", "b"]);
        assert_eq!(History::load(&p), one);
        one.update(&p, |h| h.rename("a", "b")).unwrap();
        assert_eq!((one.hosts["b"].count, one.hosts["b"].last), (3, 30));
        assert!(!one.hosts.contains_key("a"));
    }
}
