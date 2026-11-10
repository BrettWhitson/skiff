//! UI state that changes as the user works (window size, sidebar), kept out of
//! `config.toml` so dragging a splitter never rewrites the user's config.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Window and host-list layout saved in `state.toml` between runs. Missing
/// fields take their defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// Last window size in pixels (GUI only); `None` lets the front-end choose.
    pub window_size: Option<(u32, u32)>,
    /// Width of the host sidebar in pixels.
    pub sidebar_width: f32,
    /// Whether the sidebar is folded away.
    pub sidebar_collapsed: bool,
    /// Host list order.
    pub sort: SortMode,
    /// Show hosts marked hidden.
    pub show_hidden: bool,
}

/// How the host list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    #[default]
    /// Alphabetically by host name, ignoring case.
    Name,
    /// Most recently connected first; never-connected hosts last.
    Recent,
}

impl SortMode {
    /// Text shown for this mode in the UI.
    pub fn label(self) -> &'static str {
        match self {
            SortMode::Name => "Name",
            SortMode::Recent => "Recent",
        }
    }

    /// The mode a "toggle sort" action switches to.
    pub fn next(self) -> SortMode {
        match self {
            SortMode::Name => SortMode::Recent,
            SortMode::Recent => SortMode::Name,
        }
    }
}

impl Default for UiState {
    /// A 240-pixel sidebar, expanded, sorted by name, hidden hosts not shown.
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

    /// Writes the state, creating the config directory if needed.
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

    /// `path_for` points next to the config; missing or unparsable files load as
    /// defaults; a saved state loads back unchanged.
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
