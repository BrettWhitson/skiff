use super::*;
use termax_core::theme::builtin;

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

#[test]
fn xterm_256_cube_and_grey() {
    assert_eq!(xterm_256(16), Color::hex(0x000000));
    assert_eq!(xterm_256(196), Color::hex(0xff0000));
    assert_eq!(xterm_256(244), Color::hex(0x808080));
    assert_eq!(xterm_256(231), Color::hex(0xffffff));
}

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

#[test]
fn every_builtin_maps() {
    for d in builtin() {
        let t = from_def(&d);
        assert_eq!(t.palette.bg.a, 255, "{}", d.name);
    }
}
