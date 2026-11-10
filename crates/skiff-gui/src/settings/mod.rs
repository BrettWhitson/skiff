//! The settings tab: a 200 px section list on the left (with a search box
//! that filters every setting) and the chosen page on the right, at most
//! 720 px wide, under a page heading and grouped by section headers.
//!
//! How changes apply: toggles, dropdowns and steppers apply at once; text
//! settings (paths, the ssh program) apply on Enter or when focus leaves
//! the field, with an inline error when invalid. Forms that create or edit
//! something (hosts, snippets, forwards) keep an explicit primary Save.
//! Everything saves through `GuiApp::save_config` (comments in
//! `config.toml` survive). A form the user has not edited follows the
//! config, so an outside edit shows up at once; one with unsaved edits
//! keeps them.

use skiff_ui::a11y::Role;
use skiff_ui::geom::Rect;
use skiff_ui::icons::Icon;
use skiff_ui::input::{CursorIcon, Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::{
    ButtonKind, Dialog, DialogButton, DialogResult, TextInputOpts, TextOpts, ToastKind, badge, badge_width, dialog, dropdown_labeled,
    scroll_area, section_header, text, text_input, tooltip,
};

use crate::app::GuiApp;
use crate::keys::Action;

pub mod appearance;
pub mod connections;
pub mod general;
pub mod keys;
pub mod snippets;

/// Width of the label column of a form row, logical px.
pub const LABEL_W: i32 = 180;
/// Height of a form row, logical px.
pub const ROW_H: i32 = tokens::CONTROL_H;
/// Width of the section list, logical px.
pub const NAV_W: i32 = 200;
/// Widest the page content gets, logical px.
pub const CONTENT_MAX: i32 = 720;
/// Width of the list column of the Connections and Snippets pages, logical px.
pub const LIST_W: i32 = 240;
/// Height of a section-list item, logical px.
const NAV_ROW_H: i32 = 32;
/// Below this tab width (logical px) the section list becomes an icon rail
/// and the search box moves above the page.
pub const RAIL_BELOW: i32 = 700;
/// Width of the icon rail, logical px.
const RAIL_W: i32 = 52;
/// Below this page width (logical px) list-and-editor pages stack the list
/// above the editor.
pub const STACK_BELOW: i32 = LIST_W + tokens::SPACE_XL + 360;
/// Most notices kept for the app to pick up.
const MAX_NOTICES: usize = 8;

/// A page of the settings tab, picked in the left-hand list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Connections,
    Appearance,
    Keybindings,
    Snippets,
    General,
}

impl Section {
    /// All sections, in list order.
    pub const ALL: [Section; 5] = [Section::Connections, Section::Appearance, Section::Keybindings, Section::Snippets, Section::General];

    /// The name shown in the section list and as the page heading.
    pub fn label(self) -> &'static str {
        match self {
            Section::Connections => "Connections",
            Section::Appearance => "Appearance",
            Section::Keybindings => "Keybindings",
            Section::Snippets => "Snippets",
            Section::General => "General",
        }
    }

    /// The icon beside the name in the section list.
    fn icon(self) -> Icon {
        match self {
            Section::Connections => Icon::Host,
            Section::Appearance => Icon::Eye,
            Section::Keybindings => Icon::Key,
            Section::Snippets => Icon::Terminal,
            Section::General => Icon::Settings,
        }
    }
}

/// The settings tab's UI state: the open section, the search, and each
/// page's state (kept while switching sections).
#[derive(Default)]
pub struct SettingsState {
    pub section: Section,
    /// The settings search; non-empty shows results instead of a page.
    pub search: String,
    pub conn: connections::ConnState,
    pub look: appearance::AppearanceState,
    pub keys: keys::KeysState,
    pub snippets: snippets::SnippetsState,
    pub general: general::GeneralState,
    /// Saves to tell the user about (as toasts), oldest first.
    pub notices: Vec<(ToastKind, String)>,
    /// A control to focus once its page is drawn (a search result's target).
    focus_after: Option<Id>,
}

impl SettingsState {
    /// A keybinding is being recorded: app shortcuts must not fire.
    pub fn capturing(&self) -> bool {
        self.section == Section::Keybindings && self.keys.capturing.is_some()
    }

    /// Takes the notices queued since the last call, for the app to show as toasts.
    pub fn take_notices(&mut self) -> Vec<(ToastKind, String)> {
        std::mem::take(&mut self.notices)
    }

    /// Queues a notice (dropping the oldest past the cap).
    pub fn notify(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.notices.push((kind, text.into()));
        if self.notices.len() > MAX_NOTICES {
            self.notices.remove(0);
        }
    }

    /// Opens `section`; recording a key stops.
    fn open(&mut self, section: Section) {
        self.keys.capturing = None;
        self.section = section;
    }
}

/// Where a search result leads.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Focus this control on the result's page.
    Focus(Id),
    /// Open this host in the editor.
    Host(String),
    /// Open the snippet with this name.
    Snippet(String),
    /// Just open the page.
    Page,
}

/// One searchable setting.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub section: Section,
    /// What the result shows (the setting's label).
    pub title: String,
    /// The group it sits in on its page, e.g. "SSH".
    pub group: &'static str,
    /// Extra words that find it.
    pub keywords: &'static str,
    pub target: Target,
}

/// Every searchable setting: the fixed ones on each page, every action on
/// the Keybindings page, and the saved hosts and snippets.
pub fn index(app: &GuiApp) -> Vec<Entry> {
    let e = |section, title: &str, group, keywords, target| Entry { section, title: title.to_string(), group, keywords, target };
    let f = |id: &str| Target::Focus(Id::new(id));
    // A stepper takes focus in its text field.
    let fs = |id: &str| Target::Focus(Id::new(id).with("field"));
    let mut v = vec![
        e(Section::General, "ssh program", "SSH", "binary openssh path executable", f(general::ID_SSH)),
        e(Section::General, "ssh config file", "SSH", "path ~/.ssh/config include", f(general::ID_CFGPATH)),
        e(Section::General, "Load ~/.ssh/config", "SSH", "ssh config hosts import", f(general::ID_LOAD)),
        e(Section::General, "Scrollback lines", "Terminal", "history buffer lines terminal", fs(general::ID_SCROLL)),
        e(Section::General, "Dim inactive panes", "Terminal", "split focus fade", f(general::ID_DIM)),
        e(Section::General, "Local start folder", "Files", "directory home file transfer", f(general::ID_START)),
        e(Section::General, "Show hidden files", "Files", "dot files dotfiles", f(general::ID_HIDDEN)),
        e(Section::General, "Check host status", "Hosts", "online offline ping reachability", f(general::ID_STATUS)),
        e(Section::General, "Show hidden hosts", "Hosts", "hidden hosts sidebar", f(general::ID_HIDDEN_HOSTS)),
        e(Section::General, "Clear history", "Hosts", "recent connections history", f(general::ID_CLEAR)),
        e(Section::Appearance, "Theme", "Theme", "colors colours dark light high contrast", f(appearance::ID_THEME)),
        e(Section::Appearance, "Interface font", "Fonts", "font file typeface ui", f(appearance::ID_UI_FONT)),
        e(Section::Appearance, "Terminal font", "Fonts", "font file monospace typeface", f(appearance::ID_TERM_FONT)),
        e(Section::Appearance, "Terminal font size", "Fonts", "text size zoom points", fs(appearance::ID_SIZE)),
        e(Section::Appearance, "Interface scale", "Fonts", "zoom ui scale dpi size", fs(appearance::ID_SCALE)),
        e(Section::Connections, "Add host", "Hosts", "new connection server", Target::Host(String::new())),
        e(Section::Snippets, "Add snippet", "Snippets", "new command", Target::Snippet(String::new())),
    ];
    for a in Action::ALL {
        let id = Id::new(("settings-key", a.config_name()));
        v.push(Entry {
            section: Section::Keybindings,
            title: a.label().to_string(),
            group: keys::group(a).label(),
            keywords: "shortcut key binding chord",
            target: Target::Focus(id),
        });
    }
    for h in &app.hosts {
        v.push(Entry {
            section: Section::Connections,
            title: h.name.clone(),
            group: "Hosts",
            keywords: "host",
            target: Target::Host(h.name.clone()),
        });
    }
    for s in &app.cfg.snippets {
        v.push(Entry {
            section: Section::Snippets,
            title: s.name.clone(),
            group: "Snippets",
            keywords: "snippet",
            target: Target::Snippet(s.name.clone()),
        });
    }
    v
}

/// The entries matching `query`: every word must appear (ignoring case) in
/// the title, group, section name or keywords. Titles that start with the
/// query come first.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            let hay = format!("{} {} {} {}", e.title, e.group, e.section.label(), e.keywords).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .collect();
    hits.sort_by_key(|e| !e.title.to_lowercase().starts_with(&q));
    hits
}

/// Draws the settings tab: the section list (with the search box) on the
/// left and the chosen page, or the search results, on the right.
pub fn view(ui: &mut Ui, rect: Rect, app: &mut GuiApp) {
    // Taken out of the app so the pages can borrow both; put back at the end.
    let mut st = std::mem::take(&mut app.settings);
    let rail = rect.w < ui.px(RAIL_BELOW);
    let cols = ui.split_h(rect, &[L::Px(if rail { RAIL_W } else { NAV_W }), L::Fill], 0);
    let p = ui.style().palette;
    let bw = ui.px(1).max(1);
    ui.painter().rect(cols[0], p.surface, 0.0);
    ui.painter().rect(Rect::new(cols[0].right() - bw, cols[0].y, bw, cols[0].h), p.border, 0.0);
    let entries = index(app);
    let hits = search(&entries, &st.search);
    // A section picked in the list is opened after the page is drawn, so a
    // text field losing focus to the click still applies its edit.
    let picked = nav(ui, cols[0], &mut st, &entries, rail);
    // Narrow: smaller margins, and the search box above the page.
    let pad = ui.px(if rail { tokens::SPACE_LG } else { tokens::SPACE_XL });
    let mut area =
        Rect::new(cols[1].x + pad, cols[1].y + ui.px(tokens::SPACE_LG), cols[1].w - 2 * pad, cols[1].h - ui.px(tokens::SPACE_LG));
    if rail {
        let sr = Rect::new(area.x, area.y - ui.px(tokens::SPACE_XS), area.w, ui.px(tokens::CONTROL_H));
        search_box(ui, sr, &mut st);
        let top = sr.bottom() + ui.px(tokens::SPACE_MD);
        area = Rect::new(area.x, top, area.w, area.bottom() - top);
    }
    // List-and-editor pages get the list's width on top of the 720 px.
    let max = if matches!(st.section, Section::Connections | Section::Snippets) && st.search.trim().is_empty() {
        CONTENT_MAX + LIST_W + tokens::SPACE_XL
    } else {
        CONTENT_MAX
    };
    let body = Rect::new(area.x, area.y, area.w.min(ui.px(max)), area.h);
    if !st.search.trim().is_empty() {
        let hit = results(ui, body, &st.search, &hits);
        if let Some(i) = hit {
            let e = hits[i].clone();
            go_to(&mut st, e, app, ui);
        }
    } else {
        let (title, sid) = (st.section.label(), Id::new(("settings-page", st.section.label())));
        page(ui, sid, body, title, |ui, r| match st.section {
            Section::Connections => connections::view(ui, r, &mut st.conn, app, &mut st.notices),
            Section::Appearance => appearance::view(ui, r, &mut st.look, app),
            Section::Keybindings => keys::view(ui, r, &mut st.keys, app),
            Section::Snippets => snippets::view(ui, r, &mut st.snippets, app, &mut st.notices),
            Section::General => general::view(ui, r, &mut st.general, app),
        });
        if let Some(id) = st.focus_after.take() {
            ui.request_focus(id);
        }
    }
    if let Some(s) = picked {
        st.search.clear();
        st.open(s);
    }
    while st.notices.len() > MAX_NOTICES {
        st.notices.remove(0);
    }
    app.settings = st;
}

/// The search box with a magnifier at its right end; Esc clears it.
fn search_box(ui: &mut Ui, sr: Rect, st: &mut SettingsState) {
    let p = ui.style().palette;
    let (pad, s) = (ui.px(tokens::SPACE_SM), ui.px(tokens::ICON));
    let sid = Id::new("settings-search");
    let opts = TextInputOpts { placeholder: "Search settings", label: "Search settings", ..Default::default() };
    text_input(ui, sid, sr, &mut st.search, opts);
    if ui.has_focus(sid) && ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) {
        st.search.clear();
    }
    if st.search.is_empty() {
        ui.painter().icon(Rect::new(sr.right() - s - pad, sr.y + (sr.h - s) / 2, s, s), Icon::Search, p.muted);
    }
}

/// Splits a list-and-editor page (Connections, Snippets): the list column
/// beside the editor, or, narrower than `STACK_BELOW`, the list (room for
/// up to five of its `rows`) above the editor.
pub fn list_and_editor(ui: &Ui, rect: Rect, rows: usize) -> (Rect, Rect) {
    if rect.w >= ui.px(STACK_BELOW) {
        let cols = ui.split_h(rect, &[L::Px(LIST_W), L::Fill], tokens::SPACE_XL);
        return (cols[0], cols[1]);
    }
    let list_h = ui.px(tokens::HEADER_H + tokens::SPACE_XS + 4) + ui.px(tokens::ROW_H) * rows.clamp(2, 5) as i32;
    let gap = ui.px(tokens::SPACE_LG);
    let list = Rect::new(rect.x, rect.y, rect.w, list_h);
    (list, Rect::new(rect.x, list.bottom() + gap, rect.w, (rect.h - list_h - gap).max(0)))
}

/// The section list: the search box, then one item per section (icon and
/// name; the open one filled with an accent bar). While searching, each
/// section shows how many results it has. Returns a newly picked section.
/// In `rail` mode only the sections' icons show (each named, with its
/// name as a tooltip) and the search box is drawn above the page instead.
fn nav(ui: &mut Ui, rect: Rect, st: &mut SettingsState, entries: &[Entry], rail: bool) -> Option<Section> {
    let p = ui.style().palette;
    let pad = ui.px(tokens::SPACE_SM);
    let s = ui.px(tokens::ICON);
    let mut y = rect.y + ui.px(tokens::SPACE_MD);
    if !rail {
        let sr = Rect::new(rect.x + pad, y, rect.w - 2 * pad, ui.px(tokens::CONTROL_H));
        search_box(ui, sr, st);
        y = sr.bottom() + ui.px(tokens::SPACE_MD);
    }
    let searching = !st.search.trim().is_empty();
    let mut picked = None;
    let rh = ui.px(NAV_ROW_H);
    for sec in Section::ALL {
        let r = Rect::new(rect.x + pad, y, rect.w - 2 * pad, rh);
        y += rh + ui.px(tokens::SPACE_2XS);
        let id = Id::new(("settings-nav", sec.label()));
        let mut resp = ui.interact(id, r, Sense::focusable());
        if resp.focused && ui.keys_for(id) {
            let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
            resp.clicked |= enter || ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
        }
        let on = sec == st.section && !searching;
        let count = searching.then(|| search(entries, &st.search).iter().filter(|e| e.section == sec).count());
        if let Some(n) = ui.a11y_node(id, Role::Button, sec.label()) {
            n.selected = Some(on);
            if let Some(c) = count {
                n.value = Some(format!("{c} results"));
            }
        }
        if resp.hovered {
            ui.set_cursor_icon(CursorIcon::Pointer);
            if rail {
                tooltip(ui, r, sec.label());
            }
        }
        let radius = tokens::RADIUS_MD * ui.scale();
        if on {
            ui.painter().rect(r, p.selection, radius);
            let bar = Rect::new(r.x, r.y + ui.px(6), ui.px(3), r.h - ui.px(12));
            let br = ui.px(2) as f32;
            ui.painter().rect(bar, p.accent, br);
        } else if resp.hovered {
            ui.painter().rect(r, p.hover, radius);
        }
        let dim = count == Some(0);
        let fg = if dim { p.text_disabled } else { p.fg };
        let ic = if on {
            p.accent
        } else if dim {
            p.text_disabled
        } else {
            p.muted
        };
        let ix = if rail { r.x + (r.w - s) / 2 } else { r.x + ui.px(tokens::SPACE_MD) };
        ui.painter().icon(Rect::new(ix, r.y + (r.h - s) / 2, s, s), sec.icon(), ic);
        if rail {
            if resp.clicked && (sec != st.section || searching) {
                picked = Some(sec);
            }
            continue;
        }
        let role = if on { TextRole::Strong } else { TextRole::Body };
        let tx = ix + s + ui.px(tokens::SPACE_SM + tokens::SPACE_2XS);
        text(
            ui,
            Rect::new(tx, r.y, r.right() - tx - pad, r.h),
            sec.label(),
            TextOpts { role: Some(role), color: Some(fg), ..Default::default() },
        );
        if let Some(c) = count.filter(|c| *c > 0) {
            let t = c.to_string();
            let bw = badge_width(ui, &t);
            let bh = ui.px(18);
            badge(ui, Rect::new(r.right() - pad - bw, r.y + (r.h - bh) / 2, bw, bh), &t, skiff_ui::widgets::Tone::Accent);
        }
        if resp.clicked && (sec != st.section || searching) {
            picked = Some(sec);
        }
    }
    picked
}

/// The search results: one focusable row per match (title, and the page
/// and group it is on). Enter in the search box opens the first. Returns
/// the result picked.
fn results(ui: &mut Ui, rect: Rect, query: &str, hits: &[&Entry]) -> Option<usize> {
    let p = ui.style().palette;
    let head = Rect::new(rect.x, rect.y, rect.w, ui.px(36));
    let title = format!("Results for \u{201C}{}\u{201D}", query.trim());
    text(ui, head, &title, TextOpts { role: Some(TextRole::Heading), ..Default::default() });
    if let Some(n) = ui.a11y_node(Id::new("settings-results-title"), Role::Heading, &title) {
        n.bounds = head;
        n.live = true;
    }
    let mut picked = None;
    if ui.has_focus(Id::new("settings-search")) && ui.input_mut().take_key(&Key::Enter, Modifiers::NONE) && !hits.is_empty() {
        picked = Some(0);
    }
    let top = head.bottom() + ui.px(tokens::SPACE_MD);
    if hits.is_empty() {
        let r = Rect::new(rect.x, top, rect.w, ui.px(24));
        text(ui, r, "No settings match. Try another word.", TextOpts { color: Some(p.muted), ..Default::default() });
        return picked;
    }
    let rh = ui.px(44);
    let list = Rect::new(rect.x, top, rect.w, rect.bottom() - top);
    scroll_area(ui, Id::new("settings-results"), list, rh * hits.len() as i32, |ui, vis, off| {
        for (i, e) in hits.iter().enumerate() {
            let r = Rect::new(vis.x, vis.y + i as i32 * rh - off, vis.w, rh - ui.px(tokens::SPACE_2XS));
            if r.bottom() < vis.y || r.y > vis.bottom() {
                continue;
            }
            let id = Id::new(("settings-result", i));
            let mut resp = ui.interact(id, r, Sense::focusable());
            if resp.focused && ui.keys_for(id) {
                let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
                resp.clicked |= enter || ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
            }
            let where_ = format!("{} \u{203A} {}", e.section.label(), e.group);
            ui.a11y_node(id, Role::Button, &format!("{}, in {where_}", e.title));
            let radius = tokens::RADIUS_MD * ui.scale();
            if resp.hovered {
                ui.set_cursor_icon(CursorIcon::Pointer);
                ui.painter().rect(r, p.hover, radius);
            }
            let s = ui.px(tokens::ICON);
            let pad = ui.px(tokens::SPACE_MD);
            ui.painter().icon(Rect::new(r.x + pad, r.y + (r.h - s) / 2, s, s), e.section.icon(), p.muted);
            let tx = r.x + 2 * pad + s;
            let half = r.h / 2;
            text(
                ui,
                Rect::new(tx, r.y + ui.px(3), r.right() - tx - pad, half),
                &e.title,
                TextOpts { role: Some(TextRole::Strong), ..Default::default() },
            );
            let cap = TextOpts { role: Some(TextRole::Caption), color: Some(p.muted), ..Default::default() };
            text(ui, Rect::new(tx, r.y + half - ui.px(1), r.right() - tx - pad, half), &where_, cap);
            if resp.clicked {
                picked = Some(i);
            }
        }
    });
    picked
}

/// Opens search result `e`: its page, then its control, host or snippet.
fn go_to(st: &mut SettingsState, e: Entry, app: &mut GuiApp, ui: &mut Ui) {
    st.search.clear();
    st.open(e.section);
    match e.target {
        Target::Focus(id) => st.focus_after = Some(id),
        Target::Host(name) => st.conn.switch(Some(name).filter(|n| !n.is_empty()), app),
        Target::Snippet(name) => {
            let i = app.cfg.snippets.iter().position(|s| s.name == name);
            st.snippets.open(ui, i, app);
        }
        Target::Page => {}
    }
}

/// A settings page: its heading, then `f`'s content in a scroll area (the
/// content height is remembered from the last frame). `f` gets the content
/// rect (full height) and returns the height it used.
pub fn page(ui: &mut Ui, id: Id, rect: Rect, title: &str, f: impl FnOnce(&mut Ui, Rect) -> i32) {
    let head = Rect::new(rect.x, rect.y, rect.w, ui.px(40));
    text(ui, head, title, TextOpts { role: Some(TextRole::Heading), ..Default::default() });
    if let Some(n) = ui.a11y_node(id.with("title"), Role::Heading, title) {
        n.bounds = head;
    }
    let body = Rect::new(rect.x, head.bottom() + ui.px(tokens::SPACE_SM), rect.w, rect.bottom() - head.bottom() - ui.px(tokens::SPACE_SM));
    scrolled(ui, id, body, f);
}

/// `f`'s content in a scroll area over `rect`, sized by the height `f`
/// returned last frame (`f` gets the full content rect).
pub fn scrolled(ui: &mut Ui, id: Id, rect: Rect, f: impl FnOnce(&mut Ui, Rect) -> i32) {
    let content = ui.memory::<PageMem>(id).content.max(rect.h);
    ui.memory::<PageMem>(id).view_h = rect.h;
    let used = scroll_area(ui, id.with("scroll"), rect, content, |ui, vis, off| f(ui, Rect::new(vis.x, vis.y - off, vis.w, content)));
    // Room below the last row when it scrolls; content that fits stays put.
    ui.memory::<PageMem>(id).content = if used > rect.h { used + ui.px(tokens::SPACE_XL) } else { used };
}

/// What a page remembers between frames.
#[derive(Debug, Clone, Copy, Default)]
struct PageMem {
    /// Height of the content last frame.
    content: i32,
    /// Height of the visible part.
    view_h: i32,
}

/// Scrolls `section`'s page so content rows `top..bottom` (relative to the
/// content's top) are visible.
pub fn reveal(ui: &mut Ui, section: Section, top: i32, bottom: i32) {
    let id = Id::new(("settings-page", section.label()));
    let view_h = ui.memory::<PageMem>(id).view_h;
    skiff_ui::widgets::scroll_into_view(ui, id.with("scroll"), top, bottom, view_h);
}

/// Consecutive form rows from the top of `rect`.
pub struct Rows {
    rect: Rect,
    y: i32,
}

impl Rows {
    /// Rows starting at the top of `rect`.
    pub fn new(rect: Rect) -> Rows {
        Rows { rect, y: rect.y }
    }

    /// The next standard-height row.
    pub fn next(&mut self, ui: &Ui) -> Rect {
        self.next_h(ui, ROW_H)
    }

    /// The next row `h` logical px tall (plus an 8 px gap after it).
    pub fn next_h(&mut self, ui: &Ui, h: i32) -> Rect {
        let r = Rect::new(self.rect.x, self.y, self.rect.w, ui.px(h));
        self.y += ui.px(h + tokens::SPACE_SM);
        r
    }

    /// Leaves `h` logical px empty.
    pub fn gap(&mut self, ui: &Ui, h: i32) {
        self.y += ui.px(h);
    }

    /// A section header ("SSH", "Fonts"…) with space above it; returns the
    /// room right of the title for header actions.
    pub fn section(&mut self, ui: &mut Ui, title: &str) -> Rect {
        if self.y > self.rect.y {
            self.gap(ui, tokens::SPACE_MD);
        }
        let r = self.next_h(ui, tokens::HEADER_H);
        section_header(ui, r, title, None)
    }

    /// Height used so far.
    pub fn used(&self) -> i32 {
        self.y - self.rect.y
    }
}

/// Splits a row into its label and control parts.
/// The control part is capped at 460 logical px wide.
pub fn split_row(ui: &Ui, row: Rect) -> (Rect, Rect) {
    let lw = ui.px(LABEL_W).min(row.w);
    (Rect::new(row.x, row.y, lw, row.h), Rect::new(row.x + lw, row.y, (row.w - lw).min(ui.px(460)), row.h))
}

/// A form label in the label column.
pub fn row_label(ui: &mut Ui, r: Rect, text_: &str) {
    let c = ui.style().palette.muted;
    text(ui, Rect::new(r.x, r.y, r.w - ui.px(tokens::SPACE_SM), r.h), text_, TextOpts { color: Some(c), ..Default::default() });
}

/// A labelled text input; true when edited this frame.
pub fn field(ui: &mut Ui, id: Id, row: Rect, text: &str, value: &mut String) -> bool {
    field_ph(ui, id, row, text, value, "")
}

/// A labelled text input with a placeholder; true when edited this frame.
pub fn field_ph(ui: &mut Ui, id: Id, row: Rect, text: &str, value: &mut String, placeholder: &str) -> bool {
    let (l, c) = split_row(ui, row);
    row_label(ui, l, text);
    text_input(ui, id, c, value, TextInputOpts { label: text, placeholder, ..Default::default() }).changed
}

/// A text setting that applies on Enter or when focus leaves it: shows
/// `current` until edited, keeps the edit in `draft`, and returns the text
/// to apply once committed (only when it differs from `current`). A failed
/// apply should leave the draft in place to be fixed.
pub fn commit_field(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    label: &str,
    current: &str,
    draft: &mut Option<String>,
    placeholder: &str,
) -> Option<String> {
    let mut t = draft.clone().unwrap_or_else(|| current.to_string());
    let r = text_input(ui, id, rect, &mut t, TextInputOpts { label, placeholder, ..Default::default() });
    if r.changed {
        *draft = Some(t);
    }
    let left = draft.is_some() && !ui.has_focus(id);
    if r.submitted || left {
        let d = draft.take()?;
        if d != current {
            *draft = Some(d.clone()); // until the caller applies it
            return Some(d);
        }
    }
    None
}

/// A labelled dropdown (named by its label for screen readers); true when changed.
pub fn labeled_dropdown(ui: &mut Ui, id: Id, row: Rect, text: &str, options: &[&str], selected: &mut usize) -> bool {
    let (l, c) = split_row(ui, row);
    row_label(ui, l, text);
    dropdown_labeled(ui, id, c, text, options, selected)
}

/// An inline error (or nothing), with an error icon.
pub fn error_line(ui: &mut Ui, row: Rect, err: &Option<String>) {
    if let Some(e) = err {
        let c = ui.style().palette.danger;
        let s = ui.px(tokens::ICON);
        ui.painter().icon(Rect::new(row.x, row.y + (row.h - s) / 2, s, s), Icon::Error, c);
        let x = row.x + s + ui.px(tokens::SPACE_SM);
        text(ui, Rect::new(x, row.y, row.right() - x, row.h), e, TextOpts { color: Some(c), ..Default::default() });
        if let Some(n) = ui.a11y_node(Id::new(("settings-error", e.as_str())), Role::Status, e) {
            n.live = true;
            n.bounds = row;
        }
    }
}

/// A card behind a group of controls: surface fill, border, medium radius.
pub fn card(ui: &mut Ui, r: Rect) {
    let p = ui.style().palette;
    let radius = tokens::RADIUS_MD * ui.scale();
    let bw = ui.px(1).max(1);
    let mut painter = ui.painter();
    painter.rect(r, p.surface, radius);
    painter.border(r, p.border, bw, radius);
}

/// A switch away from a form with unsaved edits (to `to`), waiting for
/// "Discard …?" to be answered.
pub struct PendingDiscard<T> {
    /// Where to go if the edits are discarded.
    pub to: T,
}

impl<T> PendingDiscard<T> {
    /// A pending switch to `to`.
    pub fn new(to: T) -> Self {
        PendingDiscard { to }
    }
}

/// The dialog id for the discard question of form `ids`.
pub fn discard_id(ids: &str) -> Id {
    Id::new(format!("{ids}-discard"))
}

/// Asks "Discard unsaved changes to `what`?" in a modal dialog with Keep
/// editing (focused first, so a reflexive Enter keeps the edits) and
/// Discard. Esc keeps them too. `Some(true)`: discard, `Some(false)`: keep;
/// `None` while unanswered.
pub fn discard_prompt(ui: &mut Ui, ids: &str, what: &str) -> Option<bool> {
    let title = format!("Discard unsaved changes to {what}?");
    let buttons =
        [DialogButton { label: "Keep editing", kind: ButtonKind::Secondary }, DialogButton { label: "Discard", kind: ButtonKind::Danger }];
    let d = Dialog {
        title: &title,
        body: "Your edits have not been saved.",
        buttons: &buttons,
        initial: 0,
        icon: Some(Icon::Warning),
        ..Default::default()
    };
    match dialog(ui, discard_id(ids), &d)? {
        DialogResult::Button(1) => Some(true),
        _ => Some(false),
    }
}

/// Asks to delete `what` in a modal dialog (Cancel focused first; Delete
/// in the danger style). `Some(true)`: delete, `Some(false)`: keep.
pub fn delete_prompt(ui: &mut Ui, id: Id, title: &str, body: &str) -> Option<bool> {
    let buttons =
        [DialogButton { label: "Cancel", kind: ButtonKind::Secondary }, DialogButton { label: "Delete", kind: ButtonKind::Danger }];
    let d = Dialog { title, body, buttons: &buttons, initial: 0, icon: Some(Icon::Warning), ..Default::default() };
    match dialog(ui, id, &d)? {
        DialogResult::Button(1) => Some(true),
        _ => Some(false),
    }
}

/// The header of a list column (Connections, Snippets): a section header
/// with the count and an "Add" ghost button at the right. True when Add
/// was pressed.
pub fn list_header(ui: &mut Ui, id: Id, r: Rect, title: &str, count: usize) -> bool {
    let room = section_header(ui, r, title, Some(count));
    let w = skiff_ui::widgets::button_width(ui, "Add", 1);
    let h = ui.px(tokens::ROW_H);
    let b = Rect::new(room.right() - w, r.y + (r.h - h) / 2, w, h);
    let opts = skiff_ui::widgets::ButtonOpts { kind: ButtonKind::Ghost, icon: Some(Icon::Plus), ..Default::default() };
    skiff_ui::widgets::button_with(ui, id, b, "Add", opts).clicked
}

#[cfg(test)]
mod tests;
