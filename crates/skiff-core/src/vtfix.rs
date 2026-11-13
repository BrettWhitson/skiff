//! Fills the gaps in the `vt100` parser. Programs routinely send
//! sequences it ignores (the line-drawing character set, REP, HVP, `CSI s`
//! / `CSI u`, IND / NEL, insert mode, autowrap off, and more), which leaves
//! text missing and the cursor in the wrong place. [`VtFix`] rewrites them
//! into sequences vt100 does handle, and answers the queries a terminal is
//! expected to answer (cursor position, status, device attributes) at the
//! point in the output where they were asked.

/// One piece of rewritten output, in order.
#[derive(Debug, PartialEq)]
pub enum Seg {
    /// Bytes vt100 handles as they are.
    Bytes(Vec<u8>),
    /// Something that depends on the screen at this point in the output.
    Op(Op),
}

/// Work done where it falls in the output, after everything before it.
#[derive(Debug, PartialEq)]
pub enum Op {
    /// Send these bytes back to the program (a fixed reply).
    Reply(&'static [u8]),
    /// Cursor position report (`CSI 6 n`).
    CursorReport,
    /// Back tab `n` stops (`CSI n Z`).
    BackTab(u16),
    /// Autowrap is off and a character follows: keep it on the last column.
    NoWrap,
}

/// Where the rewriter is in the escape-sequence grammar.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum State {
    /// Plain text and control characters.
    #[default]
    Ground,
    /// After ESC.
    Esc,
    /// After ESC and one or more intermediate bytes (`(`, `)`, `#`, ...).
    EscInter,
    /// Inside `ESC [ ...`, before its final byte.
    Csi,
    /// Inside an OSC, DCS, SOS, PM or APC string (passed through).
    Str,
    /// An ESC inside such a string: `\` ends it, anything else starts a new sequence.
    StrEsc,
}

/// Longest CSI kept; longer ones are dropped, as vte drops them.
const MAX_CSI: usize = 256;
/// Most characters one REP may write.
const MAX_REP: u16 = 4096;

/// DEC special graphics for `_` and `` ` `` through `~`.
const DEC_GRAPHICS: [char; 32] = [
    ' ', '◆', '▒', '␉', '␌', '␍', '␊', '°', '±', '␤', '␋', '┘', '┐', '┌', '└', '┼', '⎺', '⎻', '─', '⎼', '⎽', '├', '┤', '┴', '┬', '│', '≤',
    '≥', 'π', '≠', '£', '·',
];

/// The streaming rewriter. Escape sequences split across reads are held
/// until complete, so everything it passes on is whole.
#[derive(Debug, Default)]
pub struct VtFix {
    /// Grammar state.
    state: State,
    /// The escape sequence being collected (from its ESC).
    seq: Vec<u8>,
    /// Whether G0 / G1 is the line-drawing set.
    dec: [bool; 2],
    /// Shift Out is active: G1 is in use.
    shifted: bool,
    /// Insert mode (`CSI 4 h`).
    insert: bool,
    /// Autowrap is off (`CSI ? 7 l`).
    nowrap: bool,
    /// The last character written, for REP.
    last: Option<char>,
    /// Bytes of an unfinished UTF-8 character.
    utf8: Vec<u8>,
    /// Output bytes not yet handed over as a segment.
    cur: Vec<u8>,
}

impl VtFix {
    /// Rewrites `bytes`, appending the result to `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Seg>) {
        let mut i = 0;
        while i < bytes.len() {
            // Fast path: plain text with nothing to rewrite is copied in runs.
            let plain =
                self.state == State::Ground && self.utf8.is_empty() && !self.dec[self.shifted as usize] && !self.insert && !self.nowrap;
            if plain {
                let rest = &bytes[i..];
                let run = rest.iter().position(|&b| matches!(b, 0x1b | 0x0e | 0x0f)).unwrap_or(rest.len());
                // A character cut off by the end of the read goes the slow way.
                let whole = whole_utf8(&rest[..run]);
                if whole > 0 {
                    let text = &rest[..whole];
                    self.cur.extend_from_slice(text);
                    if let Some(c) = last_graphic(text) {
                        self.last = Some(c);
                    }
                    i += whole;
                    continue;
                }
            }
            self.byte(bytes[i], out);
            i += 1;
        }
        self.flush(out);
    }

    /// Hands the pending bytes over as one segment.
    fn flush(&mut self, out: &mut Vec<Seg>) {
        if !self.cur.is_empty() {
            out.push(Seg::Bytes(std::mem::take(&mut self.cur)));
        }
    }

    /// Queues `op` after everything so far.
    fn op(&mut self, op: Op, out: &mut Vec<Seg>) {
        self.flush(out);
        out.push(Seg::Op(op));
    }

    /// Starts a new escape sequence.
    fn begin_esc(&mut self) {
        self.seq.clear();
        self.seq.push(0x1b);
        self.state = State::Esc;
    }

    /// Steps the grammar with one byte.
    fn byte(&mut self, b: u8, out: &mut Vec<Seg>) {
        match self.state {
            State::Ground => self.ground(b, out),
            State::Esc | State::EscInter => match b {
                0x1b => self.begin_esc(),
                0x18 | 0x1a => {
                    self.state = State::Ground;
                    self.cur.push(b);
                }
                0x00..=0x1f => self.cur.push(b), // executed mid-sequence
                0x20..=0x2f => {
                    self.seq.push(b);
                    self.state = State::EscInter;
                }
                b'[' if self.state == State::Esc => {
                    self.seq.push(b);
                    self.state = State::Csi;
                }
                b']' | b'P' | b'X' | b'^' | b'_' if self.state == State::Esc => {
                    self.cur.extend_from_slice(&[0x1b, b]);
                    self.state = State::Str;
                }
                _ => {
                    self.state = State::Ground;
                    self.esc_final(b);
                }
            },
            State::Csi => match b {
                0x1b => self.begin_esc(),
                0x18 | 0x1a => {
                    self.state = State::Ground;
                    self.cur.push(b);
                }
                0x00..=0x1f => self.cur.push(b),
                0x20..=0x3f if self.seq.len() < MAX_CSI => self.seq.push(b),
                0x20..=0x3f => self.state = State::Ground, // too long: dropped
                0x40..=0x7e => {
                    self.state = State::Ground;
                    let seq = std::mem::take(&mut self.seq);
                    self.csi(&seq[2..], b, out);
                    self.seq = seq;
                }
                _ => {
                    self.state = State::Ground;
                    self.ground(b, out);
                }
            },
            State::Str => match b {
                0x1b => self.state = State::StrEsc,
                0x07 | 0x18 | 0x1a => {
                    self.cur.push(b);
                    self.state = State::Ground;
                }
                _ => self.cur.push(b),
            },
            State::StrEsc => {
                if b == b'\\' {
                    self.cur.extend_from_slice(b"\x1b\\");
                    self.state = State::Ground;
                } else {
                    // A new sequence ends the string, as in vte.
                    self.begin_esc();
                    self.byte(b, out);
                }
            }
        }
    }

    /// Plain text: maps the line-drawing set, assembles UTF-8, and writes.
    fn ground(&mut self, b: u8, out: &mut Vec<Seg>) {
        if !(0x80..0xc0).contains(&b) && !self.utf8.is_empty() {
            self.cur.append(&mut self.utf8); // a broken character goes through as it was
        }
        match b {
            0x1b => self.begin_esc(),
            0x0e => self.shifted = true,
            0x0f => self.shifted = false,
            0x00..=0x1f | 0x7f => self.cur.push(b),
            0x20..=0x7e => {
                let c = if self.dec[self.shifted as usize] && b >= 0x5f { DEC_GRAPHICS[(b - 0x5f) as usize] } else { b as char };
                self.put(c, out);
            }
            _ => {
                self.utf8.push(b);
                let need = match self.utf8[0] {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    _ => 1,
                };
                if self.utf8.len() >= need {
                    let bytes = std::mem::take(&mut self.utf8);
                    match std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()) {
                        Some(c) => self.put(c, out),
                        None => self.cur.extend_from_slice(&bytes),
                    }
                }
            }
        }
    }

    /// Writes one character, with insert mode and autowrap-off applied.
    fn put(&mut self, c: char, out: &mut Vec<Seg>) {
        if self.nowrap {
            self.op(Op::NoWrap, out);
        }
        if self.insert {
            let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if w > 0 {
                self.cur.extend_from_slice(format!("\x1b[{w}@").as_bytes());
            }
        }
        let mut buf = [0u8; 4];
        self.cur.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        self.last = Some(c);
    }

    /// Back to the power-on modes (RIS, DECSTR).
    fn reset_modes(&mut self) {
        (self.dec, self.shifted, self.insert, self.nowrap) = ([false; 2], false, false, false);
    }

    /// An escape sequence's final byte; `self.seq` holds what came before.
    fn esc_final(&mut self, b: u8) {
        match (&self.seq[1..], b) {
            // Character sets: G0 / G1 as line drawing (`0`) or text.
            ([b'('], _) => self.dec[0] = b == b'0',
            ([b')'], _) => self.dec[1] = b == b'0',
            ([], b'D') => self.cur.push(b'\n'),                // IND
            ([], b'E') => self.cur.extend_from_slice(b"\r\n"), // NEL
            ([], b'c') => {
                self.reset_modes();
                self.cur.extend_from_slice(b"\x1bc");
            }
            _ => {
                self.cur.extend_from_slice(&self.seq);
                self.cur.push(b);
            }
        }
    }

    /// A complete CSI: `body` is everything between `ESC [` and `fin`.
    fn csi(&mut self, body: &[u8], fin: u8, out: &mut Vec<Seg>) {
        let (prefix, rest) = match body.first() {
            Some(&p @ (b'?' | b'>' | b'<' | b'=')) => (Some(p), &body[1..]),
            _ => (None, body),
        };
        // Only digits and separators: no intermediates.
        let numeric = rest.iter().all(|b| (0x30..=0x3b).contains(b));
        let plain = prefix.is_none() && numeric;
        let first = param(rest, 0);
        let p = String::from_utf8_lossy(rest);
        let mut emit = |s: String| self.cur.extend_from_slice(s.as_bytes());
        match (prefix, fin) {
            (None, b'f') if plain => emit(format!("\x1b[{p}H")), // HVP
            (None, b'`') if plain => emit(format!("\x1b[{p}G")), // HPA
            (None, b'a') if plain => emit(format!("\x1b[{p}C")), // HPR
            (None, b'e') if plain => emit(format!("\x1b[{p}B")), // VPR
            (None, b'I') if plain => emit("\t".repeat(first.unwrap_or(1).clamp(1, 256).into())),
            (None, b's') if rest.is_empty() => emit("\x1b7".into()),
            (None, b'u') if rest.is_empty() => emit("\x1b8".into()),
            (None, b'b') if plain => {
                if let Some(c) = self.last {
                    for _ in 0..first.unwrap_or(1).clamp(1, MAX_REP) {
                        self.put(c, out);
                    }
                }
            }
            (None, b'Z') if plain => self.op(Op::BackTab(first.unwrap_or(1).clamp(1, 256)), out),
            // ANSI modes: only insert mode does anything.
            (None, b'h' | b'l') if plain => {
                if params(rest).any(|n| n == Some(4)) {
                    self.insert = fin == b'h';
                }
            }
            (None, b'n') if plain => match first {
                Some(5) => self.op(Op::Reply(b"\x1b[0n"), out),
                Some(6) => self.op(Op::CursorReport, out),
                _ => {}
            },
            (None, b'c') if plain && first.unwrap_or(0) == 0 => self.op(Op::Reply(b"\x1b[?62;22c"), out),
            (Some(b'>'), b'c') if numeric && first.unwrap_or(0) == 0 => self.op(Op::Reply(b"\x1b[>1;10;0c"), out),
            // DEC private modes, one per sequence so each can be rewritten.
            (Some(b'?'), b'h' | b'l') if numeric => {
                let (set, f) = (fin == b'h', fin as char);
                for n in params(rest).flatten() {
                    match n {
                        7 => self.nowrap = !set,
                        1047 => emit(format!("\x1b[?47{f}")),
                        1048 => emit(if set { "\x1b7" } else { "\x1b8" }.into()),
                        _ => emit(format!("\x1b[?{n}{f}")),
                    }
                }
            }
            _ => {
                if (prefix, fin) == (None, b'p') && rest == b"!" {
                    self.reset_modes(); // DECSTR
                }
                self.cur.extend_from_slice(b"\x1b[");
                self.cur.extend_from_slice(body);
                self.cur.push(fin);
            }
        }
    }
}

/// How much of `text` ends on a character boundary: all of it, unless it
/// ends partway through a UTF-8 character.
fn whole_utf8(text: &[u8]) -> usize {
    let n = text.len();
    // The last lead byte within reach of the end, and how long its character is.
    for back in 1..=n.min(4) {
        let b = text[n - back];
        if b < 0x80 {
            return n;
        }
        if b >= 0xc0 {
            let need = match b {
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            return if back < need { n - back } else { n };
        }
    }
    n
}

/// The last printable character in `text` (what REP repeats).
fn last_graphic(text: &[u8]) -> Option<char> {
    let mut end = text.len();
    while end > 0 {
        let mut start = end - 1;
        while start > 0 && (0x80..0xc0).contains(&text[start]) {
            start -= 1;
        }
        let b = text[start];
        if b >= 0x20 && b != 0x7f {
            return std::str::from_utf8(&text[start..end]).ok()?.chars().next();
        }
        end = start;
    }
    None
}

/// The CSI parameters in `rest` (`None` where empty or not a number).
fn params(rest: &[u8]) -> impl Iterator<Item = Option<u16>> + '_ {
    rest.split(|&b| b == b';').map(|p| std::str::from_utf8(p).ok().and_then(|s| s.split(':').next()?.parse().ok()))
}

/// The `i`th CSI parameter, if present.
fn param(rest: &[u8], i: usize) -> Option<u16> {
    params(rest).nth(i).flatten()
}

/// The cell the cursor is on, as a terminal shows it. After a character
/// fills the last column, vt100 puts the cursor one past it (a pending
/// wrap); terminals keep showing and reporting it on the last column.
pub fn cursor_cell(screen: &vt100::Screen) -> (u16, u16) {
    let (row, col) = screen.cursor_position();
    (row, col.min(screen.size().1.saturating_sub(1)))
}

/// Resolves `op` against the screen as it is now: returns bytes for the
/// parser and appends any reply for the program to `replies`.
pub fn resolve(op: &Op, screen: &vt100::Screen, replies: &mut Vec<u8>) -> Vec<u8> {
    let at = screen.cursor_position().1;
    let cols = screen.size().1.max(1);
    let (row, col) = cursor_cell(screen);
    match op {
        Op::Reply(r) => replies.extend_from_slice(r),
        Op::CursorReport => replies.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes()),
        Op::BackTab(n) => {
            let to = (0..*n).fold(col, |c, _| c.saturating_sub(1) / 8 * 8);
            return format!("\x1b[{}G", to + 1).into_bytes();
        }
        Op::NoWrap if at >= cols => return format!("\x1b[{cols}G").into_bytes(),
        Op::NoWrap => {}
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use crate::termx::ScrollTracker;

    /// A 4 x 10 terminal after `chunks` (each one read), and the replies sent.
    fn run(chunks: &[&str]) -> (vt100::Parser, String) {
        let (mut p, mut t) = (vt100::Parser::new(4, 10, 100), ScrollTracker::default());
        for c in chunks {
            t.process(&mut p, c.as_bytes());
        }
        let replies = String::from_utf8(t.take_replies()).unwrap();
        (p, replies)
    }

    /// Row `r` of the screen, trailing blanks trimmed.
    fn row(p: &vt100::Parser, r: usize) -> String {
        p.screen().rows(0, 10).nth(r).unwrap().trim_end().to_string()
    }

    /// Each sequence vt100 ignores lands where a real terminal puts it.
    #[test]
    fn rewrites_what_vt100_ignores() {
        let cases: &[(&str, &[&str], usize, &str)] = &[
            ("line drawing (G0)", &["\x1b(0lqk\x1b(Bq"], 0, "┌─┐q"),
            ("line drawing (G1 via SO/SI)", &["\x1b)0\x0eqx\x0fq"], 0, "─│q"),
            ("REP repeats the last character", &["ab\x1b[3b"], 0, "abbbb"),
            ("HVP is CUP", &["\x1b[1;3fX"], 0, "  X"),
            ("CSI s / CSI u save and restore", &["ab\x1b[s\x1b[3;3Hz\x1b[uc"], 0, "abc"),
            ("IND moves down, keeps the column", &["a\x1bDb"], 1, " b"),
            ("NEL starts the next line", &["a\x1bEb"], 1, "b"),
            ("HPA, HPR", &["\x1b[3`x\x1b[2ay"], 0, "  x  y"),
            ("VPR", &["a\x1b[2eb"], 2, " b"),
            ("CHT", &["\x1b[1Ix"], 0, "        x"),
            ("CBT", &["\x1b[10G\x1b[Zx"], 0, "        x"),
            ("insert mode shifts the line right", &["abc\r\x1b[4hX\x1b[4l"], 0, "Xabc"),
            ("autowrap off overwrites the last column", &["\x1b[?7l0123456789AB"], 0, "012345678B"),
            ("autowrap off writes no second row", &["\x1b[?7l0123456789AB"], 1, ""),
            ("a sequence split across reads", &["\x1b(", "0q\x1b[", "2b"], 0, "───"),
            ("REP after a wide-byte character and a CR", &["é\r\x1b[2C\x1b[1b"], 0, "é é"),
        ];
        for (what, chunks, r, want) in cases {
            let (p, _) = run(chunks);
            assert_eq!(row(&p, *r), *want, "{what}");
        }
        // A UTF-8 character split between reads arrives whole.
        let (mut p, mut t) = (vt100::Parser::new(4, 10, 100), ScrollTracker::default());
        for c in [&b"a\xc3"[..], b"\xa9\x1b[1b"] {
            t.process(&mut p, c);
        }
        assert_eq!(row(&p, 0), "aéé");
    }

    /// 1047 is the alternate screen; 1048 saves and restores the cursor.
    #[test]
    fn alternate_screen_and_cursor_save_modes() {
        let (p, _) = run(&["main\x1b[?1047halt"]);
        assert!(p.screen().alternate_screen());
        let (p, _) = run(&["main\x1b[?1047halt\x1b[?1047l"]);
        assert_eq!(row(&p, 0), "main");
        let (p, _) = run(&["ab\x1b[?1048h\x1b[3;3H\x1b[?1048lc"]);
        assert_eq!(row(&p, 0), "abc");
    }

    /// A full row leaves vt100's cursor past the edge; it shows on the last column.
    #[test]
    fn cursor_cell_stays_on_the_grid() {
        let (p, _) = run(&["0123456789"]);
        assert_eq!(p.screen().cursor_position(), (0, 10), "vt100's pending-wrap position");
        assert_eq!(super::cursor_cell(p.screen()), (0, 9));
    }

    /// Queries are answered with the state at the point they were asked.
    #[test]
    fn answers_queries_in_place() {
        let (_, r) = run(&["\x1b[3;4H\x1b[6n\x1b[1;1H"]);
        assert_eq!(r, "\x1b[3;4R", "position where asked, not after the chunk");
        let (_, r) = run(&["0123456789\x1b[6n"]);
        assert_eq!(r, "\x1b[1;10R", "a pending wrap reports the last column");
        let (_, r) = run(&["\x1b[5n\x1b[c"]);
        assert!(r.starts_with("\x1b[0n\x1b[?"), "status, then device attributes: {r:?}");
    }
}
