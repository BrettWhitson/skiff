use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use termax_core::theme::ThemeDef;

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub muted: Color,
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

    pub fn base(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }
    pub fn muted(&self) -> Style {
        Style::new().fg(self.muted)
    }
    pub fn accent(&self) -> Style {
        Style::new().fg(self.accent)
    }
    pub fn title(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }
    pub fn border(&self, focused: bool) -> Style {
        Style::new().fg(if focused { self.accent } else { self.border })
    }
    pub fn selected(&self) -> Style {
        Style::new().fg(self.selection_fg).bg(self.selection_bg).add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termax_core::theme::builtin;

    #[test]
    fn builtins_parse() {
        for d in builtin() {
            Theme::from_def(&d).unwrap();
        }
    }

    #[test]
    fn bad_color_is_reported() {
        let mut d = builtin().remove(0);
        d.accent = "notacolor".into();
        assert!(Theme::from_def(&d).unwrap_err().contains("accent"));
    }
}
