//! Host-list flows driven through `App::handle_event`. Every config and ssh
//! config is a temp file; terminal tests connect only to the local
//! skiff-testserver fixture.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use skiff_core::config::Config;
use skiff_core::history::ConnKind;
use skiff_core::state::{SortMode, UiState};

use super::*;

struct T {
    dir: tempfile::TempDir,
    app: App,
    _rt: tokio::runtime::Runtime,
}

impl T {
    fn new(ssh: &str) -> T {
        T::with(ssh, |_| {})
    }

    /// `edit` adjusts the saved config before the app loads it.
    fn with(ssh: &str, edit: impl FnOnce(&mut Config)) -> T {
        let dir = tempfile::tempdir().unwrap();
        let ssh_path = dir.path().join("ssh_config");
        std::fs::write(&ssh_path, ssh).unwrap();
        let cfg_path = dir.path().join("config.toml");
        let base = Config::load_or_create(&cfg_path).unwrap();
        let mut cfg = base.clone();
        cfg.settings.ssh_config_path = Some(ssh_path.display().to_string());
        edit(&mut cfg);
        cfg.save(&base, &cfg_path).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut app = App::new(cfg, cfg_path, rt.handle().clone());
        app.question_delay = Duration::ZERO; // tests answer at once
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
    let mut t = T::new("Host a\nHost b\nHost c\n    # skiff: hidden\n");
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
        Some(Modal::Forward(f)) => {
            assert_eq!(f.error.as_deref(), Some("Listen port must be 1–65535"))
        }
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
    use skiff_core::config::Connection;
    use skiff_core::forward::{Forward, ForwardKind};
    let mut t = T::new("");
    let args = ["-F", "none", "-o", "BatchMode=yes"].map(String::from).to_vec();
    t.app.cfg.connections.push(Connection {
        name: "loop".into(),
        host: "127.0.0.1".into(),
        port: Some(1),
        extra_args: args,
        ..Default::default()
    });
    t.app.reload_hosts();
    let fw = Forward {
        name: Some("w".into()),
        host: "loop".into(),
        kind: ForwardKind::Local,
        bind: None,
        listen: 47311,
        target: Some("localhost:80".into()),
    };
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
    t.app.drain_messages();
    assert!(!t.app.history.hosts.contains_key("loop"), "a forward that never came up isn't history");
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
        Some(Modal::Forward(f)) => {
            assert_eq!((f.editing, f.listen.value.as_str()), (Some(0), "47311"))
        }
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
fn fixture_terminal(t: &mut T) -> skiff_testserver::TestFixture {
    let fx = skiff_testserver::TestFixture::with_shell(skiff_testserver::FixtureAuth::Key).unwrap();
    let mut args = fx.ssh_options();
    args.push("-tt".into());
    args.push(fx.destination());
    let s = TerminalSession::spawn("ssh", &args, 24, 80, 1000).unwrap();
    t.app.push_tab("fx".into(), TabKind::Terminal(Box::new(s)));
    wait_screen(t, "skiff-testserver shell");
    fx
}

fn screen(t: &T) -> String {
    match &t.app.tabs[t.app.active].kind {
        TabKind::Terminal(s) => {
            let mut p = s.parser.lock().unwrap();
            skiff_core::termx::screen_lines(&mut p).0.join("\n")
        }
        _ => String::new(),
    }
}

fn wait_screen(t: &T, s: &str) {
    let start = std::time::Instant::now();
    while !screen(t).contains(s) {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "waiting for {s:?}: ...{}",
            screen(t).chars().rev().take(300).collect::<String>().chars().rev().collect::<String>()
        );
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
    alt(&mut t, '/');
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
    assert!(matches!(t.app.modal, Some(Modal::Confirm { action: ConfirmAction::Paste { .. }, .. })));
}

#[test]
fn tui_snippet_picker_sends() {
    let mut t = T::new("");
    t.app.cfg.snippets.push(skiff_core::termx::Snippet { name: "greet".into(), command: "echo hi-snippet".into(), host: None, run: false });
    let _fx = fixture_terminal(&mut t);
    alt(&mut t, 's');
    assert!(matches!(t.app.modal, Some(Modal::Snippets { .. })));
    t.key(KeyCode::Enter);
    assert!(t.app.modal.is_none());
    wait_screen(&t, "echo hi-snippet");
}

fn press_keys(text: &str) -> Vec<Event> {
    text.chars()
        .map(|c| {
            let code = if c == '\r' { KeyCode::Enter } else { KeyCode::Char(c) };
            Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
        })
        .collect()
}

#[test]
fn tui_key_burst_is_a_paste() {
    // Windows consoles deliver a paste as a burst of key presses.
    let mut t = T::new("");
    let _fx = fixture_terminal(&mut t);
    t.app.paste_bursts = true;
    t.app.handle_batch(press_keys("ls\r"));
    assert!(t.app.modal.is_none(), "a few keys typed quickly are just typing");
    t.app.handle_batch(press_keys("echo one\recho two\r"));
    assert!(matches!(t.app.modal, Some(Modal::Confirm { action: ConfirmAction::Paste { .. }, .. })));
}

fn draw(t: &mut T) -> ratatui::Terminal<ratatui::backend::TestBackend> {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 26)).unwrap();
    term.draw(|f| crate::ui::draw(f, &mut t.app)).unwrap();
    term
}

/// Where `s` is drawn: (x, y) of its first cell.
fn find_text(term: &ratatui::Terminal<ratatui::backend::TestBackend>, s: &str) -> Option<(u16, u16)> {
    let buf = term.backend().buffer();
    (0..buf.area.height).find_map(|y| {
        let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        row.find(s).map(|i| (row[..i].chars().count() as u16, y))
    })
}

fn write(t: &mut T, bytes: &[u8]) {
    if let TabKind::Terminal(s) = &mut t.app.tabs[t.app.active].kind {
        s.write(bytes);
    }
}

fn mouse(t: &mut T, kind: crossterm::event::MouseEventKind, column: u16, row: u16) {
    t.app.handle_event(Event::Mouse(crossterm::event::MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }));
}

#[test]
fn alt_shortcuts_ignore_case_but_leave_alt_shift_to_the_shell() {
    let mut t = T::new("");
    t.app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::ALT | KeyModifiers::SHIFT)));
    assert!(t.app.modal.is_none(), "Alt+Shift+H isn't a shortcut");
    t.app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::ALT)));
    assert!(matches!(t.app.modal, Some(Modal::Help { .. })), "Alt+H with Caps Lock on");
}

#[test]
fn tui_search_highlights_and_follows_new_output() {
    let mut t = T::new("");
    let _fx = fixture_terminal(&mut t);
    write(&mut t, b"needle\r");
    wait_screen(&t, "needle\n");
    t.app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::ALT | KeyModifiers::SHIFT)));
    assert!(t.app.modal.is_none(), "Alt+Shift+F goes to the shell");
    alt(&mut t, '/');
    t.app.handle_event(Event::Paste("needle".into()));
    assert_eq!(t.app.search_status().as_deref(), Some("match 1/1"), "a paste goes into the query");
    let term = draw(&mut t);
    let (x, y) = find_text(&term, "needle").expect("on screen");
    let cell = &term.backend().buffer()[(x, y)];
    assert!(
        cell.modifier.contains(ratatui::style::Modifier::REVERSED | ratatui::style::Modifier::BOLD),
        "the current match is highlighted"
    );
    // New output: the current match stays, the new one is found.
    write(&mut t, b"needle 2\r");
    wait_screen(&t, "needle 2\n");
    t.app.drain_messages();
    assert_eq!(t.app.search_status().as_deref(), Some("match 1/2"));
}

#[test]
fn tui_mouse_release_outside_reaches_the_program() {
    use crossterm::event::{MouseButton as B, MouseEventKind as K};
    let mut t = T::new("");
    let _fx = fixture_terminal(&mut t);
    write(&mut t, b"\x1b[?1002h\x1b[?1006h"); // echoed back: the "program" asks for the mouse
    let start = std::time::Instant::now();
    while !t.app.wants_mouse() {
        assert!(start.elapsed() < std::time::Duration::from_secs(15), "mouse mode never turned on");
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    draw(&mut t);
    mouse(&mut t, K::Down(B::Left), 5, 5);
    assert_eq!(t.app.mouse_held, Some(0));
    mouse(&mut t, K::Drag(B::Left), 5, 25); // over the status line
    mouse(&mut t, K::Up(B::Left), 5, 25);
    assert_eq!(t.app.mouse_held, None, "the release is reported at the terminal's edge");
    // A dialog opening, or a tab change by keyboard, mid-drag releases it.
    mouse(&mut t, K::Down(B::Left), 5, 5);
    alt(&mut t, 'h');
    assert_eq!(t.app.mouse_held, None, "released when a dialog opens");
    t.key(KeyCode::Esc);
    mouse(&mut t, K::Down(B::Left), 5, 5);
    alt(&mut t, '1');
    assert_eq!(t.app.mouse_held, None, "released when the tab changes");
}

#[test]
fn tui_mouse_always_clicks_tabs_scrolls_and_copies() {
    use crossterm::event::{MouseButton as B, MouseEventKind as K};
    let mut t = T::new("");
    t.app.cfg.settings.tui_mouse = skiff_core::config::TuiMouse::Always;
    let _fx = fixture_terminal(&mut t);
    assert!(t.app.capture_mouse());
    let lines: String = (0..40).map(|i| format!("row{i}\r")).collect();
    write(&mut t, format!("alpha beta\r{lines}").as_bytes());
    wait_screen(&t, "row39");
    let term = draw(&mut t);
    let (x, _) = find_text(&term, " 1:Hosts ").unwrap();
    mouse(&mut t, K::Down(B::Left), x + 1, 0);
    assert_eq!(t.app.active, 0, "a click on a tab switches to it");
    let (x, _) = find_text(&term, " 2:fx ").unwrap();
    mouse(&mut t, K::Down(B::Left), x + 1, 0);
    assert_eq!(t.app.active, 1);
    mouse(&mut t, K::ScrollUp, 10, 10);
    let TabKind::Terminal(s) = &t.app.tabs[1].kind else { panic!() };
    assert_eq!(s.scroll_offset(), 3, "the wheel scrolls the scrollback");
    for _ in 0..10 {
        mouse(&mut t, K::ScrollUp, 10, 10);
    }
    let term = draw(&mut t);
    let (x, y) = find_text(&term, "alpha beta").expect("scrolled back to it");
    mouse(&mut t, K::Down(B::Left), x, y);
    mouse(&mut t, K::Drag(B::Left), x + 4, y);
    let term = draw(&mut t);
    assert_eq!(term.backend().buffer()[(x + 2, y)].bg, t.app.theme.selection_bg, "the selection is drawn");
    mouse(&mut t, K::Up(B::Left), x + 4, y);
    assert_eq!(t.app.clipboard_out.as_deref(), Some("alpha"), "a drag copies");
}

#[test]
fn rename_carries_forwards_and_history() {
    use skiff_core::forward::{Forward, ForwardKind};
    let mut t = T::new("Host web\n    HostName 10.0.0.5\n");
    t.app.cfg.forwards.push(Forward { host: "web".into(), kind: ForwardKind::Dynamic, listen: 47312, ..Default::default() });
    t.app.save_config();
    t.app.record("web", ConnKind::Terminal);
    t.key(KeyCode::Char('e'));
    t.ctrl('u');
    t.typed("web2");
    t.ctrl('s');
    assert!(t.app.modal.is_none(), "{:?}", t.app.status);
    assert!(t.ssh_text().contains("Host web2"));
    assert_eq!(t.app.cfg.forwards[0].host, "web2");
    assert!(std::fs::read_to_string(t.dir.path().join("config.toml")).unwrap().contains("host = \"web2\""));
    let hist = skiff_core::history::History::load(&t.dir.path().join("history.toml"));
    assert_eq!(hist.recent(5), ["web2"]);
}

#[test]
fn tui_keys_remap_end_to_end() {
    let mut t = T::with("", |cfg| {
        cfg.tui_keys.insert("help".into(), "f1".into());
        cfg.tui_keys.insert("bogus".into(), "alt+z".into());
    });
    let (msg, is_err, _) = t.app.status.clone().expect("the bad entry is reported");
    assert!(is_err && msg.contains("[tui_keys] bogus: unknown action"), "{msg}");
    alt(&mut t, 'h');
    assert!(t.app.modal.is_none(), "Alt+H no longer opens help");
    t.key(KeyCode::F(1));
    assert!(matches!(t.app.modal, Some(Modal::Help { .. })));
    let term = draw(&mut t);
    assert!(find_text(&term, "F1").is_some(), "the help lists the effective key");
    assert!(find_text(&term, "bogus").is_some(), "and the config problem");
    t.key(KeyCode::F(1));
    assert!(t.app.modal.is_none(), "the help key closes it again");
}

#[test]
fn help_scrolls_on_a_small_screen() {
    let mut t = T::new("");
    t.key(KeyCode::Char('?'));
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    term.draw(|f| crate::ui::draw(f, &mut t.app)).unwrap();
    assert!(find_text(&term, "cancel the newest running transfer").is_none(), "the end is below the fold");
    t.key(KeyCode::End);
    term.draw(|f| crate::ui::draw(f, &mut t.app)).unwrap();
    assert!(find_text(&term, "cancel the newest running transfer").is_some(), "End shows the end");
}

/// Removing a tab before the active one must not switch tabs.
#[test]
fn stopping_a_forward_keeps_the_active_tab() {
    use skiff_core::forward::{Forward, ForwardKind};
    let mut t = T::new("");
    for name in ["a", "fwd", "b", "c"] {
        t.app.push_tab(name.into(), TabKind::Hosts);
    }
    let f = Forward { host: "x".into(), kind: ForwardKind::Dynamic, listen: 47313, ..Default::default() };
    t.app.tabs[2].forward = Some(f.title());
    t.app.cfg.forwards.push(f);
    t.app.active = 3; // "b"
    t.app.stop_forward(0);
    assert_eq!(t.app.tabs[t.app.active].title, "b");
}

/// An SFTP tab still connecting, for askpass questions.
fn connecting_sftp_tab(t: &mut T) -> String {
    let id = t.app.next_id;
    let tab = SftpTab::new(id, "web".into(), t.dir.path().to_path_buf(), false, t.app.rt.clone(), t.app.tx.clone());
    let tag = tab.ask_tag();
    t.app.push_tab("web [sftp]".into(), TabKind::Sftp(Box::new(tab)));
    tag
}

fn ask(t: &mut T, tag: &str, prompt: &str) -> (oneshot::Receiver<Option<String>>, tokio_util::sync::CancellationToken) {
    let (reply, rx) = oneshot::channel();
    let cancelled = tokio_util::sync::CancellationToken::new();
    let req = AskRequest { tag: tag.into(), prompt: prompt.into(), kind: AskKind::of(prompt), reply, cancelled: cancelled.clone() };
    t.app.tx.send(Msg::Ask(req)).unwrap();
    t.app.drain_messages();
    (rx, cancelled)
}

#[test]
fn askpass_modal_masks_answers_and_cancels() {
    let mut t = T::new("");
    let tag = connecting_sftp_tab(&mut t);
    // A password: typed masked, Enter answers.
    let (mut rx, _) = ask(&mut t, &tag, "tester@127.0.0.1's password: ");
    assert!(matches!(t.app.modal, Some(Modal::Ask { .. })));
    t.typed("pw9x");
    let term = draw(&mut t);
    assert!(find_text(&term, "pw9x").is_none(), "the password is never drawn");
    assert!(find_text(&term, "••••").is_some());
    t.key(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap().as_deref(), Some("pw9x"));
    assert!(t.app.modal.is_none());
    // A host key: y answers yes.
    let (mut rx, _) = ask(&mut t, &tag, "Are you sure you want to continue connecting (yes/no/[fingerprint])? ");
    t.key(KeyCode::Char('y'));
    assert_eq!(rx.try_recv().unwrap().as_deref(), Some("yes"));
    // Esc cancels.
    let (mut rx, _) = ask(&mut t, &tag, "Verification code: ");
    t.key(KeyCode::Esc);
    assert_eq!(rx.try_recv().unwrap(), None);
    // ssh withdrew the question (gave up, tab closed): the prompt goes away.
    let (_rx, cancelled) = ask(&mut t, &tag, "password: ");
    assert!(t.app.modal.is_some());
    cancelled.cancel();
    t.app.drain_messages();
    assert!(t.app.modal.is_none());
    // A question for an attempt that's over is refused without showing.
    let (mut rx, _) = ask(&mut t, "sftp:999:1", "password: ");
    assert!(t.app.modal.is_none());
    assert_eq!(rx.try_recv().unwrap(), None);
    // A security-key touch notice is shown on the status line, never as a
    // dialog, until ssh is done with it.
    let (reply, _nrx) = oneshot::channel();
    let cancelled = tokio_util::sync::CancellationToken::new();
    let prompt = "Confirm user presence for key ED25519-SK";
    let req = AskRequest { tag: tag.clone(), prompt: prompt.into(), kind: AskKind::Notice, reply, cancelled: cancelled.clone() };
    t.app.tx.send(Msg::Ask(req)).unwrap();
    t.app.drain_messages();
    assert!(t.app.modal.is_none());
    assert!(find_text(&draw(&mut t), prompt).is_some());
    t.app.set_error("boom");
    let term = draw(&mut t);
    assert!(find_text(&term, "Error: boom").is_some() && find_text(&term, prompt).is_some(), "an error doesn't hide the notice");
    t.app.status = None;
    cancelled.cancel();
    t.app.drain_messages();
    assert!(find_text(&draw(&mut t), prompt).is_none());
}

#[test]
fn background_questions_wait_for_their_own_tab() {
    let mut t = T::new("");
    let tag = connecting_sftp_tab(&mut t);
    let own = t.app.active;
    t.app.active = 0; // the user works in another tab
    let (mut rx, _) = ask(&mut t, &tag, "password: ");
    assert!(t.app.modal.is_none(), "never shown over another tab");
    let term = draw(&mut t);
    assert!(find_text(&term, "web [sftp] ?").is_some(), "the tab is flagged");
    assert!(find_text(&term, "waiting for an answer").is_some(), "the status line says where");
    // On its own tab it shows, but ignores keys typed right as it opens.
    t.app.question_delay = std::time::Duration::from_millis(250);
    t.app.active = own;
    t.app.drain_messages();
    assert!(matches!(t.app.modal, Some(Modal::Ask { .. })));
    t.typed("ls");
    t.key(KeyCode::Enter);
    assert!(rx.try_recv().is_err(), "keys right after it opened are ignored");
    t.app.question_shown = t.app.question_shown.map(|at| at - std::time::Duration::from_millis(300));
    t.typed("pw");
    t.key(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap().as_deref(), Some("pw"));
}

/// Runs the (current-thread) runtime and the app's message loop until `done`.
fn pump(t: &mut T, what: &str, done: impl Fn(&App) -> bool) {
    let start = std::time::Instant::now();
    while !done(&t.app) {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "waiting for {what}: {:?} {:?}", t.app.status, last_state(&t.app));
        t._rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(20)).await });
        t.app.drain_messages();
    }
}

fn sftp(app: &App) -> &SftpTab {
    match &app.tabs[app.active].kind {
        TabKind::Sftp(s) => s,
        _ => panic!("not an sftp tab"),
    }
}

fn last_state(app: &App) -> Option<crate::sftp_tab::TransferState> {
    let s = app.tabs.iter().find_map(|t| match &t.kind {
        TabKind::Sftp(s) => Some(s),
        _ => None,
    })?;
    s.transfers.last().map(|t| t.state.clone())
}

#[test]
fn transfers_ask_before_overwriting_and_cancel() {
    use crate::sftp_tab::TransferState as St;
    let fx = skiff_testserver::TestFixture::new(skiff_testserver::FixtureAuth::Key).unwrap();
    let mut t = T::new("");
    let local = t.dir.path().join("up");
    std::fs::create_dir(&local).unwrap();
    std::fs::write(local.join("a.txt"), "new").unwrap();
    std::fs::write(local.join("b.txt"), "bee").unwrap();
    std::fs::write(fx.root().join("a.txt"), "old").unwrap();
    let mut tab = SftpTab::new(t.app.next_id, "fx".into(), local.clone(), false, t.app.rt.clone(), t.app.tx.clone());
    let mut args = fx.ssh_options();
    args.extend(["-s".into(), fx.destination(), "sftp".into()]);
    tab.connect(skiff_core::fs::connect::SshLaunch { program: "ssh".into(), args, env: vec![], start_dir: None });
    t.app.push_tab("fx [sftp]".into(), TabKind::Sftp(Box::new(tab)));
    pump(&mut t, "the remote listing", |a| matches!(sftp(a).status, Status::Connected) && !sftp(a).remote.loading);
    let select = |t: &mut T, name: &str| {
        let TabKind::Sftp(s) = &mut t.app.tabs[t.app.active].kind else { panic!() };
        s.focus = Side::Local;
        s.local.selected = s.local.entries.iter().position(|e| e.name == name).unwrap();
    };
    let remote = fx.root().join("a.txt");

    // Overwrite.
    select(&mut t, "a.txt");
    t.key(KeyCode::Char('c'));
    pump(&mut t, "the overwrite question", |a| matches!(a.modal, Some(Modal::Overwrite { .. })));
    t.key(KeyCode::Char('o'));
    pump(&mut t, "the copy", |a| last_state(a) == Some(St::Done));
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "new");

    // Skip.
    std::fs::write(local.join("a.txt"), "newer").unwrap();
    t.key(KeyCode::Char('c'));
    pump(&mut t, "the overwrite question", |a| matches!(a.modal, Some(Modal::Overwrite { .. })));
    t.key(KeyCode::Char('s'));
    pump(&mut t, "the skip", |a| last_state(a) == Some(St::Done));
    assert_eq!(sftp(&t.app).transfers.last().unwrap().skipped, 1);
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "new");

    // Cancel from the question.
    t.key(KeyCode::Char('c'));
    pump(&mut t, "the overwrite question", |a| matches!(a.modal, Some(Modal::Overwrite { .. })));
    t.key(KeyCode::Esc);
    pump(&mut t, "the cancel", |a| last_state(a) == Some(St::Cancelled));
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "new");

    // Cancel running transfers (X, then X again for the next); nothing has
    // run yet on this runtime.
    select(&mut t, "b.txt");
    t.key(KeyCode::Char('c'));
    t.key(KeyCode::Char('c'));
    t.key(KeyCode::Char('X'));
    t.key(KeyCode::Char('X'));
    pump(&mut t, "the cancels", |a| sftp(a).transfers.iter().rev().take(2).all(|x| x.state == St::Cancelled));
    assert!(!fx.root().join("b.txt").exists());
    t.key(KeyCode::Char('X'));
    assert!(t.app.status.as_ref().is_some_and(|s| s.1), "nothing left to cancel");

    // Closing the tab cancels its transfers.
    t.key(KeyCode::Char('c'));
    alt(&mut t, 'w');
    for _ in 0..10 {
        t._rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(20)).await });
    }
    assert!(!fx.root().join("b.txt").exists());
}

/// A local shell (no network) for split-pane tests.
fn local_shell(t: &mut T) {
    let (prog, args): (&str, Vec<String>) = if cfg!(windows) { ("cmd.exe", vec![]) } else { ("sh", vec![]) };
    let s = TerminalSession::spawn(prog, &args, 24, 80, 100).unwrap();
    t.app.push_tab("sh".into(), TabKind::Terminal(Box::new(s)));
}

fn pane_text(s: &TerminalSession) -> String {
    s.parser.lock().unwrap().screen().contents()
}

fn panes(t: &T) -> (&TerminalSession, &TerminalSession) {
    let tab = &t.app.tabs[t.app.active];
    let TabKind::Terminal(focused) = &tab.kind else { panic!() };
    (focused, &tab.split.as_ref().expect("split").other)
}

fn wait_pane(t: &T, focused: bool, s: &str) {
    let start = std::time::Instant::now();
    loop {
        let (f, o) = panes(t);
        if pane_text(if focused { f } else { o }).contains(s) {
            return;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(15), "waiting for {s:?}");
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

#[test]
fn split_panes_size_and_route_input() {
    let mut t = T::new("");
    local_shell(&mut t);
    alt(&mut t, '|');
    let sp = t.app.tabs[t.app.active].split.as_ref().expect("split");
    assert!(!sp.focus_first, "the new (right) pane has the focus");
    draw(&mut t); // 80x26: the body is 80x24 at row 1
    let (left, right) = crate::ui::split_areas(Rect::new(0, 1, 80, 24), SplitDir::Right, 0.5);
    assert_eq!((left.width + right.width, left.height), (80, 24));
    let size = |s: &TerminalSession| s.parser.lock().unwrap().screen().size();
    let (f, o) = panes(&t);
    assert_eq!(size(f), (22, right.width - 2), "each pane's pty fits inside its border");
    assert_eq!(size(o), (22, left.width - 2));
    assert_eq!(t.app.term_origin, (right.x + 1, 2), "mouse reports are relative to the focused pane");

    // Typing goes to the focused pane only.
    t.typed("echo right-pane-mark");
    t.key(KeyCode::Enter);
    wait_pane(&t, true, "right-pane-mark");
    assert!(!pane_text(panes(&t).1).contains("right-pane-mark"));
    alt(&mut t, 'o');
    t.typed("echo left-pane-mark");
    t.key(KeyCode::Enter);
    wait_pane(&t, true, "left-pane-mark");
    assert!(!pane_text(panes(&t).1).contains("left-pane-mark"));

    // A paste goes to the focused pane too; a click on the other one focuses it.
    t.app.handle_event(Event::Paste("echo pasted-mark".into()));
    wait_pane(&t, true, "pasted-mark");
    draw(&mut t);
    mouse(&mut t, crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left), right.x + 5, 5);
    assert!(!t.app.tabs[t.app.active].split.as_ref().unwrap().focus_first, "the click focused the right pane");

    // Stacked: halves of the height.
    alt(&mut t, '-');
    draw(&mut t);
    let (f, o) = panes(&t);
    assert_eq!((size(f), size(o)), ((10, 78), (10, 78)));

    let tabs = t.app.tabs.len();
    alt(&mut t, 'w');
    assert!(t.app.tabs[t.app.active].split.is_none());
    assert_eq!(t.app.tabs.len(), tabs, "Alt+W closed only the focused pane");
    let TabKind::Terminal(s) = &t.app.tabs[t.app.active].kind else { panic!() };
    assert!(pane_text(s).contains("left-pane-mark"), "the other pane stays");
}

#[test]
fn waiting_tab_past_nine_has_no_shortcut_hint() {
    let mut t = T::new("");
    for i in 0..8 {
        t.app.push_tab(format!("t{i}"), TabKind::Hosts);
    }
    let tag = connecting_sftp_tab(&mut t);
    t.app.active = 0;
    let _q = ask(&mut t, &tag, "password: ");
    let term = draw(&mut t);
    assert!(find_text(&term, "switch to tab 10").is_some());
    assert!(find_text(&term, "(Alt+9)").is_none(), "Alt+9 opens tab 9, not this one");
}

#[test]
fn split_divider_math() {
    use crate::ui::{divider_ratio, on_divider, split_areas};
    let area = Rect::new(0, 1, 80, 24);
    let (a, b) = split_areas(area, SplitDir::Right, 0.25);
    assert_eq!((a.width, b.x), (20, 20));
    // Each pane keeps 10 columns (plus its border).
    assert_eq!(split_areas(area, SplitDir::Right, 0.05).0.width, 12);
    assert_eq!(split_areas(area, SplitDir::Down, 0.95).1.height, 5);
    // The border columns on both sides of the boundary are the divider.
    assert!(on_divider(area, SplitDir::Right, 0.25, 19, 5) && on_divider(area, SplitDir::Right, 0.25, 20, 5));
    assert!(!on_divider(area, SplitDir::Right, 0.25, 21, 5) && !on_divider(area, SplitDir::Right, 0.25, 19, 0));
    // A drag snaps within a cell of a snap point, unless snapping is off.
    assert_eq!(divider_ratio(area, SplitDir::Right, 26, 5, true), 1.0 / 3.0);
    assert_eq!(divider_ratio(area, SplitDir::Right, 26, 5, false), 27.0 / 80.0);
    assert_eq!(divider_ratio(area, SplitDir::Right, 49, 5, true), 50.0 / 80.0);
}

#[test]
fn split_panes_swap_resize_equalize_and_drag() {
    use crossterm::event::{MouseButton as B, MouseEventKind as K};
    let mut t = T::new("");
    t.app.cfg.settings.tui_mouse = skiff_core::config::TuiMouse::Always;
    local_shell(&mut t);
    alt(&mut t, '|');
    draw(&mut t);
    let split = |t: &T| {
        let sp = t.app.tabs[t.app.active].split.as_ref().unwrap();
        (sp.focus_first, sp.ratio)
    };
    let key = |t: &mut T, c: KeyCode, m: KeyModifiers| t.app.handle_event(Event::Key(KeyEvent::new(c, m)));
    let alt_shift = KeyModifiers::ALT | KeyModifiers::SHIFT;
    assert_eq!(split(&t), (false, 0.5));
    alt(&mut t, 'x');
    assert_eq!(split(&t), (false, 0.5), "Alt+X is the shell's (M-x)");
    key(&mut t, KeyCode::Char('X'), alt_shift);
    assert_eq!(split(&t), (true, 0.5), "swapped: the focused pane is now on the left");
    key(&mut t, KeyCode::Right, alt_shift);
    assert_eq!(split(&t).1, 2.0 / 3.0, "the next snap point");
    key(&mut t, KeyCode::Up, alt_shift);
    assert_eq!(split(&t).1, 2.0 / 3.0, "a side-by-side split has no up");
    alt(&mut t, '=');
    assert_eq!(split(&t).1, 0.5);

    // Drag the divider (column 39/40 of 80) to near a quarter: it snaps.
    draw(&mut t);
    mouse(&mut t, K::Down(B::Left), 40, 10);
    mouse(&mut t, K::Drag(B::Left), 20, 10);
    mouse(&mut t, K::Up(B::Left), 20, 10);
    assert_eq!(split(&t), (true, 0.25), "focus unchanged by the drag");
    draw(&mut t);
    let size = |s: &TerminalSession| s.parser.lock().unwrap().screen().size();
    assert_eq!(size(panes(&t).0).1, 18, "the left pane is 20 columns with its border");
}

#[test]
fn readline_alt_keys_reach_the_shell() {
    let mut t = T::new("");
    local_shell(&mut t);
    for c in ['f', 't', 'b', 'd', '.', '\\'] {
        alt(&mut t, c);
        assert!(t.app.modal.is_none() && t.app.tabs[t.app.active].split.is_none(), "Alt+{c} is the shell's");
    }
    alt(&mut t, '/');
    assert!(matches!(t.app.modal, Some(Modal::Search { .. })), "Alt+/ finds");
}
