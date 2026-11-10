//! Works out where a status check for a host should connect, in-process
//! (never `ssh -G`). Conservative: anything that might send the connection
//! through a proxy, or that this resolver can't evaluate, is `Unknown`.

use super::edit::ConfigSet;
use super::{MAX_INCLUDE_DEPTH, glob_match, resolve_include, split_kv, unquote};

/// Where to probe a host.
#[derive(Debug, Clone, PartialEq)]
pub enum Probe {
    Direct { host: String, port: u16 },
    /// Not probed; the reason is shown to the user.
    Unknown(String),
}

pub const VIA_PROXY: &str = "via proxy";

/// True when `alias` matches a `Host` line's patterns (`!` negates).
pub fn host_matches(patterns: &[String], alias: &str) -> bool {
    let a = alias.to_ascii_lowercase();
    let mut hit = false;
    for p in patterns {
        let p = p.to_ascii_lowercase();
        match p.strip_prefix('!') {
            Some(neg) if glob_match(neg, &a) => return false,
            Some(_) => {}
            None => hit |= glob_match(&p, &a),
        }
    }
    hit
}

#[derive(Default)]
struct Found {
    hostname: Option<String>,
    port: Option<String>,
    proxy: Option<String>,
    /// A `Match` block sets something this resolver can't evaluate.
    match_blocks: bool,
}

/// Walks doc `di` in file order, splicing in included files where the
/// `Include` line is, so "first value wins" follows OpenSSH.
fn walk(set: &ConfigSet, di: usize, alias: &str, mut active: bool, depth: usize, found: &mut Found) {
    if depth > MAX_INCLUDE_DEPTH {
        return;
    }
    let mut in_match = false;
    for (line, _) in &set.docs[di].lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((k, v)) = split_kv(t) else { continue };
        let key = k.to_ascii_lowercase();
        match key.as_str() {
            "host" => {
                in_match = false;
                let pats: Vec<String> = v.split_whitespace().map(|p| unquote(p).to_string()).collect();
                active = host_matches(&pats, alias);
            }
            "match" => {
                in_match = true;
                active = false;
            }
            "include" => {
                for p in v.split_whitespace() {
                    for path in resolve_include(unquote(p)) {
                        if let Some(inc) = set.docs.iter().position(|d| d.path == path) {
                            walk(set, inc, alias, active, depth + 1, found);
                        }
                    }
                }
            }
            "hostname" | "port" | "proxyjump" | "proxycommand" if in_match => found.match_blocks = true,
            _ if !active => {}
            "hostname" => {
                found.hostname.get_or_insert_with(|| unquote(v).to_string());
            }
            "port" => {
                found.port.get_or_insert_with(|| unquote(v).to_string());
            }
            "proxyjump" | "proxycommand" => {
                found.proxy.get_or_insert_with(|| unquote(v).to_string());
            }
            _ => {}
        }
    }
}

/// The address a status check for ssh config host `alias` connects to.
pub fn probe_target(set: &ConfigSet, alias: &str) -> Probe {
    let mut f = Found::default();
    if !set.docs.is_empty() {
        walk(set, 0, alias, true, 0, &mut f);
    }
    if f.match_blocks {
        return Probe::Unknown("Match rules".into());
    }
    if f.proxy.as_deref().is_some_and(|p| !p.eq_ignore_ascii_case("none")) {
        return Probe::Unknown(VIA_PROXY.into());
    }
    let host = f.hostname.unwrap_or_else(|| alias.to_string());
    let host = host.replace("%h", alias);
    if host.contains('%') {
        return Probe::Unknown("uses % tokens".into());
    }
    let port = match f.port {
        None => 22,
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n > 0 => n,
            _ => return Probe::Unknown(format!("port {p}")),
        },
    };
    Probe::Direct { host, port }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(text: &str, alias: &str) -> Probe {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config");
        std::fs::write(&p, text).unwrap();
        probe_target(&ConfigSet::load(&p).unwrap(), alias)
    }

    fn direct(h: &str, port: u16) -> Probe {
        Probe::Direct { host: h.into(), port }
    }

    #[test]
    fn direct_uses_hostname_and_wildcard_port() {
        assert_eq!(probe("Host web\n HostName 10.0.0.5\nHost *\n Port 2200\n", "web"), direct("10.0.0.5", 2200));
        assert_eq!(probe("Host web\n Port 1\nHost *\n Port 2200\n", "web"), direct("web", 1));
        assert_eq!(probe("Host *.lan !nas.lan\n Port 9\n", "nas.lan"), direct("nas.lan", 22));
        assert_eq!(probe("Host *.lan\n ProxyJump none\n", "a.lan"), direct("a.lan", 22));
    }

    #[test]
    fn inherited_proxy_is_unknown() {
        assert!(matches!(probe("Host web\n HostName x\nHost *\n ProxyJump bastion\n", "web"), Probe::Unknown(_)));
        assert!(matches!(probe("Host web\n ProxyCommand nc %h 22\n", "web"), Probe::Unknown(_)));
    }

    #[test]
    fn match_with_proxy_makes_all_unknown() {
        assert!(matches!(probe("Host web\n HostName x\nMatch host x\n ProxyJump b\n", "web"), Probe::Unknown(_)));
        assert_eq!(probe("Host web\n HostName x\nMatch host x\n User b\n", "web"), direct("x", 22));
    }

    #[test]
    fn percent_h_substituted_other_tokens_unknown() {
        assert_eq!(probe("Host web\n HostName %h.example.com\n", "web"), direct("web.example.com", 22));
        assert!(matches!(probe("Host web\n HostName %r.example.com\n", "web"), Probe::Unknown(_)));
    }
}
