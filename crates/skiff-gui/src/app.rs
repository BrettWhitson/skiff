//! The application: owns config, hosts, theme, tabs and the engine, and
//! implements the toolkit's `App` trait. Screens live in their own modules.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use skiff_core::config::{Config, FileWatcher, StatusChecks};
use skiff_core::forward::{Forward, ForwardHandle, ForwardState};
use skiff_core::history::{self, ConnKind, History};
use skiff_core::hostform::{self, Saved};
use skiff_core::hosts::{self, HostEntry};
use skiff_core::sshconfig::edit::{ConfigSet, backups_dir};
use skiff_core::state;
use skiff_core::status::{self, Status};
use skiff_core::theme::{ThemeDef, builtin};
use skiff_ui::geom::Rect;
use skiff_ui::icons::Icon;
use skiff_ui::input::Event;
use skiff_ui::layout::Size as L;
use skiff_ui::shell::{App, FrameCtx, Waker};
use skiff_ui::style::{Style, TextRole, tokens};
use skiff_ui::text::Fonts;
use skiff_ui::ui::{Id, Ui, UiState};
use skiff_ui::widgets::{
    Align, ButtonKind, Dialog, DialogButton, DialogResult, Elide, EmptyState, Status as Dot, TabTitle, TabsOpts, TextOpts, Toast,
    ToastEvent, ToastKind, Toasts, dialog, dialog_dismiss, empty_state, splitter_h, status_dot, tabs_with, text, toasts_in,
};

use crate::dock::{Dock, DockDrag, Drop, Zone};
use crate::engine::{Engine, Msg};
use crate::files::{FilesTab, Location};
use crate::forwards::{ForwardsState, Running};
use crate::keys::{Action, Keymap};
use crate::palette::{Cmd, Item, PaletteState};
use crate::sidebar::{Sidebar, SidebarAction, SidebarCtx, sidebar};
use crate::term::{TermTab, term_id};
use crate::theme::{GuiTheme, from_def};

/// Starts the askpass broker; its questions arrive as `Msg::Ask`.
pub fn start_broker(engine: &Engine, helper: PathBuf) -> anyhow::Result<skiff_core::askpass::Broker> {
    let (broker, mut rx) = skiff_core::askpass::Broker::start(engine.rt.handle(), helper)?;
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
/// Height of the status bar, logical px.
const STATUS_H: i32 = 22;
/// How many recent warnings `GuiApp::warnings` keeps.
const WARNINGS_KEPT: usize = 20;
/// The narrowest a dock group gets, logical px.
const MIN_GROUP_W: i32 = 200;
/// The least height below a dock group's tab strip, logical px.
const MIN_GROUP_H: i32 = 96;
/// How much a terminal in a group without focus is dimmed toward the
/// background (0..=1; `gui.dim_inactive_panes`).
const DIM: f32 = 0.15;

/// What one dock group is drawn with this frame.
struct GroupCtx<'a> {
    /// The group's id.
    id: u64,
    /// Its whole area (strip and content).
    rect: Rect,
    /// The Settings gear ends its strip (the top-right group).
    gear: bool,
    /// "Group 2 of 3" ("" with a single group).
    words: String,
    /// There are several groups.
    many: bool,
    /// Tabs shown last frame.
    prev_visible: &'a [u64],
}

/// What the groups asked for this frame, carried out after they are drawn.
#[derive(Default)]
struct GroupEvents {
    /// A tab dropped somewhere it changes something.
    drop: Option<(u64, Drop)>,
    /// Tabs whose close button was clicked.
    close: Vec<u64>,
    /// A tab chosen in a strip (or the group whose "+" was clicked).
    show: Option<u64>,
    /// A "+" was clicked: the palette opens.
    plus: bool,
    /// A press landed in this group.
    pressed: Option<u64>,
}

/// What a tab shows. Settings and Forwards are singletons; their state
/// lives on `GuiApp` so it survives closing the tab.
pub enum TabKind {
    /// A terminal tab: one session to a host.
    Terminal(Box<TermTab>),
    /// A two-pane file-transfer tab.
    Files(Box<FilesTab>),
    /// The settings screen.
    Settings,
    /// The port-forward manager.
    Forwards,
}

/// One entry in the tab strip.
pub struct Tab {
    /// Unique for the app's lifetime (never reused), so it can name a tab
    /// across frames and engine messages.
    pub id: u64,
    /// What the tab shows.
    pub kind: TabKind,
}

/// The whole application state, driven by the toolkit through `App`.
pub struct GuiApp {
    /// The live config (edited by settings, saved to `cfg_path`).
    pub cfg: Config,
    /// Where `config.toml` lives.
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
    /// When each host was last connected (for sorting and the status bar).
    pub history: History,
    /// Where `history` is saved.
    history_path: PathBuf,
    /// Terminals (by `TermTab::uid`) to record in history once they prove connected.
    pub(crate) pending_records: Vec<(u64, String, Instant)>,
    /// Port forwards: the running ones and the manager tab's form.
    pub forwards: ForwardsState,
    /// The active theme, mapped onto toolkit and terminal colours.
    pub theme: GuiTheme,
    /// The toolkit style (palette, zoom, font sizes) derived from theme and config.
    pub style: Style,
    /// Active key bindings (defaults plus `[keys]` overrides).
    pub keymap: Keymap,
    /// Geometry kept outside `config.toml`.
    pub geometry: state::UiState,
    /// Where `geometry` is saved (`state.toml`).
    pub geometry_path: PathBuf,
    /// When geometry first changed since the last save; saved after
    /// `GEOMETRY_SAVE_DELAY`.
    geometry_dirty: Option<Instant>,
    /// Recent warnings and errors (newest last, at most `WARNINGS_KEPT`),
    /// kept after their toasts are gone.
    pub warnings: Vec<String>,
    /// The `[keys]` problems last warned about: a save warns only when
    /// they change, so every settings change doesn't repeat them.
    keymap_warnings: Vec<String>,
    /// Notifications shown as toasts at the bottom right (see `notify`).
    pub toasts: Toasts,
    /// Background runtime; results come back as `Msg`s.
    pub engine: Engine,
    /// Open tabs (any order: the dock's groups hold their strip order).
    pub tabs: Vec<Tab>,
    /// Index into `tabs` of the focused tab (shown in the focused group).
    pub active: usize,
    /// The tab groups and their split layout (kept in step with `tabs` and
    /// `active` by `sync_dock`).
    pub dock: Dock,
    /// The id the next tab gets.
    next_tab_id: u64,
    /// Host sidebar state.
    pub sidebar: Sidebar,
    /// The command palette is open (its shortcuts, "+" or the empty state).
    pub palette_open: bool,
    /// The palette's query and selection.
    palette: PaletteState,
    /// The palette was open last frame (to detect it opening or closing).
    palette_was_open: bool,
    /// Where focus goes back to when the palette closes.
    palette_return_focus: Option<Id>,
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
    pub askpass: Option<skiff_core::askpass::Broker>,
    /// `ssh -V`, read when the first file tab opens.
    ssh_version: Option<Option<(u32, u32)>>,
    /// The focused tab and its group at the end of last frame (keyboard
    /// focus moves into a tab focused or moved some other way than by its
    /// strip).
    last_focus: Option<(u64, u64)>,
    /// Tabs shown in some group last frame (a terminal coming into view
    /// takes its screen as read).
    visible_tabs: Vec<u64>,
    /// A files tab whose close waits for "Close anyway" (transfers running).
    pub(crate) closing: Option<u64>,
    /// Form state of the settings tab (kept while the tab is closed).
    pub settings: crate::settings::SettingsState,
    /// Fonts to install after this frame (set by the settings screen).
    pub pending_fonts: Option<Fonts>,
    /// Notices outside edits of `config.toml`.
    watcher: FileWatcher,
    /// When `watcher` was last polled.
    last_poll: Instant,
    /// Whether the window has focus (config polling only runs then).
    window_focused: bool,
    /// The OS high-contrast mode and text size the theme and style are for.
    os_prefs: (bool, f32),
    /// The toolkit's retained per-widget state (focus, scroll, ...).
    ui_state: UiState,
    /// A drag-and-drop was in progress at the end of the last frame.
    drag_live: bool,
}

/// The theme named in the config, falling back to the first built-in.
pub fn find_theme(cfg: &Config) -> ThemeDef {
    cfg.themes
        .iter()
        .chain(builtin().iter())
        .find(|t| t.name == skiff_core::theme::effective_name(&cfg.settings.theme, crate::os::high_contrast()))
        .cloned()
        .unwrap_or_else(|| builtin().remove(0))
}

/// The configured local start directory, else the home directory.
pub fn local_start_dir(configured: &Option<String>) -> String {
    configured
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(skiff_core::sshconfig::expand_tilde)
        .or_else(skiff_core::sshconfig::home_dir)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into())
}

/// The toolkit style for a theme: its palette, the interface zoom, the
/// terminal font size from the config, and the OS animation setting.
pub fn style_for(theme: &GuiTheme, cfg: &Config) -> Style {
    let mut s = Style::dark_default();
    s.palette = theme.palette;
    s.reduce_motion = !crate::os::animations();
    // `ui_scale` on top of Windows' text size (Settings > Accessibility).
    s.zoom = cfg.gui.ui_scale.clamp(0.5, 3.0) * crate::os::text_scale();
    // The interface zoom leaves terminal text at its configured size.
    s.mono_px = cfg.gui.font_size.clamp(6.0, 72.0) / s.zoom;
    s
}

impl GuiApp {
    /// Builds the app from a loaded config: hosts, theme, keymap, saved
    /// geometry and history, the engine and (with a window) the askpass
    /// broker. `waker` is `None` in tests, which run without a window.
    pub fn new(cfg: Config, cfg_path: PathBuf, waker: Option<Waker>) -> GuiApp {
        let wake = waker.map(|w| Arc::new(move || w.wake()) as crate::engine::WakeFn);
        let (hosts, host_warning) = hosts::collect(&cfg);
        let theme = from_def(&find_theme(&cfg));
        let style = style_for(&theme, &cfg);
        let (keymap, mut warnings) = Keymap::platform(&cfg.keys);
        let keymap_warnings = warnings.clone();
        warnings.extend(host_warning);
        warnings.extend(theme.warning.clone());
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
            warnings: vec![],
            keymap_warnings,
            toasts: Toasts::default(),
            engine,
            tabs: vec![],
            active: 0,
            dock: Dock::default(),
            next_tab_id: 1,
            sidebar: Sidebar::default(),
            palette_open: false,
            palette: PaletteState::default(),
            palette_was_open: false,
            palette_return_focus: None,
            new_connection: false,
            view_actions: vec![],
            edit_target: None,
            open: None,
            start_dir: local_start_dir(&cfg_start),
            askpass,
            ssh_version: None,
            last_focus: None,
            visible_tabs: vec![],
            closing: None,
            settings: Default::default(),
            pending_fonts: None,
            watcher: FileWatcher::new(&cfg_path_for_watch),
            last_poll: Instant::now(),
            window_focused: true,
            os_prefs: (crate::os::high_contrast(), crate::os::text_scale()),
            ui_state: UiState::default(),
            drag_live: false,
        };
        for w in warnings {
            app.warn(w);
        }
        // Opt-in status checks of every host at startup.
        if on_start {
            app.check_status(None);
        }
        app
    }

    /// Shows `text` as a toast of `kind` (info and success fade after 4 s,
    /// warnings after 8 s, errors stay until closed). The same message
    /// again replaces its earlier toast, so it comes back to the bottom and
    /// is read out again rather than stacking copies. Warnings and errors
    /// are also kept in `warnings`.
    pub fn notify(&mut self, kind: ToastKind, text: impl Into<String>) {
        let text = text.into();
        let same: Vec<u64> = self.toasts.visible().filter(|(_, t)| t.kind == kind && t.text == text).map(|(id, _)| id).collect();
        for id in same {
            self.toasts.dismiss(id);
        }
        if matches!(kind, ToastKind::Warning | ToastKind::Error) {
            self.warnings.push(text.clone());
            if self.warnings.len() > WARNINGS_KEPT {
                self.warnings.remove(0);
            }
        }
        self.toasts.push(Toast::new(kind, text));
    }

    /// A warning toast (see `notify`).
    pub fn warn(&mut self, w: impl Into<String>) {
        self.notify(ToastKind::Warning, w);
    }

    /// An error toast, which stays until closed (see `notify`).
    pub fn error(&mut self, e: impl Into<String>) {
        self.notify(ToastKind::Error, e);
    }

    // ----- tabs -------------------------------------------------------

    /// Adds a tab with a fresh id and makes it the focused tab (it joins
    /// the focused group, at the end of its strip); returns the id.
    pub(crate) fn push_tab(&mut self, kind: TabKind) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab { id, kind });
        self.active = self.tabs.len() - 1;
        id
    }

    /// Opens a terminal tab to `host`; history records it once it proves
    /// connected (see `pending_records`).
    pub fn open_terminal(&mut self, host: &HostEntry) -> u64 {
        let term = TermTab::new(host.clone(), &self.cfg.settings.ssh_binary, self.cfg.settings.scrollback);
        self.pending_records.push((term.uid, host.name.clone(), Instant::now()));
        self.push_tab(TabKind::Terminal(Box::new(term)))
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

    /// Opens a file-transfer tab with the given pane locations. The first one
    /// also reads the local `ssh -V`.
    pub fn open_files(&mut self, left: Location, right: Location) -> u64 {
        if self.ssh_version.is_none() {
            self.ssh_version = Some(skiff_core::sshver::client_version(&self.cfg.settings.ssh_binary).ok());
        }
        let mut tab = FilesTab::new(left, right);
        for p in &mut tab.panes {
            p.show_hidden = self.cfg.settings.show_hidden_files;
        }
        self.push_tab(TabKind::Files(Box::new(tab)))
    }

    /// Shows the settings tab (opening it if needed). A pending
    /// `edit_target` or `new_connection` first points the Connections
    /// section at that host or at a new form.
    pub fn open_settings(&mut self) {
        // The connection form needs `&mut self`, so the settings state is
        // taken out while it switches.
        if let Some(name) = self.edit_target.take() {
            let mut st = std::mem::take(&mut self.settings);
            st.section = crate::settings::Section::Connections;
            st.conn.switch(Some(name), self);
            self.settings = st;
        } else if std::mem::take(&mut self.new_connection) {
            let mut st = std::mem::take(&mut self.settings);
            st.section = crate::settings::Section::Connections;
            st.conn.switch(None, self);
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
        // Running transfers: show the question on that tab instead of closing.
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
        if let Some(i) = self.tab_index(id) {
            self.sync_dock();
            let focused = self.active_tab_id();
            // The tab shown in its place: its strip neighbor, or the group
            // that takes its group's space.
            let next = self.dock.remove_tab(id);
            let mut t = self.tabs.remove(i);
            if let TabKind::Files(f) = &mut t.kind {
                f.shutdown(self.askpass.as_ref().map(|b| (b, id)));
            }
            drop(t);
            let keep = if focused == Some(id) { next.or_else(|| self.dock.focused().map(|g| g.active)) } else { focused };
            self.active = keep.and_then(|k| self.tab_index(k)).unwrap_or(0);
            self.sync_dock();
        }
    }

    /// Index into `tabs` of tab `id`.
    pub fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Makes tab `id` the focused tab (its group is focused next sync).
    pub fn show_tab(&mut self, id: u64) {
        if let Some(i) = self.tab_index(id) {
            self.active = i;
        }
    }

    /// Brings the dock in line with the open tabs and the focused one.
    pub fn sync_dock(&mut self) {
        let ids: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        let active = self.active_tab_id();
        self.dock.sync(&ids, active);
    }

    /// The tab strip label for tab `i` (terminals show the host).
    pub fn tab_title(&self, i: usize) -> String {
        match &self.tabs[i].kind {
            TabKind::Terminal(t) => t.host.name.clone(),
            // The strip draws its own icon in place of the title's arrow.
            TabKind::Files(f) => f.title().trim_start_matches("\u{21C5} ").to_string(),
            TabKind::Settings => "Settings".into(),
            TabKind::Forwards => "Forwards".into(),
        }
    }

    /// What the strip draws before tab `i`'s title: a terminal's connection
    /// dot (with its words), or the icon of the other kinds.
    fn tab_lead(&mut self, i: usize) -> (Option<(Dot, &'static str)>, Option<Icon>) {
        match &mut self.tabs[i].kind {
            TabKind::Terminal(t) => (Some(t.conn_state()), None),
            TabKind::Files(_) => (None, Some(Icon::Transfer)),
            TabKind::Settings => (None, Some(Icon::Settings)),
            TabKind::Forwards => (None, Some(Icon::Forward)),
        }
    }

    /// The id of the shown tab, if any tab is open.
    pub fn active_tab_id(&self) -> Option<u64> {
        self.tabs.get(self.active).map(|t| t.id)
    }

    // ----- config -----------------------------------------------------

    /// Writes changed settings (comments and other fields kept) and
    /// rebuilds what depends on them.
    pub fn save_config(&mut self) {
        match self.cfg.save(&self.baseline, &self.cfg_path) {
            Ok(()) => self.baseline = self.cfg.clone(),
            Err(e) => self.error(format!("could not save config: {e}")),
        }
        self.watcher.poll(); // our own write is not an outside edit
        let (km, warnings) = Keymap::platform(&self.cfg.keys);
        self.keymap = km;
        let new: Vec<String> = warnings.iter().filter(|w| !self.keymap_warnings.contains(w)).cloned().collect();
        self.keymap_warnings = warnings;
        for w in new {
            self.warn(w);
        }
        self.rebuild_hosts();
    }

    /// Re-collects the host list from config and ssh config, bumping
    /// `hosts_gen` and refreshing the list of ssh config files.
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
            Saved::Skiff(_) => self.save_config(),
        }
        Ok(())
    }

    /// The listed host named `name`.
    fn host_named(&self, name: &str) -> Result<HostEntry, String> {
        self.hosts.iter().find(|h| h.name == name).cloned().ok_or_else(|| format!("{name} is gone"))
    }

    /// Adds a copy of host `name` (named `<name>-copy`) next to it; returns its name.
    pub fn duplicate_host(&mut self, name: &str) -> Result<String, String> {
        let h = self.host_named(name)?;
        let mut set = self.load_set();
        let taken = hostform::taken_names(&self.hosts);
        let form = hostform::duplicate(&h, &self.cfg, &set, &taken)?;
        let saved = form.save(&mut self.cfg, &mut set, &taken)?;
        let name = saved.name().to_string();
        self.apply_saved(saved, &mut set)?;
        Ok(name)
    }

    /// Removes host `name` from wherever it is defined.
    pub fn delete_host(&mut self, name: &str) -> Result<(), String> {
        let h = self.host_named(name)?;
        let mut set = self.load_set();
        let saved = hostform::delete(&h, &mut self.cfg, &mut set)?;
        self.apply_saved(saved, &mut set)
    }

    /// Hides or unhides host `name` and saves the change.
    fn set_host_hidden(&mut self, name: &str, hidden: bool) -> Result<(), String> {
        let h = self.host_named(name)?;
        let mut set = self.load_set();
        let saved = hostform::set_hidden(&h, hidden, &mut self.cfg, &mut set)?;
        self.apply_saved(saved, &mut set)
    }

    /// Moves an ssh config host into the included config file `file`.
    fn move_host(&mut self, name: &str, file: &std::path::Path) -> Result<(), String> {
        let h = self.host_named(name)?;
        let mut set = self.load_set();
        let doc = set.docs.iter().position(|d| d.path == file).ok_or_else(|| format!("{} is no longer included", file.display()))?;
        set.move_host(&h.name, doc)?;
        self.apply_saved(Saved::Ssh(h.name), &mut set)
    }

    /// Sets whether the sidebar lists hidden hosts (kept in `state.toml`).
    pub fn set_show_hidden(&mut self, show: bool) {
        self.geometry.show_hidden = show;
        self.geometry_changed();
    }

    /// Empties the connection history on disk and in memory.
    pub fn clear_history(&mut self) {
        match self.history.update(&self.history_path, History::clear) {
            Ok(()) => self.notify(ToastKind::Success, "History cleared"),
            Err(e) => self.error(format!("could not save history: {e:#}")),
        }
    }

    /// Records a connection to host `name` now, saving the history file.
    pub fn record(&mut self, name: &str, kind: ConnKind) {
        if let Err(e) = self.history.update(&self.history_path, |h| h.record(name, kind, history::now())) {
            self.error(format!("could not save history: {e:#}"));
        }
    }

    /// After host `old` was renamed to `new`: its saved and running
    /// forwards, history and status follow it.
    pub fn after_rename(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        if hostform::rename_forwards(&mut self.cfg, old, new) {
            self.save_config();
        }
        for r in self.forwards.running.iter_mut().filter(|r| r.fwd.host == old) {
            r.fwd.host = new.to_string();
        }
        if let Err(e) = self.history.update(&self.history_path, |h| h.rename(old, new)) {
            self.error(format!("could not save history: {e:#}"));
        }
        if let Some(s) = self.statuses.remove(old) {
            self.statuses.insert(new.to_string(), s);
        }
    }

    /// Checks host `only`, or every listed host. Never runs on its own
    /// unless `status_checks = "on_start"`.
    pub fn check_status(&mut self, only: Option<&str>) {
        let sets = hosts::probe_sets(&self.cfg);
        let idx: Vec<usize> = match only {
            Some(n) => self.hosts.iter().position(|h| h.name == n).into_iter().collect(),
            None => hosts::visible(&self.hosts, self.geometry.show_hidden),
        };
        let items: Vec<(String, hosts::Probe)> =
            idx.into_iter().filter_map(|i| self.hosts.get(i)).map(|h| (h.name.clone(), h.probe(sets.as_deref()))).collect();
        // Show "checking" right away; results arrive as `Msg::HostStatus`.
        for (n, _) in &items {
            self.statuses.insert(n.clone(), Status::Checking);
        }
        let tx = self.engine.sender();
        status::check_all(
            self.engine.rt.handle(),
            items,
            Arc::new(move |host, status| {
                tx.send(Msg::HostStatus { host, status });
            }),
        );
    }

    /// The sidebar's selected host.
    pub fn sidebar_selected(&self) -> Option<String> {
        self.sidebar.selected_host(&self.sidebar_ctx())
    }

    /// What the sidebar needs to draw, borrowed from the app.
    fn sidebar_ctx(&self) -> SidebarCtx<'_> {
        SidebarCtx {
            hosts: &self.hosts,
            statuses: &self.statuses,
            history: &self.history,
            sort: self.geometry.sort,
            show_hidden: self.geometry.show_hidden,
            files: &self.ssh_files,
            ssh_config_on: self.cfg.settings.load_ssh_config,
            palette_key: self.keymap.hint(Action::QuickSwitcher, self.in_terminal()).map(|c| c.to_string()),
        }
    }

    /// Starts forward `f` in the background; prompts arrive in the Forwards tab.
    pub fn start_forward(&mut self, st: &mut ForwardsState, f: &Forward) {
        let Some(host) = self.hosts.iter().find(|h| h.name == f.host).cloned() else {
            self.warn(format!("unknown host: {}", f.host));
            return;
        };
        // Restarting: stop any earlier run of the same forward and drop dead entries.
        st.stop(f);
        st.running.retain(|r| &r.fwd != f || r.handle.state().is_live());
        if self.ssh_version.is_none() {
            self.ssh_version = Some(skiff_core::sshver::client_version(&self.cfg.settings.ssh_binary).ok());
        }
        // Prompts need the askpass broker and an ssh new enough to honour it;
        // otherwise run in BatchMode so ssh fails instead of waiting for input.
        let can_prompt = self.askpass.is_some() && self.ssh_version.flatten().is_none_or(skiff_core::sshver::supports_askpass_require);
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
        // By terminal, not tab id (a tab's terminal is found by its uid).
        self.pending_records.retain(|(uid, name, at)| {
            let Some(t) = self.tabs.iter_mut().find_map(|tab| match &mut tab.kind {
                TabKind::Terminal(t) if t.uid == *uid => Some(t),
                _ => None,
            }) else {
                return false;
            };
            // After 2 s with output: record if still alive, stop tracking either
            // way. Before that, keep waiting unless the terminal has an error.
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
        // Forwards are recorded once, the first time they reach Running.
        for r in &mut self.forwards.running {
            if !r.recorded && r.handle.state() == ForwardState::Running {
                r.recorded = true;
                done.push((r.fwd.host.clone(), ConnKind::Forward));
            }
        }
        for (name, kind) in done {
            self.record(&name, kind);
        }
        self.forwards.prune(&self.cfg.forwards);
    }

    /// Carries out what the user asked for in the sidebar; failures become
    /// status-bar warnings.
    fn on_sidebar(&mut self, a: SidebarAction) {
        match a {
            SidebarAction::OpenTerminal(h) => {
                if let Ok(host) = self.host_named(&h) {
                    self.open_terminal(&host);
                }
            }
            SidebarAction::OpenFiles(h) => {
                if let Ok(host) = self.host_named(&h) {
                    self.open_files(Location::Local, Location::Host(Box::new(host)));
                }
            }
            SidebarAction::Edit(h) => {
                self.edit_target = Some(h);
                self.open_settings();
            }
            SidebarAction::Duplicate(h) => {
                if let Err(e) = self.duplicate_host(&h) {
                    self.error(e);
                }
            }
            SidebarAction::Delete(h) => {
                if let Err(e) = self.delete_host(&h) {
                    self.error(e);
                }
            }
            SidebarAction::SetHidden(h, hidden) => {
                if let Err(e) = self.set_host_hidden(&h, hidden) {
                    self.error(e);
                }
            }
            SidebarAction::MoveTo(h, file) => {
                if let Err(e) = self.move_host(&h, &file) {
                    self.error(e);
                }
            }
            SidebarAction::Forward(h) => {
                self.forwards.new_form(&h);
                self.open_forwards();
            }
            SidebarAction::Check(h) => self.check_status(Some(&h)),
            SidebarAction::CheckAll => self.check_status(None),
            SidebarAction::SetSort(s) => {
                self.geometry.sort = s;
                self.geometry_changed();
            }
            SidebarAction::ToggleShowHidden => self.set_show_hidden(!self.geometry.show_hidden),
            SidebarAction::OpenForwards => self.open_forwards(),
            SidebarAction::OpenLocalFiles => {
                self.open_files(Location::Local, Location::Local);
            }
            SidebarAction::ImportSshConfig => {
                self.cfg.settings.load_ssh_config = true;
                self.save_config();
                let n = self.hosts.len();
                self.notify(ToastKind::Success, format!("Listing ~/.ssh/config: {n} host{}", if n == 1 { "" } else { "s" }));
            }
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
        self.pick_theme();
        self.save_config();
    }

    /// Re-picks the configured theme (for the OS high-contrast mode now).
    fn pick_theme(&mut self) {
        self.os_prefs = (crate::os::high_contrast(), crate::os::text_scale());
        let theme = from_def(&find_theme(&self.cfg));
        // Once per change, not on every re-pick of the same theme.
        if let Some(w) = theme.warning.clone().filter(|w| self.theme.warning.as_ref() != Some(w)) {
            self.warn(w);
        }
        self.theme = theme;
        self.style = style_for(&self.theme, &self.cfg);
    }

    /// Re-reads `config.toml` (after an outside edit, or on request).
    pub fn reload_config(&mut self) {
        match Config::load_or_create(&self.cfg_path) {
            Ok(cfg) => {
                self.baseline = cfg.clone();
                self.cfg = cfg;
                self.pick_theme();
                let (km, warnings) = Keymap::platform(&self.cfg.keys);
                self.keymap = km;
                for w in &warnings {
                    self.warn(w.clone());
                }
                self.keymap_warnings = warnings;
                self.rebuild_hosts();
            }
            Err(e) => self.error(format!("config not reloaded: {e:#}")),
        }
    }

    /// Built-in theme names, then config themes that don't reuse one.
    pub fn theme_names(&self) -> Vec<String> {
        let mut names: Vec<String> = builtin().into_iter().map(|t| t.name).collect();
        for t in &self.cfg.themes {
            if !names.contains(&t.name) {
                names.push(t.name.clone());
            }
        }
        names
    }

    /// Carries out the item chosen in the command palette.
    fn on_palette(&mut self, item: Item) {
        match item {
            Item::Host(i) => {
                if let Some(h) = self.hosts.get(i).cloned() {
                    self.open_terminal(&h);
                }
            }
            Item::HostFiles(i) => {
                if let Some(h) = self.hosts.get(i).cloned() {
                    self.open_files(Location::Local, Location::Host(Box::new(h)));
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
            Item::Command(Cmd::Forwards) => self.open_forwards(),
            Item::Command(Cmd::CheckStatus) => self.check_status(None),
            Item::Command(Cmd::ReloadConfig) => self.reload_config(),
            Item::Command(Cmd::Action(a)) => {
                self.do_action(a);
            }
            // A saved forward starts, or stops when it runs.
            Item::Forward(i) => {
                let Some(f) = self.cfg.forwards.get(i).cloned() else { return };
                let mut st = std::mem::take(&mut self.forwards);
                if st.running.iter().any(|r| r.fwd == f && r.handle.state().is_live()) {
                    st.stop(&f);
                } else {
                    self.start_forward(&mut st, &f);
                }
                self.forwards = st;
            }
            // Snippets go to the focused terminal.
            Item::Snippet(i) => {
                let Some(snippet) = self.cfg.snippets.get(i).cloned() else {
                    return;
                };
                if let Some(TabKind::Terminal(t)) = self.tabs.get_mut(self.active).map(|t| &mut t.kind) {
                    t.send_snippet(&snippet);
                }
            }
        }
    }

    /// Draws the command palette while open: resets it and remembers focus
    /// when it opens, and gives focus back when it closes.
    fn palette_ui(&mut self, ui: &mut Ui) {
        if self.palette_open && !self.palette_was_open {
            self.palette = PaletteState { just_opened: true, ..Default::default() };
            self.palette_return_focus = ui.focused();
        }
        self.palette_was_open = self.palette_open;
        if !self.palette_open {
            return;
        }
        // Candidates: open tabs, hosts, commands (with their shortcuts),
        // saved forwards, themes, and the snippets for the active terminal.
        let tabs = (0..self.tabs.len())
            .map(|i| {
                let icon = match &self.tabs[i].kind {
                    TabKind::Terminal(_) => Icon::Terminal,
                    TabKind::Files(_) => Icon::Transfer,
                    TabKind::Settings => Icon::Settings,
                    TabKind::Forwards => Icon::Forward,
                };
                (self.tabs[i].id, self.tab_title(i), icon)
            })
            .collect();
        // Shortcuts as they work from the active tab (a terminal keeps plain Ctrl+letter).
        let in_term = self.in_terminal();
        let shortcuts = Cmd::ALL
            .into_iter()
            .filter_map(|c| match c {
                Cmd::Action(a) => self.keymap.hint(a, in_term).map(|k| (c, k.to_string())),
                _ => None,
            })
            .collect();
        let forwards = self
            .cfg
            .forwards
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let live = self.forwards.running.iter().any(|r| &r.fwd == f && r.handle.state().is_live());
                (i, f.title().trim_start_matches("\u{21C4} ").to_string(), live)
            })
            .collect();
        let src = crate::palette::Sources {
            hosts: &self.hosts,
            show_hidden: self.geometry.show_hidden,
            recent: self.history.recent(crate::palette::RECENT_HOSTS),
            tabs,
            themes: self.theme_names(),
            forwards,
            shortcuts,
            snippets: match self.tabs.get(self.active).map(|t| &t.kind) {
                Some(TabKind::Terminal(t)) => self
                    .cfg
                    .snippets
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.host.as_deref().is_none_or(|h| h == t.host.name))
                    .map(|(i, s)| (i, s.name.clone()))
                    .collect(),
                _ => vec![],
            },
        };
        let entries = crate::palette::rank(&self.palette.query, &src);
        let out = crate::palette::show(ui, &mut self.palette, &entries);
        if out.close {
            self.palette_open = false;
            self.palette_was_open = false;
            if let Some(f) = self.palette_return_focus.take() {
                ui.request_focus(f);
            }
        }
        if let Some(item) = out.chosen {
            self.on_palette(item);
        }
    }

    // ----- geometry ---------------------------------------------------

    /// Marks `state.toml` as needing a save; the timer starts at the first
    /// change, so a burst of changes saves once.
    fn geometry_changed(&mut self) {
        self.geometry_dirty.get_or_insert_with(Instant::now);
    }

    /// Writes `state.toml` now if anything changed.
    pub fn flush_geometry(&mut self) {
        if self.geometry_dirty.take().is_some()
            && let Err(e) = self.geometry.save(&self.geometry_path)
        {
            self.error(format!("could not save window state: {e}"));
        }
    }

    // ----- actions ----------------------------------------------------

    /// Whether the active tab is a terminal (shell-owned chords then go to it).
    fn in_terminal(&self) -> bool {
        matches!(self.tabs.get(self.active).map(|t| &t.kind), Some(TabKind::Terminal(_)))
    }

    /// Runs an app action. Copy, paste, find and zoom are queued in
    /// `view_actions` for the active view to handle. False when it doesn't
    /// apply (a pane move, resize or equalize with nothing to change), so a
    /// shortcut's key can go to the shell instead.
    pub fn do_action(&mut self, a: Action) -> bool {
        match a {
            Action::QuickSwitcher | Action::NewTab => self.palette_open = true,
            Action::CloseTab => {
                if let Some(id) = self.active_tab_id() {
                    self.request_close_tab(id);
                }
            }
            Action::NextTab | Action::PrevTab => return self.cycle_tab(a == Action::NextTab),
            Action::ToggleSidebar => {
                self.geometry.sidebar_collapsed = !self.geometry.sidebar_collapsed;
                self.geometry_changed();
            }
            Action::Settings => self.open_settings(),
            Action::UiScaleUp | Action::UiScaleDown | Action::UiScaleReset => {
                // Steps of 0.1, rounded to one decimal so repeats don't drift.
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
            Action::SplitRight
            | Action::SplitDown
            | Action::FocusNextPane
            | Action::ClosePane
            | Action::MovePaneLeft
            | Action::MovePaneRight
            | Action::MovePaneUp
            | Action::MovePaneDown
            | Action::MoveToNewGroupRight
            | Action::MoveToNewGroupDown
            | Action::FocusGroupLeft
            | Action::FocusGroupRight
            | Action::FocusGroupUp
            | Action::FocusGroupDown
            | Action::ResizePaneLeft
            | Action::ResizePaneRight
            | Action::ResizePaneUp
            | Action::ResizePaneDown
            | Action::EqualizePanes => return self.group_action(a),
            Action::Copy | Action::Paste | Action::Find | Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => self.view_actions.push(a),
        }
        true
    }

    /// Shows the next (or previous) tab of the focused group, wrapping;
    /// false when it has a single tab (or none).
    fn cycle_tab(&mut self, forward: bool) -> bool {
        self.sync_dock();
        let Some(g) = self.dock.focused() else { return false };
        let n = g.tabs.len();
        if n < 2 {
            return false;
        }
        let i = g.tabs.iter().position(|t| *t == g.active).unwrap_or(0);
        let next = g.tabs[if forward { (i + 1) % n } else { (i + n - 1) % n }];
        self.show_tab(next);
        true
    }

    /// The dock actions (split, close, focus, move, resize, equalize) on
    /// the focused tab and group. False when one does not apply (no group
    /// that way, a lone tab already at that edge, nothing to equalize), so
    /// its key can go to the shell instead.
    fn group_action(&mut self, a: Action) -> bool {
        self.sync_dock();
        let Some(tab) = self.active_tab_id() else { return false };
        let side = |a| match a {
            Action::MovePaneLeft | Action::FocusGroupLeft | Action::ResizePaneLeft => Zone::Left,
            Action::MovePaneRight
            | Action::FocusGroupRight
            | Action::ResizePaneRight
            | Action::SplitRight
            | Action::MoveToNewGroupRight => Zone::Right,
            Action::MovePaneUp | Action::FocusGroupUp | Action::ResizePaneUp => Zone::Top,
            _ => Zone::Bottom,
        };
        match a {
            Action::SplitRight | Action::SplitDown => self.split_focused(side(a)),
            Action::FocusNextPane => self.focus_group(|d| d.focus_next(true)),
            Action::FocusGroupLeft | Action::FocusGroupRight | Action::FocusGroupUp | Action::FocusGroupDown => {
                self.focus_group(|d| d.focus_toward(side(a)))
            }
            Action::ClosePane => self.close_group(),
            Action::MovePaneLeft | Action::MovePaneRight | Action::MovePaneUp | Action::MovePaneDown => {
                self.dock.move_target(tab, side(a)).is_some_and(|to| self.move_tab(tab, to))
            }
            Action::MoveToNewGroupRight | Action::MoveToNewGroupDown => {
                let to = Drop::Beside { group: self.dock.focus, zone: side(a) };
                self.move_tab(tab, to)
            }
            Action::ResizePaneLeft | Action::ResizePaneRight | Action::ResizePaneUp | Action::ResizePaneDown => {
                self.dock.resize_focused(side(a))
            }
            Action::EqualizePanes => self.dock.equalize(),
            _ => false,
        }
    }

    /// Splits the focused tab towards `zone`: a terminal opens a new session
    /// to the same host, and a file transfer a new tab on the same two
    /// places, in a new group on that side; Settings and Forwards (one of
    /// each) move there instead, when their group has other tabs.
    fn split_focused(&mut self, zone: Zone) -> bool {
        let (Some(tab), group) = (self.active_tab_id(), self.dock.focus) else { return false };
        let new = match &self.tabs[self.active].kind {
            TabKind::Terminal(t) => {
                let s = t.sibling();
                self.push_tab(TabKind::Terminal(Box::new(s)))
            }
            TabKind::Files(f) => {
                let (l, r) = (f.panes[0].loc.clone(), f.panes[1].loc.clone());
                self.open_files(l, r)
            }
            TabKind::Settings | TabKind::Forwards => tab,
        };
        if self.move_tab(new, Drop::Beside { group, zone }) {
            return true;
        }
        // A new tab that could not be placed stays where it joined.
        new != tab
    }

    /// Moves tab `tab` as `to` says (a drop or a keyboard move): it shows,
    /// its group gets focus, and screen readers hear where it went. False
    /// when nothing would change.
    pub fn move_tab(&mut self, tab: u64, to: Drop) -> bool {
        self.sync_dock();
        let Some(group) = self.dock.apply(tab, to) else { return false };
        self.show_tab(tab);
        let title = self.tab_index(tab).map(|i| self.tab_title(i)).unwrap_or_default();
        let place = match to {
            Drop::Beside { zone, .. } => {
                let side = match zone {
                    Zone::Left => "on the left",
                    Zone::Right => "on the right",
                    Zone::Top => "above",
                    Zone::Bottom | Zone::Center => "below",
                };
                format!("moved to a new group {side}")
            }
            Drop::Into { .. } => match self.dock.position_words(group) {
                w if w.is_empty() => "moved".to_string(),
                w => format!("moved to {}", w.to_lowercase()),
            },
        };
        self.announce(format!("{title} {place}"));
        true
    }

    /// Focuses another group (`f` picks it on the dock; false if none):
    /// its shown tab becomes the focused tab, and it is announced.
    fn focus_group(&mut self, f: impl FnOnce(&mut Dock) -> bool) -> bool {
        if !f(&mut self.dock) {
            return false;
        }
        let Some(g) = self.dock.focused() else { return false };
        let (id, tab) = (g.id, g.active);
        self.show_tab(tab);
        let title = self.tab_index(tab).map(|i| self.tab_title(i)).unwrap_or_default();
        let words = self.dock.position_words(id);
        self.announce(format!("{words}: {title}"));
        true
    }

    /// Closes the focused group's tabs (each asks first if it must). With a
    /// single group only the focused tab closes, so the window never
    /// empties by surprise.
    fn close_group(&mut self) -> bool {
        let Some(g) = self.dock.focused() else { return false };
        let tabs = if self.dock.groups.len() > 1 { g.tabs.clone() } else { vec![g.active] };
        for id in tabs {
            self.request_close_tab(id);
        }
        self.sync_dock();
        let n = self.dock.groups.len();
        self.announce(format!("Group closed, {n} group{} left", if n == 1 { "" } else { "s" }));
        true
    }

    /// Says `text` through the dock's live region (screen readers).
    fn announce(&mut self, text: String) {
        self.dock.announce = skiff_ui::a11y::fresh_announcement(&self.dock.announce, format!("{text}."));
    }

    /// Runs the app shortcuts in this frame's input, taking out the keys of
    /// those that applied. A modal dialog (or the palette) owns the
    /// keyboard: shortcuts stand aside until it is answered. So does an
    /// open context menu, whose own item shortcuts then act (and close it).
    fn handle_shortcuts(&mut self, ui: &mut Ui) {
        if ui.modal_open() || skiff_ui::widgets::menu_holds_keyboard(ui) {
            return;
        }
        let in_term = self.in_terminal();
        let hits: Vec<_> = ui
            .input()
            .events
            .iter()
            .filter_map(|e| match e {
                // AltGr typing is text, never a shortcut.
                Event::KeyDown { mods, text, .. } if crate::keys::altgr_text(*mods, text.as_deref()) => None,
                Event::KeyDown { key, mods, .. } => self.keymap.match_key(key, *mods, in_term).map(|a| (key.clone(), *mods, a)),
                _ => None,
            })
            .collect();
        // An action that doesn't apply leaves its key for the terminal.
        for (key, mods, a) in hits {
            if self.do_action(a) {
                ui.input_mut().take_key(&key, mods);
            }
        }
    }

    /// Applies everything the engine sent since last frame; true if there
    /// was anything (so another frame is needed).
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
                Msg::Notice { text, .. } => self.notify(ToastKind::Info, text),
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
                    let mut notices = vec![];
                    if let Some(TabKind::Files(f)) = self.tabs.iter_mut().find(|t| t.id == tab).map(|t| &mut t.kind) {
                        f.on_transfer(ev, &ctx);
                        notices = f.take_notices();
                    }
                    // A finished copy batch becomes a toast.
                    for (kind, text) in notices {
                        self.notify(kind, text);
                    }
                }
                // Password / host-key questions: from a forward still running go
                // to the Forwards tab; from a file pane go to that pane; others
                // are refused.
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
                    // A successful SFTP connect counts as a files connection in history.
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
        // A host named on the command line opens once, on the first frame.
        if let Some((name, files)) = self.open.take() {
            match self.hosts.iter().find(|h| h.name == name).cloned() {
                Some(h) if files => {
                    self.open_files(Location::Local, Location::Host(Box::new(h)));
                }
                Some(h) => {
                    self.open_terminal(&h);
                }
                None => self.warn(format!("unknown host: {name}")),
            }
        }
        // Esc ended a drag (the toolkit drops it): it goes nowhere else.
        if std::mem::take(&mut self.drag_live) {
            ui.input_mut().take_key(&skiff_ui::input::Key::Escape, skiff_ui::input::Modifiers::NONE);
        }
        // Shortcuts are skipped while the settings tab records a new chord.
        if !self.settings.capturing() || !self.tabs.get(self.active).is_some_and(|t| matches!(t.kind, TabKind::Settings)) {
            self.handle_shortcuts(ui);
        }
        self.track_window_size(ui);
        self.record_connections();

        // Layout: body (sidebar + main area) over a status bar.
        let win = ui.window_rect();
        let p = self.theme.palette;
        ui.painter().rect(win, p.bg, 0.0);
        let parts = ui.split_v(win, &[L::Fill, L::Px(STATUS_H)], 0);
        let (body, status) = (parts[0], parts[1]);

        let main = if self.geometry.sidebar_collapsed {
            body
        } else {
            let before = self.geometry.sidebar_width;
            // The divider is its own F6 stop (just before Hosts when cycling
            // round), so the keyboard can resize the sidebar.
            let width = &mut self.geometry.sidebar_width;
            let (side, main) = ui.region(Id::new("region-sidebar-divider"), "Sidebar divider", |ui| {
                splitter_h(ui, Id::new("sidebar-split"), body, width, 160.0, 600.0, "Sidebar width")
            });
            if self.geometry.sidebar_width != before {
                self.geometry_changed();
            }
            // The sidebar borrows the app through its ctx, so its own state is
            // taken out for the call.
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
        self.palette_ui(ui);
        // Toasts paint last, over everything (dialogs included).
        // Toasts stack above the status bar.
        // On a files tab they stack above its transfer drawer, not over its rows.
        let mut toast_area = body;
        if let Some(TabKind::Files(f)) = self.tabs.get(self.active).map(|t| &t.kind)
            && let Some(top) = f.drawer_top
            && top > body.y
        {
            toast_area.h = top - body.y;
        }
        if let Some(ToastEvent::Action(_) | ToastEvent::Dismissed(_)) = toasts_in(ui, Id::new("toasts"), toast_area, &mut self.toasts) {
            ui.request_repaint_in(Duration::ZERO);
        }
        self.save_geometry_when_due(ui);
        self.drag_live = ui.dragging();
    }

    /// Keeps the window size in `geometry`; the first size seen is only
    /// recorded, later changes schedule a save.
    fn track_window_size(&mut self, ui: &mut Ui) {
        let win = ui.window_rect();
        if win.w <= 0 || win.h <= 0 {
            return;
        }
        // DPI-only logical px: main.rs restores it as a winit LogicalSize.
        let s = ui.dpi_scale();
        let logical = ((win.w as f32 / s).round() as u32, (win.h as f32 / s).round() as u32);
        if self.geometry.window_size != Some(logical) {
            let first = self.geometry.window_size.is_none();
            self.geometry.window_size = Some(logical);
            if !first {
                self.geometry_changed();
            }
        }
    }

    /// Saves geometry once it has been dirty for `GEOMETRY_SAVE_DELAY` and no
    /// mouse button is held (mid-drag); otherwise asks for a frame then.
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

    /// The main area: the dock's groups (each a tab strip above its shown
    /// tab), the dividers between them and, while a tab is dragged, the
    /// drop overlays. Keyboard focus and the focused group follow each
    /// other: a press or focus inside a group focuses it, and a tab focused
    /// some other way (palette, shortcut, drop) gets keyboard focus.
    fn main_area(&mut self, ui: &mut Ui, rect: Rect) {
        self.sync_dock();
        if let Some(id) = self.closing {
            self.close_confirm(ui, id);
        }
        let Some(mut shown) = self.dock.layout.clone() else {
            self.visible_tabs.clear();
            self.empty_area(ui, rect);
            return;
        };
        // Every group keeps a minimum size, for this frame only: the stored
        // shares come back when the window grows again.
        let gap = ui.px(crate::dock::GAP).max(1);
        let min = self.group_min(ui);
        shown.enforce_min(rect, gap, min);
        let (rects, dividers) = shown.arrange(rect, gap);
        let n = rects.len();
        // The gear ends the strip of the group at the top right.
        let gear_group = rects.iter().filter(|(_, r)| r.y == rect.y).max_by_key(|(_, r)| r.right()).map(|(g, _)| *g);
        let pressed_at = ui.input().events.iter().find_map(|e| match e {
            Event::MouseDown(skiff_ui::input::MouseButton::Left, p) => Some(*p),
            _ => None,
        });
        let mut ev = GroupEvents::default();
        let prev_visible = std::mem::take(&mut self.visible_tabs);
        for &(gid, r) in &rects {
            let words = self.dock.position_words(gid);
            let shown_title = self.dock.group(gid).and_then(|g| self.tab_index(g.active)).map(|i| self.tab_title(i)).unwrap_or_default();
            let ctx = GroupCtx { id: gid, rect: r, gear: gear_group == Some(gid), words, many: n > 1, prev_visible: &prev_visible };
            if n > 1 {
                let name = format!("{}: {shown_title}", ctx.words);
                ui.a11y_group(Id::new(("dock-group", gid)), skiff_ui::a11y::Role::Region, &name, |ui| self.group_ui(ui, &ctx, &mut ev));
            } else {
                self.group_ui(ui, &ctx, &mut ev);
            }
            if pressed_at.is_some_and(|p| r.contains(p)) && !ui.dragging() {
                ev.pressed = Some(gid);
            }
        }
        if n > 1 {
            // Dividers are keyed by `Divider::key` (not path), so a focused
            // divider keeps focus when the tree around it changes.
            let layout = self.dock.layout.as_mut().expect("checked above");
            ui.region(Id::new("region-dock-dividers"), "Group dividers", |ui| {
                for d in &dividers {
                    crate::dock::view::divider(ui, Id::new("dock-divider").with(d.key), d, gap, layout);
                }
            });
        }
        // What the groups asked for, now that none is borrowed.
        if let Some((tab, to)) = ev.drop {
            self.move_tab(tab, to);
            ui.request_repaint_in(Duration::ZERO);
        }
        for id in ev.close {
            self.request_close_tab(id);
        }
        if let Some(t) = ev.show {
            self.show_tab(t);
        }
        if ev.plus {
            self.palette_open = true;
        }
        self.run_menu_requests(ui);
        self.follow_focus(ui, &rects, ev.pressed);
    }

    /// The smallest a group may get (w, h) in px: room for a few tabs, and
    /// below its strip `MIN_COLS` x `MIN_ROWS` terminal cells or so.
    fn group_min(&self, ui: &Ui) -> (i32, i32) {
        (ui.px(MIN_GROUP_W), ui.px(tokens::TAB_H + MIN_GROUP_H))
    }

    /// One group: its tab strip (in a "Tabs" region, with the gear in the
    /// top-right group) above its shown tab (in a "Content" region), and
    /// the drop overlays while a tab is dragged over either.
    fn group_ui(&mut self, ui: &mut Ui, ctx: &GroupCtx, ev: &mut GroupEvents) {
        let Some(group) = self.dock.group(ctx.id).cloned() else { return };
        let parts = ui.split_v(ctx.rect, &[L::Px(tokens::TAB_H), L::Fill], 0);
        let (strip, content) = (parts[0], parts[1]);
        let p = self.theme.palette;
        let bw = ui.px(1).max(1);
        ui.painter().rect(strip, p.surface, 0.0);
        ui.painter().rect(Rect::new(strip.x, strip.bottom() - bw, strip.w, bw), p.border, 0.0);
        let idx: Vec<usize> = group.tabs.iter().filter_map(|t| self.tab_index(*t)).collect();
        let titles: Vec<String> = idx.iter().map(|&i| self.tab_title(i)).collect();
        let leads: Vec<_> = idx.iter().map(|&i| self.tab_lead(i)).collect();
        let tt: Vec<TabTitle> = titles
            .iter()
            .zip(&leads)
            .map(|(t, (status, icon))| TabTitle { text: t, closable: true, status: *status, icon: *icon })
            .collect();
        let focused = ctx.id == self.dock.focus;
        let tab_strip = if ctx.gear { Rect::new(strip.x, strip.y, (strip.w - strip.h).max(0), strip.h) } else { strip };
        let tip = crate::gear::tip(self.keymap.chords(Action::Settings).first().map(|c| c.to_string()));
        let plus_tip = match self.keymap.chords(Action::NewTab).first() {
            Some(c) => format!("New tab ({c})"),
            None => "New tab".into(),
        };
        let before = group.tabs.iter().position(|t| *t == group.active).unwrap_or(0);
        let mut active = before;
        let (r, gear) = ui.region(Id::new(("region-tabs", ctx.id)), "Tabs", |ui| {
            let opts = TabsOpts { plus_tooltip: Some(&plus_tip), inactive: ctx.many && !focused };
            let r = tabs_with(ui, crate::dock::strip_id(ctx.id), tab_strip, &tt, &mut active, opts);
            let gear = ctx.gear && crate::gear::button(ui, crate::gear::rect(ui, strip), &tip);
            (r, gear)
        });
        if gear {
            self.open_settings();
        }
        // Reorders within the strip (drag or Shift+arrows).
        if let Some((from, to)) = r.moved {
            self.dock.reorder(ctx.id, from, to);
        }
        if let Some(&t) = group.tabs.get(active)
            && active != before
        {
            ev.show = Some(t);
        }
        if let Some(t) = r.closed.and_then(|i| group.tabs.get(i)) {
            ev.close.push(*t);
        }
        if r.plus_clicked {
            ev.plus = true;
            ev.show = Some(group.active);
        }
        // Any tab dragged out of its strip starts a dock drag.
        if let Some(i) = r.dragged_out
            && let Some(&t) = group.tabs.get(i)
        {
            ui.start_drag(Box::new(DockDrag { tab: t }), titles[i].clone());
        }
        let shown = group.tabs.get(active).copied().unwrap_or(group.active);
        let si = self.tab_index(shown);
        ui.region(Id::new(("region-content", ctx.id)), "Content", |ui| match si {
            Some(i) => self.tab_content(ui, i, content, focused, ctx),
            None => ui.painter().rect(content, p.bg, 0.0),
        });
        // With several groups, the unfocused terminals dim a little and an
        // accent line goes round the focused group's content.
        if ctx.many {
            let term = si.is_some_and(|i| matches!(self.tabs[i].kind, TabKind::Terminal(_)));
            if !focused && term && self.cfg.gui.dim_inactive_panes && !self.theme.name.starts_with("high-contrast") {
                let veil = self.theme.term_bg.with_alpha((DIM * 255.0).round() as u8);
                ui.painter().rect(content, veil, 0.0);
            }
            if focused {
                ui.painter().border(content, p.accent, bw, 0.0);
            }
        }
        self.drop_overlays(ui, ctx.id, strip, &r.tab_rects, content, ev);
    }

    /// While a tab is dragged: an insertion marker on this group's strip,
    /// or on its content the dock guide and a preview of the area a drop
    /// in the zone under the pointer would give the tab. Only drops that
    /// change something (and leave both halves their minimum size) are
    /// offered; a release elsewhere does nothing.
    fn drop_overlays(&mut self, ui: &mut Ui, gid: u64, strip: Rect, tab_rects: &[Rect], content: Rect, ev: &mut GroupEvents) {
        use crate::dock::view;
        let Some(&DockDrag { tab }) = ui.drag_payload::<DockDrag>() else { return };
        let accent = self.theme.palette.accent;
        let mouse = ui.input().mouse;
        let on_strip = ui.drop_target::<DockDrag>(strip);
        if on_strip.hovered {
            let to = Drop::Into { group: gid, index: Some(view::insertion_index(tab_rects, mouse.x)) };
            if self.dock.can_drop(tab, to) {
                if let Drop::Into { index: Some(i), .. } = to {
                    view::insertion_marker(ui, tab_rects, strip, i, accent);
                }
                if on_strip.dropped.is_some() {
                    ev.drop = Some((tab, to));
                }
            }
        }
        let on_content = ui.drop_target::<DockDrag>(content);
        if !on_content.hovered {
            return;
        }
        let gap = ui.px(crate::dock::GAP).max(1);
        let min = self.group_min(ui);
        let to = |z: Zone| match z {
            Zone::Center => Drop::Into { group: gid, index: None },
            zone => Drop::Beside { group: gid, zone },
        };
        let offered = |z: Zone| self.dock.can_drop(tab, to(z)) && z.fits(content, gap, min);
        let guide = view::guide_rects(ui, content);
        let zone = view::zone_under(&guide, content, mouse);
        if offered(zone) {
            view::drop_preview(ui, zone.preview(content, gap), accent);
        }
        if guide.iter().any(|(z, _)| offered(*z)) {
            view::draw_guide(ui, &guide, zone, offered);
        }
        if on_content.dropped.is_some() && offered(zone) {
            ev.drop = Some((tab, to(zone)));
        }
    }

    /// The main area with no tab open: a strip with only "+" and the gear
    /// above the empty state.
    fn empty_area(&mut self, ui: &mut Ui, rect: Rect) {
        let parts = ui.split_v(rect, &[L::Px(tokens::TAB_H), L::Fill], 0);
        let (strip, content) = (parts[0], parts[1]);
        let p = self.theme.palette;
        let bw = ui.px(1).max(1);
        ui.painter().rect(strip, p.surface, 0.0);
        ui.painter().rect(Rect::new(strip.x, strip.bottom() - bw, strip.w, bw), p.border, 0.0);
        let tip = crate::gear::tip(self.keymap.chords(Action::Settings).first().map(|c| c.to_string()));
        let tab_strip = Rect::new(strip.x, strip.y, (strip.w - strip.h).max(0), strip.h);
        let (r, gear) = ui.region(Id::new("region-tabs"), "Tabs", |ui| {
            let mut none = 0;
            let r = tabs_with(ui, crate::dock::strip_id(0), tab_strip, &[], &mut none, TabsOpts::default());
            (r, crate::gear::button(ui, crate::gear::rect(ui, strip), &tip))
        });
        if gear {
            self.open_settings();
        }
        if r.plus_clicked {
            self.palette_open = true;
        }
        ui.region(Id::new("region-content"), "Content", |ui| {
            ui.painter().rect(content, p.bg, 0.0);
            self.no_tabs(ui, content);
        });
    }

    /// Tab `i`'s view in `content`. Only the focused group's tab gets the
    /// view actions (copy, paste, find, zoom).
    fn tab_content(&mut self, ui: &mut Ui, i: usize, content: Rect, focused: bool, ctx: &GroupCtx) {
        let p = self.theme.palette;
        ui.painter().rect(content, p.bg, 0.0);
        let id = self.tabs[i].id;
        let newly_shown = !ctx.prev_visible.contains(&id);
        self.visible_tabs.push(id);
        let mut none = vec![];
        let actions = if focused { &mut self.view_actions } else { &mut none };
        let (mut show_settings, mut show_forwards) = (false, false);
        match &mut self.tabs[i].kind {
            TabKind::Terminal(t) => {
                // Settings may have changed since last frame: push them to the terminal.
                t.announce = self.cfg.gui.announce_output;
                t.confirm_paste = self.cfg.settings.confirm_multiline_paste;
                t.blink = self.cfg.gui.cursor_blink && crate::os::animations();
                t.pane_label = (!ctx.words.is_empty()).then(|| ctx.words.to_lowercase());
                if newly_shown {
                    t.shown();
                }
                let wake = self.engine.wake_fn();
                crate::term::view(ui, term_id(id), content, t, &self.theme, &self.keymap, wake, actions);
            }
            TabKind::Files(f) => {
                let ctx = crate::files::FilesCtx {
                    tab_id: id,
                    engine: &self.engine,
                    start_dir: &self.start_dir,
                    hosts: &self.hosts,
                    askpass: self.askpass.as_ref(),
                    ssh_binary: &self.cfg.settings.ssh_binary,
                    ssh_version: self.ssh_version.flatten(),
                };
                crate::files::view(ui, content.inset(ui.px(6)), f, &ctx);
            }
            TabKind::Settings => show_settings = true,
            TabKind::Forwards => show_forwards = true,
        }
        // Settings and forwards need `&mut self`, so they run after the
        // borrow of the tab ends.
        if show_settings {
            crate::settings::view(ui, content, self);
            // Saves and deletes in Settings become toasts.
            for (kind, text) in self.settings.take_notices() {
                self.notify(kind, text);
            }
        }
        if show_forwards {
            crate::forwards::view(ui, content, self);
        }
    }

    /// Runs a split or close-group a terminal's menu asked for, on that
    /// terminal's tab (which is focused first, so the action applies to it).
    fn run_menu_requests(&mut self, ui: &mut Ui) {
        let req = self.tabs.iter_mut().find_map(|t| match &mut t.kind {
            TabKind::Terminal(term) => term.pane_request.take().map(|a| (t.id, a)),
            _ => None,
        });
        if let Some((id, a)) = req {
            self.show_tab(id);
            self.do_action(a);
            ui.request_repaint_in(Duration::ZERO);
        }
    }

    /// Keeps the focused group and keyboard focus together. A tab focused
    /// this frame by anything but its own strip (palette, shortcut, drop,
    /// a closed neighbor) takes keyboard focus: a terminal directly, other
    /// views through their group's strip when focus was in another group.
    /// Otherwise a press in a group, or keyboard focus inside one, focuses
    /// that group.
    fn follow_focus(&mut self, ui: &mut Ui, rects: &[(u64, Rect)], pressed: Option<u64>) {
        self.sync_dock();
        let in_group =
            |ui: &Ui, g: u64| ui.focus_in_region(Id::new(("region-tabs", g))) || ui.focus_in_region(Id::new(("region-content", g)));
        let now = self.active_tab_id();
        let focus_group = rects.iter().map(|(g, _)| *g).find(|g| in_group(ui, *g));
        if now.map(|t| (t, self.dock.focus)) != self.last_focus {
            let g = self.dock.focus;
            let on_strip = ui.focus_in_region(Id::new(("region-tabs", g)));
            match (now, self.tabs.get(self.active).map(|t| &t.kind)) {
                _ if on_strip => {}
                (Some(id), Some(TabKind::Terminal(_))) => ui.request_focus(term_id(id)),
                _ if focus_group.is_some_and(|f| f != g) => ui.request_focus(crate::dock::strip_id(g)),
                _ => {}
            }
        } else if let Some(g) = pressed.or(focus_group)
            && g != self.dock.focus
            && let Some(t) = self.dock.group(g).map(|g| g.active)
        {
            self.show_tab(t);
            self.sync_dock();
        }
        self.last_focus = self.active_tab_id().map(|t| (t, self.dock.focus));
    }

    /// Reloads `config.toml` after an outside edit: checked once a second
    /// while the window has focus, and at once when it regains focus.
    fn watch_config(&mut self, ui: &mut Ui) {
        // Track window focus; regaining it polls at once.
        let mut poll_now = false;
        for e in &ui.input().events {
            match e {
                Event::FocusLost => self.window_focused = false,
                Event::FocusGained => {
                    self.window_focused = true;
                    poll_now = true;
                    crate::os::settings_changed(); // e.g. high contrast toggled meanwhile
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
            } else if (crate::os::high_contrast(), crate::os::text_scale()) != self.os_prefs {
                self.pick_theme(); // `theme = "auto"` and the text size follow the OS
            }
            ui.request_repaint_in(CONFIG_POLL);
        } else {
            ui.request_repaint_in(CONFIG_POLL - age);
        }
    }

    /// The modal "close this tab?" dialog for a files tab with transfers
    /// running: Keep (focused first) or Close anyway (danger).
    fn close_confirm(&mut self, ui: &mut Ui, id: u64) {
        let n = match self.tabs.iter().find(|t| t.id == id).map(|t| &t.kind) {
            Some(TabKind::Files(f)) => f.transfers.running(),
            _ => 0,
        };
        if n == 0 {
            self.closing = None; // finished meanwhile: close as asked
            dialog_dismiss(ui, Id::new("close-confirm"));
            self.close_tab(id);
            return;
        }
        let title = "Close this file transfer tab?";
        let body = format!("{n} transfer{} running will be cancelled.", if n == 1 { " still" } else { "s still" });
        let buttons =
            [DialogButton { label: "Keep", kind: ButtonKind::Secondary }, DialogButton { label: "Close anyway", kind: ButtonKind::Danger }];
        let d = Dialog { title, body: &body, buttons: &buttons, initial: 0, icon: Some(Icon::Warning), ..Default::default() };
        match dialog(ui, Id::new("close-confirm"), &d) {
            Some(DialogResult::Button(1)) => self.close_tab(id),
            Some(_) => self.closing = None,
            None => {}
        }
    }

    /// The main area with no tab open: what to do next, with the palette's
    /// shortcut. With no hosts at all the sidebar's own empty state leads.
    fn no_tabs(&mut self, ui: &mut Ui, content: Rect) {
        let chord = |a: Action| self.keymap.chords(a).first().map(|c| c.to_string());
        let hint = chord(Action::QuickSwitcher).map(|c| format!("Command palette: {c}"));
        // With no hosts at all, point at the sidebar's Add host first.
        let (title, body) = if self.hosts.is_empty() {
            ("Welcome to Skiff", "Add your first host in the sidebar. The command palette finds hosts, tabs and commands.")
        } else {
            (
                "No open tabs",
                "Double-click a host in the sidebar to open a terminal, or search hosts, tabs and commands from the command palette.",
            )
        };
        let e = EmptyState {
            icon: Some(Icon::Terminal),
            title,
            body,
            action: Some("Open command palette"),
            secondary: None,
            hint: hint.as_deref(),
        };
        if empty_state(ui, Id::new("no-tabs"), content.inset(ui.px(tokens::SPACE_XL)), &e).action {
            self.palette_open = true;
        }
    }

    /// The status bar's right-hand summary: sessions, running transfers and
    /// forwards ("" when nothing runs).
    fn activity_summary(&mut self) -> String {
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let (mut sessions, mut transfers) = (0, 0);
        for t in &mut self.tabs {
            match &mut t.kind {
                TabKind::Terminal(t) => sessions += usize::from(t.session.as_mut().is_some_and(|s| s.is_alive())),
                TabKind::Files(f) => transfers += f.transfers.running(),
                _ => {}
            }
        }
        let forwards = self.forwards.running.iter().filter(|r| r.handle.state() == ForwardState::Running).count();
        let mut parts = vec![];
        if sessions > 0 {
            parts.push(plural(sessions, "session", "sessions"));
        }
        if transfers > 0 {
            parts.push(plural(transfers, "transfer running", "transfers running"));
        }
        if forwards > 0 {
            parts.push(plural(forwards, "forward", "forwards"));
        }
        parts.join(" \u{00B7} ")
    }

    /// The bottom bar (slim, caption text): the active view's state on the
    /// left (a terminal's with its connection dot), the hovered host in the
    /// middle, and what is running (sessions, transfers, forwards) on the
    /// right. Warnings show as toasts, not here.
    fn status_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let p = self.theme.palette;
        let bw = ui.px(1).max(1);
        // Dock changes (moves, new groups, focus) are spoken from here.
        if !self.dock.announce.is_empty()
            && let Some(n) = ui.a11y_node(Id::new("dock-announce"), skiff_ui::a11y::Role::Status, &self.dock.announce)
        {
            n.live = true;
            n.bounds = rect;
        }
        ui.painter().rect(rect, p.surface, 0.0);
        ui.painter().rect(Rect::new(rect.x, rect.y, rect.w, bw), p.border, 0.0);
        let (font, px) = ui.text_style(TextRole::Caption);
        // A terminal's state leads with its connection dot.
        let mut dot = None;
        let left = match self.tabs.get_mut(self.active).map(|t| &mut t.kind) {
            Some(TabKind::Terminal(t)) => {
                dot = Some(t.conn_state());
                t.status()
            }
            Some(TabKind::Files(f)) => format!("{} \u{2194} {}", f.panes[0].loc.label(), f.panes[1].loc.label()),
            Some(TabKind::Settings) => "Settings".into(),
            Some(TabKind::Forwards) => "Port forwards".into(),
            None => format!("{} host{}", self.hosts.len(), if self.hosts.len() == 1 { "" } else { "s" }),
        };
        // The host under the mouse is described in words (not only by its dot).
        let hovered = self.sidebar.hovered.as_ref().and_then(|n| self.hosts.iter().find(|h| &h.name == n)).map(|h| {
            let s = crate::sidebar::describe(h, self.statuses.get(&h.name), &self.history);
            format!("{}: {s}", h.name)
        });
        let right = self.activity_summary();
        let pad = ui.px(tokens::SPACE_MD);
        let inner = Rect::new(rect.x + pad, rect.y + bw, rect.w - 2 * pad, rect.h - bw);
        let rw = (ui.measure(&right, font, px).ceil() as i32).min(inner.w / 3);
        let mut x = inner.x;
        if let Some((st, words)) = dot {
            let d = ui.px(tokens::DOT);
            status_dot(ui, Id::new("status-dot"), Rect::new(x, inner.y, d, inner.h), st, words);
            x += d + ui.px(tokens::SPACE_SM);
        }
        let caption = |c| TextOpts { role: Some(TextRole::Caption), color: Some(c), elide: Elide::End, ..Default::default() };
        let left_w = ((inner.w - rw) * 2 / 5).max(0);
        text(ui, Rect::new(x, inner.y, inner.x + left_w - x, inner.h), &left, caption(p.muted));
        if let Some(h) = hovered {
            let mx = inner.x + left_w + pad;
            text(ui, Rect::new(mx, inner.y, inner.right() - rw - pad - mx, inner.h), &h, caption(p.fg));
        }
        if !right.is_empty() {
            text(ui, Rect::new(inner.right() - rw, inner.y, rw, inner.h), &right, TextOpts { align: Align::End, ..caption(p.muted) });
        }
    }
}

impl App for GuiApp {
    /// Runs one frame with the retained UI state and hands over any fonts
    /// the settings screen picked.
    fn frame(&mut self, ctx: &mut FrameCtx) {
        let mut st = std::mem::take(&mut self.ui_state);
        let style = self.style.clone();
        Ui::run(&mut st, ctx, &style, |ui| self.ui(ui));
        self.ui_state = st;
        if let Some(f) = self.pending_fonts.take() {
            ctx.replace_fonts = Some(f);
        }
    }

    /// On quit: stops running forwards and saves pending geometry.
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
