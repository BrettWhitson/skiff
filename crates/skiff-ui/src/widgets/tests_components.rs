//! Tests for the design-system components driven through the `Harness`:
//! the dialog's focus trap, the stepper's keys, sortable table headers,
//! menu separators, the toast live region and reduced motion.

use std::cell::{Cell, RefCell};

use super::*;
use crate::a11y::{Role, SortDir};
use crate::geom::{Point, Rect};
use crate::input::{Event, Key, Modifiers, MouseButton};
use crate::ui::testing::Harness;
use crate::ui::{Id, Ui, trap_focus};

/// A single key press without modifiers.
fn key(k: Key) -> Vec<Event> {
    vec![Event::KeyDown { key: k, mods: Modifiers::NONE, text: None }]
}

/// A left click at `p` within one frame.
fn click(p: Point) -> Vec<Event> {
    vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]
}

/// Tab steps wrap inside the trap; focus from outside is pulled in.
#[test]
fn trap_focus_wraps_and_pulls_in() {
    let (a, b, c, out) = (Id(1), Id(2), Id(3), Id(9));
    let order = [a, b, c];
    assert_eq!(trap_focus(&order, Some(c), Some(true), None), Some(a));
    assert_eq!(trap_focus(&order, Some(a), Some(false), None), Some(c));
    assert_eq!(trap_focus(&order, Some(out), None, Some(b)), Some(b));
    assert_eq!(trap_focus(&order, None, Some(true), None), Some(a));
    assert_eq!(trap_focus(&[], Some(a), Some(true), None), None);
}

/// A dialog focuses its initial button, keeps Tab inside itself (the
/// button behind it is never reached), answers Escape with `Cancelled`
/// and gives focus back to where it was, also when it is dismissed
/// unanswered (`dialog_dismiss`).
#[test]
fn dialog_traps_focus_and_restores_it() {
    let mut h = Harness::new(800, 600, 1.0);
    let open = Cell::new(false);
    let dismiss = Cell::new(false);
    let result = Cell::new(None);
    let behind = Id::new("behind");
    let buttons =
        [DialogButton { label: "Cancel", kind: ButtonKind::Secondary }, DialogButton { label: "Delete", kind: ButtonKind::Danger }];
    let mut f = |ui: &mut Ui| {
        button(ui, behind, Rect::new(10, 10, 80, 30), "Behind");
        if open.get() {
            let d = Dialog { title: "Delete?", body: "Gone for good.", buttons: &buttons, initial: 0, ..Default::default() };
            if let Some(r) = dialog(ui, Id::new("dlg"), &d) {
                open.set(false);
                result.set(Some(r));
            }
        } else if dismiss.take() {
            dialog_dismiss(ui, Id::new("dlg"));
        }
    };
    h.frame(key(Key::Tab), &mut f);
    assert_eq!(h.state.focused(), Some(behind));
    open.set(true);
    h.frame(vec![], &mut f);
    let cancel = Id::new("dlg").with(("button", 0usize));
    assert_eq!(h.state.focused(), Some(cancel));
    for _ in 0..5 {
        h.frame(key(Key::Tab), &mut f);
        assert_ne!(h.state.focused(), Some(behind), "Tab escaped the dialog");
    }
    // A click on the scrim does not move focus out either.
    h.frame(click(Point::new(5, 590)), &mut f);
    assert_ne!(h.state.focused(), Some(behind));
    let node = h.tree().find("Delete?").expect("dialog node");
    assert!(node.modal && node.role == Role::AlertDialog, "{node:?}");
    h.frame(key(Key::Escape), &mut f);
    assert_eq!(result.get(), Some(DialogResult::Cancelled));
    h.frame(vec![], &mut f);
    assert_eq!(h.state.focused(), Some(behind), "focus returns after closing");
    // Reopened, then no longer drawn without an answer: dismissing restores focus.
    open.set(true);
    h.frame(vec![], &mut f);
    assert_eq!(h.state.focused(), Some(cancel));
    open.set(false);
    dismiss.set(true);
    h.frame(vec![], &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(h.state.focused(), Some(behind), "focus returns after a dismiss");
}

/// Up/Down and Page Up/Down step a focused stepper within its range, and
/// it is announced as a spin button with its value.
#[test]
fn stepper_keys_step_and_clamp() {
    let mut h = Harness::new(400, 200, 1.0);
    let v = Cell::new(5i64);
    let mut f = |ui: &mut Ui| {
        let mut x = v.get();
        stepper(ui, Id::new("s"), Rect::new(10, 10, 140, 30), &mut x, StepperOpts { min: 0, max: 20, step: 2, label: "Count" });
        v.set(x);
    };
    h.frame(key(Key::Tab), &mut f);
    h.frame(key(Key::Up), &mut f);
    assert_eq!(v.get(), 7);
    h.frame(key(Key::PageUp), &mut f);
    assert_eq!(v.get(), 20);
    h.frame(key(Key::Down), &mut f);
    assert_eq!(v.get(), 18);
    // The − button (left of +) steps down by one step.
    h.frame(click(Point::new(150 - 2 - 26 - 13, 25)), &mut f);
    assert_eq!(v.get(), 16);
    h.frame(vec![], &mut f);
    let n = h.tree().find("Count").expect("spin button node");
    assert_eq!((n.role, n.value.as_deref()), (Role::SpinButton, Some("16")));
}

/// Clicking a sortable header sorts by it, again reverses; the header
/// node reports the direction.
#[test]
fn table_header_toggles_sort() {
    let mut h = Harness::new(600, 200, 1.0);
    let sort = RefCell::new(None::<Sort>);
    let mut f = |ui: &mut Ui| {
        let cols = [Column::new("Name", crate::layout::Size::Fill), Column::new("Size", crate::layout::Size::Px(100))];
        table_header(ui, Id::new("t"), Rect::new(0, 0, 600, 28), &cols, &mut sort.borrow_mut());
    };
    h.frame(vec![], &mut f);
    h.frame(click(Point::new(550, 14)), &mut f);
    assert_eq!(*sort.borrow(), Some(Sort { column: 1, dir: SortDir::Ascending }));
    h.frame(click(Point::new(550, 14)), &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(sort.borrow().map(|s| s.dir), Some(SortDir::Descending));
    assert_eq!(h.tree().find("Size").and_then(|n| n.sort), Some(SortDir::Descending));
}

/// Arrow keys skip separators; a chosen danger item reports its index.
#[test]
fn menu_skips_separators() {
    let mut h = Harness::new(400, 400, 1.0);
    let items = [MenuItem::new("Open"), MenuItem::SEPARATOR, MenuItem::new("Delete").danger()];
    let chosen = Cell::new(None);
    let mut f = |ui: &mut Ui| {
        if let Some(i) = context_menu(ui, Id::new("m"), &items) {
            chosen.set(Some(i));
        }
    };
    h.frame(vec![], &mut |ui: &mut Ui| open_context_menu(ui, Id::new("m"), Point::new(10, 10)));
    h.frame(key(Key::Down), &mut f);
    h.frame(key(Key::Down), &mut f);
    h.frame(key(Key::Enter), &mut f);
    assert_eq!(chosen.get(), Some(2));
}

/// Toasts are polite live-region nodes; dismissing one reports it.
#[test]
fn toasts_are_live_and_dismissable() {
    let mut h = Harness::new(800, 600, 1.0);
    let q = RefCell::new(Toasts::default());
    let id = q.borrow_mut().push(Toast::new(ToastKind::Error, "Upload failed"));
    let ev = Cell::new(None);
    let mut f = |ui: &mut Ui| {
        if let Some(e) = toasts(ui, Id::new("toasts"), &mut q.borrow_mut()) {
            ev.set(Some(e));
        }
    };
    h.frame(vec![], &mut f);
    let n = h.tree().find("Upload failed").expect("toast node");
    assert!(n.live && n.role == Role::Status);
    let close = h.tree().find("Dismiss notification").expect("close button").bounds;
    h.frame(click(close.center()), &mut f);
    assert_eq!(ev.get(), Some(ToastEvent::Dismissed(id)));
    assert!(q.borrow().is_empty());
}

/// The connecting dot animates (asks for repaints) unless reduced motion
/// is on; its text alternative is always published.
#[test]
fn status_dot_respects_reduced_motion() {
    let mut h = Harness::new(200, 100, 1.0);
    let mut f = |ui: &mut Ui| status_dot(ui, Id::new("d"), Rect::new(0, 0, 20, 20), Status::Connecting, "Connecting");
    h.frame(vec![], &mut f);
    assert!(h.repaint_in.is_some(), "pulse animates");
    h.style.reduce_motion = true;
    h.frame(vec![], &mut f);
    assert_eq!(h.repaint_in, None, "no animation under reduced motion");
    assert_eq!(h.tree().find("Connecting").map(|n| n.role), Some(Role::Image));
}

/// Typing into a stepper commits nothing until Enter (clamped then), so
/// "150" never passes through a clamped "1" → 50; blur commits too.
#[test]
fn stepper_commits_typed_value_on_enter_or_blur() {
    let mut h = Harness::new(400, 200, 1.0);
    let seen = RefCell::new(vec![]);
    let v = Cell::new(100i64);
    let mut f = |ui: &mut Ui| {
        let mut x = v.get();
        if stepper(ui, Id::new("s"), Rect::new(10, 10, 140, 30), &mut x, StepperOpts { min: 50, max: 200, step: 1, label: "Zoom" }) {
            seen.borrow_mut().push(x);
        }
        v.set(x);
        button(ui, Id::new("next"), Rect::new(10, 60, 80, 30), "Next"); // a Tab stop to blur to
    };
    h.frame(key(Key::Tab), &mut f);
    let ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
    h.frame(vec![Event::KeyDown { key: Key::Char('a'), mods: ctrl, text: None }], &mut f);
    for c in ["1", "5", "0"] {
        h.frame(vec![Event::Text(c.into())], &mut f);
    }
    assert!(seen.borrow().is_empty(), "nothing applied while typing: {:?}", seen.borrow());
    h.frame(key(Key::Enter), &mut f);
    assert_eq!(*seen.borrow(), vec![150]);
    // Out-of-range text clamps when focus leaves.
    h.frame(vec![Event::KeyDown { key: Key::Char('a'), mods: ctrl, text: None }], &mut f);
    h.frame(vec![Event::Text("999".into())], &mut f);
    h.frame(key(Key::Tab), &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(*seen.borrow(), vec![150, 200]);
}
