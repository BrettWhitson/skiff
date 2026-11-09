use std::cell::{Cell, RefCell};

use super::*;
use crate::color::Color;
use crate::display::Prim;
use crate::geom::{Point, Rect};
use crate::input::{Event, Key, Modifiers, MouseButton};
use crate::ui::testing::Harness;
use crate::ui::{Id, Ui};

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };
const SHIFT: Modifiers = Modifiers { ctrl: false, shift: true, alt: false, logo: false };
const NONE: Modifiers = Modifiers::NONE;

fn click(p: Point) -> Vec<Event> {
    vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]
}

fn key(k: Key, mods: Modifiers) -> Vec<Event> {
    vec![Event::KeyDown { key: k, mods, text: None }]
}

fn text(s: &str) -> Vec<Event> {
    vec![Event::Text(s.into())]
}

fn texts(h: &Harness) -> Vec<String> {
    h.list().prims.iter().filter_map(|p| if let Prim::Text { text, .. } = p { Some(text.clone()) } else { None }).collect()
}

#[test]
fn button_click_and_keyboard_activate() {
    let mut h = Harness::new(300, 100, 1.0);
    let n = Cell::new(0);
    let mut f = |ui: &mut Ui| {
        if button(ui, Id::new("ok"), Rect::new(10, 10, 80, 30), "OK").clicked {
            n.set(n.get() + 1);
        }
    };
    h.frame(vec![], &mut f);
    h.frame(click(Point::new(20, 20)), &mut f);
    assert_eq!(n.get(), 1);
    h.frame(key(Key::Space, NONE), &mut f);
    h.frame(key(Key::Enter, NONE), &mut f);
    assert_eq!(n.get(), 3);
    assert!(texts(&h).iter().any(|t| t == "OK"));
}

struct Field {
    value: RefCell<String>,
    changed: Cell<bool>,
    submitted: Cell<bool>,
}

impl Field {
    fn new(v: &str) -> Self {
        Field { value: RefCell::new(v.into()), changed: Cell::new(false), submitted: Cell::new(false) }
    }
    fn get(&self) -> String {
        self.value.borrow().clone()
    }
}

fn field_ui(field: &Field, password: bool) -> impl FnMut(&mut Ui) + '_ {
    move |ui: &mut Ui| {
        let r = text_input(
            ui,
            Id::new("field"),
            Rect::new(10, 10, 200, 28),
            &mut field.value.borrow_mut(),
            TextInputOpts { password, placeholder: "type here" },
        );
        field.changed.set(r.changed);
        field.submitted.set(r.submitted);
    }
}

/// Renders a frame, then clicks the field to focus it.
fn focused(h: &mut Harness, f: &mut impl FnMut(&mut Ui)) {
    h.frame(vec![], f);
    h.frame(click(Point::new(15, 20)), f);
}

#[test]
fn text_input_typing_and_selection() {
    let mut h = Harness::new(300, 100, 1.0);
    let field = Field::new("");
    let mut f = field_ui(&field, false);
    h.frame(vec![], &mut f);
    assert!(texts(&h).iter().any(|t| t == "type here"));
    focused(&mut h, &mut f);
    h.frame(text("hello"), &mut f);
    assert_eq!(field.get(), "hello");
    assert!(field.changed.get());
    let mut sel = key(Key::Left, SHIFT);
    sel.extend(key(Key::Left, SHIFT));
    h.frame(sel, &mut f);
    assert!(!field.changed.get());
    h.frame(text("p"), &mut f);
    assert_eq!(field.get(), "help");
    h.frame(key(Key::Home, NONE), &mut f);
    h.frame(text("x"), &mut f);
    assert_eq!(field.get(), "xhelp");
    h.frame(key(Key::End, NONE), &mut f);
    h.frame(text("!"), &mut f);
    assert_eq!(field.get(), "xhelp!");
    h.frame(key(Key::Enter, NONE), &mut f);
    assert!(field.submitted.get());
    // Unfocused fields ignore typing.
    h.frame(click(Point::new(290, 90)), &mut f);
    h.frame(text("zzz"), &mut f);
    assert_eq!(field.get(), "xhelp!");
}

#[test]
fn text_input_click_places_cursor() {
    let mut h = Harness::new(300, 100, 1.0);
    let field = Field::new("abcdef");
    let mut f = field_ui(&field, false);
    h.frame(vec![], &mut f);
    let x0 = h
        .list()
        .prims
        .iter()
        .find_map(|p| if let Prim::Text { origin, .. } = p { Some(origin.x) } else { None })
        .unwrap();
    // Between 'c' and 'd' (8px glyphs).
    h.frame(click(Point::new(x0 + 3 * 8 + 1, 20)), &mut f);
    h.frame(text("_"), &mut f);
    assert_eq!(field.get(), "abc_def");
}

#[test]
fn text_input_clipboard_cut_copy_paste() {
    let mut h = Harness::new(300, 100, 1.0);
    let field = Field::new("hello world");
    let mut f = field_ui(&field, false);
    focused(&mut h, &mut f);
    h.frame(key(Key::Char('a'), CTRL), &mut f);
    h.frame(key(Key::Char('c'), CTRL), &mut f);
    assert_eq!(h.clipboard.text.as_deref(), Some("hello world"));
    h.frame(key(Key::Char('x'), CTRL), &mut f);
    assert_eq!(field.get(), "");
    h.frame(key(Key::Char('v'), CTRL), &mut f);
    h.frame(vec![Event::Paste("!\nline2".into())], &mut f);
    assert_eq!(field.get(), "hello world! line2", "newlines become spaces");
}

#[test]
fn text_input_unicode_editing() {
    let mut h = Harness::new(300, 100, 1.0);
    let field = Field::new("héllo wörld");
    let mut f = field_ui(&field, false);
    focused(&mut h, &mut f);
    h.frame(key(Key::End, NONE), &mut f);
    h.frame(key(Key::Backspace, CTRL), &mut f);
    assert_eq!(field.get(), "héllo ");
    let mut ev = key(Key::Left, NONE);
    ev.extend(key(Key::Left, NONE));
    ev.extend(key(Key::Delete, NONE));
    h.frame(ev, &mut f);
    assert_eq!(field.get(), "héll ");
    let mut ev = key(Key::Backspace, NONE);
    ev.extend(key(Key::Backspace, NONE));
    ev.extend(text("ü日"));
    h.frame(ev, &mut f);
    assert_eq!(field.get(), "héü日 ");
    h.frame(key(Key::Left, CTRL), &mut f);
    h.frame(key(Key::Delete, CTRL), &mut f);
    assert_eq!(field.get(), " ");
    // Selection spanning multibyte chars.
    *field.value.borrow_mut() = "ééé".into();
    h.frame(key(Key::Char('a'), CTRL), &mut f);
    h.frame(text("ö"), &mut f);
    assert_eq!(field.get(), "ö");
}

#[test]
fn password_input_never_paints_plaintext() {
    let mut h = Harness::new(300, 100, 1.0);
    let field = Field::new("");
    let mut f = field_ui(&field, true);
    focused(&mut h, &mut f);
    h.frame(text("s3cret"), &mut f);
    assert_eq!(field.get(), "s3cret");
    let t = texts(&h);
    assert!(t.iter().all(|t| !t.contains("s3cret") && !t.contains('s')), "{t:?}");
    assert!(t.iter().any(|t| t.chars().count() == 6), "masked text drawn: {t:?}");
    // Copy and cut are disabled for passwords.
    h.frame(key(Key::Char('a'), CTRL), &mut f);
    h.frame(key(Key::Char('c'), CTRL), &mut f);
    h.frame(key(Key::Char('x'), CTRL), &mut f);
    assert_eq!(h.clipboard.text, None);
    assert_eq!(field.get(), "s3cret");
}

#[test]
fn checkbox_toggles() {
    let mut h = Harness::new(300, 100, 1.0);
    let v = Cell::new(false);
    let mut f = |ui: &mut Ui| {
        let mut b = v.get();
        checkbox(ui, Id::new("cb"), Rect::new(10, 10, 150, 24), &mut b, "Show hidden");
        v.set(b);
    };
    h.frame(vec![], &mut f);
    h.frame(click(Point::new(15, 20)), &mut f);
    assert!(v.get());
    h.frame(key(Key::Space, NONE), &mut f);
    assert!(!v.get());
    h.frame(click(Point::new(100, 20)), &mut f);
    assert!(v.get(), "clicking the label toggles too");
}

#[test]
fn label_truncates_with_ellipsis() {
    let mut h = Harness::new(300, 100, 1.0);
    h.frame(vec![], &mut |ui: &mut Ui| {
        label(ui, Rect::new(0, 0, 40, 20), "abcdefghij", None);
        label(ui, Rect::new(0, 20, 200, 20), "short", Some(Color::WHITE));
    });
    assert_eq!(texts(&h), vec!["abcd…".to_string(), "short".to_string()]);
}

#[test]
fn zero_size_frame_is_noop() {
    let mut h = Harness::new(0, 0, 1.0);
    let field = Field::new("abc");
    let mut f = |ui: &mut Ui| {
        let r = ui.rect();
        button(ui, Id::new("b"), r, "B");
        label(ui, r, "label", None);
        let mut c = true;
        checkbox(ui, Id::new("c"), r, &mut c, "c");
        text_input(ui, Id::new("t"), r, &mut field.value.borrow_mut(), TextInputOpts::default());
    };
    h.frame(vec![], &mut f);
    h.frame(click(Point::new(0, 0)), &mut f);
    h.frame(text("x"), &mut f);
    assert!(h.render().pixels.is_empty());
}
