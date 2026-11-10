//! Toolkit key events → bytes for the pty.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termax_core::terminal::encode_key;
use termax_ui::input::{Event, Key, Modifiers};

use crate::keys::Keymap;

pub fn to_key_event(key: &Key, mods: Modifiers) -> Option<KeyEvent> {
    let code = match key {
        Key::Char(c) => KeyCode::Char(*c),
        Key::Space => KeyCode::Char(' '),
        Key::F(n) => KeyCode::F(*n),
        Key::Left => KeyCode::Left,
        Key::Right => KeyCode::Right,
        Key::Up => KeyCode::Up,
        Key::Down => KeyCode::Down,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::Insert => KeyCode::Insert,
        Key::Delete => KeyCode::Delete,
        Key::Backspace => KeyCode::Backspace,
        Key::Enter => KeyCode::Enter,
        Key::Tab if mods.shift => KeyCode::BackTab,
        Key::Tab => KeyCode::Tab,
        Key::Escape => KeyCode::Esc,
        Key::Other(_) | Key::ContextMenu => return None,
    };
    let mut m = KeyModifiers::empty();
    if mods.ctrl {
        m |= KeyModifiers::CONTROL;
    }
    if mods.alt {
        m |= KeyModifiers::ALT;
    }
    if mods.shift && !matches!(code, KeyCode::BackTab | KeyCode::Char(_)) {
        m |= KeyModifiers::SHIFT;
    }
    Some(KeyEvent::new(code, m))
}

/// The bytes this frame's keyboard events send to the shell. App shortcuts
/// are skipped; printable text comes from `Text` events, except Alt+key
/// (sent as ESC + key, its `Text` dropped). AltGr (Ctrl+Alt) text passes.
pub fn route(events: &[Event], keymap: &Keymap, app_cursor: bool) -> Vec<u8> {
    let mut out = vec![];
    let mut skip_text = false;
    for e in events {
        match e {
            Event::KeyDown { key: Key::F(6), mods, .. } if !mods.alt && !mods.logo => {
                // Plain / Shift+F6 move between regions; Ctrl+Shift+F6 is F6.
                if mods.ctrl && mods.shift {
                    out.extend(encode_key(KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE), app_cursor));
                }
            }
            Event::KeyDown { key, mods, text } => {
                if keymap.match_key(key, *mods, true).is_some() {
                    skip_text = text.is_some();
                    continue;
                }
                let altgr = mods.ctrl && mods.alt;
                let printable = text.is_some() && (!mods.ctrl || mods.alt) && !mods.logo;
                if printable && !(mods.alt && !altgr) {
                    continue; // its Text event carries it
                }
                if mods.logo {
                    continue;
                }
                // Alt+key: use the produced character (keeps Shift's case).
                let key = match (mods.alt && !altgr, text.as_deref().and_then(|t| t.chars().next())) {
                    (true, Some(c)) => Key::Char(c),
                    _ => key.clone(),
                };
                let m = if mods.alt && !altgr { Modifiers { shift: false, ..*mods } } else { *mods };
                if let Some(ev) = to_key_event(&key, m) {
                    out.extend(encode_key(ev, app_cursor));
                }
                skip_text = printable;
            }
            Event::Text(t) => {
                if skip_text {
                    skip_text = false;
                } else {
                    out.extend(t.as_bytes());
                }
            }
            _ => {}
        }
    }
    out
}
