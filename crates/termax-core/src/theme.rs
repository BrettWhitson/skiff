//! Theme definitions shared by all front-ends. Each front-end turns a
//! [`ThemeDef`] into its own color type.

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

/// A front-end-agnostic color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rgb {
    /// The front-end's default (the host terminal's color in the TUI).
    Reset,
    Rgb(u8, u8, u8),
    /// An ANSI palette index (0-15 named colors, 16-255 extended).
    Indexed(u8),
}

const NAMES: [&str; 16] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "gray",
    "darkgray", "lightred", "lightgreen", "lightyellow", "lightblue", "lightmagenta", "lightcyan", "white",
];

/// Parses a config color string (see [`ThemeDef`]).
pub fn parse_color(s: &str) -> Option<Rgb> {
    let s = s.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
    if s == "reset" {
        return Some(Rgb::Reset);
    }
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        return Some(Rgb::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
    }
    if let Ok(i) = s.parse::<u8>() {
        return Some(Rgb::Indexed(i));
    }
    let s = match s.as_str() {
        "grey" => "gray",
        "darkgrey" => "darkgray",
        other => other,
    };
    NAMES.iter().position(|n| *n == s).map(|i| Rgb::Indexed(i as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_formats() {
        assert_eq!(parse_color("#ff0080"), Some(Rgb::Rgb(255, 0, 128)));
        assert_eq!(parse_color("reset"), Some(Rgb::Reset));
        assert_eq!(parse_color("12"), Some(Rgb::Indexed(12)));
        assert_eq!(parse_color("lightblue"), Some(Rgb::Indexed(12)));
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn builtins_parse() {
        for d in builtin() {
            for c in [&d.bg, &d.fg, &d.accent, &d.muted, &d.border, &d.selection_bg, &d.selection_fg, &d.success, &d.warning, &d.error] {
                assert!(parse_color(c).is_some(), "{}: {c}", d.name);
            }
        }
    }
}
