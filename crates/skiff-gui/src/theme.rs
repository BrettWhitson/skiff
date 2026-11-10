//! Turns core [`ThemeDef`]s into toolkit colours: a widget [`Palette`] plus
//! the terminal's 16 ANSI colours, nudged where needed so text meets WCAG
//! contrast targets.

use skiff_core::theme::{Rgb, ThemeDef, contrast, parse_color};
use skiff_ui::color::Color;
use skiff_ui::style::Palette;

/// Concrete background for `reset` (the TUI's "use the terminal's colour").
const RESET_BG: Color = Color::hex(0x1e1f24);
/// Foreground counterpart of `RESET_BG`.
const RESET_FG: Color = Color::hex(0xd8dae0);

/// A theme ready to draw with: widget palette and terminal colours.
#[derive(Debug, Clone, PartialEq)]
pub struct GuiTheme {
    /// The theme's name from its definition.
    pub name: String,
    /// Widget colours, already adjusted for contrast.
    pub palette: Palette,
    /// Terminal colours 0-15.
    pub ansi: [Color; 16],
    /// Terminal default foreground (the theme's unadjusted `fg`).
    pub term_fg: Color,
    /// Terminal default background (the theme's `bg`).
    pub term_bg: Color,
    /// Why the theme can't meet its contrast targets (a custom theme whose
    /// colours are too close); shown once when it is applied.
    pub warning: Option<String>,
}

/// The standard xterm 256-colour palette.
pub fn xterm_256(n: u8) -> Color {
    const BASE: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff,
        0xff00ff, 0x00ffff, 0xffffff,
    ];
    match n {
        // 6x6x6 colour cube for 16-231, a 24-step grey ramp for 232-255.
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

/// Builds a [`GuiTheme`] from a theme definition: unset or unparsable colours
/// fall back to defaults derived from `bg`/`fg`, then the palette is made
/// readable (7:1 for `high-contrast*` themes, else 4.5:1) and `warning` is
/// set if it can't be.
pub fn from_def(d: &ThemeDef) -> GuiTheme {
    let mut ansi: [Color; 16] = std::array::from_fn(|i| xterm_256(i as u8));
    // Custom ANSI entries override the xterm defaults per slot; only literal
    // RGB values count, anything else keeps the default.
    if let Some(list) = &d.ansi {
        for (slot, s) in ansi.iter_mut().zip(list) {
            if let Some(Rgb::Rgb(r, g, b)) = parse_color(s) {
                *slot = Color::rgb(r, g, b);
            }
        }
    }
    // `reset` / unparsable values use the given fallback.
    let get = |s: &str, reset: Color| parse_color(s).and_then(|c| resolve(c, &ansi)).unwrap_or(reset);
    let bg = get(&d.bg, RESET_BG);
    let fg = get(&d.fg, RESET_FG);
    let accent = get(&d.accent, fg);
    let palette = Palette {
        bg,
        surface: bg.mix(fg, 0.04),
        surface_alt: bg.mix(fg, 0.10),
        overlay: bg.mix(fg, 0.07),
        fg,
        muted: get(&d.muted, bg.mix(fg, 0.5)),
        text_disabled: bg.mix(fg, 0.4),
        accent,
        accent_fg: bg,
        focus: accent,
        border: get(&d.border, bg.mix(fg, 0.2)),
        control: Color::BLACK, // set below
        selection: get(&d.selection_bg, bg.mix(fg, 0.25)),
        hover: bg.mix(fg, 0.07),
        danger: get(&d.error, Color::hex(0xe5534b)),
        success: get(&d.success, Color::hex(0x57ab5a)),
        warning: get(&d.warning, Color::hex(0xc69026)),
        // Bright blue from the theme's own terminal palette.
        info: ansi[12],
        // Darker over dark themes, lighter touch over light ones.
        scrim: Color::rgba(0, 0, 0, if bg.luminance() < 0.2 { 150 } else { 100 }),
    };
    let mut palette = palette;
    palette.control = control_outline(&palette);
    let min = if d.name.starts_with("high-contrast") { 7.0 } else { 4.5 };
    let warning = (!make_readable(&mut palette, min))
        .then(|| format!("theme {}: its colours are too close for {min}:1 text contrast; some text may be hard to read", d.name));
    GuiTheme { name: d.name.clone(), palette, ansi, term_fg: fg, term_bg: bg, warning }
}

/// How much dimmer than `fg` muted text stays (a contrast ratio).
pub const MUTED_STEP: f32 = 1.3;

/// The least contrast that still tells muted text from `fg`, and the
/// selection from `bg`, when a theme can't have more.
pub const MIN_VISIBLE: f32 = 1.15;

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
/// Muted text stays visibly dimmer than `fg` and the selection visibly apart
/// from `bg` even when that costs contrast; false when the targets can't all
/// be met.
fn make_readable(p: &mut Palette, min: f32) -> bool {
    let surface = p.surface;
    let unfocused = |s: Color| s.mix(surface, 0.5);
    // Primary-button labels (`accent_fg` on the accent) read at `min`.
    let away = if p.accent_fg.contrast(p.fg) > p.accent_fg.contrast(p.bg) { p.fg } else { p.bg };
    p.accent = nudge(p.accent, away, |a| ratio(p.accent_fg, a) >= min);
    let (fg, accent) = (p.fg, p.accent);
    let rest = [p.bg, p.surface, p.surface_alt, p.overlay, p.hover];
    // First pass: muted readable on the resting backgrounds only.
    let muted = nudge(p.muted, fg, |c| rest.iter().all(|&r| ratio(c, r) >= min));
    // The brightest `muted` may get while still looking dimmer than `fg`.
    let brightest = (0..=20).map(|i| muted.mix(fg, i as f32 / 20.0)).take_while(|&c| ratio(c, fg) >= MUTED_STEP).last().unwrap_or(muted);
    let reads = |s: Color| [fg, brightest].iter().all(|&t| ratio(t, s) >= min && ratio(t, unfocused(s)) >= min);
    p.selection = nudge(p.selection, p.bg, |s| reads(s) && ratio(accent, s) >= 3.0);
    // Couldn't find one: settle for a selection just visibly apart from bg.
    if ratio(p.selection, p.bg) < MIN_VISIBLE {
        p.selection = nudge(p.bg, fg, |s| ratio(s, p.bg) >= MIN_VISIBLE);
    }
    // Second pass: muted readable on every row state, now the selection is fixed.
    let rows = [p.bg, p.surface, p.surface_alt, p.overlay, p.hover, p.selection, unfocused(p.selection)];
    p.muted = nudge(muted, fg, |c| rows.iter().all(|&r| ratio(c, r) >= min));
    if ratio(p.muted, fg) < MIN_VISIBLE {
        p.muted = nudge(fg, p.bg, |c| ratio(c, fg) >= MIN_VISIBLE);
    }
    // Status colours read as text on the non-selected backgrounds, and on
    // their own badge tint over each of them.
    let plain = &rows[..5];
    for c in [&mut p.danger, &mut p.warning, &mut p.success, &mut p.info] {
        *c = nudge(*c, fg, |c| plain.iter().all(|&r| ratio(c, r) >= min && ratio(c, r.mix(c, skiff_ui::style::TINT)) >= min));
    }
    // Disabled text and the focus ring: 3:1 (non-text contrast) everywhere.
    p.text_disabled = nudge(p.text_disabled, fg, |c| plain.iter().all(|&r| ratio(c, r) >= 3.0));
    let focus_on = [plain, &[p.selection]].concat();
    p.focus = nudge(p.focus, fg, |c| focus_on.iter().all(|&r| ratio(c, r) >= 3.0));
    let text_reads = [fg, p.muted].iter().all(|&t| rows.iter().all(|&r| ratio(t, r) >= min));
    text_reads && ratio(p.muted, fg) >= MIN_VISIBLE && ratio(p.selection, p.bg) >= MIN_VISIBLE
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
