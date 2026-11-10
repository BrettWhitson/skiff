//! Port forwarding through the system `ssh` to the in-process test server on
//! 127.0.0.1 — never a real host.

use std::time::{Duration, Instant};

use skiff_core::config::{Config, Connection};
use skiff_core::forward::{Forward, ForwardHandle, ForwardKind, ForwardState};
use skiff_core::hosts::{HostEntry, collect};
use skiff_testserver::{FixtureAuth, TestFixture};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn host(fx: &TestFixture) -> HostEntry {
    let mut cfg = Config::default();
    cfg.settings.load_ssh_config = false;
    cfg.connections = vec![Connection { name: "fx".into(), host: fx.destination(), extra_args: fx.ssh_options(), ..Default::default() }];
    collect(&cfg).0.remove(0)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

async fn wait(h: &ForwardHandle, secs: u64, done: impl Fn(&ForwardState) -> bool) -> ForwardState {
    let start = Instant::now();
    loop {
        let s = h.state();
        if done(&s) || start.elapsed() > Duration::from_secs(secs) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn local(listen: u16, target: String) -> Forward {
    Forward { name: None, host: "fx".into(), kind: ForwardKind::Local, bind: None, listen, target: Some(target) }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_forward_reaches_echo() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = echo.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 64];
                while let Ok(n) = c.read(&mut buf).await {
                    if n == 0 || c.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let listen = free_port();
    let f = local(listen, format!("127.0.0.1:{echo_port}"));
    let h = ForwardHandle::start(&tokio::runtime::Handle::current(), "ssh", f.background_args(&host(&fx)), vec![]);
    let s = wait(&h, 20, |s| *s != ForwardState::Starting).await;
    assert_eq!(s, ForwardState::Running);

    let mut c = tokio::net::TcpStream::connect(("127.0.0.1", listen)).await.unwrap();
    c.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(10), c.read_exact(&mut buf)).await.unwrap().unwrap();
    assert_eq!(&buf, b"ping");

    h.stop();
    assert_eq!(wait(&h, 5, |s| *s == ForwardState::Stopped).await, ForwardState::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
async fn forward_port_in_use_fails() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = held.local_addr().unwrap().port();
    let f = local(listen, "127.0.0.1:9".into());
    let h = ForwardHandle::start(&tokio::runtime::Handle::current(), "ssh", f.background_args(&host(&fx)), vec![]);
    let s = wait(&h, 20, |s| matches!(s, ForwardState::Failed(_))).await;
    match s {
        ForwardState::Failed(m) => assert!(m.contains(&listen.to_string()), "{m}"),
        other => panic!("{other:?}"),
    }
}
