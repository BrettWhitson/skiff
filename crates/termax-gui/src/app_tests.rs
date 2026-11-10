//! App-level tests driven through the toolkit Harness. No network: the
//! config disables `~/.ssh/config`; the only terminal spawned goes to the
//! local test server (`switcher_runs_snippet`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use termax_ui::display::Prim;
use termax_ui::geom::Point;
use termax_ui::input::{Event, Key, Modifiers, MouseButton};
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

pub(crate) const CONFIG: &str = "# my hand-written comment\n[settings]\nload_ssh_config = false\ntheme = \"midnight\"\nssh_binary = \"termax-test-no-such-ssh\"\n\n[[connection]]\nname = \"alpha\"\nhost = \"127.0.0.1\"\nport = 1\n\n[[connection]]\nname = \"beta\"\nhost = \"127.0.0.2\"\nport = 1\n";

impl Fixture {
    pub fn new() -> Fixture {
        Self::with_config(CONFIG)
    }

    pub fn with_config(text: &str) -> Fixture {
        Self::with_config_in(tempfile::tempdir().unwrap(), text)
    }

    /// Like `with_config`, in `dir` (which may hold other files the config names).
    pub fn with_config_in(dir: tempfile::TempDir, text: &str) -> Fixture {
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
        self.h.style = app.style.clone(); // as `GuiApp::frame` does
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
    assert!(matches!(ft.panes[0].loc, Location::Local));
    assert!(matches!(&ft.panes[1].loc, Location::Host(h) if h.name == "beta"));
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
    assert!(f.app.cfg.connections.iter().any(|c| c.name == "alpha-copy"));
    assert!(std::fs::read_to_string(&f.path).unwrap().contains("alpha-copy"));
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

// ----- tab lifecycle (Task 12) --------------------------------------------

fn pump(f: &mut Fixture, what: &str, cond: impl Fn(&GuiApp) -> bool) {
    let start = std::time::Instant::now();
    while !cond(&f.app) {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "timed out: {what}");
        f.frame(vec![]);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn files_tab(app: &mut GuiApp, id: u64) -> &mut crate::files::FilesTab {
    match &mut app.tabs.iter_mut().find(|t| t.id == id).unwrap().kind {
        TabKind::Files(f) => f,
        _ => panic!("not files"),
    }
}

/// A files tab (both panes local) with one transfer waiting on an overwrite question.
fn waiting_transfer(f: &mut Fixture) -> (u64, tempfile::TempDir, tempfile::TempDir) {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("x.txt"), b"new").unwrap();
    std::fs::write(b.path().join("x.txt"), b"old").unwrap();
    f.app.start_dir = a.path().display().to_string();
    let id = f.app.open_files(Location::Local, Location::Local);
    pump(f, "panes loaded", |app| {
        app.tabs.iter().any(|t| matches!(&t.kind, TabKind::Files(ft) if ft.panes.iter().all(|p| p.ep.is_some() && p.state == crate::files::PaneState::Idle)))
    });
    let bdir = b.path().display().to_string();
    {
        let app = &mut f.app;
        let ctx = crate::files::FilesCtx {
            tab_id: id,
            engine: &app.engine,
            start_dir: &app.start_dir,
            hosts: &app.hosts,
            askpass: None,
            ssh_binary: "ssh",
            ssh_version: None,
        };
        let ft = match &mut app.tabs.iter_mut().find(|t| t.id == id).unwrap().kind {
            TabKind::Files(ft) => ft,
            _ => unreachable!(),
        };
        ft.panes[1].navigate(bdir.clone(), &ctx, 1);
    }
    pump(f, "right in b", move |app| {
        app.tabs
            .iter()
            .any(|t| matches!(&t.kind, TabKind::Files(ft) if ft.panes[1].cwd == bdir && ft.panes[1].state == crate::files::PaneState::Idle))
    });
    {
        let app = &mut f.app;
        let ctx = crate::files::FilesCtx {
            tab_id: id,
            engine: &app.engine,
            start_dir: &app.start_dir,
            hosts: &app.hosts,
            askpass: None,
            ssh_binary: "ssh",
            ssh_version: None,
        };
        let ft = match &mut app.tabs.iter_mut().find(|t| t.id == id).unwrap().kind {
            TabKind::Files(ft) => ft,
            _ => unreachable!(),
        };
        let path = a.path().join("x.txt").display().to_string();
        ft.copy(0, vec![(path, false, "x.txt".into())], &ctx);
    }
    pump(f, "question", move |app| {
        app.tabs.iter().any(|t| matches!(&t.kind, TabKind::Files(ft) if ft.transfers.running() == 1 && ft.transfers.rows.iter().any(|r| matches!(r.state, crate::files::transfers::RowState::Waiting(_)))))
    });
    (id, a, b)
}

#[test]
fn close_with_running_transfer_asks_first() {
    let mut f = Fixture::new();
    let (id, _a, b) = waiting_transfer(&mut f);
    f.app.request_close_tab(id);
    f.frame(vec![]);
    assert_eq!(f.app.tabs.len(), 1, "not closed yet");
    assert!(f.texts().iter().any(|t| t.contains("1 transfer running")), "{:?}", f.texts());
    assert_eq!(f.h.state.focused(), Some(Id::new("close-keep")), "the question takes focus, on its safe answer");
    f.click_text("Keep");
    assert_eq!(f.app.tabs.len(), 1);
    f.app.request_close_tab(id);
    f.frame(vec![]);
    f.click_text("Close anyway");
    assert!(f.app.tabs.is_empty());
    assert_eq!(std::fs::read(b.path().join("x.txt")).unwrap(), b"old");
}

#[test]
fn shutdown_cancels_transfers() {
    let mut f = Fixture::new();
    let (id, _a, _b) = waiting_transfer(&mut f);
    files_tab(&mut f.app, id).shutdown(None);
    pump(&mut f, "cancelled", move |app| {
        app.tabs.iter().any(
            |t| matches!(&t.kind, TabKind::Files(ft) if matches!(ft.transfers.rows[0].state, crate::files::transfers::RowState::Cancelled)),
        )
    });
}

#[test]
fn close_tab_cancels_and_ignores_late_events() {
    let mut f = Fixture::new();
    let id = f.app.open_files(Location::Local, Location::Local);
    f.frame(vec![]);
    f.app.close_tab(id);
    let tx = f.app.engine.sender();
    tx.send(Msg::Files { tab: id, side: 0, ev: crate::files::FilesEvent::Listed { seq: 1, cwd: "x".into(), result: Ok(vec![]) } });
    tx.send(Msg::Transfer {
        tab: id,
        ev: termax_core::fs::transfer::TransferEvent::Progress { id: 1, done: 1, total: None, current: "x".into() },
    });
    f.frame(vec![]);
    f.frame(vec![]);
    assert!(f.app.tabs.is_empty());
}

#[test]
fn late_ask_for_gone_tab_is_refused() {
    let mut f = Fixture::new();
    let (reply, rx) = tokio::sync::oneshot::channel();
    let req = termax_core::askpass::AskRequest {
        tag: "999:1".into(),
        prompt: "Password: ".into(),
        kind: termax_core::askpass::AskKind::Secret,
        reply,
        cancelled: tokio_util::sync::CancellationToken::new(),
    };
    f.app.engine.sender().send(Msg::Ask(req));
    f.frame(vec![]);
    assert_eq!(rx.blocking_recv().unwrap(), None);
}

#[test]
fn show_hidden_setting_seeds_file_panes() {
    let mut f = Fixture::with_config(&CONFIG.replace("load_ssh_config = false", "load_ssh_config = false\nshow_hidden_files = true"));
    let id = f.app.open_files(Location::Local, Location::Local);
    let TabKind::Files(ft) = &f.app.tabs.iter().find(|t| t.id == id).unwrap().kind else { panic!() };
    assert!(ft.panes.iter().all(|p| p.show_hidden));
}

#[test]
fn f6_cycles_regions_from_terminal() {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    f.frame(vec![]);
    let term = crate::term::split::pane_id(f.app.tabs[0].id, 0);
    assert_eq!(f.h.state.focused(), Some(term));
    let f6 = |shift| Event::KeyDown { key: Key::F(6), mods: Modifiers { shift, ..Modifiers::NONE }, text: None };
    f.frame(vec![f6(false)]);
    f.frame(vec![]);
    assert!(f.h.state.focused().is_some_and(|id| id != term), "F6 leaves the terminal");
    f.frame(vec![f6(true)]);
    f.frame(vec![]);
    assert_eq!(f.h.state.focused(), Some(term), "Shift+F6 comes back");
}

#[test]
fn tab_strip_arrows_keep_focus_on_strip() {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    f.frame(vec![]);
    f.double_click_text("beta");
    f.frame(vec![]);
    assert_eq!(f.app.active, 1);
    let key = |key, shift| Event::KeyDown { key, mods: Modifiers { shift, ..Modifiers::NONE }, text: None };
    f.frame(vec![key(Key::F(6), true)]);
    f.frame(vec![]);
    assert_eq!(f.h.state.focused(), Some(Id::new("tabs")));
    f.frame(vec![key(Key::Left, false)]);
    f.frame(vec![]);
    assert_eq!(f.app.active, 0);
    assert_eq!(f.h.state.focused(), Some(Id::new("tabs")), "arrows stay on the strip, not the shell");
}

#[test]
fn shift_f10_opens_host_menu() {
    let mut f = Fixture::new();
    f.click_text("alpha");
    f.frame(vec![Event::KeyDown { key: Key::F(10), mods: Modifiers { shift: true, ..Modifiers::NONE }, text: None }]);
    f.frame(vec![]);
    assert!(f.try_find("Open terminal").is_some(), "{:?}", f.texts());
}

#[test]
fn sidebar_tree_names_hosts() {
    let f = Fixture::new();
    let t = f.h.tree();
    let names: Vec<&str> = t.nodes.iter().map(|n| n.name.as_str()).collect();
    let row = t.find("alpha, not checked, 127.0.0.1:1").unwrap_or_else(|| panic!("{names:?}"));
    assert_eq!(row.role, termax_ui::a11y::Role::ListItem);
    assert_eq!(t.find("Hosts").map(|n| n.role), Some(termax_ui::a11y::Role::Region));
}

#[test]
fn a11y_click_opens_host() {
    use termax_ui::a11y::A11yAction;
    let mut f = Fixture::new();
    let row = f.h.tree().find("alpha, not checked, 127.0.0.1:1").expect("row").id;
    let click = Event::A11y { target: row, action: A11yAction::Click };
    f.frame(vec![click]);
    f.frame(vec![]);
    assert!(matches!(f.app.tabs.first().map(|t| &t.kind), Some(TabKind::Terminal(_))), "one Click opens an unselected host");
    assert_eq!(f.app.tabs.len(), 1);
    assert_eq!(f.h.tree().find("Terminal: alpha").map(|n| n.role), Some(termax_ui::a11y::Role::Terminal));
}

#[test]
fn settings_inputs_have_labels() {
    let mut f = Fixture::new();
    f.app.open_settings();
    f.frame(vec![]);
    f.click_text("Connections");
    f.click_text("Add");
    let t = f.h.tree();
    assert_eq!(t.find("Name").map(|n| n.role), Some(termax_ui::a11y::Role::TextInput));
}

#[test]
fn ui_scale_shortcuts_persist() {
    let mut f = Fixture::new();
    let ctrl_alt = Modifiers { ctrl: true, alt: true, ..Modifiers::NONE };
    let up = Event::KeyDown { key: Key::Char('='), mods: ctrl_alt, text: None };
    f.frame(vec![up.clone()]);
    f.frame(vec![up]);
    assert_eq!(f.app.cfg.gui.ui_scale, 1.2);
    assert!(std::fs::read_to_string(&f.path).unwrap().contains("ui_scale = 1.2"));
    // AltGr (Ctrl+Alt) that types a character is typing, not a shortcut.
    f.frame(vec![Event::KeyDown { key: Key::Char('0'), mods: ctrl_alt, text: Some("}".into()) }]);
    assert_eq!(f.app.cfg.gui.ui_scale, 1.2);
    f.frame(vec![Event::KeyDown { key: Key::Char('0'), mods: ctrl_alt, text: None }]);
    assert_eq!(f.app.cfg.gui.ui_scale, 1.0);
}

#[test]
fn switcher_runs_snippet() {
    let cfg = CONFIG.replace("[[connection]]", "[[snippet]]\nname = \"greet\"\ncommand = \"echo snippet-ran\"\n\n[[connection]]");
    let mut f = Fixture::with_config(&cfg);
    // A terminal on the local test server (never a real host).
    let fx = termax_testserver::TestFixture::with_shell(termax_testserver::FixtureAuth::Key).unwrap();
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let host = f.app.hosts[0].clone();
    f.app.push_tab(TabKind::Terminal(Box::new(TermSplit::new(TermTab::with_command(host, "ssh".into(), args, 500)))));
    f.frame(vec![]);
    let screen = |f: &Fixture| match &f.app.tabs[f.app.active].kind {
        TabKind::Terminal(t) => t.focused().session.as_ref().map(|s| s.parser.lock().unwrap().screen().contents()).unwrap_or_default(),
        _ => String::new(),
    };
    let wait = |f: &mut Fixture, s: &str| {
        let start = Instant::now();
        while !screen(f).contains(s) {
            assert!(start.elapsed() < Duration::from_secs(15), "waiting for {s:?}: {}", screen(f));
            std::thread::sleep(Duration::from_millis(30));
            f.frame(vec![]);
        }
    };
    wait(&mut f, "termax-testserver shell");
    f.app.do_action(Action::QuickSwitcher);
    f.frame(vec![]);
    f.frame(vec![Event::Text("snippet gr".into())]);
    f.frame(vec![]);
    assert!(f.try_find("Snippet: greet").is_some(), "{:?}", f.texts());
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    wait(&mut f, "echo snippet-ran");
}

#[test]
fn repeated_warning_is_announced_again() {
    let mut f = Fixture::new();
    let status = |f: &Fixture| f.h.tree().nodes.iter().find(|n| n.role == termax_ui::a11y::Role::Status && n.live).map(|n| n.name.clone());
    f.app.warn("disk full");
    f.frame(vec![]);
    let first = status(&f).expect("live status");
    f.app.warn("disk full");
    f.frame(vec![]);
    let second = status(&f).expect("live status");
    assert_ne!(first, second, "the same warning twice still changes the live text");
    assert_eq!(second.trim_end_matches(termax_ui::a11y::REPEAT_MARK), "disk full");
}

#[test]
fn ui_scale_scales_layout_with_text() {
    let mut f = Fixture::new();
    let pitch = |f: &Fixture| f.find("beta").y - f.find("alpha").y;
    let before = pitch(&f);
    f.app.cfg.gui.ui_scale = 2.0;
    f.app.style = style_for(&f.app.theme, &f.app.cfg);
    f.frame(vec![]);
    assert_eq!(pitch(&f), 2 * before, "rows grow with the text, so large text is not clipped");
    let mono = f.app.style.mono_px * f.app.style.zoom;
    assert!((mono - f.app.cfg.gui.font_size.clamp(6.0, 72.0)).abs() < 1e-3, "terminal text keeps its size: {mono}");
}

#[test]
fn saved_window_size_ignores_ui_zoom() {
    let mut f = Fixture::new();
    f.app.cfg.gui.ui_scale = 2.0;
    f.app.style = style_for(&f.app.theme, &f.app.cfg);
    f.frame(vec![]);
    // main.rs restores it as a winit LogicalSize (DPI only), so zoom must not shrink it.
    assert_eq!(f.app.geometry.window_size, Some((1280, 800)));
}
