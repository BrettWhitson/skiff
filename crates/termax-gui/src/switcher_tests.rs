use termax_ui::input::{Event, Key, Modifiers};

use super::*;
use crate::app::tests::Fixture;

const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, alt: false, logo: false };

fn hosts() -> Vec<HostEntry> {
    let cfg = termax_core::config::Config::parse(
        "[settings]\nload_ssh_config=false\n[[connection]]\nname=\"alpha\"\nhost=\"a\"\n[[connection]]\nname=\"beta\"\nhost=\"b\"\n",
    )
    .unwrap();
    termax_core::hosts::collect(&cfg).0
}

#[test]
fn rank_prefers_hosts_by_fuzzy() {
    let r = rank("bet", &hosts(), &[], &["dracula".into()]);
    assert_eq!(r[0].0, Item::Host(1), "{:?}", r.iter().map(|x| &x.1).collect::<Vec<_>>());
    assert!(r.iter().all(|(i, _, _)| !matches!(i, Item::Host(0))));
    let r = rank("drac", &hosts(), &[], &["dracula".into(), "nord".into()]);
    assert_eq!(r[0].0, Item::Theme("dracula".into()));
}

#[test]
fn empty_query_lists_tabs_first() {
    let tabs = vec![(7u64, "\u{25CF} alpha".to_string())];
    let r = rank("", &hosts(), &tabs, &[]);
    assert_eq!(r[0].0, Item::Tab(7));
    assert_eq!(r[1].0, Item::Host(0));
}

fn open(f: &mut Fixture) {
    f.frame(vec![Event::KeyDown { key: Key::Char('k'), mods: CTRL_SHIFT, text: None }]);
    f.frame(vec![]);
    assert!(f.app.switcher_open);
}

#[test]
fn enter_opens_terminal_for_host() {
    let mut f = Fixture::new();
    open(&mut f);
    f.frame(vec![Event::Text("beta".into())]);
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(!f.app.switcher_open);
    assert_eq!(f.app.tabs.len(), 1);
    assert!(f.app.tab_title(0).contains("beta"));
}

#[test]
fn escape_closes_and_restores_focus() {
    let mut f = Fixture::new();
    f.click_text("Filter hosts");
    let before = f.h.state.focused();
    assert!(before.is_some());
    open(&mut f);
    assert_ne!(f.h.state.focused(), before);
    f.frame(vec![Event::KeyDown { key: Key::Escape, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(!f.app.switcher_open);
    assert_eq!(f.h.state.focused(), before);
}

#[test]
fn theme_item_applies_and_saves() {
    let mut f = Fixture::new();
    open(&mut f);
    f.frame(vec![Event::Text("dracula".into())]);
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    assert_eq!(f.app.theme.name, "dracula");
    let text = std::fs::read_to_string(&f.path).unwrap();
    assert!(text.contains("theme = \"dracula\""), "{text}");
    assert!(text.contains("# my hand-written comment"));
}

#[test]
fn plus_button_opens_switcher() {
    let mut f = Fixture::new();
    f.click_text("+");
    assert!(f.app.switcher_open);
}
