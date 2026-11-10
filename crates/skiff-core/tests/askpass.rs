//! Askpass tests: ssh asks skiff for passwords / host-key confirmation.
//! Only ever against the in-process test server on 127.0.0.1.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use skiff_core::askpass::{AskKind, Broker};
use skiff_core::fs::connect::{SshLaunch, connect_sftp};
use skiff_testserver::{FixtureAuth, TestFixture};

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_askpass-shim"))
}

/// Starts a broker whose requests are answered by `answer(kind, prompt)`.
/// Returns the broker and a log of (kind, prompt) seen.
type AskLog = Arc<Mutex<Vec<(AskKind, String)>>>;

fn broker(answer: impl Fn(AskKind, &str) -> Option<String> + Send + 'static) -> (Broker, AskLog) {
    let (broker, mut rx) = Broker::start(&tokio::runtime::Handle::current(), helper()).unwrap();
    let log = Arc::new(Mutex::new(vec![]));
    let log2 = log.clone();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            log2.lock().unwrap().push((req.kind, req.prompt.clone()));
            let _ = req.reply.send(answer(req.kind, &req.prompt));
        }
    });
    (broker, log)
}

fn launch(pre: Vec<String>, fx: &TestFixture, broker: &Broker, tag: &str) -> SshLaunch {
    let mut args = pre;
    args.extend(fx.ssh_options());
    args.extend(["-s".into(), fx.destination(), "sftp".into()]);
    SshLaunch { program: "ssh".into(), args, env: broker.env(tag), start_dir: None }
}

#[tokio::test(flavor = "multi_thread")]
async fn password_prompt_roundtrip() {
    let fx = TestFixture::new(FixtureAuth::Password("s3cret".into())).unwrap();
    let (b, log) = broker(|kind, prompt| {
        assert_eq!(kind, AskKind::Secret);
        assert!(prompt.to_lowercase().contains("password"), "{prompt}");
        Some("s3cret".into())
    });
    let conn = connect_sftp(launch(vec![], &fx, &b, "t1")).await.expect("connect with askpass");
    assert_eq!(conn.home, "/");
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn askpass_cancel_fails_fast() {
    let fx = TestFixture::new(FixtureAuth::Password("s3cret".into())).unwrap();
    let (b, _log) = broker(|_, _| None);
    let started = Instant::now();
    let res = connect_sftp(launch(vec![], &fx, &b, "t2")).await;
    assert!(res.is_err());
    assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_password_fails() {
    let fx = TestFixture::new(FixtureAuth::Password("s3cret".into())).unwrap();
    let (b, log) = broker(|_, _| Some("nope".into()));
    let res = connect_sftp(launch(vec![], &fx, &b, "t3")).await;
    let err = res.err().expect("must fail");
    assert!(err.contains("Permission denied"), "{err}");
    assert!(!log.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn host_key_confirm() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let empty = tempfile::tempdir().unwrap();
    let kh = empty.path().join("known_hosts");
    std::fs::write(&kh, "").unwrap();
    // First occurrence of an option wins in ssh, so these override the fixture's.
    let pre = vec!["-o".into(), "StrictHostKeyChecking=ask".into(), "-o".into(), format!("UserKnownHostsFile={}", kh.display())];
    let (b, log) = broker(|kind, _| {
        assert_eq!(kind, AskKind::Confirm);
        Some("yes".into())
    });
    connect_sftp(launch(pre, &fx, &b, "t4")).await.expect("connect after confirming host key");
    let seen = log.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].1.contains("(yes/no"), "{}", seen[0].1);
    assert!(std::fs::read_to_string(&kh).unwrap().contains("ssh-ed25519"));
}

#[test]
fn env_names_helper_and_endpoint() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (b, _rx) = Broker::start(rt.handle(), helper()).unwrap();
    let env: std::collections::HashMap<_, _> = b.env("tagX").into_iter().collect();
    assert_eq!(env["SSH_ASKPASS"], helper().display().to_string());
    assert_eq!(env["SSH_ASKPASS_REQUIRE"], "force");
    assert_eq!(env["SKIFF_ASKPASS_TAG"], "tagX");
    assert!(!env["SKIFF_ASKPASS_ENDPOINT"].is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_answer_still_connects() {
    // A person may take a while to type a password; the handshake must wait.
    let fx = TestFixture::new(FixtureAuth::Password("s3cret".into())).unwrap();
    let (b, _log) = broker(|_, _| {
        std::thread::sleep(Duration::from_secs(12));
        Some("s3cret".into())
    });
    let conn = connect_sftp(launch(vec![], &fx, &b, "slow")).await.expect("connect after a slow answer");
    assert_eq!(conn.home, "/");
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_token_is_refused() {
    let (b, mut rx) = Broker::start(&tokio::runtime::Handle::current(), helper()).unwrap();
    let mut cmd = std::process::Command::new(helper());
    for (k, v) in b.env("t") {
        cmd.env(k, v);
    }
    cmd.env(skiff_core::askpass::ENV_TOKEN, "not-the-token").arg("password: ");
    let out = tokio::task::spawn_blocking(move || cmd.output()).await.unwrap().unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(rx.try_recv().is_err(), "a request with a bad token must not reach the front-end");
}

#[tokio::test(flavor = "multi_thread")]
async fn retire_cancels_pending_prompt() {
    let fx = TestFixture::new(FixtureAuth::Password("s3cret".into())).unwrap();
    let (b, mut rx) = Broker::start(&tokio::runtime::Handle::current(), helper()).unwrap();
    let b = Arc::new(b);
    let b2 = b.clone();
    let watcher = tokio::spawn(async move {
        // Never answer; retire the tag instead (e.g. the user closed the tab).
        let req = rx.recv().await.unwrap();
        b2.retire("t9");
        tokio::time::timeout(Duration::from_secs(5), req.cancelled.cancelled()).await.expect("request marked cancelled");
        drop(req);
        // ssh may ask again; retired tags are refused at once.
        while let Ok(Some(r)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            assert!(r.cancelled.is_cancelled());
        }
    });
    let started = Instant::now();
    let res = connect_sftp(launch(vec![], &fx, &b, "t9")).await;
    assert!(res.is_err());
    assert!(started.elapsed() < Duration::from_secs(15), "took {:?}", started.elapsed());
    watcher.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn touch_notice_is_not_a_prompt_and_closes_with_the_helper() {
    let (b, mut rx) = Broker::start(&tokio::runtime::Handle::current(), helper()).unwrap();
    let spawn = |hint: &str| {
        let mut cmd = std::process::Command::new(helper());
        cmd.envs(b.env("t")).env("SSH_ASKPASS_PROMPT", hint).arg("Confirm user presence for key ED25519-SK");
        cmd.spawn().unwrap()
    };
    let next = async |rx: &mut tokio::sync::mpsc::UnboundedReceiver<_>| {
        tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap()
    };
    let mut confirm = spawn("confirm");
    let req: skiff_core::askpass::AskRequest = next(&mut rx).await;
    assert_eq!(req.kind, AskKind::Confirm);
    let _ = req.reply.send(Some("yes".into()));
    assert!(tokio::task::spawn_blocking(move || confirm.wait()).await.unwrap().unwrap().success());

    // ssh kills the helper once the key is touched: the notice goes away.
    let mut notice = spawn("none");
    let req: skiff_core::askpass::AskRequest = next(&mut rx).await;
    assert_eq!(req.kind, AskKind::Notice);
    notice.kill().unwrap();
    let _ = notice.wait();
    tokio::time::timeout(Duration::from_secs(5), req.cancelled.cancelled()).await.expect("notice withdrawn when the helper exits");
}
