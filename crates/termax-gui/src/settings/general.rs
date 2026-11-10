//! ssh program, ssh config, scrollback, hidden files, local start folder.

use termax_ui::geom::Rect;
use termax_ui::ui::{Id, Ui};
use termax_ui::widgets::{button, checkbox};

use super::{LABEL_W, Rows, error_line, field, split_row};
use crate::app::{GuiApp, local_start_dir};

#[derive(Default)]
pub struct GeneralState {
    /// ssh program, ssh config file, scrollback lines, local start folder.
    fields: [String; 4],
    load_ssh_config: bool,
    show_hidden: bool,
    dirty: bool,
    error: Option<String>,
}

pub fn view(ui: &mut Ui, rect: Rect, st: &mut GeneralState, app: &mut GuiApp) {
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
}

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
    app.start_dir = local_start_dir(&app.cfg.settings.local_start_dir);
    app.save_config();
    Ok(())
}
