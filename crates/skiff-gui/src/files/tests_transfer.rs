//! Transfer tests between local temp directories (no network).

use std::path::Path;

use skiff_ui::geom::Point;
use skiff_ui::input::{Event, Key, Modifiers, MouseButton};

use super::transfers::RowState;
use crate::testing::Env;

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };

/// Left pane in `a`, right pane in `b`.
fn env(a: &Path, b: &Path) -> Env {
    let mut e = Env::new(a);
    e.until("left", |e| e.loaded(0));
    e.until("right", |e| e.loaded(1));
    e.navigate(1, &b.display().to_string());
    let bs = b.display().to_string();
    e.until("right in b", move |e| e.loaded(1) && e.tab.panes[1].cwd == bs);
    e
}

fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("x.txt"), b"new x").unwrap();
    std::fs::write(a.path().join("y.txt"), b"new y").unwrap();
    std::fs::create_dir(a.path().join("sub")).unwrap();
    std::fs::write(a.path().join("sub").join("deep.txt"), b"deep").unwrap();
    (a, b)
}

fn all_done(e: &Env) -> bool {
    !e.tab.transfers.rows.is_empty() && e.tab.transfers.rows.iter().all(|r| !matches!(r.state, RowState::Running | RowState::Waiting(_)))
}

#[test]
fn drag_between_panes_copies_local_to_local() {
    let (a, b) = dirs();
    let mut e = env(a.path(), b.path());
    let from = e.find("x.txt");
    e.frame(vec![Event::MouseMove(from), Event::MouseDown(MouseButton::Left, from)]);
    e.frame(vec![Event::MouseMove(Point::new(from.x + 12, from.y))]);
    assert!(e.h.state.dragging());
    let to = Point::new(900, 400); // inside the right pane's listing
    e.frame(vec![Event::MouseMove(to)]);
    e.frame(vec![Event::MouseUp(MouseButton::Left, to)]);
    e.until("transfer done", all_done);
    assert_eq!(std::fs::read(b.path().join("x.txt")).unwrap(), b"new x");
    e.until("right pane refreshed", |e| e.names(1).contains(&"x.txt".to_string()));
}

#[test]
fn f5_copies_directory_recursively() {
    let (a, b) = dirs();
    let mut e = env(a.path(), b.path());
    e.click("sub");
    e.key(Key::F(5), Modifiers::NONE);
    e.until("transfer done", all_done);
    assert_eq!(std::fs::read(b.path().join("sub").join("deep.txt")).unwrap(), b"deep");
}

#[test]
fn overwrite_prompt_skip_all() {
    let (a, b) = dirs();
    std::fs::write(b.path().join("x.txt"), b"old x").unwrap();
    std::fs::write(b.path().join("y.txt"), b"old y").unwrap();
    let mut e = env(a.path(), b.path());
    e.click("x.txt");
    let y = e.find("y.txt");
    e.frame(vec![
        Event::Modifiers(CTRL),
        Event::MouseMove(y),
        Event::MouseDown(MouseButton::Left, y),
        Event::MouseUp(MouseButton::Left, y),
    ]);
    e.frame(vec![Event::Modifiers(Modifiers::NONE)]);
    e.click("Copy \u{2192}");
    e.until("overwrite question", |e| e.tab.transfers.rows.iter().any(|r| matches!(r.state, RowState::Waiting(_))));
    assert!(e.texts().iter().any(|t| t.contains("exists")), "{:?}", e.texts());
    e.click("Apply to all");
    e.click("Skip");
    e.until("transfers done", all_done);
    assert_eq!(std::fs::read(b.path().join("x.txt")).unwrap(), b"old x");
    assert_eq!(std::fs::read(b.path().join("y.txt")).unwrap(), b"old y");
}

#[test]
fn cancel_marks_row_cancelled() {
    let (a, b) = dirs();
    std::fs::write(b.path().join("x.txt"), b"old x").unwrap();
    let mut e = env(a.path(), b.path());
    e.click("x.txt");
    e.click("Copy \u{2192}");
    e.until("overwrite question", |e| e.tab.transfers.rows.iter().any(|r| matches!(r.state, RowState::Waiting(_))));
    // the row's own Cancel (the question's buttons are Overwrite / Skip)
    e.click("Cancel");
    e.until("cancelled", |e| matches!(e.tab.transfers.rows[0].state, RowState::Cancelled));
    assert_eq!(std::fs::read(b.path().join("x.txt")).unwrap(), b"old x");
}

#[test]
fn failed_transfer_retry_succeeds() {
    let (a, b) = dirs();
    let mut e = env(a.path(), b.path());
    let gone = b.path().join("target");
    std::fs::create_dir(&gone).unwrap();
    e.navigate(1, &gone.display().to_string());
    let gs = gone.display().to_string();
    e.until("right in target", move |e| e.loaded(1) && e.tab.panes[1].cwd == gs);
    std::fs::remove_dir(&gone).unwrap();
    e.click("x.txt");
    e.click("Copy \u{2192}");
    e.until("failed", |e| matches!(e.tab.transfers.rows[0].state, RowState::Failed(_)));
    std::fs::create_dir(&gone).unwrap();
    e.click("Retry");
    e.until("retried", |e| matches!(e.tab.transfers.rows[0].state, RowState::Done(_)));
    assert_eq!(std::fs::read(gone.join("x.txt")).unwrap(), b"new x");
}

#[test]
fn os_file_drop_uploads() {
    let (a, b) = dirs();
    let outside = tempfile::tempdir().unwrap();
    let dropped = outside.path().join("dropped.bin");
    std::fs::write(&dropped, b"from explorer").unwrap();
    let mut e = env(a.path(), b.path());
    e.frame(vec![Event::MouseMove(Point::new(900, 400)), Event::FileDropped(dropped.clone())]);
    e.until("transfer done", all_done);
    assert_eq!(std::fs::read(b.path().join("dropped.bin")).unwrap(), b"from explorer");
}

#[test]
fn clear_finished_removes_done_rows() {
    let (a, b) = dirs();
    let mut e = env(a.path(), b.path());
    e.click("y.txt");
    e.key(Key::F(5), Modifiers::NONE);
    e.until("transfer done", all_done);
    e.click("Clear finished");
    assert!(e.tab.transfers.rows.is_empty());
}

#[test]
fn queue_runs_at_most_four_at_once() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    for i in 0..12 {
        std::fs::write(a.path().join(format!("f{i:02}.bin")), vec![1u8; 200_000]).unwrap();
    }
    let mut e = env(a.path(), b.path());
    e.tab.panes[0].sel.set = (0..12).collect();
    e.click("Copy \u{2192}");
    let mut most = 0;
    let mut saw_queued = false;
    let start = std::time::Instant::now();
    while !all_done(&e) {
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "never finished");
        let rows = &e.tab.transfers.rows;
        most = most.max(rows.iter().filter(|r| matches!(r.state, RowState::Running | RowState::Waiting(_))).count());
        saw_queued |= rows.iter().any(|r| matches!(r.state, RowState::Queued));
        e.frame(vec![]);
    }
    assert_eq!(e.tab.transfers.rows.len(), 12);
    assert!(most <= 4, "{most} ran at once");
    assert!(saw_queued);
    for i in 0..12 {
        assert!(b.path().join(format!("f{i:02}.bin")).exists());
    }
}

#[test]
fn retry_takes_the_panes_current_connection() {
    use super::transfers::Spec;
    use skiff_core::fs::endpoint::Endpoint;
    let mut spec = Spec {
        src: Endpoint::Local,
        src_path: "/x".into(),
        src_side: Some(0),
        dst: Endpoint::Local,
        dst_dir: "/y".into(),
        dst_side: 1,
        src_label: "web-01".into(),
        dst_host: "db-02".into(),
        dst_label: "db-02:/y".into(),
    };
    // Same hosts still open: both endpoints are taken from the panes.
    assert_eq!(spec.refresh_endpoints([("web-01".into(), Some(Endpoint::Local)), ("db-02".into(), Some(Endpoint::Local))]), (true, true));
    // The source pane moved to another host: keep the original source.
    assert_eq!(spec.refresh_endpoints([("other".into(), Some(Endpoint::Local)), ("db-02".into(), None)]), (false, false));
}
