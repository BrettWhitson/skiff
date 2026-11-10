//! Works out where a status check for a host should connect, in-process
//! (never `ssh -G`). Conservative: a `ProxyCommand`, or anything this
//! resolver can't evaluate, is `Unknown`. A `ProxyJump` host is checked by
//! connecting to its first jump host only (TCP and banner, never a login).

use super::edit::ConfigSet;
use super::{MAX_INCLUDE_DEPTH, glob_match, resolve_include, split_kv, unquote};

/// Where to probe a host.
#[derive(Debug, Clone, PartialEq)]
pub enum Probe {
    Direct { host: String, port: u16 },
    /// The host is reached through jump host `jump` (as written in
    /// `ProxyJump`); only the jump host, at `host:port`, is checked.
    Via { jump: String, host: String, port: u16 },
    /// Not probed; the reason is shown to the user.
    Unknown(String),
}

pub const VIA_PROXY: &str = "via proxy";

/// How many jump hosts deep `ProxyJump` chains are followed.
const MAX_JUMP_DEPTH: usize = 8;

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
    /// The first `ProxyJump` or `ProxyCommand` value, and whether it was a jump.
    proxy: Option<(bool, String)>,
    /// A `Match` block sets something this resolver can't evaluate.
    match_blocks: bool,
}

/// Walks doc `di` in file order, splicing in included files where the
/// `Include` line is, so "first value wins" follows OpenSSH.
/// `in_match` is set for a file included from inside a `Match` block.
fn walk(set: &ConfigSet, di: usize, alias: &str, mut active: bool, mut in_match: bool, depth: usize, found: &mut Found) {
    if depth > MAX_INCLUDE_DEPTH {
        return;
    }
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
                            walk(set, inc, alias, active, in_match, depth + 1, found);
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
                found.proxy.get_or_insert_with(|| (key == "proxyjump", unquote(v).to_string()));
            }
            _ => {}
        }
    }
}

/// The address a status check for ssh config host `alias` connects to.
pub fn probe_target(set: &ConfigSet, alias: &str) -> Probe {
    probe_target_all(std::slice::from_ref(set), alias)
}

/// Like [`probe_target`] over several config files read in order (the
/// user's, then the system-wide one): the first value wins across them.
pub fn probe_target_all(sets: &[ConfigSet], alias: &str) -> Probe {
    probe_at_depth(sets, alias, 0)
}

fn probe_at_depth(sets: &[ConfigSet], alias: &str, depth: usize) -> Probe {
    let mut f = Found::default();
    for set in sets.iter().filter(|s| !s.docs.is_empty()) {
        walk(set, 0, alias, true, false, 0, &mut f);
    }
    if f.match_blocks {
        return Probe::Unknown("Match rules".into());
    }
    match f.proxy {
        Some((_, p)) if p.eq_ignore_ascii_case("none") => {}
        // ssh ignores a ProxyJump that points at the host itself (the usual
        // `Host *` + `ProxyJump bastion` setup, as seen from bastion).
        Some((true, jump)) if first_hop(&jump).is_some_and(|(h, _)| h.eq_ignore_ascii_case(alias)) => {}
        Some((true, jump)) => return jump_at_depth(sets, &jump, depth + 1),
        Some((false, _)) => return Probe::Unknown(VIA_PROXY.into()),
        None => {}
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

/// Splits the first hop of a `ProxyJump` / `-J` value
/// (`[user@]host[:port]` or `ssh://[user@]host[:port]`) into host and port.
fn first_hop(spec: &str) -> Option<(String, Option<u16>)> {
    let hop = spec.split(',').next()?.trim();
    let hop = hop.strip_prefix("ssh://").unwrap_or(hop);
    let hop = hop.rsplit_once('@').map_or(hop, |(_, h)| h);
    let (host, port) = if let Some(rest) = hop.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        (h, after.strip_prefix(':'))
    } else {
        match hop.split_once(':') {
            Some((h, p)) if !p.contains(':') => (h, Some(p)),
            _ => (hop, None),
        }
    };
    if host.is_empty() {
        return None;
    }
    let port = match port {
        None => None,
        Some(p) => Some(p.parse::<u16>().ok().filter(|p| *p > 0)?),
    };
    Some((host.to_string(), port))
}

/// Where to check a host reached with `ProxyJump` value `spec`: its first
/// jump host, resolved through the ssh config like any host (a jump host
/// that itself jumps is followed to the first network hop).
pub fn jump_probe(sets: &[ConfigSet], spec: &str) -> Probe {
    jump_at_depth(sets, spec, 1)
}

fn jump_at_depth(sets: &[ConfigSet], spec: &str, depth: usize) -> Probe {
    if depth > MAX_JUMP_DEPTH {
        return Probe::Unknown("jump hosts loop".into());
    }
    let Some((jump, port)) = first_hop(spec) else { return Probe::Unknown(format!("jump host {spec}")) };
    match probe_at_depth(sets, &jump, depth) {
        Probe::Direct { host, port: p } => Probe::Via { jump, host, port: port.unwrap_or(p) },
        other => other,
    }
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
    fn proxy_command_is_unknown_and_jumps_probe_the_first_hop() {
        assert!(matches!(probe("Host web\n ProxyCommand nc %h 22\n", "web"), Probe::Unknown(_)));
        let via = |jump: &str, h: &str, port| Probe::Via { jump: jump.into(), host: h.into(), port };
        let cfg = "Host web\n HostName x\nHost bastion\n HostName 10.0.0.1\n Port 2222\n ProxyJump none\nHost *\n ProxyJump bastion\n";
        assert_eq!(probe(cfg, "web"), via("bastion", "10.0.0.1", 2222));
        assert_eq!(probe("Host web\n ProxyJump me@b1:2200,b2\n", "web"), via("b1", "b1", 2200));
        assert_eq!(probe("Host web\n ProxyJump ssh://[::1]:23\n", "web"), via("::1", "::1", 23));
        assert_eq!(probe("Host web\n ProxyJump a\nHost a\n ProxyJump b\n", "web"), via("b", "b", 22));
        assert!(matches!(probe("Host web\n ProxyJump a\nHost a\n ProxyJump web\n", "web"), Probe::Unknown(_)));
        assert_eq!(probe("Host *\n ProxyJump bastion\n", "web"), via("bastion", "bastion", 22));
        assert!(matches!(probe("Host web\n ProxyJump a\nHost a\n ProxyCommand nc\n", "web"), Probe::Unknown(_)));
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

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn match_then_include_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let inc = dir.path().join("proxied.conf");
        std::fs::write(&inc, "ProxyJump bastion\n").unwrap();
        let inc = inc.display().to_string().replace('\\', "/");
        let p = dir.path().join("config");
        std::fs::write(&p, format!("Host web\n    HostName 10.0.0.5\nMatch host 10.0.0.5\n    Include \"{inc}\"\n")).unwrap();
        let set = ConfigSet::load(&p).unwrap();
        assert!(matches!(probe_target(&set, "web"), Probe::Unknown(_)));
    }
}
