use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use termax_ui::geom::{Point, Rect, Size};
use termax_ui::input::{Event, Key, Modifiers, MouseButton};
use termax_ui::ui::Id;
use termax_ui::ui::testing::Harness;

use super::select::{SelectState, handle_mouse, selection_text};
use super::*;
use crate::keys::{Action, Keymap};
use crate::theme::from_def;

const CELL: Size = Size::new(8, 16);

fn parser(text: &str) -> vt100::Parser {
    let mut p = vt100::Parser::new(5, 30, 0);
    p.process(text.as_bytes());
    p
}

/// Centre of cell (row, col) inside a view at the origin.
fn at(row: i32, col: i32) -> Point {
    Point::new(col * CELL.w + 4, row * CELL.h + 8)
}

fn mouse(h: &mut Harness, st: &mut SelectState, events: Vec<Event>) {
    h.frame(events, &mut |ui| {
        handle_mouse(ui, Id::new("t"), Rect::new(0, 0, 240, 80), CELL, st, 0);
    });
}

fn press(p: Point) -> Vec<Event> {
    vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p)]
}

fn release(p: Point) -> Vec<Event> {
    vec![Event::MouseUp(MouseButton::Left, p)]
}

#[test]
fn drag_selects_and_copies_text() {
    let p = parser("hello world\r\nsecond line");
    let mut h = Harness::new(300, 100, 1.0);
    let mut st = SelectState::default();
    mouse(&mut h, &mut st, vec![]);
    mouse(&mut h, &mut st, press(at(0, 6)));
    mouse(&mut h, &mut st, vec![Event::MouseMove(at(1, 5))]);
    mouse(&mut h, &mut st, release(at(1, 5)));
    let sel = st.sel.expect("selection");
    assert_eq!(selection_text(p.screen(), &sel, 0), "world\nsecond");
}

#[test]
fn click_without_drag_clears_selection() {
    let mut h = Harness::new(300, 100, 1.0);
    let mut st = SelectState::default();
    mouse(&mut h, &mut st, vec![]);
    mouse(&mut h, &mut st, press(at(0, 0)));
    mouse(&mut h, &mut st, vec![Event::MouseMove(at(0, 4))]);
    mouse(&mut h, &mut st, release(at(0, 4)));
    assert!(st.sel.is_some());
    std::thread::sleep(Duration::from_millis(500)); // not a double click
    mouse(&mut h, &mut st, press(at(2, 2)));
    mouse(&mut h, &mut st, release(at(2, 2)));
    assert!(st.sel.is_none());
}

#[test]
fn double_click_selects_word() {
    let p = parser("cd /var/log/nginx now");
    let mut h = Harness::new(300, 100, 1.0);
    let mut st = SelectState::default();
    mouse(&mut h, &mut st, vec![]);
    for _ in 0..2 {
        mouse(&mut h, &mut st, press(at(0, 8)));
        mouse(&mut h, &mut st, release(at(0, 8)));
    }
    st.resolve(p.screen(), 0);
    assert_eq!(selection_text(p.screen(), &st.sel.unwrap(), 0), "/var/log/nginx");
}

#[test]
fn triple_click_selects_line() {
    let p = parser("first\r\n  the whole line  \r\nlast");
    let mut h = Harness::new(300, 100, 1.0);
    let mut st = SelectState::default();
    mouse(&mut h, &mut st, vec![]);
    for _ in 0..3 {
        mouse(&mut h, &mut st, press(at(1, 3)));
        mouse(&mut h, &mut st, release(at(1, 3)));
    }
    st.resolve(p.screen(), 0);
    assert_eq!(selection_text(p.screen(), &st.sel.unwrap(), 0), "  the whole line");
}

// ----- with a live session on the local test server ----------------------

fn fixture_tab(fx: &termax_testserver::TestFixture) -> TermTab {
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let cfg = termax_core::config::Config::parse("[settings]\nload_ssh_config=false\n[[connection]]\nname=\"fx\"\nhost=\"127.0.0.1\"\n").unwrap();
    let host = termax_core::hosts::collect(&cfg).0.remove(0);
    TermTab::with_command(host, "ssh".into(), args, 500)
}

struct Live {
    _fx: termax_testserver::TestFixture,
    tab: TermTab,
    h: Harness,
    km: Keymap,
    theme: GuiTheme,
}

impl Live {
    fn new() -> Live {
        Self::with_harness(Harness::new(800, 400, 1.0))
    }

    fn with_harness(h: Harness) -> Live {
        let fx = termax_testserver::TestFixture::with_shell(termax_testserver::FixtureAuth::Key).unwrap();
        let tab = fixture_tab(&fx);
        let theme = from_def(&termax_core::theme::builtin()[1]);
        let mut l = Live { _fx: fx, tab, h, km: Keymap::new(&BTreeMap::new(), false).0, theme };
        l.frame(vec![], vec![]);
        // focus the view
        l.frame(vec![Event::MouseMove(Point::new(400, 200)), Event::MouseDown(MouseButton::Left, Point::new(400, 200)), Event::MouseUp(MouseButton::Left, Point::new(400, 200))], vec![]);
        l.wait_for("termax-testserver shell");
        l
    }

    fn frame(&mut self, events: Vec<Event>, mut actions: Vec<Action>) {
        let (tab, km, theme) = (&mut self.tab, &self.km, &self.theme);
        self.h.frame(events, &mut |ui| view(ui, Id::new("t"), ui.window_rect(), tab, theme, km, None, &mut actions));
    }

    fn screen_text(&self) -> String {
        self.tab.session.as_ref().unwrap().parser.lock().unwrap().screen().contents()
    }

    fn wait_for(&mut self, s: &str) {
        let start = Instant::now();
        while !self.screen_text().contains(s) {
            assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for {s:?}; screen: {}", self.screen_text());
            std::thread::sleep(Duration::from_millis(30));
        }
        self.frame(vec![], vec![]);
    }
}

#[test]
fn copy_and_paste_through_clipboard() {
    let mut l = Live::new();
    l.h.clipboard.text = Some("pasted-text".into());
    l.frame(vec![], vec![Action::Paste]);
    l.wait_for("pasted-text");
    // select "pasted" on the line it was echoed on, then copy
    let row = l.screen_text().lines().position(|x| x.contains("pasted-text")).unwrap() as i32;
    let (cw, chh) = l.tab.cell_size();
    let pt = |c: i32| Point::new(6 + c * cw + cw / 2, 6 + row * chh + chh / 2);
    l.frame(press(pt(0)), vec![]);
    l.frame(vec![Event::MouseMove(pt(5))], vec![]);
    l.frame(release(pt(5)), vec![]);
    l.frame(vec![], vec![Action::Copy]);
    assert_eq!(l.h.clipboard.text.as_deref(), Some("pasted"));
}

#[test]
fn wheel_scrolls_and_key_snaps_back() {
    let mut l = Live::new();
    l.tab.confirm_paste = false; // the multi-line paste is just test setup
    let lines: String = (0..60).map(|i| format!("line{i}\r")).collect();
    l.h.clipboard.text = Some(lines);
    l.frame(vec![], vec![Action::Paste]);
    l.wait_for("line59");
    l.frame(vec![Event::MouseMove(Point::new(400, 200)), Event::Wheel { dx: 0.0, dy: 48.0 }], vec![]);
    assert_eq!(l.tab.session.as_ref().unwrap().scroll_offset(), 3, "one notch = 3 lines back");
    l.frame(vec![Event::KeyDown { key: Key::Char('a'), mods: Modifiers::NONE, text: Some("a".into()) }, Event::Text("a".into())], vec![]);
    assert_eq!(l.tab.session.as_ref().unwrap().scroll_offset(), 0);
}

#[test]
fn zoom_changes_grid_size() {
    let fonts = termax_ui::text::Fonts::load(None, None).expect("system fonts");
    let mut l = Live::with_harness(Harness::with_fonts(800, 400, 1.0, fonts));
    let before = l.tab.grid;
    l.frame(vec![], vec![Action::ZoomIn]);
    l.frame(vec![], vec![Action::ZoomIn]);
    l.frame(vec![], vec![]);
    assert!(l.tab.grid.1 < before.1, "{before:?} -> {:?}", l.tab.grid);
    l.frame(vec![], vec![Action::ZoomReset]);
    l.frame(vec![], vec![]);
    assert_eq!(l.tab.grid, before);
}

#[test]
fn shown_again_does_not_reannounce_screen() {
    let mut l = Live::new();
    let start = Instant::now();
    while l.tab.announcer.has_pending() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(100));
        l.frame(vec![], vec![]);
    }
    // Output arrives while the tab is in the background (not drawn).
    l.tab.session.as_mut().unwrap().write(b"while-hidden\r");
    let start = Instant::now();
    while !l.screen_text().contains("while-hidden") {
        assert!(start.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(30));
    }
    l.tab.shown();
    l.frame(vec![], vec![]);
    assert!(!l.tab.announcer.has_pending(), "switching to a tab reads nothing old");
}

fn on_screen(l: &Live, s: &str) -> bool {
    l.h.list().prims.iter().any(|p| matches!(p, termax_ui::display::Prim::Text { text, .. } if text == s))
}

#[test]
fn gui_search_highlights_and_counts() {
    let mut l = Live::new();
    l.tab.session.as_mut().unwrap().write(b"needle one\rneedle two\rneedle three\r");
    l.wait_for("needle three");
    l.frame(vec![], vec![Action::Find]);
    l.frame(vec![Event::Text("needle".into())], vec![]);
    l.frame(vec![], vec![]);
    let texts: Vec<String> = l.h.list().prims.iter().filter_map(|p| match p { termax_ui::display::Prim::Text { text, .. } => Some(text.clone()), _ => None }).collect();
    assert!(on_screen(&l, "3 of 3"), "the newest match is current: {texts:?} {:?}", l.tab.search);
    l.frame(vec![Event::KeyDown { key: Key::F(3), mods: Modifiers::NONE, text: None }], vec![]);
    l.frame(vec![], vec![]);
    assert!(on_screen(&l, "2 of 3"), "F3 goes to the next older match");
    let accent = l.theme.palette.accent;
    let lit = l.h.list().prims.iter().any(|p| match p {
        termax_ui::display::Prim::CellRow { cells, .. } => cells.iter().any(|c| c.bg == accent),
        _ => false,
    });
    assert!(lit, "the current match is highlighted");
    l.frame(vec![Event::KeyDown { key: Key::Escape, mods: Modifiers::NONE, text: None }], vec![]);
    assert!(l.tab.search.is_none(), "Esc closes the search");
}

fn text_at(l: &Live, s: &str) -> Option<Point> {
    l.h.list().prims.iter().find_map(|p| match p {
        termax_ui::display::Prim::Text { text, bounds, .. } if text == s => Some(Point::new(bounds.x + bounds.w / 2, bounds.y + bounds.h / 2)),
        _ => None,
    })
}

#[test]
fn gui_multiline_paste_prompts() {
    let mut l = Live::new();
    l.h.clipboard.text = Some("echo a\necho b".into());
    l.frame(vec![], vec![Action::Paste]);
    l.frame(vec![], vec![]);
    assert!(on_screen(&l, "Paste 2 lines into fx?"), "the paste waits for a yes");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!l.screen_text().contains("echo a"), "nothing sent yet");
    l.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }], vec![]);
    l.frame(vec![], vec![]);
    assert!(l.tab.pending_paste.is_none(), "a reflexive Enter cancels");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!l.screen_text().contains("echo a"));
    l.frame(vec![], vec![Action::Paste]);
    l.frame(vec![], vec![]);
    let at = text_at(&l, "Paste").expect("Paste button");
    l.frame(press(at), vec![]);
    l.frame(release(at), vec![]);
    l.wait_for("echo b");
    assert!(l.screen_text().contains("echo a"));
}

#[test]
fn shift_bypasses_mouse_reporting() {
    let mut l = Live::new();
    // The echo shell sends this back, so the "program" turns on SGR mouse.
    l.tab.session.as_mut().unwrap().write(b"\x1b[?1000h\x1b[?1006h");
    let start = Instant::now();
    let on = |l: &Live| l.tab.session.as_ref().unwrap().parser.lock().unwrap().screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None;
    while !on(&l) {
        assert!(start.elapsed() < Duration::from_secs(15), "mouse mode never turned on");
        std::thread::sleep(Duration::from_millis(30));
    }
    let (cw, chh) = l.tab.cell_size();
    let pt = |c: i32| Point::new(6 + c * cw + cw / 2, 6 + chh / 2);
    l.frame(press(pt(0)), vec![]);
    l.frame(vec![Event::MouseMove(pt(5))], vec![]);
    l.frame(release(pt(5)), vec![]);
    assert!(l.tab.select.sel.is_none(), "the program gets the drag, no selection");
    let shift = Modifiers { shift: true, ..Modifiers::NONE };
    l.frame(vec![Event::Modifiers(shift)], vec![]);
    l.frame(press(pt(0)), vec![]);
    l.frame(vec![Event::MouseMove(pt(5))], vec![]);
    l.frame(release(pt(5)), vec![]);
    assert!(l.tab.select.sel.is_some(), "Shift+drag selects");
}

#[test]
fn search_repaints_after_new_output() {
    let mut l = Live::new();
    l.frame(vec![], vec![Action::Find]);
    l.frame(vec![Event::Text("needle".into())], vec![]);
    l.frame(vec![], vec![]);
    l.tab.session.as_mut().unwrap().write(b"needle late\r");
    l.wait_for("needle late");
    assert_eq!(l.h.repaint_in, Some(Duration::ZERO), "new matches are drawn on the next frame, not whenever input comes");
}

#[test]
fn clicks_on_the_find_bar_stay_out_of_the_terminal() {
    let mut l = Live::new();
    l.frame(vec![], vec![Action::Find]);
    l.frame(vec![], vec![]);
    let bar = l.h.list().prims.iter().find_map(|p| match p {
        termax_ui::display::Prim::Text { text, bounds, .. } if text == "Find" => Some(Point::new(bounds.x + 2, bounds.y + bounds.h / 2)),
        _ => None,
    });
    let at = bar.expect("find bar placeholder");
    for _ in 0..2 {
        l.frame(press(at), vec![]);
        l.frame(release(at), vec![]);
    }
    assert!(l.tab.select.sel.is_none(), "a double-click in the find box doesn't select terminal text");
}
