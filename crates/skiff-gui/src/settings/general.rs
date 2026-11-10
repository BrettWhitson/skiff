//! General settings, grouped: SSH (program, config file, load it), Terminal
//! (scrollback), Files (start folder, hidden files) and Hosts (status
//! checks, hidden hosts, history). Toggles, the dropdown and the stepper
//! apply at once; the text fields apply on Enter or when focus leaves them.

use skiff_core::config::StatusChecks;
use skiff_ui::geom::Rect;
use skiff_ui::layout::Size as L;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{ButtonKind, ButtonOpts, StepperOpts, TextOpts, button_with, checkbox, dropdown_labeled, stepper, text};

use super::{Rows, commit_field, error_line, row_label, split_row};
use crate::app::{GuiApp, local_start_dir};

/// Control ids (also the search results' targets).
pub const ID_SSH: &str = "settings-gen-ssh";
pub const ID_CFGPATH: &str = "settings-gen-cfgpath";
pub const ID_LOAD: &str = "settings-gen-load";
pub const ID_SCROLL: &str = "settings-gen-scroll";
pub const ID_DIM: &str = "settings-gen-dim";
pub const ID_START: &str = "settings-gen-start";
pub const ID_HIDDEN: &str = "settings-gen-hidden";
pub const ID_STATUS: &str = "settings-gen-status";
pub const ID_HIDDEN_HOSTS: &str = "settings-gen-hidden-hosts";
pub const ID_CLEAR: &str = "settings-gen-clear-history";

/// Scrollback range and step, in lines.
pub const SCROLLBACK: StepperOpts<'static> = StepperOpts { min: 100, max: 1_000_000, step: 1000, label: "Scrollback lines" };

/// The text fields on this page, in `GeneralState::drafts` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Text {
    Ssh,
    CfgPath,
    Start,
}

/// The General page's state: text being edited, and the last error.
#[derive(Default)]
pub struct GeneralState {
    /// Unapplied text of the ssh program, ssh config file and start folder.
    drafts: [Option<String>; 3],
    /// Which field the error belongs to, and the message.
    error: Option<(usize, String)>,
}

/// Applies text field `which` (empty clears the optional ones; the ssh
/// program is required) and saves.
fn apply_text(app: &mut GuiApp, which: Text, v: &str) -> Result<(), String> {
    let v = v.trim();
    let opt = Some(v.to_string()).filter(|s| !s.is_empty());
    let s = &mut app.cfg.settings;
    match which {
        Text::Ssh => s.ssh_binary = opt.ok_or("The ssh program is required")?,
        Text::CfgPath => s.ssh_config_path = opt,
        Text::Start => {
            s.local_start_dir = opt;
            app.start_dir = local_start_dir(&app.cfg.settings.local_start_dir);
        }
    }
    app.save_config();
    Ok(())
}

/// A text row: label, a field that applies on commit, and its error below.
fn text_row(ui: &mut Ui, rows: &mut Rows, st: &mut GeneralState, app: &mut GuiApp, which: Text, label: &str, ph: &str) {
    let i = which as usize;
    let current = match which {
        Text::Ssh => app.cfg.settings.ssh_binary.clone(),
        Text::CfgPath => app.cfg.settings.ssh_config_path.clone().unwrap_or_default(),
        Text::Start => app.cfg.settings.local_start_dir.clone().unwrap_or_default(),
    };
    let id = Id::new([ID_SSH, ID_CFGPATH, ID_START][i]);
    let (l, c) = split_row(ui, rows.next(ui));
    row_label(ui, l, label);
    if let Some(v) = commit_field(ui, id, c, label, &current, &mut st.drafts[i], ph) {
        match apply_text(app, which, &v) {
            Ok(()) => {
                st.drafts[i] = None;
                st.error = st.error.take().filter(|e| e.0 != i);
            }
            Err(e) => st.error = Some((i, e)),
        }
    }
    if let Some((_, e)) = st.error.as_ref().filter(|e| e.0 == i) {
        let e = Some(e.clone());
        let (_, c) = split_row(ui, rows.next(ui));
        error_line(ui, c, &e);
    }
}

/// Draws the General page into `rect` (content coordinates); returns the
/// height used.
pub fn view(ui: &mut Ui, rect: Rect, st: &mut GeneralState, app: &mut GuiApp) -> i32 {
    let mut rows = Rows::new(rect);
    let home = local_start_dir(&None);
    rows.section(ui, "SSH");
    text_row(ui, &mut rows, st, app, Text::Ssh, "ssh program", "ssh");
    text_row(ui, &mut rows, st, app, Text::CfgPath, "ssh config file", "~/.ssh/config");
    let (_, c) = split_row(ui, rows.next(ui));
    let mut load = app.cfg.settings.load_ssh_config;
    if checkbox(ui, Id::new(ID_LOAD), c, &mut load, "Load hosts from ~/.ssh/config").clicked {
        app.cfg.settings.load_ssh_config = load;
        app.save_config();
    }

    rows.section(ui, "Terminal");
    let (l, c) = split_row(ui, rows.next(ui));
    row_label(ui, l, "Scrollback lines");
    let mut lines = app.cfg.settings.scrollback as i64;
    let sr = Rect::new(c.x, c.y, c.w.min(ui.px(180)), c.h);
    if stepper(ui, Id::new(ID_SCROLL), sr, &mut lines, SCROLLBACK) {
        app.cfg.settings.scrollback = lines as usize;
        app.save_config();
    }
    let (_, c) = split_row(ui, rows.next(ui));
    let mut dim = app.cfg.gui.dim_inactive_panes;
    if checkbox(ui, Id::new(ID_DIM), c, &mut dim, "Dim inactive panes").clicked {
        app.cfg.gui.dim_inactive_panes = dim;
        app.save_config();
    }

    rows.section(ui, "Files");
    text_row(ui, &mut rows, st, app, Text::Start, "Local start folder", &home);
    let (_, c) = split_row(ui, rows.next(ui));
    let mut hidden = app.cfg.settings.show_hidden_files;
    if checkbox(ui, Id::new(ID_HIDDEN), c, &mut hidden, "Show hidden files in new file panes").clicked {
        app.cfg.settings.show_hidden_files = hidden;
        app.save_config();
    }

    rows.section(ui, "Hosts");
    let (l, c) = split_row(ui, rows.next(ui));
    row_label(ui, l, "Check host status");
    let mut checks = usize::from(app.cfg.settings.status_checks == StatusChecks::OnStart);
    let dr = Rect::new(c.x, c.y, c.w.min(ui.px(240)), c.h);
    if dropdown_labeled(ui, Id::new(ID_STATUS), dr, "Check host status", &["When I ask", "Also at start"], &mut checks) {
        app.cfg.settings.status_checks = if checks == 1 { StatusChecks::OnStart } else { StatusChecks::Manual };
        app.save_config();
    }
    let (_, c) = split_row(ui, rows.next(ui));
    let mut show = app.geometry.show_hidden;
    if checkbox(ui, Id::new(ID_HIDDEN_HOSTS), c, &mut show, "Show hidden hosts").clicked {
        app.set_show_hidden(show);
    }
    let (l, c) = split_row(ui, rows.next(ui));
    row_label(ui, l, "History");
    let n = app.history.hosts.len();
    let parts = ui.split_h(c, &[L::Px(130), L::Fill], skiff_ui::style::tokens::SPACE_MD);
    let opts = ButtonOpts { enabled: n > 0, ..ButtonOpts::kind(ButtonKind::Secondary) };
    if button_with(ui, Id::new(ID_CLEAR), parts[0], "Clear history", opts).clicked {
        app.clear_history();
    }
    let muted = ui.style().palette.muted;
    text(
        ui,
        parts[1],
        &format!("{n} host{} remembered", if n == 1 { "" } else { "s" }),
        TextOpts { color: Some(muted), ..Default::default() },
    );
    rows.used()
}
