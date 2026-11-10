//! The TUI's resolved color theme: a skiff-core `ThemeDef` (color strings)
//! parsed into ratatui colors, plus the handful of styles the UI draws with.
//! Every style goes through here so `NO_COLOR` (monochrome: bold and
//! reverse only) and the ASCII border fallback apply everywhere.

use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use skiff_core::theme::ThemeDef;

/// Box drawing with ASCII only (`tui_ascii_borders`).
pub const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// True when `NO_COLOR` holds a non-empty value (see no-color.org).
pub fn no_color(var: Option<std::ffi::OsString>) -> bool {
    var.is_some_and(|v| !v.is_empty())
}

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
    /// Monochrome (`NO_COLOR`): no colors, emphasis by bold and reverse only.
    pub mono: bool,
    /// ASCII borders and marks (`tui_ascii_borders`).
    pub ascii: bool,
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
            mono: false,
            ascii: false,
        })
    }

    /// `color` as a foreground, or no style at all in monochrome.
    fn fg(&self, color: Color) -> Style {
        if self.mono { Style::new() } else { Style::new().fg(color) }
    }

    /// `uni`, or `ascii` with ASCII borders on.
    pub fn glyph(&self, uni: &'static str, ascii: &'static str) -> &'static str {
        if self.ascii { ascii } else { uni }
    }

    /// The border line set: rounded, or ASCII.
    pub fn border_set(&self) -> border::Set<'static> {
        if self.ascii { ASCII_BORDER } else { border::ROUNDED }
    }

    /// Default text on the default background.
    pub fn base(&self) -> Style {
        if self.mono { Style::new().fg(Color::Reset).bg(Color::Reset) } else { Style::new().fg(self.fg).bg(self.bg) }
    }
    /// Default text color only.
    pub fn text(&self) -> Style {
        self.fg(self.fg)
    }
    /// Secondary text.
    pub fn muted(&self) -> Style {
        self.fg(self.muted)
    }
    /// Accent-colored text (bold in monochrome).
    pub fn accent(&self) -> Style {
        if self.mono { Style::new().add_modifier(Modifier::BOLD) } else { Style::new().fg(self.accent) }
    }
    /// Bold accent, for panel and dialog titles.
    pub fn title(&self) -> Style {
        self.fg(self.accent).add_modifier(Modifier::BOLD)
    }
    /// Bold default text, for names and headings.
    pub fn strong(&self) -> Style {
        self.fg(self.fg).add_modifier(Modifier::BOLD)
    }
    /// Border style: accent when focused, the border color otherwise (in
    /// monochrome: bold when focused).
    pub fn border(&self, focused: bool) -> Style {
        match (self.mono, focused) {
            (true, true) => Style::new().add_modifier(Modifier::BOLD),
            (true, false) => Style::new(),
            (false, _) => Style::new().fg(if focused { self.accent } else { self.border }),
        }
    }
    /// The selected row: selection colors, bold (reverse in monochrome).
    pub fn selected(&self) -> Style {
        if self.mono {
            return Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        Style::new().fg(self.selection_fg).bg(self.selection_bg).add_modifier(Modifier::BOLD)
    }
    /// A key in a hint (`Enter` of `Enter ssh`).
    pub fn key(&self) -> Style {
        self.accent()
    }
    /// A badge: reversed accent, bold.
    pub fn badge(&self) -> Style {
        if self.mono {
            return Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        Style::new().fg(self.selection_fg).bg(self.selection_bg).add_modifier(Modifier::BOLD)
    }
    /// The pending-prefix badge: reversed warning color, bold.
    pub fn alert_badge(&self) -> Style {
        if self.mono {
            return Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        Style::new().fg(self.warning).add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
    /// Success text (plain in monochrome).
    pub fn success(&self) -> Style {
        self.fg(self.success)
    }
    /// Warning text (bold in monochrome).
    pub fn warning(&self) -> Style {
        if self.mono { Style::new().add_modifier(Modifier::BOLD) } else { Style::new().fg(self.warning) }
    }
    /// Error text (bold in monochrome).
    pub fn error(&self) -> Style {
        if self.mono { Style::new().add_modifier(Modifier::BOLD) } else { Style::new().fg(self.error) }
    }
    /// A destructive dialog's border: the error color, bold.
    pub fn danger_border(&self) -> Style {
        self.error().add_modifier(Modifier::BOLD)
    }
    /// A cell behind an open dialog: its text dimmed to the muted color, with
    /// no background or reverse (monochrome keeps only the dim).
    pub fn backdrop(&self, cell: Style) -> Style {
        let mut s = cell.remove_modifier(Modifier::BOLD | Modifier::REVERSED).add_modifier(Modifier::DIM);
        if !self.mono {
            s = s.fg(self.muted).bg(self.bg);
        }
        s
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

    /// Monochrome styles carry no colors; emphasis is bold or reverse.
    #[test]
    fn no_color_is_monochrome() {
        assert!(no_color(Some("1".into())) && !no_color(Some("".into())) && !no_color(None));
        let mut t = Theme::from_def(&builtin()[1]).unwrap();
        t.mono = true;
        for s in [
            t.base(),
            t.text(),
            t.muted(),
            t.accent(),
            t.title(),
            t.border(true),
            t.selected(),
            t.badge(),
            t.error(),
            t.backdrop(t.title()),
        ] {
            assert!(matches!(s.fg, None | Some(Color::Reset)) && matches!(s.bg, None | Some(Color::Reset)), "{s:?}");
        }
        assert!(t.selected().add_modifier.contains(Modifier::REVERSED));
    }

    /// A bad color is rejected with the offending field named.
    #[test]
    fn bad_color_is_reported() {
        let mut d = builtin().remove(0);
        d.accent = "notacolor".into();
        assert!(Theme::from_def(&d).unwrap_err().contains("accent"));
    }
}
