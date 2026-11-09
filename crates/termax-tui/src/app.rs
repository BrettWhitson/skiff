use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use termax_core::fs::connect::RemoteConn;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use termax_core::config::{Config, Connection};
use termax_core::hosts::{self, HostEntry, Source};
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
}

pub const EDITOR_LABELS: [&str; 9] =
    ["Name", "Host", "User", "Port", "Identity file", "ProxyJump", "Extra ssh args", "Remote dir", "Tags"];

pub struct Editor {
    pub fields: Vec<TextInput>,
    pub focus: usize,
    /// Index of the custom connection being edited, `None` when creating.
    pub editing: Option<usize>,
    pub error: Option<String>,
}

impl Editor {
    fn new(c: &Connection, editing: Option<usize>) -> Self {
        let o = |v: &Option<String>| v.clone().unwrap_or_default();
        let fields = [
            c.name.clone(),
            c.host.clone(),
            o(&c.user),
            c.port.map(|p| p.to_string()).unwrap_or_default(),
            o(&c.identity_file),
            o(&c.proxy_jump),
            shell_words::join(&c.extra_args),
            o(&c.remote_dir),
            c.tags.join(", "),
        ];
        Self { fields: fields.into_iter().map(TextInput::new).collect(), focus: 0, editing, error: None }
    }

    fn to_connection(&self) -> Result<Connection, String> {
        let v = |i: usize| self.fields[i].value.trim().to_string();
        let opt = |i: usize| Some(v(i)).filter(|s| !s.is_empty());
        if v(0).is_empty() {
            return Err("Name is required".into());
        }
        if v(1).is_empty() {
            return Err("Host is required".into());
        }
        let port = match opt(3) {
            None => None,
            Some(p) => Some(p.parse::<u16>().map_err(|_| format!("Invalid port: {p}"))?),
        };
        let extra_args = shell_words::split(&v(6)).map_err(|e| format!("Extra ssh args: {e}"))?;
        Ok(Connection {
            name: v(0),
            host: v(1),
            user: opt(2),
            port,
            identity_file: opt(4),
            proxy_jump: opt(5),
            extra_args,
            remote_dir: opt(7),
            tags: v(8).split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        })
    }
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
    DeleteConnection(usize),
    DeleteFile { name: String, is_dir: bool },
    Quit,
}

pub enum Modal {
    Help,
    ThemePicker { selected: usize, original: String },
    Editor(Editor),
    Prompt(Prompt),
    Confirm { text: String, action: ConfirmAction },
}

pub struct App {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    pub theme: Theme,
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
    next_id: u64,
    rt: Handle,
    tx: UnboundedSender<Msg>,
    rx: UnboundedReceiver<Msg>,
}

impl App {
    pub fn new(cfg: Config, cfg_path: PathBuf, rt: Handle) -> Self {
        let (tx, rx) = unbounded_channel();
        let mut app = Self {
            theme: Theme::from_def(&theme::builtin()[0]).expect("builtin theme"),
            cfg,
            cfg_path,
            hosts: vec![],
            filtered: vec![],
            host_selected: 0,
            host_offset: 0,
            filter: TextInput::default(),
            filtering: false,
            tabs: vec![Tab { id: 0, title: "Hosts".into(), kind: TabKind::Hosts }],
            active: 0,
            modal: None,
            status: None,
            should_quit: false,
            term_area: (24, 80),
            next_id: 1,
            rt,
            tx,
            rx,
        };
        app.apply_theme_name(&app.cfg.settings.theme.clone());
        app.reload_hosts();
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
        let mut scored: Vec<(i64, usize)> = self
            .hosts
            .iter()
            .enumerate()
            .filter_map(|(i, h)| hosts::fuzzy_score(&q, h).map(|s| (s, i)))
            .collect();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.filtered = scored.into_iter().map(|(_, i)| i).collect();
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
        if let Err(e) = self.cfg.save(&self.cfg_path) {
            self.set_error(format!("saving config: {e:#}"));
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
        self.tabs.push(Tab { id, title, kind });
        self.active = self.tabs.len() - 1;
    }

    fn open_terminal(&mut self) {
        let Some(host) = self.selected_host().cloned() else { return };
        let (rows, cols) = self.term_area;
        match TerminalSession::spawn(&self.cfg.settings.ssh_binary, &host.shell_args(), rows, cols, self.cfg.settings.scrollback) {
            Ok(s) => self.push_tab(host.name.clone(), TabKind::Terminal(Box::new(s))),
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    fn open_sftp(&mut self) {
        let Some(host) = self.selected_host().cloned() else { return };
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
        while let Ok(msg) = self.rx.try_recv() {
            let tab_id = match &msg {
                Msg::SftpConnected { tab, .. }
                | Msg::RemoteListing { tab, .. }
                | Msg::TransferProgress { tab, .. }
                | Msg::TransferDone { tab, .. }
                | Msg::RemoteOpDone { tab, .. } => *tab,
            };
            // Messages for tabs that have since been closed are dropped.
            let Some(TabKind::Sftp(s)) = self.tabs.iter_mut().find(|t| t.id == tab_id).map(|t| &mut t.kind) else {
                continue;
            };
            match msg {
                Msg::SftpConnected { result, .. } => s.on_connected(result),
                Msg::RemoteListing { result, .. } => s.on_remote_listing(result),
                Msg::TransferProgress { id, done, .. } => s.on_transfer_progress(id, done),
                Msg::TransferDone { id, result, .. } => s.on_transfer_done(id, result),
                Msg::RemoteOpDone { result, keep, .. } => s.on_remote_op(result, keep),
            }
        }
        if let Some((_, _, at)) = &self.status {
            if at.elapsed() > Duration::from_secs(6) {
                self.status = None;
            }
        }
    }

    // ----- input -----

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.handle_key(key),
            Event::Paste(text) => self.handle_paste(&text),
            _ => {}
        }
    }

    fn handle_paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or("");
        match &mut self.modal {
            Some(Modal::Editor(ed)) => {
                let f = ed.focus;
                ed.fields[f].insert_str(line);
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
            TabKind::Terminal(s) => s.paste(text),
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
            KeyCode::Char('n') => self.modal = Some(Modal::Editor(Editor::new(&Connection::default(), None))),
            KeyCode::Char('e') => self.edit_selected(),
            KeyCode::Char('d') | KeyCode::Delete => self.delete_selected(),
            KeyCode::Char('r') => {
                self.reload_config();
            }
            KeyCode::Char('t') => self.open_theme_picker(),
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            _ => {}
        }
    }

    fn reload_config(&mut self) {
        match Config::load_or_create(&self.cfg_path) {
            Ok(cfg) => {
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
        let ed = match h.source {
            Source::Custom(i) => Editor::new(&self.cfg.connections[i], Some(i)),
            // OpenSSH hosts are read-only: editing makes a termax copy.
            Source::SshConfig => {
                self.set_info("ssh_config hosts are read-only — saving creates a termax connection");
                Editor::new(
                    &Connection {
                        name: format!("{} (copy)", h.name),
                        host: h.hostname.clone(),
                        user: h.user.clone(),
                        port: h.port,
                        identity_file: h.identity_file.clone(),
                        proxy_jump: h.proxy_jump.clone(),
                        ..Default::default()
                    },
                    None,
                )
            }
        };
        self.modal = Some(Modal::Editor(ed));
    }

    fn delete_selected(&mut self) {
        let Some(h) = self.selected_host() else { return };
        match h.source {
            Source::Custom(i) => {
                self.modal = Some(Modal::Confirm {
                    text: format!("Delete connection '{}'?", h.name),
                    action: ConfirmAction::DeleteConnection(i),
                })
            }
            Source::SshConfig => self.set_error("Hosts from ssh_config can only be removed by editing that file"),
        }
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
            Modal::Editor(mut ed) => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let n = ed.fields.len();
                match key.code {
                    KeyCode::Esc => None,
                    KeyCode::Char('s') if ctrl => self.save_editor(ed),
                    KeyCode::Enter if ed.focus == n - 1 => self.save_editor(ed),
                    KeyCode::Enter | KeyCode::Tab | KeyCode::Down => {
                        ed.focus = (ed.focus + 1) % n;
                        Some(Modal::Editor(ed))
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        ed.focus = (ed.focus + n - 1) % n;
                        Some(Modal::Editor(ed))
                    }
                    _ => {
                        let f = ed.focus;
                        ed.fields[f].handle_key(key);
                        Some(Modal::Editor(ed))
                    }
                }
            }
        };
    }

    /// Returns the modal to keep showing (with an error) or `None` on success.
    fn save_editor(&mut self, mut ed: Editor) -> Option<Modal> {
        let conn = match ed.to_connection() {
            Ok(c) => c,
            Err(e) => {
                ed.error = Some(e);
                return Some(Modal::Editor(ed));
            }
        };
        let dup = self.cfg.connections.iter().enumerate().any(|(i, c)| c.name == conn.name && Some(i) != ed.editing);
        if dup {
            ed.error = Some(format!("A connection named '{}' already exists", conn.name));
            return Some(Modal::Editor(ed));
        }
        let name = conn.name.clone();
        match ed.editing {
            Some(i) => self.cfg.connections[i] = conn,
            None => self.cfg.connections.push(conn),
        }
        self.save_config();
        self.reload_hosts();
        if let Some(pos) = self.filtered.iter().position(|&i| self.hosts[i].name == name && matches!(self.hosts[i].source, Source::Custom(_))) {
            self.host_selected = pos;
        }
        self.set_info(format!("Saved '{name}'"));
        None
    }

    fn run_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::Quit => self.should_quit = true,
            ConfirmAction::DeleteConnection(i) => {
                let c = self.cfg.connections.remove(i);
                self.save_config();
                self.reload_hosts();
                self.set_info(format!("Deleted '{}'", c.name));
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
