use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

/// A theme as written in the config file. Colors are `#rrggbb`, a color name
/// (`red`, `lightblue`, ...), an ANSI index (`0`-`255`), or `reset` for the
/// terminal's own default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThemeDef {
    pub name: String,
    pub bg: String,
    pub fg: String,
    pub accent: String,
    pub muted: String,
    pub border: String,
    pub selection_bg: String,
    pub selection_fg: String,
    pub success: String,
    pub warning: String,
    pub error: String,
}

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
}

impl Theme {
    pub fn from_def(def: &ThemeDef) -> Result<Self, String> {
        let c = |field: &str, v: &str| {
            Color::from_str(v.trim())
                .map_err(|_| format!("theme '{}': invalid color for {field}: '{v}'", def.name))
        };
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

fn def(name: &str, c: [&str; 10]) -> ThemeDef {
    ThemeDef {
        name: name.into(),
        bg: c[0].into(),
        fg: c[1].into(),
        accent: c[2].into(),
        muted: c[3].into(),
        border: c[4].into(),
        selection_bg: c[5].into(),
        selection_fg: c[6].into(),
        success: c[7].into(),
        warning: c[8].into(),
        error: c[9].into(),
    }
}

/// Built-in themes. Order: bg, fg, accent, muted, border, sel_bg, sel_fg, ok, warn, err.
pub fn builtin() -> Vec<ThemeDef> {
    vec![
        def("terminal", ["reset", "reset", "cyan", "darkgray", "gray", "cyan", "black", "green", "yellow", "red"]),
        def("midnight", ["#0f1117", "#d6deeb", "#7fdbca", "#637777", "#2a3140", "#1d3b53", "#ffffff", "#addb67", "#ecc48d", "#ef5350"]),
        def("dracula", ["#282a36", "#f8f8f2", "#bd93f9", "#6272a4", "#44475a", "#44475a", "#f8f8f2", "#50fa7b", "#f1fa8c", "#ff5555"]),
        def("nord", ["#2e3440", "#d8dee9", "#88c0d0", "#616e88", "#434c5e", "#4c566a", "#eceff4", "#a3be8c", "#ebcb8b", "#bf616a"]),
        def("gruvbox", ["#282828", "#ebdbb2", "#fabd2f", "#928374", "#504945", "#504945", "#fbf1c7", "#b8bb26", "#fe8019", "#fb4934"]),
        def("solarized-dark", ["#002b36", "#93a1a1", "#268bd2", "#586e75", "#073642", "#073642", "#eee8d5", "#859900", "#b58900", "#dc322f"]),
        def("catppuccin", ["#1e1e2e", "#cdd6f4", "#cba6f7", "#6c7086", "#45475a", "#45475a", "#cdd6f4", "#a6e3a1", "#f9e2af", "#f38ba8"]),
        def("paper", ["#fafafa", "#383a42", "#4078f2", "#a0a1a7", "#d0d0d0", "#e5e5e6", "#202227", "#50a14f", "#c18401", "#e45649"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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
