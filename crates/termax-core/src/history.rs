//! Connection history (`history.toml` next to `config.toml`): when each host
//! was last connected, how often, and how. Drives "Recent" sorting and the
//! switcher's recent hosts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hosts::HostEntry;
use crate::state::SortMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnKind {
    #[default]
    Terminal,
    Files,
    Forward,
    Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HostHistory {
    /// Unix seconds (UTC).
    pub last: u64,
    pub count: u32,
    pub kind: ConnKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct History {
    pub hosts: BTreeMap<String, HostHistory>,
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl History {
    pub fn path_for(config_path: &Path) -> PathBuf {
        config_path.with_file_name("history.toml")
    }

    /// Missing or unreadable history is empty.
    pub fn load(path: &Path) -> History {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string(self)?)?;
        Ok(())
    }

    pub fn record(&mut self, name: &str, kind: ConnKind, now: u64) {
        let h = self.hosts.entry(name.to_string()).or_default();
        h.last = now;
        h.count = h.count.saturating_add(1);
        h.kind = kind;
    }

    /// Up to `n` host names, most recent first.
    pub fn recent(&self, n: usize) -> Vec<String> {
        let mut v: Vec<(&String, &HostHistory)> = self.hosts.iter().collect();
        v.sort_by(|a, b| b.1.last.cmp(&a.1.last).then_with(|| a.0.cmp(b.0)));
        v.into_iter().take(n).map(|(k, _)| k.clone()).collect()
    }

    pub fn clear(&mut self) {
        self.hosts.clear();
    }

    /// "5 min ago" style text for when `name` was last connected.
    pub fn ago(&self, name: &str, now: u64) -> Option<String> {
        let secs = now.saturating_sub(self.hosts.get(name)?.last);
        Some(match secs {
            0..60 => "just now".into(),
            60..3600 => format!("{} min ago", secs / 60),
            3600..86_400 => format!("{} h ago", secs / 3600),
            _ => format!("{} d ago", secs / 86_400),
        })
    }
}

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

    fn hosts(names: &[&str]) -> Vec<HostEntry> {
        let mut cfg = Config::default();
        cfg.settings.load_ssh_config = false;
        cfg.connections = names.iter().map(|n| Connection { name: n.to_string(), host: "h".into(), ..Default::default() }).collect();
        crate::hosts::collect(&cfg).0
    }

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
}
