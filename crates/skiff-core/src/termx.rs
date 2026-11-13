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
    // Show the oldest lines first, then walk forward a screenful at a time.
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
        // Overlapping windows near the bottom repeat rows already taken; skip them.
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

/// The scrollback length (how far back the view can go). The view stays.
pub fn history_len(screen: &mut vt100::Screen) -> usize {
    let prev = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let n = screen.scrollback();
    screen.set_scrollback(prev);
    n
}

/// Feeds output to a parser and counts the lines it pushes into the
/// scrollback. A line's number is how many lines were pushed before it, so
/// it keeps its number as the scrollback fills and drops old lines (vt100
/// has no such counter). The view's scroll position behaves as without it.
/// Output first goes through [`crate::vtfix::VtFix`], which rewrites what
/// vt100 would ignore and collects the answers to the program's queries.
#[derive(Debug, Default)]
pub struct ScrollTracker {
    /// The rewriter; it holds unfinished escape sequences until whole, so
    /// a switch to or from the alternate screen always ends a piece.
    fix: crate::vtfix::VtFix,
    /// Answers owed to the program, oldest first.
    replies: Vec<u8>,
}

impl ScrollTracker {
    /// Processes `bytes`; returns how many lines entered the scrollback.
    pub fn process(&mut self, parser: &mut vt100::Parser, bytes: &[u8]) -> u64 {
        let mut segs = Vec::new();
        self.fix.feed(bytes, &mut segs);
        let mut pushed = 0;
        for seg in segs {
            let data = match seg {
                crate::vtfix::Seg::Bytes(b) => b,
                crate::vtfix::Seg::Op(op) => crate::vtfix::resolve(&op, parser.screen(), &mut self.replies),
            };
            let mut i = 0;
            while i < data.len() {
                let end = piece_end(&data, i);
                pushed += piece(parser, &data[i..end]);
                i = end;
            }
        }
        pushed
    }

    /// Replies owed to the program (answers to its queries), oldest first.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }
}

/// Pieces start at each ESC, and a CSI sequence is a piece of its own: a
/// mode change then only ever ends a piece, so the lines of one piece all
/// go to one screen.
fn piece_end(d: &[u8], i: usize) -> usize {
    if d[i] == 0x1b && d.get(i + 1) == Some(&b'[') {
        let mut j = i + 2;
        while j < d.len() && (0x20..=0x3f).contains(&d[j]) {
            j += 1;
        }
        return (j + 1).min(d.len());
    }
    d[i + 1..].iter().position(|&b| b == 0x1b).map_or(d.len(), |p| i + 1 + p)
}

/// Processes one piece. vt100 moves a scrolled-back view up a line for each
/// line pushed (up to the scrollback's length), so the view is parked one
/// line back to count them, then put where vt100 would have left it. Parts
/// of the piece go in with fewer line breaks than the scrollback holds.
fn piece(parser: &mut vt100::Parser, mut seg: &[u8]) -> u64 {
    let reset = seg.starts_with(b"\x1bc");
    let mut total = 0;
    while !seg.is_empty() {
        if parser.screen().alternate_screen() {
            // The alternate screen keeps no scrollback.
            parser.process(seg);
            break;
        }
        let screen = parser.screen_mut();
        let prev = screen.scrollback();
        let len0 = history_len(screen);
        // Probe: if the view can sit one line back, vt100 will move it per pushed line.
        screen.set_scrollback(1);
        let probe = screen.scrollback() == 1;
        let limit = if probe { len0.saturating_sub(1).max(1) } else { 1 };
        let (_, cols) = screen.size();
        let (_, col) = screen.cursor_position();
        let (now, rest) = seg.split_at(after_rows(seg, limit, col, cols));
        seg = rest;
        parser.process(now);
        let screen = parser.screen_mut();
        if screen.alternate_screen() {
            continue; // switching reset the main view to live output, as it does anyway
        }
        let off = screen.scrollback();
        let len1 = history_len(screen);
        // Pushed lines: how far the parked view moved, or the scrollback growth if larger.
        let pushed = if probe { off.saturating_sub(1) } else { 0 }.max(len1.saturating_sub(len0));
        // Restore the view: a scrolled-back view follows its text; a reset returns it to live.
        screen.set_scrollback(if prev > 0 && !reset { prev + pushed } else { 0 });
        total += pushed as u64;
    }
    total
}

/// The length of `seg` up to and including whatever may start its `n`th new
/// row (all of it if nothing more can): a line break, or a character that
/// auto-wraps past `cols` from cursor column `col`. Columns are overcounted
/// rather than under (non-ASCII as wide, escape bytes as printable), so a
/// part never pushes more rows than counted.
fn after_rows(seg: &[u8], n: usize, col: u16, cols: u16) -> usize {
    let cols = usize::from(cols.max(1));
    // One past the cursor: a wrap may be pending at the right edge.
    let (mut col, mut rows) = (usize::from(col) + 1, 0);
    let mut i = 0;
    while i < seg.len() {
        let b = seg[i];
        let (width, len) = match b {
            b'\n' | 0x0b | 0x0c => {
                rows += 1;
                (0, 1)
            }
            b'\r' => {
                col = 0;
                (0, 1)
            }
            0x00..=0x1f | 0x7f => (0, 1),
            0x20..=0x7e => (1, 1),
            0xc0..=0xdf => (2, 2),
            0xe0..=0xef => (2, 3),
            0xf0..=0xff => (2, 4),
            _ => (0, 1), // stray continuation byte
        };
        if width > 0 {
            if col + width > cols {
                rows += 1;
                col = 0;
            }
            col += width;
        }
        i = (i + len).min(seg.len());
        if rows >= n {
            return i;
        }
    }
    seg.len()
}

/// One search hit: `line` is a line number (see [`ScrollTracker`]; for
/// [`find`], the index into its lines); `col` / `len` in chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Line number of the hit.
    pub line: u64,
    /// Start column, in chars.
    pub col: usize,
    /// Length of the hit, in chars.
    pub len: usize,
}

/// A prepared query: case-insensitive unless it has an uppercase letter.
struct Query {
    /// Match case exactly (the query had an uppercase letter).
    exact: bool,
    /// The query's chars, lowercased unless `exact`.
    chars: Vec<char>,
}

impl Query {
    /// Prepares `q`; `None` for an empty query.
    fn new(q: &str) -> Option<Query> {
        if q.is_empty() {
            return None;
        }
        let exact = q.chars().any(char::is_uppercase);
        Some(Query { exact, chars: if exact { q.chars().collect() } else { q.to_lowercase().chars().collect() } })
    }

    /// Appends the non-overlapping hits in `line`, numbered `at`, to `out`.
    fn find_in(&self, line: &str, at: u64, out: &mut Vec<Match>) {
        let q = &self.chars;
        let chars: Vec<char> = if self.exact { line.chars().collect() } else { line.to_lowercase().chars().collect() };
        let mut i = 0;
        while i + q.len() <= chars.len() {
            if chars[i..i + q.len()] == q[..] {
                out.push(Match { line: at, col: i, len: q.len() });
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
}

/// Plain-text matches of `query`, in order. Case-insensitive unless the
/// query has an uppercase letter.
pub fn find(lines: &[String], query: &str) -> Vec<Match> {
    let mut out = vec![];
    if let Some(q) = Query::new(query) {
        for (i, line) in lines.iter().enumerate() {
            q.find_in(line, i as u64, &mut out);
        }
    }
    out
}

/// Columns `start..start + width` of the row `idx` rows from the oldest
/// (scrollback, then screen). Moves the view; callers put it back.
fn row_text(screen: &mut vt100::Screen, history: usize, idx: usize, start: u16, width: u16) -> Option<String> {
    let rows = screen.size().0 as usize;
    let (off, row) = if idx < history { (history - idx, 0) } else { (0, idx - history) };
    if row >= rows {
        return None;
    }
    screen.set_scrollback(off);
    screen.rows(start, width).nth(row).map(|r| r.trim_end().to_string())
}

/// The text from `a` to `b`, each `(line, col)` with line numbers as from
/// [`ScrollTracker`] (`scrolled` is its count) and `b`'s column included;
/// one line per row. The view stays.
pub fn text_between(parser: &mut vt100::Parser, scrolled: u64, a: (u64, u16), b: (u64, u16)) -> String {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let screen = parser.screen_mut();
    let prev = screen.scrollback();
    let history = history_len(screen);
    let cols = screen.size().1;
    let base = scrolled.saturating_sub(history as u64);
    let mut out = vec![];
    for line in a.0.max(base)..=b.0 {
        let from = if line == a.0 { a.1.min(cols) } else { 0 };
        let to = if line == b.0 { b.1.saturating_add(1).min(cols) } else { cols };
        match row_text(screen, history, (line - base) as usize, from, to.saturating_sub(from)) {
            Some(t) => out.push(t),
            None => break,
        }
    }
    screen.set_scrollback(prev);
    out.join("\n")
}

/// Find in a terminal. The lines are cached and numbered as by
/// [`ScrollTracker`], so new output reads only what changed, typing
/// searches the cache without the parser, and matches stay on their text
/// as the scrollback shifts.
#[derive(Debug, Default)]
pub struct TermSearch {
    /// The current search text.
    query: String,
    /// Scrollback lines, oldest first; `hist[0]` is line `base`.
    hist: std::collections::VecDeque<String>,
    base: u64,
    /// Screen rows, numbered after the scrollback.
    screen: Vec<String>,
    /// Whether the cached lines came from the alternate screen.
    alt: bool,
    /// (generation, scrolled count, size) the lines were read at.
    stamp: Option<(u64, u64, (u16, u16))>,
    /// Matches from this line on need finding again.
    stale: Option<u64>,
    /// Matches in line order.
    matches: Vec<Match>,
    /// Index of the current match in `matches`.
    current: usize,
}

impl TermSearch {
    /// The current search text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// All matches, oldest line first.
    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    /// Index into [`Self::matches`].
    pub fn current(&self) -> usize {
        self.current
    }

    /// Brings the lines (when the output changed) and the matches up to
    /// date; true if anything was read. `scrolled` is the
    /// [`ScrollTracker`] count for this parser.
    pub fn refresh(&mut self, parser: &mut vt100::Parser, scrolled: u64, generation: u64) -> bool {
        let changed = self.read_lines(parser, scrolled, generation);
        self.rematch();
        changed
    }

    /// [`Self::refresh`] for a session: only reading the lines holds its lock.
    pub fn sync(&mut self, s: &crate::terminal::TerminalSession) -> bool {
        let changed = match s.parser.lock() {
            Ok(mut p) => self.read_lines(&mut p, s.scrolled(), s.generation()),
            Err(_) => false,
        };
        self.rematch();
        changed
    }

    /// Reads what changed: the new scrollback lines and the screen.
    fn read_lines(&mut self, parser: &mut vt100::Parser, scrolled: u64, generation: u64) -> bool {
        let screen = parser.screen_mut();
        let size = screen.size();
        let stamp = (generation, scrolled, size);
        if self.stamp == Some(stamp) {
            return false;
        }
        let alt = screen.alternate_screen();
        let prev = screen.scrollback();
        let history = history_len(screen);
        let base = scrolled.saturating_sub(history as u64);
        // Start over when nothing is cached, the screen switched, or lines were lost.
        let mut full = self.stamp.is_none() || alt != self.alt || base < self.base;
        if !full && let Some(last) = self.hist.back() {
            // Check the count: the newest cached line is still where expected.
            let at = self.base + self.hist.len() as u64 - 1;
            if at >= base {
                full = row_text(screen, history, (at - base) as usize, 0, size.1).as_deref() != Some(last.as_str());
            }
        }
        let old_end = self.base + self.hist.len() as u64;
        if full {
            self.hist.clear();
            self.base = base;
            self.stale = Some(0);
        // Drop cached lines that left the scrollback; re-search from the old end.
        } else {
            while self.base < base && self.hist.pop_front().is_some() {
                self.base += 1;
            }
            if self.hist.is_empty() {
                self.base = base;
            }
            self.stale = Some(self.stale.map_or(old_end, |s| s.min(old_end)));
        }
        // New scrollback lines, a screenful at a time.
        let rows = size.0 as usize;
        let mut next = self.hist.len();
        while next < history {
            screen.set_scrollback(history - next);
            let take = rows.min(history - next);
            self.hist.extend(screen.rows(0, size.1).take(take).map(|r| r.trim_end().to_string()));
            next += take;
        }
        screen.set_scrollback(0);
        self.screen = screen.rows(0, size.1).map(|r| r.trim_end().to_string()).collect();
        screen.set_scrollback(prev);
        self.alt = alt;
        self.stamp = Some(stamp);
        true
    }

    /// Finds matches in the lines read since last time, keeping the current
    /// match on the same text (or the next newer one when it went away).
    fn rematch(&mut self) {
        let Some(from) = self.stale.take() else {
            return;
        };
        let was = self.matches.get(self.current).copied();
        let base = self.base;
        self.matches.retain(|m| m.line >= base && m.line < from);
        if let Some(q) = Query::new(&self.query) {
            let start = from.max(base);
            for (i, line) in self.hist.iter().enumerate().skip((start - base) as usize) {
                q.find_in(line, base + i as u64, &mut self.matches);
            }
            let top = base + self.hist.len() as u64;
            for (i, line) in self.screen.iter().enumerate() {
                if top + i as u64 >= start {
                    q.find_in(line, top + i as u64, &mut self.matches);
                }
            }
        }
        // Keep the current match on the same text, or move to the next newer one.
        let last = self.matches.len().saturating_sub(1);
        self.current = match was {
            None => last,
            Some(w) => self.matches.partition_point(|m| (m.line, m.col) < (w.line, w.col)).min(last),
        };
    }

    /// Searches for `q`; the newest match becomes current. False if unchanged.
    pub fn set_query(&mut self, q: &str) -> bool {
        if q == self.query {
            return false;
        }
        self.query = q.to_string();
        self.matches.clear();
        self.stale = Some(0);
        self.rematch();
        self.current = self.matches.len().saturating_sub(1);
        true
    }

    /// Next older (`older`) or newer match, wrapping; false without matches.
    pub fn step(&mut self, older: bool) -> bool {
        let n = self.matches.len();
        if n == 0 {
            return false;
        }
        self.current = if older { (self.current + n - 1) % n } else { (self.current + 1) % n };
        true
    }

    /// The scrollback offset that shows the current match a third of the
    /// way down a view of `rows` rows (0 when it is on the live screen).
    pub fn offset(&self, scrolled: u64, rows: usize) -> Option<usize> {
        let m = self.matches.get(self.current)?;
        Some(if m.line >= scrolled { 0 } else { (scrolled - m.line) as usize + rows / 3 })
    }

    /// Visible matches as (row, col, len, is_current) for a view whose top
    /// row is line `top` (the scrolled count minus the view's offset).
    pub fn marks(&self, top: u64, rows: usize) -> Vec<(usize, usize, usize, bool)> {
        let lo = self.matches.partition_point(|m| m.line < top);
        self.matches[lo..]
            .iter()
            .enumerate()
            .take_while(|(_, m)| m.line < top + rows as u64)
            .map(|(i, m)| ((m.line - top) as usize, m.col, m.len, lo + i == self.current))
            .collect()
    }
}

/// The OSC 52 sequence that puts `text` on the clipboard of the terminal
/// it is written to.
pub fn osc52(text: &str) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("\x1b]52;c;");
    // Base64-encode three bytes into four chars, padding the last group with '='.
    for chunk in text.as_bytes().chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            out.push(if k <= chunk.len() { ABC[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out.push('\x07');
    out
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
    /// Button pressed.
    /// Button 0 = left, 1 = middle, 2 = right.
    Press(u8),
    /// Button released.
    Release(u8),
    /// Moved with the button held.
    Drag(u8),
    /// Moved with no button held.
    Move,
    /// Wheel scrolled up.
    WheelUp,
    /// Wheel scrolled down.
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
    // Which events the program's mouse mode wants reported.
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
    // Button code; the default encodings cannot say which button was released.
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
    // Modifier bits are not reported in X10 (press-only) mode.
    if mode != M::Press {
        code += (shift as u32) * 4 + (alt as u32) * 8 + (ctrl as u32) * 16;
    }
    let (x, y) = (col as u32 + 1, row as u32 + 1);
    match enc {
        MouseProtocolEncoding::Sgr => {
            let end = if matches!(ev, MouseEv::Release(_)) { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes())
        }
        // The classic encoding fits coordinates in one byte each.
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
    /// Name shown in the snippet list.
    pub name: String,
    /// The command text; may span several lines.
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

/// How a snippet reaches the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnippetSend {
    /// One line: typed as is (with its Enter if it has one).
    Type(String),
    /// Several lines: pasted (the paste rule applies), then Enter if
    /// `enter`, outside any bracketed paste so the shell runs it.
    Paste { body: String, enter: bool },
}

impl Snippet {
    /// How to deliver this snippet: typed for one line, pasted for several.
    pub fn send(&self) -> SnippetSend {
        if self.command.contains(['\n', '\r']) {
            SnippetSend::Paste { body: self.command.clone(), enter: self.run }
        } else {
            SnippetSend::Type(self.text())
        }
    }
}

/// The snippets usable in a tab connected to `host`.
pub fn snippets_for<'a>(all: &'a [Snippet], host: &str) -> Vec<&'a Snippet> {
    all.iter().filter(|s| s.host.as_deref().is_none_or(|h| h == host)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `s` through the tracker and adds the pushed-line count to `total`.
    fn feed(p: &mut vt100::Parser, t: &mut ScrollTracker, total: &mut u64, s: &str) {
        *total += t.process(p, s.as_bytes());
    }

    /// Search reads the whole scrollback, keeps the view, honours smart case, and scrolls to a match.
    #[test]
    fn search_finds_in_scrollback() {
        let (mut p, mut t, mut n) = (vt100::Parser::new(24, 80, 10_000), ScrollTracker::default(), 0);
        let lines: String = (0..5000).map(|i| format!("line {i}\r\n")).collect();
        feed(&mut p, &mut t, &mut n, &lines);
        let mut s = TermSearch::default();
        assert!(s.refresh(&mut p, n, 1));
        assert!(!s.refresh(&mut p, n, 1), "nothing new, nothing read");
        assert_eq!(p.screen().scrollback(), 0, "the view stays where it was");
        s.set_query("line 42");
        assert_eq!(s.matches().len(), 1 + 10 + 100, "line 42, 420-429 and 4200-4299");
        s.set_query("LINE 42");
        assert!(s.matches().is_empty(), "uppercase makes it case-sensitive");
        s.set_query("line 42");
        while s.current() != 0 {
            s.step(true);
        }
        p.screen_mut().set_scrollback(s.offset(n, 24).unwrap());
        let shown = p.screen().contents();
        assert_eq!(shown.lines().nth(8), Some("line 42"), "the match shows a third of the way down: {shown:?}");
        assert_eq!(s.marks(n - p.screen().scrollback() as u64, 24), vec![(8, 0, 7, true)]);
    }

    /// Line counts stay right once the scrollback is full, across alternate-screen switches.
    #[test]
    fn tracker_counts_lines_past_a_full_scrollback() {
        let (mut p, mut t, mut n) = (vt100::Parser::new(5, 20, 10), ScrollTracker::default(), 0);
        let lines = |a: usize, b: usize| (a..b).map(|i| format!("l{i}\r\n")).collect::<String>();
        feed(&mut p, &mut t, &mut n, &lines(0, 30));
        assert_eq!(n, 26, "rows 0-25 left the 5-row screen");
        assert_eq!(p.screen().scrollback(), 0);
        p.screen_mut().set_scrollback(3);
        feed(&mut p, &mut t, &mut n, &lines(30, 32));
        assert_eq!((n, p.screen().scrollback()), (28, 5), "a scrolled-back view moves with its text, as in vt100");
        p.screen_mut().set_scrollback(0);
        // The alternate screen keeps nothing; the switch may be split across reads.
        feed(&mut p, &mut t, &mut n, "\x1b[?10");
        feed(&mut p, &mut t, &mut n, &format!("49h{}\x1b[?104", lines(0, 9)));
        assert_eq!(n, 28);
        feed(&mut p, &mut t, &mut n, &format!("9l{}", lines(32, 35)));
        assert_eq!((n, p.screen().scrollback()), (31, 0));
    }

    /// Wrapped long rows (including wide chars) are counted the same as vt100's own scrollback.
    #[test]
    fn tracker_counts_wrapped_rows_past_a_full_scrollback() {
        let (mut p, mut t, mut n) = (vt100::Parser::new(5, 10, 10), ScrollTracker::default(), 0);
        let mut reference = vt100::Parser::new(5, 10, 10_000);
        let mut out = (0..30).map(|i| format!("l{i}\r\n")).collect::<String>();
        out += &"x".repeat(200);
        // The second read starts with a wrap pending at the right edge.
        let more = format!("{}{}\r\nend\r\n", "x".repeat(5), "ü".repeat(33));
        feed(&mut p, &mut t, &mut n, &out);
        feed(&mut p, &mut t, &mut n, &more);
        out += &more;
        reference.process(out.as_bytes());
        assert_eq!(n, history_len(reference.screen_mut()) as u64);
    }

    /// Matches keep their lines as the scrollback shifts, and vanish when their line is dropped.
    #[test]
    fn search_matches_follow_the_scrollback() {
        let (mut p, mut t, mut n) = (vt100::Parser::new(3, 20, 10), ScrollTracker::default(), 0);
        let mut s = TermSearch::default();
        let lines = |a: usize, b: usize| (a..b).map(|i| format!("f{i}\r\n")).collect::<String>();
        feed(&mut p, &mut t, &mut n, &format!("x0\r\nx1\r\nx2\r\nneedle\r\n{}", lines(0, 4)));
        s.refresh(&mut p, n, 1);
        s.set_query("needle");
        let m = s.matches()[0];
        // The scrollback fills up and shifts: the match keeps its line.
        feed(&mut p, &mut t, &mut n, &lines(4, 10));
        s.refresh(&mut p, n, 2);
        assert_eq!(s.matches(), [m]);
        p.screen_mut().set_scrollback(s.offset(n, 3).unwrap());
        assert_eq!(p.screen().contents().lines().nth(1), Some("needle"), "{}", p.screen().contents());
        // New output that matches is found without losing the current match.
        p.screen_mut().set_scrollback(0);
        feed(&mut p, &mut t, &mut n, "needle 2\r\n");
        s.refresh(&mut p, n, 3);
        assert_eq!((s.matches().len(), s.current()), (2, 0));
        // Once its line is dropped, so is the match.
        feed(&mut p, &mut t, &mut n, &(0..12).map(|i| format!("g{i}\r\n")).collect::<String>());
        s.refresh(&mut p, n, 4);
        assert!(s.matches().is_empty(), "{:?}", s.matches());
    }

    /// Selected text spans scrollback and screen rows; OSC 52 base64 is correct.
    #[test]
    fn text_between_spans_scrollback_and_screen() {
        let (mut p, mut t, mut n) = (vt100::Parser::new(3, 20, 10), ScrollTracker::default(), 0);
        feed(&mut p, &mut t, &mut n, "alpha one\r\nbeta two\r\ngamma three\r\ndelta");
        assert_eq!(n, 1);
        assert_eq!(text_between(&mut p, n, (2, 6), (0, 4)), "a one\nbeta two\ngamma t");
        assert_eq!(text_between(&mut p, n, (3, 1), (3, 2)), "el");
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
        assert_eq!(osc52("abc"), "\x1b]52;c;YWJj\x07");
    }

    /// When a multi-line paste needs confirming.
    #[test]
    fn paste_prompt_rules() {
        assert_eq!(paste_lines("a\nb", false, true), Some(2));
        assert_eq!(paste_lines("a", false, true), None);
        assert_eq!(paste_lines("a\nb", true, true), None, "bracketed paste is safe");
        assert_eq!(paste_lines("a\nb", false, false), None, "turned off");
        assert_eq!(paste_lines("a\n", false, true), Some(1), "a trailing newline would run it");
        assert_eq!(paste_lines("a\r\nb\r\n", false, true), Some(2));
    }

    /// Mouse reports for each mode and encoding, including modifiers and range limits.
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

    /// A multi-line snippet is pasted, with Enter sent separately when it runs.
    #[test]
    fn multi_line_run_snippet_enters_after_the_paste() {
        let s = |command: &str, run| Snippet { name: "x".into(), command: command.into(), host: None, run };
        assert_eq!(s("ls", true).send(), SnippetSend::Type("ls\r".into()));
        assert_eq!(s("a\nb", true).send(), SnippetSend::Paste { body: "a\nb".into(), enter: true });
        assert_eq!(s("a\nb", false).send(), SnippetSend::Paste { body: "a\nb".into(), enter: false });
    }

    /// Host-specific snippets show only in that host's tabs; `run` adds Enter.
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
