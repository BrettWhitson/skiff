//! These tests only ever talk to the in-process server on 127.0.0.1.

use std::process::Stdio;

use russh_sftp::client::SftpSession;
use termax_testserver::{FixtureAuth, TestFixture};
use tokio::process::Command;

fn sftp_command(fx: &TestFixture, batch: bool) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(fx.ssh_options());
    if batch {
        cmd.args(["-o", "BatchMode=yes"]);
    }
    cmd.args(["-s", &fx.destination(), "sftp"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    cmd
}

async fn open_sftp(fx: &TestFixture) -> (SftpSession, tokio::process::Child) {
    let mut child = sftp_command(fx, true).spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let sftp = SftpSession::new(tokio::io::join(stdout, stdin)).await.expect("sftp handshake");
    (sftp, child)
}

#[test]
fn ssh_options_never_read_user_config() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let opts = fx.ssh_options();
    assert_eq!(opts[0], "-F");
    assert!(opts.iter().any(|o| o == "IdentityAgent=none"));
    assert_eq!(fx.destination(), "127.0.0.1");
}

#[tokio::test]
async fn sftp_lists_seeded_file_with_key_auth() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::write(fx.root().join("hello.txt"), "hello").unwrap();
    let (sftp, _child) = open_sftp(&fx).await;
    let entries: Vec<_> = sftp.read_dir("/").await.unwrap().collect();
    let hello = entries.iter().find(|e| e.file_name() == "hello.txt").expect("hello.txt listed");
    assert_eq!(hello.metadata().len(), 5);
}

#[tokio::test]
async fn sftp_read_write_roundtrip() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let (sftp, _child) = open_sftp(&fx).await;
    sftp.create_dir("/d").await.unwrap();
    // `SftpSession::write` opens without CREATE, so create the file first.
    let mut f = sftp.create("/d/f.bin").await.unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut f, &[7u8; 70_000]).await.unwrap();
    tokio::io::AsyncWriteExt::shutdown(&mut f).await.unwrap();
    drop(f);
    assert_eq!(sftp.read("/d/f.bin").await.unwrap(), vec![7u8; 70_000]);
    assert_eq!(std::fs::read(fx.root().join("d").join("f.bin")).unwrap().len(), 70_000);
}

#[tokio::test]
async fn wrong_key_is_rejected() {
    let fx = TestFixture::new(FixtureAuth::Password("pw".into())).unwrap();
    let out = sftp_command(&fx, true).output().await.unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Permission denied"), "{stderr}");
}

#[tokio::test]
async fn path_escape_denied() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::write(fx.root().parent().unwrap().join("outside.txt"), "secret").unwrap();
    let (sftp, _child) = open_sftp(&fx).await;
    assert!(sftp.read("/../outside.txt").await.is_err());
    let names: Vec<String> = match sftp.read_dir("/../").await {
        Ok(rd) => rd.map(|e| e.file_name()).collect(),
        Err(_) => vec![],
    };
    assert!(!names.contains(&"outside.txt".to_string()), "{names:?}");
}
