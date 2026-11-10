//! App-level tests driven through the toolkit Harness. No network: the
//! config disables `~/.ssh/config` and no terminal is ever spawned here.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use termax_ui::display::Prim;
use termax_ui::geom::Point;
use termax_ui::input::{Event, MouseButton};
use termax_ui::ui::testing::Harness;

use super::*;
use crate::engine::{Engine, Msg};
use crate::files::Location;

pub(crate) struct Fixture {
    pub _dir: tempfile::TempDir,
    pub path: std::path::PathBuf,
    pub app: GuiApp,
    pub h: Harness,
}

pub(crate) const CONFIG: &str = "# my hand-written comment\n[settings]\nload_ssh_config = false\ntheme = \"midnight\"\n\n[[connection]]\nname = \"alpha\"\nhost = \"127.0.0.1\"\nport = 1\n\n[[connection]]\nname = \"beta\"\nhost = \"127.0.0.2\"\nport = 1\n";

impl Fixture {
    pub fn new() -> Fixture {
        Self::with_config(CONFIG)
    }

    pub fn with_config(text: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        let cfg = Config::load_or_create(&path).unwrap();
        let app = GuiApp::new(cfg, path.clone(), None);
        let mut f = Fixture { _dir: dir, path, app, h: Harness::new(1280, 800, 1.0) };
        f.frame(vec![]);
        f
    }

    pub fn frame(&mut self, events: Vec<Event>) {
        let app = &mut self.app;
        self.h.frame(events, &mut |ui| app.ui(ui));
    }

    /// Top-left of the first text prim equal to `s` (panics if absent).
    pub fn find(&self, s: &str) -> Point {
        self.try_find(s).unwrap_or_else(|| panic!("no text {s:?} on screen; have {:?}", self.texts()))
    }

    pub fn try_find(&self, s: &str) -> Option<Point> {
        self.h.list().prims.iter().find_map(|p| match p {
            Prim::Text { text, origin, .. } if text == s => Some(*origin),
            _ => None,
        })
    }

    pub fn texts(&self) -> Vec<String> {
        self.h.list().prims.iter().filter_map(|p| if let Prim::Text { text, .. } = p { Some(text.clone()) } else { None }).collect()
    }

    pub fn click(&mut self, p: Point) {
        let p = Point::new(p.x + 4, p.y + 6);
        self.frame(vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]);
        self.frame(vec![]);
    }

    pub fn click_text(&mut self, s: &str) {
        let p = self.find(s);
        self.click(p);
    }

    pub fn right_click_text(&mut self, s: &str) {
        let p = self.find(s);
        let p = Point::new(p.x + 4, p.y + 6);
        self.frame(vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Right, p), Event::MouseUp(MouseButton::Right, p)]);
        self.frame(vec![]);
    }

    pub fn double_click_text(&mut self, s: &str) {
        let p = self.find(s);
        let p = Point::new(p.x + 4, p.y + 6);
        let c = vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)];
        self.frame(c.clone());
        self.frame(c);
        self.frame(vec![]);
    }
}

#[test]
fn filter_narrows_sidebar() {
    let mut f = Fixture::new();
    assert!(f.try_find("alpha").is_some() && f.try_find("beta").is_some());
    f.click_text("Filter hosts");
    f.frame(vec![Event::Text("bet".into())]);
    f.frame(vec![]);
    assert!(f.try_find("alpha").is_none(), "{:?}", f.texts());
    assert!(f.try_find("beta").is_some());
}

#[test]
fn double_click_host_opens_tab() {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    assert_eq!(f.app.tabs.len(), 1);
    assert!(f.app.tab_title(0).contains("alpha"));
    assert!(matches!(f.app.tabs[0].kind, TabKind::Terminal(_)));
    assert!(f.texts().iter().any(|t| t.contains("alpha") && t.contains('\u{25CF}')), "tab strip shows it");
}

#[test]
fn context_menu_open_file_transfer() {
    let mut f = Fixture::new();
    f.right_click_text("beta");
    f.click_text("Open file transfer");
    assert_eq!(f.app.tabs.len(), 1);
    let TabKind::Files(ft) = &f.app.tabs[0].kind else { panic!("not a files tab") };
    assert!(matches!(ft.locations[0], Location::Local));
    assert!(matches!(&ft.locations[1], Location::Host(h) if h.name == "beta"));
}

#[test]
fn sidebar_width_persists_to_state_file() {
    let mut f = Fixture::new();
    let x = f.app.geometry.sidebar_width as i32 + 2; // inside the 5 px splitter gap
    let (a, b) = (Point::new(x, 300), Point::new(x + 80, 300));
    f.frame(vec![Event::MouseMove(a), Event::MouseDown(MouseButton::Left, a)]);
    f.frame(vec![Event::MouseMove(b)]);
    f.frame(vec![Event::MouseUp(MouseButton::Left, b)]);
    f.app.flush_geometry();
    let saved = termax_core::state::UiState::load(&f.app.geometry_path);
    assert!((saved.sidebar_width - 320.0).abs() <= 2.0, "{}", saved.sidebar_width);
    // config.toml untouched by geometry
    assert_eq!(std::fs::read_to_string(&f.path).unwrap(), CONFIG);
}

#[test]
fn delete_custom_connection_saves_config() {
    let mut f = Fixture::new();
    f.right_click_text("beta");
    f.click_text("Delete");
    assert!(f.texts().iter().any(|t| t.contains("Delete beta?")), "{:?}", f.texts());
    f.click_text("Delete");
    let text = std::fs::read_to_string(&f.path).unwrap();
    assert!(!text.contains("beta"), "{text}");
    assert!(text.contains("# my hand-written comment"));
    assert!(f.try_find("beta").is_none());
    assert_eq!(f.app.hosts.len(), 1);
}

#[test]
fn duplicate_connection_gets_unique_name() {
    let mut f = Fixture::new();
    f.right_click_text("alpha");
    f.click_text("Duplicate");
    assert!(f.app.cfg.connections.iter().any(|c| c.name == "alpha (copy)"));
    assert!(std::fs::read_to_string(&f.path).unwrap().contains("alpha (copy)"));
}

#[test]
fn engine_message_wakes_and_is_drained() {
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    let engine = Engine::new(Some(Arc::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
    })));
    let tx = engine.sender();
    std::thread::spawn(move || tx.send(Msg::Notice { tab: None, text: "hello from a worker".into() })).join().unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert_eq!(engine.drain().len(), 1);
    assert!(engine.drain().is_empty());

    let mut f = Fixture::new();
    f.app.engine.sender().send(Msg::Notice { tab: None, text: "background notice".into() });
    f.frame(vec![]);
    assert!(f.texts().iter().any(|t| t.contains("background notice")), "{:?}", f.texts());
}

#[test]
fn close_tab_shortcut_and_toggle_sidebar() {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    let ctrl_shift = termax_ui::input::Modifiers { ctrl: true, shift: true, alt: false, logo: false };
    f.frame(vec![Event::KeyDown { key: termax_ui::input::Key::Char('w'), mods: ctrl_shift, text: None }]);
    assert!(f.app.tabs.is_empty());
    f.frame(vec![Event::KeyDown { key: termax_ui::input::Key::Char('b'), mods: ctrl_shift, text: None }]);
    f.frame(vec![]);
    assert!(f.app.geometry.sidebar_collapsed);
    assert!(f.try_find("alpha").is_none());
}
