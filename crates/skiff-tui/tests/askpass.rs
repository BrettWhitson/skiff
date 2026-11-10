//! The `skiff` binary is ssh's askpass helper for SFTP connections: a
//! password asked by ssh reaches the broker and the answer gets back to
//! ssh. Only against the local test server on 127.0.0.1.

use skiff_core::askpass::{AskKind, Broker};
use skiff_core::fs::connect::{SshLaunch, connect_sftp};
use skiff_testserver::{FixtureAuth, TestFixture};

#[tokio::test(flavor = "multi_thread")]
async fn skiff_binary_relays_password_prompts() {
    let fx = TestFixture::new(FixtureAuth::Password("s3cret".into())).unwrap();
    let helper = std::path::PathBuf::from(env!("CARGO_BIN_EXE_skiff"));
    let (broker, mut rx) = Broker::start(&tokio::runtime::Handle::current(), helper).unwrap();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            assert_eq!(req.kind, AskKind::Secret, "{}", req.prompt);
            let _ = req.reply.send(Some("s3cret".into()));
        }
    });
    let mut args = fx.ssh_options();
    args.extend(["-s".into(), fx.destination(), "sftp".into()]);
    let launch = SshLaunch { program: "ssh".into(), args, env: broker.env("sftp:1:1"), start_dir: None };
    let conn = connect_sftp(launch).await.expect("connect with the password relayed by skiff");
    assert_eq!(conn.home, "/");
}
