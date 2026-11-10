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

/// Which direction a forward carries connections (ssh's `-L`, `-R` or `-D`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    #[default]
    /// `-L`: a local port reaches a target through the host.
    Local,
    /// `-R`: a port on the host reaches a target through this computer.
    Remote,
    /// `-D`: a local SOCKS proxy that connects out through the host.
    Dynamic,
}

impl ForwardKind {
    /// Every kind, in the order the form lists them.
    pub const ALL: [ForwardKind; 3] = [ForwardKind::Local, ForwardKind::Remote, ForwardKind::Dynamic];

    /// Short name for the kind.
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
            ForwardKind::Local => "Opens a port on this computer. Connections to it go through the host to the target.",
            ForwardKind::Remote => {
                "Opens a port on the host. Connections to it come back through this computer to the target. \
                 Other machines can reach it only if the server allows GatewayPorts."
            }
            ForwardKind::Dynamic => "Opens a SOCKS proxy on this computer. Apps set to use it connect out through the host.",
        }
    }
}

/// One saved port forward.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Forward {
    /// Optional display name; the host is shown when unset.
    pub name: Option<String>,
    /// Host name from the host list.
    pub host: String,
    /// Direction of the forward.
    pub kind: ForwardKind,
    /// Address to listen on (default 127.0.0.1; for remote forwards, the server's default).
    pub bind: Option<String>,
    /// Port to listen on.
    pub listen: u16,
    /// `host:port` (not used by dynamic forwards).
    pub target: Option<String>,
}

/// Whether `t` is `host:port` (host may be `[ipv6]`) with a nonzero port.
fn valid_target(t: &str) -> bool {
    let Some((h, p)) = t.rsplit_once(':') else { return false };
    let h = h.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(h);
    !h.is_empty() && !h.contains(char::is_whitespace) && p.parse::<u16>().is_ok_and(|p| p > 0)
}

impl Forward {
    /// Checks the form's fields; the error is a message for the user.
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

/// Where a forward's ssh process is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardState {
    /// ssh started; the forward is not known to be up yet.
    Starting,
    /// The forward is up (or ssh has stayed alive past `RUNNING_FALLBACK`).
    Running,
    /// ssh's own error line.
    Failed(String),
    /// Stopped on request.
    Stopped,
}

impl ForwardState {
    /// Plain words for the state.
    pub fn label(&self) -> String {
        match self {
            ForwardState::Starting => "starting".into(),
            ForwardState::Running => "running".into(),
            ForwardState::Failed(e) => format!("failed: {e}"),
            ForwardState::Stopped => "stopped".into(),
        }
    }

    /// Whether ssh is (or should soon be) running.
    pub fn is_live(&self) -> bool {
        matches!(self, ForwardState::Starting | ForwardState::Running)
    }
}

/// How long a quiet but alive ssh counts as running when its log shows no marker.
pub const RUNNING_FALLBACK: Duration = Duration::from_secs(10);

/// `-v` lines that mean the forward is set up.
fn is_up_marker(line: &str) -> bool {
    line.contains("forwarding listening on") || line.contains("forward success") || line.contains("Entering interactive session")
}

/// The line that best explains why ssh exited.
pub fn error_line(lines: &[String]) -> Option<String> {
    let useful: Vec<&String> =
        // Skip debug output and the version banner; prefer a line naming a known cause.
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
    /// Current state, updated by the background task.
    state: Arc<Mutex<ForwardState>>,
    /// Signalled to ask the task to kill ssh.
    stop: Arc<Notify>,
}

impl ForwardHandle {
    /// Spawns `program args` in the background on `rt` and tracks its state
    /// from its stderr and exit; the process is tied to skiff's lifetime.
    pub fn start(rt: &tokio::runtime::Handle, program: &str, args: Vec<String>, env: Vec<(String, String)>) -> ForwardHandle {
        let state = Arc::new(Mutex::new(ForwardState::Starting));
        let stop = Arc::new(Notify::new());
        let mut cmd = Command::new(program);
        cmd.args(&args).envs(env).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        tie_to_parent(&mut cmd);
        let (st, stop2, program) = (state.clone(), stop.clone(), program.to_string());
        rt.spawn(async move {
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    *st.lock().unwrap() = ForwardState::Failed(format!("starting `{program}`: {e}"));
                    return;
                }
            };
            adopt(&child);
            // Read stderr: flip to Running on an "up" marker, and keep the last 200 lines.
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
            // Wait for exit; meanwhile assume Running after the fallback delay, and kill on stop.
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
            // Give the stderr reader a moment to collect the final lines.
            if let Some(r) = reader {
                let _ = tokio::time::timeout(Duration::from_secs(1), r).await;
            }
            // A requested stop is Stopped; any other exit is a failure with ssh's reason.
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

    /// The forward's current state.
    pub fn state(&self) -> ForwardState {
        self.state.lock().unwrap().clone()
    }

    /// Asks the background task to kill ssh; the state becomes Stopped.
    pub fn stop(&self) {
        self.stop.notify_one();
    }
}

impl Drop for ForwardHandle {
    /// Stops ssh when the handle goes away.
    fn drop(&mut self) {
        self.stop();
    }
}

// A forward's ssh must not outlive skiff, even when skiff is killed
// (dropping the handle covers a normal exit).

/// Linux: the child gets SIGTERM when skiff dies.
#[cfg(target_os = "linux")]
fn tie_to_parent(cmd: &mut Command) {
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: only async-signal-safe calls (prctl, getppid) run in the child.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // skiff died before the prctl took effect.
            if libc::getppid() != parent {
                return Err(std::io::Error::other("Skiff exited"));
            }
            Ok(())
        });
    }
}

/// Other systems: handled after the spawn (Windows), or best effort (macOS
/// and the BSDs have no parent-death signal; ssh exits on its own once its
/// stderr pipe is gone and it next writes to it).
#[cfg(not(target_os = "linux"))]
fn tie_to_parent(_cmd: &mut Command) {}

/// Windows: puts the child in a job object that kills its processes when
/// the job's last handle closes, which happens when skiff exits in any way.
#[cfg(windows)]
fn adopt(child: &tokio::process::Child) {
    if let (Some(job), Some(h)) = (job::get(), child.raw_handle()) {
        // Best effort: a failure leaves the old behavior (stopped on drop).
        // SAFETY: both handles are valid for the duration of the call.
        unsafe { windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(job, h as _) };
    }
}

/// Non-Windows: nothing to do after spawning.
#[cfg(not(windows))]
fn adopt(_child: &tokio::process::Child) {}

/// The process-wide kill-on-close job object.
#[cfg(windows)]
mod job {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// An owned job handle, stored in a static.
    struct Job(HANDLE);
    // SAFETY: a job handle can be used from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    /// The job, created on first use (`None` if creation failed).
    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    /// skiff's job object, created once and never closed (the system
    /// closes it, and so kills the forwards, when skiff exits).
    pub fn get() -> Option<HANDLE> {
        JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls; the handle is checked before use.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(h);
                    return None;
                }
                Some(Job(h))
            }
        })
        .as_ref()
        .map(|j| j.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A forward to host `web` on port 8080.
    fn fwd(kind: ForwardKind, target: Option<&str>) -> Forward {
        Forward { name: None, host: "web".into(), kind, bind: None, listen: 8080, target: target.map(Into::into) }
    }

    /// ssh spec arguments, titles, and validation messages for each kind.
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

    /// Forwards survive saving and reloading config.toml, including removals.
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

    /// On Windows, a forward's child process is placed in the kill-on-close job.
    #[cfg(windows)]
    #[tokio::test]
    async fn forward_ssh_is_killed_with_skiff() {
        // A stand-in for ssh that waits; ping only talks to this machine.
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null()).kill_on_drop(true);
        let child = cmd.spawn().unwrap();
        adopt(&child);
        let mut inside = 0;
        let h = child.raw_handle().unwrap();
        // SAFETY: valid handles; `inside` is written by the call.
        unsafe { windows_sys::Win32::System::JobObjects::IsProcessInJob(h as _, job::get().unwrap(), &mut inside) };
        assert_ne!(inside, 0, "the child is in skiff's kill-on-close job");
    }

    /// The cause line is picked over ssh's generic follow-up and debug noise.
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
