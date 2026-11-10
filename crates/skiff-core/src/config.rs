//! skiff's own config file (`config.toml`): settings, GUI options, key
//! bindings, forwards, snippets, custom connections and themes. Saving edits
//! the file in place so comments, unknown keys and other front-ends' changes
//! survive.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike};

use crate::theme::ThemeDef;

/// A connection defined in skiff's own config (as opposed to `~/.ssh/config`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Connection {
    /// Display name; also the key used to match this block when saving.
    pub name: String,
    /// Host name or address to connect to.
    pub host: String,
    /// Login user; `None` leaves it to ssh.
    pub user: Option<String>,
    /// Port; `None` leaves it to ssh (normally 22).
    pub port: Option<u16>,
    /// Private key file (`-i`).
    pub identity_file: Option<String>,
    /// Jump host(s) (`-J`).
    pub proxy_jump: Option<String>,
    /// Passed as `-o ProxyCommand=…`.
    pub proxy_command: Option<String>,
    /// Extra arguments passed to `ssh` before the destination, e.g. `["-A", "-o", "Compression=yes"]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
    /// Directory the SFTP browser opens in on the remote side.
    pub remote_dir: Option<String>,
    /// Free-form labels shown in host lists and matched by search.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Left out of host lists unless "Show hidden hosts" is on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// Number of text fields in the connection editor.
pub const CONNECTION_FIELDS: usize = 10;

/// Field labels of the connection editor (TUI and GUI), in order.
pub const CONNECTION_LABELS: [&str; CONNECTION_FIELDS] =
    ["Name", "Host", "User", "Port", "Identity file", "ProxyJump", "ProxyCommand", "Extra ssh args", "Remote dir", "Tags"];

impl Connection {
    /// The editor's text fields, in `CONNECTION_LABELS` order.
    pub fn to_fields(&self) -> [String; CONNECTION_FIELDS] {
        let o = |v: &Option<String>| v.clone().unwrap_or_default();
        [
            self.name.clone(),
            self.host.clone(),
            o(&self.user),
            self.port.map(|p| p.to_string()).unwrap_or_default(),
            o(&self.identity_file),
            o(&self.proxy_jump),
            o(&self.proxy_command),
            shell_words::join(&self.extra_args),
            o(&self.remote_dir),
            self.tags.join(", "),
        ]
    }

    /// Parses and validates the editor's fields (name and host required,
    /// port 1-65535 when given). `hidden` is not a text field and stays false.
    pub fn from_fields(fields: &[String; CONNECTION_FIELDS]) -> Result<Connection, String> {
        // `v` is the trimmed field text; `opt` is the same but `None` when empty.
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
            // Port 0 is rejected along with non-numbers.
            Some(p) => Some(p.parse::<u16>().ok().filter(|&n| n > 0).ok_or_else(|| format!("Invalid port: {p}"))?),
        };
        // Extra args are split with shell quoting rules, so quoted spaces survive.
        let extra_args = shell_words::split(&v(7)).map_err(|e| format!("Extra ssh args: {e}"))?;
        Ok(Connection {
            name: v(0),
            host: v(1),
            user: opt(2),
            port,
            identity_file: opt(4),
            proxy_jump: opt(5),
            proxy_command: opt(6),
            extra_args,
            remote_dir: opt(8),
            hidden: false,
            tags: v(9).split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        })
    }
}

/// General settings (`[settings]`), used by both front-ends.
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
    /// Show dot-files in the SFTP browser.
    pub show_hidden_files: bool,
    /// Directory the local pane of the SFTP browser starts in (default: home).
    pub local_start_dir: Option<String>,
    /// When host status checks run: only on request, or also at start.
    pub status_checks: StatusChecks,
    /// TUI without box-drawing borders and symbols (for screen readers).
    pub tui_plain: bool,
    /// Ask before pasting several lines into a program without bracketed paste.
    pub confirm_multiline_paste: bool,
    /// When the TUI takes the mouse from the terminal it runs in.
    pub tui_mouse: TuiMouse,
    /// TUI borders drawn with `+ - |` (and ASCII marks) for fonts or
    /// terminals without box-drawing characters.
    pub tui_ascii_borders: bool,
    /// How long the TUI waits after the prefix key before listing the keys
    /// that can follow it, in milliseconds (0: at once).
    pub which_key_delay_ms: u64,
}

/// `tui_mouse`: `"auto"` captures the mouse only while a program in the
/// active terminal asks for it (the terminal's own selection works the rest
/// of the time); `"always"` captures it for tab clicks, wheel scrollback
/// and skiff's own selection (copied over OSC 52).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TuiMouse {
    #[default]
    /// Capture only while a program in the active terminal asks for the mouse.
    Auto,
    /// Always capture the mouse.
    Always,
}

/// `status_checks`: when host reachability probes run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StatusChecks {
    #[default]
    /// Only when the user asks.
    Manual,
    /// Also once at startup.
    OnStart,
}

/// The defaults a fresh install uses (matching the first-run template).
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
            status_checks: StatusChecks::Manual,
            tui_plain: false,
            confirm_multiline_paste: true,
            tui_mouse: TuiMouse::Auto,
            tui_ascii_borders: false,
            which_key_delay_ms: 300,
        }
    }
}

/// Which terminal output screen readers hear (`gui.announce_output`):
/// `true` (all tabs), `false`, or `"focused"` (only the focused terminal).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Announce {
    #[default]
    All,
    Off,
    Focused,
}

impl Serialize for Announce {
    /// Writes `All`/`Off` as booleans and `Focused` as `"focused"`.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Announce::All => s.serialize_bool(true),
            Announce::Off => s.serialize_bool(false),
            Announce::Focused => s.serialize_str("focused"),
        }
    }
}

impl<'de> Deserialize<'de> for Announce {
    /// Accepts a boolean or the string `"focused"` (any case).
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        /// Either form `announce_output` accepts in the file.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            B(bool),
            S(String),
        }
        match Repr::deserialize(d)? {
            Repr::B(true) => Ok(Announce::All),
            Repr::B(false) => Ok(Announce::Off),
            Repr::S(s) if s.eq_ignore_ascii_case("focused") => Ok(Announce::Focused),
            Repr::S(s) => Err(serde::de::Error::custom(format!("announce_output: expected true, false or \"focused\", got {s:?}"))),
        }
    }
}

/// GUI-only settings (`[gui]`). The TUI ignores them. A `font_family` key
/// from older configs is accepted and ignored (unknown keys are kept on save).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GuiSettings {
    /// Terminal (monospace) font file to use instead of the bundled fonts.
    pub font_path: Option<String>,
    /// Interface font file to use instead of the bundled fonts.
    pub ui_font_path: Option<String>,
    /// Terminal font size in points.
    pub font_size: f32,
    /// Multiplier for the whole interface's size.
    pub ui_scale: f32,
    /// New terminal output read out by screen readers.
    pub announce_output: Announce,
    /// Blinking text and terminal cursors.
    pub cursor_blink: bool,
}

/// 14 pt terminal font, 100% scale, all output announced, blinking on.
impl Default for GuiSettings {
    fn default() -> Self {
        Self { font_path: None, ui_font_path: None, font_size: 14.0, ui_scale: 1.0, announce_output: Announce::All, cursor_blink: true }
    }
}

/// The whole `config.toml`. Missing sections and keys take their defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Config {
    /// `[settings]`.
    pub settings: Settings,
    /// `[gui]`.
    pub gui: GuiSettings,
    /// Action name → key chord (e.g. `quick_switcher = "Ctrl+Shift+K"`). Parsed by the GUI.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
    /// Action name → key chord for the terminal UI (e.g. `close_tab = "alt+x"`). Parsed by the TUI.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tui_keys: BTreeMap<String, String>,
    /// Port forwards (`[[forward]]`).
    #[serde(rename = "forward", skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<crate::forward::Forward>,
    /// Saved commands (`[[snippet]]`).
    #[serde(rename = "snippet", skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<crate::termx::Snippet>,
    /// Custom connections (`[[connection]]`).
    #[serde(rename = "connection", skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<Connection>,
    /// Custom themes (`[[theme]]`), alongside the built-in ones.
    #[serde(rename = "theme", skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<ThemeDef>,
}

/// The variable that overrides where the config file lives.
pub const CONFIG_ENV: &str = "SKIFF_CONFIG";

/// Written as the config on first run: the defaults plus commented examples.
const TEMPLATE: &str = r##"# Skiff configuration
# Skiff only rewrites the settings you change in the app; comments and
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
    /// `$SKIFF_CONFIG` (or the pre-rename `$TERMAX_CONFIG`), or the platform
    /// config dir (`%APPDATA%\skiff\config\config.toml`, `~/.config/skiff/config.toml`, ...).
    pub fn default_path() -> PathBuf {
        // Either variable wins when set to a non-empty value.
        for var in [CONFIG_ENV, crate::migrate::LEGACY_CONFIG_ENV] {
            if let Some(p) = std::env::var_os(var).filter(|p| !p.is_empty()) {
                return PathBuf::from(p);
            }
        }
        directories::ProjectDirs::from("", "", "skiff")
            .map(|d| d.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("skiff.toml"))
    }

    /// Loads the config, writing the commented template on first run (not
    /// into a folder held after a failed migration: defaults then).
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() && path.parent().is_some_and(crate::migrate::is_held) {
            return Self::parse(TEMPLATE);
        }
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
            std::fs::write(path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Loads the config when the file exists, else the template's defaults,
    /// writing nothing (used after a failed migration so the next start retries).
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if path.exists() { Self::load_or_create(path) } else { Self::parse(TEMPLATE) }
    }

    /// Parses config text; unknown keys are ignored, missing ones defaulted.
    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// Writes the changes between `baseline` (what this front-end loaded or
    /// last saved) and `self` into the file on disk. Everything else in the
    /// file — comments, unknown keys, and changes made meanwhile by another
    /// front-end — is kept.
    pub fn save(&self, baseline: &Config, path: &Path) -> Result<()> {
        // After a failed migration the config folder must not appear (see `migrate::hold_dir`).
        if path.parent().is_some_and(crate::migrate::is_held) {
            anyhow::bail!(crate::migrate::HELD_REASON);
        }
        // Diff via `toml::Value` (baseline vs. new) but edit `doc`, a format-preserving
        // parse of the file as it is now; `new_doc` supplies the formatted values.
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: DocumentMut = existing.parse().context("parsing existing config")?;
        let old = toml::Value::try_from(baseline)?;
        let new = toml::Value::try_from(self)?;
        let new_doc: DocumentMut = toml::to_string(self)?.parse()?;
        let section = |v: &toml::Value, k: &str| v.get(k).and_then(|t| t.as_table()).cloned().unwrap_or_default();

        // Plain tables: rewrite only the keys whose values changed.
        for name in ["settings", "gui", "keys", "tui_keys"] {
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

        // Arrays of tables, matched by their `name` key.
        for name in ["connection", "theme", "forward", "snippet"] {
            let list = |v: &toml::Value| -> Vec<toml::Table> {
                v.get(name).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|t| t.as_table().cloned()).collect()).unwrap_or_default()
            };
            let (old_l, new_l) = (list(&old), list(&new));
            if old_l == new_l {
                continue;
            }
            // Forwards have no unique name: rewrite the whole list.
            if name == "forward" {
                match new_doc.get(name) {
                    Some(item) => {
                        doc.insert(name, item.clone());
                    }
                    None => {
                        doc.remove(name);
                    }
                }
                continue;
            }
            let key = |t: &toml::Table| t.get("name").and_then(|n| n.as_str()).map(str::to_string);
            let new_items: Vec<Table> =
                new_doc.get(name).and_then(|i| i.as_array_of_tables()).map(|a| a.iter().cloned().collect()).unwrap_or_default();
            if !doc.contains_array_of_tables(name) {
                doc.insert(name, Item::ArrayOfTables(ArrayOfTables::new()));
            }
            let aot = doc[name].as_array_of_tables_mut().context("config array is not an array of tables")?;
            let find = |aot: &ArrayOfTables, n: &str| aot.iter().position(|t| t.get("name").and_then(|v| v.as_str()) == Some(n));

            // Removed here → remove from the file.
            for o in &old_l {
                let Some(n) = key(o) else { continue };
                if !new_l.iter().any(|t| key(t).as_deref() == Some(n.as_str()))
                    && let Some(i) = find(aot, &n)
                {
                    aot.remove(i);
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
        // Write to a temp file and rename, so a reader never sees half a file.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Sets or removes, in `target`, each key whose value differs between `old`
/// and `new`. Values come from `new_items` (the serialized new config).
fn apply_key_changes(target: &mut dyn TableLike, old: &toml::Table, new: &toml::Table, new_items: Option<&dyn TableLike>) {
    // Every key present in either version.
    let keys: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for key in keys {
        if old.get(key) == new.get(key) {
            continue;
        }
        match new_items.and_then(|t| t.get(key)) {
            // Changed or added: copy the formatted value from `new_items`. Anything else
            // (removed in `new`) is deleted from the file.
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
    /// The watched file.
    path: PathBuf,
    /// Modification time last seen; `None` if the file was missing.
    last: Option<std::time::SystemTime>,
}

impl FileWatcher {
    /// Starts watching `path`, taking its current mtime as the baseline.
    pub fn new(path: &Path) -> Self {
        let last = Self::mtime(path);
        Self { path: path.to_path_buf(), last }
    }

    /// The file's modification time, or `None` if it can't be read.
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

    /// Writes `text` to `config.toml` in a fresh temp dir; keep the dir alive.
    fn write_tmp(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    /// Changing one setting keeps the file's comments and unknown keys.
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

    /// A config written before newer fields existed still loads with defaults.
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

    /// The retired `[gui] font_family` key still loads, has no effect, and
    /// survives a save untouched (fonts are chosen by `font_path` / `ui_font_path`).
    #[test]
    fn legacy_font_family_is_ignored() {
        let (_d, path) = write_tmp("[gui]\nfont_family = \"Fira Code\"\n");
        let base = Config::load_or_create(&path).unwrap();
        assert_eq!(base.gui, GuiSettings::default());
        let mut cfg = base.clone();
        cfg.gui.font_size = 16.0;
        cfg.save(&base, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("font_family = \"Fira Code\""), "{text}");
    }

    /// `tui_mouse` defaults to auto, accepts "always", rejects other values.
    #[test]
    fn tui_mouse_setting() {
        assert_eq!(Config::parse("").unwrap().settings.tui_mouse, TuiMouse::Auto);
        assert_eq!(
            Config::parse(
                "[settings]
tui_mouse = \"always\"
"
            )
            .unwrap()
            .settings
            .tui_mouse,
            TuiMouse::Always
        );
        assert!(
            Config::parse(
                "[settings]
tui_mouse = \"sometimes\"
"
            )
            .is_err()
        );
    }

    /// Saving an unchanged config leaves the file byte-for-byte the same.
    #[test]
    fn save_unchanged_is_noop() {
        let (_d, path) = write_tmp(TEMPLATE);
        let before = std::fs::read(&path).unwrap();
        let c = Config::load_or_create(&path).unwrap();
        c.save(&c, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// Adding and then removing connections (plus key/GUI changes) round-trips
    /// through the file.
    #[test]
    fn connections_array_replaced_on_change() {
        let (_d, path) = write_tmp(TEMPLATE);
        let base = Config::load_or_create(&path).unwrap();
        let mut cfg = base.clone();
        cfg.connections.push(Connection { name: "a".into(), host: "h".into(), port: Some(2), ..Default::default() });
        cfg.keys.insert("quick_switcher".into(), "Ctrl+Shift+K".into());
        cfg.tui_keys.insert("close_tab".into(), "alt+x".into());
        cfg.gui.font_size = 16.0;
        cfg.save(&base, &path).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap(), cfg);
        let base = cfg.clone();
        cfg.connections.clear();
        cfg.save(&base, &path).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap(), cfg);
    }

    /// `poll` reports an mtime change once, then nothing until the next change.
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

    /// Two front-ends saving different settings from the same baseline both keep
    /// their change.
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

    /// Two connection blocks; the second has a comment and an unknown key.
    const TWO_CONNS: &str =
        "[[connection]]\nname = \"a\"\nhost = \"ha\"\n\n[[connection]]\n# note b\nname = \"b\"\nhost = \"hb\"\nfuture = 1\n";

    /// Editing one connection leaves the other block's comment and unknown key.
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

    /// One front-end adds a connection while another edits and deletes others:
    /// all three changes survive.
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

    /// The first-run template is exactly the defaults.
    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(Config::parse(TEMPLATE).unwrap(), Config::default());
    }

    /// A config with settings, a connection and a theme survives serialize/parse.
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
