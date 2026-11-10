//! File pane tests against local temp directories (no network).

use std::time::Duration;

use termax_ui::input::{Event, Key, Modifiers, MouseButton};

use super::*;
use crate::testing::Env;

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };

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

#[test]
fn lists_sorted_dirs_first_hidden_filtered() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    assert_eq!(e.names(0), vec!["adir", "zdir", "A.txt", "b.txt"]);
    e.click("Hidden");
    assert!(e.names(0).contains(&".hidden".to_string()));
}

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

#[test]
fn stale_listing_dropped() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    let stale_seq = e.tab.panes[0].seq;
    let adir = format!("{}{}adir", e.start, std::path::MAIN_SEPARATOR);
    let ctx = FilesCtx { tab_id: 1, engine: &e.engine, start_dir: &e.start, hosts: &e.hosts, askpass: None, ssh_binary: "ssh", ssh_version: None };
    e.tab.panes[0].navigate(adir.clone(), &ctx, 0);
    // a listing that answers an older request arrives late
    let stale = FilesEvent::Listed { seq: stale_seq, cwd: e.start.clone(), result: Ok(vec![]) };
    e.tab.on_event(0, stale, &ctx);
    e.until("adir listing", |e| e.names(0) == vec!["inner.txt"]);
    assert_eq!(e.tab.panes[0].cwd, adir);
}

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

#[test]
fn delete_key_confirms_multi_selection() {
    let d = tree();
    let mut e = Env::new(d.path());
    e.until("listing", |e| e.loaded(0));
    e.click("A.txt");
    let b = e.find("b.txt");
    e.frame(vec![Event::Modifiers(CTRL), Event::MouseMove(b), Event::MouseDown(MouseButton::Left, b), Event::MouseUp(MouseButton::Left, b)]);
    e.frame(vec![Event::Modifiers(Modifiers::NONE)]);
    e.key(Key::Delete, Modifiers::NONE);
    assert!(e.texts().iter().any(|t| t == "Delete 2 items?"), "{:?}", e.texts());
    e.click("Cancel");
    assert!(d.path().join("A.txt").exists());
}

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

#[test]
fn formats_modified_time() {
    let t = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert_eq!(format_time(t), "2023-11-14 22:13");
}

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
    e.frame(vec![Event::Modifiers(CTRL), Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]);
    e.frame(vec![Event::Modifiers(Modifiers::NONE)]);
    std::fs::write(d.path().join("b.log"), b"x").unwrap();
    e.click("Refresh");
    e.until("b.log listed", |e| e.names(0).contains(&"b.log".to_string()));
    let names: Vec<String> = e.tab.panes[0].selected().into_iter().map(|(_, _, n)| n).collect();
    assert_eq!(names, vec!["c.log", "e.log"]);
}
