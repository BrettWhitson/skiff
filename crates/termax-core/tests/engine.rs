//! Engine tests. Every remote endpoint is the in-process test server on
//! 127.0.0.1 — never a real host.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use termax_core::fs::connect::{connect_sftp, SshLaunch};
use termax_core::fs::endpoint::Endpoint;
use termax_core::fs::transfer::{self, OverwriteDecision, OverwritePolicy, TransferEvent, TransferRequest, TransferSummary};
use termax_testserver::{FixtureAuth, TestFixture};
use tokio::sync::mpsc::unbounded_channel;

fn launch(fx: &TestFixture, batch: bool) -> SshLaunch {
    let mut args = fx.ssh_options();
    if batch {
        args.extend(["-o".into(), "BatchMode=yes".into()]);
    }
    args.extend(["-s".into(), fx.destination(), "sftp".into()]);
    SshLaunch { program: "ssh".into(), args, env: vec![], start_dir: None }
}

async fn remote(fx: &TestFixture) -> Endpoint {
    Endpoint::Remote(Arc::new(connect_sftp(launch(fx, true)).await.expect("connect")))
}

fn data(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2654435761).max(1);
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// Runs a transfer to completion. `decide` answers overwrite questions;
/// `on_progress` may cancel or kill things once progress starts.
async fn run_transfer(
    req: TransferRequest,
    policy: OverwritePolicy,
    mut decide: impl FnMut(&str) -> OverwriteDecision,
    mut on_first_progress: impl FnMut(&transfer::TransferHandle),
) -> (Result<TransferSummary, String>, usize) {
    let (tx, mut rx) = unbounded_channel();
    let handle = transfer::start(&tokio::runtime::Handle::current(), req, policy, tx);
    let mut asked = 0;
    let mut seen_progress = false;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await.expect("transfer stalled").expect("channel open");
        match ev {
            TransferEvent::Progress { .. } => {
                if !seen_progress {
                    seen_progress = true;
                    on_first_progress(&handle);
                }
            }
            TransferEvent::NeedsDecision { path, reply, .. } => {
                asked += 1;
                let _ = reply.send(decide(&path));
            }
            TransferEvent::Done { result, .. } => return (result, asked),
        }
    }
}

fn req(src: &Endpoint, src_path: &str, dst: &Endpoint, dst_dir: &str) -> TransferRequest {
    TransferRequest { id: 1, src: src.clone(), src_path: src_path.into(), dst: dst.clone(), dst_dir: dst_dir.into() }
}

fn p(path: &Path) -> String {
    path.display().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_download_roundtrip() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    let local_a = tempfile::tempdir().unwrap();
    let local_b = tempfile::tempdir().unwrap();
    let bytes = data(3 * 1024 * 1024, 1);
    std::fs::write(local_a.path().join("big.bin"), &bytes).unwrap();

    let (res, _) = run_transfer(req(&Endpoint::Local, &p(&local_a.path().join("big.bin")), &r, "/"), OverwritePolicy::Ask, |_| panic!("no conflict"), |_| {}).await;
    assert_eq!(res.unwrap().files, 1);
    assert_eq!(std::fs::read(fx.root().join("big.bin")).unwrap(), bytes);

    let (res, _) = run_transfer(req(&r, "/big.bin", &Endpoint::Local, &p(local_b.path())), OverwritePolicy::Ask, |_| panic!("no conflict"), |_| {}).await;
    assert_eq!(res.unwrap().bytes, bytes.len() as u64);
    assert_eq!(std::fs::read(local_b.path().join("big.bin")).unwrap(), bytes);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_to_remote_relay() {
    let fa = TestFixture::new(FixtureAuth::Key).unwrap();
    let fb = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::create_dir_all(fa.root().join("a").join("sub")).unwrap();
    let x = data(1024 * 1024, 2);
    std::fs::write(fa.root().join("a").join("x.bin"), &x).unwrap();
    std::fs::write(fa.root().join("a").join("sub").join("y.txt"), "why").unwrap();
    let (ra, rb) = (remote(&fa).await, remote(&fb).await);

    let (res, _) = run_transfer(req(&ra, "/a", &rb, "/"), OverwritePolicy::Ask, |_| panic!("no conflict"), |_| {}).await;
    assert_eq!(res.unwrap().files, 2);
    assert_eq!(std::fs::read(fb.root().join("a").join("x.bin")).unwrap(), x);
    assert_eq!(std::fs::read_to_string(fb.root().join("a").join("sub").join("y.txt")).unwrap(), "why");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_mid_transfer() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("huge.bin"), data(40 * 1024 * 1024, 3)).unwrap();

    let (res, _) = run_transfer(req(&Endpoint::Local, &p(&local.path().join("huge.bin")), &r, "/"), OverwritePolicy::Ask, |_| panic!(), |h| h.cancel()).await;
    assert!(res.unwrap().cancelled);
    assert!(!fx.root().join("huge.bin").exists(), "partial file left behind");
}

#[tokio::test(flavor = "multi_thread")]
async fn overwrite_ask_skip_and_all() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    std::fs::create_dir(fx.root().join("d")).unwrap();
    std::fs::write(fx.root().join("d").join("f1"), "old1").unwrap();
    std::fs::write(fx.root().join("d").join("f2"), "old2").unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir(local.path().join("d")).unwrap();
    std::fs::write(local.path().join("d").join("f1"), "new1").unwrap();
    std::fs::write(local.path().join("d").join("f2"), "new2").unwrap();

    let mut answers = vec![OverwriteDecision::OverwriteAll, OverwriteDecision::Skip];
    let mut asked_paths = vec![];
    let (res, asked) = run_transfer(
        req(&Endpoint::Local, &p(&local.path().join("d")), &r, "/"),
        OverwritePolicy::Ask,
        |path| {
            asked_paths.push(path.to_string());
            answers.pop().unwrap()
        },
        |_| {},
    )
    .await;
    let summary = res.unwrap();
    assert_eq!(asked, 2, "{asked_paths:?}");
    assert_eq!((summary.files, summary.skipped), (1, 1));
    let contents: Vec<String> = ["f1", "f2"].iter().map(|f| std::fs::read_to_string(fx.root().join("d").join(f)).unwrap()).collect();
    // The first file asked about is skipped; OverwriteAll applies to the second.
    let first = Path::new(&asked_paths[0]).file_name().unwrap().to_string_lossy().to_string();
    for (name, c) in ["f1", "f2"].iter().zip(&contents) {
        let expected = if *name == first { format!("old{}", &name[1..]) } else { format!("new{}", &name[1..]) };
        assert_eq!(c, &expected);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn overwrite_all_stops_asking() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    std::fs::create_dir(fx.root().join("d")).unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir(local.path().join("d")).unwrap();
    for f in ["a", "b", "c"] {
        std::fs::write(fx.root().join("d").join(f), "old").unwrap();
        std::fs::write(local.path().join("d").join(f), "new").unwrap();
    }
    let (res, asked) = run_transfer(req(&Endpoint::Local, &p(&local.path().join("d")), &r, "/"), OverwritePolicy::Ask, |_| OverwriteDecision::OverwriteAll, |_| {}).await;
    assert_eq!(res.unwrap().files, 3);
    assert_eq!(asked, 1);
    for f in ["a", "b", "c"] {
        assert_eq!(std::fs::read_to_string(fx.root().join("d").join(f)).unwrap(), "new");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_file_onto_directory_errors() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    std::fs::create_dir(fx.root().join("same")).unwrap();
    std::fs::write(fx.root().join("same").join("keep.txt"), "keep").unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("same"), "file").unwrap();

    let (res, _) = run_transfer(req(&Endpoint::Local, &p(&local.path().join("same")), &r, "/"), OverwritePolicy::Overwrite, |_| panic!(), |_| {}).await;
    assert!(res.is_err());
    assert_eq!(std::fs::read_to_string(fx.root().join("same").join("keep.txt")).unwrap(), "keep");
}

#[tokio::test(flavor = "multi_thread")]
async fn transfers_names_with_spaces_and_unicode() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    let name = "my file ü.txt";
    let local_a = tempfile::tempdir().unwrap();
    let local_b = tempfile::tempdir().unwrap();
    std::fs::write(local_a.path().join(name), "ünïcode").unwrap();

    run_transfer(req(&Endpoint::Local, &p(&local_a.path().join(name)), &r, "/"), OverwritePolicy::Ask, |_| panic!(), |_| {}).await.0.unwrap();
    let listed: Vec<String> = r.list("/").await.unwrap().into_iter().map(|e| e.name).collect();
    assert!(listed.contains(&name.to_string()), "{listed:?}");
    run_transfer(req(&r, &format!("/{name}"), &Endpoint::Local, &p(local_b.path())), OverwritePolicy::Ask, |_| panic!(), |_| {}).await.0.unwrap();
    assert_eq!(std::fs::read_to_string(local_b.path().join(name)).unwrap(), "ünïcode");
    r.remove(&format!("/{name}"), false).await.unwrap();
    assert!(!fx.root().join(name).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_fails_when_server_dies() {
    let mut fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("huge.bin"), data(40 * 1024 * 1024, 4)).unwrap();
    let request = req(&Endpoint::Local, &p(&local.path().join("huge.bin")), &r, "/");
    let server = fx.server_mut();
    let started = std::time::Instant::now();
    let (res, _) = run_transfer(request, OverwritePolicy::Ask, |_| panic!(), |_| server.kill()).await;
    assert!(res.is_err(), "{res:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoint_ops_mkdir_rename_remove() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    r.mkdir("/x").await.unwrap();
    r.mkdir("/x/y").await.unwrap();
    std::fs::write(fx.root().join("x").join("y").join("f"), "1").unwrap();
    r.rename("/x", "/z").await.unwrap();
    assert!(r.stat("/z/y/f").await.unwrap().is_some());
    assert!(r.stat("/x").await.unwrap().is_none());
    r.remove("/z", true).await.unwrap();
    assert!(!fx.root().join("z").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_error_shows_ssh_stderr() {
    let fx = TestFixture::new(FixtureAuth::Password("pw".into())).unwrap();
    let err = connect_sftp(launch(&fx, true)).await.err().expect("must fail");
    assert!(err.contains("Permission denied"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn self_copy_is_rejected() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("f"), "data").unwrap();
    std::fs::create_dir(d.path().join("a")).unwrap();
    std::fs::write(d.path().join("a").join("x"), "x").unwrap();

    let (res, _) = run_transfer(req(&Endpoint::Local, &p(&d.path().join("f")), &Endpoint::Local, &p(d.path())), OverwritePolicy::Overwrite, |_| panic!(), |_| {}).await;
    assert!(res.is_err(), "{res:?}");
    assert_eq!(std::fs::read_to_string(d.path().join("f")).unwrap(), "data");

    let (res, _) = run_transfer(req(&Endpoint::Local, &p(&d.path().join("a")), &Endpoint::Local, &p(&d.path().join("a"))), OverwritePolicy::Overwrite, |_| panic!(), |_| {}).await;
    assert!(res.is_err(), "{res:?}");
    assert!(!d.path().join("a").join("a").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn same_remote_self_copy_is_rejected() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    std::fs::create_dir(fx.root().join("a")).unwrap();
    let (res, _) = run_transfer(req(&r, "/a", &r, "/a"), OverwritePolicy::Overwrite, |_| panic!(), |_| {}).await;
    assert!(res.is_err(), "{res:?}");
    assert!(!fx.root().join("a").join("a").exists());
}

/// Creates a directory symlink, or returns false where the OS doesn't allow it
/// (Windows without Developer Mode).
fn symlink_dir(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    let r = std::os::windows::fs::symlink_dir(target, link);
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(target, link);
    match r {
        Ok(()) => true,
        Err(e) => {
            eprintln!("skipping: cannot create symlinks here ({e})");
            false
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinked_dir_in_tree_is_skipped_not_fatal() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let t = fx.root().join("t");
    std::fs::create_dir_all(t.join("real")).unwrap();
    std::fs::write(t.join("real").join("file"), "f").unwrap();
    if !symlink_dir(&t.join("real"), &t.join("link")) {
        return;
    }
    let r = remote(&fx).await;
    let local = tempfile::tempdir().unwrap();
    let (res, _) = run_transfer(req(&r, "/t", &Endpoint::Local, &p(local.path())), OverwritePolicy::Ask, |_| panic!(), |_| {}).await;
    let s = res.unwrap();
    assert_eq!((s.files, s.skipped), (1, 1));
    assert_eq!(std::fs::read_to_string(local.path().join("t").join("real").join("file")).unwrap(), "f");
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_remote_dir_symlink_keeps_target() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::create_dir(fx.root().join("real")).unwrap();
    std::fs::write(fx.root().join("real").join("keep.txt"), "keep").unwrap();
    if !symlink_dir(&fx.root().join("real"), &fx.root().join("link")) {
        return;
    }
    let r = remote(&fx).await;
    r.remove("/link", true).await.unwrap();
    assert!(fx.root().join("real").join("keep.txt").exists());
    assert!(std::fs::symlink_metadata(fx.root().join("link")).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_directory_onto_file_errors() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let r = remote(&fx).await;
    std::fs::write(fx.root().join("same"), "file").unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir(local.path().join("same")).unwrap();
    let (res, _) = run_transfer(req(&Endpoint::Local, &p(&local.path().join("same")), &r, "/"), OverwritePolicy::Overwrite, |_| panic!(), |_| {}).await;
    assert!(res.is_err());
    assert_eq!(std::fs::read_to_string(fx.root().join("same")).unwrap(), "file");
}
