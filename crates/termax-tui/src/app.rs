use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use termax_core::termx::{self, Match, MouseEv};
use termax_core::fs::connect::RemoteConn;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use termax_core::config::{Config, StatusChecks};
use termax_core::forward::Forward;
use termax_core::history::{self, ConnKind, History};
use termax_core::hostform::{self, Dest, HostForm, Saved};
use termax_core::hosts::{self, HostEntry, Source};
use termax_core::sshconfig::edit::{ConfigSet, backups_dir, display_path};
use termax_core::state::UiState;
use termax_core::status::{self, Status as HostStatus};
use crate::forms::{Editor, ForwardForm, Outcome};
use crate::input::TextInput;
use crate::sftp_tab::{FileEntry, FileOp, SftpTab, Side, Status};
use termax_core::sshconfig;
use termax_core::terminal::{encode_key, TerminalSession};
use crate::tui_theme::Theme;
use termax_core::theme::{self, ThemeDef};

/// Results from background tasks, delivered to the UI thread.
pub enum Msg {
    SftpConnected { tab: u64, result: Result<Arc<RemoteConn>, String> },
    RemoteListing { tab: u64, result: Result<(String, Vec<FileEntry>, Option<String>), String> },
    TransferProgress { tab: u64, id: u64, done: u64 },
    TransferDone { tab: u64, id: u64, result: Result<(), String> },
    RemoteOpDone { tab: u64, result: Result<(), String>, keep: Option<String> },
    Status { host: String, status: HostStatus },
}

pub enum TabKind {
    Hosts,
    Terminal(Box<TerminalSession>),
    Sftp(Box<SftpTab>),
}

pub struct Tab {
    pub id: u64,
    pub title: String,
    pub kind: TabKind,
    /// History to record once the session proves it connected.
    pub record: Option<(String, ConnKind, Instant)>,
    /// The saved forward (by title) this tab runs.
    pub forward: Option<String>,
    /// The host a terminal tab is connected to (for its snippets).
    pub host: Option<String>,
}

pub enum PromptAction {
    Mkdir,
    Rename { from: String },
    Goto,
}

pub struct Prompt {
    pub title: String,
    pub input: TextInput,
    pub action: PromptAction,
}

pub enum ConfirmAction {
    DeleteHost(String),
    DeleteForward(usize),
    DeleteFile { name: String, is_dir: bool },
    Quit,
    /// A multi-line paste into the active terminal, then Enter if `enter`
    /// (a multi-line `run` snippet).
    Paste { text: String, enter: bool },
}

pub enum Modal {
    Help,
    ThemePicker { selected: usize, original: String },
    Editor(Editor),
    Forward(ForwardForm),
    Forwards { selected: usize },
    Prompt(Prompt),
    Confirm { text: String, action: ConfirmAction },
    /// Find in the active terminal's output; `current` indexes `matches`.
    Search { query: TextInput, matches: Vec<Match>, current: usize },
    /// Saved commands for the active terminal's host.
    Snippets { selected: usize },
}

pub struct App {
    pub cfg: Config,
    /// What `cfg` looked like when last loaded or saved; saves write only the difference.
    cfg_saved: Config,
    pub cfg_path: PathBuf,
    pub theme: Theme,
    /// Plain mode (`--plain` or `settings.tui_plain`).
    pub plain: bool,
    pub hosts: Vec<HostEntry>,
    pub filtered: Vec<usize>,
    pub host_selected: usize,
    pub host_offset: usize,
    pub filter: TextInput,
    pub filtering: bool,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub modal: Option<Modal>,
    pub status: Option<(String, bool, Instant)>,
    pub should_quit: bool,
    /// Last known size of the terminal content area, used for new sessions.
    pub term_area: (u16, u16),
    /// Screen position of the terminal content's top-left cell (mouse reports).
    pub term_origin: (u16, u16),
    /// Treat a burst of typed keys with a line break as a paste (Windows
    /// consoles have no bracketed paste, so pastes arrive as key presses).
    pub paste_bursts: bool,
    /// Reachability per host name.
    pub statuses: HashMap<String, HostStatus>,
    pub history: History,
    history_path: PathBuf,
    /// Sort order and hidden-host toggle (state.toml).
    pub ui_state: UiState,
    next_id: u64,
    rt: Handle,
    tx: UnboundedSender<Msg>,
    rx: UnboundedReceiver<Msg>,
}

impl App {
    pub fn new(cfg: Config, cfg_path: PathBuf, rt: Handle) -> Self {
        let (tx, rx) = unbounded_channel();
        let history_path = History::path_for(&cfg_path);
        let ui_state = UiState::load(&UiState::path_for(&cfg_path));
        let mut app = Self {
            statuses: HashMap::new(),
            history: History::load(&history_path),
            history_path,
            ui_state,
            theme: Theme::from_def(&theme::builtin()[0]).expect("builtin theme"),
            plain: cfg.settings.tui_plain,
            cfg_saved: cfg.clone(),
            cfg,
            cfg_path,
            hosts: vec![],
            filtered: vec![],
            host_selected: 0,
            host_offset: 0,
            filter: TextInput::default(),
            filtering: false,
            tabs: vec![Tab { id: 0, title: "Hosts".into(), kind: TabKind::Hosts, record: None, forward: None, host: None }],
            active: 0,
            modal: None,
            status: None,
            should_quit: false,
            term_area: (24, 80),
            term_origin: (1, 1),
            paste_bursts: cfg!(windows),
            next_id: 1,
            rt,
            tx,
            rx,
        };
        app.apply_theme_name(&app.cfg.settings.theme.clone());
        app.reload_hosts();
        if app.cfg.settings.status_checks == StatusChecks::OnStart {
            app.check_status();
        }
        app
    }

    // ----- themes -----

    pub fn all_themes(&self) -> Vec<ThemeDef> {
        let mut list = theme::builtin();
        for t in &self.cfg.themes {
            match list.iter_mut().find(|b| b.name == t.name) {
                Some(slot) => *slot = t.clone(),
                None => list.push(t.clone()),
            }
        }
        list
    }

    fn apply_theme_name(&mut self, name: &str) {
        let themes = self.all_themes();
        // The host terminal owns its colors, so "auto" is simply "terminal".
        let name = termax_core::theme::effective_name(name, false);
        match themes.iter().find(|t| t.name == name) {
            Some(def) => match Theme::from_def(def) {
                Ok(t) => self.theme = t,
                Err(e) => self.set_error(e),
            },
            None => self.set_error(format!("unknown theme '{name}'")),
        }
    }

    // ----- hosts -----

    pub fn reload_hosts(&mut self) {
        let (hosts, warning) = hosts::collect(&self.cfg);
        self.hosts = hosts;
        if let Some(w) = warning {
            self.set_error(w);
        }
        self.refilter();
    }

    fn refilter(&mut self) {
        let q = self.filter.value.trim().to_string();
        let mut shown = hosts::visible(&self.hosts, self.ui_state.show_hidden);
        if q.is_empty() {
            history::order(&self.hosts, &mut shown, self.ui_state.sort, &self.history);
            self.filtered = shown;
        } else {
            let mut scored: Vec<(i64, usize)> =
                shown.into_iter().filter_map(|i| hosts::fuzzy_score(&q, &self.hosts[i]).map(|s| (s, i))).collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.host_selected = self.host_selected.min(self.filtered.len().saturating_sub(1));
    }

    pub fn selected_host(&self) -> Option<&HostEntry> {
        self.filtered.get(self.host_selected).map(|&i| &self.hosts[i])
    }

    fn move_host(&mut self, delta: i64) {
        if self.filtered.is_empty() {
            return;
        }
        let max = self.filtered.len() as i64 - 1;
        self.host_selected = (self.host_selected as i64 + delta).clamp(0, max) as usize;
    }

    fn save_config(&mut self) {
        match self.cfg.save(&self.cfg_saved, &self.cfg_path) {
            Ok(()) => self.cfg_saved = self.cfg.clone(),
            Err(e) => self.set_error(format!("saving config: {e:#}")),
        }
    }

    // ----- status -----

    pub fn set_info(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), false, Instant::now()));
    }

    pub fn set_error(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), true, Instant::now()));
    }

    // ----- tabs -----

    fn unique_title(&self, base: String) -> String {
        let mut title = base.clone();
        let mut n = 2;
        while self.tabs.iter().any(|t| t.title == title) {
            title = format!("{base} ({n})");
            n += 1;
        }
        title
    }

    fn push_tab(&mut self, title: String, kind: TabKind) {
        let id = self.next_id;
        self.next_id += 1;
        let title = self.unique_title(title);
        self.tabs.push(Tab { id, title, kind, record: None, forward: None, host: None });
        self.active = self.tabs.len() - 1;
    }

    /// Opens host `name` (hidden hosts too): a terminal, or a file transfer.
    pub fn open_named(&mut self, name: &str, files: bool) {
        let Some(host) = self.hosts.iter().find(|h| h.name == name).cloned() else {
            self.set_error(format!("unknown host: {name}"));
            return;
        };
        if files { self.open_sftp_with(host) } else { self.open_terminal_with(host) }
    }

    fn open_terminal(&mut self) {
        let Some(host) = self.selected_host().cloned() else { return };
        self.open_terminal_with(host);
    }

    fn open_terminal_with(&mut self, host: HostEntry) {
        let (rows, cols) = self.term_area;
        match TerminalSession::spawn(&self.cfg.settings.ssh_binary, &host.shell_args(), rows, cols, self.cfg.settings.scrollback) {
            Ok(s) => {
                self.push_tab(host.name.clone(), TabKind::Terminal(Box::new(s)));
                let tab = self.tabs.last_mut().expect("pushed");
                tab.record = Some((host.name.clone(), ConnKind::Terminal, Instant::now()));
                tab.host = Some(host.name.clone());
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    // ----- status, history, ordering -----

    /// The ssh config files, or an empty set when ssh config is off or unreadable.
    fn load_set(&mut self) -> ConfigSet {
        match hosts::ssh_config_path(&self.cfg) {
            Some(p) => ConfigSet::load(&p).unwrap_or_else(|e| {
                self.set_error(format!("{e:#}"));
                ConfigSet::default()
            }),
            None => ConfigSet::default(),
        }
    }

    /// Checks every listed host (opt-in: never runs unless asked or `on_start`).
    pub fn check_status(&mut self) {
        let sets = hosts::probe_sets(&self.cfg);
        let items: Vec<(String, hosts::Probe)> =
            self.filtered.iter().map(|&i| (self.hosts[i].name.clone(), self.hosts[i].probe(sets.as_deref()))).collect();
        for (n, _) in &items {
            self.statuses.insert(n.clone(), HostStatus::Checking);
        }
        let tx = self.tx.clone();
        status::check_all(&self.rt, items, Arc::new(move |host, status| {
            let _ = tx.send(Msg::Status { host, status });
        }));
    }

    fn record(&mut self, name: &str, kind: ConnKind) {
        self.history.record(name, kind, history::now());
        if let Err(e) = self.history.save(&self.history_path) {
            self.set_error(format!("saving history: {e:#}"));
        }
    }

    fn save_ui_state(&mut self) {
        if let Err(e) = self.ui_state.save(&UiState::path_for(&self.cfg_path)) {
            self.set_error(format!("saving state: {e:#}"));
        }
    }

    fn cycle_sort(&mut self) {
        self.ui_state.sort = self.ui_state.sort.next();
        self.save_ui_state();
        self.refilter();
        self.set_info(format!("Sort: {}", self.ui_state.sort.label()));
    }

    fn toggle_hidden(&mut self) {
        self.ui_state.show_hidden = !self.ui_state.show_hidden;
        self.save_ui_state();
        self.refilter();
        self.set_info(if self.ui_state.show_hidden { "Showing hidden hosts" } else { "Hiding hidden hosts" });
    }

    // ----- forwards -----

    /// Starts forward `f` in its own tab. A forward that is already running
    /// is shown instead; a failed one's tab is replaced.
    pub fn start_forward(&mut self, f: &Forward) {
        let Some(host) = self.hosts.iter().find(|h| h.name == f.host).cloned() else {
            self.set_error(format!("unknown host: {}", f.host));
            return;
        };
        let title = f.title();
        if let Some(i) = self.forward_tab(&title) {
            if let TabKind::Terminal(s) = &mut self.tabs[i].kind {
                if s.is_alive() {
                    self.active = i;
                    return;
                }
            }
            self.tabs.remove(i);
            self.active = self.active.min(self.tabs.len() - 1);
        }
        let (rows, cols) = self.term_area;
        match TerminalSession::spawn(&self.cfg.settings.ssh_binary, &f.ssh_args(&host), rows, cols, self.cfg.settings.scrollback) {
            Ok(s) => {
                self.push_tab(title.clone(), TabKind::Terminal(Box::new(s)));
                if let Some(t) = self.tabs.last_mut() {
                    t.forward = Some(title);
                }
                self.record(&host.name, ConnKind::Forward);
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    fn forward_tab(&self, title: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.forward.as_deref() == Some(title))
    }

    /// "running", "stopped" or "failed: <ssh's reason>" for saved forward `i`.
    pub fn forward_state(&mut self, i: usize) -> String {
        let Some(title) = self.cfg.forwards.get(i).map(|f| f.title()) else { return String::new() };
        let Some(ti) = self.forward_tab(&title) else { return "stopped".into() };
        let TabKind::Terminal(s) = &mut self.tabs[ti].kind else { return "stopped".into() };
        if s.is_alive() {
            return "running".into();
        }
        let text = s.parser.lock().map(|p| p.screen().contents()).unwrap_or_default();
        let lines: Vec<String> = text.lines().map(String::from).collect();
        match termax_core::forward::error_line(&lines) {
            Some(l) => format!("failed: {l}"),
            None => "failed".into(),
        }
    }

    /// Stops saved forward `i` (closes its tab).
    fn stop_forward(&mut self, i: usize) {
        let Some(title) = self.cfg.forwards.get(i).map(|f| f.title()) else { return };
        if let Some(ti) = self.forward_tab(&title) {
            self.tabs.remove(ti);
            self.active = self.active.min(self.tabs.len() - 1);
        }
    }

    fn save_forward_form(&mut self, mut form: ForwardForm) -> Option<Modal> {
        match form.to_forward() {
            Err(e) => {
                form.error = Some(e);
                Some(Modal::Forward(form))
            }
            Ok(f) => {
                match form.editing {
                    Some(i) if i < self.cfg.forwards.len() => self.cfg.forwards[i] = f.clone(),
                    _ => self.cfg.forwards.push(f.clone()),
                }
                self.save_config();
                self.start_forward(&f);
                None
            }
        }
    }

    fn open_sftp(&mut self) {
        let Some(host) = self.selected_host().cloned() else { return };
        self.open_sftp_with(host);
    }

    fn open_sftp_with(&mut self, host: HostEntry) {
        let local = self
            .cfg
            .settings
            .local_start_dir
            .as_deref()
            .map(sshconfig::expand_tilde)
            .or_else(sshconfig::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let id = self.next_id;
        let mut tab = SftpTab::new(id, host.name.clone(), local, self.cfg.settings.show_hidden_files, self.rt.clone(), self.tx.clone());
        tab.connect(host.sftp_launch(&self.cfg.settings.ssh_binary, self.cfg.settings.sftp_batch_mode));
        self.push_tab(format!("{} [sftp]", host.name), TabKind::Sftp(Box::new(tab)));
    }

    fn close_active_tab(&mut self) {
        if self.active == 0 {
            return;
        }
        self.tabs.remove(self.active);
        self.active = self.active.min(self.tabs.len() - 1);
    }

    fn request_quit(&mut self) {
        if self.tabs.len() > 1 {
            let n = self.tabs.len() - 1;
            self.modal = Some(Modal::Confirm {
                text: format!("Quit and close {n} open session{}?", if n == 1 { "" } else { "s" }),
                action: ConfirmAction::Quit,
            });
        } else {
            self.should_quit = true;
        }
    }

    fn open_theme_picker(&mut self) {
        let themes = self.all_themes();
        let selected = themes.iter().position(|t| t.name == self.theme.name).unwrap_or(0);
        self.modal = Some(Modal::ThemePicker { selected, original: self.theme.name.clone() });
    }

    // ----- background messages -----

    pub fn drain_messages(&mut self) {
        let mut connected: Vec<String> = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            if let Msg::Status { host, status } = msg {
                self.statuses.insert(host, status);
                continue;
            }
            let tab_id = match &msg {
                Msg::SftpConnected { tab, .. }
                | Msg::RemoteListing { tab, .. }
                | Msg::TransferProgress { tab, .. }
                | Msg::TransferDone { tab, .. }
                | Msg::RemoteOpDone { tab, .. } => *tab,
                Msg::Status { .. } => unreachable!("handled above"),
            };
            // Messages for tabs that have since been closed are dropped.
            let Some(TabKind::Sftp(s)) = self.tabs.iter_mut().find(|t| t.id == tab_id).map(|t| &mut t.kind) else {
                continue;
            };
            if matches!(&msg, Msg::SftpConnected { result: Ok(_), .. }) {
                connected.push(s.host_name.clone());
            }
            match msg {
                Msg::Status { .. } => {}
                Msg::SftpConnected { result, .. } => s.on_connected(result),
                Msg::RemoteListing { result, .. } => s.on_remote_listing(result),
                Msg::TransferProgress { id, done, .. } => s.on_transfer_progress(id, done),
                Msg::TransferDone { id, result, .. } => s.on_transfer_done(id, result),
                Msg::RemoteOpDone { result, keep, .. } => s.on_remote_op(result, keep),
            }
        }
        for name in connected {
            self.record(&name, ConnKind::Files);
        }
        // A terminal counts as connected once it has shown output and is
        // still running two seconds in.
        let mut done = Vec::new();
        for tab in &mut self.tabs {
            if let (Some((_, _, at)), TabKind::Terminal(s)) = (&tab.record, &mut tab.kind) {
                if at.elapsed() >= Duration::from_secs(2) && s.generation() > 0 && s.is_alive() {
                    let (name, kind, _) = tab.record.take().expect("checked");
                    done.push((name, kind));
                }
            }
        }
        for (name, kind) in done {
            self.record(&name, kind);
        }
        if let Some((_, _, at)) = &self.status {
            if at.elapsed() > Duration::from_secs(6) {
                self.status = None;
            }
        }
    }

    // ----- input -----

    /// Handles the events that were waiting together. In a terminal tab, a
    /// burst of plain keys that holds a line break and at least
    /// `PASTE_BURST_CHARS` characters can only be a paste: it goes through
    /// the paste rule instead of straight to the shell.
    pub fn handle_batch(&mut self, events: Vec<Event>) {
        if self.paste_bursts
            && self.modal.is_none()
            && matches!(self.tabs[self.active].kind, TabKind::Terminal(_))
            && let Some(text) = key_burst_text(&events)
        {
            self.handle_paste(&text);
            return;
        }
        for ev in events {
            self.handle_event(ev);
        }
    }

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.handle_key(key),
            Event::Paste(text) => self.handle_paste(&text),
            Event::Mouse(m) => self.handle_mouse(m),
            _ => {}
        }
    }

    /// The active terminal has a program that asked for the mouse.
    pub fn wants_mouse(&self) -> bool {
        self.modal.is_none()
            && matches!(&self.tabs[self.active].kind, TabKind::Terminal(s)
                if s.parser.lock().is_ok_and(|p| p.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None))
    }

    /// Reports a mouse event to the program in the active terminal. Shift
    /// is left to the user's terminal (its own selection).
    fn handle_mouse(&mut self, m: MouseEvent) {
        if m.modifiers.contains(KeyModifiers::SHIFT) || !self.wants_mouse() {
            return;
        }
        let (ox, oy) = self.term_origin;
        let (rows, cols) = self.term_area;
        let (Some(col), Some(row)) = (m.column.checked_sub(ox), m.row.checked_sub(oy)) else { return };
        if col >= cols || row >= rows {
            return;
        }
        let button = |b: MouseButton| match b {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        let ev = match m.kind {
            MouseEventKind::Down(b) => MouseEv::Press(button(b)),
            MouseEventKind::Up(b) => MouseEv::Release(button(b)),
            MouseEventKind::Drag(b) => MouseEv::Drag(button(b)),
            MouseEventKind::Moved => MouseEv::Move,
            MouseEventKind::ScrollUp => MouseEv::WheelUp,
            MouseEventKind::ScrollDown => MouseEv::WheelDown,
            _ => return,
        };
        let TabKind::Terminal(s) = &mut self.tabs[self.active].kind else { return };
        let Ok((mode, enc)) = s.parser.lock().map(|p| (p.screen().mouse_protocol_mode(), p.screen().mouse_protocol_encoding())) else {
            return;
        };
        let (alt, ctrl) = (m.modifiers.contains(KeyModifiers::ALT), m.modifiers.contains(KeyModifiers::CONTROL));
        if let Some(bytes) = termx::encode_mouse(mode, enc, ev, col, row, false, alt, ctrl) {
            s.write(&bytes);
        }
    }

    /// "match i/n" while searching ("no matches", or empty before typing).
    pub fn search_status(&self) -> Option<String> {
        let Some(Modal::Search { query, matches, current }) = &self.modal else { return None };
        Some(match (query.value.is_empty(), matches.len()) {
            (true, _) => String::new(),
            (false, 0) => "no matches".into(),
            (false, n) => format!("match {}/{n}", current + 1),
        })
    }

    /// The snippets usable in the active terminal tab.
    pub fn tab_snippets(&self) -> Vec<&termx::Snippet> {
        let host = self.tabs[self.active].host.as_deref().unwrap_or("");
        termx::snippets_for(&self.cfg.snippets, host)
    }

    /// Pastes `text` into the active terminal (the paste rule may ask
    /// first), then Enter if `enter`.
    fn send_to_terminal(&mut self, text: &str, enter: bool) {
        let title = self.tabs[self.active].title.clone();
        let TabKind::Terminal(s) = &mut self.tabs[self.active].kind else { return };
        let bracketed = s.parser.lock().map(|p| p.screen().bracketed_paste()).unwrap_or(false);
        match termx::paste_lines(text, bracketed, self.cfg.settings.confirm_multiline_paste) {
            Some(n) => {
                let what = if n == 1 { "1 line".to_string() } else { format!("{n} lines") };
                let text_owned = text.to_string();
                self.modal =
                    Some(Modal::Confirm { text: format!("Paste {what} into {title}?"), action: ConfirmAction::Paste { text: text_owned, enter } });
            }
            None => paste_then_enter(s, text, enter),
        }
    }

    /// Re-runs the search for `query`; the newest match becomes current and
    /// the view scrolls to it.
    fn run_search(&mut self, query: &TextInput) -> (Vec<Match>, usize) {
        let TabKind::Terminal(s) = &mut self.tabs[self.active].kind else { return (vec![], 0) };
        let Ok(mut p) = s.parser.lock() else { return (vec![], 0) };
        let (lines, history) = termx::screen_lines(&mut p);
        drop(p);
        let matches = termx::find(&lines, &query.value);
        let current = matches.len().saturating_sub(1);
        self.scroll_to_match(&matches, current, history);
        (matches, current)
    }

    fn scroll_to_match(&mut self, matches: &[Match], current: usize, history: usize) {
        let TabKind::Terminal(s) = &mut self.tabs[self.active].kind else { return };
        let Some(m) = matches.get(current) else { return };
        let rows = s.rows() as usize;
        let off = termx::scroll_offset_for(m.line, history, rows).saturating_sub(rows / 3);
        if let Ok(mut p) = s.parser.lock() {
            p.screen_mut().set_scrollback(off);
        }
    }

    fn history_len(&self) -> usize {
        let TabKind::Terminal(s) = &self.tabs[self.active].kind else { return 0 };
        s.parser.lock().map(|mut p| termx::screen_lines(&mut p).1).unwrap_or(0)
    }

    fn handle_paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or("");
        match &mut self.modal {
            Some(Modal::Editor(ed)) => {
                if let Some(i) = ed.focused_input() {
                    i.insert_str(line);
                }
                return;
            }
            Some(Modal::Forward(f)) => {
                if let Some(i) = f.focused_input() {
                    i.insert_str(line);
                }
                return;
            }
            Some(Modal::Prompt(p)) => {
                p.input.insert_str(line);
                return;
            }
            Some(_) => return,
            None => {}
        }
        match &mut self.tabs[self.active].kind {
            TabKind::Terminal(_) => self.send_to_terminal(text, false),
            TabKind::Hosts if self.filtering => {
                self.filter.insert_str(line);
                self.refilter();
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.modal.is_some() {
            self.handle_modal_key(key);
            return;
        }
        if self.handle_global_key(key) {
            return;
        }
        match &self.tabs[self.active].kind {
            TabKind::Hosts => self.hosts_key(key),
            TabKind::Terminal(_) => self.terminal_key(key),
            TabKind::Sftp(_) => self.sftp_key(key),
        }
    }

    /// Alt-based shortcuts work everywhere, including inside terminal tabs.
    fn handle_global_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match key.code {
            KeyCode::Char(c @ '1'..='9') => {
                let i = (c as u8 - b'1') as usize;
                if i < self.tabs.len() {
                    self.active = i;
                }
            }
            KeyCode::Right | KeyCode::Char(']') => self.active = (self.active + 1) % self.tabs.len(),
            KeyCode::Left | KeyCode::Char('[') => self.active = (self.active + self.tabs.len() - 1) % self.tabs.len(),
            KeyCode::Char('w') => self.close_active_tab(),
            KeyCode::Char('t') => self.open_theme_picker(),
            KeyCode::Char('h') => self.modal = Some(Modal::Help),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('f') if matches!(self.tabs[self.active].kind, TabKind::Terminal(_)) => {
                self.modal = Some(Modal::Search { query: TextInput::default(), matches: vec![], current: 0 });
            }
            KeyCode::Char('s') if matches!(self.tabs[self.active].kind, TabKind::Terminal(_)) => {
                self.modal = Some(Modal::Snippets { selected: 0 });
            }
            _ => return false,
        }
        true
    }

    fn hosts_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.filter = TextInput::default();
                    self.filtering = false;
                }
                KeyCode::Enter => {
                    self.filtering = false;
                    self.open_terminal();
                }
                KeyCode::Down => self.move_host(1),
                KeyCode::Up => self.move_host(-1),
                _ => {
                    if self.filter.handle_key(key) {
                        self.host_selected = 0;
                    }
                }
            }
            self.refilter();
            return;
        }
        match key.code {
            KeyCode::Char('c') if ctrl => self.request_quit(),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Esc if !self.filter.value.is_empty() => {
                self.filter = TextInput::default();
                self.refilter();
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_host(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_host(-1),
            KeyCode::PageDown => self.move_host(10),
            KeyCode::PageUp => self.move_host(-10),
            KeyCode::Home | KeyCode::Char('g') => self.host_selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.move_host(i64::MAX / 2),
            KeyCode::Enter => self.open_terminal(),
            KeyCode::Char('s') => self.open_sftp(),
            KeyCode::Char('n') => self.new_host(),
            KeyCode::Char('e') => self.edit_selected(),
            KeyCode::Char('d') | KeyCode::Delete => self.delete_selected(),
            KeyCode::Char('r') => {
                self.reload_config();
            }
            KeyCode::Char('t') => self.open_theme_picker(),
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char('p') => self.check_status(),
            KeyCode::Char('o') => self.cycle_sort(),
            KeyCode::Char('H') => self.toggle_hidden(),
            KeyCode::Char('f') => {
                if let Some(h) = self.selected_host() {
                    self.modal = Some(Modal::Forward(ForwardForm::new(&h.name.clone())));
                }
            }
            KeyCode::Char('F') => self.modal = Some(Modal::Forwards { selected: 0 }),
            _ => {}
        }
    }

    fn new_host(&mut self) {
        let set = self.load_set();
        let dest = if set.docs.is_empty() { Dest::Termax } else { Dest::SshConfig(0) };
        self.modal = Some(Modal::Editor(Editor::new(HostForm::new_host(dest), set)));
    }

    fn reload_config(&mut self) {
        match Config::load_or_create(&self.cfg_path) {
            Ok(cfg) => {
                self.cfg_saved = cfg.clone();
                self.cfg = cfg;
                self.apply_theme_name(&self.cfg.settings.theme.clone());
                self.reload_hosts();
                self.set_info(format!("Reloaded {} hosts", self.hosts.len()));
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    fn edit_selected(&mut self) {
        let Some(h) = self.selected_host().cloned() else { return };
        let set = self.load_set();
        match HostForm::for_entry(&h, &self.cfg, &set) {
            Ok(form) => self.modal = Some(Modal::Editor(Editor::new(form, set))),
            Err(why) => self.set_error(why),
        }
    }

    fn delete_selected(&mut self) {
        let Some(h) = self.selected_host().cloned() else { return };
        let text = match (&h.source, &h.source_file) {
            (Source::SshConfig, Some(file)) => {
                if let termax_core::sshconfig::edit::Locate::ReadOnly(why) = self.load_set().locate(&h.name) {
                    self.set_error(why);
                    return;
                }
                format!("Delete '{}' from {}? A backup is kept.", h.name, display_path(file))
            }
            _ => format!("Delete connection '{}'?", h.name),
        };
        self.modal = Some(Modal::Confirm { text, action: ConfirmAction::DeleteHost(h.name) });
    }

    /// Writes what a host change touched, then refreshes the list.
    fn after_host_change(&mut self, saved: Saved, set: &mut ConfigSet) -> Result<(), String> {
        if let Saved::Ssh(_) = saved {
            set.save(&backups_dir(&self.cfg_path)).map_err(|e| format!("{e:#}"))?;
        } else {
            self.save_config();
        }
        self.reload_hosts();
        Ok(())
    }

    fn terminal_key(&mut self, key: KeyEvent) {
        let TabKind::Terminal(s) = &mut self.tabs[self.active].kind else { return };
        if !s.is_alive() {
            if key.code == KeyCode::Enter {
                if let Err(e) = s.respawn() {
                    self.set_error(format!("{e:#}"));
                }
            }
            return;
        }
        let half = (s.rows() / 2).max(1) as i32;
        match key.code {
            KeyCode::PageUp if key.modifiers.contains(KeyModifiers::SHIFT) => s.scroll(half),
            KeyCode::PageDown if key.modifiers.contains(KeyModifiers::SHIFT) => s.scroll(-half),
            _ => {
                let bytes = encode_key(key, s.application_cursor());
                s.write(&bytes);
            }
        }
    }

    fn sftp_key(&mut self, key: KeyEvent) {
        let TabKind::Sftp(s) = &mut self.tabs[self.active].kind else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Status::Failed(_) | Status::Connecting = s.status {
            // Only the local pane is usable until connected.
            if matches!(key.code, KeyCode::Char('r')) {
                s.reconnect();
                return;
            }
        }
        let side = s.focus;
        let mut modal = None;
        let mut err = None;
        match key.code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => s.focus = side.other(),
            KeyCode::Down | KeyCode::Char('j') => s.pane_mut(side).move_by(1),
            KeyCode::Up | KeyCode::Char('k') => s.pane_mut(side).move_by(-1),
            KeyCode::PageDown => s.pane_mut(side).move_by(15),
            KeyCode::PageUp => s.pane_mut(side).move_by(-15),
            KeyCode::Home => s.pane_mut(side).selected = 0,
            KeyCode::End => s.pane_mut(side).move_by(i64::MAX / 2),
            KeyCode::Enter => s.open_selected(),
            KeyCode::Backspace => s.go_up(),
            KeyCode::Char('c') | KeyCode::F(5) if !ctrl => {
                if let Err(e) = s.copy_selected() {
                    err = Some(e);
                }
            }
            KeyCode::Char('m') | KeyCode::F(7) => {
                modal = Some(Modal::Prompt(Prompt {
                    title: "New directory".into(),
                    input: TextInput::default(),
                    action: PromptAction::Mkdir,
                }))
            }
            KeyCode::Char('R') | KeyCode::F(2) => {
                if let Some((_, e)) = s.selected_path() {
                    modal = Some(Modal::Prompt(Prompt {
                        title: format!("Rename '{}'", e.name),
                        input: TextInput::new(e.name.clone()),
                        action: PromptAction::Rename { from: e.name },
                    }))
                }
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::F(8) => {
                if let Some((_, e)) = s.selected_path() {
                    let what = if e.is_dir { "directory (recursively)" } else { "file" };
                    modal = Some(Modal::Confirm {
                        text: format!("Delete {what} '{}' on the {} side?", e.name, if side == Side::Local { "local" } else { "remote" }),
                        action: ConfirmAction::DeleteFile { name: e.name, is_dir: e.is_dir },
                    })
                }
            }
            KeyCode::Char('g') => {
                modal = Some(Modal::Prompt(Prompt {
                    title: "Go to path".into(),
                    input: TextInput::new(s.pane(side).path.clone()),
                    action: PromptAction::Goto,
                }))
            }
            KeyCode::Char('r') => s.refresh(side),
            KeyCode::Char('.') => s.toggle_hidden(),
            KeyCode::Char('x') => s.clear_finished_transfers(),
            _ => {}
        }
        if let Some(e) = err {
            self.set_error(e);
        }
        if modal.is_some() {
            self.modal = modal;
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.modal.take() else { return };
        self.modal = match modal {
            Modal::Help => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?') => None,
                _ => Some(modal),
            },
            Modal::ThemePicker { mut selected, original } => {
                let names: Vec<String> = self.all_themes().into_iter().map(|t| t.name).collect();
                match key.code {
                    KeyCode::Esc => {
                        self.apply_theme_name(&original);
                        None
                    }
                    KeyCode::Enter => {
                        self.cfg.settings.theme = names[selected].clone();
                        self.save_config();
                        self.set_info(format!("Theme: {}", names[selected]));
                        None
                    }
                    code => {
                        match code {
                            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % names.len(),
                            KeyCode::Up | KeyCode::Char('k') => selected = (selected + names.len() - 1) % names.len(),
                            _ => {}
                        }
                        self.apply_theme_name(&names[selected]);
                        Some(Modal::ThemePicker { selected, original })
                    }
                }
            }
            Modal::Confirm { text, action } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.run_confirm(action);
                    None
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => None,
                _ => Some(Modal::Confirm { text, action }),
            },
            Modal::Search { mut query, mut matches, mut current } => match key.code {
                KeyCode::Esc => {
                    if let TabKind::Terminal(s) = &mut self.tabs[self.active].kind
                        && let Ok(mut p) = s.parser.lock()
                    {
                        p.screen_mut().set_scrollback(0);
                    }
                    None
                }
                KeyCode::Enter => {
                    let n = matches.len();
                    if n > 0 {
                        // Enter: next older match; Shift+Enter: newer.
                        current = if key.modifiers.contains(KeyModifiers::SHIFT) { (current + 1) % n } else { (current + n - 1) % n };
                        let history = self.history_len();
                        self.scroll_to_match(&matches, current, history);
                    }
                    Some(Modal::Search { query, matches, current })
                }
                _ => {
                    if query.handle_key(key) {
                        (matches, current) = self.run_search(&query);
                    }
                    Some(Modal::Search { query, matches, current })
                }
            },
            Modal::Snippets { selected } => {
                let n = self.tab_snippets().len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => None,
                    KeyCode::Down | KeyCode::Char('j') if n > 0 => Some(Modal::Snippets { selected: (selected + 1) % n }),
                    KeyCode::Up | KeyCode::Char('k') if n > 0 => Some(Modal::Snippets { selected: (selected + n - 1) % n }),
                    KeyCode::Enter if selected < n => {
                        let send = self.tab_snippets()[selected].send();
                        match send {
                            termx::SnippetSend::Type(text) => {
                                if let TabKind::Terminal(s) = &mut self.tabs[self.active].kind {
                                    s.write(text.as_bytes());
                                }
                            }
                            // May open the paste confirmation.
                            termx::SnippetSend::Paste { body, enter } => self.send_to_terminal(&body, enter),
                        }
                        self.modal.take()
                    }
                    _ => Some(Modal::Snippets { selected }),
                }
            }
            Modal::Prompt(mut p) => match key.code {
                KeyCode::Esc => None,
                KeyCode::Enter => {
                    self.run_prompt(p);
                    None
                }
                _ => {
                    p.input.handle_key(key);
                    Some(Modal::Prompt(p))
                }
            },
            Modal::Editor(mut ed) => match ed.handle_key(key) {
                Outcome::Keep => Some(Modal::Editor(ed)),
                Outcome::Cancel => None,
                Outcome::Save => self.save_editor(ed),
            },
            Modal::Forward(mut f) => match f.handle_key(key) {
                Outcome::Keep => Some(Modal::Forward(f)),
                Outcome::Cancel => None,
                Outcome::Save => self.save_forward_form(f),
            },
            Modal::Forwards { mut selected } => {
                let n = self.cfg.forwards.len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => None,
                    KeyCode::Down | KeyCode::Char('j') if n > 0 => Some(Modal::Forwards { selected: (selected + 1) % n }),
                    KeyCode::Up | KeyCode::Char('k') if n > 0 => Some(Modal::Forwards { selected: (selected + n - 1) % n }),
                    KeyCode::Enter if selected < n => {
                        let f = self.cfg.forwards[selected].clone();
                        self.start_forward(&f);
                        None
                    }
                    KeyCode::Char('s') if selected < n => {
                        self.stop_forward(selected);
                        Some(Modal::Forwards { selected })
                    }
                    KeyCode::Char('e') if selected < n => {
                        Some(Modal::Forward(ForwardForm::edit(&self.cfg.forwards[selected], selected)))
                    }
                    KeyCode::Char('d') | KeyCode::Delete if selected < n => Some(Modal::Confirm {
                        text: format!("Delete forward '{}'?", self.cfg.forwards[selected].title()),
                        action: ConfirmAction::DeleteForward(selected),
                    }),
                    _ => {
                        selected = selected.min(n.saturating_sub(1));
                        Some(Modal::Forwards { selected })
                    }
                }
            }
        };
    }

    /// Returns the modal to keep showing (with an error) or `None` on success.
    fn save_editor(&mut self, mut ed: Editor) -> Option<Modal> {
        let taken = hostform::taken_names(&self.hosts);
        // Edit a copy: if the write fails, the form can be saved again as is.
        let mut set = ed.set.clone();
        let saved = match ed.form.save(&mut self.cfg, &mut set, &taken) {
            Ok(s) => s,
            Err(e) => {
                ed.error = Some(e);
                return Some(Modal::Editor(ed));
            }
        };
        let name = saved.name().to_string();
        if let Err(e) = self.after_host_change(saved, &mut set) {
            ed.error = Some(e);
            return Some(Modal::Editor(ed));
        }
        if let Some(pos) = self.filtered.iter().position(|&i| self.hosts[i].name == name) {
            self.host_selected = pos;
        }
        self.set_info(format!("Saved '{name}'"));
        None
    }

    fn run_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::Quit => self.should_quit = true,
            ConfirmAction::Paste { text, enter } => {
                if let TabKind::Terminal(s) = &mut self.tabs[self.active].kind {
                    paste_then_enter(s, &text, enter);
                }
            }
            ConfirmAction::DeleteHost(name) => {
                let Some(h) = self.hosts.iter().find(|h| h.name == name).cloned() else { return };
                let mut set = self.load_set();
                let res = hostform::delete(&h, &mut self.cfg, &mut set).and_then(|s| self.after_host_change(s, &mut set));
                match res {
                    Ok(()) => self.set_info(format!("Deleted '{name}'")),
                    Err(e) => self.set_error(e),
                }
            }
            ConfirmAction::DeleteForward(i) => {
                if i < self.cfg.forwards.len() {
                    self.cfg.forwards.remove(i);
                    self.save_config();
                }
                self.modal = Some(Modal::Forwards { selected: i.saturating_sub(1) });
            }
            ConfirmAction::DeleteFile { name, is_dir } => {
                if let TabKind::Sftp(s) = &mut self.tabs[self.active].kind {
                    s.file_op(FileOp::Delete { name, is_dir });
                }
            }
        }
    }

    fn run_prompt(&mut self, p: Prompt) {
        let value = p.input.value.trim().to_string();
        if value.is_empty() {
            return;
        }
        let TabKind::Sftp(s) = &mut self.tabs[self.active].kind else { return };
        match p.action {
            PromptAction::Mkdir => s.file_op(FileOp::Mkdir(value)),
            PromptAction::Rename { from } if from != value => s.file_op(FileOp::Rename { from, to: value }),
            PromptAction::Rename { .. } => {}
            PromptAction::Goto => {
                let side = s.focus;
                let path = if side == Side::Local { sshconfig::expand_tilde(&value).display().to_string() } else { value };
                s.navigate(side, path, None);
            }
        }
    }
}

/// Fewest characters in a key burst read as a paste (no one types this
/// many keys between two reads of the input, ~30 ms apart).
const PASTE_BURST_CHARS: usize = 8;

/// The text of `events` if they are all plain key presses (and releases)
/// forming a paste: a line break and at least `PASTE_BURST_CHARS` chars.
fn key_burst_text(events: &[Event]) -> Option<String> {
    let mut text = String::new();
    for e in events {
        let Event::Key(k) = e else { return None };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return None;
        }
        match k.code {
            KeyCode::Char(c) => text.push(c),
            KeyCode::Enter => text.push('\r'),
            KeyCode::Tab => text.push('\t'),
            _ => return None,
        }
    }
    (text.contains('\r') && text.chars().count() >= PASTE_BURST_CHARS).then_some(text)
}

/// Pastes `text`; then Enter, after the paste ends, so the shell runs it.
fn paste_then_enter(s: &mut TerminalSession, text: &str, enter: bool) {
    s.paste(text);
    if enter {
        s.write(b"\r");
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
