//! The TUI's resolved color theme: a skiff-core `ThemeDef` (color strings)
//! parsed into ratatui colors, plus the handful of styles the UI draws with.

use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use skiff_core::theme::ThemeDef;

/// Theme colors as ratatui values, built from a `ThemeDef` by [`Theme::from_def`].
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    /// Default background.
    pub bg: Color,
    /// Default text.
    pub fg: Color,
    /// Emphasis color (titles, focused borders).
    pub accent: Color,
    /// Secondary text.
    pub muted: Color,
    /// Unfocused borders.
    pub border: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    /// Plain mode: no borders, words instead of symbols.
    pub plain: bool,
}

impl Theme {
    /// Parses every color of `def` (names like `red`, `#rrggbb`, indexes);
    /// the error names the theme, the field and the bad value. Plain mode starts off.
    pub fn from_def(def: &ThemeDef) -> Result<Self, String> {
        let c =
            |field: &str, v: &str| Color::from_str(v.trim()).map_err(|_| format!("theme '{}': invalid color for {field}: '{v}'", def.name));
        Ok(Self {
            name: def.name.clone(),
            bg: c("bg", &def.bg)?,
            fg: c("fg", &def.fg)?,
            accent: c("accent", &def.accent)?,
            muted: c("muted", &def.muted)?,
            border: c("border", &def.border)?,
            selection_bg: c("selection_bg", &def.selection_bg)?,
            selection_fg: c("selection_fg", &def.selection_fg)?,
            success: c("success", &def.success)?,
            warning: c("warning", &def.warning)?,
            error: c("error", &def.error)?,
            plain: false,
        })
    }

    /// Default text on the default background.
    pub fn base(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }
    /// Secondary text.
    pub fn muted(&self) -> Style {
        Style::new().fg(self.muted)
    }
    /// Accent-colored text.
    pub fn accent(&self) -> Style {
        Style::new().fg(self.accent)
    }
    /// Bold accent, for panel and dialog titles.
    pub fn title(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }
    /// Border style: accent when focused, the border color otherwise.
    pub fn border(&self, focused: bool) -> Style {
        Style::new().fg(if focused { self.accent } else { self.border })
    }
    /// The selected row: selection colors, bold.
    pub fn selected(&self) -> Style {
        Style::new().fg(self.selection_fg).bg(self.selection_bg).add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_core::theme::builtin;

    /// Every built-in theme's colors parse.
    #[test]
    fn builtins_parse() {
        for d in builtin() {
            Theme::from_def(&d).unwrap();
        }
    }

    /// A bad color is rejected with the offending field named.
    #[test]
    fn bad_color_is_reported() {
        let mut d = builtin().remove(0);
        d.accent = "notacolor".into();
        assert!(Theme::from_def(&d).unwrap_err().contains("accent"));
    }
}
