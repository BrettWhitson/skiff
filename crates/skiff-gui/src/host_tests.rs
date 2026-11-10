//! Host management in the GUI: ssh config editing, status, forwards.
//! Temp config and ssh config files only; the one status check targets a
//! closed port on 127.0.0.1.

use std::time::{Duration, Instant};

use skiff_core::status::Status;
use skiff_ui::geom::Point;
use skiff_ui::input::{Event, Key, Modifiers};

use crate::app::tests::Fixture;

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };

/// Replaces the text of the input labelled `label`.
fn set_field(f: &mut Fixture, label: &str, value: &str) {
    let l = f.find_last(label);
    f.click(Point::new(l.x + crate::settings::LABEL_W + 20, l.y));
    f.frame(vec![Event::KeyDown { key: Key::Char('a'), mods: CTRL, text: None }]);
    f.frame(vec![Event::Text(value.into())]);
    f.frame(vec![]);
}

/// Activates the control named `name` the way a screen reader does.
fn activate(f: &mut Fixture, name: &str) {
    let id = f.h.tree().find(name).map(|n| n.id).unwrap_or_else(|| panic!("no control named {name:?}"));
    f.frame(vec![Event::A11y { target: id, action: skiff_ui::a11y::A11yAction::Click }]);
    f.frame(vec![]);
}

/// Opens the overflow menu of the forward row titled `title` and picks `item`.
fn row_menu(f: &mut Fixture, title: &str, item: &str) {
    activate(f, &format!("More actions for {title}"));
    let at = f.find_last(item);
    f.click(at);
}

/// A fixture whose hosts come from a temp ssh config holding `ssh`; returns
/// the fixture and that file's path. ssh itself is a nonexistent binary.
fn with_ssh(ssh: &str) -> (Fixture, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ssh_path = dir.path().join("ssh_config");
    std::fs::write(&ssh_path, ssh).unwrap();
    let cfg = format!(
        "[settings]\nload_ssh_config = true\nssh_config_path = '{}'\nssh_binary = \"skiff-test-no-such-ssh\"\n",
        ssh_path.display()
    );
    let f = Fixture::with_config_in(dir, &cfg);
    (f, ssh_path)
}

/// Editing an ssh-config host rewrites just its line, keeping comments.
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

/// A host from a shared `Host a b` block opens read-only with the reason shown.
#[test]
fn read_only_ssh_host_shows_why() {
    let (mut f, _) = with_ssh("Host a b\n    User x\n");
    f.right_click_text("a");
    f.click_text("Edit");
    assert!(f.texts().iter().any(|t| t.contains("shared block")), "{:?}", f.texts());
}

/// "Check status" against a closed local port marks the host offline, shown
/// in the status dot's hover label.
#[test]
fn check_status_shows_dot_label() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut f = Fixture::with_config(&format!(
        "[settings]\nload_ssh_config = false\nssh_binary = \"skiff-test-no-such-ssh\"\n\n[[connection]]\nname = \"alpha\"\nhost = \"127.0.0.1\"\nport = {port}\n"
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

/// Hiding a host writes the hidden marker and removes it from the list.
#[test]
fn hide_moves_host_out_of_the_list() {
    let (mut f, ssh) = with_ssh("Host a\n    HostName 1.1.1.1\nHost b\n");
    f.right_click_text("a");
    f.click_text("Hide");
    assert!(std::fs::read_to_string(&ssh).unwrap().contains("# skiff: hidden"));
    assert!(f.try_find("a").is_none(), "{:?}", f.texts());
    assert!(f.try_find("b").is_some());
}

/// An invalid listen port shows an inline error and saves nothing.
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

/// A save that failed (read-only ssh config) succeeds when retried.
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

/// The delete confirmation removes the right host even if the host list
/// is rebuilt (reindexed) while it is open.
#[test]
fn delete_confirm_follows_the_host_after_a_rebuild() {
    let mut f = Fixture::new();
    f.right_click_text("beta");
    f.click_text("Delete");
    // The list is rebuilt while the confirmation is open (an outside edit).
    let gamma = skiff_core::config::Connection { name: "gamma".into(), host: "127.0.0.3".into(), ..Default::default() };
    f.app.cfg.connections.insert(0, gamma);
    f.app.rebuild_hosts();
    f.frame(vec![]);
    f.click_text("Delete");
    let names: Vec<&str> = f.app.cfg.connections.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["gamma", "alpha"]);
}

/// One host `alpha` (unreachable port) with one saved dynamic forward.
const FWD_CONFIG: &str = "[settings]\nload_ssh_config = false\nssh_binary = \"skiff-test-no-such-ssh\"\n\n[[connection]]\nname = \"alpha\"\nhost = \"127.0.0.1\"\nport = 1\n\n[[forward]]\nhost = \"alpha\"\nkind = \"dynamic\"\nlisten = 47390\n";

/// "Save" stores a forward without starting it, and Delete only removes it
/// after the confirmation's own Delete button.
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

    let title = f.app.cfg.forwards[0].title();
    row_menu(&mut f, &title, "Delete");
    assert!(f.texts().iter().any(|t| t.starts_with("Delete forward ")), "{:?}", f.texts());
    assert_eq!(f.app.cfg.forwards.len(), 2, "nothing deleted before confirming");
    f.click_text("Cancel");
    assert_eq!(f.app.cfg.forwards.len(), 2);
    row_menu(&mut f, &title, "Delete");
    let at = f.find_last("Delete"); // the dialog's button
    f.click(at);
    assert_eq!(f.app.cfg.forwards.len(), 1);
    assert!(!std::fs::read_to_string(&f.path).unwrap().contains("47390"));
}

/// A stopped forward is pruned from the running list on a later frame.
#[test]
fn stopped_forwards_are_dropped() {
    let mut f = Fixture::with_config(FWD_CONFIG);
    let fwd = f.app.cfg.forwards[0].clone();
    // A local stand-in for ssh that just waits.
    let (prog, args) = if cfg!(windows) { ("ping", vec!["-n", "30", "127.0.0.1"]) } else { ("sleep", vec!["30"]) };
    let handle =
        skiff_core::forward::ForwardHandle::start(f.app.engine.rt.handle(), prog, args.into_iter().map(String::from).collect(), vec![]);
    f.app.forwards.running.push(crate::forwards::Running { fwd: fwd.clone(), handle, tag: "fwd:x".into(), recorded: true });
    f.app.forwards.stop(&fwd);
    let start = Instant::now();
    while !f.app.forwards.running.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(10), "{:?}", f.app.forwards.state_of(&fwd));
        std::thread::sleep(Duration::from_millis(30));
        f.frame(vec![]);
    }
}

/// Renaming a host moves its forwards and connection history to the new name.
#[test]
fn rename_carries_forwards_and_history() {
    let mut f = Fixture::with_config(FWD_CONFIG);
    f.app.record("alpha", skiff_core::history::ConnKind::Terminal);
    f.right_click_text("alpha");
    f.click_text("Edit");
    set_field(&mut f, "Name", "alpha2");
    f.click_text("Save");
    assert_eq!(f.app.cfg.connections[0].name, "alpha2");
    assert_eq!(f.app.cfg.forwards[0].host, "alpha2");
    assert!(std::fs::read_to_string(&f.path).unwrap().contains("host = \"alpha2\""));
    let hist = skiff_core::history::History::load(&skiff_core::history::History::path_for(&f.path));
    assert_eq!(hist.recent(5), ["alpha2"]);
}

/// Editing a forward whose host no longer exists keeps that host.
#[test]
fn editing_a_forward_keeps_a_host_no_longer_listed() {
    let mut f = Fixture::with_config(&FWD_CONFIG.replace("host = \"alpha\"\nkind", "host = \"gone\"\nkind"));
    f.app.open_forwards();
    f.frame(vec![]);
    let title = f.app.cfg.forwards[0].title();
    row_menu(&mut f, &title, "Edit");
    set_field(&mut f, "Listen port", "47392");
    f.click_text("Save");
    assert_eq!((f.app.cfg.forwards[0].host.as_str(), f.app.cfg.forwards[0].listen), ("gone", 47392), "not moved to another host");
}

/// Forward rows name their Start button and overflow menu by the forward,
/// the menu offers Edit and Delete, and the status pill says "Stopped".
#[test]
fn forward_rows_and_sidebar_controls_are_named() {
    use skiff_ui::a11y::Role;
    let mut f = Fixture::with_config(FWD_CONFIG);
    f.app.open_forwards();
    f.frame(vec![]);
    let title = f.app.cfg.forwards[0].title();
    let t = f.h.tree();
    assert_eq!(t.find(&format!("Start {title}")).map(|n| n.role), Some(Role::Button));
    assert!(f.try_find("Stopped").is_some(), "{:?}", f.texts());
    assert_eq!(t.find("More host actions").map(|n| n.role), Some(Role::Button));
    assert_eq!(t.find("Sort hosts by").and_then(|n| n.value.clone()).as_deref(), Some("Name"));
    activate(&mut f, &format!("More actions for {title}"));
    let t = f.h.tree();
    for item in ["Edit", "Delete"] {
        assert_eq!(t.nodes.iter().find(|n| n.name == item && n.role == Role::MenuItem).map(|n| n.role), Some(Role::MenuItem), "{item}");
    }
}

/// The Delete key asks first with focus on Cancel; Esc cancels and returns
/// focus to the host list.
#[test]
fn sidebar_delete_key_focuses_cancel_and_esc_cancels() {
    let mut f = Fixture::new();
    f.click_text("beta");
    f.frame(vec![Event::KeyDown { key: Key::Delete, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(f.try_find("Delete beta?").is_some(), "{:?}", f.texts());
    let cancel = skiff_ui::ui::Id::new("sidebar-delete-dialog").with(("button", 0usize));
    assert_eq!(f.h.state.focused(), Some(cancel), "a reflexive Enter keeps the host");
    f.frame(vec![Event::KeyDown { key: Key::Escape, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert!(f.try_find("Delete beta?").is_none());
    assert_eq!(f.app.cfg.connections.len(), 2);
    assert_eq!(f.h.state.focused(), Some(skiff_ui::ui::Id::new("sidebar-list")), "back on the list");
}

/// Saving refuses to resurrect a forward deleted meanwhile and refuses an
/// exact duplicate.
#[test]
fn forward_save_refuses_deleted_and_duplicate() {
    let mut f = Fixture::with_config(FWD_CONFIG);
    f.app.open_forwards();
    f.frame(vec![]);
    let title = f.app.cfg.forwards[0].title();
    row_menu(&mut f, &title, "Edit");
    let saved = f.app.cfg.forwards.remove(0); // deleted while the form is open
    f.click_text("Save");
    assert!(f.app.cfg.forwards.is_empty(), "not brought back");
    assert!(f.texts().iter().any(|t| t.starts_with("This forward was deleted")), "{:?}", f.texts());
    f.app.cfg.forwards.push(saved);
    f.app.forwards.form = Some(crate::forwards::FwdForm { host: "alpha".into(), kind: 2, listen: "47390".into(), ..Default::default() });
    f.frame(vec![]);
    f.click_text("Save");
    assert_eq!(f.app.cfg.forwards.len(), 1, "no duplicate");
    assert!(f.texts().iter().any(|t| t == "This forward is already saved"), "{:?}", f.texts());
}

/// Opening another form over unsaved edits asks first; "Keep editing" keeps
/// them and "Discard" replaces the form.
#[test]
fn forward_form_asks_before_discarding_edits() {
    let mut f = Fixture::with_config(FWD_CONFIG);
    f.app.open_forwards();
    f.frame(vec![]);
    let title = f.app.cfg.forwards[0].title();
    row_menu(&mut f, &title, "Edit");
    set_field(&mut f, "Listen port", "47395");
    f.click_text("New forward");
    assert!(f.try_find("Discard unsaved changes to this forward?").is_some(), "{:?}", f.texts());
    f.click_text("Keep editing");
    assert_eq!(f.app.forwards.form.as_ref().map(|x| x.listen.as_str()), Some("47395"));
    f.right_click_text("alpha");
    f.click_text("Port forward\u{2026}");
    f.click_text("Discard");
    let form = f.app.forwards.form.as_ref().unwrap();
    assert!(form.editing.is_none() && form.listen.is_empty());
}
