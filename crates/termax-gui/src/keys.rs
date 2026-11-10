//! App shortcuts: actions, key chords, defaults and `[keys]` overrides.
//!
//! Shortcut rule (spec): while a terminal has focus, chords made of plain
//! Ctrl or plain Alt plus a character always go to the shell.

use std::collections::BTreeMap;
use std::fmt;

pub use termax_ui::input::{Event, Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    QuickSwitcher,
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    Copy,
    Paste,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleSidebar,
    Settings,
    /// Find in the terminal's output.
    Find,
    UiScaleUp,
    UiScaleDown,
    UiScaleReset,
}

impl Action {
    pub const ALL: [Action; 16] = [
        Action::QuickSwitcher,
        Action::NewTab,
        Action::CloseTab,
        Action::NextTab,
        Action::PrevTab,
        Action::Copy,
        Action::Paste,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::ZoomReset,
        Action::ToggleSidebar,
        Action::Settings,
        Action::Find,
        Action::UiScaleUp,
        Action::UiScaleDown,
        Action::UiScaleReset,
    ];

    /// The name used in the `[keys]` table.
    pub fn config_name(self) -> &'static str {
        match self {
            Action::QuickSwitcher => "quick_switcher",
            Action::NewTab => "new_tab",
            Action::CloseTab => "close_tab",
            Action::NextTab => "next_tab",
            Action::PrevTab => "prev_tab",
            Action::Copy => "copy",
            Action::Paste => "paste",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::ZoomReset => "zoom_reset",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::Settings => "settings",
            Action::Find => "find",
            Action::UiScaleUp => "ui_scale_up",
            Action::UiScaleDown => "ui_scale_down",
            Action::UiScaleReset => "ui_scale_reset",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::QuickSwitcher => "Quick switcher",
            Action::NewTab => "New tab",
            Action::CloseTab => "Close tab",
            Action::NextTab => "Next tab",
            Action::PrevTab => "Previous tab",
            Action::Copy => "Copy",
            Action::Paste => "Paste",
            Action::ZoomIn => "Zoom in",
            Action::ZoomOut => "Zoom out",
            Action::ZoomReset => "Reset zoom",
            Action::ToggleSidebar => "Toggle sidebar",
            Action::Settings => "Settings",
            Action::Find => "Find",
            Action::UiScaleUp => "Larger interface",
            Action::UiScaleDown => "Smaller interface",
            Action::UiScaleReset => "Reset interface size",
        }
    }

    pub fn from_config_name(s: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.config_name() == s)
    }

    /// Default chords (Windows/Linux, or macOS when `mac`).
    pub fn defaults(self, mac: bool) -> &'static [&'static str] {
        use Action::*;
        if mac {
            return match self {
                QuickSwitcher => &["cmd+k"],
                NewTab => &["cmd+t"],
                CloseTab => &["cmd+w"],
                NextTab => &["ctrl+tab", "cmd+shift+]"],
                PrevTab => &["ctrl+shift+tab", "cmd+shift+["],
                Copy => &["cmd+c"],
                Paste => &["cmd+v"],
                ZoomIn => &["cmd+="],
                ZoomOut => &["cmd+-"],
                ZoomReset => &["cmd+0"],
                ToggleSidebar => &["cmd+b"],
                Settings => &["cmd+,"],
                Find => &["cmd+f"],
                UiScaleUp => &["cmd+alt+="],
                UiScaleDown => &["cmd+alt+-"],
                UiScaleReset => &["cmd+alt+0"],
            };
        }
        match self {
            QuickSwitcher => &["ctrl+shift+k", "ctrl+k"],
            NewTab => &["ctrl+shift+t"],
            CloseTab => &["ctrl+shift+w"],
            NextTab => &["ctrl+tab", "ctrl+pagedown"],
            PrevTab => &["ctrl+shift+tab", "ctrl+pageup"],
            Copy => &["ctrl+shift+c"],
            Paste => &["ctrl+shift+v"],
            ZoomIn => &["ctrl+shift+="],
            ZoomOut => &["ctrl+shift+-"],
            ZoomReset => &["ctrl+shift+0"],
            ToggleSidebar => &["ctrl+shift+b"],
            Settings => &["ctrl+shift+,"],
            Find => &["ctrl+shift+f"],
            UiScaleUp => &["ctrl+alt+="],
            UiScaleDown => &["ctrl+alt+-"],
            UiScaleReset => &["ctrl+alt+0"],
        }
    }
}

/// Ctrl+Alt that produces a character is AltGr typing (e.g. `}` on a German
/// keyboard), never a shortcut.
pub fn altgr_text(mods: Modifiers, text: Option<&str>) -> bool {
    mods.ctrl && mods.alt && text.is_some_and(|t| !t.is_empty())
}

/// Shifted US-layout symbols map to their base key, so `Ctrl+Shift+=`
/// matches although the key arrives as `+`.
fn base_char(c: char) -> char {
    const SHIFTED: &str = "!@#$%^&*()_+{}|:\"<>?~";
    const BASE: &str = "1234567890-=[]\\;',./`";
    SHIFTED
        .chars()
        .position(|s| s == c)
        .and_then(|i| BASE.chars().nth(i))
        .unwrap_or(c)
}

fn normalize(k: &Key) -> Key {
    match k {
        Key::Char(c) => Key::Char(base_char(c.to_lowercase().next().unwrap_or(*c))),
        other => other.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub mods: Modifiers,
}

impl Chord {
    pub fn parse(s: &str) -> Result<Chord, String> {
        let s = s.trim();
        let mut parts: Vec<String> = s
            .split('+')
            .map(|p| p.trim().to_ascii_lowercase())
            .collect();
        // "ctrl++" splits into [.., "", ""]: the key is '+'.
        if s.ends_with("++") {
            parts.truncate(parts.len() - 2);
            parts.push("+".into());
        }
        let Some(key_name) = parts.pop().filter(|k| !k.is_empty()) else {
            return Err(format!("`{s}` has no key"));
        };
        let mut mods = Modifiers::NONE;
        for m in &parts {
            match m.as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" | "option" | "opt" => mods.alt = true,
                "cmd" | "command" | "super" | "win" | "logo" | "meta" => mods.logo = true,
                other => return Err(format!("unknown modifier `{other}` in `{s}`")),
            }
        }
        let key = match key_name.as_str() {
            "tab" => Key::Tab,
            "enter" | "return" => Key::Enter,
            "esc" | "escape" => Key::Escape,
            "space" => Key::Space,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" | "pgup" => Key::PageUp,
            "pagedown" | "pgdn" => Key::PageDown,
            "insert" | "ins" => Key::Insert,
            "delete" | "del" => Key::Delete,
            "backspace" => Key::Backspace,
            "plus" => Key::Char('='),
            "minus" => Key::Char('-'),
            k if k.chars().count() == 1 => normalize(&Key::Char(k.chars().next().unwrap())),
            k if k.starts_with('f')
                && k[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) =>
            {
                Key::F(k[1..].parse().unwrap())
            }
            other => return Err(format!("unknown key `{other}` in `{s}`")),
        };
        Ok(Chord { key, mods })
    }

    /// A pressed key as config text (`ctrl+shift+q`); `None` for keys
    /// that cannot be bound (bare modifiers, unknown keys).
    pub fn from_press(key: &Key, mods: Modifiers) -> Option<String> {
        let k = match normalize(key) {
            Key::Char(c) => c.to_string(),
            Key::F(n) => format!("f{n}"),
            Key::Tab => "tab".into(),
            Key::Enter => "enter".into(),
            Key::Escape => "esc".into(),
            Key::Space => "space".into(),
            Key::Up => "up".into(),
            Key::Down => "down".into(),
            Key::Left => "left".into(),
            Key::Right => "right".into(),
            Key::Home => "home".into(),
            Key::End => "end".into(),
            Key::PageUp => "pageup".into(),
            Key::PageDown => "pagedown".into(),
            Key::Insert => "insert".into(),
            Key::Delete => "delete".into(),
            Key::Backspace => "backspace".into(),
            Key::ContextMenu => "menu".into(),
            Key::Other(_) => return None,
        };
        let mut s = String::new();
        for (on, name) in [
            (mods.ctrl, "ctrl+"),
            (mods.alt, "alt+"),
            (mods.shift, "shift+"),
            (mods.logo, "cmd+"),
        ] {
            if on {
                s.push_str(name);
            }
        }
        Some(s + &k)
    }

    fn matches(&self, key: &Key, mods: Modifiers) -> bool {
        self.mods == mods && self.key == normalize(key)
    }

    /// Plain Ctrl or plain Alt with a character: belongs to the shell.
    fn shell_owned(&self) -> bool {
        let m = self.mods;
        let only_ctrl = m.ctrl && !m.shift && !m.alt && !m.logo;
        let only_alt = m.alt && !m.shift && !m.ctrl && !m.logo;
        (only_ctrl || only_alt) && matches!(self.key, Key::Char(_) | Key::Space)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.mods;
        for (on, name) in [
            (m.ctrl, "Ctrl"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
            (m.logo, "Cmd"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match &self.key {
            Key::Char(c) => write!(f, "{}", c.to_uppercase()),
            Key::F(n) => write!(f, "F{n}"),
            Key::PageUp => write!(f, "PgUp"),
            Key::PageDown => write!(f, "PgDn"),
            Key::Other(s) => write!(f, "{s}"),
            k => write!(f, "{k:?}"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Keymap {
    binds: BTreeMap<Action, Vec<Chord>>,
}

impl Keymap {
    /// Defaults with `[keys]` overrides applied (a value may list several
    /// chords separated by commas). Problems become warnings; the default
    /// stays for an action whose override is invalid.
    pub fn new(overrides: &BTreeMap<String, String>, mac: bool) -> (Keymap, Vec<String>) {
        let mut binds = BTreeMap::new();
        for a in Action::ALL {
            let chords = a
                .defaults(mac)
                .iter()
                .map(|s| Chord::parse(s).expect("valid default"))
                .collect();
            binds.insert(a, chords);
        }
        let mut warnings = vec![];
        for (name, value) in overrides {
            let Some(action) = Action::from_config_name(name) else {
                warnings.push(format!("[keys] {name}: unknown action"));
                continue;
            };
            let parsed: Result<Vec<Chord>, String> = value
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(Chord::parse)
                .collect();
            match parsed {
                Ok(chords) => {
                    binds.insert(action, chords);
                }
                Err(e) => warnings.push(format!("[keys] {name}: {e}")),
            }
        }
        (Keymap { binds }, warnings)
    }

    pub fn platform(overrides: &BTreeMap<String, String>) -> (Keymap, Vec<String>) {
        Self::new(overrides, cfg!(target_os = "macos"))
    }

    pub fn chords(&self, a: Action) -> &[Chord] {
        self.binds.get(&a).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn match_key(&self, key: &Key, mods: Modifiers, in_terminal: bool) -> Option<Action> {
        self.binds.iter().find_map(|(a, chords)| {
            chords
                .iter()
                .any(|c| c.matches(key, mods) && !(in_terminal && c.shell_owned()))
                .then_some(*a)
        })
    }

    pub fn match_event(&self, ev: &Event, in_terminal: bool) -> Option<Action> {
        match ev {
            Event::KeyDown { key, mods, .. } => self.match_key(key, *mods, in_terminal),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
