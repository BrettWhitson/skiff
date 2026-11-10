//! The toolkit's own input types (positions in physical pixels).

use std::path::PathBuf;

use winit::keyboard::NamedKey;

use crate::a11y::A11yAction;
use crate::geom::Point;
use crate::ui::Id;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable key, lower-cased (`'a'`, `'1'`, `'/'`).
    Char(char),
    F(u8),
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Backspace,
    Enter,
    Tab,
    Escape,
    Space,
    /// The Menu / Application key.
    ContextMenu,
    Other(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers { ctrl: false, shift: false, alt: false, logo: false };

    /// The app-shortcut modifier set from the spec: Ctrl+Shift on
    /// Windows/Linux, Cmd on macOS.
    pub fn is_app_shortcut(&self) -> bool {
        if cfg!(target_os = "macos") { self.logo } else { self.ctrl && self.shift }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

/// Mouse cursor shapes an app can ask for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CursorIcon {
    #[default]
    Default,
    Pointer,
    Text,
    ResizeH,
    ResizeV,
    Grab,
    Grabbing,
    Wait,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    KeyDown {
        key: Key,
        mods: Modifiers,
        text: Option<String>,
    },
    KeyUp {
        key: Key,
        mods: Modifiers,
    },
    /// Text to insert (typed characters, IME commits).
    Text(String),
    MouseMove(Point),
    MouseDown(MouseButton, Point),
    MouseUp(MouseButton, Point),
    /// Scroll in physical pixels; positive `dy` = content moves down (scroll up).
    Wheel {
        dx: f32,
        dy: f32,
    },
    Paste(String),
    FileDropped(PathBuf),
    FocusLost,
    FocusGained,
    /// The held modifier keys changed (needed for Ctrl/Shift-click).
    Modifiers(Modifiers),
    /// A screen reader asked widget `target` to do something.
    A11y {
        target: Id,
        action: A11yAction,
    },
}

pub fn map_named_key(k: &NamedKey) -> Option<Key> {
    use NamedKey as N;
    Some(match k {
        N::ArrowLeft => Key::Left,
        N::ArrowRight => Key::Right,
        N::ArrowUp => Key::Up,
        N::ArrowDown => Key::Down,
        N::Home => Key::Home,
        N::End => Key::End,
        N::PageUp => Key::PageUp,
        N::PageDown => Key::PageDown,
        N::Insert => Key::Insert,
        N::Delete => Key::Delete,
        N::Backspace => Key::Backspace,
        N::Enter => Key::Enter,
        N::Tab => Key::Tab,
        N::Escape => Key::Escape,
        N::Space => Key::Space,
        N::ContextMenu => Key::ContextMenu,
        N::F1 => Key::F(1),
        N::F2 => Key::F(2),
        N::F3 => Key::F(3),
        N::F4 => Key::F(4),
        N::F5 => Key::F(5),
        N::F6 => Key::F(6),
        N::F7 => Key::F(7),
        N::F8 => Key::F(8),
        N::F9 => Key::F(9),
        N::F10 => Key::F(10),
        N::F11 => Key::F(11),
        N::F12 => Key::F(12),
        _ => return None,
    })
}

/// Maps a character key's logical text to a `Key`, lower-cased.
pub fn map_char_key(s: &str) -> Key {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Key::Char(c.to_lowercase().next().unwrap_or(c)),
        _ => Key::Other(s.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    #[test]
    fn winit_key_mapping() {
        assert_eq!(map_named_key(&NamedKey::Enter), Some(Key::Enter));
        assert_eq!(map_named_key(&NamedKey::ArrowLeft), Some(Key::Left));
        assert_eq!(map_named_key(&NamedKey::F12), Some(Key::F(12)));
        assert_eq!(map_named_key(&NamedKey::PageDown), Some(Key::PageDown));
        assert_eq!(map_named_key(&NamedKey::Space), Some(Key::Space));
        assert_eq!(map_named_key(&NamedKey::Shift), None);
        assert_eq!(map_char_key("A"), Key::Char('a'));
        assert_eq!(map_char_key("é"), Key::Char('é'));
        assert_eq!(map_char_key("ab"), Key::Other("ab".into()));
    }
}
