//! General settings: ssh program, ssh config, scrollback, hidden files, local
//! start folder and host status checks (saved with Save), plus hidden hosts and
//! connection history (applied at once).

use skiff_core::config::StatusChecks;
use skiff_ui::geom::Rect;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{button, checkbox, dropdown_labeled, label};

use super::{LABEL_W, Rows, error_line, field, split_row};
use crate::app::{GuiApp, local_start_dir};

/// The General page's form state.
#[derive(Default)]
pub struct GeneralState {
    /// ssh program, ssh config file, scrollback lines, local start folder.
    fields: [String; 4],
    /// "Load ~/.ssh/config".
    load_ssh_config: bool,
    /// "Show hidden files" in file panes.
    show_hidden: bool,
    /// 0 = only when asked, 1 = also at start.
    status_checks: usize,
    /// Edited since loaded or saved (stop following the config).
    dirty: bool,
    /// Why the last save failed.
    error: Option<String>,
}

/// Draws the General page. The Save part follows the config until edited;
/// the last two rows act immediately.
pub fn view(ui: &mut Ui, rect: Rect, st: &mut GeneralState, app: &mut GuiApp) {
    // An unedited form follows the config.
    if !st.dirty {
        let s = &app.cfg.settings;
        st.fields = [
            s.ssh_binary.clone(),
            s.ssh_config_path.clone().unwrap_or_default(),
            s.scrollback.to_string(),
            s.local_start_dir.clone().unwrap_or_default(),
        ];
        st.load_ssh_config = s.load_ssh_config;
        st.show_hidden = s.show_hidden_files;
        st.status_checks = usize::from(s.status_checks == StatusChecks::OnStart);
    }
    let mut rows = Rows::new(rect);
    let mut edited = false;
    edited |= field(ui, Id::new("settings-gen-ssh"), rows.next(ui), "ssh program", &mut st.fields[0]);
    let (_, c) = split_row(ui, rows.next(ui));
    edited |= checkbox(ui, Id::new("settings-gen-load"), c, &mut st.load_ssh_config, "Load ~/.ssh/config").clicked;
    edited |= field(ui, Id::new("settings-gen-cfgpath"), rows.next(ui), "ssh config file", &mut st.fields[1]);
    edited |= field(ui, Id::new("settings-gen-scroll"), rows.next(ui), "Scrollback lines", &mut st.fields[2]);
    let (_, c) = split_row(ui, rows.next(ui));
    edited |= checkbox(ui, Id::new("settings-gen-hidden"), c, &mut st.show_hidden, "Show hidden files").clicked;
    edited |= field(ui, Id::new("settings-gen-start"), rows.next(ui), "Local start folder", &mut st.fields[3]);
    let (l, c) = split_row(ui, rows.next(ui));
    label(ui, l, "Check host status", None);
    edited |= dropdown_labeled(
        ui,
        Id::new("settings-gen-status"),
        c,
        "Check host status",
        &["When I ask", "Also at start"],
        &mut st.status_checks,
    );
    st.dirty |= edited;
    let r = rows.next(ui);
    if button(ui, Id::new("settings-gen-save"), Rect::new(r.x + ui.px(LABEL_W), r.y, ui.px(100), r.h), "Save").clicked {
        match save(st, app) {
            Ok(()) => {
                st.error = None;
                st.dirty = false;
            }
            Err(e) => st.error = Some(e),
        }
    }
    error_line(ui, rows.next(ui), &st.error);

    // These apply at once (no Save).
    rows.next_h(ui, 8);
    let (_, c) = split_row(ui, rows.next(ui));
    let mut show = app.geometry.show_hidden;
    if checkbox(ui, Id::new("settings-gen-hidden-hosts"), c, &mut show, "Show hidden hosts").clicked {
        app.set_show_hidden(show);
    }
    let (l, c) = split_row(ui, rows.next(ui));
    let n = app.history.hosts.len();
    label(ui, l, "History", None);
    let parts = ui.split_h(c, &[skiff_ui::layout::Size::Px(140), skiff_ui::layout::Size::Fill], 8);
    if button(ui, Id::new("settings-gen-clear-history"), parts[0], "Clear history").clicked {
        app.clear_history();
    }
    let muted = ui.style().palette.muted;
    label(ui, parts[1], &format!("{n} host{} remembered", if n == 1 { "" } else { "s" }), Some(muted));
}

/// Validates the fields (ssh program required, scrollback 100..=1000000),
/// writes them to the config, updates the local start folder and saves.
fn save(st: &GeneralState, app: &mut GuiApp) -> Result<(), String> {
    let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    let ssh = opt(&st.fields[0]).ok_or("The ssh program is required")?;
    let scrollback: usize =
        st.fields[2].trim().parse().ok().filter(|n| (100..=1_000_000).contains(n)).ok_or("Scrollback must be 100 to 1000000 lines")?;
    let s = &mut app.cfg.settings;
    s.ssh_binary = ssh;
    s.load_ssh_config = st.load_ssh_config;
    s.ssh_config_path = opt(&st.fields[1]);
    s.scrollback = scrollback;
    s.show_hidden_files = st.show_hidden;
    s.local_start_dir = opt(&st.fields[3]);
    s.status_checks = if st.status_checks == 1 { StatusChecks::OnStart } else { StatusChecks::Manual };
    app.start_dir = local_start_dir(&app.cfg.settings.local_start_dir);
    app.save_config();
    Ok(())
}
