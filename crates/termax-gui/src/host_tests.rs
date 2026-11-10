//! Host management in the GUI: ssh config editing, status, forwards.
//! Temp config and ssh config files only; the one status check targets a
//! closed port on 127.0.0.1.

use std::time::{Duration, Instant};

use termax_core::status::Status;
use termax_ui::geom::Point;
use termax_ui::input::{Event, Key, Modifiers};

use crate::app::tests::Fixture;

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };

/// Replaces the text of the input labelled `label`.
fn set_field(f: &mut Fixture, label: &str, value: &str) {
    let l = f.find(label);
    f.click(Point::new(l.x + crate::settings::LABEL_W + 20, l.y));
    f.frame(vec![Event::KeyDown { key: Key::Char('a'), mods: CTRL, text: None }]);
    f.frame(vec![Event::Text(value.into())]);
    f.frame(vec![]);
}

fn with_ssh(ssh: &str) -> (Fixture, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ssh_path = dir.path().join("ssh_config");
    std::fs::write(&ssh_path, ssh).unwrap();
    let cfg = format!(
        "[settings]\nload_ssh_config = true\nssh_config_path = '{}'\nssh_binary = \"termax-test-no-such-ssh\"\n",
        ssh_path.display()
    );
    let f = Fixture::with_config_in(dir, &cfg);
    (f, ssh_path)
}

#[test]
fn sidebar_edit_ssh_host_saves_in_place() {
    let (mut f, ssh) = with_ssh("# servers\nHost web\n    HostName 10.0.0.5\n    Port 22\n");
    f.right_click_text("web");
    f.click_text("Edit");
    set_field(&mut f, "Port", "2200");
    f.click_text("Save");
    assert_eq!(std::fs::read_to_string(&ssh).unwrap(), "# servers\nHost web\n    HostName 10.0.0.5\n    Port 2200\n");
    assert_eq!(f.app.hosts.iter().find(|h| h.name == "web").unwrap().port, Some(2200));
}

#[test]
fn read_only_ssh_host_shows_why() {
    let (mut f, _) = with_ssh("Host a b\n    User x\n");
    f.right_click_text("a");
    f.click_text("Edit");
    assert!(f.texts().iter().any(|t| t.contains("shared block")), "{:?}", f.texts());
}

#[test]
fn check_status_shows_dot_label() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut f = Fixture::with_config(&format!(
        "[settings]\nload_ssh_config = false\nssh_binary = \"termax-test-no-such-ssh\"\n\n[[connection]]\nname = \"alpha\"\nhost = \"127.0.0.1\"\nport = {port}\n"
    ));
    f.right_click_text("alpha");
    f.click_text("Check status");
    let start = Instant::now();
    while !matches!(f.app.statuses.get("alpha"), Some(Status::Offline(_))) {
        assert!(start.elapsed() < Duration::from_secs(15), "{:?}", f.app.statuses);
        std::thread::sleep(Duration::from_millis(30));
        f.frame(vec![]);
    }
    let at = f.find("alpha");
    f.frame(vec![Event::MouseMove(Point::new(at.x + 4, at.y + 6))]);
    f.frame(vec![]);
    assert!(f.texts().iter().any(|t| t.contains("offline")), "{:?}", f.texts());
}

#[test]
fn hide_moves_host_out_of_the_list() {
    let (mut f, ssh) = with_ssh("Host a\n    HostName 1.1.1.1\nHost b\n");
    f.right_click_text("a");
    f.click_text("Hide");
    assert!(std::fs::read_to_string(&ssh).unwrap().contains("# termax: hidden"));
    assert!(f.try_find("a").is_none(), "{:?}", f.texts());
    assert!(f.try_find("b").is_some());
}

#[test]
fn forward_form_validation_inline() {
    let mut f = Fixture::new();
    f.right_click_text("alpha");
    f.click_text("Port forward\u{2026}");
    set_field(&mut f, "Listen port", "0");
    f.click_text("Save and start");
    assert!(f.texts().iter().any(|t| t == "Listen port must be 1\u{2013}65535"), "{:?}", f.texts());
    assert!(f.app.cfg.forwards.is_empty());
}

#[cfg(windows)]
#[test]
fn failed_save_can_be_retried() {
    let (mut f, ssh) = with_ssh("Host web\n    HostName 10.0.0.5\n");
    let ro = |on: bool| {
        let mut perm = std::fs::metadata(&ssh).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(on);
        std::fs::set_permissions(&ssh, perm).unwrap();
    };
    f.right_click_text("web");
    f.click_text("Edit");
    set_field(&mut f, "Name", "web2");
    ro(true);
    f.click_text("Save");
    ro(false);
    f.click_text("Save");
    assert!(std::fs::read_to_string(&ssh).unwrap().contains("Host web2"));
}

#[test]
fn delete_confirm_follows_the_host_after_a_rebuild() {
    let mut f = Fixture::new();
    f.right_click_text("beta");
    f.click_text("Delete");
    // The list is rebuilt while the confirmation is open (an outside edit).
    let gamma = termax_core::config::Connection { name: "gamma".into(), host: "127.0.0.3".into(), ..Default::default() };
    f.app.cfg.connections.insert(0, gamma);
    f.app.rebuild_hosts();
    f.frame(vec![]);
    f.click_text("Delete");
    let names: Vec<&str> = f.app.cfg.connections.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["gamma", "alpha"]);
}

const FWD_CONFIG: &str = "[settings]\nload_ssh_config = false\nssh_binary = \"termax-test-no-such-ssh\"\n\n[[connection]]\nname = \"alpha\"\nhost = \"127.0.0.1\"\nport = 1\n\n[[forward]]\nhost = \"alpha\"\nkind = \"dynamic\"\nlisten = 47390\n";

#[test]
fn forward_save_without_start_and_delete_asks() {
    let mut f = Fixture::with_config(FWD_CONFIG);
    f.right_click_text("alpha");
    f.click_text("Port forward\u{2026}");
    set_field(&mut f, "Listen port", "47391");
    set_field(&mut f, "Target", "localhost:80");
    f.click_text("Save");
    assert_eq!(f.app.cfg.forwards.len(), 2);
    assert!(f.app.forwards.running.is_empty(), "Save doesn't start it");

    let del = f.h.list().prims.iter().filter(|p| matches!(p, termax_ui::display::Prim::Text { text, .. } if text == "Delete")).count();
    assert_eq!(del, 2);
    f.click_text("Delete");
    assert!(f.texts().iter().any(|t| t.starts_with("Delete forward ")), "{:?}", f.texts());
    assert_eq!(f.app.cfg.forwards.len(), 2, "nothing deleted before confirming");
    f.click_text("Cancel");
    assert_eq!(f.app.cfg.forwards.len(), 2);
    f.click_text("Delete");
    let yes = f.texts().iter().filter(|t| *t == "Delete").count();
    assert_eq!(yes, 3, "the confirm button joins the row buttons");
    // The confirmation's button comes first on screen.
    f.click_text("Delete");
    assert_eq!(f.app.cfg.forwards.len(), 1);
    assert!(!std::fs::read_to_string(&f.path).unwrap().contains("47390"));
}

#[test]
fn stopped_forwards_are_dropped() {
    let mut f = Fixture::with_config(FWD_CONFIG);
    let fwd = f.app.cfg.forwards[0].clone();
    // A local stand-in for ssh that just waits.
    let (prog, args) = if cfg!(windows) { ("ping", vec!["-n", "30", "127.0.0.1"]) } else { ("sleep", vec!["30"]) };
    let handle =
        termax_core::forward::ForwardHandle::start(f.app.engine.rt.handle(), prog, args.into_iter().map(String::from).collect(), vec![]);
    f.app.forwards.running.push(crate::forwards::Running { fwd: fwd.clone(), handle, tag: "fwd:x".into(), recorded: true });
    f.app.forwards.stop(&fwd);
    let start = Instant::now();
    while !f.app.forwards.running.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(10), "{:?}", f.app.forwards.state_of(&fwd));
        std::thread::sleep(Duration::from_millis(30));
        f.frame(vec![]);
    }
}

#[test]
fn rename_carries_forwards_and_history() {
    let mut f = Fixture::with_config(FWD_CONFIG);
    f.app.record("alpha", termax_core::history::ConnKind::Terminal);
    f.right_click_text("alpha");
    f.click_text("Edit");
    set_field(&mut f, "Name", "alpha2");
    f.click_text("Save");
    assert_eq!(f.app.cfg.connections[0].name, "alpha2");
    assert_eq!(f.app.cfg.forwards[0].host, "alpha2");
    assert!(std::fs::read_to_string(&f.path).unwrap().contains("host = \"alpha2\""));
    let hist = termax_core::history::History::load(&termax_core::history::History::path_for(&f.path));
    assert_eq!(hist.recent(5), ["alpha2"]);
}
