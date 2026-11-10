use crate::a11y::{A11yAction, Role};
use crate::geom::Rect;
use crate::input::{Event, Key, Modifiers};
use crate::ui::Id;
use crate::ui::testing::Harness;
use crate::widgets::{TextInputOpts, button, checkbox, list_named, text_input};

fn key(k: Key) -> Event {
    Event::KeyDown { key: k, mods: Modifiers::NONE, text: None }
}

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

fn shift(k: Key) -> Event {
    Event::KeyDown { key: k, mods: Modifiers { shift: true, ..Modifiers::NONE }, text: None }
}

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
    h.frame(vec![key(Key::F(6))], &mut f);
    assert_eq!(h.state.focused(), Some(Id::new("b1")));
    h.frame(vec![shift(Key::F(6))], &mut f);
    assert_eq!(h.state.focused(), Some(Id::new("a2")), "a region remembers its last focus");
    assert_eq!(h.tree().find("Left").map(|n| n.role), Some(Role::Region));
}

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
    let accent = h.style.palette.accent;
    let ring = h.list().prims.iter().any(|p| {
        matches!(p, Prim::Border { rect, color, width, .. } if *color == accent && *width >= 2 && rect.x < r.x && rect.right() > r.right())
    });
    assert!(ring, "a 2px accent ring outside the button");
}

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

#[test]
fn tab_strip_takes_focus_and_arrows() {
    use crate::widgets::{TabTitle, tabs};
    let mut h = Harness::new(600, 100, 1.0);
    let t = [TabTitle { text: "one", closable: true }, TabTitle { text: "two", closable: true }];
    let mut active = 0;
    let mut run = |h: &mut Harness, ev: Vec<Event>| {
        h.frame(ev, &mut |ui| {
            tabs(ui, Id::new("tabs"), Rect::new(0, 0, 600, 30), &t, &mut active);
        });
        active
    };
    run(&mut h, vec![key(Key::Tab)]);
    assert_eq!(run(&mut h, vec![key(Key::Right)]), 1);
    assert_eq!(run(&mut h, vec![key(Key::Left)]), 0);
}
