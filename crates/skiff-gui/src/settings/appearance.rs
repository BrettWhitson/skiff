//! Theme (applies at once), fonts (typed or picked from the installed
//! ones), font size and interface scale.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use skiff_ui::geom::Rect;
use skiff_ui::input::{Key, Modifiers};
use skiff_ui::text::Fonts;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{TextInputOpts, button, dropdown_labeled, label, list_named, text_input};

use super::{LABEL_W, Rows, error_line, field, split_row};
use crate::app::{GuiApp, style_for};

/// The Appearance form's state.
#[derive(Default)]
pub struct AppearanceState {
    /// Interface font, terminal font, terminal font size, interface scale.
    fields: [String; 4],
    /// The fields were edited and not yet applied (stop following the config).
    dirty: bool,
    /// Why the last Apply failed.
    error: Option<String>,
    /// Installed fonts, scanned once in the background.
    fonts: Option<FontLists>,
    /// The font scan in progress, if any.
    scan: Option<Receiver<FontLists>>,
    /// The open font picker: which field it fills (0 interface, 1 terminal).
    picker: Option<usize>,
    /// The picker's filter text.
    filter: String,
    /// The highlighted row of the picker's filtered list.
    picked: Option<usize>,
}

/// Installed fonts as (name, file): monospace ones and all of them.
struct FontLists {
    mono: Vec<(String, PathBuf)>,
    all: Vec<(String, PathBuf)>,
}

/// Scans the installed fonts on a background thread; the lists arrive on the returned channel.
fn start_scan() -> Receiver<FontLists> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let found = skiff_ui::text::system::scan_all(&skiff_ui::text::system::scan_dirs());
        let mono = found.iter().filter(|f| f.2).map(|(n, p, _)| (n.clone(), p.clone())).collect();
        let all = found.into_iter().map(|(n, p, _)| (n, p)).collect();
        let _ = tx.send(FontLists { mono, all });
    });
    rx
}

/// Labels of the four text fields, in `fields` order.
const LABELS: [&str; 4] = ["Interface font file", "Terminal font file", "Terminal font size", "Interface scale"];

/// The four text fields as the config currently has them.
fn from_cfg(app: &GuiApp) -> [String; 4] {
    let g = &app.cfg.gui;
    [g.ui_font_path.clone().unwrap_or_default(), g.font_path.clone().unwrap_or_default(), g.font_size.to_string(), g.ui_scale.to_string()]
}

/// Draws the Appearance form: theme dropdown (applied at once), font and
/// size fields with Apply, and the font pickers.
pub fn view(ui: &mut Ui, rect: Rect, st: &mut AppearanceState, app: &mut GuiApp) {
    // An unedited form follows the config.
    if !st.dirty {
        st.fields = from_cfg(app);
    }
    let mut rows = Rows::new(rect);
    let (l, c) = split_row(ui, rows.next(ui));
    label(ui, l, "Theme", None);
    let names = app.theme_names();
    let opts: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut idx = names.iter().position(|n| *n == app.cfg.settings.theme).unwrap_or(0);
    if dropdown_labeled(ui, Id::new("settings-theme"), Rect::new(c.x, c.y, c.w.min(ui.px(240)), c.h), "Theme", &opts, &mut idx)
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
    label(
        ui,
        Rect::new(hint.x + ui.px(LABEL_W), hint.y, hint.w - ui.px(LABEL_W), hint.h),
        "Leave a font file empty for the system default.",
        Some(muted),
    );
    let r = rows.next(ui);
    let x = r.x + ui.px(LABEL_W);
    let mut do_apply = button(ui, Id::new("settings-look-apply"), Rect::new(x, r.y, ui.px(130), r.h), "Apply fonts").clicked;
    // The two "Choose ... font" buttons toggle the picker for their field and
    // start the font scan the first time.
    for (i, off, text) in [(1, 140, "Choose terminal font…"), (0, 330, "Choose interface font…")] {
        if button(ui, Id::new(("settings-look-choose", i)), Rect::new(x + ui.px(off), r.y, ui.px(180), r.h), text).clicked {
            st.picker = if st.picker == Some(i) { None } else { Some(i) };
            st.filter.clear();
            st.picked = Some(0);
            if st.fonts.is_none() && st.scan.is_none() {
                st.scan = Some(start_scan());
            }
            ui.request_focus(Id::new("settings-font-filter"));
        }
    }
    error_line(ui, rows.next(ui), &st.error);
    if let Some(i) = st.picker
        // A font picked (or System default) fills the field and applies at once.
        && let Some(path) = picker(ui, &mut rows, st, i)
    {
        st.fields[i] = path.map(|p| p.display().to_string()).unwrap_or_default();
        st.picker = None;
        do_apply = true;
    }
    if do_apply {
        match apply(st, app) {
            Ok(()) => {
                st.error = None;
                st.dirty = false;
            }
            Err(e) => st.error = Some(e),
        }
    }
}

/// The font list for field `i`: a filter and the matching installed fonts.
/// Returns the choice; `Some(None)` is the system default.
fn picker(ui: &mut Ui, rows: &mut Rows, st: &mut AppearanceState, i: usize) -> Option<Option<PathBuf>> {
    // Collect the background scan if it has finished; poll again soon if not.
    if let Some(rx) = &st.scan {
        match rx.try_recv() {
            Ok(lists) => {
                st.fonts = Some(lists);
                st.scan = None;
            }
            Err(TryRecvError::Empty) => ui.request_repaint_in(Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => st.scan = None,
        }
    }
    let r = rows.next(ui);
    let x = r.x + ui.px(LABEL_W);
    let w = (r.w - ui.px(LABEL_W)).min(ui.px(420));
    label(ui, Rect::new(r.x, r.y, ui.px(LABEL_W), r.h), if i == 1 { "Terminal font" } else { "Interface font" }, None);
    let filter = Id::new("settings-font-filter");
    let opts = TextInputOpts { placeholder: "Filter fonts", label: "Filter fonts", ..Default::default() };
    if text_input(ui, filter, Rect::new(x, r.y, w - ui.px(130), r.h), &mut st.filter, opts).changed {
        st.picked = Some(0);
    }
    if button(ui, Id::new("settings-font-default"), Rect::new(x + w - ui.px(120), r.y, ui.px(120), r.h), "System default").clicked {
        return Some(None);
    }
    let area = rows.next_h(ui, 8 * 24);
    let list = Rect::new(x, area.y, w, area.h);
    let muted = ui.style().palette.muted;
    let Some(fonts) = &st.fonts else {
        label(ui, list, "Scanning fonts…", Some(muted));
        return None;
    };
    // Case-insensitive substring filter; the terminal font picks from monospace fonts only.
    let q = st.filter.trim().to_lowercase();
    let src = if i == 1 { &fonts.mono } else { &fonts.all };
    let shown: Vec<&(String, PathBuf)> = src.iter().filter(|(n, _)| n.to_lowercase().contains(&q)).collect();
    if shown.is_empty() {
        label(ui, list, "No fonts match", Some(muted));
        return None;
    }
    // Enter in the filter takes the highlighted font; arrows move it.
    let mut submit = false;
    if ui.has_focus(filter) {
        let n = shown.len();
        let cur = st.picked.unwrap_or(0).min(n - 1);
        if ui.input_mut().take_key(&Key::Down, Modifiers::NONE) {
            st.picked = Some((cur + 1).min(n - 1));
        } else if ui.input_mut().take_key(&Key::Up, Modifiers::NONE) {
            st.picked = Some(cur.saturating_sub(1));
        }
        submit = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
    }
    let names: Vec<&str> = shown.iter().map(|(n, _)| n.as_str()).collect();
    let out = list_named(
        ui,
        Id::new("settings-font-list"),
        list,
        "Fonts",
        names.len(),
        24,
        &mut st.picked,
        |r| names[r].to_string(),
        |ui, rr, r, _| {
            label(ui, rr.inset(ui.px(4)), names[r], None);
        },
    );
    // A double-clicked/activated row, or the highlighted one on Enter.
    let pick = out.activated.or(st.picked.filter(|_| submit))?;
    shown.get(pick).map(|(_, p)| Some(p.clone()))
}

/// Validates the fields, loads changed fonts (handed to the app to swap in),
/// and saves font paths, size and scale to the config. Errors are user-facing.
fn apply(st: &AppearanceState, app: &mut GuiApp) -> Result<(), String> {
    let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    let (ui_font, mono) = (opt(&st.fields[0]), opt(&st.fields[1]));
    // Size and scale must parse and fall in range.
    let size: f32 =
        st.fields[2].trim().parse().ok().filter(|v| (6.0..=72.0).contains(v)).ok_or("Font size must be a number from 6 to 72")?;
    let scale: f32 =
        st.fields[3].trim().parse().ok().filter(|v| (0.5..=3.0).contains(v)).ok_or("Interface scale must be a number from 0.5 to 3")?;
    let g = &app.cfg.gui;
    // Only reload fonts when a path changed; a bad file fails before saving.
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
