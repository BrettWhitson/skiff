//! Port forwards (`[[forward]]` in config.toml), run as
//! `ssh -N -o ExitOnForwardFailure=yes <-L|-R|-D spec> <host>`.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

use crate::hosts::HostEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    #[default]
    Local,
    Remote,
    Dynamic,
}

impl ForwardKind {
    pub const ALL: [ForwardKind; 3] = [ForwardKind::Local, ForwardKind::Remote, ForwardKind::Dynamic];

    pub fn label(self) -> &'static str {
        match self {
            ForwardKind::Local => "Local",
            ForwardKind::Remote => "Remote",
            ForwardKind::Dynamic => "Dynamic (SOCKS)",
        }
    }

    /// What the forward does, in plain words, for the form.
    pub fn explain(self) -> &'static str {
        match self {
            ForwardKind::Local => {
                "Opens a port on this computer. Connections to it go through the host to the target."
            }
            ForwardKind::Remote => {
                "Opens a port on the host. Connections to it come back through this computer to the target. \
                 Other machines can reach it only if the server allows GatewayPorts."
            }
            ForwardKind::Dynamic => {
                "Opens a SOCKS proxy on this computer. Apps set to use it connect out through the host."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Forward {
    pub name: Option<String>,
    /// Host name from the host list.
    pub host: String,
    pub kind: ForwardKind,
    /// Address to listen on (default 127.0.0.1; for remote forwards, the server's default).
    pub bind: Option<String>,
    pub listen: u16,
    /// `host:port` (not used by dynamic forwards).
    pub target: Option<String>,
}

fn valid_target(t: &str) -> bool {
    let Some((h, p)) = t.rsplit_once(':') else { return false };
    let h = h.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(h);
    !h.is_empty() && !h.contains(char::is_whitespace) && p.parse::<u16>().is_ok_and(|p| p > 0)
}

impl Forward {
    pub fn validate(&self) -> Result<(), String> {
        if self.host.trim().is_empty() {
            return Err("Choose a host".into());
        }
        if self.listen == 0 {
            return Err("Listen port must be 1–65535".into());
        }
        if self.bind.as_deref().is_some_and(|b| b.contains(char::is_whitespace) || b.contains('/')) {
            return Err("Bind address can't contain spaces".into());
        }
        let target = self.target.as_deref().map(str::trim).filter(|t| !t.is_empty());
        match (self.kind, target) {
            (ForwardKind::Dynamic, Some(_)) => Err("Dynamic forwards have no target".into()),
            (ForwardKind::Dynamic, None) => Ok(()),
            (_, Some(t)) if valid_target(t) => Ok(()),
            _ => Err("Target must look like host:port".into()),
        }
    }

    /// The `-L`/`-R`/`-D` option and its value.
    pub fn spec_args(&self) -> [String; 2] {
        let bind = self.bind.as_deref().map(str::trim).filter(|b| !b.is_empty());
        let target = self.target.as_deref().unwrap_or("").trim();
        let local_bind = bind.unwrap_or("127.0.0.1");
        match self.kind {
            ForwardKind::Local => ["-L".into(), format!("{local_bind}:{}:{target}", self.listen)],
            ForwardKind::Remote => match bind {
                Some(b) => ["-R".into(), format!("{b}:{}:{target}", self.listen)],
                None => ["-R".into(), format!("{}:{target}", self.listen)],
            },
            ForwardKind::Dynamic => ["-D".into(), format!("{local_bind}:{}", self.listen)],
        }
    }

    /// `ssh` arguments that hold this forward open (no shell).
    pub fn ssh_args(&self, host: &HostEntry) -> Vec<String> {
        let mut v: Vec<String> = vec!["-N".into(), "-o".into(), "ExitOnForwardFailure=yes".into()];
        v.extend(self.spec_args());
        v.extend(host.shell_args());
        v
    }

    /// [`Self::ssh_args`] plus `-v`, so a background run can tell when the
    /// forward is up.
    pub fn background_args(&self, host: &HostEntry) -> Vec<String> {
        let mut v = vec!["-v".to_string()];
        v.extend(self.ssh_args(host));
        v
    }

    /// The name shown for the forward: its own name, else its host.
    pub fn label(&self) -> &str {
        self.name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&self.host)
    }

    /// e.g. `⇄ web 8080→localhost:80`.
    pub fn title(&self) -> String {
        let target = self.target.as_deref().unwrap_or("");
        match self.kind {
            ForwardKind::Local => format!("⇄ {} {}→{target}", self.label(), self.listen),
            ForwardKind::Remote => format!("⇄ {} remote {}→{target}", self.label(), self.listen),
            ForwardKind::Dynamic => format!("⇄ {} SOCKS {}", self.label(), self.listen),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardState {
    Starting,
    Running,
    /// ssh's own error line.
    Failed(String),
    Stopped,
}

impl ForwardState {
    pub fn label(&self) -> String {
        match self {
            ForwardState::Starting => "starting".into(),
            ForwardState::Running => "running".into(),
            ForwardState::Failed(e) => format!("failed: {e}"),
            ForwardState::Stopped => "stopped".into(),
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, ForwardState::Starting | ForwardState::Running)
    }
}

/// How long a quiet but alive ssh counts as running when its log shows no marker.
const RUNNING_FALLBACK: Duration = Duration::from_secs(10);

/// `-v` lines that mean the forward is set up.
fn is_up_marker(line: &str) -> bool {
    line.contains("forwarding listening on") || line.contains("forward success") || line.contains("Entering interactive session")
}

/// The line that best explains why ssh exited.
fn error_line(lines: &[String]) -> Option<String> {
    let useful: Vec<&String> =
        lines.iter().filter(|l| !l.starts_with("debug") && !l.starts_with("OpenSSH_") && !l.trim().is_empty()).collect();
    let key = ["bind", "listen", "address already in use", "could not", "error", "refused", "denied", "failed"];
    useful
        .iter()
        .find(|l| {
            let l = l.to_lowercase();
            key.iter().any(|k| l.contains(k))
        })
        .or(useful.last())
        .map(|l| l.trim().to_string())
}

/// A running forward. Dropping it stops ssh.
pub struct ForwardHandle {
    state: Arc<Mutex<ForwardState>>,
    stop: Arc<Notify>,
}

impl ForwardHandle {
    pub fn start(rt: &tokio::runtime::Handle, program: &str, args: Vec<String>, env: Vec<(String, String)>) -> ForwardHandle {
        let state = Arc::new(Mutex::new(ForwardState::Starting));
        let stop = Arc::new(Notify::new());
        let mut cmd = Command::new(program);
        cmd.args(&args).envs(env).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let (st, stop2, program) = (state.clone(), stop.clone(), program.to_string());
        rt.spawn(async move {
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    *st.lock().unwrap() = ForwardState::Failed(format!("starting `{program}`: {e}"));
                    return;
                }
            };
            let lines = Arc::new(Mutex::new(Vec::<String>::new()));
            let reader = child.stderr.take().map(|err| {
                let (lines, st) = (lines.clone(), st.clone());
                tokio::spawn(async move {
                    let mut r = BufReader::new(err).lines();
                    while let Ok(Some(l)) = r.next_line().await {
                        if is_up_marker(&l) {
                            let mut s = st.lock().unwrap();
                            if *s == ForwardState::Starting {
                                *s = ForwardState::Running;
                            }
                        }
                        let mut v = lines.lock().unwrap();
                        if v.len() >= 200 {
                            v.remove(0);
                        }
                        v.push(l);
                    }
                })
            });
            let fallback = tokio::time::sleep(RUNNING_FALLBACK);
            tokio::pin!(fallback);
            let mut stopped = false;
            let status = loop {
                tokio::select! {
                    s = child.wait() => break s,
                    _ = &mut fallback, if !stopped => {
                        let mut s = st.lock().unwrap();
                        if *s == ForwardState::Starting {
                            *s = ForwardState::Running;
                        }
                    }
                    _ = stop2.notified(), if !stopped => {
                        stopped = true;
                        let _ = child.start_kill();
                    }
                }
            };
            if let Some(r) = reader {
                let _ = tokio::time::timeout(Duration::from_secs(1), r).await;
            }
            let end = if stopped {
                ForwardState::Stopped
            } else {
                let why = error_line(&lines.lock().unwrap()).unwrap_or_else(|| match &status {
                    Ok(s) => format!("ssh exited ({s})"),
                    Err(e) => e.to_string(),
                });
                ForwardState::Failed(why)
            };
            *st.lock().unwrap() = end;
        });
        ForwardHandle { state, stop }
    }

    pub fn state(&self) -> ForwardState {
        self.state.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        self.stop.notify_one();
    }
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fwd(kind: ForwardKind, target: Option<&str>) -> Forward {
        Forward { name: None, host: "web".into(), kind, bind: None, listen: 8080, target: target.map(Into::into) }
    }

    #[test]
    fn spec_args_and_validation() {
        assert_eq!(fwd(ForwardKind::Local, Some("localhost:80")).spec_args(), ["-L", "127.0.0.1:8080:localhost:80"]);
        assert_eq!(fwd(ForwardKind::Remote, Some("localhost:80")).spec_args(), ["-R", "8080:localhost:80"]);
        assert_eq!(fwd(ForwardKind::Dynamic, None).spec_args(), ["-D", "127.0.0.1:8080"]);
        assert_eq!(fwd(ForwardKind::Local, Some("localhost:80")).title(), "⇄ web 8080→localhost:80");
        assert_eq!(fwd(ForwardKind::Dynamic, None).title(), "⇄ web SOCKS 8080");

        assert!(fwd(ForwardKind::Local, Some("[::1]:80")).validate().is_ok());
        assert_eq!(fwd(ForwardKind::Local, Some("nope")).validate().unwrap_err(), "Target must look like host:port");
        assert_eq!(fwd(ForwardKind::Local, None).validate().unwrap_err(), "Target must look like host:port");
        assert_eq!(fwd(ForwardKind::Dynamic, Some("a:1")).validate().unwrap_err(), "Dynamic forwards have no target");
        let zero = Forward { listen: 0, ..fwd(ForwardKind::Dynamic, None) };
        assert_eq!(zero.validate().unwrap_err(), "Listen port must be 1–65535");
    }

    #[test]
    fn forwards_round_trip_through_config_save() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        let base = crate::config::Config::load_or_create(&p).unwrap();
        let mut cfg = base.clone();
        cfg.forwards = vec![fwd(ForwardKind::Local, Some("localhost:80")), fwd(ForwardKind::Dynamic, None)];
        cfg.save(&base, &p).unwrap();
        let back = crate::config::Config::load_or_create(&p).unwrap();
        assert_eq!(back.forwards, cfg.forwards);
        let mut fewer = back.clone();
        fewer.forwards.remove(0);
        fewer.save(&back, &p).unwrap();
        assert_eq!(crate::config::Config::load_or_create(&p).unwrap().forwards, fewer.forwards);
    }

    #[test]
    fn error_line_prefers_the_cause() {
        let l = |v: &[&str]| error_line(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            l(&["OpenSSH_9.5", "debug1: x", "bind [127.0.0.1]:8080: Address already in use", "Could not request local forwarding."]),
            Some("bind [127.0.0.1]:8080: Address already in use".into())
        );
        assert_eq!(l(&["debug1: x"]), None);
    }
}
