//! The TUI's whole state and behavior: the host list, tabs (terminals, splits,
//! SFTP browsers, forwards), modal dialogs, and the handling of keys, mouse,
//! pastes and the `Msg`s background tasks send back. `ui` draws this state;
//! `main` runs the loop that feeds events in and drains messages each turn.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use skiff_core::askpass::{AskKind, AskRequest, Broker};
use skiff_core::fs::connect::RemoteConn;
use skiff_core::fs::transfer::{OverwriteDecision, TransferSummary};
use skiff_core::termx::{self, MouseEv, TermSearch};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::forms::{Editor, ForwardForm, Outcome};
use crate::input::TextInput;
use crate::keys::{Action, Keymap};
use crate::sftp_tab::{FileEntry, FileOp, SftpTab, Side, Status};
use crate::tui_theme::Theme;
use skiff_core::config::{Config, StatusChecks, TuiMouse};
use skiff_core::forward::Forward;
use skiff_core::history::{self, ConnKind, History};
use skiff_core::hostform::{self, Dest, HostForm, Saved};
use skiff_core::hosts::{self, HostEntry, Source};
use skiff_core::panes;
use skiff_core::sshconfig;
use skiff_core::sshconfig::edit::{ConfigSet, backups_dir, display_path};
use skiff_core::state::UiState;
use skiff_core::status::{self, Status as HostStatus};
use skiff_core::terminal::{TerminalSession, encode_key};

/// How long a question must be on screen before it takes keys.
const QUESTION_DELAY: Duration = Duration::from_millis(250);

/// A terminal session's screen parser, shared so a session can be identified
/// (and written to) after focus moves away from it.
type SharedParser = Arc<std::sync::Mutex<vt100::Parser>>;
use skiff_core::theme::{self, ThemeDef};

/// Results from background tasks, delivered to the UI thread.
///
/// `tab` fields are tab ids (`Tab::id`), not indexes, so a message for a tab
/// closed meanwhile is simply dropped. `Status` is a host reachability probe's
/// result.
pub enum Msg {
    /// An SFTP tab's connection finished (or failed).
    SftpConnected {
        tab: u64,
        result: Result<Arc<RemoteConn>, String>,
    },
    /// A remote directory listing as (path, entries, name to select); `seq`
    /// lets the tab ignore replies to listings it has since replaced.
    RemoteListing {
        tab: u64,
        seq: u64,
        result: Result<(String, Vec<FileEntry>, Option<String>), String>,
    },
    /// Transfer `id` has moved `done` bytes so far.
    TransferProgress {
        tab: u64,
        id: u64,
        done: u64,
    },
    /// Transfer `id` finished, with its summary or error.
    TransferDone {
        tab: u64,
        id: u64,
        result: Result<TransferSummary, String>,
    },
    /// Transfer `id` found `path` already there: overwrite it?
    TransferAsk {
        tab: u64,
        id: u64,
        path: String,
        reply: oneshot::Sender<OverwriteDecision>,
    },
    /// A remote file operation (mkdir, rename, delete) finished; `keep`
    /// names the entry to keep selected after the relisting.
    RemoteOpDone {
        tab: u64,
        result: Result<(), String>,
        keep: Option<String>,
    },
    Status {
        host: String,
        status: HostStatus,
    },
    /// ssh (connecting an SFTP tab) asks for a password, code or host-key answer.
    Ask(AskRequest),
}

/// A question from the background that waits for its turn on screen.
pub enum Question {
    /// ssh asks for a password, code or host-key answer.
    Ask(AskRequest),
    /// A transfer asks whether to overwrite `path` (see `Msg::TransferAsk`).
    Overwrite { tab: u64, id: u64, path: String, reply: oneshot::Sender<OverwriteDecision> },
}

/// How a split terminal tab lays out its two panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// Side by side.
    Right,
    /// One above the other.
    Down,
}

/// A terminal tab split in two. The focused pane's session is the tab's
/// `TabKind::Terminal`, so everything that acts on "the terminal" (keys,
/// paste, search, mouse) reaches the focused pane.
pub struct Split {
    /// The pane without focus.
    pub other: Box<TerminalSession>,
    /// Side by side or stacked.
    pub dir: SplitDir,
    /// The focused pane is the first one (left or top).
    pub focus_first: bool,
    /// The first (left or top) pane's share of the tab.
    pub ratio: f32,
}

/// What a tab shows.
pub enum TabKind {
    /// The host list (always tab 0, never closed).
    Hosts,
    /// An ssh session (a shell, or a port forward); the focused pane when split.
    Terminal(Box<TerminalSession>),
    /// A dual-pane SFTP browser.
    Sftp(Box<SftpTab>),
}

/// One tab in the tab bar.
pub struct Tab {
    /// Stable id that background messages address the tab by.
    pub id: u64,
    /// Label in the tab bar (numbered when opened to be unique).
    pub title: String,
    pub kind: TabKind,
    /// History to record once the session proves it connected.
    pub record: Option<(String, ConnKind, Instant)>,
    /// The saved forward (by title) this tab runs.
    pub forward: Option<String>,
    /// The host a terminal tab is connected to (for its snippets).
    pub host: Option<String>,
    /// A second terminal pane in this tab.
    pub split: Option<Split>,
}

/// What a one-line `Prompt` does with its text in the active SFTP pane.
pub enum PromptAction {
    /// Create a directory of that name.
    Mkdir,
    /// Rename entry `from` to the text.
    Rename { from: String },
    /// Go to the path typed.
    Goto,
}

/// A one-line text prompt (SFTP mkdir, rename, go to).
pub struct Prompt {
    pub title: String,
    pub input: TextInput,
    pub action: PromptAction,
}

/// What a yes/no `Modal::Confirm` does when confirmed (`Quit`: quit with
/// sessions still open).
pub enum ConfirmAction {
    /// Delete the named host from its config.
    DeleteHost(String),
    /// Delete saved forward `i` from the config, then reopen the forwards list.
    DeleteForward(usize),
    /// Delete the selected entry of the active SFTP pane.
    DeleteFile {
        name: String,
        is_dir: bool,
    },
    Quit,
    /// A multi-line paste into the active terminal, then Enter if `enter`
    /// (a multi-line `run` snippet).
    Paste {
        text: String,
        enter: bool,
    },
}

/// The dialog over the screen, if any (one at a time); it takes all keys.
/// `Confirm` is a yes/no question that runs its `ConfirmAction` on yes.
pub enum Modal {
    /// Keys; scrolled down `scroll` lines.
    Help {
        scroll: u16,
    },
    /// Theme list, previewed live; `original` is restored on cancel.
    ThemePicker {
        selected: usize,
        original: String,
    },
    /// The host editor (new or existing host).
    Editor(Box<Editor>),
    /// The add/edit port-forward form.
    Forward(ForwardForm),
    /// The list of saved forwards with their state.
    Forwards {
        selected: usize,
    },
    /// A one-line text prompt.
    Prompt(Prompt),
    Confirm {
        text: String,
        action: ConfirmAction,
    },
    /// Find in the active terminal's output.
    Search {
        query: TextInput,
        search: TermSearch,
    },
    /// Saved commands for the active terminal's host.
    Snippets {
        selected: usize,
    },
    /// ssh asks (password, one-time code, host key). The answer goes
    /// straight back to ssh and is never kept.
    Ask {
        host: String,
        input: TextInput,
        req: AskRequest,
    },
    /// A file a transfer would replace already exists.
    Overwrite {
        tab: u64,
        id: u64,
        host: String,
        path: String,
        reply: oneshot::Sender<OverwriteDecision>,
    },
}

/// Text selected with the mouse (`tui_mouse = "always"`): (line, col)
/// ends, line numbers as `termx::ScrollTracker` counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Id of the tab whose terminal the selection is in.
    pub tab: u64,
    /// Where the press started.
    pub anchor: (u64, u16),
    /// Where the pointer is (or was released).
    pub head: (u64, u16),
    /// The pointer moved after the press (a plain click selects nothing).
    pub dragged: bool,
}

impl Selection {
    /// The ends in order.
    pub fn span(&self) -> ((u64, u16), (u64, u16)) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }
}

/// The whole TUI: config, host list, tabs, dialogs and input state.
pub struct App {
    /// The loaded `config.toml`.
    pub cfg: Config,
    /// What `cfg` looked like when last loaded or saved; saves write only the difference.
    cfg_saved: Config,
    pub cfg_path: PathBuf,
    /// The active theme, resolved for ratatui.
    pub theme: Theme,
    /// Plain mode (`--plain` or `settings.tui_plain`).
    pub plain: bool,
    /// Every known host (ssh config and `config.toml`).
    pub hosts: Vec<HostEntry>,
    /// Indexes into `hosts` in display order, after hiding, sorting and filtering.
    pub filtered: Vec<usize>,
    /// Selected row of `filtered`.
    pub host_selected: usize,
    /// First visible row of the host list (scroll position, kept by `ui`).
    pub host_offset: usize,
    /// The host list's filter text.
    pub filter: TextInput,
    /// The filter field has the keys.
    pub filtering: bool,
    /// Open tabs; tab 0 is the host list.
    pub tabs: Vec<Tab>,
    /// Index of the shown tab.
    pub active: usize,
    pub modal: Option<Modal>,
    /// Status line message: text, is-error, and when it was set (it clears after a few seconds).
    pub status: Option<(String, bool, Instant)>,
    /// Set when the main loop should exit.
    pub should_quit: bool,
    /// Last known size of the terminal content area, used for new sessions.
    pub term_area: (u16, u16),
    /// Screen position of the terminal content's top-left cell (mouse reports).
    pub term_origin: (u16, u16),
    /// Where the unfocused pane of a split terminal is drawn (a click there focuses it).
    pub other_pane: Option<Rect>,
    /// The whole area of a split terminal (its divider drags with the mouse).
    pub split_area: Option<Rect>,
    /// The id of the tab whose split divider is being dragged; a switch to
    /// another tab, a focus change or the next press off the divider ends it.
    divider_drag: Option<u64>,
    /// Screen columns of each tab's label in the tab bar (row 0).
    pub tab_cols: Vec<(u16, u16)>,
    /// The mouse button a program was told is down.
    pub mouse_held: Option<u8>,
    /// The session told about `mouse_held` (by its parser) and the last
    /// cell reported, so the release goes to it even after focus moved.
    mouse_target: Option<(SharedParser, (u16, u16))>,
    pub selection: Option<Selection>,
    /// Text to put on the user's clipboard (written as OSC 52).
    pub clipboard_out: Option<String>,
    /// Treat a burst of typed keys with a line break as a paste (Windows
    /// consoles have no bracketed paste, so pastes arrive as key presses).
    pub paste_bursts: bool,
    /// Reachability per host name.
    pub statuses: HashMap<String, HostStatus>,
    pub history: History,
    history_path: PathBuf,
    /// Sort order and hidden-host toggle (state.toml).
    pub ui_state: UiState,
    /// Global shortcuts (`[tui_keys]` over the defaults).
    pub keys: Keymap,
    /// Problems in `[tui_keys]` (shown in the help).
    pub key_errors: Vec<String>,
    /// Answers ssh's questions for SFTP connections (see `enable_askpass`).
    askpass: Option<Arc<Broker>>,
    /// The ssh client's version once checked (`None` inside: unknown).
    ssh_version: Option<Option<(u32, u32)>>,
    /// Questions waiting for the screen (one modal at a time). Each shows
    /// only while its own SFTP tab is active; until then the tab is flagged.
    pub questions: VecDeque<Question>,
    /// When the question on screen opened: keys typed within
    /// `question_delay` of that were meant for something else and are ignored.
    pub question_shown: Option<Instant>,
    pub question_delay: Duration,
    /// ssh notices that need no answer ("touch your security key"), with
    /// the host they're for; shown on the status line until withdrawn.
    pub notices: Vec<(String, AskRequest)>,
    /// The id the next tab gets.
    next_id: u64,
    /// The tokio runtime background work runs on.
    rt: Handle,
    /// Cloned into background tasks to send `Msg`s back.
    tx: UnboundedSender<Msg>,
    /// Where `drain_messages` reads those `Msg`s.
    rx: UnboundedReceiver<Msg>,
}

impl App {
    /// Builds the app from a loaded config: applies the theme and keys,
    /// collects hosts and, with `status_checks = "on_start"`, starts probing them.
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
            tabs: vec![Tab { id: 0, title: "Hosts".into(), kind: TabKind::Hosts, record: None, forward: None, host: None, split: None }],
            active: 0,
            modal: None,
            status: None,
            should_quit: false,
            term_area: (24, 80),
            term_origin: (1, 1),
            other_pane: None,
            split_area: None,
            divider_drag: None,
            tab_cols: vec![],
            mouse_held: None,
            mouse_target: None,
            selection: None,
            clipboard_out: None,
            paste_bursts: cfg!(windows),
            keys: Keymap::default(),
            key_errors: vec![],
            askpass: None,
            ssh_version: None,
            questions: VecDeque::new(),
            question_shown: None,
            question_delay: QUESTION_DELAY,
            notices: vec![],
            next_id: 1,
            rt,
            tx,
            rx,
        };
        app.apply_theme_name(&app.cfg.settings.theme.clone());
        app.load_keys();
        app.reload_hosts();
        if app.cfg.settings.status_checks == StatusChecks::OnStart {
            app.check_status();
        }
        app
    }

    /// Starts the askpass broker so SFTP connections can ask for passwords,
    /// one-time codes and host-key confirmation. `helper` is the program ssh
    /// runs as `SSH_ASKPASS`: the `skiff` executable itself.
    pub fn enable_askpass(&mut self, helper: PathBuf) {
        match Broker::start(&self.rt, helper) {
            Ok((broker, mut rx)) => {
                // Relay the broker's requests into the UI's message queue.
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    while let Some(req) = rx.recv().await {
                        if tx.send(Msg::Ask(req)).is_err() {
                            break;
                        }
                    }
                });
                self.askpass = Some(Arc::new(broker));
            }
            Err(e) => self.set_error(format!("password prompts unavailable: {e:#}")),
        }
    }

    /// The broker, if SFTP connections can prompt (it runs and ssh is new
    /// enough for `SSH_ASKPASS_REQUIRE`).
    fn prompter(&mut self) -> Option<Arc<Broker>> {
        let broker = self.askpass.clone()?;
        let bin = self.cfg.settings.ssh_binary.clone();
        // Asked once; an unknown version is given the benefit of the doubt.
        let version = *self.ssh_version.get_or_insert_with(|| skiff_core::sshver::client_version(&bin).ok());
        match version {
            Some(v) if !skiff_core::sshver::supports_askpass_require(v) => {
                self.set_error(format!("Skiff needs OpenSSH 8.4 or newer for password prompts (found {}.{})", v.0, v.1));
                None
            }
            _ => Some(broker),
        }
    }

    /// Builds the shortcuts from `[tui_keys]`; problems show as an error.
    fn load_keys(&mut self) {
        let (keys, errors) = Keymap::new(&self.cfg.tui_keys);
        self.keys = keys;
        if let Some(first) = errors.first() {
            let more = if errors.len() > 1 { format!(" (+{} more, see help)", errors.len() - 1) } else { String::new() };
            self.set_error(format!("{first}{more}"));
        }
        self.key_errors = errors;
    }

    // ----- themes -----

    /// The built-in themes with config themes on top: a config theme with a
    /// built-in's name replaces it, others are appended.
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

    /// Switches to theme `name`; an unknown or invalid theme leaves the
    /// current one and shows an error.
    fn apply_theme_name(&mut self, name: &str) {
        let themes = self.all_themes();
        // The host terminal owns its colors, so "auto" is simply "terminal".
        let name = skiff_core::theme::effective_name(name, false);
        match themes.iter().find(|t| t.name == name) {
            Some(def) => match Theme::from_def(def) {
                Ok(t) => self.theme = t,
                Err(e) => self.set_error(e),
            },
            None => self.set_error(format!("unknown theme '{name}'")),
        }
    }

    // ----- hosts -----

    /// Re-reads the host list from the config and ssh config, then refilters.
    pub fn reload_hosts(&mut self) {
        let (hosts, warning) = hosts::collect(&self.cfg);
        self.hosts = hosts;
        if let Some(w) = warning {
            self.set_error(w);
        }
        self.refilter();
    }

    /// Rebuilds `filtered`: without a filter, the visible hosts in the chosen
    /// sort order; with one, the fuzzy matches best first (ties in list order).
    fn refilter(&mut self) {
        let q = self.filter.value.trim().to_string();
        let mut shown = hosts::visible(&self.hosts, self.ui_state.show_hidden);
        if q.is_empty() {
            history::order(&self.hosts, &mut shown, self.ui_state.sort, &self.history);
            self.filtered = shown;
        } else {
            // Score descending, then original index for a stable order.
            let mut scored: Vec<(i64, usize)> =
                shown.into_iter().filter_map(|i| hosts::fuzzy_score(&q, &self.hosts[i]).map(|s| (s, i))).collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.host_selected = self.host_selected.min(self.filtered.len().saturating_sub(1));
    }

    /// The host under the cursor in the host list, if the list isn't empty.
    pub fn selected_host(&self) -> Option<&HostEntry> {
        self.filtered.get(self.host_selected).map(|&i| &self.hosts[i])
    }

    /// Moves the host cursor by `delta` rows, stopping at either end.
    fn move_host(&mut self, delta: i64) {
        if self.filtered.is_empty() {
            return;
        }
        let max = self.filtered.len() as i64 - 1;
        self.host_selected = (self.host_selected as i64 + delta).clamp(0, max) as usize;
    }

    /// Writes the changes to `cfg` since the last load or save to `config.toml`.
    fn save_config(&mut self) {
        match self.cfg.save(&self.cfg_saved, &self.cfg_path) {
            Ok(()) => self.cfg_saved = self.cfg.clone(),
            Err(e) => self.set_error(format!("saving config: {e:#}")),
        }
    }

    // ----- status -----

    /// Shows an informational message on the status line.
    pub fn set_info(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), false, Instant::now()));
    }

    /// Shows an error on the status line.
    pub fn set_error(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), true, Instant::now()));
    }

    /// Shows the note from the one-time settings copy (if any) on the
    /// status line: an error when the copy failed.
    pub fn show_migration(&mut self, migration: &skiff_core::migrate::Migration) {
        match migration {
            skiff_core::migrate::Migration::None => {}
            skiff_core::migrate::Migration::Copied(note) => self.set_info(note.clone()),
            skiff_core::migrate::Migration::Failed(note) => self.set_error(note.clone()),
        }
    }

    // ----- tabs -----

    /// `base`, or `base (2)`, `base (3)`… if an open tab already has that title.
    fn unique_title(&self, base: String) -> String {
        let mut title = base.clone();
        let mut n = 2;
        while self.tabs.iter().any(|t| t.title == title) {
            title = format!("{base} ({n})");
            n += 1;
        }
        title
    }

    /// Adds a tab at the end with a fresh id and a unique title, and shows it.
    fn push_tab(&mut self, title: String, kind: TabKind) {
        let id = self.next_id;
        self.next_id += 1;
        let title = self.unique_title(title);
        self.tabs.push(Tab { id, title, kind, record: None, forward: None, host: None, split: None });
        self.active = self.tabs.len() - 1;
    }

    /// Removes tab `i`; the active tab stays the same one when it remains.
    fn remove_tab(&mut self, i: usize) {
        self.tabs.remove(i);
        if self.active > i {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len() - 1);
    }

    /// Opens host `name` (hidden hosts too): a terminal, or a file transfer.
    pub fn open_named(&mut self, name: &str, files: bool) {
        let Some(host) = self.hosts.iter().find(|h| h.name == name).cloned() else {
            self.set_error(format!("unknown host: {name}"));
            return;
        };
        if files { self.open_sftp_with(host) } else { self.open_terminal_with(host) }
    }

    /// Opens a terminal tab to the selected host.
    fn open_terminal(&mut self) {
        let Some(host) = self.selected_host().cloned() else {
            return;
        };
        self.open_terminal_with(host);
    }

    /// Starts ssh to `host` in a new terminal tab sized to the content area;
    /// history is recorded later, once the session proves it connected.
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
        status::check_all(
            &self.rt,
            items,
            Arc::new(move |host, status| {
                let _ = tx.send(Msg::Status { host, status });
            }),
        );
    }

    /// Records a successful connection to `name` in the history file.
    fn record(&mut self, name: &str, kind: ConnKind) {
        if let Err(e) = self.history.update(&self.history_path, |h| h.record(name, kind, history::now())) {
            self.set_error(format!("saving history: {e:#}"));
        }
    }

    /// After host `old` was renamed to `new`: its saved forwards (and the
    /// tabs running them), history and status follow it.
    fn after_rename(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        // An unnamed forward's title shows its host, so retitle the tabs
        // running the forwards that changed.
        let titles: Vec<String> = self.cfg.forwards.iter().map(Forward::title).collect();
        if hostform::rename_forwards(&mut self.cfg, old, new) {
            self.save_config();
            for (before, f) in titles.iter().zip(&self.cfg.forwards) {
                let after = f.title();
                for t in self.tabs.iter_mut().filter(|t| t.forward.as_ref() == Some(before)) {
                    t.forward = Some(after.clone());
                    t.title = after.clone();
                }
            }
        }
        for t in self.tabs.iter_mut() {
            if t.host.as_deref() == Some(old) {
                t.host = Some(new.to_string());
            }
            if let Some((name, _, _)) = &mut t.record
                && name == old
            {
                *name = new.to_string();
            }
        }
        if let Err(e) = self.history.update(&self.history_path, |h| h.rename(old, new)) {
            self.set_error(format!("saving history: {e:#}"));
        }
        if let Some(s) = self.statuses.remove(old) {
            self.statuses.insert(new.to_string(), s);
        }
    }

    /// Writes the sort order and hidden-host toggle to `state.toml`.
    fn save_ui_state(&mut self) {
        if let Err(e) = self.ui_state.save(&UiState::path_for(&self.cfg_path)) {
            self.set_error(format!("saving state: {e:#}"));
        }
    }

    /// Switches the host list to the next sort order and saves it.
    fn cycle_sort(&mut self) {
        self.ui_state.sort = self.ui_state.sort.next();
        self.save_ui_state();
        self.refilter();
        self.set_info(format!("Sort: {}", self.ui_state.sort.label()));
    }

    /// Shows or hides hosts marked hidden, and saves the choice.
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
            if let TabKind::Terminal(s) = &mut self.tabs[i].kind
                && s.is_alive()
            {
                self.active = i;
                return;
            }
            self.remove_tab(i);
        }
        let (rows, cols) = self.term_area;
        match TerminalSession::spawn(&self.cfg.settings.ssh_binary, &f.ssh_args(&host), rows, cols, self.cfg.settings.scrollback) {
            Ok(s) => {
                self.push_tab(title.clone(), TabKind::Terminal(Box::new(s)));
                if let Some(t) = self.tabs.last_mut() {
                    t.forward = Some(title);
                    // Recorded once the forward is up, not when ssh starts.
                    t.record = Some((host.name.clone(), ConnKind::Forward, Instant::now()));
                }
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// The tab (index) running the saved forward titled `title`.
    fn forward_tab(&self, title: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.forward.as_deref() == Some(title))
    }

    /// "running", "stopped" or "failed: <ssh's reason>" for saved forward `i`.
    pub fn forward_state(&mut self, i: usize) -> String {
        let Some(title) = self.cfg.forwards.get(i).map(|f| f.title()) else {
            return String::new();
        };
        let Some(ti) = self.forward_tab(&title) else {
            return "stopped".into();
        };
        let TabKind::Terminal(s) = &mut self.tabs[ti].kind else {
            return "stopped".into();
        };
        if s.is_alive() {
            return "running".into();
        }
        // ssh exited: find its error message on the tab's screen.
        let text = s.parser.lock().map(|p| p.screen().contents()).unwrap_or_default();
        let lines: Vec<String> = text.lines().map(String::from).collect();
        match skiff_core::forward::error_line(&lines) {
            Some(l) => format!("failed: {l}"),
            None => "failed".into(),
        }
    }

    /// Stops saved forward `i` (closes its tab).
    fn stop_forward(&mut self, i: usize) {
        let Some(title) = self.cfg.forwards.get(i).map(|f| f.title()) else {
            return;
        };
        if let Some(ti) = self.forward_tab(&title) {
            self.remove_tab(ti);
        }
    }

    /// Saves the forward form (adding or replacing a forward) and starts it.
    /// An invalid form comes back with its error, to stay open.
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

    /// Opens an SFTP tab to the selected host.
    fn open_sftp(&mut self) {
        let Some(host) = self.selected_host().cloned() else {
            return;
        };
        self.open_sftp_with(host);
    }

    /// Opens an SFTP tab to `host` and starts connecting in the background.
    /// The local pane starts at `local_start_dir`, else home, else `.`.
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
        // With prompts available ssh may ask; otherwise `sftp_batch_mode`
        // decides whether it fails at once instead of trying a terminal.
        let prompter = self.prompter();
        let batch = prompter.is_none() && self.cfg.settings.sftp_batch_mode;
        tab.set_askpass(prompter);
        tab.connect(host.sftp_launch(&self.cfg.settings.ssh_binary, batch));
        self.push_tab(format!("{} [sftp]", host.name), TabKind::Sftp(Box::new(tab)));
    }

    /// Closes the active tab, or only its focused pane when it is split.
    fn close_active_tab(&mut self) {
        if self.active == 0 {
            return;
        }
        if self.tabs[self.active].split.is_some() {
            self.close_pane();
        } else {
            self.remove_tab(self.active);
        }
    }

    // ----- split panes -----

    /// Splits the active terminal tab: a second pane runs the same ssh
    /// command (a new connection to the same host) and takes the focus. An
    /// already split tab changes direction.
    pub fn split(&mut self, dir: SplitDir) {
        // The new pane starts at half the content area along the split.
        let (rows, cols) = self.term_area;
        let (rows, cols) = match dir {
            SplitDir::Right => (rows, cols / 2),
            SplitDir::Down => (rows / 2, cols),
        };
        let scrollback = self.cfg.settings.scrollback;
        let tab = &mut self.tabs[self.active];
        let TabKind::Terminal(s) = &tab.kind else { return };
        if tab.forward.is_some() {
            self.set_error("A port forward has no shell to split");
            return;
        }
        if let Some(sp) = &mut tab.split {
            sp.dir = dir;
            return;
        }
        let (program, args) = s.command();
        match TerminalSession::spawn(program, args, rows, cols, scrollback) {
            Ok(other) => {
                tab.split = Some(Split { other: Box::new(other), dir, focus_first: true, ratio: 0.5 });
                self.focus_other_pane();
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// Moves the focus to the other pane of a split tab.
    pub fn focus_other_pane(&mut self) {
        let tab = &mut self.tabs[self.active];
        if let (TabKind::Terminal(s), Some(sp)) = (&mut tab.kind, &mut tab.split) {
            std::mem::swap(s, &mut sp.other);
            sp.focus_first = !sp.focus_first;
            self.divider_drag = None;
            // This belongs to the pane that had the focus (a held button
            // is released to it by `check_mouse_held`).
            self.selection = None;
        }
    }

    /// Swaps the panes of a split tab (the focus stays with its session).
    fn swap_panes(&mut self) {
        if let Some(sp) = &mut self.tabs[self.active].split {
            sp.focus_first = !sp.focus_first;
            self.selection = None;
        }
    }

    /// Moves the divider of a split tab one step (to the next snap point)
    /// towards `right_or_down` along `dir`; false when the split runs the
    /// other way (the key goes to the shell).
    fn resize_split(&mut self, dir: SplitDir, right_or_down: bool) -> bool {
        match &mut self.tabs[self.active].split {
            Some(sp) if sp.dir == dir => {
                sp.ratio = panes::step_ratio(sp.ratio, right_or_down);
                true
            }
            _ => false,
        }
    }

    /// Closes the focused pane of a split tab (its session ends).
    fn close_pane(&mut self) {
        let tab = &mut self.tabs[self.active];
        if let Some(sp) = tab.split.take() {
            tab.kind = TabKind::Terminal(sp.other);
            self.selection = None;
        }
    }

    /// Quits at once with only the host list open; otherwise asks first.
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

    /// Opens the theme picker on the current theme.
    fn open_theme_picker(&mut self) {
        let themes = self.all_themes();
        let selected = themes.iter().position(|t| t.name == self.theme.name).unwrap_or(0);
        self.modal = Some(Modal::ThemePicker { selected, original: self.theme.name.clone() });
    }

    // ----- background messages -----

    /// Called every loop turn: applies all pending background messages,
    /// queues and shows questions, records connections that proved up, and
    /// expires the status message.
    pub fn drain_messages(&mut self) {
        let mut connected: Vec<String> = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            // Messages not addressed to an SFTP tab are handled here and skipped.
            let msg = match msg {
                Msg::Status { host, status } => {
                    self.statuses.insert(host, status);
                    continue;
                }
                Msg::Ask(req) if req.kind == AskKind::Notice => {
                    // A notice is shown, not answered; one nobody owns is dismissed.
                    match self.ask_owner(&req) {
                        Some(i) => {
                            let TabKind::Sftp(s) = &self.tabs[i].kind else { unreachable!("an sftp tab") };
                            self.notices.push((s.host_name.clone(), req));
                        }
                        None => {
                            let _ = req.reply.send(None);
                        }
                    }
                    continue;
                }
                Msg::Ask(req) => {
                    self.questions.push_back(Question::Ask(req));
                    continue;
                }
                Msg::TransferAsk { tab, id, path, reply } => {
                    self.questions.push_back(Question::Overwrite { tab, id, path, reply });
                    continue;
                }
                m => m,
            };
            let tab_id = match &msg {
                Msg::SftpConnected { tab, .. }
                | Msg::RemoteListing { tab, .. }
                | Msg::TransferProgress { tab, .. }
                | Msg::TransferDone { tab, .. }
                | Msg::RemoteOpDone { tab, .. } => *tab,
                Msg::Status { .. } | Msg::Ask(_) | Msg::TransferAsk { .. } => unreachable!("handled above"),
            };
            // Messages for tabs that have since been closed are dropped.
            let Some(TabKind::Sftp(s)) = self.tabs.iter_mut().find(|t| t.id == tab_id).map(|t| &mut t.kind) else {
                continue;
            };
            if matches!(&msg, Msg::SftpConnected { result: Ok(_), .. }) {
                connected.push(s.host_name.clone());
            }
            match msg {
                Msg::Status { .. } | Msg::Ask(_) | Msg::TransferAsk { .. } => {}
                Msg::SftpConnected { result, .. } => s.on_connected(result),
                Msg::RemoteListing { seq, result, .. } => s.on_remote_listing(seq, result),
                Msg::TransferProgress { id, done, .. } => s.on_transfer_progress(id, done),
                Msg::TransferDone { id, result, .. } => s.on_transfer_done(id, result),
                Msg::RemoteOpDone { result, keep, .. } => s.on_remote_op(result, keep),
            }
        }
        for name in connected {
            self.record(&name, ConnKind::Files);
        }
        self.next_question();
        // An open search keeps up with new output.
        if let (Some(Modal::Search { search, .. }), TabKind::Terminal(s)) = (&mut self.modal, &self.tabs[self.active].kind) {
            search.sync(s);
        }
        // A terminal counts as connected once it has shown output and is
        // still running two seconds in. A forward (`ssh -N`, often silent)
        // counts once it has kept running as long as the GUI waits for a
        // quiet forward, and isn't sitting at a prompt.
        let mut done = Vec::new();
        for tab in &mut self.tabs {
            if let (Some((_, kind, at)), TabKind::Terminal(s)) = (&tab.record, &mut tab.kind) {
                let up = if *kind == ConnKind::Forward {
                    let at_prompt = s.parser.lock().map(|p| p.screen().cursor_position().1 > 0).unwrap_or(true);
                    at.elapsed() >= skiff_core::forward::RUNNING_FALLBACK && !at_prompt
                } else {
                    at.elapsed() >= Duration::from_secs(2) && s.generation() > 0
                };
                if up && s.is_alive() {
                    let (name, kind, _) = tab.record.take().expect("checked");
                    done.push((name, kind));
                }
            }
        }
        for (name, kind) in done {
            self.record(&name, kind);
        }
        // Status messages clear after six seconds.
        if let Some((_, _, at)) = &self.status
            && at.elapsed() > Duration::from_secs(6)
        {
            self.status = None;
        }
    }

    /// The tab (index) whose connection attempt asks `req`, if it still may.
    fn ask_owner(&self, req: &AskRequest) -> Option<usize> {
        if req.cancelled.is_cancelled() || req.reply.is_closed() {
            return None;
        }
        // Only the current attempt of a tab still connecting may ask.
        self.tabs
            .iter()
            .position(|t| matches!(&t.kind, TabKind::Sftp(s) if s.ask_tag() == req.tag && matches!(s.status, Status::Connecting)))
    }

    /// The tab (index) question `q` belongs to; `None` once it no longer applies.
    fn question_owner(&self, q: &Question) -> Option<usize> {
        match q {
            Question::Ask(req) => self.ask_owner(req),
            Question::Overwrite { tab, reply, .. } => {
                if reply.is_closed() {
                    return None;
                }
                self.tabs.iter().position(|t| t.id == *tab && matches!(t.kind, TabKind::Sftp(_)))
            }
        }
    }

    /// Tabs (indexes) with a question waiting for them to be shown.
    pub fn waiting_tabs(&self) -> Vec<usize> {
        self.questions.iter().filter_map(|q| self.question_owner(q)).collect()
    }

    /// The question on screen opened too recently to take keys.
    fn question_too_new(&self) -> bool {
        matches!(self.modal, Some(Modal::Ask { .. } | Modal::Overwrite { .. }))
            && self.question_shown.is_some_and(|at| at.elapsed() < self.question_delay)
    }

    /// Puts away questions that no longer apply (ssh withdrew them, their
    /// transfer or tab ended) and shows the next one for the active tab
    /// when nothing else is open. A question never opens over another tab:
    /// keys typed there (a shell command, say) must not answer it.
    fn next_question(&mut self) {
        self.notices.retain(|(_, r)| !r.cancelled.is_cancelled() && !r.reply.is_closed());
        let stale = match &self.modal {
            Some(Modal::Ask { req, .. }) => req.cancelled.is_cancelled() || req.reply.is_closed(),
            Some(Modal::Overwrite { reply, .. }) => reply.is_closed(),
            _ => false,
        };
        if stale {
            self.modal = None;
        }
        let mut waiting = VecDeque::new();
        for q in std::mem::take(&mut self.questions) {
            match self.question_owner(&q) {
                Some(i) if self.modal.is_none() && i == self.active => {
                    let TabKind::Sftp(s) = &self.tabs[i].kind else { unreachable!("an sftp tab") };
                    let host = s.host_name.clone();
                    self.modal = Some(match q {
                        Question::Ask(req) => Modal::Ask { host, input: TextInput::default(), req },
                        Question::Overwrite { tab, id, path, reply } => Modal::Overwrite { tab, id, host, path, reply },
                    });
                    self.question_shown = Some(Instant::now());
                }
                Some(_) => waiting.push_back(q),
                // A closed tab's transfer was cancelled; dropping `reply` is enough.
                None => {
                    if let Question::Ask(req) = q {
                        let _ = req.reply.send(None);
                    }
                }
            }
        }
        self.questions = waiting;
    }

    /// The SFTP tab with id `id`, if it is still open.
    fn sftp_tab_mut(&mut self, id: u64) -> Option<&mut SftpTab> {
        self.tabs.iter_mut().find_map(|t| match &mut t.kind {
            TabKind::Sftp(s) if t.id == id => Some(s.as_mut()),
            _ => None,
        })
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

    /// Routes one terminal event (key press or repeat, paste, mouse), then
    /// makes sure a held mouse button still reaches its program.
    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.handle_key(key),
            Event::Paste(text) => self.handle_paste(&text),
            Event::Mouse(m) => self.handle_mouse(m),
            _ => {}
        }
        self.check_mouse_held();
    }

    /// A program told a button is down must hear it come up: if a dialog
    /// opened or another tab or pane took the focus mid-drag, the release
    /// is sent now, to the session that saw the press.
    fn check_mouse_held(&mut self) {
        let Some((parser, cell)) = &self.mouse_target else {
            self.mouse_held = None;
            return;
        };
        let focused = match &self.tabs[self.active].kind {
            TabKind::Terminal(s) => Arc::ptr_eq(&s.parser, parser),
            _ => false,
        };
        // Nothing held, or the session that saw the press still has the keys.
        if self.mouse_held.is_none() || (focused && self.modal.is_none()) {
            if self.mouse_held.is_none() {
                self.mouse_target = None;
            }
            return;
        }
        let (button, cell) = (self.mouse_held.take().unwrap_or(0), *cell);
        let parser = parser.clone();
        self.mouse_target = None;
        // Find that session in any tab, focused pane or not.
        let session = self.tabs.iter_mut().find_map(|t| {
            let other = t.split.as_mut().map(|sp| &mut sp.other);
            match &mut t.kind {
                TabKind::Terminal(s) if Arc::ptr_eq(&s.parser, &parser) => Some(s),
                _ => other.filter(|o| Arc::ptr_eq(&o.parser, &parser)),
            }
        });
        if let Some(s) = session {
            let mode = s.parser.lock().map(|p| (p.screen().mouse_protocol_mode(), p.screen().mouse_protocol_encoding())).ok();
            if let Some((mode, enc)) = mode
                && let Some(bytes) = termx::encode_mouse(mode, enc, MouseEv::Release(button), cell.0, cell.1, false, false, false)
            {
                s.write(&bytes);
            }
        }
    }

    /// The active terminal has a program that asked for the mouse and the
    /// view is live (scrolled back, the program can't see the text under
    /// the pointer), or the program saw a button go down.
    pub fn wants_mouse(&self) -> bool {
        self.modal.is_none()
            && matches!(&self.tabs[self.active].kind, TabKind::Terminal(s)
                if s.parser.lock().is_ok_and(|p| p.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
                    && (p.screen().scrollback() == 0 || self.mouse_held.is_some())))
    }

    /// Whether to capture the mouse from the user's terminal now.
    pub fn capture_mouse(&self) -> bool {
        self.cfg.settings.tui_mouse == TuiMouse::Always || self.wants_mouse()
    }

    /// With `tui_mouse = "always"`, the border between the panes of a split
    /// drags (snapping to 25 / 33 / 50 / 67 / 75% unless Alt is held).
    /// True when `m` was part of a divider drag and needs nothing else.
    fn drag_divider(&mut self, m: &MouseEvent) -> bool {
        let always = self.cfg.settings.tui_mouse == TuiMouse::Always;
        let tab = &mut self.tabs[self.active];
        let (Some(area), Some(sp)) = (self.split_area, tab.split.as_mut()) else {
            self.divider_drag = None;
            return false;
        };
        // A drag started in another tab (or before a focus change) is over.
        let dragging = self.divider_drag == Some(tab.id);
        match m.kind {
            // Every press decides afresh, so a release that never arrived can't
            // leave a drag stuck on.
            MouseEventKind::Down(MouseButton::Left) => {
                let on = always && crate::ui::on_divider(area, sp.dir, sp.ratio, m.column, m.row);
                self.divider_drag = on.then_some(tab.id);
                return on;
            }
            MouseEventKind::Drag(MouseButton::Left) if dragging => {
                let free = m.modifiers.contains(KeyModifiers::ALT);
                sp.ratio = crate::ui::divider_ratio(area, sp.dir, m.column, m.row, !free);
            }
            MouseEventKind::Up(MouseButton::Left) if dragging => self.divider_drag = None,
            _ => return false,
        }
        true
    }

    /// A click on the tab bar switches tabs. In the terminal, a program
    /// that asked for the mouse gets it (see `wants_mouse`); a button it
    /// saw go down is released even outside the terminal, at its edge.
    /// Otherwise, with `tui_mouse = "always"` (or Shift held while a
    /// program has the mouse), the wheel scrolls the scrollback and a drag
    /// selects text and copies it. Dialogs ignore the mouse.
    fn handle_mouse(&mut self, m: MouseEvent) {
        if self.modal.is_some() {
            return;
        }
        // Tab bar (row 0).
        if m.kind == MouseEventKind::Down(MouseButton::Left)
            && m.row == 0
            && let Some(i) = self.tab_cols.iter().position(|&(a, b)| (a..b).contains(&m.column))
        {
            self.active = i;
            return;
        }
        if self.drag_divider(&m) {
            return;
        }
        // A click in the other pane of a split focuses it.
        if matches!(m.kind, MouseEventKind::Down(_))
            && self.mouse_held.is_none()
            && self.tabs[self.active].split.is_some()
            && self.other_pane.is_some_and(|r| r.contains(ratatui::layout::Position::new(m.column, m.row)))
        {
            self.focus_other_pane();
            return;
        }
        let shift = m.modifiers.contains(KeyModifiers::SHIFT);
        let always = self.cfg.settings.tui_mouse == TuiMouse::Always;
        let tab_id = self.tabs[self.active].id;
        let (ox, oy) = self.term_origin;
        let (rows, cols) = self.term_area;
        let inside = (ox..ox + cols).contains(&m.column) && (oy..oy + rows).contains(&m.row);
        // The cell under the pointer, clamped to the terminal.
        let cell =
            (m.column.clamp(ox, (ox + cols).saturating_sub(1).max(ox)) - ox, m.row.clamp(oy, (oy + rows).saturating_sub(1).max(oy)) - oy);
        // Shift takes the mouse back from the program, unless it holds a button.
        let program = self.wants_mouse() && (self.mouse_held.is_some() || !shift);
        let TabKind::Terminal(s) = &mut self.tabs[self.active].kind else {
            return;
        };
        let Ok((mode, enc, offset, alt)) = s.parser.lock().map(|p| {
            let sc = p.screen();
            (sc.mouse_protocol_mode(), sc.mouse_protocol_encoding(), sc.scrollback(), sc.alternate_screen())
        }) else {
            return;
        };
        let button = |b: MouseButton| match b {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        // Report to the program; once it saw a press, drags and the release
        // reach it even outside the terminal (at the clamped edge cell).
        if program {
            let held = self.mouse_held.is_some();
            let ev = match m.kind {
                MouseEventKind::Down(b) if inside => {
                    self.mouse_held = Some(button(b));
                    MouseEv::Press(button(b))
                }
                MouseEventKind::Up(b) if inside || held => {
                    self.mouse_held = None;
                    self.mouse_target = None;
                    MouseEv::Release(button(b))
                }
                MouseEventKind::Drag(b) if inside || held => MouseEv::Drag(button(b)),
                MouseEventKind::Moved if inside => MouseEv::Move,
                MouseEventKind::ScrollUp if inside => MouseEv::WheelUp,
                MouseEventKind::ScrollDown if inside => MouseEv::WheelDown,
                _ => return,
            };
            if self.mouse_held.is_some() {
                self.mouse_target = Some((s.parser.clone(), cell));
            }
            let (alt_key, ctrl) = (m.modifiers.contains(KeyModifiers::ALT), m.modifiers.contains(KeyModifiers::CONTROL));
            if let Some(bytes) = termx::encode_mouse(mode, enc, ev, cell.0, cell.1, false, alt_key, ctrl) {
                s.write(&bytes);
            }
            return;
        }
        if !always {
            return; // the user's terminal does its own thing
        }
        // The pointer as an absolute (line, col), counting the scrollback view.
        let top = s.scrolled().saturating_sub(offset as u64);
        let at = (top + cell.1 as u64, cell.0);
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if inside => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if alt {
                    // Full-screen programs scroll with the arrow keys.
                    s.write(if up { b"\x1b[A\x1b[A\x1b[A" } else { b"\x1b[B\x1b[B\x1b[B" });
                } else {
                    s.scroll(if up { WHEEL_LINES } else { -WHEEL_LINES });
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside => {
                self.selection = Some(Selection { tab: tab_id, anchor: at, head: at, dragged: false });
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(sel) = self.selection.as_mut().filter(|s| s.tab == tab_id) {
                    sel.head = at;
                    sel.dragged = true;
                }
            }
            // Releasing after a drag copies the selected text (via OSC 52).
            MouseEventKind::Up(MouseButton::Left) => match self.selection {
                Some(sel) if sel.tab == tab_id && sel.dragged => {
                    let (a, b) = sel.span();
                    let text = s.parser.lock().map(|mut p| termx::text_between(&mut p, s.scrolled(), a, b)).unwrap_or_default();
                    if !text.is_empty() {
                        self.set_info(format!("Copied {} characters", text.chars().count()));
                        self.clipboard_out = Some(text);
                    }
                }
                _ => self.selection = None, // a click clears it
            },
            _ => {}
        }
    }

    /// "match i/n" while searching ("no matches", or empty before typing).
    pub fn search_status(&self) -> Option<String> {
        let Some(Modal::Search { query, search }) = &self.modal else {
            return None;
        };
        Some(match (query.value.is_empty(), search.matches().len()) {
            (true, _) => String::new(),
            (false, 0) => "no matches".into(),
            (false, n) => format!("match {}/{n}", search.current() + 1),
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
        let TabKind::Terminal(s) = &mut self.tabs[self.active].kind else {
            return;
        };
        let bracketed = s.parser.lock().map(|p| p.screen().bracketed_paste()).unwrap_or(false);
        match termx::paste_lines(text, bracketed, self.cfg.settings.confirm_multiline_paste) {
            Some(n) => {
                let what = if n == 1 { "1 line".to_string() } else { format!("{n} lines") };
                let text_owned = text.to_string();
                self.modal = Some(Modal::Confirm {
                    text: format!("Paste {what} into {title}?"),
                    action: ConfirmAction::Paste { text: text_owned, enter },
                });
            }
            None => paste_then_enter(s, text, enter),
        }
    }

    /// Catches the search up with the active terminal and `query`; scrolls
    /// to the current match when the query changed or `scroll`.
    fn run_search(&mut self, query: &str, search: &mut TermSearch, scroll: bool) {
        let TabKind::Terminal(s) = &self.tabs[self.active].kind else {
            return;
        };
        search.sync(s);
        let changed = search.set_query(query);
        if (changed || scroll)
            && let Ok(mut p) = s.parser.lock()
            && let Some(off) = search.offset(s.scrolled(), s.rows() as usize)
        {
            p.screen_mut().set_scrollback(off);
        }
    }

    /// A paste goes to the open dialog's field (first line only; an ask
    /// takes it only for a secret), else to the terminal or the host filter.
    fn handle_paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or("");
        if self.question_too_new() {
            return; // meant for whatever was there before
        }
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
            Some(Modal::Ask { input, req, .. }) => {
                if req.kind == AskKind::Secret {
                    input.insert_str(line);
                }
                return;
            }
            Some(Modal::Search { .. }) => {
                let Some(Modal::Search { mut query, mut search }) = self.modal.take() else {
                    return;
                };
                query.insert_str(line);
                self.run_search(&query.value, &mut search, false);
                self.modal = Some(Modal::Search { query, search });
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

    /// Routes a key: the open dialog takes everything, then global shortcuts,
    /// then the active tab's own keys.
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

    /// Shortcuts from the keymap work everywhere, including inside terminal
    /// tabs (the defaults are Alt chords; letters match in either case for
    /// Caps Lock, and Alt+Shift letter chords are left for the shell except
    /// `swap_panes`; the defaults avoid readline / emacs Alt keys). Terminal-only
    /// actions elsewhere let the key through. True when the key was used.
    fn handle_global_key(&mut self, key: KeyEvent) -> bool {
        let Some(action) = self.keys.match_key(&key) else {
            return false;
        };
        let terminal = matches!(self.tabs[self.active].kind, TabKind::Terminal(_));
        let split = terminal && self.tabs[self.active].split.is_some();
        match action {
            Action::Tab(n) => {
                let i = n as usize - 1;
                if i < self.tabs.len() {
                    self.active = i;
                }
            }
            Action::NextTab => self.active = (self.active + 1) % self.tabs.len(),
            Action::PrevTab => self.active = (self.active + self.tabs.len() - 1) % self.tabs.len(),
            Action::CloseTab => self.close_active_tab(),
            Action::Theme => self.open_theme_picker(),
            Action::Help => self.modal = Some(Modal::Help { scroll: 0 }),
            Action::Quit => self.request_quit(),
            Action::Find if terminal => {
                self.modal = Some(Modal::Search { query: TextInput::default(), search: TermSearch::default() });
            }
            Action::Snippets if terminal => self.modal = Some(Modal::Snippets { selected: 0 }),
            Action::SplitRight if terminal => self.split(SplitDir::Right),
            Action::SplitDown if terminal => self.split(SplitDir::Down),
            Action::FocusNextPane if terminal => self.focus_other_pane(),
            Action::SwapPanes if split => self.swap_panes(),
            Action::EqualizePanes if split => {
                if let Some(sp) = &mut self.tabs[self.active].split {
                    sp.ratio = 0.5;
                }
            }
            // A resize that doesn't apply leaves the key for the shell.
            Action::ResizePaneLeft => return split && self.resize_split(SplitDir::Right, false),
            Action::ResizePaneRight => return split && self.resize_split(SplitDir::Right, true),
            Action::ResizePaneUp => return split && self.resize_split(SplitDir::Down, false),
            Action::ResizePaneDown => return split && self.resize_split(SplitDir::Down, true),
            _ => return false,
        }
        true
    }

    /// Keys in the host list: the filter field while filtering, else the
    /// list's single-letter commands.
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
            KeyCode::Char('?') => self.modal = Some(Modal::Help { scroll: 0 }),
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

    /// Opens the host editor for a new host, saved to the first ssh config
    /// file, or to Skiff's own config when there is none.
    fn new_host(&mut self) {
        let set = self.load_set();
        let dest = if set.docs.is_empty() { Dest::Skiff } else { Dest::SshConfig(0) };
        self.modal = Some(Modal::Editor(Box::new(Editor::new(HostForm::new_host(dest), set))));
    }

    /// Re-reads `config.toml` (theme, hosts, keys) after outside edits.
    fn reload_config(&mut self) {
        match Config::load_or_create(&self.cfg_path) {
            Ok(cfg) => {
                self.cfg_saved = cfg.clone();
                self.cfg = cfg;
                self.apply_theme_name(&self.cfg.settings.theme.clone());
                self.reload_hosts();
                self.set_info(format!("Reloaded {} hosts", self.hosts.len()));
                self.load_keys();
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// Opens the host editor on the selected host (or shows why it can't be edited).
    fn edit_selected(&mut self) {
        let Some(h) = self.selected_host().cloned() else {
            return;
        };
        let set = self.load_set();
        match HostForm::for_entry(&h, &self.cfg, &set) {
            Ok(form) => self.modal = Some(Modal::Editor(Box::new(Editor::new(form, set)))),
            Err(why) => self.set_error(why),
        }
    }

    /// Asks to delete the selected host; an ssh config host in a read-only
    /// place is refused up front.
    fn delete_selected(&mut self) {
        let Some(h) = self.selected_host().cloned() else {
            return;
        };
        let text = match (&h.source, &h.source_file) {
            (Source::SshConfig, Some(file)) => {
                if let skiff_core::sshconfig::edit::Locate::ReadOnly(why) = self.load_set().locate(&h.name) {
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
        // ssh config edits are written with backups; others go to config.toml.
        if let Saved::Ssh(_) = saved {
            set.save(&backups_dir(&self.cfg_path)).map_err(|e| format!("{e:#}"))?;
        } else {
            self.save_config();
        }
        self.reload_hosts();
        Ok(())
    }

    /// Keys in a terminal tab go to the session (Shift+PageUp/Down scroll
    /// half a screen); once the session ended, Enter starts it again.
    fn terminal_key(&mut self, key: KeyEvent) {
        self.selection = None;
        let TabKind::Terminal(s) = &mut self.tabs[self.active].kind else {
            return;
        };
        if !s.is_alive() {
            if key.code == KeyCode::Enter
                && let Err(e) = s.respawn()
            {
                self.set_error(format!("{e:#}"));
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

    /// Keys in an SFTP tab: navigation, copy, file operations (which open a
    /// prompt or confirmation) and transfer control, on the focused pane.
    fn sftp_key(&mut self, key: KeyEvent) {
        let TabKind::Sftp(s) = &mut self.tabs[self.active].kind else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Status::Failed(_) | Status::Connecting = s.status {
            // Only the local pane is usable until connected.
            if matches!(key.code, KeyCode::Char('r')) {
                s.reconnect();
                return;
            }
        }
        let side = s.focus;
        // Collected here and applied after the match, while `s` borrows `self`.
        let mut modal = None;
        let mut err = None;
        let mut info = None;
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
                modal =
                    Some(Modal::Prompt(Prompt { title: "New directory".into(), input: TextInput::default(), action: PromptAction::Mkdir }))
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
            KeyCode::Char('X') => match s.cancel_newest_transfer() {
                Some(label) => info = Some(format!("Cancelling {label}")),
                None => err = Some("No transfer is running".to_string()),
            },
            _ => {}
        }
        if let Some(i) = info {
            self.set_info(i);
        }
        if let Some(e) = err {
            self.set_error(e);
        }
        if modal.is_some() {
            self.modal = modal;
        }
    }

    /// Keys for the open dialog. The modal is taken out and each arm returns
    /// what to show next (`None` closes it).
    fn handle_modal_key(&mut self, key: KeyEvent) {
        if self.question_too_new() {
            return;
        }
        let Some(modal) = self.modal.take() else {
            return;
        };
        self.modal = match modal {
            Modal::Help { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?') => None,
                _ if self.keys.match_key(&key) == Some(Action::Help) => None,
                KeyCode::Down | KeyCode::Char('j') => Some(Modal::Help { scroll: scroll.saturating_add(1) }),
                KeyCode::Up | KeyCode::Char('k') => Some(Modal::Help { scroll: scroll.saturating_sub(1) }),
                KeyCode::PageDown | KeyCode::Char(' ') => Some(Modal::Help { scroll: scroll.saturating_add(10) }),
                KeyCode::PageUp => Some(Modal::Help { scroll: scroll.saturating_sub(10) }),
                KeyCode::Home | KeyCode::Char('g') => Some(Modal::Help { scroll: 0 }),
                // Drawing stops at the last page.
                KeyCode::End | KeyCode::Char('G') => Some(Modal::Help { scroll: u16::MAX }),
                _ => Some(modal),
            },
            // Moving previews the theme; Enter saves it, Esc restores the original.
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
            Modal::Search { mut query, mut search } => match key.code {
                // Closing the search returns the view to the live screen.
                KeyCode::Esc => {
                    if let TabKind::Terminal(s) = &mut self.tabs[self.active].kind
                        && let Ok(mut p) = s.parser.lock()
                    {
                        p.screen_mut().set_scrollback(0);
                    }
                    None
                }
                KeyCode::Enter => {
                    // Enter: next older match; Shift+Enter: newer.
                    if search.step(!key.modifiers.contains(KeyModifiers::SHIFT)) {
                        self.run_search(&query.value, &mut search, true);
                    }
                    Some(Modal::Search { query, search })
                }
                _ => {
                    if query.handle_key(key) {
                        self.run_search(&query.value, &mut search, false);
                    }
                    Some(Modal::Search { query, search })
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
                        // Keep the paste confirmation if one opened; else close.
                        self.modal.take()
                    }
                    _ => Some(Modal::Snippets { selected }),
                }
            }
            // Answers go straight back to ssh; Esc declines. A notice only
            // closes with Esc.
            Modal::Ask { host, mut input, req } => match (req.kind, key.code) {
                (_, KeyCode::Esc) => {
                    let _ = req.reply.send(None);
                    None
                }
                (AskKind::Secret, KeyCode::Enter) => {
                    let _ = req.reply.send(Some(std::mem::take(&mut input.value)));
                    None
                }
                (AskKind::Confirm, KeyCode::Char('y' | 'Y')) => {
                    let _ = req.reply.send(Some("yes".into()));
                    None
                }
                (AskKind::Confirm, KeyCode::Char('n' | 'N')) => {
                    let _ = req.reply.send(Some("no".into()));
                    None
                }
                (AskKind::Secret, _) => {
                    input.handle_key(key);
                    Some(Modal::Ask { host, input, req })
                }
                (AskKind::Confirm | AskKind::Notice, _) => Some(Modal::Ask { host, input, req }),
            },
            Modal::Overwrite { tab, id, host, path, reply } => {
                let decision = match key.code {
                    KeyCode::Char('o' | 'y') => Some(OverwriteDecision::Overwrite),
                    KeyCode::Char('s' | 'n') => Some(OverwriteDecision::Skip),
                    KeyCode::Char('a') => Some(OverwriteDecision::OverwriteAll),
                    KeyCode::Char('S') => Some(OverwriteDecision::SkipAll),
                    KeyCode::Char('c') | KeyCode::Esc => {
                        // Cancel first: the transfer then stops instead of moving on.
                        if let Some(s) = self.sftp_tab_mut(tab) {
                            s.cancel_transfer(id);
                        }
                        Some(OverwriteDecision::Skip)
                    }
                    _ => None,
                };
                match decision {
                    Some(d) => {
                        let _ = reply.send(d);
                        None
                    }
                    None => Some(Modal::Overwrite { tab, id, host, path, reply }),
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
                    KeyCode::Char('e') if selected < n => Some(Modal::Forward(ForwardForm::edit(&self.cfg.forwards[selected], selected))),
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
        // An answered question may make way for the next one.
        self.next_question();
    }

    /// Returns the modal to keep showing (with an error) or `None` on success.
    fn save_editor(&mut self, mut ed: Box<Editor>) -> Option<Modal> {
        let taken = hostform::taken_names(&self.hosts);
        let old = ed.form.original_name(&self.cfg);
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
        if let Some(old) = old {
            self.after_rename(&old, &name);
        }
        if let Some(pos) = self.filtered.iter().position(|&i| self.hosts[i].name == name) {
            self.host_selected = pos;
        }
        self.set_info(format!("Saved '{name}'"));
        None
    }

    /// Carries out a confirmed action.
    fn run_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::Quit => self.should_quit = true,
            ConfirmAction::Paste { text, enter } => {
                if let TabKind::Terminal(s) = &mut self.tabs[self.active].kind {
                    paste_then_enter(s, &text, enter);
                }
            }
            ConfirmAction::DeleteHost(name) => {
                let Some(h) = self.hosts.iter().find(|h| h.name == name).cloned() else {
                    return;
                };
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

    /// Applies a submitted prompt to the active SFTP tab's focused pane; an
    /// empty answer (or a rename to the same name) does nothing.
    fn run_prompt(&mut self, p: Prompt) {
        let value = p.input.value.trim().to_string();
        if value.is_empty() {
            return;
        }
        let TabKind::Sftp(s) = &mut self.tabs[self.active].kind else {
            return;
        };
        match p.action {
            PromptAction::Mkdir => s.file_op(FileOp::Mkdir(value)),
            PromptAction::Rename { from } if from != value => s.file_op(FileOp::Rename { from, to: value }),
            PromptAction::Rename { .. } => {}
            PromptAction::Goto => {
                // `~` expands only on the local side; remote paths go as typed.
                let side = s.focus;
                let path = if side == Side::Local { sshconfig::expand_tilde(&value).display().to_string() } else { value };
                s.navigate(side, path, None);
            }
        }
    }
}

/// Lines one wheel notch scrolls.
const WHEEL_LINES: i32 = 3;

/// Fewest characters in a key burst read as a paste (no one types this
/// many keys between two reads of the input, ~30 ms apart).
const PASTE_BURST_CHARS: usize = 8;

/// The text of `events` if they are all plain key presses (and releases)
/// forming a paste: a line break and at least `PASTE_BURST_CHARS` chars.
fn key_burst_text(events: &[Event]) -> Option<String> {
    let mut text = String::new();
    // Any non-key event, modifier chord or special key means it's typing.
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
