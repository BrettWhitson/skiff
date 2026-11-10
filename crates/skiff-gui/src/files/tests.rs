//! File pane tests against local temp directories (no network).

use std::time::Duration;

use skiff_ui::input::{Event, Key, Modifiers, MouseButton};

use super::*;
use crate::testing::Env;

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };

/// A temp folder with two files, a dot-file, and two folders (`adir` holding `inner.txt`).
fn tree() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for f in ["b.txt", "A.txt", ".hidden"] {
        std::fs::write(d.path().join(f), b"x").unwrap();
    }
    for dir in ["zdir", "adir"] {
        std::fs::create_dir(d.path().join(dir)).unwrap();
    }
    std::fs::write(d.path().join("adir").join("inner.txt"), b"inner").unwrap();
    d
}

/// Folders come first, names sort case-insensitively, and dot-files show only with "Hidden".
#[test]
fn lists_sorted_dirs_first_hidden_filtered() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    assert_eq!(e.names(0), vec!["adir", "zdir", "A.txt", "b.txt"]);
    e.click("Hidden");
    assert!(e.names(0).contains(&".hidden".to_string()));
}

/// Double-clicking a folder opens it and Backspace goes back to the parent.
#[test]
fn navigate_into_and_up() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    e.double_click("adir");
    e.until("adir listing", |e| e.names(0) == vec!["inner.txt"]);
    assert!(e.tab.panes[0].cwd.ends_with("adir"));
    e.key(Key::Backspace, Modifiers::NONE);
    e.until("back up", |e| e.names(0).contains(&"zdir".to_string()));
    assert_eq!(e.tab.panes[0].cwd, e.start);
}

/// A listing answering an older request is ignored in favour of the newest one.
#[test]
fn stale_listing_dropped() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    let stale_seq = e.tab.panes[0].seq;
    let adir = format!("{}{}adir", e.start, std::path::MAIN_SEPARATOR);
    let ctx = FilesCtx {
        tab_id: 1,
        engine: &e.engine,
        start_dir: &e.start,
        hosts: &e.hosts,
        askpass: None,
        ssh_binary: "ssh",
        ssh_version: None,
    };
    e.tab.panes[0].navigate(adir.clone(), &ctx, 0);
    // a listing that answers an older request arrives late
    let stale = FilesEvent::Listed { seq: stale_seq, cwd: e.start.clone(), result: Ok(vec![]) };
    e.tab.on_event(0, stale, &ctx);
    e.until("adir listing", |e| e.names(0) == vec!["inner.txt"]);
    assert_eq!(e.tab.panes[0].cwd, adir);
}

/// New folder, rename and delete work end to end, and delete waits for confirmation.
#[test]
fn mkdir_rename_delete_with_confirm() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));

    e.click("New folder");
    e.typed("made");
    e.key(Key::Enter, Modifiers::NONE);
    e.until("mkdir", |e| e.names(0).contains(&"made".to_string()));
    assert!(d.path().join("made").is_dir());

    e.click("made");
    e.click("Rename");
    e.key(Key::Char('a'), CTRL);
    e.typed("renamed");
    e.key(Key::Enter, Modifiers::NONE);
    e.until("rename", |e| e.names(0).contains(&"renamed".to_string()));
    assert!(d.path().join("renamed").is_dir() && !d.path().join("made").exists());

    e.click("renamed");
    e.click("Delete");
    assert!(e.texts().iter().any(|t| t == "Delete renamed?"), "{:?}", e.texts());
    assert!(d.path().join("renamed").exists(), "nothing deleted before confirming");
    e.click("Delete");
    e.until("delete", |e| !e.names(0).contains(&"renamed".to_string()));
    assert!(!d.path().join("renamed").exists());
}

/// The Delete key asks about a multi-selection by count, and Cancel deletes nothing.
#[test]
fn delete_key_confirms_multi_selection() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    e.click("A.txt");
    let b = e.find("b.txt");
    e.frame(vec![
        Event::Modifiers(CTRL),
        Event::MouseMove(b),
        Event::MouseDown(MouseButton::Left, b),
        Event::MouseUp(MouseButton::Left, b),
    ]);
    e.frame(vec![Event::Modifiers(Modifiers::NONE)]);
    e.key(Key::Delete, Modifiers::NONE);
    assert!(e.texts().iter().any(|t| t == "Delete 2 items?"), "{:?}", e.texts());
    e.click("Cancel");
    assert!(d.path().join("A.txt").exists());
}

/// Typing a missing path shows an error and keeps the old folder and listing.
#[test]
fn go_to_path_invalid_shows_error() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    let start = e.start.clone();
    e.click(&start);
    e.key(Key::Char('a'), CTRL);
    let bogus = d.path().join("no-such-dir").display().to_string();
    e.typed(&bogus);
    e.key(Key::Enter, Modifiers::NONE);
    e.until("error", |e| matches!(e.tab.panes[0].state, PaneState::Error(_)));
    assert_eq!(e.tab.panes[0].cwd, start, "cwd unchanged");
    assert_eq!(e.names(0), vec!["adir", "zdir", "A.txt", "b.txt"], "old listing kept");
    assert!(e.texts().iter().any(|t| t.contains("no-such-dir")), "{:?}", e.texts());
}

/// A known timestamp formats as the right UTC date and time.
#[test]
fn formats_modified_time() {
    let t = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert_eq!(format_time(t), "2023-11-14 22:13");
}

/// After a refresh adds a file, the same files stay selected even though rows moved.
#[test]
fn selection_follows_names_across_refresh() {
    let d = tempfile::tempdir().unwrap();
    for f in ["a.log", "c.log", "e.log"] {
        std::fs::write(d.path().join(f), b"x").unwrap();
    }
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    e.click("c.log");
    let p = e.find("e.log");
    e.frame(vec![
        Event::Modifiers(CTRL),
        Event::MouseMove(p),
        Event::MouseDown(MouseButton::Left, p),
        Event::MouseUp(MouseButton::Left, p),
    ]);
    e.frame(vec![Event::Modifiers(Modifiers::NONE)]);
    std::fs::write(d.path().join("b.log"), b"x").unwrap();
    e.click("Refresh");
    e.until("b.log listed", |e| e.names(0).contains(&"b.log".to_string()));
    let names: Vec<String> = e.tab.panes[0].selected().into_iter().map(|(_, _, n)| n).collect();
    assert_eq!(names, vec!["c.log", "e.log"]);
}

/// The path bar is a button for screen readers and Enter on it opens editing.
#[test]
fn path_bar_edits_from_keyboard() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    let node = e.h.tree().nodes.iter().find(|n| n.name.starts_with("Path ")).map(|n| (n.id, n.role)).expect("path bar node");
    assert_eq!(node.1, skiff_ui::a11y::Role::Button);
    e.frame(vec![Event::A11y { target: node.0, action: skiff_ui::a11y::A11yAction::Focus }]);
    e.key(Key::Enter, Modifiers::NONE);
    assert!(e.tab.panes[0].path_edit.is_some(), "Enter on the focused path bar edits it");
}

/// A folder name with a slash is refused with a reason and the bar stays open.
#[test]
fn invalid_new_folder_name_says_why() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    e.click("New folder");
    e.typed("a/b");
    e.key(Key::Enter, Modifiers::NONE);
    assert!(e.texts().iter().any(|t| t.contains("a/b") && t.contains("not a valid name")), "{:?}", e.texts());
    assert!(e.tab.panes[0].confirm.is_some(), "the bar stays open to fix the name");
}

/// Esc closes only the inline bar of the pane that has focus; a delete question starts on Cancel.
#[test]
fn esc_closes_only_the_focused_panes_bar() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0) && e.loaded(1));
    e.click("A.txt"); // left pane
    e.key(Key::Delete, Modifiers::NONE);
    assert!(e.texts().iter().any(|t| t == "Delete A.txt?"), "{:?}", e.texts());
    assert_eq!(
        e.h.state.focused(),
        Some(skiff_ui::ui::Id::new(("pane", 1u64, 0usize)).with("bar-cancel")),
        "the question starts on Cancel"
    );
    let b = e.find_right("b.txt");
    e.click_at(b);
    e.key(Key::F(2), Modifiers::NONE); // rename in the right pane
    assert!(e.tab.panes[1].confirm.is_some());
    e.key(Key::Escape, Modifiers::NONE);
    assert!(e.tab.panes[1].confirm.is_none(), "Esc closed the right pane's rename");
    assert!(e.tab.panes[0].confirm.is_some(), "and left the other question alone");
}

/// The location pickers have accessible names, and a host missing from the list still shows by name.
#[test]
fn location_picker_is_named_and_keeps_a_removed_host() {
    let d = tree();
    let cfg =
        skiff_core::config::Config::parse("[settings]\nload_ssh_config=false\n[[connection]]\nname=\"web\"\nhost=\"127.0.0.1\"\nport=1\n")
            .unwrap();
    let web = skiff_core::hosts::collect(&cfg).0.remove(0);
    let mut e = Env::with(d.path(), vec![], Location::Local, Location::Host(Box::new(web)), None);
    e.frame(vec![]);
    let t = e.h.tree();
    assert_eq!(t.find("Left pane location").and_then(|n| n.value.clone()).as_deref(), Some("Local"));
    assert_eq!(t.find("Right pane location").and_then(|n| n.value.clone()).as_deref(), Some("web"), "not shown as Local");
}
