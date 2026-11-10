use std::collections::BTreeMap;

use termax_ui::color::Color;
use termax_ui::display::{Prim, TileRenderer};
use termax_ui::geom::{Rect, Size};
use termax_ui::input::{Event, Key, Modifiers};
use termax_ui::text::cache::GlyphCache;
use termax_ui::ui::testing::Harness;
use termax_ui::ui::Id;

use super::input::route;
use super::render::{draw_screen, screen_rows};
use super::*;
use crate::keys::Keymap;
use crate::theme::{from_def, xterm_256};

fn theme() -> GuiTheme {
    from_def(&termax_core::theme::builtin().into_iter().find(|t| t.name == "midnight").unwrap())
}

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };
const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, alt: false, logo: false };
const ALT: Modifiers = Modifiers { ctrl: false, shift: false, alt: true, logo: false };
const ALTGR: Modifiers = Modifiers { ctrl: true, shift: false, alt: true, logo: false };

fn kd(key: Key, mods: Modifiers, text: Option<&str>) -> Event {
    Event::KeyDown { key, mods, text: text.map(String::from) }
}

#[test]
fn screen_rows_colors_and_attrs() {
    let t = theme();
    let mut p = vt100::Parser::new(4, 10, 0);
    p.process("\x1b[31mR\x1b[1;38;5;196mX\x1b[7mI\x1b[0m日".as_bytes());
    let rows = screen_rows(p.screen(), &t);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].len(), 10);
    let r = rows[0][0];
    assert_eq!((r.ch, r.fg, r.bg), ('R', t.ansi[1], Color::TRANSPARENT));
    let x = rows[0][1];
    assert_eq!((x.ch, x.fg), ('X', xterm_256(196)));
    assert!(x.attrs.bold);
    let i = rows[0][2];
    assert_eq!((i.ch, i.fg, i.bg), ('I', t.term_bg, xterm_256(196)), "inverse swaps");
    assert!(rows[0][3].attrs.wide);
    assert_eq!(rows[0][3].ch, '日');
    assert_eq!(rows[0][4].ch, '\0', "wide continuation");
    assert_eq!(rows[0][5].fg, t.term_fg, "reset to default fg");
}

#[test]
fn bold_brightens_basic_colors() {
    let t = theme();
    let mut p = vt100::Parser::new(2, 10, 0);
    p.process(b"\x1b[1;32mG");
    assert_eq!(screen_rows(p.screen(), &t)[0][0].fg, t.ansi[10]);
}

#[test]
fn keys_encode_to_pty() {
    let (km, _) = Keymap::new(&BTreeMap::new(), false);
    let r = |evs: Vec<Event>, app: bool| route(&evs, &km, app);
    assert_eq!(r(vec![kd(Key::Char('c'), CTRL, None)], false), vec![0x03]);
    assert_eq!(r(vec![kd(Key::Up, Modifiers::NONE, None)], true), b"\x1bOA".to_vec());
    assert_eq!(r(vec![kd(Key::Up, Modifiers::NONE, None)], false), b"\x1b[A".to_vec());
    assert!(r(vec![kd(Key::Char('c'), CTRL_SHIFT, None)], false).is_empty(), "app shortcut never reaches the shell");
    // a printable key arrives as KeyDown + Text: sent once
    assert_eq!(r(vec![kd(Key::Char('a'), Modifiers::NONE, Some("a")), Event::Text("a".into())], false), b"a".to_vec());
    // Alt+x: ESC x, and the Text that follows is not sent again
    assert_eq!(r(vec![kd(Key::Char('x'), ALT, Some("x")), Event::Text("x".into())], false), b"\x1bx".to_vec());
    // AltGr (Ctrl+Alt) produces text, e.g. '@' on German layouts
    assert_eq!(r(vec![kd(Key::Char('q'), ALTGR, Some("@")), Event::Text("@".into())], false), b"@".to_vec());
    assert_eq!(r(vec![kd(Key::Enter, Modifiers::NONE, None)], false), b"\r".to_vec());
    assert_eq!(r(vec![kd(Key::Tab, Modifiers { shift: true, ..Modifiers::NONE }, None)], false), b"\x1b[Z".to_vec());
    assert_eq!(r(vec![Event::Text("日本".into())], false), "日本".as_bytes().to_vec());
    assert_eq!(r(vec![kd(Key::Backspace, Modifiers::NONE, None)], false), vec![0x7f]);
}

#[test]
fn grid_clamps_tiny_areas() {
    assert_eq!(grid_for(Rect::new(0, 0, 0, 0), Size::new(8, 16)), (2, 10));
    assert_eq!(grid_for(Rect::new(0, 0, 30, 20), Size::new(8, 16)), (2, 10));
    assert_eq!(grid_for(Rect::new(0, 0, 800, 480), Size::new(8, 16)), (30, 100));
}

fn fixture_tab(fx: &termax_testserver::TestFixture) -> TermTab {
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    // A label only (never reads ~/.ssh/config): the command talks to the local fixture.
    let cfg = termax_core::config::Config::parse("[settings]\nload_ssh_config=false\n[[connection]]\nname=\"fx\"\nhost=\"127.0.0.1\"\n").unwrap();
    let host = termax_core::hosts::collect(&cfg).0.remove(0);
    TermTab::with_command(host, "ssh".into(), args, 100)
}

#[test]
fn terminal_view_tiny_area_no_panic() {
    let fx = termax_testserver::TestFixture::with_shell(termax_testserver::FixtureAuth::Key).unwrap();
    let mut tab = fixture_tab(&fx);
    let (km, _) = Keymap::new(&BTreeMap::new(), false);
    let t = theme();
    for (w, h) in [(30, 20), (0, 0), (5, 3)] {
        let mut harness = Harness::new(w, h, 1.0);
        harness.frame(vec![], &mut |ui| view(ui, Id::new("t"), ui.window_rect(), &mut tab, &t, &km, None, &mut vec![]));
        harness.render();
        assert!(tab.grid.0 >= 2 && tab.grid.1 >= 10, "{:?}", tab.grid);
    }
    assert!(tab.session.is_some(), "spawned once the grid was known: {:?}", tab.error);
}

#[test]
fn resize_only_on_cell_change() {
    let fx = termax_testserver::TestFixture::with_shell(termax_testserver::FixtureAuth::Key).unwrap();
    let mut tab = fixture_tab(&fx);
    let (km, _) = Keymap::new(&BTreeMap::new(), false);
    let t = theme();
    let mut h = Harness::new(800, 600, 1.0);
    let frame = |h: &mut Harness, tab: &mut TermTab| {
        h.frame(vec![], &mut |ui| view(ui, Id::new("t"), ui.window_rect(), tab, &t, &km, None, &mut vec![]));
    };
    frame(&mut h, &mut tab);
    frame(&mut h, &mut tab);
    assert_eq!(tab.resizes, 0, "spawned at the right size, no resize yet");
    h.resize(803, 600); // less than a cell wider
    frame(&mut h, &mut tab);
    assert_eq!(tab.resizes, 0);
    h.resize(1000, 600);
    frame(&mut h, &mut tab);
    assert_eq!(tab.resizes, 1);
}

#[test]
fn only_changed_row_damaged() {
    let t = theme();
    let mut p = vt100::Parser::new(24, 80, 0);
    p.process(b"hello\r\nworld");
    let mut h = Harness::new(800, 24 * 16, 1.0);
    let mut renderer = TileRenderer::default();
    let mut cache = GlyphCache::default();
    let mut canvas = termax_ui::canvas::Canvas::new(800, 24 * 16);
    let mut paint = |h: &mut Harness, p: &vt100::Parser| {
        h.frame(vec![], &mut |ui| draw_screen(ui, ui.window_rect(), p.screen(), &t, Size::new(8, 16), 14.0, None, None));
        renderer.render(h.list(), &mut canvas, &h.fonts, &mut cache, Color::BLACK)
    };
    paint(&mut h, &p);
    p.process(b"\x1b[10;1Hchanged");
    let damage = paint(&mut h, &p);
    assert!(!damage.is_empty());
    let band = Rect::new(0, 9 * 16, 800, 16);
    for d in &damage {
        assert!(d.y <= band.y && d.bottom() >= band.bottom() && d.h <= 128, "{d:?} outside row 10's tiles");
    }
    assert!(h.list().prims.iter().filter(|p| matches!(p, Prim::CellRow { .. })).count() == 24);
}
