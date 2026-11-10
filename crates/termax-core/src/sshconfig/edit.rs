//! Editing hosts in ssh config files in place, and saving them safely:
//! a conflict check, a rotating backup and an atomic replace.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use super::doc::{BlockKind, Doc, FIELD_KEYS, HostFields, format_extras, is_comment_or_blank, parse_extras, text_hash};
use super::{MAX_INCLUDE_DEPTH, resolve_include, split_kv, unquote};

/// How many backups are kept per file.
pub const KEEP_BACKUPS: usize = 10;

/// Key spellings used for new lines, in [`FIELD_KEYS`] order.
const FIELD_NAMES: [&str; 6] = ["HostName", "User", "Port", "IdentityFile", "ProxyJump", "ProxyCommand"];

/// The main ssh config file and every file it includes.
#[derive(Debug, Clone, Default)]
pub struct ConfigSet {
    /// The main file first, then included files in include order.
    pub docs: Vec<Doc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Locate {
    Editable {
        doc: usize,
        block: usize,
    },
    /// Why the host can't be edited here.
    ReadOnly(String),
    NotFound,
}

/// Where termax keeps ssh config backups, next to its own config file.
pub fn backups_dir(config_path: &Path) -> PathBuf {
    config_path.with_file_name("backups")
}

/// `path` with the home directory shown as `~`.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = super::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display().to_string().replace('\\', "/"));
    }
    path.display().to_string()
}

fn file_label(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn is_wildcard(p: &str) -> bool {
    p.contains(['*', '?', '!'])
}

/// Checks a host form. `taken` lists every host name in use (ssh config and
/// custom); `editing` is the host's current name when editing.
pub fn validate(f: &HostFields, taken: &[String], editing: Option<&str>) -> Result<(), String> {
    validate_with(f, taken, editing, &[])
}

/// [`validate`], allowing `existing` "other" lines (already in the block)
/// to stay as they are.
pub fn validate_with(f: &HostFields, taken: &[String], editing: Option<&str>, existing: &[String]) -> Result<(), String> {
    let alias = f.alias.trim();
    if alias.is_empty() {
        return Err("Alias is required".into());
    }
    // `=` and `"` change how ssh splits the line, `#` starts a comment and
    // `,` separates hosts in `-J` and `ProxyJump` lists.
    if alias.contains(|c: char| c.is_whitespace() || "*?!=\"#,".contains(c)) {
        return Err("Alias can't contain spaces or * ? ! = \" # ,".into());
    }
    if editing != Some(alias) && taken.iter().any(|t| t == alias) {
        return Err(format!("A host named {alias} already exists"));
    }
    let port = f.port.trim();
    if !port.is_empty() && port.parse::<u16>().map_or(true, |p| p == 0) {
        return Err(format!("Invalid port: {port}"));
    }
    let singles = [&f.hostname, &f.user, &f.identity_file, &f.proxy_jump, &f.proxy_command, &f.remote_dir];
    if singles.iter().any(|v| v.contains(['\n', '\r'])) || f.tags.iter().any(|t| t.contains(['\n', '\r', ',', ';'])) {
        return Err("Values can't contain line breaks (or , ; in tags)".into());
    }
    if f.remote_dir.contains(';') {
        return Err("Remote dir can't contain ;".into());
    }
    for line in &f.other {
        if line.contains(['\n', '\r']) {
            return Err("Values can't contain line breaks (or , ; in tags)".into());
        }
        let key = split_kv(line.trim()).map(|(k, _)| k.to_ascii_lowercase()).unwrap_or_default();
        let kept = existing.iter().any(|e| e.trim() == line.trim());
        if !kept && matches!(key.as_str(), "host" | "match" | "include") {
            return Err("Other options can't contain Host, Match or Include".into());
        }
    }
    Ok(())
}

/// `value` as an ssh config value: quoted when it holds whitespace.
/// ProxyCommand takes the rest of the line verbatim, so it is never quoted.
fn quote(key: usize, value: &str) -> String {
    let v = value.trim();
    if key != 5 && v.contains(char::is_whitespace) && !(v.starts_with('"') && v.ends_with('"')) {
        format!("\"{v}\"")
    } else {
        v.to_string()
    }
}

/// `line` (`  Key = value`) with its value replaced, keeping indent and separator.
fn with_value(line: &str, value: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let body = &line[indent..];
    let key_end = body.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(body.len());
    let sep_end = key_end + body[key_end..].find(|c: char| !(c.is_whitespace() || c == '=')).unwrap_or(body.len() - key_end);
    let sep = if sep_end > key_end { &body[key_end..sep_end] } else { " " };
    format!("{}{}{}{}", &line[..indent], &body[..key_end], sep, value)
}

fn leading_ws(s: &str) -> &str {
    &s[..s.len() - s.trim_start().len()]
}

/// The separator between key and value in option line `line` (`" "`,
/// `"="`, `" = "`…), or `None` for comments and blank lines.
fn separator(line: &str) -> Option<String> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let key_end = t.find(|c: char| c.is_whitespace() || c == '=')?;
    let rest = &t[key_end..];
    let sep_len = rest.find(|c: char| !(c.is_whitespace() || c == '=')).unwrap_or(rest.len());
    Some(rest[..sep_len].to_string()).filter(|s| !s.is_empty())
}

/// The separator the first option line among `lines` uses.
fn first_separator<'a>(lines: impl IntoIterator<Item = &'a (String, &'static str)>) -> Option<String> {
    lines.into_iter().find_map(|(c, _)| separator(c))
}

/// What a line inside a host block is.
enum LineKind {
    Field(usize),
    Other,
    Extras,
    Keep,
}

impl Doc {
    /// The indent used for options in this file (default four spaces).
    fn indent(&self) -> String {
        for b in self.blocks() {
            for (c, _) in &self.lines[b.start + 1..b.end] {
                if !is_comment_or_blank(c) && !leading_ws(c).is_empty() {
                    return leading_ws(c).to_string();
                }
            }
        }
        "    ".into()
    }

    /// The key/value separator used for options in this file (default a space).
    fn separator(&self) -> String {
        self.blocks()
            .iter()
            .filter(|b| b.kind != BlockKind::Preamble)
            .find_map(|b| first_separator(&self.lines[b.start + 1..b.end]))
            .unwrap_or_else(|| " ".into())
    }

    /// Fixes line terminators after an edit: every line but the last ends
    /// with one; the last keeps the file's final-newline style.
    fn normalize(&mut self) {
        let n = self.lines.len();
        for (i, (_, t)) in self.lines.iter_mut().enumerate() {
            if i + 1 < n {
                if t.is_empty() {
                    *t = self.eol;
                }
            } else if !self.final_newline {
                *t = "";
            } else if t.is_empty() {
                *t = self.eol;
            }
        }
        self.dirty = true;
    }

    /// Inserts `block` (raw lines) before the first wildcard `Host` or `Match`
    /// block (and the comments right above it), else at the end, with one
    /// blank line between blocks.
    fn insert_block(&mut self, block: Vec<String>) {
        let at = self
            .blocks()
            .into_iter()
            .find(|b| b.kind == BlockKind::Match || (b.kind == BlockKind::Host && b.patterns.iter().any(|p| is_wildcard(p))));
        let mut new: Vec<(String, &'static str)> = block.into_iter().map(|l| (l, self.eol)).collect();
        match at {
            Some(b) => {
                let mut i = b.start;
                while i > 0 && self.lines[i - 1].0.trim().starts_with('#') {
                    i -= 1;
                }
                new.push((String::new(), self.eol));
                if i > 0 && !self.lines[i - 1].0.trim().is_empty() {
                    new.insert(0, (String::new(), self.eol));
                }
                self.lines.splice(i..i, new);
            }
            None => {
                if self.lines.last().is_some_and(|(c, _)| !c.trim().is_empty()) {
                    self.lines.push((String::new(), self.eol));
                }
                self.lines.extend(new);
            }
        }
        self.normalize();
    }

    /// Inserts `block` (raw lines) at line `end`, right after a block, with
    /// one blank line on each side.
    fn insert_block_at(&mut self, end: usize, block: Vec<String>) {
        let mut new: Vec<(String, &'static str)> = vec![(String::new(), self.eol)];
        new.extend(block.into_iter().map(|l| (l, self.eol)));
        if self.lines.get(end).is_some_and(|(c, _)| !c.trim().is_empty()) {
            new.push((String::new(), self.eol));
        }
        self.lines.splice(end..end, new);
        self.normalize();
    }
}

fn block_lines(f: &HostFields, indent: &str, sep: &str) -> Vec<String> {
    let mut out = vec![format!("Host {}", f.alias.trim())];
    if let Some(x) = format_extras(&f.tags, f.hidden, &f.remote_dir) {
        out.push(format!("{indent}{x}"));
    }
    for (i, name) in FIELD_NAMES.iter().enumerate() {
        if !f.field(i).trim().is_empty() {
            out.push(format!("{indent}{name}{sep}{}", quote(i, f.field(i))));
        }
    }
    out.extend(f.other.iter().map(|o| o.trim()).filter(|o| !o.is_empty()).map(|o| format!("{indent}{o}")));
    out
}

impl ConfigSet {
    /// Loads `main` and the files it includes. A missing main file is empty.
    pub fn load(main: &Path) -> anyhow::Result<ConfigSet> {
        let mut set = ConfigSet { docs: Vec::new() };
        set.load_into(main, 0)?;
        if set.docs.is_empty() {
            set.docs.push(Doc::parse(main.to_path_buf(), ""));
        }
        Ok(set)
    }

    fn load_into(&mut self, path: &Path, depth: usize) -> anyhow::Result<()> {
        if depth > MAX_INCLUDE_DEPTH || self.docs.iter().any(|d| d.path == path) {
            return Ok(());
        }
        let text = if path.is_file() {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        } else if depth == 0 {
            String::new()
        } else {
            return Ok(());
        };
        let doc = Doc::parse(path.to_path_buf(), &text);
        let includes: Vec<PathBuf> = doc
            .lines
            .iter()
            .filter_map(|(c, _)| split_kv(c.trim()).filter(|(k, _)| k.eq_ignore_ascii_case("include")).map(|(_, v)| v.to_string()))
            .flat_map(|v| v.split_whitespace().flat_map(|p| resolve_include(unquote(p))).collect::<Vec<_>>())
            .collect();
        self.docs.push(doc);
        for inc in includes {
            self.load_into(&inc, depth + 1)?;
        }
        Ok(())
    }

    /// Every host block naming `alias`: (doc, block index, block).
    fn occurrences(&self, alias: &str) -> Vec<(usize, usize, super::doc::Block)> {
        let mut out = Vec::new();
        for (di, d) in self.docs.iter().enumerate() {
            for (bi, b) in d.blocks().into_iter().enumerate() {
                if b.kind == BlockKind::Host && b.patterns.iter().any(|p| p == alias) {
                    out.push((di, bi, b));
                }
            }
        }
        out
    }

    pub fn locate(&self, alias: &str) -> Locate {
        let occ = self.occurrences(alias);
        match occ.as_slice() {
            [] => Locate::NotFound,
            [(d, bi, b)] if b.patterns.len() == 1 => Locate::Editable { doc: *d, block: *bi },
            [(_, _, b)] => {
                let others: Vec<&str> = b.patterns.iter().filter(|p| *p != alias).map(String::as_str).collect();
                Locate::ReadOnly(format!(
                    "Defined together with {} in a shared block — edit the file directly or split it.",
                    others.join(", ")
                ))
            }
            many => {
                let places: Vec<String> = many.iter().map(|(d, _, b)| format!("{}:{}", file_label(&self.docs[*d].path), b.line)).collect();
                Locate::ReadOnly(format!("Defined in more than one place ({}) — edit the file directly.", places.join(", ")))
            }
        }
    }

    /// The form fields of an editable host.
    pub fn fields(&self, alias: &str) -> Option<HostFields> {
        match self.locate(alias) {
            Locate::Editable { doc, block } => {
                let d = &self.docs[doc];
                Some(d.host_fields(&d.blocks()[block]))
            }
            _ => None,
        }
    }

    /// Every concrete (non-wildcard) host name, in file order.
    pub fn aliases(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for d in &self.docs {
            for b in d.blocks() {
                if b.kind == BlockKind::Host {
                    for p in b.patterns {
                        if !is_wildcard(&p) && !out.contains(&p) {
                            out.push(p);
                        }
                    }
                }
            }
        }
        out
    }

    fn editable(&self, alias: &str) -> Result<(usize, super::doc::Block), String> {
        match self.locate(alias) {
            Locate::Editable { doc, block } => Ok((doc, self.docs[doc].blocks().swap_remove(block))),
            Locate::ReadOnly(m) => Err(m),
            Locate::NotFound => Err(format!("No host named {alias} in the ssh config")),
        }
    }

    /// Rewrites host `alias` to match `f`, touching only lines that change.
    pub fn update_host(&mut self, alias: &str, f: &HostFields) -> Result<(), String> {
        let existing = self.fields(alias).map(|h| h.other).unwrap_or_default();
        validate_with(f, &self.aliases(), Some(alias), &existing)?;
        let (di, b) = self.editable(alias)?;
        let doc = &mut self.docs[di];
        let indent = doc.lines[b.start + 1..b.end]
            .iter()
            .find(|(c, _)| !is_comment_or_blank(c))
            .map(|(c, _)| leading_ws(c).to_string())
            .unwrap_or_else(|| doc.indent());
        let sep = first_separator(&doc.lines[b.start + 1..b.end]).unwrap_or_else(|| doc.separator());
        let eol = doc.eol;
        let old: Vec<(String, &'static str)> = doc.lines[b.start..b.end].to_vec();

        let mut seen = [false; FIELD_KEYS.len()];
        let kinds: Vec<LineKind> = old
            .iter()
            .skip(1)
            .map(|(c, _)| {
                let t = c.trim();
                if t.starts_with('#') {
                    return if parse_extras(t).is_some() { LineKind::Extras } else { LineKind::Keep };
                }
                let Some((k, _)) = split_kv(t) else { return LineKind::Keep };
                match FIELD_KEYS.iter().position(|x| x.eq_ignore_ascii_case(k)) {
                    Some(i) if !seen[i] => {
                        seen[i] = true;
                        LineKind::Field(i)
                    }
                    _ => LineKind::Other,
                }
            })
            .collect();

        let current = doc.host_fields(&b);
        let extras = format_extras(&f.tags, f.hidden, &f.remote_dir);
        let mut wanted_other: Vec<&str> = f.other.iter().map(|o| o.trim()).filter(|o| !o.is_empty()).collect();
        let mut out: Vec<(String, &'static str)> = Vec::new();

        let (host_line, host_term) = &old[0];
        let new_alias = f.alias.trim();
        out.push((if new_alias == alias { host_line.clone() } else { with_value(host_line, new_alias) }, host_term));
        let mut extras_done = false;
        for ((c, t), kind) in old.iter().skip(1).zip(&kinds) {
            match kind {
                LineKind::Keep => out.push((c.clone(), t)),
                LineKind::Extras => {
                    if let (Some(x), false) = (&extras, extras_done) {
                        out.push((format!("{}{x}", leading_ws(c)), t));
                    }
                    extras_done = true;
                }
                LineKind::Field(i) => {
                    let v = f.field(*i).trim();
                    if v.is_empty() {
                        continue;
                    }
                    if v == current.field(*i) {
                        out.push((c.clone(), t));
                    } else {
                        out.push((with_value(c, &quote(*i, v)), t));
                    }
                }
                LineKind::Other => {
                    if let Some(pos) = wanted_other.iter().position(|o| *o == c.trim()) {
                        wanted_other.remove(pos);
                        out.push((c.clone(), t));
                    }
                }
            }
        }
        if let (Some(x), false) = (&extras, extras_done) {
            out.insert(1, (format!("{indent}{x}"), eol));
        }
        for (i, name) in FIELD_NAMES.iter().enumerate() {
            let v = f.field(i).trim();
            if !seen[i] && !v.is_empty() {
                out.push((format!("{indent}{name}{sep}{}", quote(i, v)), eol));
            }
        }
        out.extend(wanted_other.into_iter().map(|o| (format!("{indent}{o}"), eol)));

        if out != old {
            doc.lines.splice(b.start..b.end, out);
            doc.normalize();
        }
        Ok(())
    }

    /// Adds a new host to file `doc`.
    pub fn add_host(&mut self, doc: usize, f: &HostFields) -> Result<(), String> {
        validate(f, &self.aliases(), None)?;
        let d = self.docs.get_mut(doc).ok_or("No such file")?;
        let lines = block_lines(f, &d.indent(), &d.separator());
        d.insert_block(lines);
        Ok(())
    }

    /// Adds a new host right after host `alias`, in its file: ssh takes the
    /// first value it finds, so the copy sees the same `Host *` defaults and
    /// overrides as the original.
    pub fn add_host_after(&mut self, alias: &str, f: &HostFields) -> Result<(), String> {
        validate(f, &self.aliases(), None)?;
        let (di, b) = self.editable(alias)?;
        let d = &mut self.docs[di];
        let body = &d.lines[b.start + 1..b.end];
        let indent = match body.iter().find(|(c, _)| !is_comment_or_blank(c)) {
            Some((c, _)) => leading_ws(c).to_string(),
            None => d.indent(),
        };
        let sep = first_separator(body).unwrap_or_else(|| d.separator());
        let lines = block_lines(f, &indent, &sep);
        d.insert_block_at(b.end, lines);
        Ok(())
    }

    /// Removes host `alias` and one blank line next to it.
    pub fn delete_host(&mut self, alias: &str) -> Result<(), String> {
        let (di, b) = self.editable(alias)?;
        self.remove_block(di, &b, false);
        Ok(())
    }

    /// Removes block `b`; with `comments`, also the comment lines right
    /// above its `Host` line (they describe it). Returns the removed lines.
    fn remove_block(&mut self, di: usize, b: &super::doc::Block, comments: bool) -> Vec<String> {
        let d = &mut self.docs[di];
        let mut start = b.start;
        if comments {
            // Never into the previous block (its termax comment is its own).
            let floor = d.blocks().iter().filter(|o| o.end <= b.start).map(|o| o.end).max().unwrap_or(0);
            while start > floor && d.lines[start - 1].0.trim().starts_with('#') {
                start -= 1;
            }
        }
        let removed: Vec<String> = d.lines.drain(start..b.end).map(|(c, _)| c).collect();
        let blank = |d: &Doc, i: usize| d.lines.get(i).is_some_and(|(c, _)| c.trim().is_empty());
        if blank(d, start) {
            d.lines.remove(start);
        } else if start > 0 && blank(d, start - 1) {
            d.lines.remove(start - 1);
        }
        d.normalize();
        removed
    }

    /// Moves host `alias`, with its comments, into file `to_doc`.
    pub fn move_host(&mut self, alias: &str, to_doc: usize) -> Result<(), String> {
        let (di, b) = self.editable(alias)?;
        if di == to_doc {
            return Err("The host is already in that file".into());
        }
        if to_doc >= self.docs.len() {
            return Err("No such file".into());
        }
        let lines = self.remove_block(di, &b, true);
        self.docs[to_doc].insert_block(lines);
        Ok(())
    }

    /// Writes every changed file: refuses if any changed on disk since it
    /// was loaded, backs each up (keeping [`KEEP_BACKUPS`]), then replaces
    /// it atomically.
    pub fn save(&mut self, backups: &Path) -> anyhow::Result<()> {
        for d in self.docs.iter().filter(|d| d.dirty) {
            let now = std::fs::read_to_string(&d.path).unwrap_or_default();
            if text_hash(&now) != d.loaded_hash {
                bail!("{} changed on disk — reopen to see the new version.", display_path(&d.path));
            }
        }
        // A move adds to one file and removes from another: write the one
        // that gains first, so a failed write can't lose the host from both.
        let hosts_in = |text: &str| Doc::parse(PathBuf::new(), text).blocks().iter().filter(|b| b.kind == BlockKind::Host).count();
        let mut order: Vec<usize> = (0..self.docs.len()).filter(|&i| self.docs[i].dirty).collect();
        order.sort_by_key(|&i| {
            let d = &self.docs[i];
            let on_disk = std::fs::read_to_string(&d.path).unwrap_or_default();
            hosts_in(&d.render()) < hosts_in(&on_disk)
        });
        for i in order {
            let d = &mut self.docs[i];
            let text = d.render();
            if d.path.is_file() {
                backup(&d.path, backups)?;
            }
            write_atomic(&d.path, &text)?;
            d.loaded_hash = text_hash(&text);
            d.dirty = false;
        }
        Ok(())
    }
}

fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// `YYYYMMDD-HHMMSS` in UTC.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

/// A short hash of `path`'s full (absolute) spelling, stable across runs
/// and toolchains (FNV-1a folded to 32 bits), so files that share a name
/// get their own backups.
fn path_hash(path: &Path) -> u32 {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in full.to_string_lossy().bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    (h ^ (h >> 32)) as u32
}

/// Copies `path` to `backups/<name>-<path hash>-<stamp>.bak` and keeps the
/// newest [`KEEP_BACKUPS`] of that file. Saves within one second get `~01`,
/// `~02`… so names still sort oldest first.
fn backup(path: &Path, backups: &Path) -> anyhow::Result<()> {
    create_private_dir(backups)?;
    let name = format!("{}-{:08x}", file_label(path), path_hash(path));
    let stamp = timestamp();
    let mut target = backups.join(format!("{name}-{stamp}.bak"));
    let mut n = 1;
    while target.exists() {
        target = backups.join(format!("{name}-{stamp}~{n:02}.bak"));
        n += 1;
    }
    let bytes = std::fs::read(path)?;
    write_private(&target, &bytes, None)?;

    let prefix = format!("{name}-");
    let mut all: Vec<String> = std::fs::read_dir(backups)?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| {
            f.ends_with(".bak")
                && f.strip_prefix(&prefix).is_some_and(|r| r.len() >= 15 && r.as_bytes()[8] == b'-' && r.as_bytes()[0].is_ascii_digit())
        })
        .collect();
    all.sort();
    let excess = all.len().saturating_sub(KEEP_BACKUPS);
    for old in &all[..excess] {
        let _ = std::fs::remove_file(backups.join(old));
    }
    Ok(())
}

/// Writes a new file with mode `mode` (0600 when `None`) on Unix.
fn write_private(path: &Path, bytes: &[u8], mode: Option<u32>) -> anyhow::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode.unwrap_or(0o600));
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut f = opts.open(path).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(mode.unwrap_or(0o600)))?;
    }
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

/// Replaces `path` with `text` via a temp file in the same directory.
fn write_atomic(path: &Path, text: &str) -> anyhow::Result<()> {
    // A symlinked config (dotfile managers) is updated at its target, so the
    // link stays a link.
    let real = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?,
        _ => path.to_path_buf(),
    };
    let path = real.as_path();
    let dir = path.parent().context("config path has no directory")?;
    create_private_dir(dir)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o7777)
    };
    #[cfg(not(unix))]
    let mode = None;
    let tmp = dir.join(format!(".{}.termax-tmp", file_label(path)));
    write_private(&tmp, text.as_bytes(), mode)?;
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            // Windows: another process (an editor, antivirus) may hold the file briefly.
            Err(_) if cfg!(windows) && tries < 5 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e).with_context(|| format!("replacing {}", path.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(text: &str) -> (tempfile::TempDir, PathBuf, ConfigSet) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config");
        std::fs::write(&p, text).unwrap();
        let set = ConfigSet::load(&p).unwrap();
        (dir, p, set)
    }

    fn host(alias: &str, hostname: &str) -> HostFields {
        HostFields { alias: alias.into(), hostname: hostname.into(), ..Default::default() }
    }

    #[test]
    fn update_changes_only_touched_lines() {
        let (_d, _p, mut s) =
            setup("# top comment\nHost a\n    HostName 10.0.0.1\n    User root\n    Port 2222\n\n# db\nHost b\n    HostName b.example\n");
        let mut f = s.fields("a").unwrap();
        f.port = "2200".into();
        f.user.clear();
        f.tags = vec!["prod".into()];
        s.update_host("a", &f).unwrap();
        assert_eq!(
            s.docs[0].render(),
            "# top comment\nHost a\n    # termax: tags=prod\n    HostName 10.0.0.1\n    Port 2200\n\n# db\nHost b\n    HostName b.example\n"
        );
        assert!(s.docs[0].is_dirty());
    }

    #[test]
    fn update_quotes_values_with_spaces() {
        let (_d, _p, mut s) = setup("Host a\r\n\tUser=x");
        let mut f = s.fields("a").unwrap();
        f.identity_file = "C:/my keys/id".into();
        f.proxy_command = "ssh -W %h:%p jump".into();
        f.other = vec!["ForwardAgent yes".into()];
        s.update_host("a", &f).unwrap();
        let text = s.docs[0].render();
        assert_eq!(
            text,
            "Host a\r\n\tUser=x\r\n\tIdentityFile=\"C:/my keys/id\"\r\n\tProxyCommand=ssh -W %h:%p jump\r\n\tForwardAgent yes"
        );
        let again = Doc::parse("x".into(), &text);
        assert_eq!(again.host_fields(&again.blocks()[0]), f);
    }

    #[test]
    fn new_keys_follow_the_separator_style() {
        let (_d, _p, mut s) = setup("Host a\n  User = x\n");
        s.add_host(0, &host("b", "h")).unwrap();
        assert_eq!(s.docs[0].render(), "Host a\n  User = x\n\nHost b\n  HostName = h\n");
    }

    #[test]
    fn add_goes_before_first_wildcard() {
        let (_d, _p, mut s) = setup("Host a\n  User x\n\n# defaults\nHost *\n  User y\nMatch all\n  Port 1\n");
        s.add_host(0, &host("b", "1.2.3.4")).unwrap();
        assert_eq!(
            s.docs[0].render(),
            "Host a\n  User x\n\nHost b\n  HostName 1.2.3.4\n\n# defaults\nHost *\n  User y\nMatch all\n  Port 1\n"
        );
    }

    #[test]
    fn delete_removes_block_and_one_blank() {
        let text = "Host a\n  User x\n\nHost b\n  User y\n";
        let (_d, _p, mut s) = setup(text);
        s.delete_host("a").unwrap();
        assert_eq!(s.docs[0].render(), "Host b\n  User y\n");
        let (_d, _p, mut s) = setup(text);
        s.delete_host("b").unwrap();
        assert_eq!(s.docs[0].render(), "Host a\n  User x\n");
    }

    #[test]
    fn move_between_included_files() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work.conf");
        std::fs::write(&work, "Host w\n  User z\n").unwrap();
        let main = dir.path().join("config");
        let inc = format!("Include {}\n", work.display().to_string().replace('\\', "/"));
        std::fs::write(&main, format!("{inc}\nHost a\n  # mine\n  User x\n")).unwrap();
        let mut s = ConfigSet::load(&main).unwrap();
        assert_eq!(s.docs.len(), 2);
        s.move_host("a", 1).unwrap();
        assert_eq!(s.docs[0].render(), inc);
        assert_eq!(s.docs[1].render(), "Host w\n  User z\n\nHost a\n  # mine\n  User x\n");
    }

    #[test]
    fn move_takes_the_comments_above_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work.conf");
        std::fs::write(&work, "").unwrap();
        let main = dir.path().join("config");
        let inc = format!("Include {}\n", work.display().to_string().replace('\\', "/"));
        std::fs::write(&main, format!("{inc}\n# the db box\n# (ask ops)\nHost a\n  User x\n\nHost b\n")).unwrap();
        let mut s = ConfigSet::load(&main).unwrap();
        s.move_host("a", 1).unwrap();
        assert_eq!(s.docs[0].render(), format!("{inc}\nHost b\n"));
        assert_eq!(s.docs[1].render(), "# the db box\n# (ask ops)\nHost a\n  User x\n");
    }

    #[test]
    fn add_after_puts_the_copy_next_to_the_original() {
        let (_d, _p, mut s) = setup("Host a\n  User x\n# defaults\nHost *\n  User y\n");
        s.add_host_after("a", &host("a-copy", "h")).unwrap();
        assert_eq!(s.docs[0].render(), "Host a\n  User x\n\nHost a-copy\n  HostName h\n\n# defaults\nHost *\n  User y\n");
    }

    #[test]
    fn shared_and_duplicate_hosts_are_read_only() {
        let (_d, _p, s) = setup("Host a b\n  User x\nHost c\n  User y\nHost c\n  Port 2\nHost d\n");
        match s.locate("a") {
            Locate::ReadOnly(m) => assert!(m.contains("together with b"), "{m}"),
            other => panic!("{other:?}"),
        }
        match s.locate("c") {
            Locate::ReadOnly(m) => assert!(m.contains("more than one place (config:3, config:5)"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(s.locate("d"), Locate::Editable { doc: 0, .. }));
        assert!(matches!(s.locate("zz"), Locate::NotFound));
        assert_eq!(s.aliases(), ["a", "b", "c", "d"]);
    }

    #[test]
    fn save_backs_up_and_rotates() {
        let (d, p, mut s) = setup("Host a\n  Port 1\n");
        let backups = d.path().join("backups");
        for port in 2..14 {
            let mut f = s.fields("a").unwrap();
            f.port = port.to_string();
            s.update_host("a", &f).unwrap();
            s.save(&backups).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "Host a\n  Port 13\n");
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 10);
        assert!(!s.docs[0].is_dirty());
    }

    #[test]
    fn backups_of_same_named_files_rotate_separately() {
        let d = tempfile::tempdir().unwrap();
        let backups = d.path().join("backups");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        for dir in [&a, &b] {
            std::fs::create_dir(dir).unwrap();
            std::fs::write(dir.join("config"), "Host x\n").unwrap();
        }
        for _ in 0..KEEP_BACKUPS {
            backup(&a.join("config"), &backups).unwrap();
        }
        backup(&b.join("config"), &backups).unwrap();
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), KEEP_BACKUPS + 1, "b's backup doesn't push out a's");
    }

    #[test]
    fn save_refuses_after_external_change() {
        let (d, p, mut s) = setup("Host a\n  Port 1\n");
        std::fs::write(&p, "Host other\n").unwrap();
        let mut f = s.fields("a").unwrap();
        f.port = "2".into();
        s.update_host("a", &f).unwrap();
        let err = s.save(&d.path().join("backups")).unwrap_err().to_string();
        assert!(err.contains("changed on disk"), "{err}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "Host other\n");
    }

    #[test]
    fn add_creates_missing_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("home").join(".ssh").join("config");
        let mut s = ConfigSet::load(&p).unwrap();
        s.add_host(0, &host("a", "h")).unwrap();
        s.save(&d.path().join("backups")).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "Host a\n    HostName h\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (d, p, mut s) = setup("Host a\n");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        s.add_host(0, &host("b", "h")).unwrap();
        s.save(&d.path().join("backups")).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn validate_rejects_bad_input() {
        let taken = vec!["web".to_string()];
        let check = |f: HostFields| validate(&f, &taken, None).unwrap_err();
        assert_eq!(check(host("", "h")), "Alias is required");
        for bad in ["a b", "a*", "a=b", "a\"b", "a#b", "a,b", "a\tb"] {
            assert_eq!(check(host(bad, "h")), "Alias can't contain spaces or * ? ! = \" # ,", "{bad}");
        }
        assert_eq!(check(host("web", "h")), "A host named web already exists");
        assert_eq!(check(HostFields { port: "0".into(), ..host("x", "h") }), "Invalid port: 0");
        assert_eq!(
            check(HostFields { other: vec!["Host y".into()], ..host("x", "h") }),
            "Other options can't contain Host, Match or Include"
        );
        assert!(validate(&host("web", "h"), &taken, Some("web")).is_ok());
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    fn setup(text: &str) -> (tempfile::TempDir, PathBuf, ConfigSet) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config");
        std::fs::write(&p, text).unwrap();
        let set = ConfigSet::load(&p).unwrap();
        (dir, p, set)
    }

    fn set_hidden(p: &Path, alias: &str, hidden: bool) -> ConfigSet {
        let mut s = ConfigSet::load(p).unwrap();
        let mut f = s.fields(alias).unwrap();
        f.hidden = hidden;
        s.update_host(alias, &f).unwrap();
        s
    }

    #[test]
    fn bare_host_hide_unhide_delete() {
        let (d, p, _) = setup("Host a\n    HostName 1.1.1.1\n\nHost bare\n");
        let b = d.path().join("backups");
        set_hidden(&p, "bare", true).save(&b).unwrap();
        assert!(ConfigSet::load(&p).unwrap().fields("bare").unwrap().hidden, "the editor sees the comment");
        set_hidden(&p, "bare", false).save(&b).unwrap();
        assert!(!std::fs::read_to_string(&p).unwrap().contains("termax"));
        let mut s = set_hidden(&p, "bare", true);
        s.delete_host("bare").unwrap();
        s.save(&b).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, "Host a\n    HostName 1.1.1.1\n");
    }

    #[test]
    fn name_only_host_keeps_tags() {
        let (d, p, mut s) = setup("");
        let f = HostFields { alias: "n".into(), tags: vec!["prod".into()], ..Default::default() };
        s.add_host(0, &f).unwrap();
        s.save(&d.path().join("backups")).unwrap();
        assert_eq!(ConfigSet::load(&p).unwrap().fields("n").unwrap().tags, ["prod"]);
    }

    #[test]
    fn existing_include_line_survives_update() {
        let (_d, p, _) = setup("Host web\n    HostName 1.1.1.1\n    Include extra.conf\n");
        let s = set_hidden(&p, "web", true);
        assert!(s.docs[0].render().contains("    Include extra.conf\n"));
        let mut f = s.fields("web").unwrap();
        f.other.push("Include more.conf".into());
        assert!(validate_update(&s, "web", &f).is_err(), "a newly typed Include is still refused");
    }

    fn validate_update(s: &ConfigSet, alias: &str, f: &HostFields) -> Result<(), String> {
        s.clone().update_host(alias, f)
    }

    #[test]
    fn failed_move_keeps_host_in_source() {
        let (d, p, _) = setup("");
        let sub = d.path().join("work");
        std::fs::write(&sub, "Host other\n    HostName 2.2.2.2\n").unwrap();
        let inc = sub.display().to_string().replace('\\', "/");
        std::fs::write(&p, format!("Include \"{inc}\"\n\nHost web\n    HostName 1.1.1.1\n")).unwrap();
        let mut s = ConfigSet::load(&p).unwrap();
        s.move_host("web", 1).unwrap();
        let lock = || {
            #[cfg(windows)]
            {
                let mut perm = std::fs::metadata(&sub).unwrap().permissions();
                perm.set_readonly(true);
                std::fs::set_permissions(&sub, perm).unwrap();
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o400)).unwrap();
            }
        };
        // Make the target unwritable; on Unix the file mode alone can't stop a
        // rename, so the write is refused through the directory instead.
        lock();
        #[cfg(unix)]
        let locked_dir = {
            use std::os::unix::fs::PermissionsExt;
            let sd = d.path().join("locked");
            std::fs::create_dir(&sd).unwrap();
            let target = sd.join("work");
            std::fs::copy(&sub, &target).unwrap();
            s.docs[1].path = target;
            std::fs::set_permissions(&sd, std::fs::Permissions::from_mode(0o500)).unwrap();
            sd
        };
        assert!(s.save(&d.path().join("backups")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        #[cfg(windows)]
        {
            let mut perm = std::fs::metadata(&sub).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            std::fs::set_permissions(&sub, perm).unwrap();
        }
        assert!(std::fs::read_to_string(&p).unwrap().contains("Host web"), "source untouched when the target write fails");
    }

    #[cfg(unix)]
    #[test]
    fn save_through_symlink_keeps_link() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("dotfiles-config");
        std::fs::write(&real, "Host a\n    HostName 1.1.1.1\n").unwrap();
        let link = d.path().join("config");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut s = ConfigSet::load(&link).unwrap();
        let mut f = s.fields("a").unwrap();
        f.user = "root".into();
        s.update_host("a", &f).unwrap();
        s.save(&d.path().join("backups")).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(std::fs::read_to_string(&real).unwrap().contains("User root"));
    }
}
