//! Opt-in reachability checks: TCP connect, then read the server's first
//! line and expect an `SSH-` banner. Never authenticates, never goes
//! through a proxy.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

pub use crate::hosts::Probe;

/// Per-check time limit.
pub const TIMEOUT: Duration = Duration::from_secs(3);
/// Checks running at once.
pub const MAX_CHECKS: usize = 16;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Status {
    #[default]
    Unchecked,
    Checking,
    Online {
        ms: u32,
    },
    /// Something answered, but not an SSH server.
    NotSsh,
    Offline(String),
    Unknown(String),
}

impl Status {
    /// Plain words, for tooltips, screen readers and the TUI.
    pub fn label(&self) -> String {
        match self {
            Status::Unchecked => "not checked".into(),
            Status::Checking => "checking".into(),
            Status::Online { ms } => format!("online, {ms} ms"),
            Status::NotSsh => "not SSH".into(),
            Status::Offline(r) => format!("offline: {r}"),
            Status::Unknown(r) => format!("unknown: {r}"),
        }
    }
}

fn reason(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused => "connection refused".into(),
        TimedOut => "timed out".into(),
        HostUnreachable => "host unreachable".into(),
        NetworkUnreachable => "network unreachable".into(),
        _ => e.to_string(),
    }
}

/// Checks one host.
pub async fn check(probe: Probe, timeout: Duration) -> Status {
    let (host, port) = match probe {
        Probe::Unknown(r) => return Status::Unknown(r),
        Probe::Direct { host, port } => (host, port),
    };
    let start = Instant::now();
    let connect = async {
        let addrs: Vec<_> = match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(a) => a.collect(),
            Err(_) => return Err("name not found".to_string()),
        };
        let mut last = "name not found".to_string();
        for a in addrs {
            match TcpStream::connect(a).await {
                Ok(s) => return Ok(s),
                Err(e) => last = reason(&e),
            }
        }
        Err(last)
    };
    let mut stream = match tokio::time::timeout(timeout, connect).await {
        Err(_) => return Status::Offline("timed out".into()),
        Ok(Err(r)) => return Status::Offline(r),
        Ok(Ok(s)) => s,
    };
    let ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;
    // The first line must start with `SSH-`; decide as soon as 4 bytes are in.
    let banner = async {
        let mut buf = [0u8; 255];
        let mut n = 0;
        while n < 4 {
            match stream.read(&mut buf[n..]).await {
                Ok(0) | Err(_) => break,
                Ok(k) => n += k,
            }
        }
        buf[..n].starts_with(b"SSH-")
    };
    let left = timeout.saturating_sub(start.elapsed());
    match tokio::time::timeout(left, banner).await {
        Ok(true) => Status::Online { ms },
        _ => Status::NotSsh,
    }
}

/// Checks every host, [`MAX_CHECKS`] at a time; `done` gets each result.
/// `Unknown` probes report at once, without any I/O.
pub fn check_all(rt: &tokio::runtime::Handle, items: Vec<(String, Probe)>, done: Arc<dyn Fn(String, Status) + Send + Sync>) {
    let sem = Arc::new(Semaphore::new(MAX_CHECKS));
    for (name, probe) in items {
        if let Probe::Unknown(r) = probe {
            done(name, Status::Unknown(r));
            continue;
        }
        let (sem, done) = (sem.clone(), done.clone());
        rt.spawn(async move {
            let _permit = sem.acquire_owned().await;
            let s = check(probe, TIMEOUT).await;
            done(name, s);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termax_testserver::{FixtureAuth, TestFixture};

    const T: Duration = Duration::from_secs(3);

    fn local(port: u16) -> Probe {
        Probe::Direct { host: "127.0.0.1".into(), port }
    }

    #[tokio::test]
    async fn online_against_fixture() {
        let fx = TestFixture::new(FixtureAuth::Key).unwrap();
        let s = check(local(fx.port()), T).await;
        assert!(matches!(s, Status::Online { .. }), "{s:?}");
        assert!(s.label().starts_with("online, "));
    }

    #[tokio::test]
    async fn offline_on_closed_port() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let s = check(local(port), T).await;
        assert_eq!(s, Status::Offline("connection refused".into()));
        assert_eq!(s.label(), "offline: connection refused");
    }

    #[tokio::test]
    async fn not_ssh_when_no_banner() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut c, _) = l.accept().await.unwrap();
            let _ = tokio::io::AsyncWriteExt::write_all(&mut c, b"HTTP/1.1 200 OK\r\n").await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        assert_eq!(check(local(port), T).await, Status::NotSsh);
    }

    #[tokio::test]
    async fn unknown_is_not_probed() {
        let start = std::time::Instant::now();
        assert_eq!(check(Probe::Unknown("via proxy".into()), T).await, Status::Unknown("via proxy".into()));
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
