//! The TUI's global shortcuts: actions, key chords, defaults and the
//! `[tui_keys]` config section that remaps them.

use std::collections::BTreeMap;
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A global TUI command that can be bound to key chords. Variants without a
/// doc are as named: tab navigation, closing a tab, opening the theme picker,
/// help or snippets list, and quitting.
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
    /// Move focus to the other pane of a split.
    FocusNextPane,
    /// Swap the two panes of a split.
    SwapPanes,
    /// Move the divider of a split to the next snap point that way.
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,
    /// Give the two panes of a split the same size.
    EqualizePanes,
    /// Starts a two-key shortcut: the next key picks one of the bare-key
    /// bindings (pressed twice, it goes to the shell itself).
    Prefix,
}

impl Action {
    /// Every action, including each of the nine tab slots.
    pub const ALL: [Action; 27] = [
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
        Action::SwapPanes,
        Action::ResizePaneLeft,
        Action::ResizePaneRight,
        Action::ResizePaneUp,
        Action::ResizePaneDown,
        Action::EqualizePanes,
        Action::Prefix,
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
            Action::SwapPanes => "swap_panes".into(),
            Action::ResizePaneLeft => "resize_pane_left".into(),
            Action::ResizePaneRight => "resize_pane_right".into(),
            Action::ResizePaneUp => "resize_pane_up".into(),
            Action::ResizePaneDown => "resize_pane_down".into(),
            Action::EqualizePanes => "equalize_panes".into(),
            Action::Prefix => "prefix".into(),
        }
    }

    /// The action a `[tui_keys]` entry name refers to, if any.
    pub fn from_config_name(s: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.config_name() == s)
    }

    /// Description shown next to the action's keys in the help screen.
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
            Action::SwapPanes => "swap the panes of a split",
            Action::ResizePaneLeft => "move a side-by-side divider left (25/33/50/67/75%)",
            Action::ResizePaneRight => "move a side-by-side divider right",
            Action::ResizePaneUp => "move a top / bottom divider up",
            Action::ResizePaneDown => "move a top / bottom divider down",
            Action::EqualizePanes => "split panes the same size",
            Action::Prefix => "then a key below (twice: send it to the shell)",
        }
    }

    /// The built-in chords, in `[tui_keys]` syntax. Bash's readline binds
    /// almost every Alt key and emacs the rest, so only three Alt chords are
    /// taken: `Alt+O` as the prefix and `Alt+[` / `Alt+]` for the tabs. Every
    /// other action is a bare key pressed after the prefix (tmux style).
    pub fn defaults(self) -> &'static [&'static str] {
        match self {
            Action::Tab(n) => {
                const D: [&[&str]; 9] = [&["1"], &["2"], &["3"], &["4"], &["5"], &["6"], &["7"], &["8"], &["9"]];
                D[(n as usize).clamp(1, 9) - 1]
            }
            Action::NextTab => &["alt+]", "n"],
            Action::PrevTab => &["alt+[", "p"],
            Action::CloseTab => &["w"],
            Action::Theme => &["t"],
            Action::Help => &["?", "h"],
            Action::Quit => &["q"],
            Action::Find => &["/"],
            Action::Snippets => &["s"],
            Action::SplitRight => &["|"],
            Action::SplitDown => &["-"],
            Action::FocusNextPane => &["o"],
            Action::SwapPanes => &["x"],
            Action::ResizePaneLeft => &["left"],
            Action::ResizePaneRight => &["right"],
            Action::ResizePaneUp => &["up"],
            Action::ResizePaneDown => &["down"],
            Action::EqualizePanes => &["="],
            Action::Prefix => &["alt+o"],
        }
    }
}

/// A key with modifiers. Letters are stored lower case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    /// The key itself.
    pub code: KeyCode,
    /// Ctrl / Alt / Shift that must be held.
    pub mods: KeyModifiers,
    /// A bare key (no Ctrl or Alt, not an F-key): it acts only right after
    /// the prefix chord.
    pub after_prefix: bool,
}

/// The modifiers a chord compares; others (Super, Hyper, ...) are ignored.
const MODS: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::ALT).union(KeyModifiers::SHIFT);

impl Chord {
    /// Parses config text like `alt+w`, `ctrl+shift+x` or `f4`
    /// (case-insensitive). A key without Ctrl or Alt (other than an F-key),
    /// like `w` or `shift+left`, is one pressed after the prefix.
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
        // Everything before the key is a modifier name.
        let mut mods = KeyModifiers::NONE;
        for m in parts {
            mods |= match m {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" | "option" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                other => return Err(format!("unknown modifier \"{other}\" in \"{s}\"")),
            };
        }
        // Named keys, then any single character, then F1-F12.
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
        // Without Ctrl or Alt the key would be taken from the shell and from
        // every list and form, so it only counts after the prefix (F-keys
        // excepted: skiff's own lists use few).
        let after_prefix = !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) && !matches!(code, KeyCode::F(_));
        Ok(Chord { code, mods, after_prefix })
    }

    /// Whether a key press triggers this chord. Letters match regardless of
    /// case (Caps Lock); Shift is ignored for symbols, and Alt for a key
    /// after the prefix (so Alt can stay held).
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
        let mods = if self.after_prefix { mods - KeyModifiers::ALT } else { mods };
        code == self.code && (key.modifiers & mods) == (self.mods & mods)
    }

    /// What the shell uses this chord for, if it is one of the control keys
    /// a terminal can't do without (interrupt, EOF, suspend, quit, Esc).
    fn shell_key(&self) -> Option<&'static str> {
        // Shift only counts for letters (Ctrl+Shift+C is a distinct chord);
        // a symbol chord matches with or without it.
        let mods = match self.code {
            KeyCode::Char(c) if !c.is_alphabetic() => self.mods - KeyModifiers::SHIFT,
            _ => self.mods,
        };
        if mods != KeyModifiers::CONTROL {
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
    /// Writes the chord for display, e.g. `Ctrl+Shift+X`, `Alt+←`, `F4`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (m, name) in [(KeyModifiers::CONTROL, "Ctrl"), (KeyModifiers::ALT, "Alt"), (KeyModifiers::SHIFT, "Shift")] {
            if self.mods.contains(m) {
                write!(f, "{name}+")?;
            }
        }
        match self.code {
            KeyCode::Char(' ') => write!(f, "Space"),
            // After the prefix a letter is shown as typed (`Alt+O w`, not Shift+W).
            KeyCode::Char(c) if self.after_prefix => write!(f, "{c}"),
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

/// The active bindings: each action's chords, after `[tui_keys]` overrides.
#[derive(Debug, Clone)]
pub struct Keymap {
    /// Chords per action; an action may have several (or none).
    binds: BTreeMap<Action, Vec<Chord>>,
}

impl Default for Keymap {
    /// The built-in bindings, with no overrides.
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
        // Apply each override; a shell control key or parse error keeps the default.
        for (name, value) in overrides {
            let Some(action) = Action::from_config_name(name) else {
                errors.push(format!("[tui_keys] {name}: unknown action"));
                continue;
            };
            let parsed: Result<Vec<Chord>, String> = value.split(',').filter(|s| !s.trim().is_empty()).map(Chord::parse).collect();
            if action == Action::Prefix
                && let Ok(Some(c)) = parsed.as_ref().map(|cs| cs.iter().find(|c| c.after_prefix))
            {
                errors.push(format!("[tui_keys] {name}: {c} needs Ctrl or Alt (or use F1-F12), keeping the default"));
                continue;
            }
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

    /// The chords bound to `a` (empty if none).
    pub fn chords(&self, a: Action) -> &[Chord] {
        self.binds.get(&a).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The chords for `a` as text (`Alt+O W`, or `Alt+] / Alt+O N`);
    /// "unbound" if none (a key after the prefix needs a bound prefix).
    pub fn show(&self, a: Action) -> String {
        let c: Vec<String> = self.chords(a).iter().filter_map(|c| self.show_chord(c)).collect();
        if c.is_empty() { "unbound".into() } else { c.join(" / ") }
    }

    /// One chord as text, with the prefix in front of a key after it;
    /// `None` for such a key when no prefix is bound.
    fn show_chord(&self, c: &Chord) -> Option<String> {
        if !c.after_prefix {
            return Some(c.to_string());
        }
        self.chords(Action::Prefix).first().map(|p| format!("{p} {c}"))
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
            Some(f) if pattern && f.after_prefix => self.show_chord(&f).map_or_else(|| "unbound".into(), |s| format!("{s}…9")),
            Some(f) if pattern => format!("{f} … {}", self.chords(Action::Tab(9))[0]),
            _ => (1..=9).map(|n| self.show(Action::Tab(n))).collect::<Vec<_>>().join(" "),
        }
    }

    /// The action a key press triggers on its own, if any.
    pub fn match_key(&self, key: &KeyEvent) -> Option<Action> {
        self.binds.iter().find_map(|(a, chords)| chords.iter().any(|c| !c.after_prefix && c.matches(key)).then_some(*a))
    }

    /// The action a key pressed right after the prefix triggers, if any.
    pub fn match_after_prefix(&self, key: &KeyEvent) -> Option<Action> {
        self.binds.iter().find_map(|(a, chords)| chords.iter().any(|c| c.after_prefix && c.matches(key)).then_some(*a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `[tui_keys]` table from name / value pairs.
    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// A key press event.
    fn key(c: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(c, m)
    }

    /// The default keymap matches the documented keys and displays them compactly.
    #[test]
    fn defaults_are_current_bindings() {
        let (k, errors) = Keymap::new(&BTreeMap::new());
        assert!(errors.is_empty());
        assert_eq!(k.match_key(&key(KeyCode::Char('o'), KeyModifiers::ALT)), Some(Action::Prefix));
        assert_eq!(k.match_key(&key(KeyCode::Char(']'), KeyModifiers::ALT)), Some(Action::NextTab));
        assert_eq!(k.match_key(&key(KeyCode::Char('w'), KeyModifiers::NONE)), None, "bare keys only count after the prefix");
        assert_eq!(k.match_after_prefix(&key(KeyCode::Char('w'), KeyModifiers::NONE)), Some(Action::CloseTab));
        // Alt may stay held, and Caps Lock sends an upper-case letter without Shift.
        assert_eq!(k.match_after_prefix(&key(KeyCode::Char('W'), KeyModifiers::ALT)), Some(Action::CloseTab));
        assert_eq!(k.match_after_prefix(&key(KeyCode::Char('3'), KeyModifiers::NONE)), Some(Action::Tab(3)));
        assert_eq!(k.match_after_prefix(&key(KeyCode::Left, KeyModifiers::NONE)), Some(Action::ResizePaneLeft));
        assert_eq!(k.show_tabs(), "Alt+O 1…9");
        assert_eq!(k.show(Action::NextTab), "Alt+] / Alt+O n");
        assert_eq!(k.show(Action::Help), "Alt+O ? / Alt+O h");
    }

    /// Overrides replace defaults; bad names, chords and clashes are reported
    /// and keep the defaults; swapping two actions' keys is allowed.
    #[test]
    fn overrides_and_validation() {
        let (k, errors) = Keymap::new(&map(&[
            ("close_tab", "ctrl+shift+x, f4"),
            ("nope", "alt+z"),
            ("help", "alt+banana"),
            ("theme", "x"),     // taken by swap_panes
            ("quit", "alt+]"),  // taken by next_tab
            ("prefix", "w"),    // the prefix itself needs Ctrl or Alt
        ]));
        assert_eq!(
            errors,
            [
                "[tui_keys] help: unknown key \"banana\" in \"alt+banana\"",
                "[tui_keys] nope: unknown action",
                "[tui_keys] prefix: w needs Ctrl or Alt (or use F1-F12), keeping the default",
                "[tui_keys] quit: Alt+] is already bound to next_tab",
                "[tui_keys] theme: x is already bound to swap_panes",
            ]
        );
        assert_eq!(k.match_key(&key(KeyCode::Char('X'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)), Some(Action::CloseTab));
        assert_eq!(k.match_key(&key(KeyCode::F(4), KeyModifiers::NONE)), Some(Action::CloseTab));
        assert_eq!(k.match_key(&key(KeyCode::Char('w'), KeyModifiers::ALT)), None, "the default is replaced");
        assert_eq!(k.show(Action::Help), "Alt+O ? / Alt+O h", "a bad entry keeps the default");
        assert_eq!(k.show(Action::Quit), "Alt+O q", "a clashing entry keeps the default");
        // Swapping two actions is fine.
        let (k, errors) = Keymap::new(&map(&[("find", "alt+q"), ("quit", "alt+f")]));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(k.match_key(&key(KeyCode::Char('f'), KeyModifiers::ALT)), Some(Action::Quit));
    }

    /// Symbol chords match with or without Shift, and the shell's control keys can't be bound.
    #[test]
    fn symbol_chords_ignore_shift_and_shell_keys_are_refused() {
        let (k, errors) = Keymap::new(&map(&[
            ("help", "alt+?"),
            ("theme", "alt++"),
            ("quit", "ctrl+c"),
            ("find", "ctrl+["),
            ("snippets", "ctrl+alt+c"),
            ("close_tab", "ctrl+shift+c"),
        ]));
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
        assert_eq!(k.show(Action::Quit), "Alt+O q");
        assert_eq!(k.show(Action::Snippets), "Ctrl+Alt+C");
        assert_eq!(k.show(Action::CloseTab), "Ctrl+Shift+C", "Shift tells letters apart");
    }

    /// Every Alt key that bash's default (emacs-mode) readline binds, from
    /// `bind -p` in bash 5.2 on Ubuntu 24.04: `\eX` for each X below
    /// (`\eA`-`\eZ` are do-lowercase-version; `\eO` is left out as the start
    /// of SS3 key codes), `\e\C-X` for the Ctrl letters, and Alt+←/→
    /// (`\e[1;3D` / `\e[1;3C`, backward / forward-word).
    const BASH_META: &str = " !#$&*-./0123456789<=>?@ABCDEFGHIJKLMNPQRSTUVWXYZ\\^_bcdfglnprtuy{~";
    /// The `\e\C-X` bindings: abort, backward-kill-word, character-search-backward,
    /// clear-display, dynamic-complete-history, revert-line, shell-backward-word,
    /// shell-expand-line, shell-forward-word, shell-kill-word,
    /// shell-transpose-words, yank-nth-arg.
    const BASH_CTRL_META: &str = "gh]lirbefdty";

    /// No default binding takes a key bash's readline binds; the only
    /// defaults of their own are the prefix and the tab keys.
    #[test]
    fn defaults_leave_readline_and_emacs_alt_keys_to_the_shell() {
        let k = Keymap::default();
        let shift = |c: char| if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        let mut taken: Vec<KeyEvent> = BASH_META.chars().map(|c| key(KeyCode::Char(c), KeyModifiers::ALT | shift(c))).collect();
        taken.extend(BASH_CTRL_META.chars().map(|c| key(KeyCode::Char(c), KeyModifiers::ALT | KeyModifiers::CONTROL)));
        taken.extend([KeyCode::Backspace, KeyCode::Left, KeyCode::Right].map(|c| key(c, KeyModifiers::ALT)));
        for ev in taken {
            assert_eq!(k.match_key(&ev), None, "{:?} {:?} is bash's", ev.modifiers, ev.code);
        }
        let own: Vec<Action> = Action::ALL.into_iter().filter(|&a| k.chords(a).iter().any(|c| !c.after_prefix)).collect();
        assert_eq!(own, [Action::NextTab, Action::PrevTab, Action::Prefix]);
    }
    /// The documented `[tui_keys]` block that restores the termax Alt keys loads cleanly.
    #[test]
    fn termax_keys_can_be_restored() {
        let doc = include_str!("../../../docs/CONFIGURATION.md");
        let block = doc.split("The termax keys").nth(1).and_then(|b| b.split("```toml").nth(1)).and_then(|b| b.split("```").next()).unwrap();
        // `name = "value"  # comment` lines; the only escape used is `\\`.
        let pairs: BTreeMap<String, String> = block
            .lines()
            .filter_map(|l| l.split_once(" = "))
            .map(|(k, v)| (k.trim().to_string(), v.split('"').nth(1).unwrap().replace("\\\\", "\\")))
            .collect();
        assert_eq!(pairs.len(), Action::ALL.len(), "every action");
        let (k, errors) = Keymap::new(&pairs);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(k.match_key(&key(KeyCode::Char('o'), KeyModifiers::ALT)), Some(Action::FocusNextPane));
        assert_eq!(k.match_key(&key(KeyCode::Char('\\'), KeyModifiers::ALT)), Some(Action::SplitRight));
    }

    /// Config names are unique and round-trip through `from_config_name`.
    #[test]
    fn every_action_has_a_unique_name() {
        let names: std::collections::BTreeSet<String> = Action::ALL.iter().map(|a| a.config_name()).collect();
        assert_eq!(names.len(), Action::ALL.len());
        for a in Action::ALL {
            assert_eq!(Action::from_config_name(&a.config_name()), Some(a));
        }
    }
}
