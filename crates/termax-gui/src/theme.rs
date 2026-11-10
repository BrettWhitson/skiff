//! Turns core [`ThemeDef`]s into toolkit colours.

use termax_core::theme::{Rgb, ThemeDef, contrast, parse_color};
use termax_ui::color::Color;
use termax_ui::style::Palette;

/// Concrete colours for `reset` (the TUI's "use the terminal's colour").
const RESET_BG: Color = Color::hex(0x1e1f24);
const RESET_FG: Color = Color::hex(0xd8dae0);

#[derive(Debug, Clone, PartialEq)]
pub struct GuiTheme {
    pub name: String,
    pub palette: Palette,
    /// Terminal colours 0-15.
    pub ansi: [Color; 16],
    pub term_fg: Color,
    pub term_bg: Color,
}

/// The standard xterm 256-colour palette.
pub fn xterm_256(n: u8) -> Color {
    const BASE: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff,
        0xff00ff, 0x00ffff, 0xffffff,
    ];
    match n {
        0..=15 => Color::hex(BASE[n as usize]),
        16..=231 => {
            let i = n - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color::rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (n - 232) * 10;
            Color::rgb(v, v, v)
        }
    }
}

/// Resolves a parsed colour against a terminal palette; `None` for `reset`.
pub fn resolve(c: Rgb, ansi: &[Color; 16]) -> Option<Color> {
    match c {
        Rgb::Reset => None,
        Rgb::Rgb(r, g, b) => Some(Color::rgb(r, g, b)),
        Rgb::Indexed(i) if i < 16 => Some(ansi[i as usize]),
        Rgb::Indexed(i) => Some(xterm_256(i)),
    }
}

pub fn from_def(d: &ThemeDef) -> GuiTheme {
    let mut ansi: [Color; 16] = std::array::from_fn(|i| xterm_256(i as u8));
    if let Some(list) = &d.ansi {
        for (slot, s) in ansi.iter_mut().zip(list) {
            if let Some(Rgb::Rgb(r, g, b)) = parse_color(s) {
                *slot = Color::rgb(r, g, b);
            }
        }
    }
    let get = |s: &str, reset: Color| parse_color(s).and_then(|c| resolve(c, &ansi)).unwrap_or(reset);
    let bg = get(&d.bg, RESET_BG);
    let fg = get(&d.fg, RESET_FG);
    let palette = Palette {
        bg,
        surface: bg.mix(fg, 0.04),
        surface_alt: bg.mix(fg, 0.10),
        fg,
        muted: get(&d.muted, bg.mix(fg, 0.5)),
        accent: get(&d.accent, fg),
        accent_fg: bg,
        border: get(&d.border, bg.mix(fg, 0.2)),
        control: Color::BLACK, // set below
        selection: get(&d.selection_bg, bg.mix(fg, 0.25)),
        danger: get(&d.error, Color::hex(0xe5534b)),
        success: get(&d.success, Color::hex(0x57ab5a)),
        warning: get(&d.warning, Color::hex(0xc69026)),
    };
    let mut palette = palette;
    palette.control = control_outline(&palette);
    make_readable(&mut palette, if d.name.starts_with("high-contrast") { 7.0 } else { 4.5 });
    GuiTheme { name: d.name.clone(), palette, ansi, term_fg: fg, term_bg: bg }
}

/// How much dimmer than `fg` muted text stays (a contrast ratio).
pub const MUTED_STEP: f32 = 1.3;

/// WCAG contrast of two toolkit colours.
fn ratio(a: Color, b: Color) -> f32 {
    contrast((a.r, a.g, a.b), (b.r, b.g, b.b))
}

/// `c` moved toward `toward` in 5% steps until `ok`; `toward` if never.
fn nudge(c: Color, toward: Color, ok: impl Fn(Color) -> bool) -> Color {
    (0..=20).map(|i| c.mix(toward, i as f32 / 20.0)).find(|&c| ok(c)).unwrap_or(toward)
}

/// Adjusts a theme so text reaches `min`:1 wherever the widgets draw it:
/// rows at rest, hovered and selected (with and without focus). The
/// selection moves toward `bg` until `fg` and `muted` read on it and the
/// accent (focus ring, focused row bar) stands out 3:1 from it; `muted` and
/// the status colours move toward `fg` until they read on every background.
fn make_readable(p: &mut Palette, min: f32) {
    let surface = p.surface;
    let unfocused = |s: Color| s.mix(surface, 0.5);
    let (fg, accent) = (p.fg, p.accent);
    let rest = [p.bg, p.surface, p.surface_alt];
    let muted = nudge(p.muted, fg, |c| rest.iter().all(|&r| ratio(c, r) >= min));
    // The brightest `muted` may get while still looking dimmer than `fg`.
    let brightest = (0..=20).map(|i| muted.mix(fg, i as f32 / 20.0)).take_while(|&c| ratio(c, fg) >= MUTED_STEP).last().unwrap_or(muted);
    let reads = |s: Color| [fg, brightest].iter().all(|&t| ratio(t, s) >= min && ratio(t, unfocused(s)) >= min);
    p.selection = nudge(p.selection, p.bg, |s| reads(s) && ratio(accent, s) >= 3.0);
    let rows = [p.bg, p.surface, p.surface_alt, p.selection, unfocused(p.selection)];
    p.muted = nudge(muted, fg, |c| rows.iter().all(|&r| ratio(c, r) >= min));
    for c in [&mut p.danger, &mut p.warning, &mut p.success] {
        *c = nudge(*c, fg, |c| rows[..3].iter().all(|&r| ratio(c, r) >= min));
    }
}

/// The theme's border, moved toward `fg` until it stands out 3:1 from the
/// window and panel backgrounds.
fn control_outline(p: &Palette) -> Color {
    let rgb = |c: Color| (c.r, c.g, c.b);
    let ok = |c: Color| contrast(rgb(c), rgb(p.bg)) >= 3.0 && contrast(rgb(c), rgb(p.surface)) >= 3.0;
    (0..=20).map(|i| p.border.mix(p.fg, i as f32 / 20.0)).find(|&c| ok(c)).unwrap_or(p.fg)
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
