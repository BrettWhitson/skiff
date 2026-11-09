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
    /// Terminal palette: 16 colors (black, red, green, yellow, blue, magenta,
    /// cyan, white, then their bright variants). Optional; the GUI falls back
    /// to a default palette.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ansi: Option<Vec<String>>,
}

fn def(name: &str, c: [&str; 10], ansi: [&str; 16]) -> ThemeDef {
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
        ansi: Some(ansi.iter().map(|s| s.to_string()).collect()),
    }
}

/// Built-in themes. Order: bg, fg, accent, muted, border, sel_bg, sel_fg, ok, warn, err.
pub fn builtin() -> Vec<ThemeDef> {
    vec![
        def("terminal", ["reset", "reset", "cyan", "darkgray", "gray", "cyan", "black", "green", "yellow", "red"], ["#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5", "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff"]),
        def("midnight", ["#0f1117", "#d6deeb", "#7fdbca", "#637777", "#2a3140", "#1d3b53", "#ffffff", "#addb67", "#ecc48d", "#ef5350"], ["#011627", "#ef5350", "#22da6e", "#c5e478", "#82aaff", "#c792ea", "#21c7a8", "#ffffff", "#575656", "#ef5350", "#22da6e", "#ffeb95", "#82aaff", "#c792ea", "#7fdbca", "#ffffff"]),
        def("dracula", ["#282a36", "#f8f8f2", "#bd93f9", "#6272a4", "#44475a", "#44475a", "#f8f8f2", "#50fa7b", "#f1fa8c", "#ff5555"], ["#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff"]),
        def("nord", ["#2e3440", "#d8dee9", "#88c0d0", "#616e88", "#434c5e", "#4c566a", "#eceff4", "#a3be8c", "#ebcb8b", "#bf616a"], ["#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0", "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"]),
        def("gruvbox", ["#282828", "#ebdbb2", "#fabd2f", "#928374", "#504945", "#504945", "#fbf1c7", "#b8bb26", "#fe8019", "#fb4934"], ["#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984", "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2"]),
        def("solarized-dark", ["#002b36", "#93a1a1", "#268bd2", "#586e75", "#073642", "#073642", "#eee8d5", "#859900", "#b58900", "#dc322f"], ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"]),
        def("catppuccin", ["#1e1e2e", "#cdd6f4", "#cba6f7", "#6c7086", "#45475a", "#45475a", "#cdd6f4", "#a6e3a1", "#f9e2af", "#f38ba8"], ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de", "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8"]),
        def("paper", ["#fafafa", "#383a42", "#4078f2", "#a0a1a7", "#d0d0d0", "#e5e5e6", "#202227", "#50a14f", "#c18401", "#e45649"], ["#383a42", "#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc", "#a0a1a7", "#4f525e", "#e06c75", "#98c379", "#d19a66", "#61afef", "#c678dd", "#56b6c2", "#ffffff"]),
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
    fn builtin_palettes_have_16_valid_colors() {
        for d in builtin() {
            let ansi = d.ansi.as_ref().unwrap_or_else(|| panic!("{} has no palette", d.name));
            assert_eq!(ansi.len(), 16, "{}", d.name);
            assert!(ansi.iter().all(|c| matches!(parse_color(c), Some(Rgb::Rgb(..)))), "{}", d.name);
        }
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
