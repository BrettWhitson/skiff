//! Theme (applies at once), font files, font size and interface scale.

use std::path::Path;

use termax_ui::geom::Rect;
use termax_ui::text::Fonts;
use termax_ui::ui::{Id, Ui};
use termax_ui::widgets::{button, dropdown, label};

use super::{LABEL_W, Rows, error_line, field, split_row};
use crate::app::{GuiApp, style_for};

#[derive(Default)]
pub struct AppearanceState {
    /// Interface font, terminal font, terminal font size, interface scale.
    fields: [String; 4],
    dirty: bool,
    error: Option<String>,
}

const LABELS: [&str; 4] = ["Interface font file", "Terminal font file", "Terminal font size", "Interface scale"];

fn from_cfg(app: &GuiApp) -> [String; 4] {
    let g = &app.cfg.gui;
    [
        g.ui_font_path.clone().unwrap_or_default(),
        g.font_path.clone().unwrap_or_default(),
        g.font_size.to_string(),
        g.ui_scale.to_string(),
    ]
}

pub fn view(ui: &mut Ui, rect: Rect, st: &mut AppearanceState, app: &mut GuiApp) {
    if !st.dirty {
        st.fields = from_cfg(app);
    }
    let mut rows = Rows::new(rect);
    let (l, c) = split_row(ui, rows.next(ui));
    label(ui, l, "Theme", None);
    let names = app.theme_names();
    let opts: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut idx = names.iter().position(|n| *n == app.cfg.settings.theme).unwrap_or(0);
    if dropdown(ui, Id::new("settings-theme"), Rect::new(c.x, c.y, c.w.min(ui.px(240)), c.h), &opts, &mut idx)
        && let Some(n) = names.get(idx)
    {
        app.apply_theme(n);
    }
    rows.next_h(ui, 6);
    for (i, l) in LABELS.iter().enumerate() {
        if field(ui, Id::new(("settings-look", i)), rows.next(ui), l, &mut st.fields[i]) {
            st.dirty = true;
        }
    }
    let muted = ui.style().palette.muted;
    let hint = rows.next(ui);
    label(ui, Rect::new(hint.x + ui.px(LABEL_W), hint.y, hint.w - ui.px(LABEL_W), hint.h), "Leave a font file empty for the bundled font.", Some(muted));
    let r = rows.next(ui);
    if button(ui, Id::new("settings-look-apply"), Rect::new(r.x + ui.px(LABEL_W), r.y, ui.px(130), r.h), "Apply fonts").clicked {
        match apply(st, app) {
            Ok(()) => {
                st.error = None;
                st.dirty = false;
            }
            Err(e) => st.error = Some(e),
        }
    }
    error_line(ui, rows.next(ui), &st.error);
}

fn apply(st: &AppearanceState, app: &mut GuiApp) -> Result<(), String> {
    let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    let (ui_font, mono) = (opt(&st.fields[0]), opt(&st.fields[1]));
    let size: f32 = st.fields[2].trim().parse().ok().filter(|v| (6.0..=72.0).contains(v)).ok_or("Font size must be a number from 6 to 72")?;
    let scale: f32 =
        st.fields[3].trim().parse().ok().filter(|v| (0.5..=3.0).contains(v)).ok_or("Interface scale must be a number from 0.5 to 3")?;
    let g = &app.cfg.gui;
    if ui_font != g.ui_font_path || mono != g.font_path {
        let fonts = Fonts::load(ui_font.as_deref().map(Path::new), mono.as_deref().map(Path::new))
            .map_err(|e| format!("could not load font: {e:#}"))?;
        app.pending_fonts = Some(fonts);
    }
    let g = &mut app.cfg.gui;
    g.ui_font_path = ui_font;
    g.font_path = mono;
    g.font_size = size;
    g.ui_scale = scale;
    app.style = style_for(&app.theme, &app.cfg);
    app.save_config();
    Ok(())
}
