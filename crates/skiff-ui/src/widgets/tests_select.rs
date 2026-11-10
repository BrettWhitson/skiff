use std::cell::Cell;

use super::*;
use crate::geom::{Point, Rect};
use crate::input::{Event, Key, Modifiers, MouseButton};
use crate::ui::testing::Harness;
use crate::ui::{Id, Ui};

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };
const SHIFT: Modifiers = Modifiers { ctrl: false, shift: true, alt: false, logo: false };

/// Rows are 20 px tall at scale 1; row `i`'s centre.
fn row(i: i32) -> Point {
    Point::new(50, i * 20 + 10)
}

fn click_with(p: Point, mods: Modifiers) -> Vec<Event> {
    vec![Event::Modifiers(mods), Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]
}

fn keyd(k: Key, mods: Modifiers) -> Vec<Event> {
    vec![Event::KeyDown { key: k, mods, text: None }]
}

fn set(sel: &Selection) -> Vec<usize> {
    sel.iter().collect()
}

fn run_list(h: &mut Harness, sel: &mut Selection, rows: usize, events: Vec<Event>) -> SelectListResponse {
    let mut out = SelectListResponse::default();
    h.frame(events, &mut |ui: &mut Ui| {
        out = select_list(ui, Id::new("l"), Rect::new(0, 0, 200, 200), rows, 20, sel, |_, _, _, _| {});
    });
    out
}

#[test]
fn click_ctrl_shift_selection() {
    let mut h = Harness::new(300, 300, 1.0);
    let mut sel = Selection::default();
    run_list(&mut h, &mut sel, 20, vec![]);
    run_list(&mut h, &mut sel, 20, click_with(row(2), Modifiers::NONE));
    assert_eq!(set(&sel), vec![2]);
    run_list(&mut h, &mut sel, 20, click_with(row(5), CTRL));
    assert_eq!(set(&sel), vec![2, 5]);
    run_list(&mut h, &mut sel, 20, click_with(row(8), SHIFT));
    assert_eq!(set(&sel), vec![5, 6, 7, 8]);
    assert_eq!(sel.anchor, Some(5));
    // ctrl-click again toggles off
    run_list(&mut h, &mut sel, 20, click_with(row(6), CTRL));
    assert_eq!(set(&sel), vec![5, 7, 8]);
}

#[test]
fn shift_arrow_extends_and_ctrl_a_all() {
    let mut h = Harness::new(300, 300, 1.0);
    let mut sel = Selection::default();
    run_list(&mut h, &mut sel, 10, vec![]);
    run_list(&mut h, &mut sel, 10, click_with(row(3), Modifiers::NONE));
    run_list(&mut h, &mut sel, 10, keyd(Key::Down, SHIFT));
    run_list(&mut h, &mut sel, 10, keyd(Key::Down, SHIFT));
    assert_eq!(set(&sel), vec![3, 4, 5]);
    assert_eq!(sel.cursor, Some(5));
    run_list(&mut h, &mut sel, 10, keyd(Key::Up, Modifiers::NONE));
    assert_eq!(set(&sel), vec![4]);
    run_list(&mut h, &mut sel, 10, keyd(Key::Char('a'), CTRL));
    assert_eq!(sel.iter().count(), 10);
    let r = run_list(&mut h, &mut sel, 10, keyd(Key::Enter, Modifiers::NONE));
    assert_eq!(r.activated, Some(4));
}

#[test]
fn selection_clamps_when_rows_shrink() {
    let mut sel = Selection::default();
    sel.select_only(2);
    sel.toggle(9);
    sel.clamp(5);
    assert_eq!(set(&sel), vec![2]);
    assert!(sel.cursor.is_none_or(|c| c < 5));
    assert!(sel.anchor.is_none_or(|a| a < 5));
}

#[test]
fn pressing_selected_row_keeps_multi_selection_until_release() {
    let mut h = Harness::new(300, 300, 1.0);
    let mut sel = Selection::default();
    run_list(&mut h, &mut sel, 10, vec![]);
    run_list(&mut h, &mut sel, 10, click_with(row(1), Modifiers::NONE));
    run_list(&mut h, &mut sel, 10, click_with(row(3), CTRL));
    run_list(
        &mut h,
        &mut sel,
        10,
        vec![Event::Modifiers(Modifiers::NONE), Event::MouseMove(row(1)), Event::MouseDown(MouseButton::Left, row(1))],
    );
    assert_eq!(set(&sel), vec![1, 3], "press alone keeps the selection (could be a drag)");
    run_list(&mut h, &mut sel, 10, vec![Event::MouseUp(MouseButton::Left, row(1))]);
    assert_eq!(set(&sel), vec![1], "plain click without a drag selects only that row");
}

/// A list on the left, two drop targets on the right.
fn dnd_frame(h: &mut Harness, sel: &mut Selection, events: Vec<Event>, got: &mut [Option<Vec<usize>>; 2]) -> bool {
    let mut started = false;
    h.frame(events, &mut |ui: &mut Ui| {
        let r = select_list(ui, Id::new("l"), Rect::new(0, 0, 200, 200), 10, 20, sel, |_, _, _, _| {});
        if r.drag_started {
            started = true;
            ui.start_drag(Box::new(sel.iter().collect::<Vec<usize>>()), "items".into());
        }
        for (i, target) in [Rect::new(300, 0, 100, 100), Rect::new(300, 100, 100, 100)].into_iter().enumerate() {
            if let Some(v) = ui.drop_target::<Vec<usize>>(target).dropped {
                got[i] = Some(v);
            }
        }
    });
    started
}

#[test]
fn drag_from_selected_row_delivers_payload() {
    let mut h = Harness::new(500, 300, 1.0);
    let mut sel = Selection::default();
    let mut got = [None, None];
    dnd_frame(&mut h, &mut sel, vec![], &mut got);
    dnd_frame(&mut h, &mut sel, vec![Event::MouseMove(row(1)), Event::MouseDown(MouseButton::Left, row(1))], &mut got);
    let started = dnd_frame(&mut h, &mut sel, vec![Event::MouseMove(Point::new(60, 30))], &mut got);
    assert!(started, "moving 10 px with the row pressed starts a drag");
    assert!(h.state.dragging());
    let again = dnd_frame(&mut h, &mut sel, vec![Event::MouseMove(Point::new(350, 50))], &mut got);
    assert!(!again, "drag_started fires once");
    dnd_frame(&mut h, &mut sel, vec![Event::MouseUp(MouseButton::Left, Point::new(350, 50))], &mut got);
    assert_eq!(got[0], Some(vec![1]));
    assert_eq!(got[1], None);
    assert!(!h.state.dragging());
}

#[test]
fn escape_cancels_drag() {
    let mut h = Harness::new(500, 300, 1.0);
    let mut sel = Selection::default();
    let mut got = [None, None];
    dnd_frame(&mut h, &mut sel, vec![], &mut got);
    dnd_frame(&mut h, &mut sel, vec![Event::MouseMove(row(1)), Event::MouseDown(MouseButton::Left, row(1))], &mut got);
    dnd_frame(&mut h, &mut sel, vec![Event::MouseMove(Point::new(350, 50))], &mut got);
    dnd_frame(&mut h, &mut sel, keyd(Key::Escape, Modifiers::NONE), &mut got);
    assert!(!h.state.dragging());
    dnd_frame(&mut h, &mut sel, vec![Event::MouseUp(MouseButton::Left, Point::new(350, 50))], &mut got);
    assert_eq!(got, [None, None]);
}

#[test]
fn dropdown_changes_selection() {
    let mut h = Harness::new(400, 300, 1.0);
    let mut selected = 0usize;
    let mut changed = false;
    let mut f = |ui: &mut Ui| {
        changed |= dropdown(ui, Id::new("d"), Rect::new(10, 10, 150, 28), &["Local", "web-01", "db-02"], &mut selected);
    };
    h.frame(vec![], &mut f);
    let p = Point::new(50, 20);
    h.frame(vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)], &mut f);
    h.frame(vec![], &mut f);
    // menu opens below the button (y = 38); rows are 26 px; pick item 2
    let item = Point::new(50, 38 + 2 * 26 + 13);
    h.frame(vec![Event::MouseMove(item), Event::MouseDown(MouseButton::Left, item), Event::MouseUp(MouseButton::Left, item)], &mut f);
    assert_eq!(selected, 2);
    assert!(changed);
}

#[test]
fn select_list_virtualizes() {
    let mut h = Harness::new(300, 300, 1.0);
    let mut sel = Selection::default();
    let calls = Cell::new(0);
    h.frame(vec![], &mut |ui: &mut Ui| {
        select_list(ui, Id::new("l"), Rect::new(0, 0, 200, 200), 100_000, 20, &mut sel, |_, _, _, _| calls.set(calls.get() + 1));
    });
    assert!(calls.get() <= 11, "{} rows drawn", calls.get());
}

#[test]
fn progress_bar_fills_fraction() {
    let mut h = Harness::new(200, 40, 1.0);
    h.frame(vec![], &mut |ui: &mut Ui| progress_bar(ui, Rect::new(0, 10, 200, 10), Some(0.5)));
    let c = h.render();
    let accent = h.style.palette.accent.to_u32();
    assert_eq!(c.pixel(50, 15), accent);
    assert_ne!(c.pixel(150, 15), accent);
}

#[test]
fn modifiers_event_updates_input_mods() {
    let mut h = Harness::new(100, 100, 1.0);
    let mut seen = Modifiers::NONE;
    h.frame(vec![Event::Modifiers(CTRL)], &mut |ui: &mut Ui| seen = ui.input().mods);
    assert!(seen.ctrl);
}
