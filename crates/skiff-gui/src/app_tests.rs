//! App-level tests driven through the toolkit Harness. No network: the
//! config disables `~/.ssh/config` and names a non-existent ssh binary; the
//! only real ssh session goes to the local test server
//! (`palette_runs_snippet`). `Fixture` is shared with other test modules.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use skiff_ui::display::Prim;
use skiff_ui::geom::Point;
use skiff_ui::input::{Event, Key, Modifiers, MouseButton};
use skiff_ui::ui::testing::Harness;

use super::*;
use crate::engine::{Engine, Msg};
use crate::files::Location;

/// A `GuiApp` on a temporary config, rendered by a headless 1280x800 harness.
pub(crate) struct Fixture {
    /// Keeps the temporary directory (and the config in it) alive.
    pub _dir: tempfile::TempDir,
    /// The `config.toml` path.
    pub path: std::path::PathBuf,
    /// The app under test.
    pub app: GuiApp,
    /// The toolkit test harness that runs frames and records output.
    pub h: Harness,
}

/// Base config: ssh config off, a missing ssh binary, and two custom hosts
/// (`alpha`, `beta`) on unreachable loopback ports.
pub(crate) const CONFIG: &str = "# my hand-written comment\n[settings]\nload_ssh_config = false\ntheme = \"midnight\"\nssh_binary = \"skiff-test-no-such-ssh\"\n\n[[connection]]\nname = \"alpha\"\nhost = \"127.0.0.1\"\nport = 1\n\n[[connection]]\nname = \"beta\"\nhost = \"127.0.0.2\"\nport = 1\n";

impl Fixture {
    /// A fixture on the base `CONFIG`.
    pub fn new() -> Fixture {
        Self::with_config(CONFIG)
    }

    /// A fixture on config `text`, in a fresh temp directory.
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

    /// Runs one app frame with these input events.
    pub fn frame(&mut self, events: Vec<Event>) {
        let app = &mut self.app;
        self.h.style = app.style.clone(); // as `GuiApp::frame` does
        self.h.frame(events, &mut |ui| app.ui(ui));
    }

    /// Top-left of the first text prim equal to `s` (panics if absent).
    pub fn find(&self, s: &str) -> Point {
        self.try_find(s).unwrap_or_else(|| panic!("no text {s:?} on screen; have {:?}", self.texts()))
    }

    /// Top-left of the last text prim equal to `s` (a form label drawn after
    /// a sidebar control with the same text); panics if absent.
    pub fn find_last(&self, s: &str) -> Point {
        let hit = self.h.list().prims.iter().rev().find_map(|p| match p {
            Prim::Text { text, origin, .. } if text == s => Some(*origin),
            _ => None,
        });
        hit.unwrap_or_else(|| panic!("no text {s:?} on screen; have {:?}", self.texts()))
    }

    /// Top-left of the first text prim equal to `s`, if drawn.
    pub fn try_find(&self, s: &str) -> Option<Point> {
        self.h.list().prims.iter().find_map(|p| match p {
            Prim::Text { text, origin, .. } if text == s => Some(*origin),
            _ => None,
        })
    }

    /// Every text drawn last frame (for assertion messages and searches).
    pub fn texts(&self) -> Vec<String> {
        self.h.list().prims.iter().filter_map(|p| if let Prim::Text { text, .. } = p { Some(text.clone()) } else { None }).collect()
    }

    /// A left click just inside `p`, then one more frame to settle.
    pub fn click(&mut self, p: Point) {
        let p = Point::new(p.x + 4, p.y + 6);
        self.frame(vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)]);
        self.frame(vec![]);
    }

    /// Left-clicks the text `s`.
    pub fn click_text(&mut self, s: &str) {
        let p = self.find(s);
        self.click(p);
    }

    /// Right-clicks the text `s` (opens context menus).
    pub fn right_click_text(&mut self, s: &str) {
        let p = self.find(s);
        let p = Point::new(p.x + 4, p.y + 6);
        self.frame(vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Right, p), Event::MouseUp(MouseButton::Right, p)]);
        self.frame(vec![]);
    }

    /// Double-clicks the text `s` (two clicks in consecutive frames).
    pub fn double_click_text(&mut self, s: &str) {
        let p = self.find(s);
        let p = Point::new(p.x + 4, p.y + 6);
        let c = vec![Event::MouseMove(p), Event::MouseDown(MouseButton::Left, p), Event::MouseUp(MouseButton::Left, p)];
        self.frame(c.clone());
        self.frame(c);
        self.frame(vec![]);
    }
}

/// Typing in the filter box hides hosts that don't match.
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

/// Double-clicking a host opens a terminal tab titled with its name.
#[test]
fn double_click_host_opens_tab() {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    assert_eq!(f.app.tabs.len(), 1);
    assert!(f.app.tab_title(0).contains("alpha"));
    assert!(matches!(f.app.tabs[0].kind, TabKind::Terminal(_)));
    let tab = f.h.tree().nodes.iter().find(|n| n.role == skiff_ui::a11y::Role::Tab).map(|n| n.name.clone());
    assert!(tab.as_deref().is_some_and(|t| t.starts_with("alpha, ")), "the tab names its host and state: {tab:?}");
}

/// "Open file transfer" in the host menu opens a Local / host files tab.
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

/// Dragging the sidebar splitter saves the width to `state.toml`, not `config.toml`.
#[test]
fn sidebar_width_persists_to_state_file() {
    let mut f = Fixture::new();
    let x = f.app.geometry.sidebar_width as i32 + 2; // inside the 5 px splitter gap
    let (a, b) = (Point::new(x, 300), Point::new(x + 80, 300));
    f.frame(vec![Event::MouseMove(a), Event::MouseDown(MouseButton::Left, a)]);
    f.frame(vec![Event::MouseMove(b)]);
    f.frame(vec![Event::MouseUp(MouseButton::Left, b)]);
    f.app.flush_geometry();
    let saved = skiff_core::state::UiState::load(&f.app.geometry_path);
    assert!((saved.sidebar_width - 320.0).abs() <= 2.0, "{}", saved.sidebar_width);
    // config.toml untouched by geometry
    assert_eq!(std::fs::read_to_string(&f.path).unwrap(), CONFIG);
}

/// Deleting a custom host after confirming removes it from the config, keeping comments.
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

/// A delete dialog dropped because its host vanished meanwhile gives
/// focus back to the host list.
#[test]
fn vanished_host_dialog_restores_focus() {
    let mut f = Fixture::new();
    f.click_text("beta");
    let list = f.h.state.focused();
    assert!(list.is_some());
    f.frame(vec![Event::KeyDown { key: Key::Delete, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert_ne!(f.h.state.focused(), list, "the dialog has focus");
    f.app.cfg.connections.retain(|c| c.name != "beta"); // deleted elsewhere
    f.app.rebuild_hosts();
    f.frame(vec![]);
    f.frame(vec![]);
    assert!(f.app.sidebar.confirm_delete.is_none());
    assert_eq!(f.h.state.focused(), list);
}

/// Duplicate adds `<name>-copy` and saves it.
#[test]
fn duplicate_connection_gets_unique_name() {
    let mut f = Fixture::new();
    f.right_click_text("alpha");
    f.click_text("Duplicate");
    assert!(f.app.cfg.connections.iter().any(|c| c.name == "alpha-copy"));
    assert!(std::fs::read_to_string(&f.path).unwrap().contains("alpha-copy"));
}

/// An engine message wakes the UI once, drains once, and a notice reaches the status bar.
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

/// Ctrl+Shift+W closes the tab and Ctrl+Shift+B hides the sidebar.
#[test]
fn close_tab_shortcut_and_toggle_sidebar() {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    let ctrl_shift = skiff_ui::input::Modifiers { ctrl: true, shift: true, alt: false, logo: false };
    f.frame(vec![Event::KeyDown { key: skiff_ui::input::Key::Char('w'), mods: ctrl_shift, text: None }]);
    assert!(f.app.tabs.is_empty());
    f.frame(vec![Event::KeyDown { key: skiff_ui::input::Key::Char('b'), mods: ctrl_shift, text: None }]);
    f.frame(vec![]);
    assert!(f.app.geometry.sidebar_collapsed);
    assert!(f.try_find("alpha").is_none());
}

// ----- tab lifecycle (Task 12) --------------------------------------------

/// Runs frames until `cond` holds, failing after 20 s with `what`.
fn pump(f: &mut Fixture, what: &str, cond: impl Fn(&GuiApp) -> bool) {
    let start = std::time::Instant::now();
    while !cond(&f.app) {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "timed out: {what}");
        f.frame(vec![]);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The files tab with id `id` (panics if it is something else).
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
    // Point the right pane at `b`, then copy `x.txt` across so it hits the
    // existing file and waits on the overwrite question.
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

/// Closing a tab with a running transfer asks first (focus on Keep); Close anyway cancels it.
#[test]
fn close_with_running_transfer_asks_first() {
    let mut f = Fixture::new();
    let (id, _a, b) = waiting_transfer(&mut f);
    f.app.request_close_tab(id);
    f.frame(vec![]);
    assert_eq!(f.app.tabs.len(), 1, "not closed yet");
    assert!(f.texts().iter().any(|t| t.contains("1 transfer still running")), "{:?}", f.texts());
    let keep = Id::new("close-confirm").with(("button", 0usize));
    assert_eq!(f.h.state.focused(), Some(keep), "the dialog takes focus, on its safe answer");
    f.click_text("Keep");
    assert_eq!(f.app.tabs.len(), 1);
    f.app.request_close_tab(id);
    f.frame(vec![]);
    f.click_text("Close anyway");
    assert!(f.app.tabs.is_empty());
    assert_eq!(std::fs::read(b.path().join("x.txt")).unwrap(), b"old");
}

/// App shortcuts stand aside while a modal is open: Ctrl+Tab neither
/// abandons the close dialog nor switches tabs, Ctrl+Shift+P opens no
/// palette behind it; the palette is itself a modal dialog.
#[test]
fn shortcuts_stand_aside_while_a_modal_is_open() {
    let mut f = Fixture::new();
    let (id, _a, _b) = waiting_transfer(&mut f);
    f.app.do_action(Action::Settings);
    f.app.active = f.app.tabs.iter().position(|t| t.id == id).unwrap();
    f.app.request_close_tab(id);
    f.frame(vec![]);
    let keep = Id::new("close-confirm").with(("button", 0usize));
    assert_eq!(f.h.state.focused(), Some(keep));
    let ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
    let active = f.app.active;
    f.frame(vec![Event::KeyDown { key: Key::Tab, mods: ctrl, text: None }]);
    f.frame(vec![Event::KeyDown { key: Key::Char('p'), mods: Modifiers { shift: true, ..ctrl }, text: None }]);
    f.frame(vec![]);
    assert_eq!(f.app.active, active, "Ctrl+Tab did not switch tabs");
    assert!(!f.app.palette_open, "no palette behind the dialog");
    assert_eq!(f.h.state.focused(), Some(keep), "the dialog keeps focus");
    f.click_text("Keep");
    f.app.do_action(Action::QuickSwitcher);
    f.frame(vec![]);
    let node = f.h.tree().find("Command palette").expect("palette node");
    assert!(node.modal, "{node:?}");
    f.frame(vec![Event::KeyDown { key: Key::Tab, mods: ctrl, text: None }]);
    assert_eq!(f.app.active, active, "shortcuts wait for the palette too");
}

/// Shutting a files tab down cancels its waiting transfer.
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

/// Engine events for a closed tab are dropped without panicking.
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
        ev: skiff_core::fs::transfer::TransferEvent::Progress { id: 1, done: 1, total: None, current: "x".into() },
    });
    f.frame(vec![]);
    f.frame(vec![]);
    assert!(f.app.tabs.is_empty());
}

/// An askpass question for a tab that no longer exists is answered with `None`.
#[test]
fn late_ask_for_gone_tab_is_refused() {
    let mut f = Fixture::new();
    let (reply, rx) = tokio::sync::oneshot::channel();
    let req = skiff_core::askpass::AskRequest {
        tag: "999:1".into(),
        prompt: "Password: ".into(),
        kind: skiff_core::askpass::AskKind::Secret,
        reply,
        cancelled: tokio_util::sync::CancellationToken::new(),
    };
    f.app.engine.sender().send(Msg::Ask(req));
    f.frame(vec![]);
    assert_eq!(rx.blocking_recv().unwrap(), None);
}

/// `show_hidden_files` starts new file panes with hidden files shown.
#[test]
fn show_hidden_setting_seeds_file_panes() {
    let mut f = Fixture::with_config(&CONFIG.replace("load_ssh_config = false", "load_ssh_config = false\nshow_hidden_files = true"));
    let id = f.app.open_files(Location::Local, Location::Local);
    let TabKind::Files(ft) = &f.app.tabs.iter().find(|t| t.id == id).unwrap().kind else { panic!() };
    assert!(ft.panes.iter().all(|p| p.show_hidden));
}

/// F6 moves focus out of the terminal and Shift+F6 brings it back.
#[test]
fn f6_cycles_regions_from_terminal() {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    f.frame(vec![]);
    let term = crate::term::term_id(f.app.tabs[0].id);
    assert_eq!(f.h.state.focused(), Some(term));
    let f6 = |shift| Event::KeyDown { key: Key::F(6), mods: Modifiers { shift, ..Modifiers::NONE }, text: None };
    f.frame(vec![f6(false)]);
    f.frame(vec![]);
    assert!(f.h.state.focused().is_some_and(|id| id != term), "F6 leaves the terminal");
    f.frame(vec![f6(true)]);
    f.frame(vec![]);
    assert_eq!(f.h.state.focused(), Some(term), "Shift+F6 comes back");
}

/// Arrow keys on the focused tab strip switch tabs without moving focus to the terminal.
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
    let strip = crate::dock::strip_id(f.app.dock.focus);
    assert_eq!(f.h.state.focused(), Some(strip));
    f.frame(vec![key(Key::Left, false)]);
    f.frame(vec![]);
    assert_eq!(f.app.active, 0);
    assert_eq!(f.h.state.focused(), Some(strip), "arrows stay on the strip, not the shell");
}

/// Shift+F10 on a selected host opens its context menu.
#[test]
fn shift_f10_opens_host_menu() {
    let mut f = Fixture::new();
    f.click_text("alpha");
    f.frame(vec![Event::KeyDown { key: Key::F(10), mods: Modifiers { shift: true, ..Modifiers::NONE }, text: None }]);
    f.frame(vec![]);
    assert!(f.try_find("Open terminal").is_some(), "{:?}", f.texts());
}

/// The sidebar divider is a named splitter that F6 reaches and the arrow
/// keys (or a screen reader's increment) resize.
#[test]
fn sidebar_divider_is_keyboard_accessible() {
    use skiff_ui::a11y::{A11yAction, Role};
    let mut f = Fixture::new();
    let div = f.h.tree().find("Sidebar width").map(|n| (n.role, n.id));
    let Some((Role::Splitter, id)) = div else { panic!("no splitter: {div:?}") };
    let f6 = Event::KeyDown { key: Key::F(6), mods: Modifiers::NONE, text: None };
    let mut reached = false;
    for _ in 0..6 {
        f.frame(vec![f6.clone()]);
        f.frame(vec![]);
        reached |= f.h.state.focused() == Some(id);
        if reached {
            break;
        }
    }
    assert!(reached, "F6 reaches the sidebar divider");
    let before = f.app.geometry.sidebar_width;
    f.frame(vec![Event::KeyDown { key: Key::Right, mods: Modifiers::NONE, text: None }]);
    assert_eq!(f.app.geometry.sidebar_width, before + 16.0);
    f.frame(vec![Event::A11y { target: id, action: A11yAction::Decrement }]);
    assert_eq!(f.app.geometry.sidebar_width, before);
}

/// Host rows are list items named with host, status and target, inside a Hosts region.
#[test]
fn sidebar_tree_names_hosts() {
    let f = Fixture::new();
    let t = f.h.tree();
    let names: Vec<&str> = t.nodes.iter().map(|n| n.name.as_str()).collect();
    let row = t.find("alpha, not checked, 127.0.0.1:1").unwrap_or_else(|| panic!("{names:?}"));
    assert_eq!(row.role, skiff_ui::a11y::Role::ListItem);
    assert_eq!(t.find("Hosts").map(|n| n.role), Some(skiff_ui::a11y::Role::Region));
}

/// A screen reader click on an unselected host opens its terminal.
#[test]
fn a11y_click_opens_host() {
    use skiff_ui::a11y::A11yAction;
    let mut f = Fixture::new();
    let row = f.h.tree().find("alpha, not checked, 127.0.0.1:1").expect("row").id;
    let click = Event::A11y { target: row, action: A11yAction::Click };
    f.frame(vec![click]);
    f.frame(vec![]);
    assert!(matches!(f.app.tabs.first().map(|t| &t.kind), Some(TabKind::Terminal(_))), "one Click opens an unselected host");
    assert_eq!(f.app.tabs.len(), 1);
    assert_eq!(f.h.tree().find("Terminal: alpha").map(|n| n.role), Some(skiff_ui::a11y::Role::Terminal));
}

/// The new-connection form's Name field has an accessible label.
#[test]
fn settings_inputs_have_labels() {
    let mut f = Fixture::new();
    f.app.open_settings();
    f.frame(vec![]);
    f.click_text("Connections");
    f.click_text("Add");
    let t = f.h.tree();
    assert_eq!(t.find("Name").map(|n| n.role), Some(skiff_ui::a11y::Role::TextInput));
}

/// Ctrl+Alt+= / 0 change and save `ui_scale`; AltGr typing does not.
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

/// Choosing a snippet in the command palette types it into the active terminal.
#[test]
fn palette_runs_snippet() {
    let cfg = CONFIG.replace("[[connection]]", "[[snippet]]\nname = \"greet\"\ncommand = \"echo snippet-ran\"\n\n[[connection]]");
    let mut f = Fixture::with_config(&cfg);
    // A terminal on the local test server (never a real host).
    let fx = skiff_testserver::TestFixture::with_shell(skiff_testserver::FixtureAuth::Key).unwrap();
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let host = f.app.hosts[0].clone();
    f.app.push_tab(TabKind::Terminal(Box::new(TermTab::with_command(host, "ssh".into(), args, 500))));
    f.frame(vec![]);
    let screen = |f: &Fixture| match &f.app.tabs[f.app.active].kind {
        TabKind::Terminal(t) => t.session.as_ref().map(|s| s.parser.lock().unwrap().screen().contents()).unwrap_or_default(),
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
    wait(&mut f, "skiff-testserver shell");
    f.app.do_action(Action::QuickSwitcher);
    f.frame(vec![]);
    f.frame(vec![Event::Text("#gr".into())]);
    f.frame(vec![]);
    assert!(f.h.tree().find("greet, Snippet").is_some(), "{:?}", f.texts());
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    wait(&mut f, "echo snippet-ran");
}

/// `notify` shows a toast (a live status node); the same message again
/// replaces it (a new node, so it is read out again) instead of stacking,
/// and warnings are also logged.
#[test]
fn notify_shows_toast_and_replaces_repeats() {
    let mut f = Fixture::new();
    let live = |f: &Fixture| {
        f.h.tree()
            .nodes
            .iter()
            .filter(|n| n.role == skiff_ui::a11y::Role::Status && n.live)
            .map(|n| (n.id, n.name.clone()))
            .collect::<Vec<_>>()
    };
    f.app.notify(ToastKind::Warning, "disk full");
    f.frame(vec![]);
    let first = live(&f);
    assert_eq!(first.iter().map(|n| n.1.as_str()).collect::<Vec<_>>(), ["disk full"]);
    assert!(f.try_find("disk full").is_some(), "{:?}", f.texts());
    f.app.notify(ToastKind::Warning, "disk full");
    f.frame(vec![]);
    let second = live(&f);
    assert_eq!(second.len(), 1, "a repeat replaces the toast");
    assert_ne!(first[0].0, second[0].0, "as a new live node");
    f.app.notify(ToastKind::Success, "saved");
    assert_eq!(f.app.toasts.visible().count(), 2);
    assert_eq!(f.app.warnings, ["disk full", "disk full"]);
}

/// A bad `[keys]` entry warns once at startup, not again on every save;
/// a new problem still warns.
#[test]
fn keymap_warnings_not_repeated_on_save() {
    let mut f = Fixture::with_config(&format!(
        "{CONFIG}
[keys]
close_tab = \"ctrl+nosuchkey\"
"
    ));
    let n = f.app.warnings.len();
    assert!(n >= 1, "startup warns: {:?}", f.app.warnings);
    f.app.save_config();
    f.app.save_config();
    assert_eq!(f.app.warnings.len(), n, "{:?}", f.app.warnings);
    f.app.cfg.keys.insert("next_tab".into(), "ctrl+alsobad".into());
    f.app.save_config();
    assert!(f.app.warnings.len() > n, "{:?}", f.app.warnings);
}

/// UI scale grows sidebar rows with the text but leaves terminal text size alone.
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

/// The saved window size is in DPI-only logical pixels, unaffected by UI zoom.
#[test]
fn saved_window_size_ignores_ui_zoom() {
    let mut f = Fixture::new();
    f.app.cfg.gui.ui_scale = 2.0;
    f.app.style = style_for(&f.app.theme, &f.app.cfg);
    f.frame(vec![]);
    // main.rs restores it as a winit LogicalSize (DPI only), so zoom must not shrink it.
    assert_eq!(f.app.geometry.window_size, Some((1280, 800)));
}

/// A theme failing contrast checks warns once, not on every re-pick.
#[test]
fn unreadable_theme_warns_once() {
    let theme = "
[[theme]]
name = \"grey\"
bg = \"#777777\"
fg = \"#888888\"
accent = \"red\"
muted = \"#808080\"
border = \"gray\"
selection_bg = \"#777777\"
selection_fg = \"white\"
success = \"green\"
warning = \"yellow\"
error = \"red\"
";
    let mut f = Fixture::with_config(&format!("{}{theme}", CONFIG.replace("midnight", "grey")));
    f.app.apply_theme("grey");
    let n = f.app.warnings.iter().filter(|w| w.contains("theme grey")).count();
    assert_eq!(n, 1, "{:?}", f.app.warnings);
}

/// The gear button (tooltip with shortcut) opens Settings by mouse and by keyboard, reusing the open tab.
#[test]
fn settings_gear_opens_settings_even_with_sidebar_collapsed() {
    let mut f = Fixture::new();
    f.app.do_action(Action::ToggleSidebar);
    f.frame(vec![]);
    let gear = f.h.tree().find("Settings").expect("gear button");
    assert_eq!(gear.role, skiff_ui::a11y::Role::Button);
    let at = gear.bounds.center();
    f.frame(vec![Event::MouseMove(at)]);
    // The tooltip shows after the hover delay.
    f.h.set_clock(Some(std::time::Instant::now() + std::time::Duration::from_secs(2)));
    f.frame(vec![]);
    f.h.set_clock(None);
    assert!(f.try_find("Settings (Ctrl+Shift+,)").is_some(), "tooltip names the shortcut: {:?}", f.texts());
    f.frame(vec![Event::MouseDown(MouseButton::Left, at), Event::MouseUp(MouseButton::Left, at)]);
    f.frame(vec![]);
    assert!(matches!(f.app.tabs[f.app.active].kind, TabKind::Settings));

    // Keyboard: focus it, Enter focuses the existing Settings tab again.
    f.app.do_action(Action::NewTab);
    f.app.palette_open = false;
    f.app.open_terminal(&f.app.hosts[0].clone());
    f.frame(vec![]);
    let id = crate::gear::id();
    f.frame(vec![Event::A11y { target: id, action: skiff_ui::a11y::A11yAction::Focus }]);
    f.frame(vec![Event::KeyDown { key: Key::Enter, mods: Modifiers::NONE, text: None }]);
    f.frame(vec![]);
    assert_eq!(f.app.tabs.iter().filter(|t| matches!(t.kind, TabKind::Settings)).count(), 1);
    assert!(matches!(f.app.tabs[f.app.active].kind, TabKind::Settings));
}

/// With no hosts at all the sidebar shows an empty state whose "Import
/// ~/.ssh/config" turns on reading the ssh config (saved) and lists its hosts.
#[test]
fn empty_sidebar_imports_ssh_config() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("ssh_config");
    std::fs::write(&ssh, "Host gamma\n  HostName 127.0.0.3\n  Port 1\n").unwrap();
    let cfg = format!(
        "[settings]\nload_ssh_config = false\nssh_config_path = \"{}\"\nssh_binary = \"skiff-test-no-such-ssh\"\n",
        ssh.display().to_string().replace('\\', "/")
    );
    let mut f = Fixture::with_config_in(dir, &cfg);
    assert!(f.try_find("No hosts yet").is_some(), "{:?}", f.texts());
    f.click_text("Import ~/.ssh/config");
    assert_eq!(f.app.hosts.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["gamma"]);
    assert!(std::fs::read_to_string(&f.path).unwrap().contains("load_ssh_config = true"));
    assert!(f.try_find("gamma").is_some());
}

/// While a terminal's context menu holds the keyboard, app shortcuts stand
/// aside: the menu's own item shortcut acts once and closes the menu.
#[test]
fn shortcuts_stand_aside_while_a_context_menu_is_open() {
    let mut f = Fixture::new();
    f.double_click_text("alpha");
    f.frame(vec![]);
    let r = f.h.tree().find("Terminal: alpha").expect("terminal").bounds;
    let at = Point::new(r.x + 30, r.y + 80);
    f.frame(vec![Event::MouseMove(at), Event::MouseDown(MouseButton::Right, at), Event::MouseUp(MouseButton::Right, at)]);
    f.frame(vec![]);
    assert!(f.try_find("Split right").is_some(), "the menu is open");
    let ctrl_shift = Modifiers { ctrl: true, shift: true, ..Modifiers::NONE };
    f.frame(vec![Event::KeyDown { key: Key::Char('d'), mods: ctrl_shift, text: None }]);
    f.frame(vec![]);
    f.frame(vec![]);
    assert_eq!(f.app.dock.groups.len(), 2, "split once, by the menu");
    assert!(f.try_find("Split right").is_none(), "the menu closed");
}

/// Narrow groups: Settings folds its section list into a named icon rail
/// (search above the page) and stacks the host list over the editor; the
/// forwards table drops columns; a terminal's error wraps inside its pane.
#[test]
fn pages_reflow_when_narrow() {
    let mut f = Fixture::new();
    f.h.resize(760, 700);
    f.double_click_text("alpha");
    f.frame(vec![]);
    let term = f.h.tree().find("Terminal: alpha").unwrap().bounds;
    let lines: Vec<Point> = f
        .h
        .list()
        .prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, origin, .. } if text.contains("skiff-test-no-such-ssh") || origin.y > term.y && origin.x < term.x + 20 => {
                Some(*origin)
            }
            _ => None,
        })
        .collect();
    assert!(lines.len() > 1, "the error wraps");
    f.app.open_settings();
    f.frame(vec![]);
    f.frame(vec![]);
    let t = f.h.tree();
    let nav = t.find("Appearance").expect("rail item named").bounds;
    let search = t.find("Search settings").unwrap().bounds;
    assert!(nav.w < 60 && search.x > nav.right(), "rail {nav:?}, search {search:?}");
    let list = t.nodes.iter().find(|n| n.name == "Connections" && n.role != skiff_ui::a11y::Role::Button);
    let empty = f.find("No host selected");
    assert!(list.is_some_and(|l| l.bounds.bottom() <= empty.y), "the list sits above the editor");
    assert_eq!(crate::forwards::columns_for(400), ["Name", "Status", ""]);
    assert_eq!(crate::forwards::columns_for(560), ["Name", "Host", "Status", ""]);
    assert_eq!(crate::forwards::columns_for(800), ["Name", "Host", "Type", "Route", "Status", ""]);
}
