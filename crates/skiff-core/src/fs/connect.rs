//! Starting SFTP sessions over the system `ssh` client (`ssh … -s host sftp`),
//! so OpenSSH handles auth, ProxyJump, agents and known_hosts.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh_sftp::client::SftpSession;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Effectively unlimited: if ssh gives up it exits, which ends the handshake.
const HANDSHAKE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
/// Per-request timeout once the session is up.
const REQUEST_TIMEOUT_SECS: u64 = 10;

/// How to start one `ssh` process.
#[derive(Debug, Clone, Default)]
pub struct SshLaunch {
    /// The ssh executable.
    pub program: String,
    /// Arguments to ssh, including the SFTP subsystem request (`-s host sftp`).
    pub args: Vec<String>,
    /// Extra environment variables (e.g. for askpass).
    pub env: Vec<(String, String)>,
    /// Directory to resolve as the session's starting point (default: login dir).
    pub start_dir: Option<String>,
}

/// A live SFTP session. The `ssh` process is killed when this is dropped.
pub struct RemoteConn {
    /// The SFTP client, shared by tasks that use the connection.
    pub sftp: Arc<SftpSession>,
    /// Absolute path of the starting directory.
    pub home: String,
    /// The ssh process, held so it lives (and is killed) with the connection.
    _child: Mutex<Child>,
}

/// Spawns ssh and performs the SFTP handshake. On failure the error is ssh's
/// own stderr when it printed anything (e.g. `Permission denied`).
pub async fn connect_sftp(launch: SshLaunch) -> Result<RemoteConn, String> {
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args)
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: keep ssh away from any console.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("starting `{}`: {e}", launch.program))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Keep ssh's last 20 stderr lines to explain a failure.
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let log2 = log.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut l = log2.lock().unwrap();
            l.push(line);
            if l.len() > 20 {
                l.remove(0);
            }
        }
    });

    let result = async {
        // The SFTP INIT reply only arrives after ssh has authenticated, which can
        // include a person typing a password or checking a host key. Don't time
        // that out; use the normal per-request timeout afterwards.
        let cfg = russh_sftp::client::Config { request_timeout_secs: HANDSHAKE_TIMEOUT_SECS, ..Default::default() };
        let sftp = SftpSession::new_with_config(tokio::io::join(stdout, stdin), cfg).await?;
        sftp.set_timeout(REQUEST_TIMEOUT_SECS);
        let home = sftp.canonicalize(launch.start_dir.clone().unwrap_or_else(|| ".".into())).await?;
        Ok::<_, russh_sftp::client::error::Error>((sftp, home))
    }
    .await;

    match result {
        Ok((sftp, home)) => Ok(RemoteConn { sftp: Arc::new(sftp), home, _child: Mutex::new(child) }),
        Err(e) => {
            // Let ssh finish writing its explanation.
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            let lines = log.lock().unwrap().join("\n");
            Err(if lines.trim().is_empty() { e.to_string() } else { lines })
        }
    }
}
