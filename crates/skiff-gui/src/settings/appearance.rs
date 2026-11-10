//! Appearance: the theme, the interface and terminal fonts (typed, or
//! picked from the installed ones), the terminal font size and the
//! interface scale. Everything applies at once: the dropdown, the steppers
//! and a picked font immediately, a typed font path on Enter or when focus
//! leaves it (a font that fails to load is refused with the reason).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use skiff_ui::geom::Rect;
use skiff_ui::input::{Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::style::tokens;
use skiff_ui::text::Fonts;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{
    ButtonKind, ButtonOpts, StepperOpts, TextInputOpts, TextOpts, button_with, dropdown_labeled, label, list_named, stepper, text,
    text_input,
};

use super::{LABEL_W, Rows, commit_field, error_line, row_label, split_row};
use crate::app::{GuiApp, style_for};

/// Control ids (also the search results' targets).
pub const ID_THEME: &str = "settings-theme";
pub const ID_UI_FONT: &str = "settings-look-ui-font";
pub const ID_TERM_FONT: &str = "settings-look-term-font";
pub const ID_SIZE: &str = "settings-look-size";
pub const ID_SCALE: &str = "settings-look-scale";

/// Terminal font size range, in points.
pub const FONT_SIZE: StepperOpts<'static> = StepperOpts { min: 6, max: 72, step: 1, label: "Terminal font size" };
/// Interface scale range, in percent.
pub const SCALE: StepperOpts<'static> = StepperOpts { min: 50, max: 300, step: 10, label: "Interface scale, percent" };

/// The Appearance page's state.
#[derive(Default)]
pub struct AppearanceState {
    /// Unapplied text of the interface (0) and terminal (1) font paths.
    drafts: [Option<String>; 2],
    /// Why the last font change failed.
    error: Option<String>,
    /// The path that failed (not retried until it changes).
    failed: Option<String>,
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

/// Draws the Appearance page into `rect` (content coordinates); returns
/// the height used.
pub fn view(ui: &mut Ui, rect: Rect, st: &mut AppearanceState, app: &mut GuiApp) -> i32 {
    let mut rows = Rows::new(rect);
    rows.section(ui, "Theme");
    let (l, c) = split_row(ui, rows.next(ui));
    row_label(ui, l, "Theme");
    let names = app.theme_names();
    let opts: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut idx = names.iter().position(|n| *n == app.cfg.settings.theme).unwrap_or(0);
    if dropdown_labeled(ui, Id::new(ID_THEME), Rect::new(c.x, c.y, c.w.min(ui.px(240)), c.h), "Theme", &opts, &mut idx)
        && let Some(n) = names.get(idx)
    {
        app.apply_theme(n);
    }

    rows.section(ui, "Fonts");
    // Font file rows: a path field and "Choose…", which toggles the picker
    // (starting the font scan the first time).
    for (i, label_, id) in [(0, "Interface font", ID_UI_FONT), (1, "Terminal font", ID_TERM_FONT)] {
        let (l, c) = split_row(ui, rows.next(ui));
        row_label(ui, l, label_);
        let parts = ui.split_h(c, &[L::Fill, L::Px(96)], tokens::SPACE_SM);
        let g = &app.cfg.gui;
        let current = if i == 0 { g.ui_font_path.clone() } else { g.font_path.clone() }.unwrap_or_default();
        if let Some(v) = commit_field(ui, Id::new(id), parts[0], &format!("{label_} file"), &current, &mut st.drafts[i], "System default") {
            apply_font(st, app, i, v);
        }
        let name = format!("Choose {}", label_.to_lowercase());
        let opts = ButtonOpts { a11y_name: Some(&name), ..Default::default() };
        if button_with(ui, Id::new(("settings-look-choose", i)), parts[1], "Choose\u{2026}", opts).clicked {
            st.picker = if st.picker == Some(i) { None } else { Some(i) };
            st.filter.clear();
            st.picked = Some(0);
            if st.fonts.is_none() && st.scan.is_none() {
                st.scan = Some(start_scan());
            }
            ui.request_focus(Id::new("settings-font-filter"));
        }
    }
    let (_, c) = split_row(ui, rows.next_h(ui, 18));
    let muted = ui.style().palette.muted;
    let cap = TextOpts { role: Some(skiff_ui::style::TextRole::Caption), color: Some(muted), ..Default::default() };
    text(ui, c, "Leave a font empty for the system default.", cap);
    if let Some(i) = st.picker
        // A font picked (or System default) fills the field and applies at once.
        && let Some(path) = picker(ui, &mut rows, st, i)
    {
        st.picker = None;
        apply_font(st, app, i, path.map(|p| p.display().to_string()).unwrap_or_default());
    }
    if st.error.is_some() {
        let (_, c) = split_row(ui, rows.next(ui));
        error_line(ui, c, &st.error);
    }
    let (l, c) = split_row(ui, rows.next(ui));
    row_label(ui, l, "Terminal font size");
    let sr = Rect::new(c.x, c.y, c.w.min(ui.px(140)), c.h);
    let mut size = app.cfg.gui.font_size.round() as i64;
    if stepper(ui, Id::new(ID_SIZE), sr, &mut size, FONT_SIZE) {
        app.cfg.gui.font_size = size as f32;
        app.style = style_for(&app.theme, &app.cfg);
        app.save_config();
    }
    let (l, c) = split_row(ui, rows.next(ui));
    row_label(ui, l, "Interface scale");
    let sr = Rect::new(c.x, c.y, c.w.min(ui.px(140)), c.h);
    let mut pct = (app.cfg.gui.ui_scale * 100.0).round() as i64;
    if stepper(ui, Id::new(ID_SCALE), sr, &mut pct, SCALE) {
        app.cfg.gui.ui_scale = pct as f32 / 100.0;
        app.style = style_for(&app.theme, &app.cfg);
        app.save_config();
    }
    let pr = Rect::new(sr.right() + ui.px(tokens::SPACE_SM), c.y, ui.px(24), c.h);
    label(ui, pr, "%", Some(muted));
    rows.used()
}

/// Sets font `i` (0 interface, 1 terminal) to `path` ("" = system
/// default): loads the fonts first (a bad file is refused with the reason,
/// keeping the draft to fix), hands them to the app and saves.
fn apply_font(st: &mut AppearanceState, app: &mut GuiApp, i: usize, path: String) {
    // The same refused path is not loaded again every frame.
    if st.error.is_some() && st.failed.as_deref() == Some(path.as_str()) {
        return;
    }
    match set_font(app, i, &path) {
        Ok(()) => {
            st.drafts[i] = None;
            st.error = None;
            st.failed = None;
        }
        Err(e) => {
            st.drafts[i] = Some(path.clone());
            st.error = Some(e);
            st.failed = Some(path);
        }
    }
}

/// Loads the fonts with font `i` set to `path` and, if that works, stores
/// the path and saves. Errors are user-facing.
fn set_font(app: &mut GuiApp, i: usize, path: &str) -> Result<(), String> {
    let v = Some(path.trim().to_string()).filter(|s| !s.is_empty());
    let g = &app.cfg.gui;
    let (ui_font, mono) = if i == 0 { (v, g.font_path.clone()) } else { (g.ui_font_path.clone(), v) };
    if ui_font == g.ui_font_path && mono == g.font_path {
        return Ok(());
    }
    let fonts = Fonts::load(ui_font.as_deref().map(Path::new), mono.as_deref().map(Path::new))
        .map_err(|e| format!("could not load font: {e:#}"))?;
    app.pending_fonts = Some(fonts);
    let g = &mut app.cfg.gui;
    g.ui_font_path = ui_font;
    g.font_path = mono;
    app.save_config();
    Ok(())
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
    let w = (r.w - ui.px(LABEL_W)).min(ui.px(460));
    let filter = Id::new("settings-font-filter");
    let opts = TextInputOpts { placeholder: "Filter fonts", label: "Filter fonts", ..Default::default() };
    if text_input(ui, filter, Rect::new(x, r.y, w - ui.px(136), r.h), &mut st.filter, opts).changed {
        st.picked = Some(0);
    }
    let ghost = ButtonOpts::kind(ButtonKind::Ghost);
    if button_with(ui, Id::new("settings-font-default"), Rect::new(x + w - ui.px(128), r.y, ui.px(128), r.h), "System default", ghost)
        .clicked
    {
        return Some(None);
    }
    let area = rows.next_h(ui, 8 * 24);
    let list = Rect::new(x, area.y, w, area.h);
    let muted = ui.style().palette.muted;
    let Some(fonts) = &st.fonts else {
        label(ui, list, "Scanning fonts\u{2026}", Some(muted));
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
