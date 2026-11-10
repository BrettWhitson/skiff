//! Screen-reader announcements of new terminal output: batched, capped,
//! and without the echo of what the user just typed.

use std::time::{Duration, Instant};

/// At most one announcement per this long.
pub const EVERY: Duration = Duration::from_millis(500);
const MAX_LINES: usize = 20;
const MAX_CHARS: usize = 2000;

/// Collects output between announcements.
#[derive(Debug)]
pub struct Announcer {
    last: Instant,
    pending: String,
    /// What the user typed since the last announcement (its echo is skipped).
    recent_keys: String,
}

impl Announcer {
    /// `now`: the first announcement can come `EVERY` after this.
    pub fn new(now: Instant) -> Announcer {
        Announcer { last: now, pending: String::new(), recent_keys: String::new() }
    }

    pub fn push_output(&mut self, text: &str) {
        self.pending.push_str(text);
        // Only the tail is ever said; don't let a flood grow without bound.
        if self.pending.len() > 4 * MAX_CHARS {
            let cut = self.pending.len() - 2 * MAX_CHARS;
            let cut = (cut..self.pending.len()).find(|&i| self.pending.is_char_boundary(i)).unwrap_or(0);
            self.pending.drain(..cut);
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn typed(&mut self, s: &str) {
        self.recent_keys.push_str(s);
    }

    /// The text to announce now, if any: at most once per `EVERY`, the last
    /// 20 lines / 2,000 characters, with the typed echo removed.
    pub fn take(&mut self, now: Instant) -> Option<String> {
        if self.pending.is_empty() || now.duration_since(self.last) < EVERY {
            return None;
        }
        self.last = now;
        let mut text = std::mem::take(&mut self.pending).replace("\r\n", "\n").replace('\r', "\n");
        let keys = std::mem::take(&mut self.recent_keys);
        let keys = keys.trim();
        if !keys.is_empty()
            && let Some(rest) = text.trim_start().strip_prefix(keys)
        {
            text = rest.to_string();
        }
        let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
        let tail = lines[lines.len().saturating_sub(MAX_LINES)..].join("\n");
        let n = tail.chars().count();
        let out: String = tail.chars().skip(n.saturating_sub(MAX_CHARS)).collect();
        (!out.is_empty()).then_some(out)
    }
}

/// What appeared on the screen between two snapshots: the lines from the
/// old cursor line on, allowing for the screen having scrolled. The old
/// cursor line's text (a prompt) is cut from the first of them.
pub fn new_output(old: &str, new: &str) -> String {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let Some((last, above)) = old.split_last() else { return new.join("\n") };
    let shift = (0..=above.len()).find(|&k| {
        let kept = &above[k..];
        new.len() >= kept.len() && new[..kept.len()] == *kept
    });
    let Some(k) = shift else { return new.join("\n") };
    let start = above.len() - k;
    let mut out: Vec<String> = new.get(start..).unwrap_or(&[]).iter().map(|s| s.to_string()).collect();
    if let Some(first) = out.first_mut()
        && let Some(rest) = first.strip_prefix(last)
    {
        *first = rest.to_string();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_region_batches() {
        let t0 = Instant::now();
        let mut a = Announcer::new(t0);
        for i in 0..5000 {
            a.push_output(&format!("line {i}\r\n"));
        }
        let first = a.take(t0 + Duration::from_millis(600)).expect("one announcement");
        assert!(first.chars().count() <= 2000 && first.lines().count() <= 20, "capped");
        assert!(first.contains("line 4999"), "the newest output wins");
        a.push_output("more\r\n");
        assert_eq!(a.take(t0 + Duration::from_millis(700)), None, "not again within 500 ms");
        assert_eq!(a.take(t0 + Duration::from_millis(1200)).as_deref(), Some("more"));
        a.typed("ls");
        a.push_output("ls\r\nfile1\r\n");
        assert_eq!(a.take(t0 + Duration::from_millis(1800)).as_deref(), Some("file1"), "typed echo dropped");
        assert_eq!(a.take(t0 + Duration::from_millis(2400)), None, "nothing new, nothing said");
    }

    #[test]
    fn new_output_is_the_changed_tail() {
        assert_eq!(new_output("a\nb\n$ l", "a\nb\n$ ls\nfile1\n$ "), "s\nfile1\n$ ");
        assert_eq!(new_output("1\n2\n3", "2\n3\n4"), "\n4", "the screen scrolled by one line");
        assert_eq!(new_output("", "hello"), "hello");
    }
}
