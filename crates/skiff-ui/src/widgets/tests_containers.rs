//! Tests for container widgets: scroll areas, the virtualized list, tabs,
//! the splitter and context menus, plus a full-window render timing check.

use std::cell::Cell;
use std::time::Instant;

use super::*;
use crate::geom::{Point, Rect};
use crate::input::{Event, Key, Modifiers, MouseButton};
use crate::ui::testing::Harness;
use crate::ui::{Id, Ui};

const NONE: Modifiers = Modifiers::NONE;

/// A left click at `p` within one frame.
fn click(p: Point) -> Vec<Event> {
    vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]
}

/// A right click at `p` within one frame.
fn right_click(p: Point) -> Vec<Event> {
    vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Right, p), Event::MouseUp(MouseButton::Right, p)]
}

/// A key press without modifiers.
fn key(k: Key) -> Vec<Event> {
    vec![Event::KeyDown { key: k, mods: NONE, text: None }]
}

/// A vertical wheel scroll with the mouse inside the test areas.
fn wheel(dy: f32) -> Vec<Event> {
    vec![Event::MouseMove(Point::new(50, 50)), Event::Wheel { dx: 0.0, dy }]
}

/// The wheel scrolls and the offset clamps to the content; short content never scrolls.
#[test]
fn scroll_wheel_and_clamp() {
    let mut h = Harness::new(300, 300, 1.0);
    let off = Cell::new(-1);
    let mut f = |ui: &mut Ui| {
        scroll_area(ui, Id::new("s"), Rect::new(0, 0, 200, 200), 1000, |_ui, visible, offset| {
            assert_eq!(visible.h, 200);
            off.set(offset);
        });
    };
    h.frame(vec![], &mut f);
    assert_eq!(off.get(), 0);
    h.frame(wheel(-100.0), &mut f);
    assert_eq!(off.get(), 100);
    h.frame(wheel(-5000.0), &mut f);
    assert_eq!(off.get(), 800);
    h.frame(wheel(5000.0), &mut f);
    assert_eq!(off.get(), 0);
    // Content shorter than the view never scrolls.
    let mut g = |ui: &mut Ui| {
        scroll_area(ui, Id::new("short"), Rect::new(0, 0, 200, 200), 50, |_ui, _v, offset| off.set(offset));
    };
    h.frame(wheel(-100.0), &mut g);
    assert_eq!(off.get(), 0);
}

/// Dragging the scrollbar thumb scrolls proportionally faster than the drag.
#[test]
fn scrollbar_drag_moves_content() {
    let mut h = Harness::new(300, 300, 1.0);
    let off = Cell::new(0);
    let mut f = |ui: &mut Ui| {
        scroll_area(ui, Id::new("s"), Rect::new(0, 0, 200, 200), 1000, |_ui, _v, o| off.set(o));
    };
    h.frame(vec![], &mut f);
    // Thumb sits at the top of the right edge.
    let p = Point::new(195, 5);
    h.frame(vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p)], &mut f);
    h.frame(vec![Event::MouseMove(Point::new(195, 85))], &mut f);
    h.frame(vec![Event::MouseUp(MouseButton::Left, Point::new(195, 85))], &mut f);
    assert!(off.get() > 300, "offset {}", off.get());
}

/// A frame closure for a 100,000-row list recording how many rows were drawn,
/// the selection, and the last activated and context-clicked rows.
fn list_ui<'a>(
    calls: &'a Cell<usize>,
    sel: &'a Cell<Option<usize>>,
    activated: &'a Cell<Option<usize>>,
    context: &'a Cell<Option<usize>>,
) -> impl FnMut(&mut Ui) + 'a {
    move |ui: &mut Ui| {
        calls.set(0);
        let mut s = sel.get();
        let r = list(ui, Id::new("l"), Rect::new(0, 0, 200, 200), 100_000, 20, &mut s, |ui, row, i, _selected| {
            calls.set(calls.get() + 1);
            label(ui, row, &format!("row {i}"), None);
        });
        sel.set(s);
        if r.activated.is_some() {
            activated.set(r.activated);
        }
        if let Some((i, _)) = r.context {
            context.set(Some(i));
        }
    }
}

/// Only visible rows are drawn, at any scroll position, so frames stay fast.
#[test]
fn list_virtualizes_rows() {
    let mut h = Harness::new(300, 300, 1.0);
    let (calls, sel, act, ctx) = (Cell::new(0), Cell::new(None), Cell::new(None), Cell::new(None));
    let mut f = list_ui(&calls, &sel, &act, &ctx);
    h.frame(vec![], &mut f);
    assert!(calls.get() > 0 && calls.get() <= 12, "{} rows built", calls.get());
    h.frame(wheel(-1_000_000.0), &mut f);
    assert!(calls.get() <= 12);
    let t = Instant::now();
    for _ in 0..20 {
        h.frame(wheel(-37.0), &mut f);
    }
    assert!(t.elapsed().as_millis() < 500, "frames should not depend on row count");
}

/// Click, arrows, Page Down, Home/End, Enter, right-click and double-click
/// select and activate the expected rows, scrolling the selection into view.
#[test]
fn list_keyboard_navigation() {
    let mut h = Harness::new(300, 300, 1.0);
    let (calls, sel, act, ctx) = (Cell::new(0), Cell::new(None), Cell::new(None), Cell::new(None));
    let mut f = list_ui(&calls, &sel, &act, &ctx);
    h.frame(vec![], &mut f);
    h.frame(click(Point::new(50, 45)), &mut f);
    assert_eq!(sel.get(), Some(2));
    h.frame(key(Key::Down), &mut f);
    assert_eq!(sel.get(), Some(3));
    h.frame(key(Key::Up), &mut f);
    h.frame(key(Key::Up), &mut f);
    assert_eq!(sel.get(), Some(1));
    h.frame(key(Key::PageDown), &mut f);
    assert_eq!(sel.get(), Some(11));
    h.frame(key(Key::End), &mut f);
    assert_eq!(sel.get(), Some(99_999));
    h.frame(vec![], &mut f);
    // The selected last row is scrolled into view (and drawn).
    assert!(h.list().prims.iter().any(|p| matches!(p, crate::display::Prim::Text { text, .. } if text == "row 99999")));
    h.frame(key(Key::Home), &mut f);
    assert_eq!(sel.get(), Some(0));
    h.frame(key(Key::Enter), &mut f);
    assert_eq!(act.get(), Some(0));
    h.frame(vec![], &mut f);
    h.frame(right_click(Point::new(50, 65)), &mut f);
    assert_eq!((ctx.get(), sel.get()), (Some(3), Some(3)));
    let mut dbl = click(Point::new(50, 85));
    dbl.extend(click(Point::new(50, 85)));
    h.frame(dbl, &mut f);
    assert_eq!(act.get(), Some(4));
}

/// Tabs activate on click, reorder by dragging, close via their close
/// button (only when closable), and report the plus button.
#[test]
fn tabs_close_and_reorder() {
    let mut h = Harness::new(600, 100, 1.0);
    let titles =
        [TabTitle { text: "one", closable: true }, TabTitle { text: "two", closable: true }, TabTitle { text: "three", closable: false }];
    let active = Cell::new(0usize);
    let last = Cell::new(TabsResponse::default());
    let rects = Cell::new(Vec::<Rect>::new());
    let mut f = |ui: &mut Ui| {
        let mut a = active.get();
        let r = tabs(ui, Id::new("t"), Rect::new(0, 0, 600, 30), &titles, &mut a);
        active.set(a);
        last.set(r.clone());
        rects.set(r.tab_rects.clone());
    };
    h.frame(vec![], &mut f);
    let tr = rects.take();
    assert_eq!(tr.len(), 3);
    h.frame(click(tr[1].center()), &mut f);
    assert_eq!(active.get(), 1);
    // Drag tab 0 over tab 1.
    let p0 = tr[0].center();
    h.frame(vec![Event::MouseMove(p0), Event::MouseDown(MouseButton::Left, p0)], &mut f);
    assert_eq!(active.get(), 0);
    h.frame(vec![Event::MouseMove(tr[1].center())], &mut f);
    assert_eq!(last.take().moved, Some((0, 1)));
    assert_eq!(active.get(), 1, "the dragged tab stays active at its new index");
    h.frame(vec![Event::MouseUp(MouseButton::Left, tr[1].center())], &mut f);
    // Close button of tab 1 sits at its right edge.
    h.frame(vec![], &mut f);
    let tr = rects.take();
    let close = Point::new(tr[1].right() - 10, tr[1].center().y);
    h.frame(click(close), &mut f);
    assert_eq!(last.take().closed, Some(1));
    // Tab 2 is not closable: clicking its right edge just activates it.
    h.frame(click(Point::new(tr[2].right() - 10, tr[2].center().y)), &mut f);
    let r = last.take();
    assert_eq!((r.closed, active.get()), (None, 2));
    // The plus button.
    h.frame(click(Point::new(tr[2].right() + 15, 15)), &mut f);
    assert!(last.take().plus_clicked);
    // Dragging tab 1 off the strip reports it once and gives the tab that
    // was active before the press back its place.
    h.frame(click(tr[0].center()), &mut f);
    let p1 = tr[1].center();
    h.frame(vec![Event::MouseMove(p1), Event::MouseDown(MouseButton::Left, p1)], &mut f);
    assert_eq!(active.get(), 1);
    h.frame(vec![Event::MouseMove(Point::new(p1.x, 70))], &mut f);
    assert_eq!((last.take().dragged_out, active.get()), (Some(1), 0));
    h.frame(vec![Event::MouseMove(Point::new(p1.x + 5, 75))], &mut f);
    assert_eq!(last.take().dragged_out, None, "once per drag");
    h.frame(vec![Event::MouseUp(MouseButton::Left, Point::new(p1.x + 5, 75))], &mut f);
}

/// Dragging the splitter clamps to min/max, and the width stays logical on HiDPI.
#[test]
fn splitter_drag_respects_min_max() {
    let mut h = Harness::new(800, 400, 1.0);
    let width = Cell::new(240.0f32);
    let panes = Cell::new((Rect::default(), Rect::default()));
    let mut f = |ui: &mut Ui| {
        let mut w = width.get();
        panes.set(splitter_h(ui, Id::new("sp"), Rect::new(0, 0, 800, 400), &mut w, 150.0, 500.0, "Divider"));
        width.set(w);
    };
    h.frame(vec![], &mut f);
    let (a, b) = panes.get();
    assert_eq!(a.w, 240);
    assert!(b.x > a.right() && b.right() == 800);
    let grip = Point::new(a.right() + 2, 200);
    h.frame(vec![Event::MouseMove(grip), Event::MouseDown(MouseButton::Left, grip)], &mut f);
    h.frame(vec![Event::MouseMove(Point::new(grip.x + 60, 200))], &mut f);
    assert_eq!(width.get(), 300.0);
    h.frame(vec![Event::MouseMove(Point::new(grip.x + 1000, 200))], &mut f);
    assert_eq!(width.get(), 500.0);
    h.frame(vec![Event::MouseMove(Point::new(0, 200))], &mut f);
    assert_eq!(width.get(), 150.0);
    h.frame(vec![Event::MouseUp(MouseButton::Left, Point::new(0, 200))], &mut f);
    // HiDPI: the stored width stays logical.
    let mut h2 = Harness::new(1600, 800, 2.0);
    h2.frame(vec![], &mut f);
    assert_eq!(panes.get().0.w, 300);
}

/// Menus choose by keyboard (skipping disabled items) or click, swallow
/// clicks meant for widgets below, and close on Escape or an outside click.
#[test]
fn context_menu_select_and_dismiss() {
    let mut h = Harness::new(400, 400, 1.0);
    let items = [
        MenuItem { label: "Open", shortcut: Some("Enter"), enabled: true },
        MenuItem { label: "Rename", shortcut: None, enabled: false },
        MenuItem { label: "Delete", shortcut: Some("Del"), enabled: true },
    ];
    let chosen = Cell::new(None);
    let open_at = Cell::new(None::<Point>);
    let below = Cell::new(0);
    let menu = Id::new("menu");
    let mut f = |ui: &mut Ui| {
        if button(ui, Id::new("under"), Rect::new(0, 0, 400, 400), "bg").clicked {
            below.set(below.get() + 1);
        }
        if let Some(p) = open_at.take() {
            open_context_menu(ui, menu, p);
        }
        if let Some(i) = context_menu(ui, menu, &items) {
            chosen.set(Some(i));
        }
    };
    h.frame(vec![], &mut f);
    open_at.set(Some(Point::new(100, 100)));
    h.frame(vec![], &mut f);
    assert!(is_menu_open(&mut h, menu));
    // Keyboard: Down skips the disabled item.
    h.frame(key(Key::Down), &mut f);
    h.frame(key(Key::Down), &mut f);
    h.frame(key(Key::Enter), &mut f);
    assert_eq!(chosen.take(), Some(2));
    assert!(!is_menu_open(&mut h, menu));
    // Click an item.
    open_at.set(Some(Point::new(100, 100)));
    h.frame(vec![], &mut f);
    h.frame(vec![], &mut f);
    h.frame(click(Point::new(110, 108)), &mut f);
    assert_eq!(chosen.take(), Some(0));
    assert_eq!(below.get(), 0, "menu clicks don't reach widgets below");
    // Disabled items do nothing; Esc closes.
    open_at.set(Some(Point::new(100, 100)));
    h.frame(vec![], &mut f);
    h.frame(vec![], &mut f);
    let row_h = h.scale() as i32 * 26;
    h.frame(click(Point::new(110, 100 + row_h + 8)), &mut f);
    assert_eq!(chosen.get(), None);
    assert!(is_menu_open(&mut h, menu));
    h.frame(key(Key::Escape), &mut f);
    assert!(!is_menu_open(&mut h, menu));
    // Click outside closes without choosing.
    open_at.set(Some(Point::new(100, 100)));
    h.frame(vec![], &mut f);
    h.frame(vec![], &mut f);
    h.frame(click(Point::new(380, 380)), &mut f);
    assert!(!is_menu_open(&mut h, menu));
    assert_eq!(chosen.get(), None);
}

/// Runs an empty frame and reports whether menu `id` is open.
fn is_menu_open(h: &mut Harness, id: Id) -> bool {
    let open = Cell::new(false);
    h.frame(vec![], &mut |ui: &mut Ui| open.set(menu_is_open(ui, id)));
    open.get()
}

/// A full 1920x1080 repaint of the demo stays under 50ms.
#[test]
fn full_frame_render_time_1080p() {
    let mut h = Harness::new(1920, 1080, 1.0);
    let mut demo = crate::demo::Demo::default();
    let mut f = |ui: &mut Ui| demo.ui(ui);
    h.frame(vec![], &mut f);
    h.frame(vec![], &mut f);
    let t = Instant::now();
    let c = h.render();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!("full 1920x1080 repaint of the demo: {ms:.2} ms");
    assert_eq!(c.pixels.len(), 1920 * 1080);
    assert!(ms < 50.0, "full repaint took {ms:.1} ms");
}

/// While a context menu is open its keys do not also move or activate the list.
#[test]
fn open_menu_captures_keys_from_focused_list() {
    let mut h = Harness::new(400, 400, 1.0);
    let items = [MenuItem { label: "Open", shortcut: None, enabled: true }, MenuItem { label: "Delete", shortcut: None, enabled: true }];
    let sel = Cell::new(None::<usize>);
    let act = Cell::new(None::<usize>);
    let chosen = Cell::new(None::<usize>);
    let menu = Id::new("m");
    let mut f = |ui: &mut Ui| {
        let mut s = sel.get();
        let r = list(ui, Id::new("l"), Rect::new(0, 0, 200, 200), 50, 20, &mut s, |_, _, _, _| {});
        sel.set(s);
        if r.activated.is_some() {
            act.set(r.activated);
        }
        if let Some((_, at)) = r.context {
            open_context_menu(ui, menu, at);
        }
        if let Some(i) = context_menu(ui, menu, &items) {
            chosen.set(Some(i));
        }
    };
    h.frame(vec![], &mut f);
    h.frame(click(Point::new(50, 45)), &mut f);
    h.frame(right_click(Point::new(50, 45)), &mut f);
    h.frame(key(Key::Down), &mut f);
    h.frame(key(Key::Down), &mut f);
    h.frame(key(Key::Enter), &mut f);
    assert_eq!(chosen.get(), Some(1));
    assert_eq!(sel.get(), Some(2), "list selection must not move under an open menu");
    assert_eq!(act.get(), None, "Enter must not also activate the list row");
    // Menu closed: the list gets keys again.
    h.frame(key(Key::Down), &mut f);
    assert_eq!(sel.get(), Some(3));
}

/// A highlight left past the end of a shrunken item list is dropped, not chosen.
#[test]
fn menu_items_shrinking_while_open_does_not_panic() {
    let mut h = Harness::new(400, 400, 1.0);
    let three = [
        MenuItem { label: "a", shortcut: None, enabled: true },
        MenuItem { label: "b", shortcut: None, enabled: true },
        MenuItem { label: "c", shortcut: None, enabled: true },
    ];
    let menu = Id::new("m");
    h.frame(vec![], &mut |ui: &mut Ui| {
        open_context_menu(ui, menu, Point::new(10, 10));
        context_menu(ui, menu, &three);
    });
    h.frame(vec![Event::KeyDown { key: Key::Up, mods: NONE, text: None }], &mut |ui: &mut Ui| {
        context_menu(ui, menu, &three);
    });
    let r = std::cell::Cell::new(Some(99));
    h.frame(key(Key::Enter), &mut |ui: &mut Ui| r.set(context_menu(ui, menu, &three[..1])));
    assert_eq!(r.get(), None);
}

/// Shrinking the window (even to zero) does not change the stored splitter width.
#[test]
fn splitter_width_survives_shrink_and_restore() {
    let mut h = Harness::new(1000, 400, 1.0);
    let width = Cell::new(300.0f32);
    let mut f = |ui: &mut Ui| {
        let mut w = width.get();
        let r = ui.window_rect();
        splitter_h(ui, Id::new("sp"), r, &mut w, 160.0, 600.0, "Divider");
        width.set(w);
    };
    h.frame(vec![], &mut f);
    h.resize(0, 0);
    h.frame(vec![], &mut f);
    h.resize(200, 400);
    h.frame(vec![], &mut f);
    h.resize(1000, 400);
    h.frame(vec![], &mut f);
    assert_eq!(width.get(), 300.0);
}
