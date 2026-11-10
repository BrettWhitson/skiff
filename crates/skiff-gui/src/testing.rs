//! Test driver for file tabs (used by unit and integration tests): runs a
//! `FilesTab` in the toolkit's offscreen Harness and pumps engine messages.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use skiff_core::askpass::Broker;
use skiff_core::hosts::HostEntry;
use skiff_ui::display::Prim;
use skiff_ui::geom::Point;
use skiff_ui::input::{Event, Key, Modifiers, MouseButton};
use skiff_ui::ui::testing::Harness;

use crate::engine::{Engine, Msg};
use crate::files::{FilesCtx, FilesTab, Location, PaneState, view};

/// One file-transfer tab (id 1) driven headlessly, with its own engine.
pub struct Env {
    /// Background runtime the tab's work runs on (no waker).
    pub engine: Engine,
    /// The tab under test.
    pub tab: FilesTab,
    /// Offscreen 1200x700 window that records what was drawn.
    pub h: Harness,
    /// Hosts the panes can pick from.
    pub hosts: Vec<HostEntry>,
    /// Folder local panes start in.
    pub start: String,
    /// Askpass broker, when prompts are enabled.
    pub broker: Option<Broker>,
    /// What `ssh -V` reports (tests override it).
    pub ssh_version: Option<(u32, u32)>,
}

impl Env {
    /// Two local panes starting in `start`, no hosts, no askpass.
    pub fn new(start: &Path) -> Env {
        Self::with(start, vec![], Location::Local, Location::Local, None)
    }

    /// `helper`: the askpass program (the skiff-gui binary) — enables prompts.
    pub fn with(start: &Path, hosts: Vec<HostEntry>, left: Location, right: Location, helper: Option<PathBuf>) -> Env {
        let engine = Engine::new(None);
        let broker = helper.map(|h| crate::app::start_broker(&engine, h).expect("askpass broker"));
        let mut e = Env {
            engine,
            tab: FilesTab::new(left, right),
            h: Harness::new(1200, 700, 1.0),
            hosts,
            start: start.display().to_string(),
            broker,
            ssh_version: None,
        };
        e.frame(vec![]);
        e
    }

    /// Delivers queued engine messages to the tab, then draws one frame with
    /// `events` as input.
    pub fn frame(&mut self, events: Vec<Event>) {
        let ctx = FilesCtx {
            tab_id: 1,
            engine: &self.engine,
            start_dir: &self.start,
            hosts: &self.hosts,
            askpass: self.broker.as_ref(),
            ssh_binary: "ssh",
            ssh_version: self.ssh_version,
        };
        for m in self.engine.drain() {
            match m {
                Msg::Files { side, ev, .. } => self.tab.on_event(side, ev, &ctx),
                Msg::Ask(req) => crate::files::route_ask(&mut self.tab, 1, req),
                Msg::Transfer { ev, .. } => self.tab.on_transfer(ev, &ctx),
                _ => {}
            }
        }
        let tab = &mut self.tab;
        self.h.frame(events, &mut |ui| view(ui, ui.window_rect(), tab, &ctx));
    }

    /// Runs frames until `cond` holds (engine work is asynchronous); panics
    /// after 25 s naming `what`.
    pub fn until(&mut self, what: &str, cond: impl Fn(&Env) -> bool) {
        let start = Instant::now();
        loop {
            // SKIFF_TEST_TRACE=1 logs pane states each poll, for debugging hangs.
            if std::env::var_os("SKIFF_TEST_TRACE").is_some() {
                eprintln!(
                    "until {what}: {:?} state={:?} prompt={}",
                    start.elapsed(),
                    self.tab.panes.iter().map(|p| p.state.clone()).collect::<Vec<_>>(),
                    self.tab.panes[1].prompt.is_some()
                );
            }
            self.frame(vec![]);
            if cond(self) {
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(25), "timed out waiting for {what}; texts: {:?}", self.texts());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Every text string drawn in the last frame, in draw order.
    pub fn texts(&self) -> Vec<String> {
        self.h.list().prims.iter().filter_map(|p| if let Prim::Text { text, .. } = p { Some(text.clone()) } else { None }).collect()
    }

    /// A point just inside the first drawn text equal to `s` whose origin
    /// passes `ok`.
    fn find_where(&self, s: &str, ok: impl Fn(Point) -> bool) -> Option<Point> {
        self.h.list().prims.iter().find_map(|p| match p {
            Prim::Text { text, origin, .. } if text == s && ok(*origin) => Some(Point::new(origin.x + 4, origin.y + 6)),
            _ => None,
        })
    }

    /// Where the text `s` is drawn; panics with the visible texts if absent.
    pub fn find(&self, s: &str) -> Point {
        self.find_where(s, |_| true).unwrap_or_else(|| panic!("no text {s:?}; have {:?}", self.texts()))
    }

    /// Like `find`, in the right half of the window.
    pub fn find_right(&self, s: &str) -> Point {
        self.find_where(s, |p| p.x > 600).unwrap_or_else(|| panic!("no text {s:?} on the right; have {:?}", self.texts()))
    }

    /// A left click at `p`, plus a follow-up frame.
    pub fn click_at(&mut self, p: Point) {
        self.frame(vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]);
        self.frame(vec![]);
    }

    /// Clicks the text `s`.
    pub fn click(&mut self, s: &str) {
        let p = self.find(s);
        self.click_at(p);
    }

    /// Two clicks at `p` on consecutive frames (read as a double click),
    /// plus a follow-up frame.
    pub fn double_click_at(&mut self, p: Point) {
        let c = vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)];
        self.frame(c.clone());
        self.frame(c);
        self.frame(vec![]);
    }

    /// Double-clicks the text `s`.
    pub fn double_click(&mut self, s: &str) {
        let p = self.find(s);
        self.double_click_at(p);
    }

    /// Clicks the last drawn text equal to `s` (a dialog's button drawn
    /// over a toolbar button with the same label).
    pub fn click_last(&mut self, s: &str) {
        let p = self.h.list().prims.iter().rev().find_map(|p| match p {
            Prim::Text { text, origin, .. } if text == s => Some(Point::new(origin.x + 4, origin.y + 6)),
            _ => None,
        });
        let p = p.unwrap_or_else(|| panic!("no text {s:?}; have {:?}", self.texts()));
        self.click_at(p);
    }

    /// Activates the control whose accessible name is `name` the way a
    /// screen reader does (a Click action), plus a follow-up frame.
    pub fn click_named(&mut self, name: &str) {
        let id = self.h.tree().find(name).map(|n| n.id).unwrap_or_else(|| {
            panic!("no control named {name:?}; have {:?}", self.h.tree().nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>())
        });
        self.frame(vec![Event::A11y { target: id, action: skiff_ui::a11y::A11yAction::Click }]);
        self.frame(vec![]);
    }

    /// Like `click_named`, for the last control with that name (the right
    /// pane's copy of a per-pane button).
    pub fn click_named_last(&mut self, name: &str) {
        let id = self.h.tree().nodes.iter().rev().find(|n| n.name == name).map(|n| n.id);
        let id = id.unwrap_or_else(|| panic!("no control named {name:?}"));
        self.frame(vec![Event::A11y { target: id, action: skiff_ui::a11y::A11yAction::Click }]);
        self.frame(vec![]);
    }

    /// A key press plus the follow-up frame the app requests after input.
    pub fn key(&mut self, k: Key, mods: Modifiers) {
        self.frame(vec![Event::KeyDown { key: k, mods, text: None }]);
        self.frame(vec![]);
    }

    /// Types `s` into whatever has focus (one frame).
    pub fn typed(&mut self, s: &str) {
        self.frame(vec![Event::Text(s.into())]);
    }

    /// Points pane `side` at `path`.
    pub fn navigate(&mut self, side: usize, path: &str) {
        let ctx = FilesCtx {
            tab_id: 1,
            engine: &self.engine,
            start_dir: &self.start,
            hosts: &self.hosts,
            askpass: self.broker.as_ref(),
            ssh_binary: "ssh",
            ssh_version: self.ssh_version,
        };
        self.tab.panes[side].navigate(path.to_string(), &ctx, side);
    }

    /// File names pane `side` currently lists (after filtering).
    pub fn names(&self, side: usize) -> Vec<String> {
        self.tab.panes[side].visible().iter().map(|e| e.name.clone()).collect()
    }

    /// Pane `side` is connected and has finished loading its folder.
    pub fn loaded(&self, side: usize) -> bool {
        matches!(self.tab.panes[side].state, PaneState::Idle) && self.tab.panes[side].ep.is_some()
    }
}
