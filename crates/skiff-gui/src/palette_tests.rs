//! Tests for the command palette: prefixes, ranking, highlight and the
//! open/choose/close flow.

use skiff_ui::input::{Event, Key, Modifiers};

use super::*;
use crate::app::tests::Fixture;

const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, shift: true, alt: false, logo: false };

/// Two custom hosts, `alpha` and `beta` (tagged `db`), without the ssh config.
fn hosts() -> Vec<HostEntry> {
    let cfg = skiff_core::config::Config::parse(
        "[settings]\nload_ssh_config=false\n[[connection]]\nname=\"alpha\"\nhost=\"a\"\n[[connection]]\nname=\"beta\"\nhost=\"b\"\ntags=[\"db\"]\n",
    )
    .unwrap();
    skiff_core::hosts::collect(&cfg).0
}

/// Sources with the two hosts, two themes, a forward and a snippet.
fn src(h: &[HostEntry]) -> Sources<'_> {
    Sources {
        hosts: h,
        themes: vec!["dracula".into(), "nord".into()],
        forwards: vec![(0, "web 8080\u{2192}localhost:80".into(), false)],
        snippets: vec![(0, "disk usage".into())],
        ..Default::default()
    }
}

/// Fuzzy queries rank the matching host or theme first, drop non-matches,
/// and report the matched characters.
#[test]
fn rank_prefers_hosts_by_fuzzy() {
    let h = hosts();
    let r = rank("bet", &src(&h));
    assert_eq!(r[0].item, Item::Host(1), "{:?}", r.iter().map(|x| &x.label).collect::<Vec<_>>());
    assert_eq!(r[0].matched, [0, 1, 2]);
    assert!(r.iter().all(|e| !matches!(e.item, Item::Host(0))));
    assert_eq!(rank("drac", &src(&h))[0].item, Item::Theme("dracula".into()));
    // A word-start match beats one inside a word; tags still find a host.
    assert!(fuzzy("us", "disk usage").unwrap().0 > fuzzy("us", "status").unwrap().0);
    assert_eq!(rank("db", &src(&h))[0].item, Item::Host(1));
}

/// Each prefix narrows the search to its kind; a bare prefix lists it all.
#[test]
fn prefixes_scope_the_search() {
    let h = hosts();
    let kinds = |q: &str| {
        let mut k: Vec<Kind> = rank(q, &src(&h)).iter().map(|e| e.kind).collect();
        k.dedup();
        k
    };
    assert_eq!(kinds(">"), [Kind::Command, Kind::Theme]);
    assert_eq!(kinds("@"), [Kind::Host, Kind::Files, Kind::Host, Kind::Files]);
    assert_eq!(kinds(":"), [Kind::Forward]);
    assert_eq!(kinds("# disk"), [Kind::Snippet]);
    assert_eq!(rank(">split ri", &src(&h))[0].item, Item::Command(Cmd::Action(Action::SplitRight)));
    assert_eq!(rank(":web", &src(&h))[0].label, "Start web 8080\u{2192}localhost:80");
}

/// An empty query lists Recent (tabs, then recent hosts only) before
/// Suggested commands; with no history every host is recent.
#[test]
fn empty_query_lists_recent_then_suggested() {
    let h = hosts();
    let mut s = Sources { recent: vec!["beta".into()], tabs: vec![(7, "alpha".into(), Icon::Terminal)], ..src(&h) };
    let r = rank("", &s);
    let items: Vec<&Item> = r.iter().map(|e| &e.item).collect();
    assert_eq!(items[..2], [&Item::Tab(7), &Item::Host(1)], "tabs, then only recent hosts");
    assert_eq!(r[1].section, Some(Section::Recent));
    assert!(r[2..].iter().all(|e| e.section == Some(Section::Suggested) && e.kind == Kind::Command));
    s.recent.clear();
    assert_eq!(rank("", &s).iter().filter(|e| e.kind == Kind::Host).count(), 2);
    assert_eq!(rank("alp", &s)[0].item, Item::Tab(7), "a tab beats its host on the same name");
}

/// Opens the palette with Ctrl+Shift+P and checks it opened.
fn open(f: &mut Fixture) {
    f.frame(vec![Event::KeyDown { key: Key::Char('p'), mods: CTRL_SHIFT, text: None }]);
    f.frame(vec![]);
    assert!(f.app.palette_open);
}

/// Typing a host name and pressing Enter opens a terminal tab and closes.
#[test]
fn enter_opens_terminal_for_host() {
    let mut f = Fixture::new();
    open(&mut f);
    f.frame(vec![Event::Text("beta".into())]);
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(!f.app.palette_open);
    assert_eq!(f.app.tabs.len(), 1);
    assert!(f.app.tab_title(0).contains("beta"));
}

/// Esc closes the palette and gives focus back to where it was.
#[test]
fn escape_closes_and_restores_focus() {
    let mut f = Fixture::new();
    f.click_text("Filter hosts");
    let before = f.h.state.focused();
    assert!(before.is_some());
    open(&mut f);
    assert_eq!(f.h.state.focused(), Some(input_id()));
    f.frame(vec![Event::KeyDown { key: Key::Escape, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(!f.app.palette_open);
    assert_eq!(f.h.state.focused(), before);
}

/// Choosing a theme applies it and saves it, keeping config comments; a
/// `>` command runs its action.
#[test]
fn theme_and_command_items_run() {
    let mut f = Fixture::new();
    open(&mut f);
    f.frame(vec![Event::Text("dracula".into())]);
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    assert_eq!(f.app.theme.name, "dracula");
    let text = std::fs::read_to_string(&f.path).unwrap();
    assert!(text.contains("theme = \"dracula\""), "{text}");
    assert!(text.contains("# my hand-written comment"));
    open(&mut f);
    f.frame(vec![Event::Text(">toggle side".into())]);
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    assert!(f.app.geometry.sidebar_collapsed);
}

/// The tab strip's "+" button opens the palette.
#[test]
fn plus_button_opens_palette() {
    let mut f = Fixture::new();
    let plus = f.h.tree().find("New tab").expect("plus button").bounds.center();
    f.frame(vec![Event::MouseMove(plus), Event::MouseDown(skiff_ui::input::MouseButton::Left, plus)]);
    f.frame(vec![Event::MouseUp(skiff_ui::input::MouseButton::Left, plus)]);
    assert!(f.app.palette_open);
}
