//! Minimal reader for OpenSSH client config files (`~/.ssh/config`).
//!
//! termax never needs to fully *evaluate* the config: sessions for these hosts
//! are started as `ssh <alias>`, so OpenSSH itself applies every option. This
//! parser only extracts enough to list hosts and show their details.

use std::path::{Path, PathBuf};

pub mod doc;
pub mod edit;
pub mod resolve;

pub(crate) const MAX_INCLUDE_DEPTH: usize = 16;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    /// From the `# termax:` comment of the host's first block.
    pub tags: Vec<String>,
    pub hidden: bool,
    pub remote_dir: Option<String>,
    pub source: PathBuf,
}

pub fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}

pub fn default_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".ssh").join("config"))
}

/// Expands a leading `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// Parses `path` (and any files it `Include`s). A missing file yields no hosts.
pub fn parse_file(path: &Path) -> anyhow::Result<Vec<SshHost>> {
    let mut out = Vec::new();
    parse_file_into(path, 0, &mut out)?;
    Ok(out)
}

fn parse_file_into(path: &Path, depth: usize, out: &mut Vec<SshHost>) -> anyhow::Result<()> {
    if depth > MAX_INCLUDE_DEPTH || !path.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(path)?;
    parse_str(&text, path, depth, out);
    Ok(())
}

/// Parses config text. `source` is used for display and to resolve relative
/// `Include` paths (which OpenSSH resolves against `~/.ssh`).
pub fn parse_str(text: &str, source: &Path, depth: usize, out: &mut Vec<SshHost>) {
    // Indices into `out` for the hosts declared by the current `Host` line.
    let mut current: Vec<usize> = Vec::new();
    // Hosts first declared by the current `Host` line: they take its extras.
    let mut fresh: Vec<usize> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            if let Some((tags, hidden, dir)) = doc::parse_extras(line) {
                for &i in &fresh {
                    let h = &mut out[i];
                    h.tags = tags.clone();
                    h.hidden = hidden;
                    h.remote_dir = dir.clone();
                }
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = split_kv(line) else { continue };
        let key = key.to_ascii_lowercase();

        match key.as_str() {
            "host" => {
                current.clear();
                fresh.clear();
                for pattern in value.split_whitespace() {
                    let pattern = unquote(pattern);
                    // Wildcard and negated patterns are defaults, not connectable hosts.
                    if pattern.contains(['*', '?', '!']) {
                        continue;
                    }
                    let idx = match out.iter().position(|h| h.alias == pattern) {
                        Some(i) => i,
                        None => {
                            out.push(SshHost { alias: pattern.to_string(), source: source.to_path_buf(), ..Default::default() });
                            fresh.push(out.len() - 1);
                            out.len() - 1
                        }
                    };
                    current.push(idx);
                }
            }
            "match" => {
                current.clear();
                fresh.clear();
            }
            "include" => {
                for pat in value.split_whitespace() {
                    for file in resolve_include(unquote(pat)) {
                        let _ = parse_file_into(&file, depth + 1, out);
                    }
                }
            }
            _ => {
                let value = unquote(value).to_string();
                for &i in &current {
                    let h = &mut out[i];
                    // OpenSSH semantics: the first obtained value wins.
                    match key.as_str() {
                        "hostname" => set_once(&mut h.hostname, &value),
                        "user" => set_once(&mut h.user, &value),
                        "port" => {
                            if h.port.is_none() {
                                h.port = value.parse().ok();
                            }
                        }
                        "identityfile" => set_once(&mut h.identity_file, &value),
                        "proxyjump" => set_once(&mut h.proxy_jump, &value),
                        "proxycommand" => set_once(&mut h.proxy_command, &value),
                        _ => {}
                    }
                }
            }
        }
    }
}

fn set_once(slot: &mut Option<String>, value: &str) {
    if slot.is_none() {
        *slot = Some(value.to_string());
    }
}

/// Splits `Key value`, `Key=value` or `Key = value`.
pub(crate) fn split_kv(line: &str) -> Option<(&str, &str)> {
    let end = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = &line[..end];
    let rest = line[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    if key.is_empty() || rest.is_empty() {
        return None;
    }
    Some((key, rest))
}

pub(crate) fn unquote(s: &str) -> &str {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s)
}

pub(crate) fn resolve_include(pattern: &str) -> Vec<PathBuf> {
    let mut path = expand_tilde(pattern);
    if path.is_relative()
        && let Some(home) = home_dir()
    {
        path = home.join(".ssh").join(path);
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else { return vec![] };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| glob_match(&name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}

/// Glob matching supporting `*` and `?`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<SshHost> {
        let mut out = Vec::new();
        parse_str(text, Path::new("test"), 0, &mut out);
        out
    }

    #[test]
    fn parses_basic_hosts() {
        let hosts = parse(
            "# comment\nHost web db\n  HostName 10.0.0.1\n  User admin\n  Port 2222\n\nHost jump\n  HostName=jump.example.com\n  IdentityFile \"~/.ssh/id ed\"\n  ProxyJump bastion\n",
        );
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].alias, "web");
        assert_eq!(hosts[1].alias, "db");
        assert_eq!(hosts[1].hostname.as_deref(), Some("10.0.0.1"));
        assert_eq!(hosts[1].port, Some(2222));
        assert_eq!(hosts[2].hostname.as_deref(), Some("jump.example.com"));
        assert_eq!(hosts[2].identity_file.as_deref(), Some("~/.ssh/id ed"));
        assert_eq!(hosts[2].proxy_jump.as_deref(), Some("bastion"));
    }

    #[test]
    fn skips_wildcards_and_match_blocks() {
        let hosts = parse("Host *\n  User everyone\nHost a !b c*\n  User x\nMatch host a\n  User y\n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "a");
        assert_eq!(hosts[0].user.as_deref(), Some("x"));
    }

    #[test]
    fn first_value_wins() {
        let hosts = parse("Host a\n  User one\n  User two\nHost a\n  User three\n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].user.as_deref(), Some("one"));
    }

    #[test]
    fn reader_sees_extras_and_proxycommand() {
        let hosts = parse(
            "Host a
  # termax: tags=x,y; hidden; dir=/srv
  ProxyCommand nc %h 22
Host a
  # termax: tags=z
Host b
",
        );
        assert_eq!(hosts[0].tags, ["x", "y"]);
        assert!(hosts[0].hidden);
        assert_eq!(hosts[0].remote_dir.as_deref(), Some("/srv"));
        assert_eq!(hosts[0].proxy_command.as_deref(), Some("nc %h 22"));
        assert!(hosts[1].tags.is_empty() && !hosts[1].hidden);
    }

    #[test]
    fn glob() {
        assert!(glob_match("*.conf", "work.conf"));
        assert!(glob_match("h?st", "host"));
        assert!(!glob_match("*.conf", "work.txt"));
        assert!(glob_match("*", ""));
    }
}
