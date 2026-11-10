//! Tests for `keys`: chord parsing, default keymaps, `[keys]` overrides and
//! the rule that plain Ctrl/Alt chords belong to the shell in a terminal.

use std::collections::BTreeMap;

use super::*;

/// A key-down event with the given modifiers.
fn down(key: Key, ctrl: bool, shift: bool, alt: bool, logo: bool) -> Event {
    Event::KeyDown { key, mods: Modifiers { ctrl, shift, alt, logo }, text: None }
}

/// Chord strings parse (case, aliases, `+` as shifted `=`) and the default
/// keymaps match the expected actions on Windows/Linux and macOS.
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

/// The plain Ctrl+K switcher alias works outside a terminal but goes to the
/// shell inside one.
#[test]
fn ctrl_k_alias_only_outside_terminal() {
    let (km, _) = Keymap::new(&BTreeMap::new(), false);
    let ev = down(Key::Char('k'), true, false, false, false);
    assert_eq!(km.match_event(&ev, false), Some(Action::QuickSwitcher));
    assert_eq!(km.match_event(&ev, true), None);
}

/// A user binding of plain Ctrl or Alt + character only applies outside a
/// terminal, and an override replaces the default chord.
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

/// Bad chords and unknown action names become warnings, and the defaults
/// stay in effect.
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

/// A comma-separated override binds every chord listed.
#[test]
fn multiple_chords_in_one_override() {
    let o = BTreeMap::from([("new_tab".to_string(), "ctrl+shift+t, ctrl+shift+n".to_string())]);
    let (km, warn) = Keymap::new(&o, false);
    assert!(warn.is_empty());
    assert_eq!(km.match_event(&down(Key::Char('n'), true, true, false, false), true), Some(Action::NewTab));
}

/// No default chord is bound twice, and the split-pane chords are not
/// shell-owned so they work while a terminal has focus.
#[test]
fn default_chords_are_unique_and_leave_shell_keys_alone() {
    for mac in [false, true] {
        let (km, _) = Keymap::new(&BTreeMap::new(), mac);
        let mut seen: Vec<(Chord, Action)> = vec![];
        for a in Action::ALL {
            for c in km.chords(a) {
                assert!(!seen.iter().any(|(s, _)| s == c), "{c} is bound twice (mac {mac})");
                seen.push((c.clone(), a));
            }
        }
        for a in [Action::SplitRight, Action::SplitDown, Action::FocusNextPane, Action::ClosePane] {
            let c = &km.chords(a)[0];
            assert!(!c.shell_owned(), "{c}");
            assert_eq!(km.match_key(&c.key, c.mods, true), Some(a), "{} works in a terminal", a.config_name());
        }
    }
}
