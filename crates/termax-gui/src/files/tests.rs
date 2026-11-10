//! File pane tests against local temp directories (no network).

use std::path::Path;
use std::time::{Duration, Instant};

use termax_ui::display::Prim;
use termax_ui::geom::Point;
use termax_ui::input::{Event, Key, Modifiers, MouseButton};
use termax_ui::ui::testing::Harness;

use super::*;
use crate::engine::{Engine, Msg};

const CTRL: Modifiers = Modifiers { ctrl: true, shift: false, alt: false, logo: false };

pub(crate) struct Env {
    pub engine: Engine,
    pub tab: FilesTab,
    pub h: Harness,
    pub hosts: Vec<termax_core::hosts::HostEntry>,
    pub start: String,
}

#[allow(dead_code)] // helpers shared with the remote and transfer tests
impl Env {
    pub fn new(start: &Path) -> Env {
        Self::with(start, vec![], Location::Local, Location::Local)
    }

    pub fn with(start: &Path, hosts: Vec<termax_core::hosts::HostEntry>, left: Location, right: Location) -> Env {
        let mut e = Env {
            engine: Engine::new(None),
            tab: FilesTab::new(left, right),
            h: Harness::new(1200, 700, 1.0),
            hosts,
            start: start.display().to_string(),
        };
        e.frame(vec![]);
        e
    }

    pub fn ctx(&self) -> FilesCtx<'_> {
        FilesCtx { tab_id: 1, engine: &self.engine, start_dir: &self.start, hosts: &self.hosts, askpass: None, ssh_binary: "ssh" }
    }

    pub fn frame(&mut self, events: Vec<Event>) {
        for m in self.engine.drain() {
            if let Msg::Files { side, ev, .. } = m {
                let ctx = FilesCtx { tab_id: 1, engine: &self.engine, start_dir: &self.start, hosts: &self.hosts, askpass: None, ssh_binary: "ssh" };
                self.tab.on_event(side, ev, &ctx);
            }
        }
        let ctx = FilesCtx { tab_id: 1, engine: &self.engine, start_dir: &self.start, hosts: &self.hosts, askpass: None, ssh_binary: "ssh" };
        let tab = &mut self.tab;
        self.h.frame(events, &mut |ui| view(ui, ui.window_rect(), tab, &ctx));
    }

    /// Runs frames until `cond` holds (engine work is asynchronous).
    pub fn until(&mut self, what: &str, cond: impl Fn(&Env) -> bool) {
        let start = Instant::now();
        loop {
            self.frame(vec![]);
            if cond(self) {
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}; texts: {:?}", self.texts());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn texts(&self) -> Vec<String> {
        self.h.list().prims.iter().filter_map(|p| if let Prim::Text { text, .. } = p { Some(text.clone()) } else { None }).collect()
    }

    pub fn find(&self, s: &str) -> Point {
        self.h
            .list()
            .prims
            .iter()
            .find_map(|p| match p {
                Prim::Text { text, origin, .. } if text == s => Some(Point::new(origin.x + 4, origin.y + 6)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text {s:?}; have {:?}", self.texts()))
    }

    /// Like `find`, restricted to the right half of the window.
    pub fn find_right(&self, s: &str) -> Point {
        self.h
            .list()
            .prims
            .iter()
            .find_map(|p| match p {
                Prim::Text { text, origin, .. } if text == s && origin.x > 600 => Some(Point::new(origin.x + 4, origin.y + 6)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text {s:?} on the right; have {:?}", self.texts()))
    }

    pub fn click_at(&mut self, p: Point) {
        self.frame(vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]);
        self.frame(vec![]);
    }

    pub fn click(&mut self, s: &str) {
        let p = self.find(s);
        self.click_at(p);
    }

    pub fn double_click(&mut self, s: &str) {
        let p = self.find(s);
        let c = vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)];
        self.frame(c.clone());
        self.frame(c);
        self.frame(vec![]);
    }

    /// A key press plus the follow-up frame the app requests after input.
    pub fn key(&mut self, k: Key, mods: Modifiers) {
        self.frame(vec![Event::KeyDown { key: k, mods, text: None }]);
        self.frame(vec![]);
    }

    pub fn typed(&mut self, s: &str) {
        self.frame(vec![Event::Text(s.into())]);
    }

    pub fn names(&self, side: usize) -> Vec<String> {
        self.tab.panes[side].visible().iter().map(|e| e.name.clone()).collect()
    }

    pub fn loaded(&self, side: usize) -> bool {
        matches!(self.tab.panes[side].state, PaneState::Idle) && self.tab.panes[side].ep.is_some()
    }
}

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
    let ctx = FilesCtx { tab_id: 1, engine: &e.engine, start_dir: &e.start, hosts: &e.hosts, askpass: None, ssh_binary: "ssh" };
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
