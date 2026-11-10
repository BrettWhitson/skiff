//! The Keybindings page: actions grouped (Tabs, Panes, Terminal, View,
//! App), each with its chords drawn as keycaps. Click (or Enter on) the
//! keycaps to record a new chord (saved to `[keys]` at once); a changed
//! binding shows an accent dot and a Reset button. A filter narrows the
//! list by action or key, and chords shared with another action are
//! flagged.

use skiff_ui::a11y::Role;
use skiff_ui::geom::{Point, Rect};
use skiff_ui::icons::Icon;
use skiff_ui::input::{CursorIcon, Event, Key, Modifiers};
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::{ButtonKind, TextInputOpts, TextOpts, icon_button, section_header, text, text_input};

use super::{Rows, Section, reveal};
use crate::app::GuiApp;
use crate::keys::{Action, Chord};

/// The groups the page lists actions under, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Tabs,
    Panes,
    Terminal,
    View,
    App,
}

impl Group {
    /// Every group, in page order.
    pub const ALL: [Group; 5] = [Group::Tabs, Group::Panes, Group::Terminal, Group::View, Group::App];

    /// The group's heading.
    pub fn label(self) -> &'static str {
        match self {
            Group::Tabs => "Tabs",
            Group::Panes => "Panes",
            Group::Terminal => "Terminal",
            Group::View => "View",
            Group::App => "App",
        }
    }
}

/// The group action `a` is listed under (new actions land in App).
pub fn group(a: Action) -> Group {
    use Action::*;
    match a {
        NewTab | CloseTab | NextTab | PrevTab => Group::Tabs,
        SplitRight | SplitDown | FocusNextPane | ClosePane | MovePaneLeft | MovePaneRight | MovePaneUp | MovePaneDown | ResizePaneLeft
        | ResizePaneRight | ResizePaneUp | ResizePaneDown | EqualizePanes => Group::Panes,
        Copy | Paste | Find | ZoomIn | ZoomOut | ZoomReset => Group::Terminal,
        ToggleSidebar | UiScaleUp | UiScaleDown | UiScaleReset => Group::View,
        #[allow(unreachable_patterns)]
        _ => Group::App,
    }
}

/// The keys of a chord as shown on keycaps: "Ctrl+Shift+W" gives Ctrl,
/// Shift, W; a plus key stays one cap ("Ctrl++" gives Ctrl, +).
pub fn keycaps(chord: &str) -> Vec<String> {
    let (body, plus) = match chord.strip_suffix("++") {
        Some(b) => (b, true),
        None if chord == "+" => ("", true),
        None => (chord, false),
    };
    let mut caps: Vec<String> = body.split('+').filter(|s| !s.is_empty()).map(String::from).collect();
    if plus {
        caps.push("+".into());
    }
    caps
}

/// The Keybindings page's state.
#[derive(Default)]
pub struct KeysState {
    /// The action whose chord is being recorded.
    pub capturing: Option<Action>,
    /// Text narrowing the list (action names and keys).
    pub filter: String,
}

/// The actions shown for `filter`: every word matches the action's name,
/// group or one of its chords (ignoring case).
pub fn filtered(app: &GuiApp, filter: &str) -> Vec<Action> {
    let q = filter.trim().to_lowercase();
    Action::ALL
        .into_iter()
        .filter(|&a| {
            let chords: Vec<String> = app.keymap.chords(a).iter().map(|c| c.to_string()).collect();
            let hay = format!("{} {} {}", a.label(), group(a).label(), chords.join(" ")).to_lowercase();
            q.split_whitespace().all(|w| hay.contains(w))
        })
        .collect()
}

/// Height of one binding row, logical px.
const ROW: i32 = 36;

/// Draws the Keybindings page into `rect` (content coordinates), recording
/// a chord first if one is being captured (the page then takes all
/// keyboard input). Returns the height used.
pub fn view(ui: &mut Ui, rect: Rect, st: &mut KeysState, app: &mut GuiApp) -> i32 {
    let id = Id::new("settings-keys");
    if let Some(a) = st.capturing {
        ui.capture_keyboard(id);
        record(ui, st, a, app);
    }
    let p = ui.style().palette;
    let mut rows = Rows::new(rect);
    let top = rows.next(ui);
    let fw = top.w.min(ui.px(320));
    let opts = TextInputOpts { placeholder: "Filter shortcuts", label: "Filter shortcuts", ..Default::default() };
    text_input(ui, Id::new("settings-keys-filter"), Rect::new(top.x, top.y, fw, top.h), &mut st.filter, opts);
    let cap = TextOpts { role: Some(TextRole::Caption), color: Some(p.muted), ..Default::default() };
    let hint = rows.next_h(ui, 18);
    text(ui, hint, "Click a shortcut to change it. Esc cancels, Backspace restores the default.", cap);
    let shown = filtered(app, &st.filter);
    if shown.is_empty() {
        text(ui, rows.next(ui), "No shortcuts match.", TextOpts { color: Some(p.muted), ..Default::default() });
    }
    let row_h = ui.px(ROW);
    for g in Group::ALL {
        let acts: Vec<Action> = shown.iter().copied().filter(|&a| group(a) == g).collect();
        if acts.is_empty() {
            continue;
        }
        rows.gap(ui, tokens::SPACE_MD);
        let hr = rows.next_h(ui, tokens::HEADER_H);
        section_header(ui, hr, g.label(), Some(acts.len()));
        for a in acts {
            let r = rows.next_h(ui, ROW - tokens::SPACE_SM);
            let r = Rect::new(r.x, r.y, r.w, row_h - ui.px(tokens::SPACE_SM));
            binding_row(ui, r, st, a, app, rect.y);
        }
    }
    rows.used()
}

/// One action's row: name, keycaps (the record button), the modified dot
/// with Reset, and a clash warning.
fn binding_row(ui: &mut Ui, r: Rect, st: &mut KeysState, a: Action, app: &mut GuiApp, content_top: i32) {
    let p = ui.style().palette;
    let name = a.config_name();
    let bid = Id::new(("settings-key", name));
    let lw = ui.px(220).min(r.w / 2);
    text(ui, Rect::new(r.x, r.y, lw, r.h), a.label(), TextOpts::default());
    let chords = app.keymap.chords(a).to_vec();
    let capturing = st.capturing == Some(a);
    let shown = chord_text(&chords);
    // The keycaps area doubles as the record button.
    let area = Rect::new(r.x + lw, r.y, ui.px(240).min(r.w - lw), r.h);
    let mut resp = ui.interact(bid, area, Sense::focusable());
    if resp.focused && ui.keys_for(bid) && !capturing {
        let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
        resp.clicked |= enter || ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
    }
    let a11y = if capturing { format!("{}: press the new shortcut", a.label()) } else { format!("{}: {shown}, change", a.label()) };
    ui.a11y_node(bid, Role::Button, &a11y);
    if resp.hovered {
        ui.set_cursor_icon(CursorIcon::Pointer);
        let radius = tokens::RADIUS_MD * ui.scale();
        ui.painter().rect(area, p.hover, radius);
    }
    if resp.focused {
        reveal(ui, Section::Keybindings, r.y - content_top - ui.px(tokens::HEADER_H), r.bottom() - content_top + ui.px(tokens::SPACE_SM));
    }
    if resp.clicked && !capturing {
        st.capturing = Some(a);
    }
    let mut x = area.x + ui.px(tokens::SPACE_XS);
    if capturing {
        keycap(ui, &mut x, area, "Press keys\u{2026}", true);
    } else if chords.is_empty() {
        text(ui, Rect::new(x, area.y, area.w, area.h), "Not set", TextOpts { color: Some(p.muted), ..Default::default() });
    } else {
        for (n, c) in chords.iter().enumerate() {
            if n > 0 {
                let w = text(
                    ui,
                    Rect::new(x, area.y, ui.px(24), area.h),
                    "or",
                    TextOpts { color: Some(p.muted), role: Some(TextRole::Caption), ..Default::default() },
                );
                x += w + ui.px(tokens::SPACE_XS);
            }
            for k in keycaps(&c.to_string()) {
                keycap(ui, &mut x, area, &k, false);
            }
            x += ui.px(tokens::SPACE_XS);
        }
    }
    // Changed from the default: an accent dot and Reset.
    let mut after = area.right() + ui.px(tokens::SPACE_SM);
    if app.cfg.keys.contains_key(name) {
        let d = ui.px(tokens::DOT);
        let dot = Rect::new(after, r.y + (r.h - d) / 2, d, d);
        ui.painter().rect(dot, p.accent, d as f32 / 2.0);
        if let Some(n) = ui.a11y_node(Id::new(("settings-key-modified", name)), Role::Image, "Changed from the default") {
            n.bounds = dot;
        }
        after += d + ui.px(tokens::SPACE_XS);
        let s = r.h;
        let label = format!("Reset {} to the default", a.label());
        if icon_button(ui, Id::new(("settings-key-reset", name)), Rect::new(after, r.y, s, s), Icon::Refresh, &label, ButtonKind::Ghost)
            .clicked
        {
            app.cfg.keys.remove(name);
            app.save_config();
            st.capturing = None;
        }
        after += s + ui.px(tokens::SPACE_SM);
    }
    // Another action bound to any of the same chords.
    let clash = Action::ALL.into_iter().find(|&o| o != a && app.keymap.chords(o).iter().any(|ch| chords.contains(ch)));
    if let Some(o) = clash {
        let w = (r.right() - after).max(0);
        let warn = format!("Also {}", o.label());
        text(
            ui,
            Rect::new(after, r.y, w, r.h),
            &warn,
            TextOpts { color: Some(p.danger), role: Some(TextRole::Caption), ..Default::default() },
        );
    }
}

/// The chords as one line of text ("Ctrl+Shift+W or Ctrl+F4"), or "not set".
fn chord_text(chords: &[Chord]) -> String {
    if chords.is_empty() {
        return "not set".into();
    }
    chords.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" or ")
}

/// Draws one keycap at `x` (advancing it), vertically centred in `area`:
/// a small raised key with a caption-size label; `live` outlines it in the
/// accent colour (recording).
fn keycap(ui: &mut Ui, x: &mut i32, area: Rect, label: &str, live: bool) {
    let p = ui.style().palette;
    let (font, px) = ui.text_style(TextRole::Caption);
    let tw = ui.measure(label, font, px).ceil() as i32;
    let h = ui.px(22);
    let w = (tw + ui.px(14)).max(h);
    let r = Rect::new(*x, area.y + (area.h - h) / 2, w, h);
    let radius = tokens::RADIUS_SM * ui.scale();
    let bw = ui.px(1).max(1);
    let lh = ui.line_height(font, px);
    let mut painter = ui.painter();
    painter.rect(r, p.surface_alt, radius);
    // A thicker bottom edge reads as a key.
    painter.rect(Rect::new(r.x, r.bottom() - 2 * bw, r.w, 2 * bw), if live { p.accent } else { p.control }, radius);
    painter.border(r, if live { p.accent } else { p.control }, bw, radius);
    painter.text(Point::new(r.x + (w - tw) / 2, r.y + (h - lh) / 2 - bw / 2), label, font, px, if live { p.accent } else { p.fg });
    *x += w + ui.px(tokens::SPACE_XS);
}

/// Handles key presses while recording for action `a`: Esc cancels, Backspace
/// restores the default, any other valid chord is saved.
fn record(ui: &mut Ui, st: &mut KeysState, a: Action, app: &mut GuiApp) {
    let presses: Vec<(Key, Modifiers)> = ui
        .input()
        .events
        .iter()
        .filter_map(|e| if let Event::KeyDown { key, mods, .. } = e { Some((key.clone(), *mods)) } else { None })
        .collect();
    // Every press is consumed; only the first one that ends capture counts.
    for (key, mods) in presses {
        ui.input_mut().take_key(&key, mods);
        if st.capturing.is_none() {
            continue;
        }
        let name = a.config_name().to_string();
        match (&key, mods == Modifiers::NONE) {
            (Key::Escape, true) => st.capturing = None,
            (Key::Backspace, true) => {
                app.cfg.keys.remove(&name);
                app.save_config();
                st.capturing = None;
            }
            _ => {
                if let Some(chord) = Chord::from_press(&key, mods) {
                    app.cfg.keys.insert(name, chord);
                    app.save_config();
                    st.capturing = None;
                }
            }
        }
    }
    // Ignore what the key press would also type.
    ui.input_mut().events.retain(|e| !matches!(e, Event::Text(_)));
}
