//! Tests for `theme`: colour mapping, fallbacks and contrast adjustment.

use super::*;
use skiff_core::theme::builtin;

/// `reset` colours become the concrete defaults and named ANSI colours
/// resolve through the theme's own palette.
#[test]
fn theme_reset_gets_concrete_colors() {
    let term = builtin().into_iter().find(|t| t.name == "terminal").unwrap();
    let t = from_def(&term);
    assert_eq!(t.palette.bg, Color::hex(0x1e1f24));
    assert_eq!(t.palette.fg, Color::hex(0xd8dae0));
    assert_eq!(t.term_bg, t.palette.bg);
    // "cyan" is ANSI 6, resolved through the theme's own palette
    assert_eq!(t.palette.accent, Color::hex(0x11a8cd));
}

/// Spot checks of the colour cube and grey ramp against xterm's values.
#[test]
fn xterm_256_cube_and_grey() {
    assert_eq!(xterm_256(16), Color::hex(0x000000));
    assert_eq!(xterm_256(196), Color::hex(0xff0000));
    assert_eq!(xterm_256(244), Color::hex(0x808080));
    assert_eq!(xterm_256(231), Color::hex(0xffffff));
}

/// A missing ANSI list, or an unparsable entry, uses the xterm default colour.
#[test]
fn missing_ansi_falls_back_to_default_palette() {
    let mut d = builtin().into_iter().find(|t| t.name == "nord").unwrap();
    d.ansi = None;
    let t = from_def(&d);
    assert_eq!(t.ansi[1], xterm_256(1));
    // a bad entry also falls back, per slot
    d.ansi = Some(vec!["nope".into(); 16]);
    assert_eq!(from_def(&d).ansi[4], xterm_256(4));
}

/// Every built-in theme maps to an opaque palette without a contrast warning.
#[test]
fn every_builtin_maps() {
    for d in builtin() {
        let t = from_def(&d);
        assert_eq!(t.palette.bg.a, 255, "{}", d.name);
        assert_eq!(t.warning, None, "{}", d.name);
    }
}

/// Every text color on every background it is drawn on (rows at rest,
/// hovered, selected with and without focus; overlays; every button
/// variant; badge tints), WCAG 2.1 ratios: 4.5:1 for text (7:1 in the
/// high-contrast themes) and 3:1 for outlines, disabled text, the focus
/// ring and the focused row's bar.
#[test]
fn builtin_palettes_meet_contrast() {
    use skiff_core::theme::contrast;
    let rgb = |c: Color| (c.r, c.g, c.b);
    let mut bad = vec![];
    for d in builtin() {
        let p = from_def(&d).palette;
        let text = if d.name.starts_with("high-contrast") { 7.0 } else { 4.5 };
        let rows = [
            ("bg", p.bg),
            ("surface", p.surface),
            ("hover", p.surface_alt),
            ("row-hover", p.hover),
            ("overlay", p.overlay),
            ("selected", p.selection),
            ("selected-unfocused", p.selection.mix(p.surface, 0.5)),
        ];
        let mut checks = vec![];
        for (on, bg) in rows {
            checks.push((format!("fg/{on}"), contrast(rgb(p.fg), rgb(bg)), text));
            checks.push((format!("muted/{on}"), contrast(rgb(p.muted), rgb(bg)), text));
        }
        let plain = [("bg", p.bg), ("surface", p.surface), ("hover", p.surface_alt), ("row-hover", p.hover), ("overlay", p.overlay)];
        for (on, bg) in plain {
            for (what, c) in [("danger", p.danger), ("warning", p.warning), ("success", p.success), ("info", p.info)] {
                checks.push((format!("{what}/{on}"), contrast(rgb(c), rgb(bg)), text));
                checks.push((format!("{what}-badge/{on}"), contrast(rgb(c), rgb(p.tint(c, bg))), text));
            }
            checks.push((format!("disabled/{on}"), contrast(rgb(p.text_disabled), rgb(bg)), 3.0));
            checks.push((format!("focus/{on}"), contrast(rgb(p.focus), rgb(bg)), 3.0));
        }
        checks.push(("focus/selected".into(), contrast(rgb(p.focus), rgb(p.selection)), 3.0));
        for (on, bg) in skiff_ui::widgets::button_backgrounds(&p) {
            checks.push((format!("fg/button-{on}"), contrast(rgb(p.fg), rgb(bg)), text));
        }
        use skiff_ui::widgets::{ButtonKind, button_colors};
        for kind in [ButtonKind::Primary, ButtonKind::Danger] {
            for (state, bg, label) in button_colors(&p, kind) {
                checks.push((format!("{kind:?}-{state}"), contrast(rgb(label), rgb(bg)), text));
            }
        }
        for (on, bg) in [("bg", p.bg), ("surface", p.surface)] {
            checks.push((format!("control/{on}"), contrast(rgb(p.control), rgb(bg)), 3.0));
            checks.push((format!("accent/{on}"), contrast(rgb(p.accent), rgb(bg)), 3.0));
        }
        checks.push(("accent/selected".into(), contrast(rgb(p.accent), rgb(p.selection)), 3.0));
        // Dimmed text still looks dimmer than normal text.
        let step = MUTED_STEP.min(contrast(rgb(p.fg), rgb(p.bg)) / text);
        checks.push(("fg/muted".into(), contrast(rgb(p.fg), rgb(p.muted)), step));
        for (what, got, min) in checks {
            if got < min {
                bad.push(format!("{} {what}: {got:.2} < {min}", d.name));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// A theme whose colours are nearly identical still gets distinguishable
/// muted text and selection, and a warning naming the theme.
#[test]
fn unreadable_custom_theme_keeps_muted_and_selection_visible() {
    let mut d = builtin().into_iter().find(|t| t.name == "nord").unwrap();
    (d.bg, d.fg, d.muted, d.selection_bg) = ("#777777".into(), "#888888".into(), "#808080".into(), "#777777".into());
    let t = from_def(&d);
    let p = t.palette;
    assert!(ratio(p.muted, p.fg) >= MIN_VISIBLE, "muted {:?} vs fg {:?}", p.muted, p.fg);
    assert!(ratio(p.selection, p.bg) >= MIN_VISIBLE, "selection {:?} vs bg {:?}", p.selection, p.bg);
    assert!(t.warning.as_deref().is_some_and(|w| w.contains("nord")), "{:?}", t.warning);
}

/// On light themes the terminal's ANSI colours are darkened until they
/// read on the background (4.5:1 colours, 3:1 greys); dark themes keep theirs.
#[test]
fn light_theme_ansi_colors_read_on_background() {
    use skiff_core::theme::contrast;
    let rgb = |c: Color| (c.r, c.g, c.b);
    let paper = from_def(&builtin().into_iter().find(|t| t.name == "paper").unwrap());
    for (i, c) in paper.ansi.iter().enumerate().skip(1) {
        let min = if matches!(i, 7 | 8 | 15) { 3.0 } else { 4.5 };
        assert!(contrast(rgb(*c), rgb(paper.term_bg)) >= min, "paper ansi {i} {c:?}");
    }
    let nord = builtin().into_iter().find(|t| t.name == "nord").unwrap();
    let list: Vec<Color> = nord
        .ansi
        .clone()
        .unwrap()
        .iter()
        .map(|s| match skiff_core::theme::parse_color(s) {
            Some(skiff_core::theme::Rgb::Rgb(r, g, b)) => Color::rgb(r, g, b),
            _ => panic!("{s}"),
        })
        .collect();
    assert_eq!(from_def(&nord).ansi.to_vec(), list, "dark themes unchanged");
}
