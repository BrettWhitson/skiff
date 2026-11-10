//! Split pane tests: layout math, focus routing, accessible names and pty
//! sizes (only the local test server is ever connected to).

use skiff_ui::a11y::{A11yAction, Role};
use skiff_ui::geom::{Point, Rect};
use skiff_ui::input::{Event, Key, Modifiers, MouseButton};

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

#[test]
fn divider_keys_survive_restructure() {
    let mut l = Layout::Pane(0);
    l.split(0, Dir::Right, 1);
    l.split(1, Dir::Down, 2);
    l.split(2, Dir::Right, 3);
    let keys = |l: &Layout| l.arrange(Rect::new(0, 0, 800, 600), 5).1.iter().map(|d| d.key).collect::<Vec<_>>();
    assert_eq!(keys(&l), vec![1, 2, 3], "one per split, unique");
    l.remove(1); // the 2|3 split moves up the tree
    assert_eq!(keys(&l), vec![2, 3]);
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
    // Arrow keys and the screen reader step through the snap points.
    f.frame(vec![key(Key::Down, Modifiers::NONE)]);
    f.frame(vec![Event::A11y { target: id, action: A11yAction::Increment }]);
    f.frame(vec![]);
    assert_eq!(f.h.tree().find("Pane divider, top and bottom").and_then(|n| n.value.clone()).as_deref(), Some("75%"));
    let Layout::Split { ratio, .. } = split_of(&f).layout else { panic!() };
    assert!((ratio - 0.75).abs() < 1e-4, "{ratio}");
}

#[test]
fn each_pane_gets_its_own_pty_size() {
    let fx = skiff_testserver::TestFixture::with_shell(skiff_testserver::FixtureAuth::Key).unwrap();
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

#[test]
fn terminal_menu_splits_and_closes_panes() {
    let (mut f, _) = alpha();
    let menu_pick = |f: &mut Fixture, name: &str, item: &str| {
        let r = f.h.tree().find(name).expect("pane").bounds;
        let at = Point::new(r.x + 30, r.y + 80);
        f.frame(vec![Event::MouseMove(at), Event::MouseDown(MouseButton::Right, at), Event::MouseUp(MouseButton::Right, at)]);
        f.frame(vec![]);
        f.click_text(item);
    };
    menu_pick(&mut f, "Terminal: alpha", "Split down");
    assert_eq!(split_of(&f).panes.len(), 2);
    menu_pick(&mut f, "Terminal: alpha, pane 1 of 2", "Close pane");
    assert_eq!(split_of(&f).panes.iter().map(|p| p.key).collect::<Vec<_>>(), vec![1], "the pane whose menu it was");
}

#[test]
fn tree_moves_swaps_and_equalizes() {
    // 0 | 1, then 2 dropped on the bottom of 1, then 3 on the left of 0.
    let mut l = Layout::Pane(0);
    l.split(0, Dir::Right, 1);
    assert!(l.insert(1, Zone::Bottom, Layout::Pane(2)));
    assert!(l.insert(0, Zone::Left, Layout::Pane(3)));
    assert!(!l.insert(1, Zone::Center, Layout::Pane(4)) && !l.insert(9, Zone::Left, Layout::Pane(4)));
    assert_eq!(l.keys(), vec![3, 0, 1, 2]);
    l.swap(3, 2);
    assert_eq!(l.keys(), vec![2, 0, 1, 3]);
    // The divider a pane resizes along an axis is its innermost such split.
    assert_eq!(l.axis_split(1, Dir::Down), Some(vec![true]));
    assert_eq!(l.axis_split(2, Dir::Right), Some(vec![false]));
    assert_eq!(l.axis_split(2, Dir::Down), None);
    // Equal columns: 2 | 0 | (1 over 3) are a third each.
    l.equalize();
    let (panes, _) = l.arrange(Rect::new(0, 0, 900, 600), 0);
    let widths: Vec<i32> = panes.iter().map(|(_, r)| r.w).collect();
    assert_eq!(widths, vec![300, 300, 300, 300]);
    assert_eq!(panes[2].1.h, 300);
}

#[test]
fn panes_keep_a_minimum_size() {
    let mut l = Layout::Pane(0);
    l.split(0, Dir::Right, 1);
    l.split(1, Dir::Right, 2);
    l.set_ratio(&[], MIN_RATIO); // first pane 10% of 500 px
    let min = (100, 60);
    l.enforce_min(Rect::new(0, 0, 500, 300), 0, min);
    let (panes, _) = l.arrange(Rect::new(0, 0, 500, 300), 0);
    assert!(panes.iter().all(|(_, r)| r.w >= min.0), "{panes:?}");
    // No room for all: the sides share by what they need, nothing collapses.
    l.enforce_min(Rect::new(0, 0, 150, 300), 0, min);
    let (panes, _) = l.arrange(Rect::new(0, 0, 150, 300), 0);
    assert!(panes.iter().all(|(_, r)| r.w >= 45), "{panes:?}");
}

#[test]
fn drop_zones_and_snapping() {
    let r = Rect::new(100, 100, 300, 300);
    let at = |x, y| Zone::at(r, Point::new(100 + x, 100 + y));
    assert_eq!(
        [at(150, 150), at(10, 150), at(290, 150), at(150, 10), at(150, 290)],
        [Zone::Center, Zone::Left, Zone::Right, Zone::Top, Zone::Bottom]
    );
    assert_eq!(at(20, 5), Zone::Top, "nearest edge wins in a corner");
    assert_eq!(Zone::Left.preview(r, 0), Rect::new(100, 100, 150, 300));
    assert_eq!(Zone::Bottom.preview(r, 0), Rect::new(100, 250, 300, 150));
    assert_eq!(Zone::Center.preview(r, 0), r);
    assert!(Zone::Left.fits(r, 0, (150, 50)) && !Zone::Left.fits(r, 0, (151, 50)) && Zone::Center.fits(r, 0, (999, 999)));

    // Within 8 px of a snap point (of 600) the divider snaps there.
    assert_eq!(snap(0.51, 600, 8), Some(0.5));
    assert_eq!(snap(0.52, 600, 8), None);
    assert_eq!(snap(0.33, 600, 8), Some(1.0 / 3.0));
    // Keys step snap point to snap point, then 5% at a time.
    assert_eq!(step_ratio(0.5, true), 2.0 / 3.0);
    assert_eq!(step_ratio(0.4, false), 1.0 / 3.0);
    assert!((step_ratio(0.75, true) - 0.8).abs() < 1e-6);
    assert_eq!(step_ratio(0.12, false), MIN_RATIO);
}

/// Three panes on alpha: 1 left, 2 top right, 3 bottom right (keys 0, 1, 2).
fn three() -> (Fixture, u64) {
    let (mut f, tab) = alpha();
    f.app.do_action(Action::SplitRight);
    f.app.do_action(Action::SplitDown);
    f.frame(vec![]);
    f.frame(vec![]);
    (f, tab)
}

fn keys_of(f: &Fixture) -> Vec<u64> {
    split_of(f).panes.iter().map(|p| p.key).collect()
}

#[test]
fn keyboard_moves_resizes_and_equalizes_panes() {
    let (mut f, tab) = three();
    assert_eq!((keys_of(&f), split_of(&f).focused_key()), (vec![0, 1, 2], 2));
    let alt_shift = Modifiers { alt: true, shift: true, ..Modifiers::NONE };
    let move_mods = Modifiers { ctrl: true, alt: true, shift: true, ..Modifiers::NONE };
    // Move the bottom-right pane up (swap), then left (swap with pane 0).
    f.frame(vec![key(Key::Up, move_mods)]);
    f.frame(vec![]);
    assert_eq!(keys_of(&f), vec![0, 2, 1]);
    f.frame(vec![key(Key::Left, move_mods)]);
    f.frame(vec![]);
    assert_eq!(keys_of(&f), vec![2, 0, 1]);
    assert_eq!((split_of(&f).focus, f.h.state.focused()), (0, Some(pane_id(tab, 2))), "focus moves with it");
    // Names follow the new order.
    let t = f.h.tree();
    assert!(t.find("Pane 1 of 3: alpha").is_some() && t.find("Terminal: alpha, pane 3 of 3").is_some());
    assert_eq!(t.find("Terminal: alpha, pane 1 of 3").map(|n| n.focused), Some(true));
    // Nothing to the left: nothing changes.
    f.frame(vec![key(Key::Left, move_mods)]);
    assert_eq!(keys_of(&f), vec![2, 0, 1]);

    // Resize: Alt+Shift+Right moves the left/right divider to the next snap point.
    f.frame(vec![key(Key::Right, alt_shift)]);
    f.frame(vec![key(Key::Right, alt_shift)]);
    assert_eq!(split_of(&f).layout.ratio(&[]), Some(0.75));
    // Equalize: the top-level split back to half, after the inner one moved.
    f.app.do_action(Action::FocusNextPane);
    f.frame(vec![]);
    f.frame(vec![key(Key::Down, alt_shift)]);
    assert!((split_of(&f).layout.ratio(&[true]).unwrap() - 2.0 / 3.0).abs() < 1e-6);
    f.frame(vec![key(Key::Char('+'), alt_shift)]);
    assert_eq!((split_of(&f).layout.ratio(&[]), split_of(&f).layout.ratio(&[true])), (Some(0.5), Some(0.5)));
}

#[test]
fn tabs_join_and_panes_pop_out_keeping_sessions() {
    let (mut f, a) = alpha();
    let beta = f.app.hosts[1].clone();
    let b = f.app.open_terminal(&beta);
    f.frame(vec![]);
    // A marker on beta's terminal shows the same TermTab moves (no reconnect).
    match &mut f.app.tabs[1].kind {
        TabKind::Terminal(t) => t.panes[0].term.error = Some("marker".into()),
        _ => panic!(),
    }
    f.app.drop_tab_on_pane(TabDrop { from: b, target: 0, zone: Zone::Left }, a);
    f.frame(vec![]);
    assert_eq!(f.app.tabs.len(), 1);
    let s = split_of(&f);
    assert_eq!(s.panes.iter().map(|p| p.term.host.name.as_str()).collect::<Vec<_>>(), vec!["beta", "alpha"]);
    assert_eq!(s.panes[0].term.error.as_deref(), Some("marker"));
    assert_eq!(s.focus, 0, "the joined pane has focus");
    let joined = s.panes[0].key;
    assert!(f.h.tree().find("Pane 1 of 2: beta").is_some());

    f.app.pop_out_pane(a, joined);
    f.frame(vec![]);
    assert_eq!((f.app.tabs.len(), f.app.active), (2, 1));
    assert_eq!(split_of(&f).panes[0].term.error.as_deref(), Some("marker"));
    assert_eq!(f.h.tree().find("Terminal: beta").map(|n| n.role), Some(Role::Terminal));
    // The last pane of a tab stays; a drop on the centre just shows the tab.
    let b2 = f.app.tabs[1].id;
    f.app.pop_out_pane(b2, 0);
    assert_eq!(f.app.tabs.len(), 2);
    f.app.active = 0;
    f.app.drop_tab_on_pane(TabDrop { from: b2, target: 0, zone: Zone::Center }, a);
    assert_eq!((f.app.tabs.len(), f.app.active), (2, 1));
}

/// Drags with the mouse from `from` through `path`, releasing at the end.
fn drag(f: &mut Fixture, from: Point, path: &[Point]) {
    f.frame(vec![Event::MouseMove(from), Event::MouseDown(MouseButton::Left, from)]);
    for p in path {
        f.frame(vec![Event::MouseMove(*p)]);
    }
    let end = *path.last().unwrap();
    f.frame(vec![Event::MouseUp(MouseButton::Left, end)]);
    f.frame(vec![]);
}

#[test]
fn dragging_a_tab_onto_a_pane_edge_splits_and_esc_cancels() {
    let (mut f, a) = alpha();
    let beta = f.app.hosts[1].clone();
    f.app.open_terminal(&beta);
    f.app.active = 0;
    f.frame(vec![]);
    f.frame(vec![]);
    let tab = f.h.tree().find("\u{25CF} beta").expect("tab of beta").bounds;
    let pane = f.h.tree().find("Terminal: alpha").expect("terminal of alpha").bounds;
    let right_edge = Point::new(pane.right() - 20, pane.center().y);
    let below = Point::new(tab.center().x, tab.bottom() + 40);

    // Esc cancels: nothing moves.
    f.frame(vec![Event::MouseMove(tab.center()), Event::MouseDown(MouseButton::Left, tab.center())]);
    f.frame(vec![Event::MouseMove(below)]);
    assert!(f.h.state.dragging());
    assert_eq!(f.app.active, 0, "alpha shows while beta is dragged");
    f.frame(vec![Event::MouseMove(right_edge)]);
    f.frame(vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(!f.h.state.dragging());
    f.frame(vec![Event::MouseUp(MouseButton::Left, right_edge)]);
    f.frame(vec![]);
    assert_eq!(f.app.tabs.len(), 2);

    drag(&mut f, tab.center(), &[below, right_edge]);
    assert_eq!(f.app.tabs.len(), 1, "beta joined the tab of alpha");
    assert_eq!(f.app.tabs[0].id, a);
    let s = split_of(&f);
    assert_eq!(s.panes.iter().map(|p| p.term.host.name.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta"]);
    assert!(matches!(s.layout, Layout::Split { dir: Dir::Right, .. }));

    // The grip of a pane drags it onto the tab strip: a tab of its own again.
    let beta_pane = f.h.tree().find("Terminal: beta, pane 2 of 2").expect("beta pane").bounds;
    let grip = Point::new(beta_pane.center().x, beta_pane.y + 3);
    let strip = Point::new(tab.x + 4, tab.center().y);
    drag(&mut f, grip, &[Point::new(grip.x - 30, grip.y + 30), strip]);
    assert_eq!(f.app.tabs.len(), 2);
    assert_eq!(f.app.tab_title(1), "\u{25CF} beta");
}

#[test]
fn divider_drag_snaps_alt_frees_double_click_equalizes() {
    let (mut f, _) = alpha();
    f.app.do_action(Action::SplitRight);
    f.frame(vec![]);
    f.frame(vec![]);
    let ratio = |f: &Fixture| split_of(f).layout.ratio(&[]).unwrap();
    let div = f.h.tree().find("Pane divider, left and right").expect("divider").bounds;
    let left = f.h.tree().find("Terminal: alpha, pane 1 of 2").expect("pane 1").bounds;
    let right = f.h.tree().find("Terminal: alpha, pane 2 of 2").expect("pane 2").bounds;
    let room = (right.right() - left.x - div.w) as f32;
    let x_at = |r: f32| left.x + (room * r) as i32 + div.w / 2;
    let y = div.center().y;
    // 4 px short of a third: snaps to it.
    drag(&mut f, div.center(), &[Point::new(x_at(1.0 / 3.0) + 4, y)]);
    assert!((ratio(&f) - 1.0 / 3.0).abs() < 1e-6, "{}", ratio(&f));
    // Alt held: lands where dropped.
    let div = f.h.tree().find("Pane divider, left and right").unwrap().bounds;
    f.frame(vec![Event::MouseMove(div.center()), Event::MouseDown(MouseButton::Left, div.center())]);
    f.frame(vec![Event::Modifiers(Modifiers { alt: true, ..Modifiers::NONE }), Event::MouseMove(Point::new(x_at(0.6) + 3, y))]);
    f.frame(vec![Event::MouseUp(MouseButton::Left, Point::new(x_at(0.6) + 3, y))]);
    assert!((ratio(&f) - 0.6).abs() > 1e-3 && (ratio(&f) - 0.6).abs() < 0.01, "{}", ratio(&f));
    f.frame(vec![Event::Modifiers(Modifiers::NONE)]);
    // Double-click: back to equal halves.
    let c = f.h.tree().find("Pane divider, left and right").unwrap().bounds.center();
    let click = vec![Event::MouseMove(c), Event::MouseDown(MouseButton::Left, c), Event::MouseUp(MouseButton::Left, c)];
    f.frame(click.clone());
    f.frame(click);
    f.frame(vec![]);
    assert_eq!(ratio(&f), 0.5);
}

#[test]
fn alt_drag_moves_a_pane_onto_another() {
    let (mut f, _) = three();
    let first = f.h.tree().find("Terminal: alpha, pane 1 of 3").unwrap().bounds.center();
    let last = f.h.tree().find("Terminal: alpha, pane 3 of 3").unwrap().bounds;
    f.frame(vec![Event::Modifiers(Modifiers { alt: true, ..Modifiers::NONE })]);
    drag(&mut f, first, &[last.center()]);
    assert_eq!(keys_of(&f), vec![2, 1, 0], "centre: swapped");
    assert!(split_of(&f).panes.iter().all(|p| p.term.select.sel.is_none()), "no text selected");
}

#[test]
fn minimum_size_does_not_change_stored_shares() {
    let (mut f, _) = alpha();
    f.app.do_action(Action::SplitRight);
    f.frame(vec![]);
    let set = |f: &mut Fixture| match &mut f.app.tabs[0].kind {
        TabKind::Terminal(t) => t.layout.set_ratio(&[], MIN_RATIO),
        _ => panic!(),
    };
    set(&mut f);
    let width = |f: &Fixture| f.h.tree().find("Terminal: alpha, pane 1 of 2").expect("pane 1").bounds.w;
    // Narrow: pane 1 is held at its minimum, wider than 10%.
    f.h.resize(450, 600);
    f.frame(vec![]);
    let (cw, _) = split_of(&f).panes[0].term.cell_size();
    assert!(width(&f) >= cw * MIN_COLS, "{} < 10 cells of {cw}", width(&f));
    assert_eq!(split_of(&f).layout.ratio(&[]), Some(MIN_RATIO), "the stored share is kept");
    // Wide again: back to 10%.
    f.h.resize(4000, 800);
    f.frame(vec![]);
    let total = width(&f) + f.h.tree().find("Terminal: alpha, pane 2 of 2").unwrap().bounds.w;
    assert!((width(&f) as f32 / total as f32 - MIN_RATIO).abs() < 0.01, "{} of {total}", width(&f));
}

#[test]
fn a_tab_merged_early_is_still_recorded_in_history() {
    let fx = skiff_testserver::TestFixture::with_shell(skiff_testserver::FixtureAuth::Key).unwrap();
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let (mut f, a) = alpha();
    let beta = f.app.hosts[1].clone();
    let term = TermTab::with_command(beta.clone(), "ssh".into(), args, 100);
    f.app.pending_records.push((term.uid, beta.name.clone(), std::time::Instant::now()));
    let b = f.app.push_tab(TabKind::Terminal(Box::new(TermSplit::new(term))));
    f.frame(vec![]);
    f.app.drop_tab_on_pane(TabDrop { from: b, target: 0, zone: Zone::Right }, a);
    let start = std::time::Instant::now();
    while !f.app.history.hosts.contains_key("beta") {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "beta never recorded");
        f.frame(vec![]);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
