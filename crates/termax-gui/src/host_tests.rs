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
