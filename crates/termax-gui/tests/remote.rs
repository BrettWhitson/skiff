//! Remote file panes against the local test server only (127.0.0.1). The
//! connections carry the fixture's ssh options, so the user's ~/.ssh is
//! never read.

use std::path::PathBuf;
use std::time::Duration;

use termax_core::config::{Config, Connection};
use termax_core::hosts::{HostEntry, collect};
use termax_gui::files::{Location, PaneState};
use termax_gui::testing::Env;
use termax_testserver::{FixtureAuth, TestFixture};
use termax_ui::input::{Key, Modifiers};

fn host_for(fx: &TestFixture, name: &str) -> HostEntry {
    let mut cfg = Config::default();
    cfg.settings.load_ssh_config = false;
    cfg.connections.push(Connection {
        name: name.into(),
        host: fx.destination(),
        extra_args: fx.ssh_options(),
        ..Default::default()
    });
    collect(&cfg).0.remove(0)
}

fn helper() -> Option<PathBuf> {
    Some(PathBuf::from(env!("CARGO_BIN_EXE_termax-gui")))
}

fn local_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("localdir")).unwrap();
    std::fs::create_dir(d.path().join("localdir").join("deeper")).unwrap();
    d
}

#[test]
fn key_auth_pane_lists_remote_root() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::write(fx.root().join("on-server.txt"), b"hi").unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Host(h), helper());
    e.until("remote listing", |e| e.loaded(1));
    assert_eq!(e.names(1), vec!["on-server.txt"]);
    assert!(e.tab.panes[1].cwd.starts_with('/'));
}

#[test]
fn password_prompt_inline_then_lists() {
    let fx = TestFixture::new(FixtureAuth::Password("s3cret-pw".into())).unwrap();
    std::fs::write(fx.root().join("pw-file.txt"), b"hi").unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Host(h), helper());
    e.until("prompt", |e| e.tab.panes[1].prompt.is_some());
    assert!(e.texts().iter().any(|t| t.to_lowercase().contains("password")), "{:?}", e.texts());
    assert!(e.texts().iter().all(|t| !t.contains("s3cret")));
    e.typed("s3cret-pw");
    assert!(e.texts().iter().all(|t| !t.contains("s3cret")), "input is masked");
    e.key(Key::Enter, Modifiers::NONE);
    e.until("remote listing", |e| e.loaded(1));
    assert_eq!(e.names(1), vec!["pw-file.txt"]);
}

#[test]
fn cancelled_prompt_leaves_pane_error() {
    let fx = TestFixture::new(FixtureAuth::Password("pw".into())).unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Host(h), helper());
    e.until("left listing", |e| e.loaded(0));
    e.until("prompt", |e| e.tab.panes[1].prompt.is_some());
    let cancel = e.find_right("Cancel");
    e.click_at(cancel);
    let start = std::time::Instant::now();
    e.until("error", |e| matches!(e.tab.panes[1].state, PaneState::Error(_)));
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(e.tab.panes[1].prompt.is_none());
    assert!(e.texts().iter().any(|t| t == "Retry"), "{:?}", e.texts());
    // the other pane still works
    e.double_click("localdir");
    e.until("left navigated", |e| e.names(0) == vec!["deeper"]);
}

#[test]
fn old_ssh_version_message() {
    let fx = TestFixture::new(FixtureAuth::Password("pw".into())).unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Local, helper());
    e.ssh_version = Some((8, 3));
    e.tab.panes[1].set_location(Location::Host(h));
    e.until("error", |e| matches!(e.tab.panes[1].state, PaneState::Error(_)));
    let PaneState::Error(msg) = &e.tab.panes[1].state else { unreachable!() };
    assert!(msg.contains("OpenSSH 8.4"), "{msg}");
    assert!(e.tab.panes[1].prompt.is_none(), "no prompt with an old client");
}

#[test]
fn remote_to_remote_relay_via_ui() {
    let fa = TestFixture::new(FixtureAuth::Key).unwrap();
    let fb = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::write(fa.root().join("relay.bin"), vec![7u8; 300_000]).unwrap();
    let (ha, hb) = (host_for(&fa, "web-01"), host_for(&fb, "db-02"));
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![ha.clone(), hb.clone()], Location::Host(ha), Location::Host(hb), None);
    e.until("both connected", |e| e.loaded(0) && e.loaded(1));
    let from = e.find("relay.bin");
    e.frame(vec![termax_ui::input::Event::MouseMove(from), termax_ui::input::Event::MouseDown(termax_ui::input::MouseButton::Left, from)]);
    e.frame(vec![termax_ui::input::Event::MouseMove(termax_ui::geom::Point::new(from.x + 12, from.y))]);
    let to = termax_ui::geom::Point::new(900, 400);
    e.frame(vec![termax_ui::input::Event::MouseMove(to)]);
    e.frame(vec![termax_ui::input::Event::MouseUp(termax_ui::input::MouseButton::Left, to)]);
    e.until("relayed", |e| e.names(1).contains(&"relay.bin".to_string()));
    assert_eq!(std::fs::read(fb.root().join("relay.bin")).unwrap(), vec![7u8; 300_000]);
}
