//! Theme definitions shared by all front-ends. Each front-end turns a
//! [`ThemeDef`] into its own color type.

use serde::{Deserialize, Serialize};

/// A theme as written in the config file. Colors are `#rrggbb`, a color name
/// (`red`, `lightblue`, ...), an ANSI index (`0`-`255`), or `reset` for the
/// terminal's own default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThemeDef {
    /// Name used to pick the theme in the config (`theme = "nord"`).
    pub name: String,
    /// Window / page background.
    pub bg: String,
    /// Normal text.
    pub fg: String,
    /// Highlights: focus, active tab, headings.
    pub accent: String,
    /// Secondary text (hints, details).
    pub muted: String,
    /// Borders and separators.
    pub border: String,
    /// Background of the selected row.
    pub selection_bg: String,
    /// Text on the selected row.
    pub selection_fg: String,
    /// Success / connected status.
    pub success: String,
    /// Warnings.
    pub warning: String,
    /// Errors.
    pub error: String,
    /// Terminal palette: 16 colors (black, red, green, yellow, blue, magenta,
    /// cyan, white, then their bright variants). Optional; the GUI falls back
    /// to a default palette.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ansi: Option<Vec<String>>,
}

/// Builds a built-in theme from its ten UI colors (in field order) and its
/// 16-color terminal palette.
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
        def(
            "terminal",
            ["reset", "reset", "cyan", "darkgray", "gray", "cyan", "black", "green", "yellow", "red"],
            [
                "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5", "#666666", "#f14c4c", "#23d18b",
                "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
            ],
        ),
        def(
            "midnight",
            ["#0f1117", "#d6deeb", "#7fdbca", "#738587", "#2a3140", "#1d3b53", "#ffffff", "#addb67", "#ecc48d", "#ef5350"],
            [
                "#011627", "#ef5350", "#22da6e", "#c5e478", "#82aaff", "#c792ea", "#21c7a8", "#ffffff", "#575656", "#ef5350", "#22da6e",
                "#ffeb95", "#82aaff", "#c792ea", "#7fdbca", "#ffffff",
            ],
        ),
        def(
            "dracula",
            ["#282a36", "#f8f8f2", "#bd93f9", "#8f9abb", "#44475a", "#44475a", "#f8f8f2", "#50fa7b", "#f1fa8c", "#ff5555"],
            [
                "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94",
                "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
            ],
        ),
        def(
            "nord",
            ["#2e3440", "#d8dee9", "#88c0d0", "#9ba5b8", "#434c5e", "#4c566a", "#eceff4", "#a3be8c", "#ebcb8b", "#bf616a"],
            [
                "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0", "#4c566a", "#bf616a", "#a3be8c",
                "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
            ],
        ),
        def(
            "gruvbox",
            ["#282828", "#ebdbb2", "#fabd2f", "#a39480", "#504945", "#504945", "#fbf1c7", "#b8bb26", "#fe8019", "#fb4934"],
            [
                "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984", "#928374", "#fb4934", "#b8bb26",
                "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
            ],
        ),
        def(
            "solarized-dark",
            ["#002b36", "#eee8d5", "#268bd2", "#93a1a1", "#073642", "#0a4a5a", "#eee8d5", "#859900", "#b58900", "#dc322f"],
            [
                "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#002b36", "#cb4b16", "#586e75",
                "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
            ],
        ),
        def(
            "catppuccin",
            ["#1e1e2e", "#cdd6f4", "#cba6f7", "#868ca4", "#45475a", "#45475a", "#cdd6f4", "#a6e3a1", "#f9e2af", "#f38ba8"],
            [
                "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de", "#585b70", "#f38ba8", "#a6e3a1",
                "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
            ],
        ),
        def(
            "paper",
            ["#fafafa", "#383a42", "#4078f2", "#6c6d74", "#d0d0d0", "#e5e5e6", "#202227", "#50a14f", "#c18401", "#e45649"],
            [
                "#383a42", "#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc", "#a0a1a7", "#4f525e", "#e06c75", "#98c379",
                "#d19a66", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
            ],
        ),
        def(
            "high-contrast",
            ["#000000", "#ffffff", "#ffd700", "#c8c8c8", "#a0a0a0", "#ffd700", "#000000", "#55ff55", "#ffd700", "#ff6b6b"],
            [
                "#000000", "#ff5555", "#55ff55", "#ffff55", "#5c9cff", "#ff55ff", "#55ffff", "#e0e0e0", "#a0a0a0", "#ff7777", "#77ff77",
                "#ffff77", "#88bbff", "#ff77ff", "#77ffff", "#ffffff",
            ],
        ),
        def(
            "high-contrast-light",
            ["#ffffff", "#000000", "#0000c0", "#3d3d3d", "#5a5a5a", "#0000c0", "#ffffff", "#006400", "#6b4f00", "#b00020"],
            [
                "#000000", "#b00000", "#006400", "#6b4f00", "#0000c0", "#8b008b", "#005f73", "#3a3a3a", "#5a5a5a", "#c00000", "#007000",
                "#7a5c00", "#0000e0", "#a000a0", "#006b80", "#000000",
            ],
        ),
    ]
}

/// A front-end-agnostic color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rgb {
    /// The front-end's default (the host terminal's color in the TUI).
    Reset,
    /// A 24-bit color.
    Rgb(u8, u8, u8),
    /// An ANSI palette index (0-15 named colors, 16-255 extended).
    Indexed(u8),
}

/// Color names accepted in themes, in ANSI index order (0-15).
const NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "gray",
    "darkgray",
    "lightred",
    "lightgreen",
    "lightyellow",
    "lightblue",
    "lightmagenta",
    "lightcyan",
    "white",
];

/// Parses a config color string (see [`ThemeDef`]).
pub fn parse_color(s: &str) -> Option<Rgb> {
    // Normalise so "Light Blue", "light-blue" and "light_blue" all match.
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
    // A bare number is an ANSI palette index.
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

/// `#rrggbb` as RGB; `None` for names, indices and `reset`.
pub fn hex_rgb(s: &str) -> Option<(u8, u8, u8)> {
    match parse_color(s)? {
        Rgb::Rgb(r, g, b) if s.trim().starts_with('#') => Some((r, g, b)),
        _ => None,
    }
}

/// WCAG 2.1 contrast ratio between two colors (1 to 21).
pub fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    /// Relative luminance per WCAG (sRGB channels linearised).
    fn lum((r, g, b): (u8, u8, u8)) -> f32 {
        let ch = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
    }
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `a` moved toward `b` by `t` (0 to 1), per channel.
pub fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// The theme to use for configured name `name`: `"auto"` (and the default
/// `"terminal"`) become `high-contrast` while the OS high-contrast mode is
/// on; otherwise `"auto"` is `"terminal"`.
pub fn effective_name(name: &str, os_high_contrast: bool) -> &str {
    match name {
        "auto" | "terminal" if os_high_contrast => "high-contrast",
        "auto" => "terminal",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Hex, `reset`, indices and names parse; unknown names don't.
    fn parse_color_formats() {
        assert_eq!(parse_color("#ff0080"), Some(Rgb::Rgb(255, 0, 128)));
        assert_eq!(parse_color("reset"), Some(Rgb::Reset));
        assert_eq!(parse_color("12"), Some(Rgb::Indexed(12)));
        assert_eq!(parse_color("lightblue"), Some(Rgb::Indexed(12)));
        assert_eq!(parse_color("nope"), None);
    }

    /// Every built-in theme has a full 16-color hex palette.
    #[test]
    fn builtin_palettes_have_16_valid_colors() {
        for d in builtin() {
            let ansi = d.ansi.as_ref().unwrap_or_else(|| panic!("{} has no palette", d.name));
            assert_eq!(ansi.len(), 16, "{}", d.name);
            assert!(ansi.iter().all(|c| matches!(parse_color(c), Some(Rgb::Rgb(..)))), "{}", d.name);
        }
    }

    /// Every UI color of every built-in theme parses.
    #[test]
    fn builtins_parse() {
        for d in builtin() {
            for c in [&d.bg, &d.fg, &d.accent, &d.muted, &d.border, &d.selection_bg, &d.selection_fg, &d.success, &d.warning, &d.error] {
                assert!(parse_color(c).is_some(), "{}: {c}", d.name);
            }
        }
    }

    /// Built-in hex themes meet WCAG contrast (7:1 text for high-contrast themes,
    /// 4.5:1 otherwise), and `effective_name` maps `auto` as documented.
    #[test]
    fn themes_meet_contrast() {
        let mut bad = vec![];
        for d in builtin() {
            let c = |s: &str| hex_rgb(s);
            let (Some(bg), Some(fg), Some(muted), Some(accent), Some(border)) =
                (c(&d.bg), c(&d.fg), c(&d.muted), c(&d.accent), c(&d.border))
            else {
                continue; // named terminal colors: the terminal decides
            };
            let (sel_bg, sel_fg) = (c(&d.selection_bg).unwrap(), c(&d.selection_fg).unwrap());
            let surface = mix(bg, fg, 0.04); // the GUI's panel color
            let hc = d.name.starts_with("high-contrast");
            let text = if hc { 7.0 } else { 4.5 };
            let checks = [
                ("fg/bg", contrast(fg, bg), text),
                ("fg/surface", contrast(fg, surface), text),
                ("muted/bg", contrast(muted, bg), text),
                ("muted/surface", contrast(muted, surface), text),
                ("selection", contrast(sel_fg, sel_bg), text),
                ("accent/bg", contrast(accent, bg), 3.0),
                ("border/bg", contrast(border, bg), if hc { 3.0 } else { 0.0 }),
            ];
            for (what, got, min) in checks {
                if got < min {
                    bad.push(format!("{} {what}: {got:.2} < {min}", d.name));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
        let names: Vec<String> = builtin().into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"high-contrast".into()) && names.contains(&"high-contrast-light".into()), "{names:?}");
        assert_eq!(effective_name("auto", true), "high-contrast");
        assert_eq!(effective_name("auto", false), "terminal");
        assert_eq!(effective_name("nord", true), "nord", "a chosen theme stays");
    }
}
