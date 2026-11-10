//! A lossless view of one ssh config file: every byte of an untouched file
//! renders back unchanged, and edits touch only the lines they change.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use super::{split_kv, unquote};

/// Hash of a file's text, used to notice outside changes before saving.
pub fn text_hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

#[derive(Debug, Clone)]
pub struct Doc {
    pub path: PathBuf,
    /// Line content (no terminator) and its terminator (`"\n"`, `"\r\n"` or `""`).
    pub(crate) lines: Vec<(String, &'static str)>,
    pub(crate) eol: &'static str,
    pub(crate) bom: bool,
    pub(crate) final_newline: bool,
    pub(crate) loaded_hash: u64,
    pub(crate) dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Preamble,
    Host,
    Match,
}

/// A run of lines: the lines before the first `Host`/`Match`, or one block.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    /// First line (the `Host`/`Match` line).
    pub start: usize,
    /// One past the last option line; blank lines and comments after it
    /// belong to the gap before the next block.
    pub end: usize,
    pub patterns: Vec<String>,
    /// 1-based line number of the `Host`/`Match` line.
    pub line: usize,
}

/// The editable view of one `Host` block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostFields {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: String,
    pub identity_file: String,
    pub proxy_jump: String,
    pub proxy_command: String,
    pub remote_dir: String,
    pub tags: Vec<String>,
    pub hidden: bool,
    /// Every other option line, as written (`"ForwardAgent yes"`).
    pub other: Vec<String>,
}

/// Lower-case option keys that have their own form field, in field order.
pub(crate) const FIELD_KEYS: [&str; 6] = ["hostname", "user", "port", "identityfile", "proxyjump", "proxycommand"];

pub(crate) fn is_comment_or_blank(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.starts_with('#')
}

impl HostFields {
    /// The value of field `i` of [`FIELD_KEYS`].
    pub(crate) fn field(&self, i: usize) -> &str {
        match i {
            0 => &self.hostname,
            1 => &self.user,
            2 => &self.port,
            3 => &self.identity_file,
            4 => &self.proxy_jump,
            _ => &self.proxy_command,
        }
    }

    fn field_mut(&mut self, i: usize) -> &mut String {
        match i {
            0 => &mut self.hostname,
            1 => &mut self.user,
            2 => &mut self.port,
            3 => &mut self.identity_file,
            4 => &mut self.proxy_jump,
            _ => &mut self.proxy_command,
        }
    }
}

impl Doc {
    pub fn parse(path: PathBuf, text: &str) -> Doc {
        let loaded_hash = text_hash(text);
        let (bom, body) = match text.strip_prefix('\u{feff}') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let mut lines = Vec::new();
        let mut rest = body;
        while !rest.is_empty() {
            match rest.find('\n') {
                Some(i) => {
                    let (content, term) = match rest[..i].strip_suffix('\r') {
                        Some(c) => (c, "\r\n"),
                        None => (&rest[..i], "\n"),
                    };
                    lines.push((content.to_string(), term));
                    rest = &rest[i + 1..];
                }
                None => {
                    lines.push((rest.to_string(), ""));
                    rest = "";
                }
            }
        }
        let eol = if body.contains("\r\n") { "\r\n" } else { "\n" };
        let final_newline = body.is_empty() || body.ends_with('\n');
        Doc { path, lines, eol, bom, final_newline, loaded_hash, dirty: false }
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        if self.bom {
            s.push('\u{feff}');
        }
        for (c, t) in &self.lines {
            s.push_str(c);
            s.push_str(t);
        }
        s
    }

    pub fn loaded_hash(&self) -> u64 {
        self.loaded_hash
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn blocks(&self) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        let mut cur = Block { kind: BlockKind::Preamble, start: 0, end: 0, patterns: vec![], line: 0 };
        for (i, (c, _)) in self.lines.iter().enumerate() {
            if is_comment_or_blank(c) {
                // termax's own comment is part of the host it describes, even
                // in a block with no option lines.
                if cur.kind == BlockKind::Host && parse_extras(c.trim()).is_some() {
                    cur.end = i + 1;
                }
                continue;
            }
            let kind = match split_kv(c.trim()).map(|(k, _)| k.to_ascii_lowercase()).as_deref() {
                Some("host") => Some(BlockKind::Host),
                Some("match") => Some(BlockKind::Match),
                _ => None,
            };
            match kind {
                Some(kind) => {
                    if cur.kind != BlockKind::Preamble || cur.end > 0 {
                        out.push(cur);
                    }
                    let patterns = match (kind, split_kv(c.trim())) {
                        (BlockKind::Host, Some((_, v))) => v.split_whitespace().map(|p| unquote(p).to_string()).collect(),
                        _ => vec![],
                    };
                    cur = Block { kind, start: i, end: i + 1, patterns, line: i + 1 };
                }
                None => cur.end = i + 1,
            }
        }
        if cur.kind != BlockKind::Preamble || cur.end > 0 {
            out.push(cur);
        }
        out
    }

    pub fn host_fields(&self, b: &Block) -> HostFields {
        let mut f = HostFields { alias: b.patterns.join(" "), ..Default::default() };
        let mut seen = [false; FIELD_KEYS.len()];
        for (c, _) in &self.lines[b.start + 1..b.end] {
            let t = c.trim();
            if t.starts_with('#') {
                if let Some((tags, hidden, dir)) = parse_extras(t) {
                    f.tags = tags;
                    f.hidden = hidden;
                    f.remote_dir = dir.unwrap_or_default();
                }
                continue;
            }
            let Some((k, v)) = split_kv(t) else { continue };
            let k = k.to_ascii_lowercase();
            match FIELD_KEYS.iter().position(|x| *x == k) {
                Some(i) if !seen[i] => {
                    seen[i] = true;
                    *f.field_mut(i) = unquote(v).to_string();
                }
                _ => f.other.push(t.to_string()),
            }
        }
        f
    }
}

/// Reads a `# termax: tags=a,b; hidden; dir=/path` comment.
pub fn parse_extras(comment: &str) -> Option<(Vec<String>, bool, Option<String>)> {
    let body = comment.trim().strip_prefix('#')?.trim_start().strip_prefix("termax:")?;
    let (mut tags, mut hidden, mut dir) = (Vec::new(), false, None);
    for part in body.split(';').map(str::trim) {
        if let Some(t) = part.strip_prefix("tags=") {
            tags = t.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect();
        } else if part == "hidden" {
            hidden = true;
        } else if let Some(d) = part.strip_prefix("dir=") {
            dir = Some(d.trim().to_string()).filter(|d| !d.is_empty());
        }
    }
    Some((tags, hidden, dir))
}

/// The comment for [`parse_extras`], or `None` when there is nothing to keep.
pub fn format_extras(tags: &[String], hidden: bool, dir: &str) -> Option<String> {
    let mut parts = Vec::new();
    let tags: Vec<&str> = tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect();
    if !tags.is_empty() {
        parts.push(format!("tags={}", tags.join(",")));
    }
    if hidden {
        parts.push("hidden".into());
    }
    if !dir.trim().is_empty() {
        parts.push(format!("dir={}", dir.trim()));
    }
    (!parts.is_empty()).then(|| format!("# termax: {}", parts.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\u{feff}# global\r\nInclude \"~/.ssh/conf.d/*\"\r\n\r\nHost web\r\n\tHostName=10.0.0.5\r\n\tIdentityFile \"C:/my keys/id\"\r\n  # note\r\n\r\nHost *\r\n    User x\r\nMatch host foo\r\n    Port 1";

    #[test]
    fn doc_round_trip_is_exact() {
        for t in [FIXTURE, "", "Host a\n", "Host a\n  User b", "\n\n", "Host a\r\n  User b\n"] {
            assert_eq!(Doc::parse("x".into(), t).render(), t);
        }
    }

    #[test]
    fn blocks_and_fields() {
        let d = Doc::parse(
            "x".into(),
            "Host web\n  # termax: tags=prod,web; hidden; dir=/srv\n  HostName 10.0.0.5\n  Port 2222\n  ForwardAgent yes\nHost *\n  User x\n",
        );
        let b = d.blocks();
        assert_eq!(b.iter().map(|b| b.kind).collect::<Vec<_>>(), [BlockKind::Host, BlockKind::Host]);
        assert_eq!(b[0].patterns, ["web"]);
        assert_eq!(b[1].patterns, ["*"]);
        assert_eq!((b[0].line, b[1].line), (1, 6));
        let f = d.host_fields(&b[0]);
        assert_eq!(f.hostname, "10.0.0.5");
        assert_eq!(f.port, "2222");
        assert_eq!(f.tags, ["prod", "web"]);
        assert!(f.hidden);
        assert_eq!(f.remote_dir, "/srv");
        assert_eq!(f.other, ["ForwardAgent yes"]);

        let d = Doc::parse("x".into(), FIXTURE);
        let b = d.blocks();
        assert_eq!(b.iter().map(|b| b.kind).collect::<Vec<_>>(), [BlockKind::Preamble, BlockKind::Host, BlockKind::Host, BlockKind::Match]);
        let f = d.host_fields(&b[1]);
        assert_eq!(f.alias, "web");
        assert_eq!(f.identity_file, "C:/my keys/id");
    }

    #[test]
    fn extras_round_trip() {
        let s = format_extras(&["a".into()], true, "").unwrap();
        assert_eq!(s, "# termax: tags=a; hidden");
        assert_eq!(parse_extras(&s), Some((vec!["a".to_string()], true, None)));
        assert_eq!(format_extras(&[], false, ""), None);
        assert_eq!(parse_extras("# just a comment"), None);
    }
}
