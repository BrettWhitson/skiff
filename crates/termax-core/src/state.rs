//! UI state that changes as the user works (window size, sidebar), kept out of
//! `config.toml` so dragging a splitter never rewrites the user's config.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub window_size: Option<(u32, u32)>,
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    /// Host list order.
    pub sort: SortMode,
    /// Show hosts marked hidden.
    pub show_hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    #[default]
    Name,
    Recent,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            SortMode::Name => "Name",
            SortMode::Recent => "Recent",
        }
    }

    pub fn next(self) -> SortMode {
        match self {
            SortMode::Name => SortMode::Recent,
            SortMode::Recent => SortMode::Name,
        }
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self { window_size: None, sidebar_width: 240.0, sidebar_collapsed: false, sort: SortMode::Name, show_hidden: false }
    }
}

impl UiState {
    /// `state.toml` next to the config file.
    pub fn path_for(config_path: &Path) -> PathBuf {
        config_path.with_file_name("state.toml")
    }

    /// Missing or unreadable state falls back to defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrip_and_default_on_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let path = UiState::path_for(&cfg);
        assert_eq!(path, dir.path().join("state.toml"));
        assert_eq!(UiState::load(&path), UiState::default());
        assert_eq!(UiState::default().sidebar_width, 240.0);

        let s = UiState {
            window_size: Some((1200, 800)),
            sidebar_width: 300.0,
            sidebar_collapsed: true,
            sort: SortMode::Recent,
            show_hidden: true,
        };
        s.save(&path).unwrap();
        assert_eq!(UiState::load(&path), s);

        std::fs::write(&path, "this is = = not toml").unwrap();
        assert_eq!(UiState::load(&path), UiState::default());
    }
}
