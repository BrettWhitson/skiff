use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Config {
    pub settings: Settings,
    #[serde(rename = "connection", skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<Connection>,
    #[serde(rename = "theme", skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<ThemeDef>,
}

const TEMPLATE: &str = r##"# termax configuration
# Note: termax rewrites this file when you change settings in the app,
# so comments you add here may be lost.

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

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
