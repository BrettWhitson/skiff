//! The application: owns config, hosts, theme, tabs and the engine, and
//! implements the toolkit's `App` trait. Screens live in their own modules.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use termax_core::config::{Config, FileWatcher, StatusChecks};
use termax_core::forward::{Forward, ForwardHandle, ForwardState};
use termax_core::history::{self, ConnKind, History};
use termax_core::hostform::{self, Saved};
use termax_core::hosts::{self, HostEntry};
use termax_core::sshconfig::edit::{ConfigSet, backups_dir};
use termax_core::state;
use termax_core::status::{self, Status};
use termax_core::theme::{ThemeDef, builtin};
use termax_ui::geom::Rect;
use termax_ui::input::Event;
use termax_ui::layout::Size as L;
use termax_ui::shell::{App, FrameCtx, Waker};
use termax_ui::style::Style;
use termax_ui::text::{FontKind, Fonts};
use termax_ui::ui::{Id, Ui, UiState};
use termax_ui::widgets::{TabTitle, button, label, splitter_h, tabs};

use crate::engine::{Engine, Msg};
use crate::files::{FilesTab, Location};
use crate::keys::{Action, Keymap};
use crate::forwards::{ForwardsState, Running};
use crate::sidebar::{Sidebar, SidebarAction, SidebarCtx, sidebar};
use crate::switcher::{Cmd, Item, SwitcherState};
use crate::term::TermTab;
use crate::theme::{GuiTheme, from_def};

/// Starts the askpass broker; its questions arrive as `Msg::Ask`.
pub fn start_broker(engine: &Engine, helper: PathBuf) -> anyhow::Result<termax_core::askpass::Broker> {
    let (broker, mut rx) = termax_core::askpass::Broker::start(engine.rt.handle(), helper)?;
    let tx = engine.sender();
    engine.rt.spawn(async move {
        while let Some(req) = rx.recv().await {
            tx.send(Msg::Ask(req));
        }
    });
    Ok(broker)
}

/// How long geometry changes wait before being written to `state.toml`.
const GEOMETRY_SAVE_DELAY: Duration = Duration::from_secs(1);
/// How often `config.toml` is checked for outside edits (window focused).
const CONFIG_POLL: Duration = Duration::from_secs(1);

pub enum TabKind {
    Terminal(Box<TermTab>),
    Files(Box<FilesTab>),
    Settings,
    Forwards,
}

pub struct Tab {
    pub id: u64,
    pub kind: TabKind,
}

pub struct GuiApp {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    /// The config as last loaded/saved: `Config::save` diffs against it.
    pub baseline: Config,
    pub hosts: Vec<HostEntry>,
    /// Bumped whenever `hosts` is rebuilt (forms follow the list by it).
    pub hosts_gen: u64,
    /// The ssh config files (main first), for "Move to file…".
    pub ssh_files: Vec<PathBuf>,
    /// Reachability per host name (opt-in checks).
    pub statuses: HashMap<String, Status>,
    pub history: History,
    history_path: PathBuf,
    /// Terminal tabs to record in history once they prove connected.
    pending_records: Vec<(u64, String, Instant)>,
    pub forwards: ForwardsState,
    pub theme: GuiTheme,
    pub style: Style,
    pub keymap: Keymap,
    /// Geometry kept outside `config.toml`.
    pub geometry: state::UiState,
    pub geometry_path: PathBuf,
    geometry_dirty: Option<Instant>,
    /// Problems and notices for the status bar (newest last).
    pub warnings: Vec<String>,
    pub engine: Engine,
    pub tabs: Vec<Tab>,
    pub active: usize,
    next_tab_id: u64,
    pub sidebar: Sidebar,
    /// Opened by Ctrl+Shift+K / "+" (rendered by the quick switcher).
    pub switcher_open: bool,
    switcher: SwitcherState,
    switcher_was_open: bool,
    /// Where focus goes back to when the switcher closes.
    switcher_return_focus: Option<Id>,
    /// Settings should start a new connection form.
    pub new_connection: bool,
    /// Actions for the active view (copy, paste, zoom), consumed by it.
    pub view_actions: Vec<Action>,
    /// Host (by name) the settings screen should open for editing.
    pub edit_target: Option<String>,
    /// Host to open on the first frame (from the command line; `true` = files).
    pub open: Option<(String, bool)>,
    /// Where Local file panes start.
    pub start_dir: String,
    /// Relays ssh password / host-key prompts into file panes.
    pub askpass: Option<termax_core::askpass::Broker>,
    /// `ssh -V`, read when the first file tab opens.
    ssh_version: Option<Option<(u32, u32)>>,
    /// The tab shown last frame (focus moves into a newly shown terminal).
    shown_tab: Option<u64>,
    /// A files tab whose close waits for "Close anyway" (transfers running).
    closing: Option<u64>,
    /// Form state of the settings tab (kept while the tab is closed).
    pub settings: crate::settings::SettingsState,
    /// Fonts to install after this frame (set by the settings screen).
    pub pending_fonts: Option<Fonts>,
    /// Notices outside edits of `config.toml`.
    watcher: FileWatcher,
    last_poll: Instant,
    window_focused: bool,
    ui_state: UiState,
}

/// The theme named in the config, falling back to the first built-in.
pub fn find_theme(cfg: &Config) -> ThemeDef {
    cfg.themes
        .iter()
        .chain(builtin().iter())
        .find(|t| t.name == termax_core::theme::effective_name(&cfg.settings.theme, crate::os::high_contrast()))
        .cloned()
        .unwrap_or_else(|| builtin().remove(0))
}

/// The configured local start directory, else the home directory.
pub fn local_start_dir(configured: &Option<String>) -> String {
    configured
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(termax_core::sshconfig::expand_tilde)
        .or_else(termax_core::sshconfig::home_dir)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into())
}

pub fn style_for(theme: &GuiTheme, cfg: &Config) -> Style {
    let mut s = Style::dark_default();
    s.palette = theme.palette;
    s.font_px *= cfg.gui.ui_scale.clamp(0.5, 3.0);
    s.mono_px = cfg.gui.font_size.clamp(6.0, 72.0);
    s
}

impl GuiApp {
    pub fn new(cfg: Config, cfg_path: PathBuf, waker: Option<Waker>) -> GuiApp {
        let wake = waker.map(|w| Arc::new(move || w.wake()) as crate::engine::WakeFn);
        let (hosts, host_warning) = hosts::collect(&cfg);
        let theme = from_def(&find_theme(&cfg));
        let style = style_for(&theme, &cfg);
        let (keymap, mut warnings) = Keymap::platform(&cfg.keys);
        warnings.extend(host_warning);
        let geometry_path = state::UiState::path_for(&cfg_path);
        let cfg_start = cfg.settings.local_start_dir.clone();
        let geometry = state::UiState::load(&geometry_path);
        let engine = Engine::new(wake.clone());
        // Only the real app (with a window) relays prompts: the helper is
        // this executable, started by ssh.
        let askpass = match (&wake, std::env::current_exe()) {
            (Some(_), Ok(exe)) => match start_broker(&engine, exe) {
                Ok(b) => Some(b),
                Err(e) => {
                    warnings.push(format!("password prompts unavailable: {e:#}"));
                    None
                }
            },
            _ => None,
        };
        let cfg_path_for_watch = cfg_path.clone();
        let history_path = History::path_for(&cfg_path);
        let on_start = cfg.settings.status_checks == StatusChecks::OnStart;
        let mut app = GuiApp {
            baseline: cfg.clone(),
            ssh_files: ssh_files(&cfg),
            cfg,
            cfg_path,
            hosts,
            hosts_gen: 0,
            statuses: HashMap::new(),
            history: History::load(&history_path),
            history_path,
            pending_records: vec![],
            forwards: ForwardsState::default(),
            theme,
            style,
            keymap,
            geometry,
            geometry_path,
            geometry_dirty: None,
            warnings,
            engine,
            tabs: vec![],
            active: 0,
            next_tab_id: 1,
            sidebar: Sidebar::default(),
            switcher_open: false,
            switcher: SwitcherState::default(),
            switcher_was_open: false,
            switcher_return_focus: None,
            new_connection: false,
            view_actions: vec![],
            edit_target: None,
            open: None,
            start_dir: local_start_dir(&cfg_start),
            askpass,
            ssh_version: None,
            shown_tab: None,
            closing: None,
            settings: Default::default(),
            pending_fonts: None,
            watcher: FileWatcher::new(&cfg_path_for_watch),
            last_poll: Instant::now(),
            window_focused: true,
            ui_state: UiState::default(),
        };
        if on_start {
            app.check_status(None);
        }
        app
    }

    pub fn warn(&mut self, w: impl Into<String>) {
        self.warnings.push(w.into());
        if self.warnings.len() > 20 {
            self.warnings.remove(0);
        }
    }

    // ----- tabs -------------------------------------------------------

    fn push_tab(&mut self, kind: TabKind) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab { id, kind });
        self.active = self.tabs.len() - 1;
        id
    }

    pub fn open_terminal(&mut self, host: &HostEntry) -> u64 {
        let id = self.push_tab(TabKind::Terminal(Box::new(TermTab::new(host.clone(), &self.cfg.settings.ssh_binary, self.cfg.settings.scrollback))));
        self.pending_records.push((id, host.name.clone(), Instant::now()));
        id
    }

    /// Shows the Forwards tab (opening it if needed).
    pub fn open_forwards(&mut self) {
        match self.tabs.iter().position(|t| matches!(t.kind, TabKind::Forwards)) {
            Some(i) => self.active = i,
            None => {
                self.push_tab(TabKind::Forwards);
            }
        }
    }

    pub fn open_files(&mut self, left: Location, right: Location) -> u64 {
        if self.ssh_version.is_none() {
            self.ssh_version = Some(termax_core::sshver::client_version(&self.cfg.settings.ssh_binary).ok());
        }
        let mut tab = FilesTab::new(left, right);
        for p in &mut tab.panes {
            p.show_hidden = self.cfg.settings.show_hidden_files;
        }
        self.push_tab(TabKind::Files(Box::new(tab)))
    }

    pub fn open_settings(&mut self) {
        if let Some(name) = self.edit_target.take() {
            let mut st = std::mem::take(&mut self.settings);
            st.section = crate::settings::Section::Connections;
            st.conn.edit_host(&name, self);
            self.settings = st;
        } else if std::mem::take(&mut self.new_connection) {
            let mut st = std::mem::take(&mut self.settings);
            st.section = crate::settings::Section::Connections;
            st.conn.new_form(self);
            self.settings = st;
        }
        match self.tabs.iter().position(|t| matches!(t.kind, TabKind::Settings)) {
            Some(i) => self.active = i,
            None => {
                self.push_tab(TabKind::Settings);
            }
        }
    }

    /// Closes the tab, asking first when it still has transfers running.
    pub fn request_close_tab(&mut self, id: u64) {
        let running = match self.tabs.iter().find(|t| t.id == id).map(|t| &t.kind) {
            Some(TabKind::Files(f)) => f.transfers.running(),
            _ => 0,
        };
        if running > 0 {
            self.closing = Some(id);
            if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                self.active = i;
            }
        } else {
            self.close_tab(id);
        }
    }

    /// Closes the tab now: its transfers are cancelled, its connections
    /// stopped, and anything it still receives is dropped.
    pub fn close_tab(&mut self, id: u64) {
        if self.closing == Some(id) {
            self.closing = None;
        }
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            let mut t = self.tabs.remove(i);
            if let TabKind::Files(f) = &mut t.kind {
                f.shutdown(self.askpass.as_ref().map(|b| (b, id)));
            }
            drop(t);
            if self.active >= i && self.active > 0 {
                self.active -= 1;
            }
        }
    }

    pub fn tab_title(&self, i: usize) -> String {
        match &self.tabs[i].kind {
            TabKind::Terminal(t) => format!("\u{25CF} {}", t.host.name),
            TabKind::Files(f) => f.title(),
            TabKind::Settings => "Settings".into(),
            TabKind::Forwards => "\u{21C4} Forwards".into(),
        }
    }

    pub fn active_tab_id(&self) -> Option<u64> {
        self.tabs.get(self.active).map(|t| t.id)
    }

    // ----- config -----------------------------------------------------

    /// Writes changed settings (comments and other fields kept) and
    /// rebuilds what depends on them.
    pub fn save_config(&mut self) {
        match self.cfg.save(&self.baseline, &self.cfg_path) {
            Ok(()) => self.baseline = self.cfg.clone(),
            Err(e) => self.warn(format!("could not save config: {e}")),
        }
        self.watcher.poll(); // our own write is not an outside edit
        let (km, warnings) = Keymap::platform(&self.cfg.keys);
        self.keymap = km;
        for w in warnings {
            self.warn(w);
        }
        self.rebuild_hosts();
    }

    pub fn rebuild_hosts(&mut self) {
        let (hosts, w) = hosts::collect(&self.cfg);
        self.hosts = hosts;
        self.hosts_gen += 1;
        self.ssh_files = ssh_files(&self.cfg);
        if let Some(w) = w {
            self.warn(w);
        }
    }

    // ----- host management -------------------------------------------

    /// The ssh config files, or an empty set when ssh config is off or unreadable.
    pub fn load_set(&mut self) -> ConfigSet {
        match hosts::ssh_config_path(&self.cfg) {
            Some(p) => ConfigSet::load(&p).unwrap_or_else(|e| {
                self.warn(format!("{e:#}"));
                ConfigSet::default()
            }),
            None => ConfigSet::default(),
        }
    }

    /// Writes what a host change touched (with a backup for ssh config)
    /// and rebuilds the host list.
    pub fn apply_saved(&mut self, saved: Saved, set: &mut ConfigSet) -> Result<(), String> {
        match saved {
            Saved::Ssh(_) => {
                set.save(&backups_dir(&self.cfg_path)).map_err(|e| format!("{e:#}"))?;
                self.rebuild_hosts();
            }
            Saved::Termax(_) => self.save_config(),
        }
        Ok(())
    }

    /// Adds a copy of host `i` (named `<name>-copy`) next to it; returns its name.
    pub fn duplicate_host(&mut self, i: usize) -> Result<String, String> {
        let h = self.hosts.get(i).cloned().ok_or("That host is gone")?;
        let mut set = self.load_set();
        let taken = hostform::taken_names(&self.hosts);
        let form = hostform::duplicate(&h, &self.cfg, &set, &taken)?;
        let saved = form.save(&mut self.cfg, &mut set, &taken)?;
        let name = saved.name().to_string();
        self.apply_saved(saved, &mut set)?;
        Ok(name)
    }

    pub fn delete_host(&mut self, i: usize) -> Result<(), String> {
        let h = self.hosts.get(i).cloned().ok_or("That host is gone")?;
        let mut set = self.load_set();
        let saved = hostform::delete(&h, &mut self.cfg, &mut set)?;
        self.apply_saved(saved, &mut set)
    }

    fn set_host_hidden(&mut self, i: usize, hidden: bool) -> Result<(), String> {
        let h = self.hosts.get(i).cloned().ok_or("That host is gone")?;
        let mut set = self.load_set();
        let saved = hostform::set_hidden(&h, hidden, &mut self.cfg, &mut set)?;
        self.apply_saved(saved, &mut set)
    }

    fn move_host(&mut self, i: usize, doc: usize) -> Result<(), String> {
        let h = self.hosts.get(i).cloned().ok_or("That host is gone")?;
        let mut set = self.load_set();
        set.move_host(&h.name, doc)?;
        self.apply_saved(Saved::Ssh(h.name), &mut set)
    }

    pub fn set_show_hidden(&mut self, show: bool) {
        self.geometry.show_hidden = show;
        self.geometry_changed();
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        match self.history.save(&self.history_path) {
            Ok(()) => self.warn("History cleared"),
            Err(e) => self.warn(format!("could not save history: {e:#}")),
        }
    }

    pub fn record(&mut self, name: &str, kind: ConnKind) {
        self.history.record(name, kind, history::now());
        if let Err(e) = self.history.save(&self.history_path) {
            self.warn(format!("could not save history: {e:#}"));
        }
    }

    /// Checks host `only`, or every listed host. Never runs on its own
    /// unless `status_checks = "on_start"`.
    pub fn check_status(&mut self, only: Option<usize>) {
        let sets = hosts::probe_sets(&self.cfg);
        let idx: Vec<usize> = match only {
            Some(i) => vec![i],
            None => hosts::visible(&self.hosts, self.geometry.show_hidden),
        };
        let items: Vec<(String, hosts::Probe)> =
            idx.into_iter().filter_map(|i| self.hosts.get(i)).map(|h| (h.name.clone(), h.probe(sets.as_deref()))).collect();
        for (n, _) in &items {
            self.statuses.insert(n.clone(), Status::Checking);
        }
        let tx = self.engine.sender();
        status::check_all(self.engine.rt.handle(), items, Arc::new(move |host, status| {
            tx.send(Msg::HostStatus { host, status });
        }));
    }

    /// The sidebar's selected host.
    pub fn sidebar_selected(&self) -> Option<usize> {
        self.sidebar.selected_host(&self.sidebar_ctx())
    }

    fn sidebar_ctx(&self) -> SidebarCtx<'_> {
        SidebarCtx {
            hosts: &self.hosts,
            statuses: &self.statuses,
            history: &self.history,
            sort: self.geometry.sort,
            show_hidden: self.geometry.show_hidden,
            files: &self.ssh_files,
        }
    }

    /// Starts forward `f` in the background; prompts arrive in the Forwards tab.
    pub fn start_forward(&mut self, st: &mut ForwardsState, f: &Forward) {
        let Some(host) = self.hosts.iter().find(|h| h.name == f.host).cloned() else {
            self.warn(format!("unknown host: {}", f.host));
            return;
        };
        st.stop(f);
        st.running.retain(|r| &r.fwd != f || r.handle.state().is_live());
        if self.ssh_version.is_none() {
            self.ssh_version = Some(termax_core::sshver::client_version(&self.cfg.settings.ssh_binary).ok());
        }
        let can_prompt = self.askpass.is_some() && self.ssh_version.flatten().is_none_or(termax_core::sshver::supports_askpass_require);
        let tag = st.new_tag();
        let mut args = f.background_args(&host);
        let mut env = vec![];
        match (&self.askpass, can_prompt) {
            (Some(b), true) => env = b.env(&tag),
            _ => args.splice(0..0, ["-o".to_string(), "BatchMode=yes".to_string()]).for_each(drop),
        }
        let handle = ForwardHandle::start(self.engine.rt.handle(), &self.cfg.settings.ssh_binary, args, env);
        st.running.push(Running { fwd: f.clone(), handle, tag, recorded: false });
    }

    /// History for terminals that have shown output and are still running
    /// two seconds in, and for forwards that came up.
    fn record_connections(&mut self) {
        let mut done = vec![];
        self.pending_records.retain(|(id, name, at)| {
            let Some(tab) = self.tabs.iter_mut().find(|t| t.id == *id) else { return false };
            let TabKind::Terminal(t) = &mut tab.kind else { return false };
            match &mut t.session {
                Some(s) if at.elapsed() >= Duration::from_secs(2) && s.generation() > 0 => {
                    if s.is_alive() {
                        done.push((name.clone(), ConnKind::Terminal));
                    }
                    false
                }
                _ => t.error.is_none(),
            }
        });
        for r in &mut self.forwards.running {
            if !r.recorded && r.handle.state() == ForwardState::Running {
                r.recorded = true;
                done.push((r.fwd.host.clone(), ConnKind::Forward));
            }
        }
        for (name, kind) in done {
            self.record(&name, kind);
        }
    }

    fn on_sidebar(&mut self, a: SidebarAction) {
        match a {
            SidebarAction::OpenTerminal(h) => {
                if let Some(host) = self.hosts.get(h).cloned() {
                    self.open_terminal(&host);
                }
            }
            SidebarAction::OpenFiles(h) => {
                if let Some(host) = self.hosts.get(h).cloned() {
                    self.open_files(Location::Local, Location::Host(host));
                }
            }
            SidebarAction::Edit(h) => {
                self.edit_target = self.hosts.get(h).map(|h| h.name.clone());
                self.open_settings();
            }
            SidebarAction::Duplicate(h) => {
                if let Err(e) = self.duplicate_host(h) {
                    self.warn(e);
                }
            }
            SidebarAction::Delete(h) => {
                if let Err(e) = self.delete_host(h) {
                    self.warn(e);
                }
            }
            SidebarAction::SetHidden(h, hidden) => {
                if let Err(e) = self.set_host_hidden(h, hidden) {
                    self.warn(e);
                }
            }
            SidebarAction::MoveTo(h, doc) => {
                if let Err(e) = self.move_host(h, doc) {
                    self.warn(e);
                }
            }
            SidebarAction::Forward(h) => {
                self.forwards.new_form(h);
                self.open_forwards();
            }
            SidebarAction::Check(h) => self.check_status(Some(h)),
            SidebarAction::CheckAll => self.check_status(None),
            SidebarAction::SetSort(s) => {
                self.geometry.sort = s;
                self.geometry_changed();
            }
            SidebarAction::ToggleShowHidden => self.set_show_hidden(!self.geometry.show_hidden),
            SidebarAction::OpenForwards => self.open_forwards(),
            SidebarAction::NewHost => {
                self.edit_target = None;
                self.new_connection = true;
                self.open_settings();
            }
        }
    }

    /// Switches to theme `name` and saves it.
    pub fn apply_theme(&mut self, name: &str) {
        self.cfg.settings.theme = name.to_string();
        self.theme = from_def(&find_theme(&self.cfg));
        self.style = style_for(&self.theme, &self.cfg);
        self.save_config();
    }

    /// Re-reads `config.toml` (after an outside edit, or on request).
    pub fn reload_config(&mut self) {
        match Config::load_or_create(&self.cfg_path) {
            Ok(cfg) => {
                self.baseline = cfg.clone();
                self.cfg = cfg;
                self.theme = from_def(&find_theme(&self.cfg));
                self.style = style_for(&self.theme, &self.cfg);
                let (km, warnings) = Keymap::platform(&self.cfg.keys);
                self.keymap = km;
                for w in warnings {
                    self.warn(w);
                }
                self.rebuild_hosts();
            }
            Err(e) => self.warn(format!("config not reloaded: {e:#}")),
        }
    }

    pub fn theme_names(&self) -> Vec<String> {
        let mut names: Vec<String> = builtin().into_iter().map(|t| t.name).collect();
        for t in &self.cfg.themes {
            if !names.contains(&t.name) {
                names.push(t.name.clone());
            }
        }
        names
    }

    fn on_switch(&mut self, item: Item) {
        match item {
            Item::Host(i) => {
                if let Some(h) = self.hosts.get(i).cloned() {
                    self.open_terminal(&h);
                }
            }
            Item::HostFiles(i) => {
                if let Some(h) = self.hosts.get(i).cloned() {
                    self.open_files(Location::Local, Location::Host(h));
                }
            }
            Item::Tab(id) => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                    self.active = i;
                }
            }
            Item::Theme(name) => self.apply_theme(&name),
            Item::Command(Cmd::NewConnection) => {
                self.edit_target = None;
                self.new_connection = true;
                self.open_settings();
            }
            Item::Command(Cmd::Settings) => self.open_settings(),
            Item::Command(Cmd::Forwards) => self.open_forwards(),
            Item::Command(Cmd::CheckStatus) => self.check_status(None),
            Item::Command(Cmd::ReloadConfig) => self.reload_config(),
        }
    }

    fn switcher_ui(&mut self, ui: &mut Ui) {
        if self.switcher_open && !self.switcher_was_open {
            self.switcher = SwitcherState { just_opened: true, ..Default::default() };
            self.switcher_return_focus = ui.focused();
        }
        self.switcher_was_open = self.switcher_open;
        if !self.switcher_open {
            return;
        }
        let tabs: Vec<(u64, String)> = (0..self.tabs.len()).map(|i| (self.tabs[i].id, self.tab_title(i))).collect();
        let ctx = crate::switcher::RankCtx {
            show_hidden: self.geometry.show_hidden,
            recent: self.history.recent(crate::switcher::RECENT_HOSTS),
        };
        let items = crate::switcher::rank(&self.switcher.query, &self.hosts, &tabs, &self.theme_names(), &ctx);
        let out = crate::switcher::show(ui, &mut self.switcher, &items);
        if out.close {
            self.switcher_open = false;
            self.switcher_was_open = false;
            if let Some(f) = self.switcher_return_focus.take() {
                ui.request_focus(f);
            }
        }
        if let Some(item) = out.chosen {
            self.on_switch(item);
        }
    }

    // ----- geometry ---------------------------------------------------

    fn geometry_changed(&mut self) {
        self.geometry_dirty.get_or_insert_with(Instant::now);
    }

    /// Writes `state.toml` now if anything changed.
    pub fn flush_geometry(&mut self) {
        if self.geometry_dirty.take().is_some()
            && let Err(e) = self.geometry.save(&self.geometry_path)
        {
            self.warn(format!("could not save window state: {e}"));
        }
    }

    // ----- actions ----------------------------------------------------

    fn in_terminal(&self) -> bool {
        matches!(self.tabs.get(self.active).map(|t| &t.kind), Some(TabKind::Terminal(_)))
    }

    pub fn do_action(&mut self, a: Action) {
        match a {
            Action::QuickSwitcher | Action::NewTab => self.switcher_open = true,
            Action::CloseTab => {
                if let Some(id) = self.active_tab_id() {
                    self.request_close_tab(id);
                }
            }
            Action::NextTab if !self.tabs.is_empty() => self.active = (self.active + 1) % self.tabs.len(),
            Action::PrevTab if !self.tabs.is_empty() => {
                self.active = (self.active + self.tabs.len() - 1) % self.tabs.len()
            }
            Action::ToggleSidebar => {
                self.geometry.sidebar_collapsed = !self.geometry.sidebar_collapsed;
                self.geometry_changed();
            }
            Action::Settings => self.open_settings(),
            Action::UiScaleUp | Action::UiScaleDown | Action::UiScaleReset => {
                let cur = self.cfg.gui.ui_scale;
                let next = match a {
                    Action::UiScaleUp => cur + 0.1,
                    Action::UiScaleDown => cur - 0.1,
                    _ => 1.0,
                };
                self.cfg.gui.ui_scale = ((next * 10.0).round() / 10.0).clamp(0.5, 3.0);
                self.style = style_for(&self.theme, &self.cfg);
                self.save_config();
            }
            Action::Copy | Action::Paste | Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => {
                self.view_actions.push(a)
            }
            _ => {}
        }
    }

    /// Takes app shortcuts out of this frame's input and runs them.
    fn handle_shortcuts(&mut self, ui: &mut Ui) {
        let in_term = self.in_terminal();
        let hits: Vec<_> = ui
            .input()
            .events
            .iter()
            .filter_map(|e| match e {
                // Ctrl+Alt that produces a character is AltGr typing (e.g. `}`
                // on a German keyboard), never a shortcut.
                Event::KeyDown { mods, text: Some(_), .. } if mods.ctrl && mods.alt => None,
                Event::KeyDown { key, mods, .. } => {
                    self.keymap.match_key(key, *mods, in_term).map(|a| (key.clone(), *mods, a))
                }
                _ => None,
            })
            .collect();
        for (key, mods, a) in hits {
            ui.input_mut().take_key(&key, mods);
            self.do_action(a);
        }
    }

    fn handle_messages(&mut self) -> bool {
        let msgs = self.engine.drain();
        let any = !msgs.is_empty();
        for m in msgs {
            if let Some(tab) = m.tab()
                && !self.tabs.iter().any(|t| t.id == tab)
            {
                if let Msg::Ask(req) = m {
                    let _ = req.reply.send(None); // refuse: nobody can answer it
                }
                continue; // the tab is gone
            }
            match m {
                Msg::HostStatus { host, status } => {
                    self.statuses.insert(host, status);
                }
                Msg::Notice { text, .. } => self.warn(text),
                Msg::Transfer { tab, ev } => {
                    let ctx = crate::files::FilesCtx {
                        tab_id: tab,
                        engine: &self.engine,
                        start_dir: &self.start_dir,
                        hosts: &self.hosts,
                        askpass: self.askpass.as_ref(),
                        ssh_binary: &self.cfg.settings.ssh_binary,
                        ssh_version: self.ssh_version.flatten(),
                    };
                    if let Some(TabKind::Files(f)) = self.tabs.iter_mut().find(|t| t.id == tab).map(|t| &mut t.kind) {
                        f.on_transfer(ev, &ctx);
                    }
                }
                Msg::Ask(req) if req.tag.starts_with(crate::forwards::TAG_PREFIX) => {
                    let live = self.forwards.running.iter().any(|r| r.tag == req.tag && r.handle.state().is_live());
                    if live {
                        self.forwards.ask(req);
                    } else {
                        let _ = req.reply.send(None);
                    }
                }
                Msg::Ask(req) => {
                    let tab = crate::files::parse_tag(&req.tag).map(|(t, _)| t);
                    match self.tabs.iter_mut().find(|t| Some(t.id) == tab).map(|t| &mut t.kind) {
                        Some(TabKind::Files(f)) => crate::files::route_ask(f, tab.unwrap_or(0), req),
                        _ => {
                            let _ = req.reply.send(None);
                        }
                    }
                }
                Msg::Files { tab, side, ev } => {
                    if let crate::files::FilesEvent::Connected { result: Ok(_), .. } = &ev
                        && let Some(TabKind::Files(f)) = self.tabs.iter().find(|t| t.id == tab).map(|t| &t.kind)
                        && let Some(Location::Host(h)) = f.panes.get(side).map(|p| &p.loc)
                    {
                        let name = h.name.clone();
                        self.record(&name, ConnKind::Files);
                    }
                    let ctx = crate::files::FilesCtx {
                        tab_id: tab,
                        engine: &self.engine,
                        start_dir: &self.start_dir,
                        hosts: &self.hosts,
                        askpass: self.askpass.as_ref(),
                        ssh_binary: &self.cfg.settings.ssh_binary,
                        ssh_version: self.ssh_version.flatten(),
                    };
                    if let Some(TabKind::Files(f)) = self.tabs.iter_mut().find(|t| t.id == tab).map(|t| &mut t.kind) {
                        f.on_event(side, ev, &ctx);
                    }
                }
            }
        }
        any
    }

    // ----- frame ------------------------------------------------------

    /// Builds one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        // Immediate mode shows a state change made late in a frame on the
        // next one: after any input or engine message, run one more frame.
        let had_messages = self.handle_messages();
        if had_messages || !ui.input().events.is_empty() {
            ui.request_repaint_in(Duration::ZERO);
        }
        self.watch_config(ui);
        if let Some((name, files)) = self.open.take() {
            match self.hosts.iter().find(|h| h.name == name).cloned() {
                Some(h) if files => {
                    self.open_files(Location::Local, Location::Host(h));
                }
                Some(h) => {
                    self.open_terminal(&h);
                }
                None => self.warnings.push(format!("unknown host: {name}")),
            }
        }
        if !self.settings.capturing() || !self.tabs.get(self.active).is_some_and(|t| matches!(t.kind, TabKind::Settings)) {
            self.handle_shortcuts(ui);
        }
        self.track_window_size(ui);
        self.record_connections();

        let win = ui.window_rect();
        let p = self.theme.palette;
        ui.painter().rect(win, p.bg, 0.0);
        let parts = ui.split_v(win, &[L::Fill, L::Px(24)], 0);
        let (body, status) = (parts[0], parts[1]);

        let main = if self.geometry.sidebar_collapsed {
            body
        } else {
            let before = self.geometry.sidebar_width;
            let (side, main) = splitter_h(ui, Id::new("sidebar-split"), body, &mut self.geometry.sidebar_width, 160.0, 600.0);
            if self.geometry.sidebar_width != before {
                self.geometry_changed();
            }
            let mut sb = std::mem::take(&mut self.sidebar);
            let ctx = self.sidebar_ctx();
            let a = ui.region(Id::new("region-hosts"), "Hosts", |ui| sidebar(ui, side, &mut sb, &ctx));
            self.sidebar = sb;
            if let Some(a) = a {
                self.on_sidebar(a);
            }
            main
        };
        self.main_area(ui, main);
        self.view_actions.clear(); // only the active view may use them
        self.status_bar(ui, status);
        self.switcher_ui(ui);
        self.save_geometry_when_due(ui);
    }

    fn track_window_size(&mut self, ui: &mut Ui) {
        let win = ui.window_rect();
        if win.w <= 0 || win.h <= 0 {
            return;
        }
        let s = ui.scale();
        let logical = ((win.w as f32 / s).round() as u32, (win.h as f32 / s).round() as u32);
        if self.geometry.window_size != Some(logical) {
            let first = self.geometry.window_size.is_none();
            self.geometry.window_size = Some(logical);
            if !first {
                self.geometry_changed();
            }
        }
    }

    fn save_geometry_when_due(&mut self, ui: &mut Ui) {
        if let Some(since) = self.geometry_dirty {
            let age = since.elapsed();
            if age >= GEOMETRY_SAVE_DELAY && !ui.input().left_down {
                self.flush_geometry();
            } else {
                ui.request_repaint_in(GEOMETRY_SAVE_DELAY.saturating_sub(age).max(Duration::from_millis(50)));
            }
        }
    }

    fn main_area(&mut self, ui: &mut Ui, rect: Rect) {
        let parts = ui.split_v(rect, &[L::Px(34), L::Fill], 0);
        let (strip, content) = (parts[0], parts[1]);
        let titles: Vec<String> = (0..self.tabs.len()).map(|i| self.tab_title(i)).collect();
        let tt: Vec<TabTitle> = titles.iter().map(|t| TabTitle { text: t, closable: true }).collect();
        let active = &mut self.active;
        let r = ui.region(Id::new("region-tabs"), "Tabs", |ui| tabs(ui, Id::new("tabs"), strip, &tt, active));
        if let Some((from, to)) = r.moved
            && from < self.tabs.len()
            && to < self.tabs.len()
        {
            let t = self.tabs.remove(from);
            self.tabs.insert(to, t);
        }
        if let Some(i) = r.closed
            && let Some(id) = self.tabs.get(i).map(|t| t.id)
        {
            self.request_close_tab(id);
        }
        if let Some(id) = self.closing {
            self.close_confirm(ui, strip, id);
        }
        if r.plus_clicked {
            self.switcher_open = true;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        ui.region(Id::new("region-content"), "Content", |ui| self.tab_content(ui, content));
    }

    /// The active tab's view.
    fn tab_content(&mut self, ui: &mut Ui, content: Rect) {
        let p = self.theme.palette;
        ui.painter().rect(content, p.bg, 0.0);
        let Some(tab) = self.tabs.get_mut(self.active) else {
            let hint = "Double-click a host to open a terminal, or press Ctrl+Shift+K";
            let r = Rect::new(content.x + ui.px(24), content.y + ui.px(24), content.w - ui.px(48), ui.px(24));
            label(ui, r, hint, Some(p.muted));
            return;
        };
        let line = Rect::new(content.x + ui.px(16), content.y + ui.px(16), content.w - ui.px(32), ui.px(24));
        let newly_shown = self.shown_tab != Some(tab.id);
        self.shown_tab = Some(tab.id);
        let mut show_settings = false;
        let mut show_forwards = false;
        match &mut tab.kind {
            TabKind::Terminal(t) => {
                t.announce = self.cfg.gui.announce_output;
                t.blink = self.cfg.gui.cursor_blink && crate::os::animations();
                let id = Id::new(("term", tab.id));
                if newly_shown {
                    t.shown();
                    // A tab chosen from the strip by keyboard leaves focus there.
                    if !ui.has_focus(Id::new("tabs")) {
                        ui.request_focus(id);
                    }
                }
                crate::term::view(ui, id, content, t, &self.theme, &self.keymap, self.engine.wake_fn(), &mut self.view_actions);
            }
            TabKind::Files(f) => {
                let _ = line;
                let ctx = crate::files::FilesCtx {
                    tab_id: tab.id,
                    engine: &self.engine,
                    start_dir: &self.start_dir,
                    hosts: &self.hosts,
                    askpass: self.askpass.as_ref(),
                    ssh_binary: &self.cfg.settings.ssh_binary,
                    ssh_version: self.ssh_version.flatten(),
                };
                crate::files::view(ui, content.inset(ui.px(6)), f, &ctx);
            }
            TabKind::Settings => {
                let _ = line;
                show_settings = true;
            }
            TabKind::Forwards => show_forwards = true,
        }
        if show_settings {
            crate::settings::view(ui, content, self);
        }
        if show_forwards {
            crate::forwards::view(ui, content, self);
        }
    }

    /// Reloads `config.toml` after an outside edit: checked once a second
    /// while the window has focus, and at once when it regains focus.
    fn watch_config(&mut self, ui: &mut Ui) {
        let mut poll_now = false;
        for e in &ui.input().events {
            match e {
                Event::FocusLost => self.window_focused = false,
                Event::FocusGained => {
                    self.window_focused = true;
                    poll_now = true;
                }
                _ => {}
            }
        }
        if !self.window_focused {
            return;
        }
        let age = self.last_poll.elapsed();
        if poll_now || age >= CONFIG_POLL {
            self.last_poll = Instant::now();
            if self.watcher.poll() {
                self.reload_config();
            }
            ui.request_repaint_in(CONFIG_POLL);
        } else {
            ui.request_repaint_in(CONFIG_POLL - age);
        }
    }

    /// "N transfers running — Close anyway / Keep", over the tab strip.
    fn close_confirm(&mut self, ui: &mut Ui, strip: Rect, id: u64) {
        let n = match self.tabs.iter().find(|t| t.id == id).map(|t| &t.kind) {
            Some(TabKind::Files(f)) => f.transfers.running(),
            _ => 0,
        };
        if n == 0 {
            self.closing = None; // finished meanwhile: close as asked
            self.close_tab(id);
            return;
        }
        let p = self.theme.palette;
        ui.painter().rect(strip, p.surface, 0.0);
        let parts = ui.split_h(strip.inset(ui.px(4)), &[L::Fill, L::Px(120), L::Px(72)], 6);
        let text = format!("{n} transfer{} running \u{2014} close the tab?", if n == 1 { "" } else { "s" });
        label(ui, parts[0], &text, None);
        if button(ui, Id::new("close-anyway"), parts[1], "Close anyway").clicked {
            self.close_tab(id);
        } else if button(ui, Id::new("close-keep"), parts[2], "Keep").clicked {
            self.closing = None;
        }
    }

    fn status_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let p = self.theme.palette;
        ui.painter().rect(rect, p.surface, 0.0);
        let px = ui.font_px();
        let left = match self.tabs.get_mut(self.active).map(|t| &mut t.kind) {
            Some(TabKind::Terminal(t)) => t.status(),
            Some(TabKind::Files(f)) => format!("files: {} \u{2194} {}", f.panes[0].loc.label(), f.panes[1].loc.label()),
            Some(TabKind::Settings) => "settings".into(),
            Some(TabKind::Forwards) => {
                let n = self.forwards.running.iter().filter(|r| r.handle.state() == ForwardState::Running).count();
                format!("{n} forward{} running", if n == 1 { "" } else { "s" })
            }
            None => "no tabs".into(),
        };
        // The host under the mouse is described in words (not only by its dot).
        let hovered = self.sidebar.hovered.and_then(|i| self.hosts.get(i)).map(|h| {
            let s = crate::sidebar::describe(h, self.statuses.get(&h.name), &self.history);
            format!("{}: {s}", h.name)
        });
        let n = self.tabs.len();
        let right = format!("{n} tab{} \u{00B7} theme: {}", if n == 1 { "" } else { "s" }, self.theme.name);
        let pad = ui.px(10);
        let rw = ui.measure(&right, FontKind::Ui, px).ceil() as i32;
        let inner = rect.inset(0);
        let parts = ui.split_h(Rect::new(inner.x + pad, inner.y, inner.w - 2 * pad, inner.h), &[L::Frac(0.35), L::Fill, L::Px((rw as f32 / ui.scale()).ceil() as i32 + 2)], 12);
        label(ui, parts[0], &left, Some(p.muted));
        if let Some(h) = hovered {
            label(ui, parts[1], &h, Some(p.fg));
        } else if let Some(w) = self.warnings.last().cloned() {
            if let Some(n) = ui.a11y_node(Id::new("status-warning"), termax_ui::a11y::Role::Status, &w) {
                n.live = true;
            }
            label(ui, parts[1], &w, Some(p.warning));
        }
        label(ui, parts[2], &right, Some(p.muted));
    }
}

impl App for GuiApp {
    fn frame(&mut self, ctx: &mut FrameCtx) {
        let mut st = std::mem::take(&mut self.ui_state);
        let style = self.style.clone();
        Ui::run(&mut st, ctx, &style, |ui| self.ui(ui));
        self.ui_state = st;
        if let Some(f) = self.pending_fonts.take() {
            ctx.replace_fonts = Some(f);
        }
    }

    fn exit(&mut self) {
        self.forwards.stop_all();
        self.flush_geometry();
    }
}

/// Paths of the ssh config files in use (none when ssh config is off).
fn ssh_files(cfg: &Config) -> Vec<PathBuf> {
    match hosts::ssh_config_path(cfg) {
        Some(p) => ConfigSet::load(&p).map(|s| s.docs.into_iter().map(|d| d.path).collect()).unwrap_or_default(),
        None => vec![],
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
pub(crate) mod tests;
