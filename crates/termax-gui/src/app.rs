//! The application: owns config, hosts, theme and screens, and implements
//! the toolkit's `App` trait.

use std::path::PathBuf;

use termax_core::config::Config;
use termax_core::hosts::{self, HostEntry};
use termax_core::state;
use termax_core::theme::{ThemeDef, builtin};
use termax_ui::geom::{Point, Rect};
use termax_ui::shell::{App, FrameCtx, Waker};
use termax_ui::style::Style;
use termax_ui::text::FontKind;
use termax_ui::ui::{Ui, UiState};

use crate::keys::Keymap;
use crate::theme::{GuiTheme, from_def};

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
    /// Problems to show in the status bar (bad chords, unreadable ssh config, …).
    pub warnings: Vec<String>,
    pub waker: Option<Waker>,
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
            warnings,
            waker,
            ui_state: UiState::default(),
        }
    }

    /// Builds one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        let win = ui.window_rect();
        let p = self.theme.palette;
        ui.painter().rect(win, p.bg, 0.0);
        let side_w = ui.px(self.geometry.sidebar_width.round() as i32).min(win.w);
        let side = Rect::new(0, 0, side_w, win.h);
        ui.painter().rect(side, p.surface, 0.0);
        let px = ui.font_px();
        let lh = ui.line_height(FontKind::Ui, px) + ui.px(8);
        for (i, h) in self.hosts.iter().enumerate() {
            let at = Point::new(ui.px(16), ui.px(40) + i as i32 * lh);
            ui.painter().text(at, &h.name, FontKind::Ui, px, p.fg);
        }
    }
}

impl App for GuiApp {
    fn frame(&mut self, ctx: &mut FrameCtx) {
        let mut st = std::mem::take(&mut self.ui_state);
        let style = self.style.clone();
        Ui::run(&mut st, ctx, &style, |ui| self.ui(ui));
        self.ui_state = st;
    }
}
