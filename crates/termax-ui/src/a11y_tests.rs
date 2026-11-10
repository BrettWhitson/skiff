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
