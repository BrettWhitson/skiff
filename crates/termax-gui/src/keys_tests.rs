use std::collections::BTreeMap;

use super::*;

fn down(key: Key, ctrl: bool, shift: bool, alt: bool, logo: bool) -> Event {
    Event::KeyDown { key, mods: Modifiers { ctrl, shift, alt, logo }, text: None }
}

#[test]
fn chord_parse_and_defaults() {
    assert_eq!(
        Chord::parse("Ctrl+Shift+K").unwrap(),
        Chord { key: Key::Char('k'), mods: Modifiers { ctrl: true, shift: true, alt: false, logo: false } }
    );
    assert_eq!(Chord::parse("ctrl+pgdn").unwrap().key, Key::PageDown);
    assert_eq!(Chord::parse("ctrl+shift+=").unwrap().key, Key::Char('='));
    assert_eq!(Chord::parse("ctrl++").unwrap().key, Key::Char('='), "'+' is shifted '='");
    assert!(Chord::parse("cmd+k").unwrap().mods.logo);

    let (win, warn) = Keymap::new(&BTreeMap::new(), false);
    assert!(warn.is_empty());
    assert_eq!(win.match_event(&down(Key::Char('k'), true, true, false, false), true), Some(Action::QuickSwitcher));
    // shifted '=' arrives as '+'
    assert_eq!(win.match_event(&down(Key::Char('+'), true, true, false, false), true), Some(Action::ZoomIn));
    assert_eq!(win.match_event(&down(Key::Tab, true, false, false, false), true), Some(Action::NextTab));
    assert_eq!(win.match_event(&down(Key::PageUp, true, false, false, false), true), Some(Action::PrevTab));

    let (mac, _) = Keymap::new(&BTreeMap::new(), true);
    assert_eq!(mac.match_event(&down(Key::Char('k'), false, false, false, true), true), Some(Action::QuickSwitcher));
    assert_eq!(mac.match_event(&down(Key::Char('k'), true, true, false, false), true), None);
    assert_eq!(win.chords(Action::QuickSwitcher)[0].to_string(), "Ctrl+Shift+K");
}

#[test]
fn ctrl_k_alias_only_outside_terminal() {
    let (km, _) = Keymap::new(&BTreeMap::new(), false);
    let ev = down(Key::Char('k'), true, false, false, false);
    assert_eq!(km.match_event(&ev, false), Some(Action::QuickSwitcher));
    assert_eq!(km.match_event(&ev, true), None);
}

#[test]
fn plain_ctrl_never_app_shortcut_in_terminal() {
    let o = BTreeMap::from([("copy".to_string(), "ctrl+c".to_string()), ("paste".to_string(), "alt+v".to_string())]);
    let (km, warn) = Keymap::new(&o, false);
    assert!(warn.is_empty(), "{warn:?}");
    assert_eq!(km.match_event(&down(Key::Char('c'), true, false, false, false), true), None);
    assert_eq!(km.match_event(&down(Key::Char('c'), true, false, false, false), false), Some(Action::Copy));
    assert_eq!(km.match_event(&down(Key::Char('v'), false, false, true, false), true), None);
    // the override replaced the default
    assert_eq!(km.match_event(&down(Key::Char('c'), true, true, false, false), false), None);
}

#[test]
fn invalid_chord_reported_not_fatal() {
    let o = BTreeMap::from([
        ("quick_switcher".to_string(), "ctrl+wat".to_string()),
        ("no_such_action".to_string(), "ctrl+shift+q".to_string()),
    ]);
    let (km, warn) = Keymap::new(&o, false);
    assert_eq!(warn.len(), 2, "{warn:?}");
    assert!(warn[0].contains("quick_switcher") || warn[1].contains("quick_switcher"));
    assert!(warn.iter().any(|w| w.contains("no_such_action")));
    assert_eq!(km.match_event(&down(Key::Char('k'), true, true, false, false), true), Some(Action::QuickSwitcher));
}

#[test]
fn multiple_chords_in_one_override() {
    let o = BTreeMap::from([("new_tab".to_string(), "ctrl+shift+t, ctrl+shift+n".to_string())]);
    let (km, warn) = Keymap::new(&o, false);
    assert!(warn.is_empty());
    assert_eq!(km.match_event(&down(Key::Char('n'), true, true, false, false), true), Some(Action::NewTab));
}
