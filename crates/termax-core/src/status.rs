//! Opt-in reachability checks: TCP connect, then read the server's first
//! line and expect an `SSH-` banner. Never authenticates and never runs
//! ssh: a host behind a jump host is reported by checking the jump host.

use std::future::Future;
use std::net::SocketAddr;

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
    /// The host is reached through jump host `.0`; `.1` is that jump
    /// host's own status (the host behind it isn't checked).
    Via(String, Box<Status>),
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
            Status::Via(jump, s) => match &**s {
                Status::Online { ms } => format!("via {jump}: reachable, {ms} ms"),
                s => format!("via {jump}: {}", s.label()),
            },
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

/// Tries `addrs` in order until one connects, all within `budget`: each
/// address gets an equal share of what is left, so one address that never
/// answers can't use up the whole check. Returns the stream and how long
/// the successful connect took (name lookup and failed tries not counted).
async fn connect_any<S, F, Fut>(addrs: Vec<SocketAddr>, budget: Duration, mut connect: F) -> Result<(S, Duration), String>
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = std::io::Result<S>>,
{
    let deadline = Instant::now() + budget;
    let mut last = "name not found".to_string();
    let n = addrs.len();
    for (i, a) in addrs.into_iter().enumerate() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            last = "timed out".into();
            break;
        }
        let share = left / (n - i) as u32;
        let start = Instant::now();
        match tokio::time::timeout(share, connect(a)).await {
            Ok(Ok(s)) => return Ok((s, start.elapsed())),
            Ok(Err(e)) => last = reason(&e),
            Err(_) => last = "timed out".into(),
        }
    }
    Err(last)
}

/// Checks one host (or, for a host behind a jump host, the jump host).
pub async fn check(probe: Probe, timeout: Duration) -> Status {
    let (host, port, via) = match probe {
        Probe::Unknown(r) => return Status::Unknown(r),
        Probe::Direct { host, port } => (host, port, None),
        Probe::Via { jump, host, port } => (host, port, Some(jump)),
    };
    let status = check_direct(&host, port, timeout).await;
    match via {
        Some(jump) => Status::Via(jump, Box::new(status)),
        None => status,
    }
}

async fn check_direct(host: &str, port: u16, timeout: Duration) -> Status {
    let start = Instant::now();
    let addrs: Vec<SocketAddr> = match tokio::time::timeout(timeout, tokio::net::lookup_host((host, port))).await {
        Err(_) => return Status::Offline("name lookup timed out".into()),
        Ok(Err(_)) => return Status::Offline("name not found".into()),
        Ok(Ok(a)) => a.collect(),
    };
    let left = timeout.saturating_sub(start.elapsed());
    let (mut stream, took) = match connect_any(addrs, left, TcpStream::connect).await {
        Err(r) => return Status::Offline(r),
        Ok(s) => s,
    };
    let ms = took.as_millis().min(u32::MAX as u128) as u32;
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
    async fn dead_address_gets_only_its_share() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let live = l.local_addr().unwrap();
        let dead: SocketAddr = "127.0.0.2:9".parse().unwrap();
        let start = Instant::now();
        // The first address never answers (a blackholed route).
        let res = connect_any(vec![dead, live], Duration::from_secs(2), |a| async move {
            if a == dead {
                std::future::pending::<()>().await;
            }
            TcpStream::connect(a).await
        })
        .await;
        let (_s, took) = res.unwrap();
        assert!(start.elapsed() < Duration::from_millis(1900), "{:?}", start.elapsed());
        assert!(took < Duration::from_millis(500), "only the good connect is timed: {took:?}");
    }

    #[tokio::test]
    async fn via_checks_the_jump_host() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut c, _) = l.accept().await.unwrap();
            let _ = tokio::io::AsyncWriteExt::write_all(&mut c, b"SSH-2.0-jump\r\n").await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let via = Probe::Via { jump: "bastion".into(), host: "127.0.0.1".into(), port };
        let s = check(via, T).await;
        assert!(matches!(&s, Status::Via(j, inner) if j == "bastion" && matches!(**inner, Status::Online { .. })), "{s:?}");
        assert!(s.label().starts_with("via bastion: reachable, "), "{}", s.label());
    }

    #[tokio::test]
    async fn unknown_is_not_probed() {
        let start = std::time::Instant::now();
        assert_eq!(check(Probe::Unknown("via proxy".into()), T).await, Status::Unknown("via proxy".into()));
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
