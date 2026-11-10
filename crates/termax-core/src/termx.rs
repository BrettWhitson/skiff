//! Terminal-tab helpers shared by both front-ends: searching the output,
//! deciding when a paste needs confirming, and reporting the mouse to
//! programs that asked for it.

use serde::{Deserialize, Serialize};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// Every line of the terminal, oldest first (scrollback, then the screen),
/// and how many of them are scrollback. The view's scroll position is kept.
pub fn screen_lines(parser: &mut vt100::Parser) -> (Vec<String>, usize) {
    let screen = parser.screen_mut();
    let prev = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let history = screen.scrollback();
    let (rows, cols) = screen.size();
    let rows = rows as usize;
    let total = history + rows;
    let mut lines = Vec::with_capacity(total);
    let mut next = 0;
    while next < total {
        let top = next.min(history);
        screen.set_scrollback(history - top);
        for (i, row) in screen.rows(0, cols).enumerate() {
            if top + i >= next {
                lines.push(row.trim_end().to_string());
            }
        }
        next = top + rows;
    }
    screen.set_scrollback(prev);
    (lines, history)
}

/// One search hit: `line` indexes [`screen_lines`]; `col` / `len` in chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

/// Plain-text matches of `query`, in order. Case-insensitive unless the
/// query has an uppercase letter.
pub fn find(lines: &[String], query: &str) -> Vec<Match> {
    if query.is_empty() {
        return vec![];
    }
    let exact = query.chars().any(char::is_uppercase);
    let q: Vec<char> = if exact { query.chars().collect() } else { query.to_lowercase().chars().collect() };
    let mut out = vec![];
    for (li, line) in lines.iter().enumerate() {
        let chars: Vec<char> = if exact { line.chars().collect() } else { line.to_lowercase().chars().collect() };
        let mut i = 0;
        while i + q.len() <= chars.len() {
            if chars[i..i + q.len()] == q[..] {
                out.push(Match { line: li, col: i, len: q.len() });
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
    out
}

/// The vt100 scrollback offset that shows `line` (0 = live screen).
pub fn scroll_offset_for(line: usize, history: usize, rows: usize) -> usize {
    let _ = rows;
    history.saturating_sub(line)
}

/// How many lines a paste would send when it should be confirmed first: it
/// has a line break, the program hasn't turned on bracketed paste, and the
/// user wants the prompt.
pub fn paste_lines(text: &str, bracketed: bool, confirm: bool) -> Option<usize> {
    if !confirm || bracketed || !text.contains(['\n', '\r']) {
        return None;
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    Some(normalized.trim_end_matches('\n').split('\n').count())
}

/// A mouse event for the program in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEv {
    /// Button 0 = left, 1 = middle, 2 = right.
    Press(u8),
    Release(u8),
    Drag(u8),
    Move,
    WheelUp,
    WheelDown,
}

/// The bytes reporting `ev` at cell (`col`, `row`) (0-based), or `None` when
/// the program's mouse mode doesn't ask for it (or it can't be encoded).
#[allow(clippy::too_many_arguments)]
pub fn encode_mouse(
    mode: MouseProtocolMode,
    enc: MouseProtocolEncoding,
    ev: MouseEv,
    col: u16,
    row: u16,
    shift: bool,
    alt: bool,
    ctrl: bool,
) -> Option<Vec<u8>> {
    use MouseProtocolMode as M;
    let wanted = match (mode, ev) {
        (M::None, _) => false,
        (_, MouseEv::Press(_) | MouseEv::WheelUp | MouseEv::WheelDown) => true,
        (M::Press, _) => false,
        (_, MouseEv::Release(_)) => true,
        (M::ButtonMotion | M::AnyMotion, MouseEv::Drag(_)) => true,
        (M::AnyMotion, MouseEv::Move) => true,
        _ => false,
    };
    if !wanted {
        return None;
    }
    let sgr = enc == MouseProtocolEncoding::Sgr;
    let mut code: u32 = match ev {
        MouseEv::Press(b) | MouseEv::Drag(b) => b.min(2) as u32,
        MouseEv::Release(b) => {
            if sgr {
                b.min(2) as u32
            } else {
                3
            }
        }
        MouseEv::Move => 3,
        MouseEv::WheelUp => 64,
        MouseEv::WheelDown => 65,
    };
    if matches!(ev, MouseEv::Drag(_) | MouseEv::Move) {
        code += 32;
    }
    if mode != M::Press {
        code += (shift as u32) * 4 + (alt as u32) * 8 + (ctrl as u32) * 16;
    }
    let (x, y) = (col as u32 + 1, row as u32 + 1);
    match enc {
        MouseProtocolEncoding::Sgr => {
            let end = if matches!(ev, MouseEv::Release(_)) { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes())
        }
        MouseProtocolEncoding::Default => {
            if x > 223 || y > 223 {
                return None;
            }
            Some(vec![0x1b, b'[', b'M', (32 + code) as u8, (32 + x) as u8, (32 + y) as u8])
        }
        MouseProtocolEncoding::Utf8 => {
            let mut s = String::from("\x1b[M");
            for v in [32 + code, 32 + x, 32 + y] {
                s.push(char::from_u32(v)?);
            }
            Some(s.into_bytes())
        }
    }
}

/// A saved command (`[[snippet]]`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Snippet {
    pub name: String,
    pub command: String,
    /// Shown only in this host's tabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Press Enter after typing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub run: bool,
}

impl Snippet {
    /// What gets typed into the terminal.
    pub fn text(&self) -> String {
        let mut t = self.command.clone();
        if self.run {
            t.push('\r');
        }
        t
    }
}

/// The snippets usable in a tab connected to `host`.
pub fn snippets_for<'a>(all: &'a [Snippet], host: &str) -> Vec<&'a Snippet> {
    all.iter().filter(|s| s.host.as_deref().is_none_or(|h| h == host)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_in_scrollback() {
        let mut p = vt100::Parser::new(24, 80, 10_000);
        for i in 0..5000 {
            p.process(format!("line {i}\r\n").as_bytes());
        }
        let (lines, history) = screen_lines(&mut p);
        assert_eq!(p.screen().scrollback(), 0, "the view stays where it was");
        let m = find(&lines, "line 42");
        let at: Vec<&str> = m.iter().map(|m| lines[m.line].as_str()).collect();
        assert_eq!(at[0], "line 42");
        assert!(at.iter().all(|l| l.starts_with("line 42")), "{at:?}");
        assert_eq!(m.len(), 1 + 10 + 100, "line 42, 420-429 and 4200-4299");
        assert!(find(&lines, "LINE 42").is_empty(), "uppercase makes it case-sensitive");
        let off = scroll_offset_for(m[0].line, history, 24);
        p.screen_mut().set_scrollback(off);
        assert!(p.screen().contents().starts_with("line 42\n"), "the match is on screen");
    }

    #[test]
    fn paste_prompt_rules() {
        assert_eq!(paste_lines("a\nb", false, true), Some(2));
        assert_eq!(paste_lines("a", false, true), None);
        assert_eq!(paste_lines("a\nb", true, true), None, "bracketed paste is safe");
        assert_eq!(paste_lines("a\nb", false, false), None, "turned off");
        assert_eq!(paste_lines("a\n", false, true), Some(1), "a trailing newline would run it");
        assert_eq!(paste_lines("a\r\nb\r\n", false, true), Some(2));
    }

    #[test]
    fn mouse_encoding() {
        use MouseProtocolEncoding as E;
        use MouseProtocolMode as M;
        let e = |mode, enc, ev, col| encode_mouse(mode, enc, ev, col, 0, false, false, false);
        assert_eq!(e(M::PressRelease, E::Sgr, MouseEv::Press(0), 0), Some(b"\x1b[<0;1;1M".to_vec()));
        assert_eq!(e(M::PressRelease, E::Sgr, MouseEv::Release(0), 0), Some(b"\x1b[<0;1;1m".to_vec()));
        assert_eq!(e(M::PressRelease, E::Default, MouseEv::Press(0), 0), Some(vec![0x1b, b'[', b'M', 32, 33, 33]));
        assert_eq!(e(M::PressRelease, E::Default, MouseEv::Release(0), 0), Some(vec![0x1b, b'[', b'M', 35, 33, 33]));
        assert_eq!(e(M::PressRelease, E::Sgr, MouseEv::Move, 0), None);
        assert_eq!(e(M::AnyMotion, E::Sgr, MouseEv::Move, 0), Some(b"\x1b[<35;1;1M".to_vec()));
        assert_eq!(e(M::ButtonMotion, E::Sgr, MouseEv::Drag(0), 4), Some(b"\x1b[<32;5;1M".to_vec()));
        assert_eq!(e(M::PressRelease, E::Sgr, MouseEv::WheelUp, 0), Some(b"\x1b[<64;1;1M".to_vec()));
        assert_eq!(e(M::PressRelease, E::Default, MouseEv::Press(0), 300), None, "too far right for the default encoding");
        assert_eq!(e(M::None, E::Sgr, MouseEv::Press(0), 0), None);
        let ctrl = encode_mouse(M::PressRelease, E::Sgr, MouseEv::Press(2), 0, 0, false, false, true);
        assert_eq!(ctrl, Some(b"\x1b[<18;1;1M".to_vec()));
    }

    #[test]
    fn snippets_for_host() {
        let all = vec![
            Snippet { name: "logs".into(), command: "journalctl -f".into(), host: Some("web".into()), run: true },
            Snippet { name: "df".into(), command: "df -h".into(), host: None, run: false },
        ];
        assert_eq!(snippets_for(&all, "web").len(), 2);
        assert_eq!(snippets_for(&all, "db").iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["df"]);
        assert_eq!(all[0].text(), "journalctl -f\r");
        assert_eq!(all[1].text(), "df -h");
    }
}
