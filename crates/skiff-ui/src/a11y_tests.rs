//! Tests that widgets publish the right accessibility nodes and react to
//! screen-reader actions, plus keyboard focus movement (Tab, F6, Shift+F10)
//! and the focus indicators drawn for it.

use crate::a11y::{A11yAction, Role};
use crate::geom::Rect;
use crate::input::{Event, Key, Modifiers};
use crate::ui::Id;
use crate::ui::testing::Harness;
use crate::widgets::{TextInputOpts, button, checkbox, list_named, splitter_h, text_input};

/// A key press with no modifiers.
fn key(k: Key) -> Event {
    Event::KeyDown { key: k, mods: Modifiers::NONE, text: None }
}

/// Buttons, text inputs and checkboxes appear under the root with their
/// roles and state, and Tab focus is reflected in the tree.
#[test]
fn button_and_input_nodes() {
    let mut h = Harness::new(400, 300, 1.0);
    let (mut name, mut hidden) = (String::new(), false);
    let mut f = |ui: &mut crate::ui::Ui| {
        button(ui, Id::new("save"), Rect::new(0, 0, 80, 24), "Save");
        let opts = TextInputOpts { placeholder: "Name", ..Default::default() };
        text_input(ui, Id::new("name"), Rect::new(0, 30, 200, 24), &mut name, opts);
        checkbox(ui, Id::new("hid"), Rect::new(0, 60, 200, 24), &mut hidden, "Hidden");
    };
    h.frame(vec![], &mut f);
    let t = h.tree();
    assert_eq!(t.find("Save").map(|n| n.role), Some(Role::Button));
    assert_eq!(t.find("Name").map(|n| n.role), Some(Role::TextInput));
    let cb = t.find("Hidden").expect("checkbox node");
    assert_eq!((cb.role, cb.checked), (Role::CheckBox, Some(false)));
    assert_eq!(t.nodes[0].children.len(), 3, "widgets hang off the root");
    h.frame(vec![key(Key::Tab)], &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(h.tree().focus, Some(Id::new("save")));
}

/// A password field is published as a password input and never exposes its value.
#[test]
fn password_value_hidden() {
    let mut h = Harness::new(400, 100, 1.0);
    let mut pw = "secret".to_string();
    h.frame(vec![], &mut |ui| {
        let opts = TextInputOpts { password: true, label: "Password", ..Default::default() };
        text_input(ui, Id::new("pw"), Rect::new(0, 0, 200, 24), &mut pw, opts);
    });
    let n = h.tree().find("Password").expect("password node");
    assert_eq!(n.role, Role::PasswordInput);
    assert_eq!(n.value, None);
}

/// The splitter is a named, focusable Splitter whose value is the first
/// pane's share; arrow keys and screen-reader increment/decrement resize it.
#[test]
fn splitter_is_accessible_and_keyboard_resizable() {
    let mut h = Harness::new(1000, 300, 1.0);
    let width = std::cell::Cell::new(300.0f32);
    let mut f = |ui: &mut crate::ui::Ui| {
        let mut w = width.get();
        splitter_h(ui, Id::new("sp"), Rect::new(0, 0, 1000, 300), &mut w, 160.0, 600.0, "Sidebar width");
        width.set(w);
    };
    h.frame(vec![], &mut f);
    let n = h.tree().find("Sidebar width").expect("splitter node");
    assert_eq!(n.role, Role::Splitter);
    assert_eq!(n.value.as_deref(), Some("30%"));
    h.frame(vec![key(Key::Tab)], &mut f);
    h.frame(vec![key(Key::Right)], &mut f);
    h.frame(vec![], &mut f);
    assert_eq!(h.tree().focus, Some(Id::new("sp")));
    assert_eq!(width.get(), 316.0);
    h.frame(vec![key(Key::Left), key(Key::Left)], &mut f);
    assert_eq!(width.get(), 284.0);
    h.frame(vec![Event::A11y { target: Id::new("sp"), action: A11yAction::Increment }], &mut f);
    assert_eq!(width.get(), 300.0);
}

/// Screen-reader Click presses a button and SetValue replaces a text input's text.
#[test]
fn a11y_click_and_set_value() {
    let mut h = Harness::new(400, 100, 1.0);
    let mut text = String::new();
    let mut clicked = false;
    let mut f = |ui: &mut crate::ui::Ui| {
        clicked |= button(ui, Id::new("go"), Rect::new(0, 0, 80, 24), "Go").clicked;
        text_input(ui, Id::new("in"), Rect::new(0, 30, 200, 24), &mut text, TextInputOpts { label: "Query", ..Default::default() });
    };
    h.frame(vec![], &mut f);
    h.frame(vec![Event::A11y { target: Id::new("go"), action: A11yAction::Click }], &mut f);
    h.frame(vec![Event::A11y { target: Id::new("in"), action: A11yAction::SetValue("abc".into()) }], &mut f);
    assert!(clicked);
    assert_eq!(text, "abc");
    assert_eq!(h.tree().find("Query").and_then(|n| n.value.clone()).as_deref(), Some("abc"));
}

/// A named list publishes each row as a list item with its selection state.
#[test]
fn list_items_named_and_selected() {
    let mut h = Harness::new(400, 300, 1.0);
    let mut sel = Some(1);
    h.frame(vec![], &mut |ui| {
        let names = ["alpha", "beta", "gamma"];
        list_named(ui, Id::new("l"), Rect::new(0, 0, 200, 200), "Hosts", 3, 20, &mut sel, |i| names[i].to_string(), |_, _, _, _| {});
    });
    let t = h.tree();
    assert_eq!(t.find("Hosts").map(|n| n.role), Some(Role::List));
    let beta = t.find("beta").expect("row node");
    assert_eq!((beta.role, beta.selected), (Role::ListItem, Some(true)));
    assert_eq!(t.find("alpha").and_then(|n| n.selected), Some(false));
}

/// A key press with Shift held.
fn shift(k: Key) -> Event {
    Event::KeyDown { key: k, mods: Modifiers { shift: true, ..Modifiers::NONE }, text: None }
}

/// Tab cycles within a region, F6 / Shift+F6 jump between regions, and a
/// region restores the control that last had focus in it.
#[test]
fn f6_moves_between_regions_and_back() {
    let mut h = Harness::new(400, 300, 1.0);
    let mut f = |ui: &mut crate::ui::Ui| {
        ui.region(Id::new("left"), "Left", |ui| {
            button(ui, Id::new("a1"), Rect::new(0, 0, 80, 24), "A1");
            button(ui, Id::new("a2"), Rect::new(0, 30, 80, 24), "A2");
        });
        ui.region(Id::new("right"), "Right", |ui| {
            button(ui, Id::new("b1"), Rect::new(100, 0, 80, 24), "B1");
        });
    };
    h.frame(vec![], &mut f);
    h.frame(vec![key(Key::Tab)], &mut f);
    h.frame(vec![key(Key::Tab)], &mut f);
    assert_eq!(h.state.focused(), Some(Id::new("a2")));
    h.frame(vec![key(Key::Tab)], &mut f);
    assert_eq!(h.state.focused(), Some(Id::new("a1")), "Tab wraps within its region");
    h.frame(vec![shift(Key::Tab)], &mut f);
    assert_eq!(h.state.focused(), Some(Id::new("a2")));
    h.frame(vec![key(Key::F(6))], &mut f);
    assert_eq!(h.state.focused(), Some(Id::new("b1")));
    h.frame(vec![key(Key::Tab)], &mut f);
    assert_eq!(h.state.focused(), Some(Id::new("b1")), "a one-control region keeps Tab");
    h.frame(vec![shift(Key::F(6))], &mut f);
    assert_eq!(h.state.focused(), Some(Id::new("a2")), "a region remembers its last focus");
    assert_eq!(h.tree().find("Left").map(|n| n.role), Some(Role::Region));
}

/// A keyboard-focused button gets a 2px focus ring, 2px outside its bounds.
#[test]
fn focus_ring_drawn_for_focused_button() {
    use crate::display::Prim;
    let mut h = Harness::new(400, 300, 1.0);
    let r = Rect::new(20, 20, 80, 24);
    let mut f = |ui: &mut crate::ui::Ui| {
        button(ui, Id::new("b"), r, "B");
    };
    h.frame(vec![key(Key::Tab)], &mut f);
    h.frame(vec![], &mut f);
    let focus = h.style.palette.focus;
    let ring = h.list().prims.iter().any(|p| {
        matches!(p, Prim::Border { rect, color, width, .. } if *color == focus && *width >= 2 && rect.x <= r.x - 4 && rect.right() >= r.right() + 4)
    });
    assert!(ring, "a 2px focus ring 2px outside the button");
}

/// The selected row of a focused list shows a thin accent bar on its left edge.
#[test]
fn focused_list_row_has_left_bar() {
    use crate::display::Prim;
    let mut h = Harness::new(400, 300, 1.0);
    let mut sel = Some(1);
    let mut f = |ui: &mut crate::ui::Ui| {
        list_named(ui, Id::new("l"), Rect::new(0, 0, 200, 200), "L", 3, 20, &mut sel, |i| i.to_string(), |_, _, _, _| {});
    };
    let accent = h.style.palette.accent;
    let bar = |h: &Harness| {
        let is_bar = |r: &Rect| r.x == 0 && r.y == 20 && r.h == 20 && (2..10).contains(&r.w);
        h.list().prims.iter().any(|p| matches!(p, Prim::Rect { rect, color, .. } if *color == accent && is_bar(rect)))
    };
    h.frame(vec![], &mut f);
    assert!(!bar(&h), "no bar while the list is not focused");
    h.frame(vec![key(Key::Tab)], &mut f);
    h.frame(vec![], &mut f);
    assert!(bar(&h), "the focused row has a left bar");
}

/// The focus ring is painted before overlay content, so a menu covers it.
#[test]
fn focus_ring_stays_below_overlays() {
    use crate::display::Prim;
    let mut h = Harness::new(400, 300, 1.0);
    let menu_color = crate::color::Color::hex(0x123456);
    let mut f = |ui: &mut crate::ui::Ui| {
        button(ui, Id::new("b"), Rect::new(20, 20, 80, 24), "B");
        ui.overlay(|ui| ui.painter().rect(Rect::new(0, 0, 200, 200), menu_color, 0.0));
    };
    h.frame(vec![key(Key::Tab)], &mut f);
    h.frame(vec![], &mut f);
    let focus = h.style.palette.focus;
    let prims = &h.list().prims;
    let ring = prims.iter().position(|p| matches!(p, Prim::Border { color, width, .. } if *color == focus && *width >= 2));
    let menu = prims.iter().position(|p| matches!(p, Prim::Rect { color, .. } if *color == menu_color));
    assert!(ring.expect("ring drawn") < menu.expect("menu drawn"), "the menu covers the ring");
}

/// Shift+F10 and the Menu key ask for the selected row's context menu at
/// a point inside that row.
#[test]
fn shift_f10_reports_context_row() {
    use crate::widgets::{ListResponse, list};
    let mut h = Harness::new(400, 300, 1.0);
    let mut sel = Some(1);
    let mut run = |h: &mut Harness, ev: Vec<Event>| {
        let mut out = ListResponse::default();
        h.frame(ev, &mut |ui| out = list(ui, Id::new("l"), Rect::new(0, 0, 200, 200), 3, 20, &mut sel, |_, _, _, _| {}));
        out.context
    };
    run(&mut h, vec![key(Key::Tab)]);
    let (row, at) = run(&mut h, vec![shift(Key::F(10))]).expect("Shift+F10 opens the row's menu");
    assert_eq!(row, 1);
    assert!(at.y >= 20 && at.y <= 40, "{at:?}");
    assert_eq!(run(&mut h, vec![key(Key::ContextMenu)]).map(|c| c.0), Some(1));
}

/// The tab strip takes Tab focus, Left/Right switch tabs, and its icon-only
/// buttons have accessible names.
#[test]
fn tab_strip_takes_focus_and_arrows() {
    use crate::widgets::{TabTitle, tabs};
    let mut h = Harness::new(600, 100, 1.0);
    let t = [TabTitle { text: "one", ..TabTitle::DEFAULT }, TabTitle { text: "two", ..TabTitle::DEFAULT }];
    let mut active = 0;
    let mut run = |h: &mut Harness, ev: Vec<Event>| {
        h.frame(ev, &mut |ui| {
            tabs(ui, Id::new("tabs"), Rect::new(0, 0, 600, 30), &t, &mut active);
        });
        active
    };
    run(&mut h, vec![key(Key::Tab)]);
    let role = |h: &Harness, name: &str| h.tree().find(name).map(|n| n.role);
    assert_eq!((role(&h, "New tab"), role(&h, "Close two")), (Some(Role::Button), Some(Role::Button)), "mouse-only targets are named");
    assert_eq!(run(&mut h, vec![key(Key::Right)]), 1);
    assert_eq!(run(&mut h, vec![key(Key::Left)]), 0);
}

/// A screen-reader Focus on a row leaves focus on the list itself, so arrow
/// keys keep working.
#[test]
fn a11y_focus_on_row_keeps_list_focus() {
    let mut h = Harness::new(400, 300, 1.0);
    let mut sel = Some(0);
    let names = ["alpha", "beta", "gamma"];
    let mut run = |h: &mut Harness, ev: Vec<Event>| {
        h.frame(ev, &mut |ui| {
            list_named(ui, Id::new("l"), Rect::new(0, 0, 200, 200), "Hosts", 3, 20, &mut sel, |i| names[i].to_string(), |_, _, _, _| {});
        });
        sel
    };
    run(&mut h, vec![key(Key::Tab)]);
    let row = h.tree().find("beta").expect("row").id;
    run(&mut h, vec![Event::A11y { target: row, action: A11yAction::Focus }]);
    assert_eq!(h.state.focused(), Some(Id::new("l")), "a row is not a focus target");
    assert_eq!(run(&mut h, vec![key(Key::Down)]), Some(1), "arrows still move the list");
}

/// Repeating the same announcement toggles the invisible repeat mark so the
/// text changes each time; new text is passed through unchanged.
#[test]
fn repeated_announcement_still_changes_the_text() {
    use crate::a11y::{REPEAT_MARK, fresh_announcement};
    let a = fresh_announcement("", "done".into());
    assert_eq!(a, "done");
    let b = fresh_announcement(&a, "done".into());
    let c = fresh_announcement(&b, "done".into());
    assert!(b != a && c != b, "{a:?} {b:?} {c:?}");
    assert_eq!(b.trim_end_matches(REPEAT_MARK), "done");
    assert_eq!(fresh_announcement(&b, "other".into()), "other");
}
