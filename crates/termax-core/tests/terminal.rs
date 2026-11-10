//! Terminal session hooks the GUI relies on. Uses the local test server only.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use termax_core::sshver::{client_version, parse_version, supports_askpass_require};
use termax_core::terminal::TerminalSession;
use termax_testserver::{FixtureAuth, TestFixture};

#[test]
fn notify_fires_on_output() {
    let fx = TestFixture::with_shell(FixtureAuth::Key).unwrap();
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let mut s = TerminalSession::spawn("ssh", &args, 24, 80, 100).unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    s.set_notify(Arc::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
    }));
    let g0 = s.generation();
    let start = Instant::now();
    s.write(b"hi\r");
    while n.load(Ordering::SeqCst) == 0 {
        assert!(start.elapsed() < Duration::from_secs(15), "no output notification");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(s.generation() > g0);
}

#[test]
fn parses_ssh_version_strings() {
    assert_eq!(parse_version("OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2"), Ok((9, 5)));
    assert_eq!(parse_version("OpenSSH_8.3p1 Ubuntu-1, OpenSSL 1.1.1f"), Ok((8, 3)));
    assert!(!supports_askpass_require((8, 3)));
    assert!(supports_askpass_require((8, 4)));
    assert!(supports_askpass_require((10, 0)));
    assert!(parse_version("garbage").is_err());
}

#[test]
fn local_ssh_client_version_is_readable() {
    // Runs `ssh -V` only (no connection).
    let v = client_version("ssh").expect("ssh -V");
    assert!(v.0 >= 7, "{v:?}");
}

#[test]
fn paste_cannot_end_bracketed_paste_early() {
    let evil = "ls\x1b[201~rm -rf ~\n\x1b[200~x";
    let clean = termax_core::terminal::sanitize_paste(evil);
    assert!(!clean.contains("\x1b[201~") && !clean.contains("\x1b[200~"), "{clean:?}");
    assert_eq!(clean, "lsrm -rf ~\rx");
}
