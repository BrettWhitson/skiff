//! A unified host list built from `~/.ssh/config` and termax's own connections.

use std::path::PathBuf;

use crate::config::{Config, Connection};
use crate::sshconfig::edit::ConfigSet;
pub use crate::sshconfig::resolve::Probe;
use crate::sshconfig::resolve::{VIA_PROXY, jump_probe, probe_target_all};
use crate::sshconfig::{self, SshHost};

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    SshConfig,
    /// Index into `Config::connections`.
    Custom(usize),
}

#[derive(Debug, Clone)]
pub struct HostEntry {
    pub name: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub remote_dir: Option<String>,
    pub tags: Vec<String>,
    pub hidden: bool,
    pub source: Source,
    pub source_label: String,
    /// The ssh config file that first defines this host.
    pub source_file: Option<PathBuf>,
    /// `ssh` options placed before the destination.
    options: Vec<String>,
    destination: String,
}

impl HostEntry {
    fn from_ssh(h: &SshHost) -> Self {
        Self {
            name: h.alias.clone(),
            hostname: h.hostname.clone().unwrap_or_else(|| h.alias.clone()),
            user: h.user.clone(),
            port: h.port,
            identity_file: h.identity_file.clone(),
            proxy_jump: h.proxy_jump.clone(),
            proxy_command: h.proxy_command.clone(),
            remote_dir: h.remote_dir.clone(),
            tags: h.tags.clone(),
            hidden: h.hidden,
            source: Source::SshConfig,
            source_label: h.source.display().to_string(),
            source_file: Some(h.source.clone()),
            // OpenSSH resolves everything else from its own config.
            options: vec![],
            destination: h.alias.clone(),
        }
    }

    fn from_custom(idx: usize, c: &Connection) -> Self {
        let mut options = Vec::new();
        if let Some(port) = c.port {
            options.extend(["-p".into(), port.to_string()]);
        }
        if let Some(user) = c.user.as_deref().filter(|u| !u.is_empty()) {
            options.extend(["-l".into(), user.to_string()]);
        }
        if let Some(id) = c.identity_file.as_deref().filter(|s| !s.is_empty()) {
            options.extend(["-i".into(), sshconfig::expand_tilde(id).display().to_string()]);
        }
        if let Some(j) = c.proxy_jump.as_deref().filter(|s| !s.is_empty()) {
            options.extend(["-J".into(), j.to_string()]);
        }
        if let Some(pc) = c.proxy_command.as_deref().filter(|s| !s.is_empty()) {
            options.extend(["-o".into(), format!("ProxyCommand={pc}")]);
        }
        options.extend(c.extra_args.iter().cloned());
        Self {
            name: c.name.clone(),
            hostname: c.host.clone(),
            user: c.user.clone(),
            port: c.port,
            identity_file: c.identity_file.clone(),
            proxy_jump: c.proxy_jump.clone(),
            proxy_command: c.proxy_command.clone(),
            remote_dir: c.remote_dir.clone(),
            tags: c.tags.clone(),
            hidden: c.hidden,
            source: Source::Custom(idx),
            source_label: "termax config".into(),
            source_file: None,
            options,
            destination: c.host.clone(),
        }
    }

    /// Arguments for an interactive shell: `ssh <these>`.
    pub fn shell_args(&self) -> Vec<String> {
        let mut args = self.options.clone();
        args.push(self.destination.clone());
        args
    }

    /// Arguments that start the SFTP subsystem over stdio: `ssh <these>`.
    pub fn sftp_args(&self, batch_mode: bool) -> Vec<String> {
        let mut args = Vec::new();
        if batch_mode {
            args.extend(["-o".into(), "BatchMode=yes".into()]);
        }
        args.extend(self.options.iter().cloned());
        args.extend(["-s".into(), self.destination.clone(), "sftp".into()]);
        args
    }

    /// Everything needed to open an SFTP session to this host.
    pub fn sftp_launch(&self, program: &str, batch_mode: bool) -> crate::fs::connect::SshLaunch {
        crate::fs::connect::SshLaunch {
            program: program.to_string(),
            args: self.sftp_args(batch_mode),
            env: vec![],
            start_dir: self.remote_dir.clone(),
        }
    }

    /// Where a status check connects. `set` is the loaded ssh config, which
    /// also applies to custom connections (ssh reads it for every host).
    /// `sets` is `None` when a config file couldn't be read: the host might
    /// be proxied, so it isn't probed.
    pub fn probe(&self, sets: Option<&[ConfigSet]>) -> Probe {
        let has = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty() && !s.eq_ignore_ascii_case("none"));
        if has(&self.proxy_command) || self.options.iter().any(|a| a.to_ascii_lowercase().contains("proxycommand")) {
            return Probe::Unknown(VIA_PROXY.into());
        }
        let Some(sets) = sets else { return Probe::Unknown("ssh config unreadable".into()) };
        // A jump host on the command line (`-J`, `-o ProxyJump=`) beats the config.
        if let Some(jump) = self.cli_jump() {
            return match jump {
                Some(j) => jump_probe(sets, &j),
                None => Probe::Unknown(VIA_PROXY.into()),
            };
        }
        if self.source == Source::SshConfig && sets.is_empty() && has(&self.proxy_jump) {
            return jump_probe(sets, self.proxy_jump.as_deref().unwrap_or_default());
        }
        let dest = self.destination.rsplit('@').next().unwrap_or(&self.destination);
        let probe = match sets {
            [] => Probe::Direct { host: self.hostname.clone(), port: self.port.unwrap_or(22) },
            sets => probe_target_all(sets, dest),
        };
        // `-p` on the command line (the port field or extra args) beats the config.
        let arg_port = self.options.windows(2).filter(|w| w[0] == "-p").filter_map(|w| w[1].parse::<u16>().ok()).next_back();
        match (probe, &self.source) {
            (Probe::Direct { host, port }, Source::Custom(_)) => Probe::Direct { host, port: arg_port.unwrap_or(port) },
            (p, _) => p,
        }
    }

    /// The jump host given in the ssh options: `Some(Some(spec))` for
    /// `-J spec` or `-o ProxyJump=spec`, `Some(None)` when there is one this
    /// can't read, `None` when there is none (`ProxyJump=none` included).
    fn cli_jump(&self) -> Option<Option<String>> {
        let mut args = self.options.iter();
        while let Some(a) = args.next() {
            let spec = match a.as_str() {
                "-J" => args.next().cloned(),
                "-o" => match args.next().map(|o| proxy_jump_opt(o)) {
                    Some(Some(v)) => v,
                    _ => continue,
                },
                a if a.starts_with("-J") => Some(a[2..].to_string()),
                a if a.starts_with("-o") => match proxy_jump_opt(&a[2..]) {
                    Some(v) => v,
                    None => continue,
                },
                _ => continue,
            };
            // ssh uses the first value it gets.
            return match spec.as_deref().map(str::trim) {
                Some(s) if s.eq_ignore_ascii_case("none") => None,
                Some(s) if !s.is_empty() => Some(Some(s.to_string())),
                _ => Some(None),
            };
        }
        None
    }

    /// `user@host:port` for display.
    pub fn target(&self) -> String {
        let mut s = String::new();
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        s.push_str(&self.hostname);
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        s
    }

    fn haystack(&self) -> String {
        format!("{} {} {} {}", self.name, self.hostname, self.user.as_deref().unwrap_or(""), self.tags.join(" ")).to_lowercase()
    }
}

/// For an `-o` option: `Some(value)` when it sets ProxyJump (`None` inside
/// when it does in a way this can't read), `None` for other options.
fn proxy_jump_opt(opt: &str) -> Option<Option<String>> {
    match crate::sshconfig::split_kv(opt.trim()) {
        Some((k, v)) if k.eq_ignore_ascii_case("proxyjump") => Some(Some(v.to_string())),
        _ if opt.to_ascii_lowercase().contains("proxyjump") => Some(None),
        _ => None,
    }
}

/// Indices of the hosts a list shows.
pub fn visible(hosts: &[HostEntry], show_hidden: bool) -> Vec<usize> {
    (0..hosts.len()).filter(|&i| show_hidden || !hosts[i].hidden).collect()
}

/// The OpenSSH config file in use, if loading it is on.
pub fn ssh_config_path(cfg: &Config) -> Option<PathBuf> {
    if !cfg.settings.load_ssh_config {
        return None;
    }
    cfg.settings.ssh_config_path.as_deref().map(sshconfig::expand_tilde).or_else(sshconfig::default_path)
}

/// The ssh config files a status check consults — the user's and the
/// system-wide one — since ssh reads them for every host, whether or not
/// termax lists their hosts. `None` when one exists but can't be read.
pub fn probe_sets(cfg: &Config) -> Option<Vec<ConfigSet>> {
    let user = cfg.settings.ssh_config_path.as_deref().map(sshconfig::expand_tilde).or_else(sshconfig::default_path);
    let system = if cfg!(windows) {
        std::env::var_os("PROGRAMDATA").map(|d| PathBuf::from(d).join("ssh").join("ssh_config"))
    } else {
        Some(PathBuf::from("/etc/ssh/ssh_config"))
    };
    user.into_iter().chain(system).map(|p| ConfigSet::load(&p).ok()).collect()
}

/// Builds the combined list: custom connections first, then OpenSSH hosts.
/// Returns the list plus a warning if the OpenSSH config couldn't be read.
pub fn collect(cfg: &Config) -> (Vec<HostEntry>, Option<String>) {
    let mut hosts: Vec<HostEntry> = cfg.connections.iter().enumerate().map(|(i, c)| HostEntry::from_custom(i, c)).collect();
    let mut warning = None;
    if let Some(path) = ssh_config_path(cfg) {
        match sshconfig::parse_file(&path) {
            Ok(list) => hosts.extend(list.iter().map(HostEntry::from_ssh)),
            Err(e) => warning = Some(format!("could not read {}: {e}", path.display())),
        }
    }
    (hosts, warning)
}

/// Subsequence fuzzy match. Returns a score (higher is better) or `None`.
pub fn fuzzy_score(query: &str, entry: &HostEntry) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let hay = entry.haystack();
    let q = query.to_lowercase();
    if let Some(pos) = hay.find(&q) {
        // Contiguous matches beat scattered ones; earlier is better.
        return Some(1000 - pos as i64);
    }
    let mut score = 0i64;
    let mut last: Option<usize> = None;
    let mut chars = hay.char_indices();
    for qc in q.chars() {
        let (i, _) = chars.find(|&(_, c)| c == qc)?;
        score += match last {
            Some(l) if i == l + 1 => 5,
            _ => 1,
        };
        last = Some(i);
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom() -> HostEntry {
        HostEntry::from_custom(
            0,
            &Connection {
                name: "lab".into(),
                host: "10.1.1.1".into(),
                user: Some("root".into()),
                port: Some(2222),
                proxy_jump: Some("bastion".into()),
                extra_args: vec!["-A".into()],
                tags: vec!["home".into()],
                ..Default::default()
            },
        )
    }

    #[test]
    fn custom_args() {
        let h = custom();
        assert_eq!(h.shell_args(), ["-p", "2222", "-l", "root", "-J", "bastion", "-A", "10.1.1.1"]);
        assert_eq!(h.sftp_args(true), ["-o", "BatchMode=yes", "-p", "2222", "-l", "root", "-J", "bastion", "-A", "-s", "10.1.1.1", "sftp"]);
        assert_eq!(h.target(), "root@10.1.1.1:2222");
    }

    #[test]
    fn sftp_launch_carries_args_and_dir() {
        let mut h = custom();
        h.remote_dir = Some("/srv".into());
        let l = h.sftp_launch("ssh.exe", true);
        assert_eq!(l.program, "ssh.exe");
        assert_eq!(l.args, h.sftp_args(true));
        assert_eq!(l.start_dir.as_deref(), Some("/srv"));
        assert!(l.env.is_empty());
    }

    #[test]
    fn ssh_config_hosts_use_alias() {
        let h = HostEntry::from_ssh(&SshHost { alias: "web".into(), hostname: Some("1.2.3.4".into()), ..Default::default() });
        assert_eq!(h.shell_args(), ["web"]);
        assert_eq!(h.sftp_args(false), ["-s", "web", "sftp"]);
    }

    #[test]
    fn custom_proxy_command_arg_and_probe() {
        let c = Connection { name: "p".into(), host: "10.0.0.9".into(), proxy_command: Some("nc %h %p".into()), ..Default::default() };
        let h = HostEntry::from_custom(0, &c);
        assert_eq!(h.shell_args(), ["-o", "ProxyCommand=nc %h %p", "10.0.0.9"]);
        assert!(matches!(h.probe(None), Probe::Unknown(_)));
        let c = Connection { name: "d".into(), host: "10.0.0.9".into(), port: Some(2222), ..Default::default() };
        assert_eq!(HostEntry::from_custom(0, &c).probe(Some(&[])), Probe::Direct { host: "10.0.0.9".into(), port: 2222 });
        let p = Connection { extra_args: vec!["-p".into(), "2200".into()], port: None, ..c.clone() };
        assert_eq!(HostEntry::from_custom(0, &p).probe(Some(&[])), Probe::Direct { host: "10.0.0.9".into(), port: 2200 });
        let c = Connection { extra_args: vec!["-J".into(), "b".into()], ..c };
        assert!(matches!(HostEntry::from_custom(0, &c).probe(None), Probe::Unknown(_)));
        assert_eq!(HostEntry::from_custom(0, &c).probe(Some(&[])), Probe::Via { jump: "b".into(), host: "b".into(), port: 22 });
    }

    #[test]
    fn hidden_filtered_unless_shown() {
        let a = custom();
        let mut b = custom();
        b.hidden = true;
        let hosts = [a, b];
        assert_eq!(visible(&hosts, false), [0]);
        assert_eq!(visible(&hosts, true), [0, 1]);
    }

    #[test]
    fn fuzzy() {
        let h = custom();
        assert!(fuzzy_score("lab", &h).unwrap() > fuzzy_score("l1", &h).unwrap());
        assert!(fuzzy_score("home", &h).is_some());
        assert!(fuzzy_score("zzz", &h).is_none());
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    use crate::config::Connection;

    fn conn(extra: &[&str]) -> HostEntry {
        let c = Connection {
            name: "d".into(),
            host: "10.0.0.9".into(),
            extra_args: extra.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        HostEntry::from_custom(0, &c)
    }

    #[test]
    fn unreadable_ssh_config_is_unknown() {
        assert!(matches!(conn(&[]).probe(None), Probe::Unknown(_)));
        assert_eq!(conn(&[]).probe(Some(&[])), Probe::Direct { host: "10.0.0.9".into(), port: 22 });
    }

    #[test]
    fn jump_options_probe_the_jump_host() {
        let via = |j: &str, port| Probe::Via { jump: j.into(), host: j.into(), port };
        assert_eq!(conn(&["-Jbastion"]).probe(Some(&[])), via("bastion", 22));
        assert_eq!(conn(&["-o", "ProxyJump=me@j:2022"]).probe(Some(&[])), via("j", 2022));
        assert_eq!(conn(&["-oProxyJump j2"]).probe(Some(&[])), via("j2", 22));
        assert!(matches!(conn(&["-o", "ProxyCommand=nc %h %p"]).probe(Some(&[])), Probe::Unknown(_)));
    }

    #[test]
    fn probe_sets_read_ssh_config_even_when_not_listed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config");
        std::fs::write(&p, "Host *\n    ProxyJump bastion\n").unwrap();
        let mut cfg = Config::default();
        cfg.settings.load_ssh_config = false;
        cfg.settings.ssh_config_path = Some(p.display().to_string());
        let sets = probe_sets(&cfg).expect("readable");
        assert!(matches!(conn(&[]).probe(Some(&sets)), Probe::Via { jump, .. } if jump == "bastion"));
        std::fs::write(&p, b"Host a\n    User \xe9\n").unwrap();
        assert!(probe_sets(&cfg).is_none(), "an unreadable file is reported, not skipped");
    }
}
