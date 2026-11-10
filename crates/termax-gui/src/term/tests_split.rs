//! Split pane tests: layout math, focus routing, accessible names and pty
//! sizes (only the local test server is ever connected to).

use termax_ui::a11y::{A11yAction, Role};
use termax_ui::geom::{Point, Rect};
use termax_ui::input::{Event, Key, Modifiers};

use super::*;
use crate::app::TabKind;
use crate::app::tests::Fixture;

#[test]
fn layout_math() {
    let one = Layout::Pane(0);
    assert_eq!(one.arrange(Rect::new(0, 0, 100, 50), 5), (vec![(0, Rect::new(0, 0, 100, 50))], vec![]));

    let mut l = Layout::Pane(0);
    assert!(l.split(0, Dir::Right, 1));
    let (panes, divs) = l.arrange(Rect::new(10, 0, 1005, 600), 5);
    assert_eq!(panes, vec![(0, Rect::new(10, 0, 500, 600)), (1, Rect::new(515, 0, 500, 600))]);
    assert_eq!(divs[0].handle, Rect::new(510, 0, 5, 600));

    // Splitting the right pane down nests a split inside it.
    assert!(l.split(1, Dir::Down, 2));
    assert!(!l.split(9, Dir::Down, 3), "no such pane");
    let (panes, divs) = l.arrange(Rect::new(0, 0, 1005, 605), 5);
    assert_eq!(l.keys(), vec![0, 1, 2]);
    assert_eq!(panes[1].1, Rect::new(505, 0, 500, 300));
    assert_eq!(panes[2].1, Rect::new(505, 305, 500, 300));
    assert_eq!(divs[1].path, vec![true]);

    // Ratios stay within bounds, from the keyboard or a drag past the edge.
    l.set_ratio(&[true], 0.99);
    assert_eq!(l.arrange(Rect::new(0, 0, 1005, 605), 5).1[1].ratio, 1.0 - MIN_RATIO);
    let area = Rect::new(100, 0, 405, 100);
    assert_eq!(ratio_at(area, Dir::Right, 5, Point::new(0, 0)), MIN_RATIO);
    assert!((ratio_at(area, Dir::Right, 5, Point::new(302, 0)) - 0.5).abs() < 0.01);

    // Closing a pane gives its space to its neighbor, which gets focus.
    assert_eq!(l.remove(1), Some(2));
    assert_eq!(l.keys(), vec![0, 2]);
    assert_eq!(l.remove(2), Some(0));
    assert_eq!(l, Layout::Pane(0));
    assert_eq!(l.remove(0), None, "the last pane stays");
}

fn key(key: Key, mods: Modifiers) -> Event {
    Event::KeyDown { key, mods, text: None }
}

fn split_of(f: &Fixture) -> &TermSplit {
    match &f.app.tabs[f.app.active].kind {
        TabKind::Terminal(t) => t,
        _ => panic!("not a terminal"),
    }
}

/// A terminal tab on host alpha (its ssh program doesn't exist: no network).
fn alpha() -> (Fixture, u64) {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    f.frame(vec![]);
    let id = f.app.tabs[0].id;
    (f, id)
}

#[test]
fn focus_moves_between_panes() {
    let (mut f, tab) = alpha();
    let ctrl_shift = Modifiers { ctrl: true, shift: true, ..Modifiers::NONE };
    f.frame(vec![key(Key::Char('d'), ctrl_shift)]); // split right
    f.frame(vec![]);
    assert_eq!(split_of(&f).panes.len(), 2);
    assert_eq!(f.h.state.focused(), Some(pane_id(tab, 1)), "the new pane has focus");
    assert!(f.app.tab_title(0).contains("2 panes"));

    // A click in the left pane focuses it.
    let left = f.h.tree().find("Terminal: alpha, pane 1 of 2").expect("pane 1").bounds;
    f.click(Point::new(left.x + 20, left.y + 60));
    assert_eq!((split_of(&f).focus, f.h.state.focused()), (0, Some(pane_id(tab, 0))));

    // F6 from a terminal reaches the next pane; the shortcut cycles too.
    f.frame(vec![key(Key::F(6), Modifiers::NONE)]);
    f.frame(vec![]);
    assert_eq!((split_of(&f).focus, f.h.state.focused()), (1, Some(pane_id(tab, 1))));
    f.frame(vec![key(Key::Char('o'), ctrl_shift)]);
    f.frame(vec![]);
    assert_eq!((split_of(&f).focus, f.h.state.focused()), (0, Some(pane_id(tab, 0))));

    // Close pane: the neighbor gets focus; on the last pane it closes the tab.
    f.frame(vec![key(Key::Char('x'), ctrl_shift)]);
    f.frame(vec![]);
    assert_eq!(split_of(&f).panes.len(), 1);
    assert_eq!(split_of(&f).panes[0].key, 1);
    assert_eq!(f.h.state.focused(), Some(pane_id(tab, 1)));
    assert_eq!(f.h.tree().find("Terminal: alpha").map(|n| n.role), Some(Role::Terminal), "one pane: plain name");
    f.frame(vec![key(Key::Char('x'), ctrl_shift)]);
    f.frame(vec![]);
    assert!(f.app.tabs.is_empty());
}

#[test]
fn panes_and_divider_are_named_and_keyboard_resizable() {
    let (mut f, _) = alpha();
    f.app.do_action(Action::SplitDown);
    f.frame(vec![]);
    f.frame(vec![]);
    let t = f.h.tree();
    for i in 1..=2 {
        assert_eq!(t.find(&format!("Terminal: alpha, pane {i} of 2")).map(|n| n.role), Some(Role::Terminal));
        assert_eq!(t.find(&format!("Pane {i} of 2: alpha")).map(|n| n.role), Some(Role::Region));
    }
    let div = t.find("Pane divider, top and bottom").expect("divider");
    assert_eq!((div.role, div.value.as_deref()), (Role::Splitter, Some("50%")));
    let id = div.id;
    f.frame(vec![Event::A11y { target: id, action: A11yAction::Focus }]);
    f.frame(vec![key(Key::Down, Modifiers::NONE)]);
    f.frame(vec![Event::A11y { target: id, action: A11yAction::Increment }]);
    f.frame(vec![]);
    assert_eq!(f.h.tree().find("Pane divider, top and bottom").and_then(|n| n.value.clone()).as_deref(), Some("60%"));
    let Layout::Split { ratio, .. } = split_of(&f).layout else { panic!() };
    assert!((ratio - 0.6).abs() < 1e-4, "{ratio}");
}

#[test]
fn each_pane_gets_its_own_pty_size() {
    let fx = termax_testserver::TestFixture::with_shell(termax_testserver::FixtureAuth::Key).unwrap();
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let mut f = Fixture::new();
    let host = f.app.hosts[0].clone();
    f.app.push_tab(TabKind::Terminal(Box::new(TermSplit::new(TermTab::with_command(host, "ssh".into(), args, 100)))));
    f.frame(vec![]);
    let whole = split_of(&f).panes[0].term.grid;
    f.app.do_action(Action::SplitRight);
    f.frame(vec![]);
    f.frame(vec![]);
    let s = split_of(&f);
    let (a, b) = (s.panes[0].term.grid, s.panes[1].term.grid);
    assert!(s.panes.iter().all(|p| p.term.session.is_some()), "a session per pane");
    assert_eq!((a.0, b.0), (whole.0, whole.0), "same height");
    assert!(a.1 < whole.1 / 2 + 1 && a.1 + b.1 <= whole.1 && a.1 + b.1 >= whole.1 - 4, "{whole:?} -> {a:?} + {b:?}");
    assert_eq!(s.panes[0].term.resizes, 1, "the old pane's pty shrank to its half");
}
