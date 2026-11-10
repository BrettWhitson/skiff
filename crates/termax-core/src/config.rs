use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike};

use crate::theme::ThemeDef;

/// A connection defined in termax's own config (as opposed to `~/.ssh/config`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Connection {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    /// Extra arguments passed to `ssh` before the destination, e.g. `["-A", "-o", "Compression=yes"]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
    /// Directory the SFTP browser opens in on the remote side.
    pub remote_dir: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Field labels of the connection editor (TUI and GUI), in order.
pub const CONNECTION_LABELS: [&str; 9] =
    ["Name", "Host", "User", "Port", "Identity file", "ProxyJump", "Extra ssh args", "Remote dir", "Tags"];

impl Connection {
    /// The editor's text fields, in `CONNECTION_LABELS` order.
    pub fn to_fields(&self) -> [String; 9] {
        let o = |v: &Option<String>| v.clone().unwrap_or_default();
        [
            self.name.clone(),
            self.host.clone(),
            o(&self.user),
            self.port.map(|p| p.to_string()).unwrap_or_default(),
            o(&self.identity_file),
            o(&self.proxy_jump),
            shell_words::join(&self.extra_args),
            o(&self.remote_dir),
            self.tags.join(", "),
        ]
    }

    /// Parses and validates the editor's fields (name and host required,
    /// port 1-65535 when given).
    pub fn from_fields(fields: &[String; 9]) -> Result<Connection, String> {
        let v = |i: usize| fields[i].trim().to_string();
        let opt = |i: usize| Some(v(i)).filter(|s| !s.is_empty());
        if v(0).is_empty() {
            return Err("Name is required".into());
        }
        if v(1).is_empty() {
            return Err("Host is required".into());
        }
        let port = match opt(3) {
            None => None,
            Some(p) => Some(p.parse::<u16>().ok().filter(|&n| n > 0).ok_or_else(|| format!("Invalid port: {p}"))?),
        };
        let extra_args = shell_words::split(&v(6)).map_err(|e| format!("Extra ssh args: {e}"))?;
        Ok(Connection {
            name: v(0),
            host: v(1),
            user: opt(2),
            port,
            identity_file: opt(4),
            proxy_jump: opt(5),
            extra_args,
            remote_dir: opt(7),
            tags: v(8).split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Name of the active theme (built-in or one defined under `[[theme]]`).
    pub theme: String,
    /// The OpenSSH client to run.
    pub ssh_binary: String,
    /// List hosts from the OpenSSH config file.
    pub load_ssh_config: bool,
    /// Override the OpenSSH config path (default `~/.ssh/config`).
    pub ssh_config_path: Option<String>,
    /// Lines of scrollback kept per terminal tab.
    pub scrollback: usize,
    /// Pass `-o BatchMode=yes` to SFTP connections so they fail instead of
    /// prompting (the SFTP channel has no terminal to type a password into).
    pub sftp_batch_mode: bool,
    pub show_hidden_files: bool,
    /// Directory the local pane of the SFTP browser starts in (default: home).
    pub local_start_dir: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "terminal".into(),
            ssh_binary: "ssh".into(),
            load_ssh_config: true,
            ssh_config_path: None,
            scrollback: 5000,
            sftp_batch_mode: true,
            show_hidden_files: false,
            local_start_dir: None,
        }
    }
}

/// GUI-only settings (`[gui]`). The TUI ignores them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GuiSettings {
    pub font_family: Option<String>,
    /// Terminal (monospace) font file to use instead of the bundled fonts.
    pub font_path: Option<String>,
    /// Interface font file to use instead of the bundled fonts.
    pub ui_font_path: Option<String>,
    pub font_size: f32,
    pub ui_scale: f32,
}

impl Default for GuiSettings {
    fn default() -> Self {
        Self { font_family: None, font_path: None, ui_font_path: None, font_size: 14.0, ui_scale: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Config {
    pub settings: Settings,
    pub gui: GuiSettings,
    /// Action name → key chord (e.g. `quick_switcher = "Ctrl+Shift+K"`). Parsed by the GUI.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
    #[serde(rename = "connection", skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<Connection>,
    #[serde(rename = "theme", skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<ThemeDef>,
}

const TEMPLATE: &str = r##"# termax configuration
# termax only rewrites the settings you change in the app; comments and
# formatting here are kept.

[settings]
theme = "terminal"
ssh_binary = "ssh"
load_ssh_config = true
# ssh_config_path = "~/.ssh/config"
scrollback = 5000
sftp_batch_mode = true
show_hidden_files = false
# local_start_dir = "~/Downloads"

# Custom connections (also editable in the app with `n` / `e`):
#
# [[connection]]
# name = "homelab"
# host = "192.168.1.10"
# user = "admin"
# port = 22
# identity_file = "~/.ssh/id_ed25519"
# proxy_jump = "bastion"
# extra_args = ["-A"]
# remote_dir = "/srv"
# tags = ["home"]

# Custom themes (colors: "#rrggbb", names like "lightblue", 0-255, or "reset"):
#
# [[theme]]
# name = "my-theme"
# bg = "#101010"
# fg = "#e0e0e0"
# accent = "#ff9e64"
# muted = "#707070"
# border = "#303030"
# selection_bg = "#303030"
# selection_fg = "#ffffff"
# success = "#9ece6a"
# warning = "#e0af68"
# error = "#f7768e"
"##;

impl Config {
    /// `$TERMAX_CONFIG`, or the platform config dir (`%APPDATA%\termax\config\config.toml`,
    /// `~/.config/termax/config.toml`, ...).
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("TERMAX_CONFIG") {
            return PathBuf::from(p);
        }
        directories::ProjectDirs::from("", "", "termax")
            .map(|d| d.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("termax.toml"))
    }

    /// Loads the config, writing the commented template on first run.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
            std::fs::write(path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// Writes the changes between `baseline` (what this front-end loaded or
    /// last saved) and `self` into the file on disk. Everything else in the
    /// file — comments, unknown keys, and changes made meanwhile by another
    /// front-end — is kept.
    pub fn save(&self, baseline: &Config, path: &Path) -> Result<()> {
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: DocumentMut = existing.parse().context("parsing existing config")?;
        let old = toml::Value::try_from(baseline)?;
        let new = toml::Value::try_from(self)?;
        let new_doc: DocumentMut = toml::to_string(self)?.parse()?;
        let section = |v: &toml::Value, k: &str| v.get(k).and_then(|t| t.as_table()).cloned().unwrap_or_default();

        for name in ["settings", "gui", "keys"] {
            let (old_t, new_t) = (section(&old, name), section(&new, name));
            if old_t == new_t {
                continue;
            }
            if !doc.contains_table(name) {
                doc.insert(name, Item::Table(Table::new()));
            }
            let table = doc[name].as_table_like_mut().context("config section is not a table")?;
            let new_table = new_doc.get(name).and_then(|i| i.as_table_like());
            apply_key_changes(table, &old_t, &new_t, new_table);
        }

        for name in ["connection", "theme"] {
            let list = |v: &toml::Value| -> Vec<toml::Table> {
                v.get(name)
                    .and_then(|a| a.as_array())
                    .map(|a| a.iter().filter_map(|t| t.as_table().cloned()).collect())
                    .unwrap_or_default()
            };
            let (old_l, new_l) = (list(&old), list(&new));
            if old_l == new_l {
                continue;
            }
            let key = |t: &toml::Table| t.get("name").and_then(|n| n.as_str()).map(str::to_string);
            let new_items: Vec<Table> = new_doc
                .get(name)
                .and_then(|i| i.as_array_of_tables())
                .map(|a| a.iter().cloned().collect())
                .unwrap_or_default();
            if !doc.contains_array_of_tables(name) {
                doc.insert(name, Item::ArrayOfTables(ArrayOfTables::new()));
            }
            let aot = doc[name].as_array_of_tables_mut().context("config array is not an array of tables")?;
            let find = |aot: &ArrayOfTables, n: &str| aot.iter().position(|t| t.get("name").and_then(|v| v.as_str()) == Some(n));

            // Removed here → remove from the file.
            for o in &old_l {
                let Some(n) = key(o) else { continue };
                if !new_l.iter().any(|t| key(t).as_deref() == Some(n.as_str())) {
                    if let Some(i) = find(aot, &n) {
                        aot.remove(i);
                    }
                }
            }
            // Added or changed here → update just those keys (or append).
            for (i, t) in new_l.iter().enumerate() {
                let Some(n) = key(t) else { continue };
                let before = old_l.iter().find(|o| key(o).as_deref() == Some(n.as_str()));
                if before == Some(t) {
                    continue;
                }
                match find(aot, &n) {
                    Some(pos) => {
                        let target = aot.get_mut(pos).expect("index from find");
                        let empty = toml::Table::new();
                        apply_key_changes(target, before.unwrap_or(&empty), t, new_items.get(i).map(|t| t as &dyn TableLike));
                    }
                    None => aot.push(new_items[i].clone()),
                }
            }
            if aot.is_empty() {
                doc.remove(name);
            }
        }

        let text = doc.to_string();
        if text == existing {
            return Ok(());
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Sets or removes, in `target`, each key whose value differs between `old`
/// and `new`. Values come from `new_items` (the serialized new config).
fn apply_key_changes(target: &mut dyn TableLike, old: &toml::Table, new: &toml::Table, new_items: Option<&dyn TableLike>) {
    let keys: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for key in keys {
        if old.get(key) == new.get(key) {
            continue;
        }
        match new_items.and_then(|t| t.get(key)) {
            Some(item) if new.contains_key(key) => {
                // Keep the existing key (and its leading comment) when it's there.
                match target.get_mut(key) {
                    Some(slot) => *slot = item.clone(),
                    None => {
                        target.insert(key, item.clone());
                    }
                }
            }
            _ => {
                target.remove(key);
            }
        }
    }
}

/// Detects changes to a file by modification time.
pub struct FileWatcher {
    path: PathBuf,
    last: Option<std::time::SystemTime>,
}

impl FileWatcher {
    pub fn new(path: &Path) -> Self {
        let last = Self::mtime(path);
        Self { path: path.to_path_buf(), last }
    }

    fn mtime(path: &Path) -> Option<std::time::SystemTime> {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
    }

    /// True if the file's mtime changed since the last call (or `new`).
    pub fn poll(&mut self) -> bool {
        let now = Self::mtime(&self.path);
        if now != self.last {
            self.last = now;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn save_preserves_comments_and_unknown_keys() {
        let (_d, path) = write_tmp("# top\n[settings]\n# pick one\ntheme = \"nord\"\nfuture_key = 1\n");
        let base = Config::load_or_create(&path).unwrap();
        let mut cfg = base.clone();
        cfg.settings.theme = "dracula".into();
        cfg.save(&base, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# top"), "{text}");
        assert!(text.contains("# pick one"), "{text}");
        assert!(text.contains("future_key = 1"), "{text}");
        assert!(text.contains("theme = \"dracula\""), "{text}");
    }

    #[test]
    fn old_config_loads() {
        let old = format!(
            "{TEMPLATE}\n[[theme]]\nname = \"mine\"\nbg = \"#000000\"\nfg = \"#ffffff\"\naccent = \"red\"\nmuted = \"gray\"\nborder = \"gray\"\nselection_bg = \"blue\"\nselection_fg = \"white\"\nsuccess = \"green\"\nwarning = \"yellow\"\nerror = \"red\"\n"
        );
        let cfg = Config::parse(&old).unwrap();
        assert_eq!(cfg.gui.font_size, 14.0);
        assert_eq!(cfg.gui.ui_scale, 1.0);
        assert!(cfg.themes[0].ansi.is_none());
        assert!(cfg.keys.is_empty());
    }

    #[test]
    fn save_unchanged_is_noop() {
        let (_d, path) = write_tmp(TEMPLATE);
        let before = std::fs::read(&path).unwrap();
        let c = Config::load_or_create(&path).unwrap();
        c.save(&c, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn connections_array_replaced_on_change() {
        let (_d, path) = write_tmp(TEMPLATE);
        let base = Config::load_or_create(&path).unwrap();
        let mut cfg = base.clone();
        cfg.connections.push(Connection { name: "a".into(), host: "h".into(), port: Some(2), ..Default::default() });
        cfg.keys.insert("quick_switcher".into(), "Ctrl+Shift+K".into());
        cfg.gui.font_size = 16.0;
        cfg.save(&base, &path).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap(), cfg);
        let base = cfg.clone();
        cfg.connections.clear();
        cfg.save(&base, &path).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap(), cfg);
    }

    #[test]
    fn watcher_detects_change() {
        let (_d, path) = write_tmp("[settings]\n");
        let mut w = FileWatcher::new(&path);
        assert!(!w.poll());
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "[settings]\ntheme = \"nord\"\n").unwrap();
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        assert!(w.poll());
        assert!(!w.poll());
    }

    #[test]
    fn save_keeps_other_frontends_changes() {
        let (_d, path) = write_tmp("[settings]\ntheme = \"nord\"\n");
        let a = Config::load_or_create(&path).unwrap();
        let b = Config::load_or_create(&path).unwrap();
        let mut b2 = b.clone();
        b2.settings.theme = "dracula".into();
        b2.save(&b, &path).unwrap();
        let mut a2 = a.clone();
        a2.gui.font_size = 16.0;
        a2.save(&a, &path).unwrap();
        let now = Config::load_or_create(&path).unwrap();
        assert_eq!(now.settings.theme, "dracula");
        assert_eq!(now.gui.font_size, 16.0);
    }

    const TWO_CONNS: &str = "[[connection]]\nname = \"a\"\nhost = \"ha\"\n\n[[connection]]\n# note b\nname = \"b\"\nhost = \"hb\"\nfuture = 1\n";

    #[test]
    fn connection_edit_keeps_comments_in_other_blocks() {
        let (_d, path) = write_tmp(TWO_CONNS);
        let base = Config::load_or_create(&path).unwrap();
        let mut cfg = base.clone();
        cfg.connections[0].host = "ha2".into();
        cfg.save(&base, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# note b"), "{text}");
        assert!(text.contains("future = 1"), "{text}");
        assert_eq!(Config::load_or_create(&path).unwrap().connections[0].host, "ha2");
    }

    #[test]
    fn concurrent_connection_add_survives() {
        let (_d, path) = write_tmp(TWO_CONNS);
        let a = Config::load_or_create(&path).unwrap();
        let b = Config::load_or_create(&path).unwrap();
        let mut b2 = b.clone();
        b2.connections.push(Connection { name: "c".into(), host: "hc".into(), ..Default::default() });
        b2.save(&b, &path).unwrap();
        let mut a2 = a.clone();
        a2.connections[1].host = "hb2".into();
        a2.connections.retain(|c| c.name != "a");
        a2.save(&a, &path).unwrap();
        let names: Vec<(String, String)> =
            Config::load_or_create(&path).unwrap().connections.into_iter().map(|c| (c.name, c.host)).collect();
        assert_eq!(names, [("b".into(), "hb2".into()), ("c".into(), "hc".into())]);
    }

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(Config::parse(TEMPLATE).unwrap(), Config::default());
    }

    #[test]
    fn roundtrip() {
        let mut cfg = Config::default();
        cfg.settings.theme = "nord".into();
        cfg.connections.push(Connection {
            name: "box".into(),
            host: "10.0.0.2".into(),
            port: Some(2200),
            extra_args: vec!["-A".into()],
            ..Default::default()
        });
        cfg.themes.push(crate::theme::builtin().remove(1));
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(Config::parse(&text).unwrap(), cfg);
    }
}
