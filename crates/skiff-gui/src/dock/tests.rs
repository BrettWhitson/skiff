//! Dock tests: layout math, the group model, drop-zone math, dragging any
//! tab (with real event batching) onto edges, centres and strips, divider
//! resizing, keyboard actions, focus and accessible names (only the local
//! test server is ever connected to).

use skiff_ui::a11y::{A11yAction, Role};
use skiff_ui::display::Prim;
use skiff_ui::geom::{Point, Rect};
use skiff_ui::input::{Event, Key, Modifiers, MouseButton};

use super::layout::{MIN_RATIO, snap};
use super::*;
use crate::app::TabKind;
use crate::app::tests::Fixture;
use crate::keys::Action;
use crate::term::{TermTab, term_id};

#[test]
/// Splits, nesting, ratio bounds, `ratio_at` and removal lay groups out as expected.
fn layout_math() {
    let one = Layout::Group(0);
    assert_eq!(one.arrange(Rect::new(0, 0, 100, 50), 5), (vec![(0, Rect::new(0, 0, 100, 50))], vec![]));
    let mut l = Layout::Group(0);
    assert!(l.split(0, Dir::Right, 1));
    let (groups, divs) = l.arrange(Rect::new(10, 0, 1005, 600), 5);
    assert_eq!(groups, vec![(0, Rect::new(10, 0, 500, 600)), (1, Rect::new(515, 0, 500, 600))]);
    assert_eq!(divs[0].handle, Rect::new(510, 0, 5, 600));
    assert!(l.split(1, Dir::Down, 2));
    assert!(!l.split(9, Dir::Down, 3), "no such group");
    let (groups, divs) = l.arrange(Rect::new(0, 0, 1005, 605), 5);
    assert_eq!(l.keys(), vec![0, 1, 2]);
    assert_eq!(groups[2].1, Rect::new(505, 305, 500, 300));
    assert_eq!(divs[1].path, vec![true]);
    l.set_ratio(&[true], 0.99);
    assert_eq!(l.arrange(Rect::new(0, 0, 1005, 605), 5).1[1].ratio, 1.0 - MIN_RATIO);
    let area = Rect::new(100, 0, 405, 100);
    assert_eq!(ratio_at(area, Dir::Right, 5, Point::new(0, 0)), MIN_RATIO);
    // Neighbors, then insertion on every side and equal shares.
    assert_eq!((l.neighbor(2, Zone::Top), l.neighbor(2, Zone::Left), l.neighbor(0, Zone::Left)), (Some(1), Some(0), None));
    assert!(l.insert(0, Zone::Left, Layout::Group(3)) && !l.insert(1, Zone::Center, Layout::Group(4)));
    assert_eq!(l.keys(), vec![3, 0, 1, 2]);
    l.equalize();
    let (groups, _) = l.arrange(Rect::new(0, 0, 900, 600), 0);
    assert_eq!(groups.iter().map(|(_, r)| r.w).collect::<Vec<_>>(), vec![300, 300, 300, 300]);
    // Removal gives the space to the neighbor; minimum sizes clamp a copy.
    assert_eq!(l.remove(3), Some(0));
    l.set_ratio(&[], MIN_RATIO);
    l.enforce_min(Rect::new(0, 0, 500, 300), 0, (100, 60));
    assert!(l.arrange(Rect::new(0, 0, 500, 300), 0).0.iter().all(|(_, r)| r.w >= 100));
}

#[test]
/// The model: new tabs join the focused group, moves into / beside groups,
/// empty groups close, and drops that change nothing are refused.
fn dock_model_moves_and_closes_groups() {
    let mut d = Dock::default();
    d.sync(&[10, 11, 12], Some(12));
    assert_eq!((d.groups.len(), d.group_of(11), d.focused().map(|g| g.active)), (1, Some(0), Some(12)));
    // The only tab of a group cannot dock beside itself or into itself.
    let mut solo = Dock::default();
    solo.sync(&[1], Some(1));
    assert!(!solo.can_drop(1, Drop::Beside { group: 0, zone: Zone::Right }));
    assert!(!solo.can_drop(1, Drop::Into { group: 0, index: None }));
    // 12 to a new group on the right; 10 into it at the front.
    assert_eq!(d.apply(12, Drop::Beside { group: 0, zone: Zone::Right }), Some(1));
    assert_eq!(d.apply(10, Drop::Into { group: 1, index: Some(0) }), Some(1));
    assert_eq!((d.group(0).unwrap().tabs.clone(), d.group(1).unwrap().tabs.clone()), (vec![11], vec![10, 12]));
    // Reordering in a strip: the index right after itself is a no-op.
    assert!(!d.can_drop(10, Drop::Into { group: 1, index: Some(1) }));
    assert_eq!(d.apply(10, Drop::Into { group: 1, index: Some(2) }), Some(1));
    assert_eq!(d.group(1).unwrap().tabs, vec![12, 10]);
    // Moving the last tab out closes its group; focus follows the tab.
    assert_eq!(d.move_target(11, Zone::Right), Some(Drop::Into { group: 1, index: None }));
    d.apply(11, Drop::Into { group: 1, index: None });
    assert_eq!((d.order(), d.focus), (vec![1], 1));
    assert_eq!(d.move_target(11, Zone::Left), Some(Drop::Beside { group: 1, zone: Zone::Left }), "a new group at the edge");
    // Closing tabs: the left neighbor shows; the last tab closes the group.
    d.sync(&[12, 11], Some(11));
    assert_eq!(d.group(1).unwrap().tabs, vec![12, 11]);
    assert_eq!(d.remove_tab(11), Some(12));
    assert_eq!(d.remove_tab(12), None);
    assert!(d.layout.is_none() && d.groups.is_empty());
}

#[test]
/// Zones by position and by guide square, insertion points, fits and snapping.
fn drop_zone_math() {
    let r = Rect::new(100, 100, 300, 300);
    let at = |x, y| Zone::at(r, Point::new(100 + x, 100 + y));
    assert_eq!(
        [at(150, 150), at(10, 150), at(290, 150), at(150, 10), at(150, 290)],
        [Zone::Center, Zone::Left, Zone::Right, Zone::Top, Zone::Bottom]
    );
    assert_eq!(Zone::Bottom.preview(r, 0), Rect::new(100, 250, 300, 150));
    assert!(Zone::Left.fits(r, 0, (150, 50)) && !Zone::Left.fits(r, 0, (151, 50)));
    // A guide square wins over the position (its left square is in the middle third).
    let guide = [(Zone::Left, Rect::new(220, 240, 30, 30))];
    assert_eq!(view::zone_under(&guide, r, Point::new(230, 250)), Zone::Left);
    assert_eq!(view::zone_under(&guide, r, Point::new(260, 250)), Zone::Center);
    let tabs = [Rect::new(0, 0, 100, 30), Rect::new(102, 0, 100, 30)];
    assert_eq!([0, 49, 51, 160, 500].map(|x| view::insertion_index(&tabs, x)), [0, 0, 1, 2, 2]);
    assert_eq!(snap(0.51, 600, 8), Some(0.5));
    assert_eq!(step_ratio(0.5, true), 2.0 / 3.0);
}

/// A KeyDown event with no text.
fn key(key: Key, mods: Modifiers) -> Event {
    Event::KeyDown { key, mods, text: None }
}

/// Titles per group, in reading order.
fn groups(f: &Fixture) -> Vec<Vec<String>> {
    let d = &f.app.dock;
    d.order().iter().map(|g| d.group(*g).unwrap().tabs.iter().map(|t| f.app.tab_title(f.app.tab_index(*t).unwrap())).collect()).collect()
}

/// A terminal tab on alpha (its ssh program doesn't exist: no network).
fn alpha() -> Fixture {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    f.frame(vec![]);
    f
}

/// Alpha and beta terminals in one group (beta shown and focused).
fn alpha_beta() -> Fixture {
    let mut f = alpha();
    let beta = f.app.hosts[1].clone();
    f.app.open_terminal(&beta);
    f.frame(vec![]);
    f.frame(vec![]);
    f
}

/// Where the tab named `title` is drawn.
fn tab_rect(f: &Fixture, title: &str) -> Rect {
    let n = f.h.tree().nodes.iter().find(|n| n.role == Role::Tab && (n.name == title || n.name.starts_with(&format!("{title}, "))));
    n.unwrap_or_else(|| panic!("no tab {title}")).bounds
}

/// Bounds of the node named `name`.
fn bounds(f: &Fixture, name: &str) -> Rect {
    f.h.tree().find(name).unwrap_or_else(|| panic!("no {name}")).bounds
}

/// Whether this frame drew a drop preview (the translucent accent fill).
fn preview_drawn(f: &Fixture) -> bool {
    let fill = f.app.theme.palette.accent.with_alpha(46);
    f.h.list().prims.iter().any(|p| matches!(p, Prim::Rect { color, .. } if *color == fill))
}

/// Presses at `from`, moves through `path` (batched as winit delivers them:
/// the press with its move, then a couple of moves per frame) and releases.
fn drag(f: &mut Fixture, from: Point, path: &[Point]) {
    f.frame(vec![Event::MouseMove(from), Event::MouseDown(MouseButton::Left, from)]);
    for p in path.chunks(2) {
        f.frame(p.iter().map(|p| Event::MouseMove(*p)).collect());
    }
    let end = *path.last().unwrap();
    f.frame(vec![Event::MouseUp(MouseButton::Left, end)]);
    f.frame(vec![]);
}

#[test]
/// The owner's report: dragging the active tab, with real event batching
/// and a few px of wobble, shows the dock overlays and docks it on drop;
/// the only tab of a group drags onto another group's centre; Esc cancels.
fn dragging_the_active_tab_shows_overlays_and_docks_it() {
    let mut f = alpha_beta();
    let tab = tab_rect(&f, "beta");
    let content = bounds(&f, "Terminal: beta");
    let edge = Point::new(content.right() - 20, content.center().y);
    let c = tab.center();
    // Esc mid-drag: nothing moves.
    f.frame(vec![Event::MouseMove(c), Event::MouseDown(MouseButton::Left, c)]);
    f.frame(vec![Event::MouseMove(Point::new(c.x + 1, c.y + 2)), Event::MouseMove(Point::new(c.x + 3, c.y + 30))]);
    f.frame(vec![Event::MouseMove(edge)]);
    assert!(f.h.state.dragging() && preview_drawn(&f), "the drop preview shows while the active tab is dragged");
    f.frame(vec![key(Key::Escape, Modifiers::NONE)]);
    f.frame(vec![Event::MouseUp(MouseButton::Left, edge)]);
    f.frame(vec![]);
    assert!(!preview_drawn(&f));
    assert_eq!(groups(&f), vec![vec!["alpha", "beta"]]);

    drag(&mut f, c, &[Point::new(c.x + 1, c.y + 2), Point::new(c.x + 3, c.y + 30), edge, Point::new(edge.x + 1, edge.y)]);
    assert_eq!(groups(&f), vec![vec!["alpha"], vec!["beta"]], "docked on the right");
    assert_eq!(f.h.state.focused(), Some(term_id(f.app.tabs[1].id)), "the docked terminal has focus");
    assert!(f.h.tree().nodes.iter().any(|n| n.live && n.name.starts_with("beta moved to a new group on the right")));

    // The only tab of group 2 onto group 1's centre: the groups merge.
    let tab = tab_rect(&f, "beta");
    let left = bounds(&f, "Terminal: alpha, group 1 of 2");
    drag(&mut f, tab.center(), &[Point::new(tab.center().x, tab.bottom() + 30), left.center(), left.center()]);
    assert_eq!(groups(&f), vec![vec!["alpha", "beta"]]);
}

#[test]
/// Any kind docks: Settings to the left edge; a terminal into another
/// group's strip at the marked position; a release outside does nothing.
fn any_tab_docks_and_strip_drops_insert() {
    let mut f = alpha_beta();
    f.app.open_settings();
    f.frame(vec![]);
    f.frame(vec![]);
    let s = tab_rect(&f, "Settings");
    let left = Point::new(f.app.geometry.sidebar_width as i32 + 30, 400);
    drag(&mut f, s.center(), &[Point::new(s.center().x, s.bottom() + 40), left]);
    assert_eq!(groups(&f), vec![vec!["Settings"], vec!["alpha", "beta"]]);
    // alpha onto the Settings strip, before Settings.
    let a = tab_rect(&f, "alpha");
    let s = tab_rect(&f, "Settings");
    let before = Point::new(s.x + 6, s.center().y);
    f.frame(vec![Event::MouseMove(a.center()), Event::MouseDown(MouseButton::Left, a.center())]);
    f.frame(vec![Event::MouseMove(Point::new(a.center().x, a.bottom() + 40))]);
    f.frame(vec![Event::MouseMove(before)]);
    let accent = f.app.theme.palette.accent;
    let marker =
        f.h.list()
            .prims
            .iter()
            .any(|p| matches!(p, Prim::Rect { rect, color, .. } if *color == accent && rect.w <= 3 && rect.x <= s.x && rect.h > 10));
    assert!(marker, "an insertion marker before Settings");
    f.frame(vec![Event::MouseUp(MouseButton::Left, before)]);
    f.frame(vec![]);
    assert_eq!(groups(&f), vec![vec!["alpha", "Settings"], vec!["beta"]]);
    // Released over the status bar: nothing changes.
    let b = tab_rect(&f, "beta");
    drag(&mut f, b.center(), &[Point::new(b.center().x, 790)]);
    assert_eq!(groups(&f), vec![vec!["alpha", "Settings"], vec!["beta"]]);
}

#[test]
/// Group dividers are Splitters: a drag snaps (Alt frees it), a double-click
/// equalizes, and the keyboard and screen reader step the snap points.
fn group_divider_resizes() {
    let mut f = alpha();
    f.app.do_action(Action::SplitRight);
    f.frame(vec![]);
    f.frame(vec![]);
    let ratio = |f: &Fixture| f.app.dock.layout.as_ref().unwrap().ratio(&[]).unwrap();
    let name = "Group divider, left and right";
    let div = bounds(&f, name);
    assert_eq!(f.h.tree().find(name).map(|n| n.role), Some(Role::Splitter));
    let x0 = bounds(&f, "Terminal: alpha, group 1 of 2").x;
    let x1 = bounds(&f, "Terminal: alpha, group 2 of 2").right();
    let room = (x1 - x0 - div.w) as f32;
    let x_at = |r: f32| x0 + (room * r) as i32 + div.w / 2;
    let y = div.center().y;
    drag(&mut f, div.center(), &[Point::new(x_at(1.0 / 3.0) + 4, y)]);
    assert!((ratio(&f) - 1.0 / 3.0).abs() < 1e-6, "snapped: {}", ratio(&f));
    let div = bounds(&f, name);
    f.frame(vec![Event::MouseMove(div.center()), Event::MouseDown(MouseButton::Left, div.center())]);
    f.frame(vec![Event::Modifiers(Modifiers { alt: true, ..Modifiers::NONE }), Event::MouseMove(Point::new(x_at(0.6) + 3, y))]);
    f.frame(vec![Event::MouseUp(MouseButton::Left, Point::new(x_at(0.6) + 3, y))]);
    assert!((ratio(&f) - 0.6).abs() > 1e-3 && (ratio(&f) - 0.6).abs() < 0.02, "free: {}", ratio(&f));
    f.frame(vec![Event::Modifiers(Modifiers::NONE)]);
    let c = bounds(&f, name).center();
    let click = vec![Event::MouseMove(c), Event::MouseDown(MouseButton::Left, c), Event::MouseUp(MouseButton::Left, c)];
    f.frame(click.clone());
    f.frame(click);
    f.frame(vec![]);
    assert_eq!(ratio(&f), 0.5);
    let id = f.h.tree().find(name).unwrap().id;
    f.frame(vec![Event::A11y { target: id, action: A11yAction::Focus }]);
    f.frame(vec![key(Key::Right, Modifiers::NONE)]);
    f.frame(vec![Event::A11y { target: id, action: A11yAction::Increment }]);
    f.frame(vec![]);
    assert_eq!(f.h.tree().find(name).and_then(|n| n.value.clone()).as_deref(), Some("75%"));
}

#[test]
/// Keyboard parity: split (a new session), focus by side and in turn, move
/// tabs between and into new groups, resize, equalize and close groups;
/// shortcuts with nothing to do leave their keys to the shell.
fn keyboard_dock_actions() {
    let mut f = alpha();
    for a in [Action::MovePaneLeft, Action::ResizePaneLeft, Action::EqualizePanes, Action::FocusNextPane, Action::MoveToNewGroupRight] {
        assert!(!f.app.do_action(a), "{a:?} with one tab in one group");
    }
    let ctrl_shift = Modifiers { ctrl: true, shift: true, ..Modifiers::NONE };
    f.frame(vec![key(Key::Char('d'), ctrl_shift)]); // split right
    f.frame(vec![]);
    assert_eq!(groups(&f), vec![vec!["alpha"], vec!["alpha"]]);
    let right = f.app.tabs[1].id;
    assert_eq!(f.h.state.focused(), Some(term_id(right)), "the new session has focus");
    // Focus left (Ctrl+Alt+Left), then back with the next-group shortcut.
    f.frame(vec![key(Key::Left, Modifiers { ctrl: true, alt: true, ..Modifiers::NONE })]);
    f.frame(vec![]);
    assert_eq!(f.h.state.focused(), Some(term_id(f.app.tabs[0].id)));
    f.frame(vec![key(Key::Char('o'), ctrl_shift)]);
    f.frame(vec![]);
    assert_eq!(f.h.state.focused(), Some(term_id(right)));
    // Resize and equalize the focused group's divider.
    let alt_shift = Modifiers { alt: true, shift: true, ..Modifiers::NONE };
    f.frame(vec![key(Key::Left, alt_shift)]);
    assert_eq!(f.app.dock.layout.as_ref().unwrap().ratio(&[]), Some(1.0 / 3.0));
    f.frame(vec![key(Key::Char('+'), alt_shift)]);
    assert_eq!(f.app.dock.layout.as_ref().unwrap().ratio(&[]), Some(0.5));
    // Move a second tab of the right group into the left one, then down.
    let beta = f.app.hosts[1].clone();
    f.app.open_terminal(&beta);
    f.frame(vec![]);
    assert!(f.app.do_action(Action::MovePaneLeft));
    f.frame(vec![]);
    assert_eq!(groups(&f), vec![vec!["alpha", "beta"], vec!["alpha"]]);
    assert!(f.app.do_action(Action::MoveToNewGroupDown));
    f.frame(vec![]);
    assert_eq!(groups(&f), vec![vec!["alpha"], vec!["beta"], vec!["alpha"]]);
    assert!(!f.app.do_action(Action::MovePaneDown), "alone at the bottom edge");
    // Close group: its tabs close; with one group left only the tab closes.
    f.frame(vec![key(Key::Char('x'), ctrl_shift)]);
    f.frame(vec![]);
    assert_eq!(groups(&f), vec![vec!["alpha"], vec!["alpha"]]);
    f.app.do_action(Action::ClosePane);
    f.app.do_action(Action::ClosePane);
    f.frame(vec![]);
    assert!(f.app.tabs.is_empty());
}

#[test]
/// Groups are named regions with their own tab list; F6 visits each group's
/// strip and content; the terminal knows its group; Settings splits by moving.
fn groups_are_named_regions_reached_by_f6() {
    let mut f = alpha_beta();
    f.app.open_settings();
    f.frame(vec![]);
    assert!(f.app.do_action(Action::SplitRight), "Settings moves to a new group");
    f.frame(vec![]);
    f.frame(vec![]);
    assert_eq!(groups(&f), vec![vec!["alpha", "beta"], vec!["Settings"]]);
    assert!(!f.app.do_action(Action::SplitRight), "alone in its group: nothing to split off");
    let t = f.h.tree();
    assert_eq!(t.find("Group 1 of 2: beta").map(|n| n.role), Some(Role::Region));
    assert_eq!(t.find("Group 2 of 2: Settings").map(|n| n.role), Some(Role::Region));
    assert_eq!(t.nodes.iter().filter(|n| n.role == Role::TabList).count(), 2);
    assert!(t.find("Terminal: beta, group 1 of 2").is_some());
    // F6 from the beta terminal: group 2's strip next.
    f.app.do_action(Action::FocusGroupLeft);
    f.frame(vec![]);
    f.frame(vec![]);
    f.frame(vec![key(Key::F(6), Modifiers::NONE)]);
    f.frame(vec![]);
    let g2 = f.app.dock.order()[1];
    assert_eq!(f.h.state.focused(), Some(strip_id(g2)));
    assert_eq!(f.app.dock.focus, g2, "focus in a strip focuses its group");
}

#[test]
/// With `dim_inactive_panes` the terminal of a group without focus gets a
/// translucent veil; turned off, or in a high-contrast theme, it does not.
fn unfocused_terminal_groups_dim_unless_off_or_high_contrast() {
    let mut f = alpha();
    f.app.do_action(Action::SplitRight);
    f.frame(vec![]);
    f.frame(vec![]);
    let veils = |f: &Fixture| {
        let bg = f.app.theme.term_bg;
        f.h.list().prims.iter().filter(|p| matches!(p, Prim::Rect { color, .. } if color.a == 38 && color.with_alpha(255) == bg)).count()
    };
    assert_eq!(veils(&f), 1);
    f.app.cfg.gui.dim_inactive_panes = false;
    f.frame(vec![]);
    assert_eq!(veils(&f), 0, "off by setting");
    f.app.cfg.gui.dim_inactive_panes = true;
    f.app.apply_theme("high-contrast");
    f.frame(vec![]);
    assert_eq!(veils(&f), 0, "never in high contrast");
}

#[test]
/// After a split each group's terminal has its own pty size (same rows,
/// columns shared), and the menu's Split down acts on its own terminal.
fn each_group_terminal_gets_its_own_pty_size() {
    let fx = skiff_testserver::TestFixture::with_shell(skiff_testserver::FixtureAuth::Key).unwrap();
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let mut f = Fixture::new();
    let host = f.app.hosts[0].clone();
    f.app.push_tab(TabKind::Terminal(Box::new(TermTab::with_command(host, "ssh".into(), args, 100))));
    f.frame(vec![]);
    let grid = |f: &Fixture, i: usize| match &f.app.tabs[i].kind {
        TabKind::Terminal(t) => (t.grid, t.session.is_some(), t.resizes),
        _ => panic!(),
    };
    let whole = grid(&f, 0).0;
    f.app.do_action(Action::SplitRight);
    f.frame(vec![]);
    f.frame(vec![]);
    let ((a, live_a, resized), (b, live_b, _)) = (grid(&f, 0), grid(&f, 1));
    assert!(live_a && live_b, "a session per group");
    assert_eq!((a.0, b.0), (whole.0, whole.0), "same height");
    assert!(a.1 + b.1 <= whole.1 && a.1 + b.1 >= whole.1 - 6, "{whole:?} -> {a:?} + {b:?}");
    assert_eq!(resized, 1, "the old terminal's pty shrank to its half");
    // The menu of the left terminal splits it down.
    let r = bounds(&f, "Terminal: alpha, group 1 of 2");
    let at = Point::new(r.x + 30, r.y + 80);
    f.frame(vec![Event::MouseMove(at), Event::MouseDown(MouseButton::Right, at), Event::MouseUp(MouseButton::Right, at)]);
    f.frame(vec![]);
    f.click_text("Split down");
    assert_eq!(f.app.dock.groups.len(), 3);
    assert_eq!(f.app.dock.neighbor(f.app.dock.focus, Zone::Top), Some(f.app.dock.order()[0]));
}
