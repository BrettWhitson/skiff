//! Host-list flows driven through `App::handle_event`. Every config and ssh
//! config is a temp file; terminal tests connect only to the local
//! termax-testserver fixture.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termax_core::config::Config;
use termax_core::history::ConnKind;
use termax_core::state::{SortMode, UiState};

use super::*;

struct T {
    dir: tempfile::TempDir,
    app: App,
    _rt: tokio::runtime::Runtime,
}

impl T {
    fn new(ssh: &str) -> T {
        let dir = tempfile::tempdir().unwrap();
        let ssh_path = dir.path().join("ssh_config");
        std::fs::write(&ssh_path, ssh).unwrap();
        let cfg_path = dir.path().join("config.toml");
        let base = Config::load_or_create(&cfg_path).unwrap();
        let mut cfg = base.clone();
        cfg.settings.ssh_config_path = Some(ssh_path.display().to_string());
        cfg.save(&base, &cfg_path).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let app = App::new(cfg, cfg_path, rt.handle().clone());
        T { dir, app, _rt: rt }
    }

    fn key(&mut self, code: KeyCode) {
        self.app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn keys(&mut self, code: KeyCode, n: usize) {
        for _ in 0..n {
            self.key(code);
        }
    }

    fn ctrl(&mut self, c: char) {
        self.app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
    }

    fn typed(&mut self, s: &str) {
        for c in s.chars() {
            self.key(KeyCode::Char(c));
        }
    }

    fn ssh_text(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("ssh_config")).unwrap()
    }

    fn listed(&self) -> Vec<String> {
        self.app.filtered.iter().map(|&i| self.app.hosts[i].name.clone()).collect()
    }
}

#[test]
fn edit_ssh_host_saves_in_place() {
    let mut t = T::new("# my servers\nHost web\n    HostName 10.0.0.5\n    Port 22\n");
    t.key(KeyCode::Char('e'));
    assert!(matches!(t.app.modal, Some(Modal::Editor(_))));
    t.keys(KeyCode::Down, 3); // Name, Host, User → Port
    t.ctrl('u');
    t.typed("2200");
    t.ctrl('s');
    assert!(t.app.modal.is_none(), "{:?}", t.app.status);
    assert_eq!(t.ssh_text(), "# my servers\nHost web\n    HostName 10.0.0.5\n    Port 2200\n");
    assert_eq!(std::fs::read_dir(t.dir.path().join("backups")).unwrap().count(), 1);
    assert_eq!(t.app.hosts[0].port, Some(2200));
}

#[test]
fn add_to_ssh_config_lands_before_wildcard() {
    let mut t = T::new("Host web\n    HostName a\n\nHost *\n    User me\n");
    t.key(KeyCode::Char('n'));
    t.key(KeyCode::Down); // "Save to" (ssh config) → Name
    t.typed("db");
    t.key(KeyCode::Down);
    t.typed("10.0.0.9");
    t.ctrl('s');
    assert!(t.app.modal.is_none());
    assert_eq!(t.ssh_text(), "Host web\n    HostName a\n\nHost db\n    HostName 10.0.0.9\n\nHost *\n    User me\n");
}

#[test]
fn read_only_host_explains_why() {
    let mut t = T::new("Host a b\n    User x\n");
    t.key(KeyCode::Char('e'));
    assert!(t.app.modal.is_none());
    let (msg, is_err, _) = t.app.status.clone().unwrap();
    assert!(is_err && msg.contains("shared block"), "{msg}");
}

#[test]
fn hidden_toggle_and_sort_recent() {
    let mut t = T::new("Host a\nHost b\nHost c\n    # termax: hidden\n");
    assert_eq!(t.listed(), ["a", "b"]);
    t.key(KeyCode::Char('H'));
    assert_eq!(t.listed(), ["a", "b", "c"]);
    t.app.history.record("b", ConnKind::Terminal, 100);
    t.key(KeyCode::Char('o'));
    assert_eq!(t.listed(), ["b", "a", "c"]);
    let st = UiState::load(&UiState::path_for(&t.app.cfg_path));
    assert!(st.show_hidden);
    assert_eq!(st.sort, SortMode::Recent);
}

#[test]
fn forward_form_rejects_bad_port() {
    let mut t = T::new("Host a\n    HostName 10.0.0.1\n");
    t.key(KeyCode::Char('f'));
    assert!(matches!(t.app.modal, Some(Modal::Forward(_))));
    t.keys(KeyCode::Down, 3); // Kind, Name, Bind → Listen port
    t.typed("99999");
    t.ctrl('s');
    match &t.app.modal {
        Some(Modal::Forward(f)) => assert_eq!(f.error.as_deref(), Some("Listen port must be 1–65535")),
        _ => panic!("form closed"),
    }
    assert!(t.app.cfg.forwards.is_empty());
}

#[test]
fn new_dialogs_render_on_a_small_screen() {
    let mut t = T::new("Host a\n    HostName 10.0.0.1\n");
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 16)).unwrap();
    for k in ['n', 'f', 'F', '?'] {
        t.key(KeyCode::Char(k));
        term.draw(|f| crate::ui::draw(f, &mut t.app)).unwrap();
        t.key(KeyCode::Esc);
    }
    t.key(KeyCode::Char('e'));
    t.keys(KeyCode::Up, 1); // wraps to the last "Other options" line
    term.draw(|f| crate::ui::draw(f, &mut t.app)).unwrap();
}

/// Windows only: a read-only file can't be replaced there, which stands in
/// for an editor or antivirus holding the file.
#[cfg(windows)]
fn set_readonly(p: &std::path::Path, ro: bool) {
    let mut perm = std::fs::metadata(p).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(ro);
    std::fs::set_permissions(p, perm).unwrap();
}

#[cfg(windows)]
#[test]
fn failed_save_can_be_retried() {
    let mut t = T::new("Host web\n    HostName a\n");
    let ssh = t.dir.path().join("ssh_config");
    t.key(KeyCode::Char('n'));
    t.key(KeyCode::Down);
    t.typed("db");
    t.key(KeyCode::Down);
    t.typed("10.0.0.9");
    set_readonly(&ssh, true);
    t.ctrl('s');
    assert!(t.app.modal.is_some(), "the form stays open after a failed write");
    set_readonly(&ssh, false);
    t.ctrl('s');
    assert!(t.app.modal.is_none(), "{:?}", t.app.status);
    assert!(t.ssh_text().contains("Host db"));
}

/// A forward through a custom connection to a closed port on 127.0.0.1;
/// `-F none` keeps ssh away from the real ssh config.
#[test]
fn forwards_panel_shows_state_stop_and_edit() {
    use termax_core::config::Connection;
    use termax_core::forward::{Forward, ForwardKind};
    let mut t = T::new("");
    let args = ["-F", "none", "-o", "BatchMode=yes"].map(String::from).to_vec();
    t.app.cfg.connections.push(Connection { name: "loop".into(), host: "127.0.0.1".into(), port: Some(1), extra_args: args, ..Default::default() });
    t.app.reload_hosts();
    let fw = Forward { name: Some("w".into()), host: "loop".into(), kind: ForwardKind::Local, bind: None, listen: 47311, target: Some("localhost:80".into()) };
    t.app.cfg.forwards.push(fw);
    assert_eq!(t.app.forward_state(0), "stopped");
    t.key(KeyCode::Char('F'));
    t.key(KeyCode::Enter);
    let tabs = t.app.tabs.len();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !t.app.forward_state(0).starts_with("failed") && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let state = t.app.forward_state(0);
    assert!(state.starts_with("failed: "), "{state}");
    t.app.active = 0; // back to the host list
    t.key(KeyCode::Char('F'));
    t.key(KeyCode::Enter);
    assert_eq!(t.app.tabs.len(), tabs, "a restart replaces the forward's tab");
    t.app.active = 0;
    t.key(KeyCode::Char('F'));
    t.key(KeyCode::Char('s'));
    assert_eq!(t.app.forward_state(0), "stopped");
    assert_eq!(t.app.tabs.len(), tabs - 1);
    t.key(KeyCode::Char('e'));
    match &t.app.modal {
        Some(Modal::Forward(f)) => assert_eq!((f.editing, f.listen.value.as_str()), (Some(0), "47311")),
        _ => panic!("e opens the forward editor"),
    }
}

#[test]
fn cursor_follows_selected_host() {
    use ratatui::backend::Backend;
    let mut t = T::new("Host a\n    HostName 10.0.0.1\nHost b\n    HostName 10.0.0.2\n");
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 16)).unwrap();
    term.draw(|f| crate::ui::draw(f, &mut t.app)).unwrap();
    let first = term.backend_mut().get_cursor_position().unwrap();
    t.key(KeyCode::Down);
    term.draw(|f| crate::ui::draw(f, &mut t.app)).unwrap();
    let pos = term.backend_mut().get_cursor_position().unwrap();
    assert_eq!((pos.x, pos.y), (first.x, first.y + 1), "the cursor sits on the selected row");
    let buf = term.backend().buffer();
    let row: String = (pos.x..pos.x + 4).map(|x| buf[(x, pos.y)].symbol().to_string()).collect();
    assert!(row.starts_with('›'), "selection marked by a symbol, not only color: {row:?}");
}

fn alt(t: &mut T, c: char) {
    t.app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)));
}

/// A terminal tab on the local test server (never a real host).
fn fixture_terminal(t: &mut T) -> termax_testserver::TestFixture {
    let fx = termax_testserver::TestFixture::with_shell(termax_testserver::FixtureAuth::Key).unwrap();
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let s = TerminalSession::spawn("ssh", &args, 24, 80, 1000).unwrap();
    t.app.push_tab("fx".into(), TabKind::Terminal(Box::new(s)));
    wait_screen(t, "termax-testserver shell");
    fx
}

fn screen(t: &T) -> String {
    match &t.app.tabs[t.app.active].kind {
        TabKind::Terminal(s) => {
            let mut p = s.parser.lock().unwrap();
            termax_core::termx::screen_lines(&mut p).0.join("\n")
        }
        _ => String::new(),
    }
}

fn wait_screen(t: &T, s: &str) {
    let start = std::time::Instant::now();
    while !screen(t).contains(s) {
        assert!(start.elapsed() < std::time::Duration::from_secs(15), "waiting for {s:?}: ...{}", screen(t).chars().rev().take(300).collect::<String>().chars().rev().collect::<String>());
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

#[test]
fn tui_search_scrolls_and_counts() {
    let mut t = T::new("");
    let _fx = fixture_terminal(&mut t);
    let mut text = String::from("needle\r");
    for i in 0..60 {
        text.push_str(&format!("row{i}\r"));
    }
    if let TabKind::Terminal(s) = &mut t.app.tabs[t.app.active].kind {
        s.write(text.as_bytes());
    }
    wait_screen(&t, "row59");
    alt(&mut t, 'f');
    t.typed("needle");
    t.key(KeyCode::Enter);
    assert_eq!(t.app.search_status().as_deref(), Some("match 1/1"));
    let TabKind::Terminal(s) = &t.app.tabs[t.app.active].kind else { panic!() };
    assert!(s.scroll_offset() > 0, "scrolled back to the match");
    t.key(KeyCode::Esc);
    let TabKind::Terminal(s) = &t.app.tabs[t.app.active].kind else { panic!() };
    assert_eq!(s.scroll_offset(), 0, "Esc returns to live output");
}

#[test]
fn tui_multiline_paste_confirms() {
    let mut t = T::new("");
    let _fx = fixture_terminal(&mut t);
    t.app.handle_event(Event::Paste("a\nb".into()));
    assert!(matches!(t.app.modal, Some(Modal::Confirm { action: ConfirmAction::Paste(_), .. })));
}

#[test]
fn tui_snippet_picker_sends() {
    let mut t = T::new("");
    t.app.cfg.snippets.push(termax_core::termx::Snippet { name: "greet".into(), command: "echo hi-snippet".into(), host: None, run: false });
    let _fx = fixture_terminal(&mut t);
    alt(&mut t, 's');
    assert!(matches!(t.app.modal, Some(Modal::Snippets { .. })));
    t.key(KeyCode::Enter);
    assert!(t.app.modal.is_none());
    wait_screen(&t, "echo hi-snippet");
}
