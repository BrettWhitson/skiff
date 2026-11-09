//! A unified host list built from `~/.ssh/config` and termax's own connections.

use crate::config::{Config, Connection};
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
    pub remote_dir: Option<String>,
    pub tags: Vec<String>,
    pub source: Source,
    pub source_label: String,
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
            remote_dir: None,
            tags: vec![],
            source: Source::SshConfig,
            source_label: h.source.display().to_string(),
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
        options.extend(c.extra_args.iter().cloned());
        Self {
            name: c.name.clone(),
            hostname: c.host.clone(),
            user: c.user.clone(),
            port: c.port,
            identity_file: c.identity_file.clone(),
            proxy_jump: c.proxy_jump.clone(),
            remote_dir: c.remote_dir.clone(),
            tags: c.tags.clone(),
            source: Source::Custom(idx),
            source_label: "termax config".into(),
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
        format!(
            "{} {} {} {}",
            self.name,
            self.hostname,
            self.user.as_deref().unwrap_or(""),
            self.tags.join(" ")
        )
        .to_lowercase()
    }
}

/// Builds the combined list: custom connections first, then OpenSSH hosts.
/// Returns the list plus a warning if the OpenSSH config couldn't be read.
pub fn collect(cfg: &Config) -> (Vec<HostEntry>, Option<String>) {
    let mut hosts: Vec<HostEntry> =
        cfg.connections.iter().enumerate().map(|(i, c)| HostEntry::from_custom(i, c)).collect();
    let mut warning = None;
    if cfg.settings.load_ssh_config {
        let path = cfg
            .settings
            .ssh_config_path
            .as_deref()
            .map(sshconfig::expand_tilde)
            .or_else(sshconfig::default_path);
        if let Some(path) = path {
            match sshconfig::parse_file(&path) {
                Ok(list) => hosts.extend(list.iter().map(HostEntry::from_ssh)),
                Err(e) => warning = Some(format!("could not read {}: {e}", path.display())),
            }
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
        assert_eq!(
            h.sftp_args(true),
            ["-o", "BatchMode=yes", "-p", "2222", "-l", "root", "-J", "bastion", "-A", "-s", "10.1.1.1", "sftp"]
        );
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
    fn fuzzy() {
        let h = custom();
        assert!(fuzzy_score("lab", &h).unwrap() > fuzzy_score("l1", &h).unwrap());
        assert!(fuzzy_score("home", &h).is_some());
        assert!(fuzzy_score("zzz", &h).is_none());
    }
}
