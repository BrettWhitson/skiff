//! The TUI's global shortcuts: actions, key chords, defaults and the
//! `[tui_keys]` config section that remaps them.

use std::collections::BTreeMap;
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// Switch to tab n (1-9).
    Tab(u8),
    NextTab,
    PrevTab,
    CloseTab,
    Theme,
    Help,
    Quit,
    /// Find in the terminal's output.
    Find,
    Snippets,
    /// Split the terminal tab side by side.
    SplitRight,
    /// Split the terminal tab one above the other.
    SplitDown,
    FocusNextPane,
}

impl Action {
    pub const ALL: [Action; 20] = [
        Action::Tab(1),
        Action::Tab(2),
        Action::Tab(3),
        Action::Tab(4),
        Action::Tab(5),
        Action::Tab(6),
        Action::Tab(7),
        Action::Tab(8),
        Action::Tab(9),
        Action::NextTab,
        Action::PrevTab,
        Action::CloseTab,
        Action::Theme,
        Action::Help,
        Action::Quit,
        Action::Find,
        Action::Snippets,
        Action::SplitRight,
        Action::SplitDown,
        Action::FocusNextPane,
    ];

    /// The name used in the `[tui_keys]` table.
    pub fn config_name(self) -> String {
        match self {
            Action::Tab(n) => format!("tab_{n}"),
            Action::NextTab => "next_tab".into(),
            Action::PrevTab => "prev_tab".into(),
            Action::CloseTab => "close_tab".into(),
            Action::Theme => "theme".into(),
            Action::Help => "help".into(),
            Action::Quit => "quit".into(),
            Action::Find => "find".into(),
            Action::Snippets => "snippets".into(),
            Action::SplitRight => "split_right".into(),
            Action::SplitDown => "split_down".into(),
            Action::FocusNextPane => "focus_next_pane".into(),
        }
    }

    pub fn from_config_name(s: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.config_name() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Tab(_) => "switch to tab",
            Action::NextTab => "next tab",
            Action::PrevTab => "previous tab",
            Action::CloseTab => "close tab (or the focused pane of a split)",
            Action::Theme => "theme picker",
            Action::Help => "this help",
            Action::Quit => "quit",
            Action::Find => "find (Enter older · Shift+Enter newer)",
            Action::Snippets => "snippets",
            Action::SplitRight => "split terminal side by side",
            Action::SplitDown => "split terminal top / bottom",
            Action::FocusNextPane => "other pane of a split",
        }
    }

    pub fn defaults(self) -> &'static [&'static str] {
        match self {
            Action::Tab(n) => {
                const D: [&[&str]; 9] =
                    [&["alt+1"], &["alt+2"], &["alt+3"], &["alt+4"], &["alt+5"], &["alt+6"], &["alt+7"], &["alt+8"], &["alt+9"]];
                D[(n as usize).clamp(1, 9) - 1]
            }
            Action::NextTab => &["alt+right", "alt+]"],
            Action::PrevTab => &["alt+left", "alt+["],
            Action::CloseTab => &["alt+w"],
            Action::Theme => &["alt+t"],
            Action::Help => &["alt+h"],
            Action::Quit => &["alt+q"],
            Action::Find => &["alt+f"],
            Action::Snippets => &["alt+s"],
            Action::SplitRight => &["alt+\\"],
            Action::SplitDown => &["alt+-"],
            Action::FocusNextPane => &["alt+o"],
        }
    }
}

/// A key with modifiers. Letters are stored lower case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const MODS: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::ALT).union(KeyModifiers::SHIFT);

impl Chord {
    pub fn parse(s: &str) -> Result<Chord, String> {
        let t = s.trim().to_ascii_lowercase();
        let mut parts: Vec<&str> = t.split('+').map(str::trim).collect();
        // "alt++" splits into [.., "", ""]: the key is '+'.
        if t.ends_with("++") {
            parts.truncate(parts.len() - 2);
            parts.push("+");
        }
        let Some(key) = parts.pop().filter(|k| !k.is_empty()) else {
            return Err(format!("\"{s}\" has no key"));
        };
        let mut mods = KeyModifiers::NONE;
        for m in parts {
            mods |= match m {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" | "option" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                other => return Err(format!("unknown modifier \"{other}\" in \"{s}\"")),
            };
        }
        let code = match key {
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "tab" => KeyCode::Tab,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "plus" => KeyCode::Char('+'),
            "minus" => KeyCode::Char('-'),
            k if k.chars().count() == 1 => KeyCode::Char(k.chars().next().expect("one char")),
            k if k.starts_with('f') && k[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => {
                KeyCode::F(k[1..].parse().expect("checked"))
            }
            other => return Err(format!("unknown key \"{other}\" in \"{s}\"")),
        };
        let chord = Chord { code, mods };
        // Without Ctrl or Alt the key would be taken from the shell and from
        // every list and form (F-keys excepted: termax's own lists use few).
        if !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) && !matches!(code, KeyCode::F(_)) {
            return Err(format!("\"{s}\" needs Ctrl or Alt (or use F1-F12)"));
        }
        Ok(chord)
    }

    pub fn matches(&self, key: &KeyEvent) -> bool {
        let code = match key.code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            c => c,
        };
        // A symbol like `?` or `+` is typed with Shift on many layouts, and
        // terminals report it: Shift only tells letters apart.
        let mods = match code {
            KeyCode::Char(c) if !c.is_alphabetic() => MODS - KeyModifiers::SHIFT,
            _ => MODS,
        };
        code == self.code && (key.modifiers & mods) == (self.mods & mods)
    }

    /// What the shell uses this chord for, if it is one of the control keys
    /// a terminal can't do without (interrupt, EOF, suspend, quit, Esc).
    fn shell_key(&self) -> Option<&'static str> {
        if self.mods - KeyModifiers::SHIFT != KeyModifiers::CONTROL {
            return None;
        }
        match self.code {
            KeyCode::Char('c') => Some("interrupt"),
            KeyCode::Char('d') => Some("end of input"),
            KeyCode::Char('z') => Some("suspend"),
            KeyCode::Char('\\') => Some("quit"),
            KeyCode::Char('[') => Some("Esc"),
            _ => None,
        }
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (m, name) in [(KeyModifiers::CONTROL, "Ctrl"), (KeyModifiers::ALT, "Alt"), (KeyModifiers::SHIFT, "Shift")] {
            if self.mods.contains(m) {
                write!(f, "{name}+")?;
            }
        }
        match self.code {
            KeyCode::Char(' ') => write!(f, "Space"),
            KeyCode::Char(c) => write!(f, "{}", c.to_ascii_uppercase()),
            KeyCode::Left => write!(f, "←"),
            KeyCode::Right => write!(f, "→"),
            KeyCode::Up => write!(f, "↑"),
            KeyCode::Down => write!(f, "↓"),
            KeyCode::PageUp => write!(f, "PgUp"),
            KeyCode::PageDown => write!(f, "PgDn"),
            KeyCode::F(n) => write!(f, "F{n}"),
            KeyCode::Esc => write!(f, "Esc"),
            KeyCode::Delete => write!(f, "Del"),
            KeyCode::Insert => write!(f, "Ins"),
            c => write!(f, "{c:?}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Keymap {
    binds: BTreeMap<Action, Vec<Chord>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new(&BTreeMap::new()).0
    }
}

impl Keymap {
    /// Defaults with `[tui_keys]` overrides applied (a value may list
    /// several chords separated by commas). Each problem — an unknown
    /// action, a bad chord, a chord bound twice — is reported, and the
    /// entries involved keep their defaults.
    pub fn new(overrides: &BTreeMap<String, String>) -> (Keymap, Vec<String>) {
        let mut binds: BTreeMap<Action, Vec<Chord>> =
            Action::ALL.into_iter().map(|a| (a, a.defaults().iter().map(|s| Chord::parse(s).expect("valid default")).collect())).collect();
        let mut errors = vec![];
        let mut changed = vec![];
        for (name, value) in overrides {
            let Some(action) = Action::from_config_name(name) else {
                errors.push(format!("[tui_keys] {name}: unknown action"));
                continue;
            };
            let parsed: Result<Vec<Chord>, String> = value.split(',').filter(|s| !s.trim().is_empty()).map(Chord::parse).collect();
            let stolen = parsed.as_ref().ok().and_then(|cs| cs.iter().find_map(|c| c.shell_key().map(|what| (*c, what))));
            if let Some((c, what)) = stolen {
                errors.push(format!("[tui_keys] {name}: {c} belongs to the shell ({what}), keeping the default"));
                continue;
            }
            match parsed {
                Ok(chords) => {
                    binds.insert(action, chords);
                    changed.push(action);
                }
                Err(e) => errors.push(format!("[tui_keys] {name}: {e}")),
            }
        }
        // A chord bound to two actions: the overrides involved fall back to
        // their defaults (which never clash).
        // Repeated because a restored default can clash with another override.
        loop {
            let clash = changed.iter().find_map(|&a| {
                binds[&a].iter().find_map(|c| binds.iter().find(|(o, cs)| **o != a && cs.contains(c)).map(|(o, _)| (a, *c, *o)))
            });
            let Some((a, c, other)) = clash else { break };
            errors.push(format!("[tui_keys] {}: {c} is already bound to {}", a.config_name(), other.config_name()));
            changed.retain(|x| *x != a);
            binds.insert(a, a.defaults().iter().map(|s| Chord::parse(s).expect("valid default")).collect());
        }
        (Keymap { binds }, errors)
    }

    pub fn chords(&self, a: Action) -> &[Chord] {
        self.binds.get(&a).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The chords for `a` as text (`Alt+W`, or `Alt+← / Alt+[`); "unbound" if none.
    pub fn show(&self, a: Action) -> String {
        let c = self.chords(a);
        if c.is_empty() { "unbound".into() } else { c.iter().map(Chord::to_string).collect::<Vec<_>>().join(" / ") }
    }

    /// The tab-switching keys, compact when they follow one pattern (`Alt+1…9`).
    pub fn show_tabs(&self) -> String {
        let first = self.chords(Action::Tab(1)).first().copied();
        let pattern = first.is_some_and(|f| {
            (1..=9u8).all(|n| {
                self.chords(Action::Tab(n)).first().is_some_and(|c| c.mods == f.mods && c.code == KeyCode::Char((b'0' + n) as char))
            })
        });
        match first {
            Some(f) if pattern => format!("{f} … {}", self.chords(Action::Tab(9))[0]),
            _ => (1..=9).map(|n| self.show(Action::Tab(n))).collect::<Vec<_>>().join(" "),
        }
    }

    pub fn match_key(&self, key: &KeyEvent) -> Option<Action> {
        self.binds.iter().find_map(|(a, chords)| chords.iter().any(|c| c.matches(key)).then_some(*a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn key(c: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(c, m)
    }

    #[test]
    fn defaults_are_current_bindings() {
        let (k, errors) = Keymap::new(&BTreeMap::new());
        assert!(errors.is_empty());
        assert_eq!(k.match_key(&key(KeyCode::Char('w'), KeyModifiers::ALT)), Some(Action::CloseTab));
        // Caps Lock: Alt+H arrives upper case without Shift.
        assert_eq!(k.match_key(&key(KeyCode::Char('H'), KeyModifiers::ALT)), Some(Action::Help));
        assert_eq!(k.match_key(&key(KeyCode::Char('H'), KeyModifiers::ALT | KeyModifiers::SHIFT)), None, "Alt+Shift is the shell's");
        assert_eq!(k.match_key(&key(KeyCode::Char('3'), KeyModifiers::ALT)), Some(Action::Tab(3)));
        assert_eq!(k.match_key(&key(KeyCode::Left, KeyModifiers::ALT)), Some(Action::PrevTab));
        assert_eq!(k.show_tabs(), "Alt+1 … Alt+9");
        assert_eq!(k.show(Action::NextTab), "Alt+→ / Alt+]");
    }

    #[test]
    fn overrides_and_validation() {
        let (k, errors) = Keymap::new(&map(&[
            ("close_tab", "ctrl+shift+x, f4"),
            ("nope", "alt+z"),
            ("help", "alt+banana"),
            ("theme", "x"),
            ("quit", "alt+f"), // taken by find
        ]));
        assert_eq!(
            errors,
            [
                "[tui_keys] help: unknown key \"banana\" in \"alt+banana\"",
                "[tui_keys] nope: unknown action",
                "[tui_keys] theme: \"x\" needs Ctrl or Alt (or use F1-F12)",
                "[tui_keys] quit: Alt+F is already bound to find",
            ]
        );
        assert_eq!(k.match_key(&key(KeyCode::Char('X'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)), Some(Action::CloseTab));
        assert_eq!(k.match_key(&key(KeyCode::F(4), KeyModifiers::NONE)), Some(Action::CloseTab));
        assert_eq!(k.match_key(&key(KeyCode::Char('w'), KeyModifiers::ALT)), None, "the default is replaced");
        assert_eq!(k.show(Action::Help), "Alt+H", "a bad entry keeps the default");
        assert_eq!(k.show(Action::Quit), "Alt+Q", "a clashing entry keeps the default");
        // Swapping two actions is fine.
        let (k, errors) = Keymap::new(&map(&[("find", "alt+q"), ("quit", "alt+f")]));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(k.match_key(&key(KeyCode::Char('f'), KeyModifiers::ALT)), Some(Action::Quit));
    }

    #[test]
    fn symbol_chords_ignore_shift_and_shell_keys_are_refused() {
        let (k, errors) =
            Keymap::new(&map(&[("help", "alt+?"), ("theme", "alt++"), ("quit", "ctrl+c"), ("find", "ctrl+["), ("snippets", "ctrl+alt+c")]));
        assert_eq!(
            errors,
            [
                "[tui_keys] find: Ctrl+[ belongs to the shell (Esc), keeping the default",
                "[tui_keys] quit: Ctrl+C belongs to the shell (interrupt), keeping the default",
            ]
        );
        // Terminals report Shift for symbols typed with it.
        assert_eq!(k.match_key(&key(KeyCode::Char('?'), KeyModifiers::ALT | KeyModifiers::SHIFT)), Some(Action::Help));
        assert_eq!(k.match_key(&key(KeyCode::Char('+'), KeyModifiers::ALT | KeyModifiers::SHIFT)), Some(Action::Theme));
        assert_eq!(k.match_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)), None);
        assert_eq!(k.show(Action::Quit), "Alt+Q");
        assert_eq!(k.show(Action::Snippets), "Ctrl+Alt+C");
    }

    #[test]
    fn every_action_has_a_unique_name() {
        let names: std::collections::BTreeSet<String> = Action::ALL.iter().map(|a| a.config_name()).collect();
        assert_eq!(names.len(), Action::ALL.len());
        for a in Action::ALL {
            assert_eq!(Action::from_config_name(&a.config_name()), Some(a));
        }
    }
}
