//! Remote file panes against the local test server only (127.0.0.1). The
//! connections carry the fixture's ssh options, so the user's ~/.ssh is
//! never read.

use std::path::PathBuf;
use std::time::Duration;

use skiff_core::config::{Config, Connection};
use skiff_core::hosts::{HostEntry, collect};
use skiff_gui::files::{Location, PaneState};
use skiff_gui::testing::Env;
use skiff_testserver::{FixtureAuth, TestFixture};
use skiff_ui::input::{Key, Modifiers};

/// A host entry `name` pointing at fixture `fx` with its ssh options.
fn host_for(fx: &TestFixture, name: &str) -> HostEntry {
    let mut cfg = Config::default();
    cfg.settings.load_ssh_config = false;
    cfg.connections.push(Connection { name: name.into(), host: fx.destination(), extra_args: fx.ssh_options(), ..Default::default() });
    collect(&cfg).0.remove(0)
}

/// The built skiff-gui binary, used as the askpass helper.
fn helper() -> Option<PathBuf> {
    Some(PathBuf::from(env!("CARGO_BIN_EXE_skiff-gui")))
}

/// A temp folder holding `localdir/deeper`.
fn local_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("localdir")).unwrap();
    std::fs::create_dir(d.path().join("localdir").join("deeper")).unwrap();
    d
}

/// With key auth, the remote pane connects and lists the server root.
#[test]
fn key_auth_pane_lists_remote_root() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::write(fx.root().join("on-server.txt"), b"hi").unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Host(Box::new(h)), helper());
    e.until("remote listing", |e| e.loaded(1));
    assert_eq!(e.names(1), vec!["on-server.txt"]);
    assert!(e.tab.panes[1].cwd.starts_with('/'));
}

/// A password prompt shows inline in the pane, masks typing, and the pane
/// lists once the password is entered.
#[test]
fn password_prompt_inline_then_lists() {
    let fx = TestFixture::new(FixtureAuth::Password("s3cret-pw".into())).unwrap();
    std::fs::write(fx.root().join("pw-file.txt"), b"hi").unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Host(Box::new(h)), helper());
    e.until("prompt", |e| e.tab.panes[1].prompt.is_some());
    assert!(e.texts().iter().any(|t| t.to_lowercase().contains("password")), "{:?}", e.texts());
    assert!(e.texts().iter().all(|t| !t.contains("s3cret")));
    e.typed("s3cret-pw");
    assert!(e.texts().iter().all(|t| !t.contains("s3cret")), "input is masked");
    e.key(Key::Enter, Modifiers::NONE);
    e.until("remote listing", |e| e.loaded(1));
    assert_eq!(e.names(1), vec!["pw-file.txt"]);
}

/// Cancelling the password prompt leaves the pane in an error state with
/// Retry, while the other pane keeps working.
#[test]
fn cancelled_prompt_leaves_pane_error() {
    let fx = TestFixture::new(FixtureAuth::Password("pw".into())).unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Host(Box::new(h)), helper());
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

/// An ssh older than OpenSSH 8.4 gets an error naming the needed version and
/// no prompt.
#[test]
fn old_ssh_version_message() {
    let fx = TestFixture::new(FixtureAuth::Password("pw".into())).unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Local, helper());
    e.ssh_version = Some((8, 3));
    e.tab.panes[1].set_location(Location::Host(Box::new(h)));
    e.until("error", |e| matches!(e.tab.panes[1].state, PaneState::Error(_)));
    let PaneState::Error(msg) = &e.tab.panes[1].state else { unreachable!() };
    assert!(msg.contains("OpenSSH 8.4"), "{msg}");
    assert!(e.tab.panes[1].prompt.is_none(), "no prompt with an old client");
}

/// Dragging a file from one remote pane to another relays it intact.
#[test]
fn remote_to_remote_relay_via_ui() {
    let fa = TestFixture::new(FixtureAuth::Key).unwrap();
    let fb = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::write(fa.root().join("relay.bin"), vec![7u8; 300_000]).unwrap();
    let (ha, hb) = (host_for(&fa, "web-01"), host_for(&fb, "db-02"));
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![ha.clone(), hb.clone()], Location::Host(Box::new(ha)), Location::Host(Box::new(hb)), None);
    e.until("both connected", |e| e.loaded(0) && e.loaded(1));
    // Press on the file, move past the drag threshold, drop on the right pane.
    let from = e.find("relay.bin");
    e.frame(vec![skiff_ui::input::Event::MouseMove(from), skiff_ui::input::Event::MouseDown(skiff_ui::input::MouseButton::Left, from)]);
    e.frame(vec![skiff_ui::input::Event::MouseMove(skiff_ui::geom::Point::new(from.x + 12, from.y))]);
    let to = skiff_ui::geom::Point::new(900, 400);
    e.frame(vec![skiff_ui::input::Event::MouseMove(to)]);
    e.frame(vec![skiff_ui::input::Event::MouseUp(skiff_ui::input::MouseButton::Left, to)]);
    e.until("relayed", |e| e.names(1).contains(&"relay.bin".to_string()));
    assert_eq!(std::fs::read(fb.root().join("relay.bin")).unwrap(), vec![7u8; 300_000]);
}

/// Copying a file onto itself (same host and folder) is refused, file intact.
#[test]
fn same_host_same_folder_copy_is_refused() {
    let fx = TestFixture::new(FixtureAuth::Key).unwrap();
    std::fs::write(fx.root().join("keep.txt"), b"precious").unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Host(Box::new(h.clone())), Location::Host(Box::new(h)), None);
    e.until("both connected", |e| e.loaded(0) && e.loaded(1));
    e.click("keep.txt");
    e.click("Copy \u{2192}");
    e.frame(vec![]);
    assert!(e.tab.transfers.rows.is_empty(), "no transfer started");
    assert!(e.texts().iter().any(|t| t.contains("onto itself")), "{:?}", e.texts());
    assert_eq!(std::fs::read(fx.root().join("keep.txt")).unwrap(), b"precious");
}

/// After the server dies, a refresh fails with a Reconnect button that
/// starts a fresh connection.
#[test]
fn dropped_connection_offers_reconnect() {
    let mut fx = TestFixture::new(FixtureAuth::Key).unwrap();
    let h = host_for(&fx, "fx");
    let d = local_dir();
    let mut e = Env::with(d.path(), vec![h.clone()], Location::Local, Location::Host(Box::new(h)), None);
    e.until("remote listing", |e| e.loaded(1));
    fx.server_mut().kill();
    let refresh = e.find_right("Refresh");
    e.click_at(refresh);
    e.until("listing fails", |e| e.texts().iter().any(|t| t == "Reconnect"));
    let r = e.find_right("Reconnect");
    e.click_at(r);
    assert!(e.tab.panes[1].ep.is_none(), "reconnecting from scratch");
}
