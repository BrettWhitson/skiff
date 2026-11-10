use std::cell::Cell;

use super::testing::Harness;
use super::*;
use crate::input::{Event, Key, Modifiers, MouseButton};
use crate::layout::Size as L;

fn press(p: Point) -> Vec<Event> {
    vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p)]
}

fn release(p: Point) -> Vec<Event> {
    vec![Event::MouseMove(p), Event::MouseUp(MouseButton::Left, p)]
}

fn tab(shift: bool) -> Vec<Event> {
    let mods = Modifiers { shift, ..Modifiers::NONE };
    vec![Event::KeyDown { key: Key::Tab, mods, text: None }]
}

#[test]
fn click_requires_press_and_release_inside() {
    let mut h = Harness::new(200, 200, 1.0);
    let clicks = Cell::new(0);
    let r = Rect::new(10, 10, 50, 30);
    let mut f = |ui: &mut Ui| {
        if ui.interact(Id::new("b"), r, Sense::click()).clicked {
            clicks.set(clicks.get() + 1);
        }
    };
    h.frame(vec![], &mut f);
    h.frame(press(Point::new(20, 20)), &mut f);
    h.frame(release(Point::new(20, 20)), &mut f);
    assert_eq!(clicks.get(), 1);
    // Press inside, release outside: no click.
    h.frame(press(Point::new(20, 20)), &mut f);
    h.frame(release(Point::new(150, 150)), &mut f);
    // Press outside, release inside: no click.
    h.frame(press(Point::new(150, 150)), &mut f);
    h.frame(release(Point::new(20, 20)), &mut f);
    assert_eq!(clicks.get(), 1);
    // Press and release within one frame still counts.
    let mut both = press(Point::new(30, 30));
    both.extend(release(Point::new(30, 30)));
    h.frame(both, &mut f);
    assert_eq!(clicks.get(), 2);
}

#[test]
fn overlay_blocks_hits_below() {
    let mut h = Harness::new(300, 300, 1.0);
    let clicked = Cell::new(false);
    let mut f = |ui: &mut Ui| {
        if ui.interact(Id::new("under"), Rect::new(0, 0, 100, 100), Sense::click()).clicked {
            clicked.set(true);
        }
        ui.overlay(|ui| {
            ui.interact(Id::new("popup"), Rect::new(0, 0, 200, 200), Sense::hover());
        });
    };
    h.frame(vec![], &mut f);
    h.frame(press(Point::new(50, 50)), &mut f);
    h.frame(release(Point::new(50, 50)), &mut f);
    assert!(!clicked.get());
}

#[test]
fn tab_moves_focus_in_order() {
    let mut h = Harness::new(300, 300, 1.0);
    let focused = Cell::new(None::<&'static str>);
    let mut f = |ui: &mut Ui| {
        focused.set(None);
        for (i, name) in ["a", "b", "c"].into_iter().enumerate() {
            let r = ui.interact(Id::new(name), Rect::new(0, i as i32 * 20, 50, 20), Sense::focusable());
            if r.focused {
                focused.set(Some(name));
            }
        }
    };
    h.frame(vec![], &mut f);
    assert_eq!(focused.get(), None);
    h.frame(tab(false), &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(focused.get(), Some("a"));
    h.frame(tab(false), &mut f);
    h.frame(tab(false), &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(focused.get(), Some("c"));
    h.frame(tab(false), &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(focused.get(), Some("a"), "wraps around");
    h.frame(tab(true), &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(focused.get(), Some("c"), "shift+tab goes back");
    // A focused widget that takes Tab keeps focus.
    let mut g = |ui: &mut Ui| {
        let id = Id::new("c");
        ui.interact(id, Rect::new(0, 40, 50, 20), Sense::focusable());
        if ui.has_focus(id) {
            ui.input_mut().take_key(&Key::Tab, Modifiers::NONE);
        }
    };
    h.frame(tab(false), &mut g);
    h.frame(vec![], &mut f);
    assert_eq!(focused.get(), Some("c"));
}

#[test]
fn memory_persists_across_frames() {
    let mut h = Harness::new(100, 100, 1.0);
    let seen = Cell::new(0);
    let mut f = |ui: &mut Ui| {
        let n = ui.memory::<u32>(Id::new("counter"));
        *n += 1;
        seen.set(*n);
    };
    for _ in 0..3 {
        h.frame(vec![], &mut f);
    }
    assert_eq!(seen.get(), 3);
}

#[test]
fn hit_test_matches_scaled_layout() {
    let mut h = Harness::new(600, 300, 1.5);
    let clicked = Cell::new(false);
    let rect = Cell::new(Rect::default());
    let mut f = |ui: &mut Ui| {
        let cols = ui.split_h(ui.rect(), &[L::Px(100), L::Fill], 0);
        rect.set(cols[0]);
        if ui.interact(Id::new("left"), cols[0], Sense::click()).clicked {
            clicked.set(true);
        }
    };
    h.frame(vec![], &mut f);
    assert_eq!(rect.get().w, 150);
    let c = Point::new(rect.get().x + 140, rect.get().y + 10);
    h.frame(press(c), &mut f);
    h.frame(release(c), &mut f);
    assert!(clicked.get());
    assert_eq!(h.scale(), 1.5);
}

#[test]
fn drag_reports_deltas_and_clip_limits_hits() {
    let mut h = Harness::new(300, 300, 1.0);
    let drag = Cell::new(None::<Point>);
    let hit_outside_clip = Cell::new(false);
    let mut f = |ui: &mut Ui| {
        let r = ui.interact(Id::new("handle"), Rect::new(0, 0, 50, 50), Sense::drag());
        if r.dragged.is_some() {
            drag.set(r.dragged);
        }
        ui.with_clip(Rect::new(100, 100, 50, 50), |ui| {
            // Only the part inside the clip is hittable.
            if ui.interact(Id::new("clipped"), Rect::new(100, 100, 150, 150), Sense::click()).clicked {
                hit_outside_clip.set(true);
            }
        });
    };
    h.frame(vec![], &mut f);
    h.frame(press(Point::new(10, 10)), &mut f);
    h.frame(vec![Event::MouseMove(Point::new(25, 13))], &mut f);
    assert_eq!(drag.get(), Some(Point::new(15, 3)));
    h.frame(release(Point::new(25, 13)), &mut f);
    h.frame(press(Point::new(200, 200)), &mut f);
    h.frame(release(Point::new(200, 200)), &mut f);
    assert!(!hit_outside_clip.get());
}

#[test]
fn painter_text_records_bounds_and_overlay_draws_last() {
    let mut h = Harness::new(200, 100, 1.0);
    h.frame(vec![], &mut |ui: &mut Ui| {
        ui.overlay(|ui| ui.painter().rect(Rect::new(0, 0, 10, 10), Color::WHITE, 0.0));
        let w = ui.painter().text(Point::new(5, 5), "abc", FontKind::Ui, 14.0, Color::WHITE);
        assert_eq!(w, 24);
    });
    let prims = &h.list().prims;
    match &prims[0] {
        Prim::Text { bounds, .. } => assert_eq!(*bounds, Rect::new(5, 5, 24, 16)),
        p => panic!("expected text first, got {p:?}"),
    }
    assert!(matches!(prims.last(), Some(Prim::Rect { .. })));
}
