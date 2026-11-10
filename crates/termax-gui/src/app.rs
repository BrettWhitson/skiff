//! The application: owns config, hosts, theme, tabs and the engine, and
//! implements the toolkit's `App` trait. Screens live in their own modules.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use termax_core::config::{Config, Connection};
use termax_core::hosts::{self, HostEntry, Source};
use termax_core::state;
use termax_core::theme::{ThemeDef, builtin};
use termax_ui::geom::Rect;
use termax_ui::input::Event;
use termax_ui::layout::Size as L;
use termax_ui::shell::{App, FrameCtx, Waker};
use termax_ui::style::Style;
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Ui, UiState};
use termax_ui::widgets::{TabTitle, label, splitter_h, tabs};

use crate::engine::{Engine, Msg};
use crate::files::{FilesTab, Location};
use crate::keys::{Action, Keymap};
use crate::sidebar::{Sidebar, SidebarAction, sidebar};
use crate::term::TermTab;
use crate::theme::{GuiTheme, from_def};

/// How long geometry changes wait before being written to `state.toml`.
const GEOMETRY_SAVE_DELAY: Duration = Duration::from_secs(1);

pub enum TabKind {
    Terminal(Box<TermTab>),
    Files(Box<FilesTab>),
    Settings,
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
    /// Actions for the active view (copy, paste, zoom), consumed by it.
    pub view_actions: Vec<Action>,
    /// Connection the settings screen should open for editing.
    pub edit_target: Option<usize>,
    /// The tab shown last frame (focus moves into a newly shown terminal).
    shown_tab: Option<u64>,
    ui_state: UiState,
}

/// The theme named in the config, falling back to the first built-in.
pub fn find_theme(cfg: &Config) -> ThemeDef {
    cfg.themes
        .iter()
        .chain(builtin().iter())
        .find(|t| t.name == cfg.settings.theme)
        .cloned()
        .unwrap_or_else(|| builtin().remove(0))
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
        let geometry = state::UiState::load(&geometry_path);
        GuiApp {
            baseline: cfg.clone(),
            cfg,
            cfg_path,
            hosts,
            theme,
            style,
            keymap,
            geometry,
            geometry_path,
            geometry_dirty: None,
            warnings,
            engine: Engine::new(wake),
            tabs: vec![],
            active: 0,
            next_tab_id: 1,
            sidebar: Sidebar::default(),
            switcher_open: false,
            view_actions: vec![],
            edit_target: None,
            shown_tab: None,
            ui_state: UiState::default(),
        }
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
        self.push_tab(TabKind::Terminal(Box::new(TermTab::new(host.clone(), &self.cfg.settings.ssh_binary, self.cfg.settings.scrollback))))
    }

    pub fn open_files(&mut self, left: Location, right: Location) -> u64 {
        self.push_tab(TabKind::Files(Box::new(FilesTab::new(left, right))))
    }

    pub fn open_settings(&mut self) {
        match self.tabs.iter().position(|t| matches!(t.kind, TabKind::Settings)) {
            Some(i) => self.active = i,
            None => {
                self.push_tab(TabKind::Settings);
            }
        }
    }

    pub fn close_tab(&mut self, id: u64) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.tabs.remove(i);
            if self.active >= i && self.active > 0 {
                self.active -= 1;
            }
        }
    }

    pub fn tab_title(&self, i: usize) -> String {
        match &self.tabs[i].kind {
            TabKind::Terminal(t) => format!("\u{25CF} {}", t.host.name),
            TabKind::Files(f) => format!("\u{21C5} {} \u{2194} {}", f.locations[0].label(), f.locations[1].label()),
            TabKind::Settings => "Settings".into(),
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
        self.rebuild_hosts();
    }

    pub fn rebuild_hosts(&mut self) {
        let (hosts, w) = hosts::collect(&self.cfg);
        self.hosts = hosts;
        if let Some(w) = w {
            self.warn(w);
        }
    }

    fn unique_name(&self, base: &str) -> String {
        let taken = |n: &str| self.cfg.connections.iter().any(|c| c.name == n);
        let mut name = format!("{base} (copy)");
        let mut k = 2;
        while taken(&name) {
            name = format!("{base} (copy {k})");
            k += 1;
        }
        name
    }

    fn custom_index(&self, host: usize) -> Option<usize> {
        match self.hosts.get(host)?.source {
            Source::Custom(i) => Some(i),
            Source::SshConfig => None,
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
                self.edit_target = self.custom_index(h);
                self.open_settings();
            }
            SidebarAction::Duplicate(h) => {
                if let Some(i) = self.custom_index(h) {
                    let mut c: Connection = self.cfg.connections[i].clone();
                    c.name = self.unique_name(&c.name);
                    self.cfg.connections.push(c);
                    self.save_config();
                }
            }
            SidebarAction::Delete(h) => {
                if let Some(i) = self.custom_index(h) {
                    self.cfg.connections.remove(i);
                    self.save_config();
                }
            }
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
                    self.close_tab(id);
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

    fn handle_messages(&mut self) {
        for m in self.engine.drain() {
            if let Some(tab) = m.tab()
                && !self.tabs.iter().any(|t| t.id == tab)
            {
                continue; // the tab is gone
            }
            match m {
                Msg::Notice { text, .. } => self.warn(text),
            }
        }
    }

    // ----- frame ------------------------------------------------------

    /// Builds one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.handle_messages();
        self.handle_shortcuts(ui);
        self.track_window_size(ui);

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
            if let Some(a) = sidebar(ui, side, &mut self.sidebar, &self.hosts) {
                self.on_sidebar(a);
            }
            main
        };
        self.main_area(ui, main);
        self.view_actions.clear(); // only the active view may use them
        self.status_bar(ui, status);
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
        let r = tabs(ui, Id::new("tabs"), strip, &tt, &mut self.active);
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
            self.close_tab(id);
        }
        if r.plus_clicked {
            self.switcher_open = true;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));

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
        match &mut tab.kind {
            TabKind::Terminal(t) => {
                let id = Id::new(("term", tab.id));
                if newly_shown {
                    ui.request_focus(id);
                }
                crate::term::view(ui, id, content, t, &self.theme, &self.keymap, self.engine.wake_fn(), &mut self.view_actions);
            }
            TabKind::Files(f) => {
                label(ui, line, &format!("Files: {} \u{2194} {}", f.locations[0].label(), f.locations[1].label()), None);
            }
            TabKind::Settings => {
                label(ui, line, "Settings", None);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let p = self.theme.palette;
        ui.painter().rect(rect, p.surface, 0.0);
        let px = ui.font_px();
        let left = match self.tabs.get_mut(self.active).map(|t| &mut t.kind) {
            Some(TabKind::Terminal(t)) => t.status(),
            Some(TabKind::Files(f)) => format!("files: {} \u{2194} {}", f.locations[0].label(), f.locations[1].label()),
            Some(TabKind::Settings) => "settings".into(),
            None => "no tabs".into(),
        };
        let n = self.tabs.len();
        let right = format!("{n} tab{} \u{00B7} theme: {}", if n == 1 { "" } else { "s" }, self.theme.name);
        let pad = ui.px(10);
        let rw = ui.measure(&right, FontKind::Ui, px).ceil() as i32;
        let inner = rect.inset(0);
        let parts = ui.split_h(Rect::new(inner.x + pad, inner.y, inner.w - 2 * pad, inner.h), &[L::Frac(0.35), L::Fill, L::Px((rw as f32 / ui.scale()).ceil() as i32 + 2)], 12);
        label(ui, parts[0], &left, Some(p.muted));
        if let Some(w) = self.warnings.last().cloned() {
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
    }

    fn exit(&mut self) {
        self.flush_geometry();
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
pub(crate) mod tests;
