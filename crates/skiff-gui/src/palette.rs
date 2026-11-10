//! Command palette (Ctrl+Shift+P, Ctrl+Shift+K, Ctrl+K outside terminals;
//! Cmd+Shift+P / Cmd+K on macOS): one fuzzy search over open tabs, hosts,
//! app commands, port forwards, snippets and themes.
//!
//! An empty query lists Recent (open tabs and recently used hosts), then
//! Suggested commands. A leading prefix narrows the search: `>` commands
//! (and themes), `@` hosts, `:` forwards, `#` snippets. Each row shows an
//! icon, the label with the matched characters highlighted, a muted detail
//! (a host's target), a kind badge and, for commands, the shortcut.
//! Up/Down (wrapping) and Enter, or the mouse; Esc or a click outside
//! closes and focus goes back where it was (the app restores it).

use skiff_core::hosts::{HostEntry, fuzzy_score};
use skiff_ui::a11y::Role;
use skiff_ui::color::Color;
use skiff_ui::geom::{Point, Rect};
use skiff_ui::icons::Icon;
use skiff_ui::input::{Event, Key, Modifiers};
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::text::FontKind;
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::{Elide, TextInputOpts, Tone, badge, badge_width, elide_mode, text_input};

use crate::keys::Action;

/// Most result rows shown at once; the list scrolls past them.
pub const MAX_ROWS: usize = 10;

/// How many recent hosts an empty query lists.
pub const RECENT_HOSTS: usize = 6;

/// App commands the palette can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// Open the new-host form in the settings tab.
    NewConnection,
    /// Open the port-forwards tab.
    Forwards,
    /// Re-probe the hosts' reachability.
    CheckStatus,
    /// Re-read the config files.
    ReloadConfig,
    /// Run an app action (it has a shortcut).
    Action(Action),
}

impl Cmd {
    /// Every command, in the order `>` lists them; the first
    /// `SUGGESTED` are suggested on an empty query.
    pub const ALL: [Cmd; 20] = [
        Cmd::NewConnection,
        Cmd::Forwards,
        Cmd::Action(Action::Settings),
        Cmd::CheckStatus,
        Cmd::Action(Action::ToggleSidebar),
        Cmd::Action(Action::SplitRight),
        Cmd::Action(Action::SplitDown),
        Cmd::Action(Action::FocusNextPane),
        Cmd::Action(Action::ClosePane),
        Cmd::Action(Action::EqualizePanes),
        Cmd::Action(Action::CloseTab),
        Cmd::Action(Action::NextTab),
        Cmd::Action(Action::PrevTab),
        Cmd::Action(Action::Find),
        Cmd::Action(Action::ZoomIn),
        Cmd::Action(Action::ZoomOut),
        Cmd::Action(Action::ZoomReset),
        Cmd::Action(Action::UiScaleUp),
        Cmd::Action(Action::UiScaleDown),
        Cmd::ReloadConfig,
    ];

    /// How many of `ALL` an empty query suggests.
    pub const SUGGESTED: usize = 5;

    /// The text shown and matched against the query.
    pub fn label(self) -> &'static str {
        match self {
            Cmd::NewConnection => "New host\u{2026}",
            Cmd::Forwards => "Port forwards",
            Cmd::CheckStatus => "Check host status",
            Cmd::ReloadConfig => "Reload config",
            Cmd::Action(Action::Settings) => "Open settings",
            Cmd::Action(Action::ZoomIn) => "Terminal zoom in",
            Cmd::Action(Action::ZoomOut) => "Terminal zoom out",
            Cmd::Action(Action::ZoomReset) => "Terminal zoom reset",
            Cmd::Action(Action::Find) => "Find in terminal",
            Cmd::Action(a) => a.label(),
        }
    }

    /// The row's icon.
    fn icon(self) -> Icon {
        match self {
            Cmd::NewConnection => Icon::Plus,
            Cmd::Forwards => Icon::Forward,
            Cmd::CheckStatus | Cmd::ReloadConfig => Icon::Refresh,
            Cmd::Action(Action::Settings) => Icon::Settings,
            Cmd::Action(Action::SplitRight) => Icon::SplitH,
            Cmd::Action(Action::SplitDown) => Icon::SplitV,
            Cmd::Action(Action::Find) => Icon::Search,
            Cmd::Action(Action::CloseTab | Action::ClosePane) => Icon::Close,
            Cmd::Action(Action::ZoomIn | Action::UiScaleUp) => Icon::Plus,
            Cmd::Action(Action::ZoomOut | Action::UiScaleDown) => Icon::Minus,
            Cmd::Action(Action::NextTab) => Icon::ChevronRight,
            Cmd::Action(Action::PrevTab) => Icon::ChevronLeft,
            Cmd::Action(_) => Icon::Terminal,
        }
    }
}

/// What a palette row does when chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Open a terminal to `hosts[i]`.
    Host(usize),
    /// Open a file transfer to `hosts[i]`.
    HostFiles(usize),
    /// Switch to the open tab with this id.
    Tab(u64),
    /// Apply (and save) the named theme.
    Theme(String),
    /// Run an app command.
    Command(Cmd),
    /// Type `cfg.snippets[i]` into the active terminal.
    Snippet(usize),
    /// Start `cfg.forwards[i]`, or stop it when it runs.
    Forward(usize),
}

/// The kind badge on a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Tab,
    Host,
    Files,
    Command,
    Forward,
    Snippet,
    Theme,
}

impl Kind {
    /// The badge text.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Tab => "Tab",
            Kind::Host => "Terminal",
            Kind::Files => "Files",
            Kind::Command => "Command",
            Kind::Forward => "Forward",
            Kind::Snippet => "Snippet",
            Kind::Theme => "Theme",
        }
    }

    /// Order among equal scores (tabs first, themes last).
    fn rank(self) -> u8 {
        self as u8
    }
}

/// A group heading shown on an empty query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Recent,
    Suggested,
}

impl Section {
    /// The heading text.
    fn label(self) -> &'static str {
        match self {
            Section::Recent => "Recent",
            Section::Suggested => "Suggested",
        }
    }
}

/// One palette row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub item: Item,
    pub label: String,
    /// Muted text after the label (a host's `user@host:port`).
    pub detail: String,
    pub kind: Kind,
    pub icon: Icon,
    /// Right-aligned shortcut hint (commands with a binding).
    pub shortcut: Option<String>,
    /// Higher is better.
    pub score: i64,
    /// Char indices of `label` the query matched (drawn highlighted).
    pub matched: Vec<usize>,
    /// The heading this row is listed under (empty query only).
    pub section: Option<Section>,
}

/// Everything the palette searches, gathered by the app each frame it is open.
#[derive(Debug, Default)]
pub struct Sources<'a> {
    pub hosts: &'a [HostEntry],
    /// Include hosts the user marked hidden.
    pub show_hidden: bool,
    /// Most recent first (see `History::recent`).
    pub recent: Vec<String>,
    /// Open tabs: id, title and icon.
    pub tabs: Vec<(u64, String, Icon)>,
    pub themes: Vec<String>,
    /// Saved forwards: index in the config, title, running now.
    pub forwards: Vec<(usize, String, bool)>,
    /// Snippets usable in the active terminal tab: (index in the config, name).
    pub snippets: Vec<(usize, String)>,
    /// The shortcut shown for a command, if bound.
    pub shortcuts: Vec<(Cmd, String)>,
}

/// What a query asks for: its prefix scope and the text after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    /// `>`: commands and themes.
    Commands,
    /// `@`: hosts.
    Hosts,
    /// `:`: port forwards.
    Forwards,
    /// `#`: snippets.
    Snippets,
}

/// Splits a leading scope prefix off `query`; returns the scope and the
/// trimmed rest.
pub fn parse_query(query: &str) -> (Scope, &str) {
    let q = query.trim_start();
    let scope = match q.chars().next() {
        Some('>') => Scope::Commands,
        Some('@') => Scope::Hosts,
        Some(':') => Scope::Forwards,
        Some('#') => Scope::Snippets,
        _ => return (Scope::All, q.trim()),
    };
    (scope, q[1..].trim())
}

/// Case-insensitive fuzzy match of `query` in `text`: the score (higher is
/// better) and the matched char indices, or `None`. A contiguous match
/// beats a scattered one, a match at the start of the text or of a word
/// beats one inside a word, and shorter texts win ties.
pub fn fuzzy(query: &str, text: &str) -> Option<(i64, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, vec![]));
    }
    let hay: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    // `to_lowercase` can change the length (rare); fall back to no highlight then.
    let same_len = hay.len() == text.chars().count();
    let word_start = |i: usize| i == 0 || !hay[i - 1].is_alphanumeric();
    let len_penalty = hay.len() as i64;
    // Contiguous: the first occurrence that starts a word, else the first.
    let starts: Vec<usize> = (0..hay.len().saturating_sub(q.len() - 1)).filter(|&i| hay[i..].starts_with(&q)).collect();
    if let Some(&pos) = starts.iter().find(|&&i| word_start(i)).or(starts.first()) {
        let bonus = if pos == 0 {
            600
        } else if word_start(pos) {
            400
        } else {
            0
        };
        let idx = if same_len { (pos..pos + q.len()).collect() } else { vec![] };
        return Some((2000 + bonus - pos as i64 - len_penalty, idx));
    }
    // Scattered: each query char in order, preferring word starts and runs.
    let mut idx = Vec::with_capacity(q.len());
    let mut score = 0i64;
    let mut from = 0;
    for qc in &q {
        let i = (from..hay.len()).find(|&i| hay[i] == *qc)?;
        score += if idx.last() == Some(&(i.wrapping_sub(1))) {
            10
        } else if word_start(i) {
            8
        } else {
            1
        };
        idx.push(i);
        from = i + 1;
    }
    Some((score * 10 - len_penalty, if same_len { idx } else { vec![] }))
}

/// Builds an entry with no section.
#[allow(clippy::too_many_arguments)]
fn entry(item: Item, label: String, detail: String, kind: Kind, icon: Icon, score: i64, matched: Vec<usize>) -> Entry {
    Entry { item, label, detail, kind, icon, shortcut: None, score, matched, section: None }
}

/// The rows for `query`, best first.
///
/// Empty query (no prefix): Recent (open tabs, then recent hosts; every
/// host when there is no history yet) and Suggested (the first commands).
/// A prefix alone lists everything in its scope; typed text ranks the
/// scope's items by `fuzzy` match (hosts also match on their address, user
/// and tags, ranked below name matches).
pub fn rank(query: &str, src: &Sources) -> Vec<Entry> {
    let (scope, q) = parse_query(query);
    let mut out = vec![];
    let shortcut = |c: Cmd| src.shortcuts.iter().find(|(k, _)| *k == c).map(|(_, s)| s.clone());
    if scope == Scope::All && q.is_empty() {
        for (id, title, icon) in &src.tabs {
            out.push(Entry {
                section: Some(Section::Recent),
                ..entry(Item::Tab(*id), title.clone(), String::new(), Kind::Tab, *icon, 0, vec![])
            });
        }
        let visible = |h: &&HostEntry| !h.hidden || src.show_hidden;
        let mut hosts: Vec<usize> =
            src.recent.iter().filter_map(|n| src.hosts.iter().position(|h| &h.name == n)).filter(|&i| visible(&&src.hosts[i])).collect();
        if src.recent.is_empty() {
            hosts = src.hosts.iter().enumerate().filter(|(_, h)| visible(h)).map(|(i, _)| i).collect();
        }
        for i in hosts.into_iter().take(RECENT_HOSTS) {
            let h = &src.hosts[i];
            out.push(Entry {
                section: Some(Section::Recent),
                ..entry(Item::Host(i), h.name.clone(), h.target(), Kind::Host, Icon::Host, 0, vec![])
            });
        }
        for c in Cmd::ALL.into_iter().take(Cmd::SUGGESTED) {
            out.push(Entry {
                section: Some(Section::Suggested),
                shortcut: shortcut(c),
                ..entry(Item::Command(c), c.label().into(), String::new(), Kind::Command, c.icon(), 0, vec![])
            });
        }
        return out;
    }
    let all = scope == Scope::All;
    if all {
        for (id, title, icon) in &src.tabs {
            if let Some((s, m)) = fuzzy(q, title) {
                out.push(entry(Item::Tab(*id), title.clone(), String::new(), Kind::Tab, *icon, s, m));
            }
        }
    }
    if all || scope == Scope::Hosts {
        for (i, h) in src.hosts.iter().enumerate() {
            if h.hidden && !src.show_hidden {
                continue;
            }
            // The name first (highlighted); else address, user or tags, lower.
            let hit = fuzzy(q, &h.name).or_else(|| fuzzy_score(q, h).map(|s| (s / 4, vec![])));
            if let Some((s, m)) = hit {
                out.push(entry(Item::Host(i), h.name.clone(), h.target(), Kind::Host, Icon::Host, s, m.clone()));
                // The file-transfer variant once something is typed (or asked for with `@`).
                if !q.is_empty() || scope == Scope::Hosts {
                    out.push(entry(Item::HostFiles(i), h.name.clone(), h.target(), Kind::Files, Icon::Transfer, s - 1, m));
                }
            }
        }
    }
    if all || scope == Scope::Commands {
        for c in Cmd::ALL {
            if let Some((s, m)) = fuzzy(q, c.label()) {
                out.push(Entry {
                    shortcut: shortcut(c),
                    ..entry(Item::Command(c), c.label().into(), String::new(), Kind::Command, c.icon(), s, m)
                });
            }
        }
    }
    if all || scope == Scope::Forwards {
        for (i, title, running) in &src.forwards {
            let label = format!("{} {title}", if *running { "Stop" } else { "Start" });
            if let Some((s, m)) = fuzzy(q, &label).or_else(|| fuzzy(q, title).map(|(s, _)| (s, vec![]))) {
                let detail = if *running { "running" } else { "" };
                out.push(entry(Item::Forward(*i), label, detail.into(), Kind::Forward, Icon::Forward, s, m));
            }
        }
    }
    if all || scope == Scope::Snippets {
        for (i, name) in &src.snippets {
            if let Some((s, m)) = fuzzy(q, name) {
                out.push(entry(Item::Snippet(*i), name.clone(), String::new(), Kind::Snippet, Icon::Terminal, s, m));
            }
        }
    }
    if all || scope == Scope::Commands {
        for t in &src.themes {
            let label = format!("Theme: {t}");
            // Typing a theme's name (or "theme") finds it; `>` alone lists them all.
            if let Some((s, m)) = fuzzy(q, &label) {
                out.push(entry(Item::Theme(t.clone()), label, String::new(), Kind::Theme, Icon::Eye, s, m));
            }
        }
    }
    // A bare prefix keeps the order above; typed text ranks by score, equal
    // scores by kind (stable, so then the order above).
    if !q.is_empty() {
        out.sort_by(|a, b| b.score.cmp(&a.score).then(a.kind.rank().cmp(&b.kind.rank())));
    }
    out
}

/// The palette's state between frames while it is open.
#[derive(Debug, Default)]
pub struct PaletteState {
    /// The search text (with any prefix).
    pub query: String,
    /// Index of the highlighted entry.
    pub sel: usize,
    /// First entry shown (the list scrolls to keep `sel` visible).
    pub top: usize,
    /// Set on the frame the palette opens (focus moves into the input).
    pub just_opened: bool,
}

/// What happened in the palette this frame.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// The entry picked by Enter or a click.
    pub chosen: Option<Item>,
    /// The palette should close (Esc, click outside, or a choice was made).
    pub close: bool,
}

/// Widget id of the search field.
pub fn input_id() -> Id {
    Id::new("palette-input")
}

/// Draws the palette over everything and handles its keys and clicks;
/// `entries` comes from [`rank`].
pub fn show(ui: &mut Ui, st: &mut PaletteState, entries: &[Entry]) -> Outcome {
    let mut out = Outcome::default();
    let input_id = input_id();
    if st.just_opened {
        ui.request_focus(input_id);
        st.just_opened = false;
    }
    // Keyboard first: Up/Down wrap around the entries, Page keys jump,
    // Esc closes, Enter picks the highlighted entry.
    let n = entries.len();
    st.sel = st.sel.min(n.saturating_sub(1));
    for (key, delta) in [(Key::Down, 1i32), (Key::Up, -1)] {
        if ui.input_mut().take_key(&key, Modifiers::NONE) && n > 0 {
            st.sel = (st.sel as i32 + delta).rem_euclid(n as i32) as usize;
        }
    }
    if ui.input_mut().take_key(&Key::PageDown, Modifiers::NONE) && n > 0 {
        st.sel = (st.sel + MAX_ROWS).min(n - 1);
    }
    if ui.input_mut().take_key(&Key::PageUp, Modifiers::NONE) {
        st.sel = st.sel.saturating_sub(MAX_ROWS);
    }
    if ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) {
        out.close = true;
    }
    if ui.input_mut().take_key(&Key::Enter, Modifiers::NONE) {
        out.chosen = entries.get(st.sel).map(|e| e.item.clone());
    }
    // Scroll so the selection is on screen.
    if st.sel < st.top {
        st.top = st.sel;
    } else if st.sel >= st.top + MAX_ROWS {
        st.top = st.sel + 1 - MAX_ROWS;
    }
    st.top = st.top.min(n.saturating_sub(MAX_ROWS));
    let shown = &entries[st.top.min(n)..(st.top + MAX_ROWS).min(n)];

    // Geometry: 600 px wide, 20% from the top, 44 px input, 32 px rows,
    // 24 px section headings, a caption footer with the prefixes.
    let p = ui.style().palette;
    let win = ui.window_rect();
    let w = ui.px(600).min(win.w - ui.px(tokens::SPACE_XL) * 2).max(ui.px(240));
    let input_h = ui.px(44);
    let row_h = ui.px(32);
    let head_h = ui.px(tokens::HEADER_H - 4);
    let foot_h = ui.px(tokens::ROW_H);
    let edge = ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
    let bw = ui.px(1).max(1);
    let mut list_h = 0;
    let mut last_section = None;
    for e in shown {
        if e.section.is_some() && e.section != last_section {
            list_h += head_h;
        }
        last_section = e.section;
        list_h += row_h;
    }
    if n == 0 {
        list_h = row_h;
    }
    let h = input_h + bw + edge + list_h + edge + bw + foot_h;
    let y = win.y + (win.h / 5).min((win.h - h).max(0));
    let panel = Rect::new(win.x + (win.w - w) / 2, y, w, h);
    let radius = tokens::RADIUS_LG * ui.scale();
    let (font, px) = ui.text_style(TextRole::Body);
    let (cfont, cpx) = ui.text_style(TextRole::Caption);
    let pad = ui.px(tokens::SPACE_MD);
    let mouse_moved = ui.input().events.iter().any(|e| matches!(e, Event::MouseMove(_)));
    ui.overlay(|ui| {
        ui.a11y_group(Id::new("palette-dialog"), Role::Dialog, "Command palette", |ui| {
            // A click on the scrim (outside the panel) closes.
            if ui.interact(Id::new("palette-backdrop"), win, Sense::click()).clicked {
                out.close = true;
            }
            ui.painter().rect(win, p.scrim.with_alpha((p.scrim.a as u32 * 2 / 3) as u8), 0.0);
            ui.elevate(panel, radius);
            {
                let mut painter = ui.painter();
                painter.rect(panel, p.overlay, radius);
                painter.border(panel, p.border, bw, radius);
            }
            // Claims the panel so clicks on it don't reach the backdrop.
            ui.interact(Id::new("palette-panel"), panel, Sense::hover());
            let input = Rect::new(panel.x + edge, panel.y, panel.w - 2 * edge, input_h);
            let opts = TextInputOpts {
                placeholder: "Search hosts, tabs and commands",
                label: "Command palette search",
                icon: Some(Icon::Search),
                frameless: true,
                ..Default::default()
            };
            if text_input(ui, input_id, input, &mut st.query, opts).changed {
                st.sel = 0;
                st.top = 0;
            }
            ui.capture_keyboard(input_id);
            ui.painter().rect(Rect::new(panel.x, panel.y + input_h, panel.w, bw), p.border, 0.0);
            let lh = ui.line_height(font, px);
            let clh = ui.line_height(cfont, cpx);
            let mut ry = panel.y + input_h + bw + edge;
            if n == 0 {
                let msg = match parse_query(&st.query).0 {
                    Scope::Snippets => "No snippets for this tab",
                    Scope::Forwards => "No saved port forwards",
                    _ => "No matches",
                };
                ui.painter().text(Point::new(panel.x + pad, ry + (row_h - lh) / 2), msg, font, px, p.muted);
            }
            let mut last_section = None;
            for (k, e) in shown.iter().enumerate() {
                let i = st.top + k;
                if let Some(sec) = e.section
                    && e.section != last_section
                {
                    let t = sec.label().to_uppercase();
                    let at = Point::new(panel.x + pad, ry + (head_h - clh) / 2 + ui.px(2));
                    ui.painter().text(at, &t, FontKind::UiSemibold, cpx, p.muted);
                    ry += head_h;
                }
                last_section = e.section;
                let row = Rect::new(panel.x + edge, ry, panel.w - 2 * edge, row_h);
                ry += row_h;
                let rid = Id::new(("palette-row", i));
                let rr = ui.interact(rid, row, Sense::click());
                // Hover moves the highlight only on real mouse movement, so a
                // still pointer doesn't fight the arrow keys.
                if rr.hovered && mouse_moved {
                    st.sel = i;
                }
                if rr.clicked || ui.a11y_clicked(rid) {
                    out.chosen = Some(e.item.clone());
                }
                let sel = i == st.sel;
                let bg = if sel { p.selection } else { p.overlay };
                if sel {
                    let r = tokens::RADIUS_MD * ui.scale();
                    ui.painter().rect(row, p.selection, r);
                }
                let name = match &e.shortcut {
                    Some(s) => format!("{}, {}, {s}", e.label, e.kind.label()),
                    None => format!("{}, {}", e.label, e.kind.label()),
                };
                if let Some(nd) = ui.a11y_node(rid, Role::ListItem, &name) {
                    nd.selected = Some(sel);
                    nd.bounds = row;
                }
                draw_row(ui, row, e, bg, sel);
            }
            // Footer: what Enter does, and the prefixes.
            let foot = Rect::new(panel.x, panel.bottom() - foot_h, panel.w, foot_h);
            ui.painter().rect(Rect::new(panel.x, foot.y - bw, panel.w, bw), p.border, 0.0);
            let hint = "\u{2191}\u{2193} move \u{00B7} Enter open \u{00B7} Esc close";
            let prefixes = "> commands   @ hosts   : forwards   # snippets";
            let ty = foot.y + (foot.h - clh) / 2;
            ui.painter().text(Point::new(foot.x + pad, ty), hint, cfont, cpx, p.muted);
            let pw = ui.measure(prefixes, cfont, cpx).ceil() as i32;
            if foot.w - 2 * pad - pw > ui.measure(hint, cfont, cpx) as i32 + pad {
                ui.painter().text(Point::new(foot.right() - pad - pw, ty), prefixes, cfont, cpx, p.muted);
            }
        })
    });
    if out.chosen.is_some() {
        out.close = true;
    }
    out
}

/// One row: icon, label (matched chars semibold, in the accent when it
/// reads on `bg`), muted detail, then the shortcut and kind badge at the right.
fn draw_row(ui: &mut Ui, row: Rect, e: &Entry, bg: Color, sel: bool) {
    let p = ui.style().palette;
    let (font, px) = ui.text_style(TextRole::Body);
    let (bold, _) = ui.text_style(TextRole::Strong);
    let (cfont, cpx) = ui.text_style(TextRole::Caption);
    let pad = ui.px(tokens::SPACE_SM);
    let icon_px = ui.px(tokens::ICON);
    let lh = ui.line_height(font, px);
    let ty = row.y + (row.h - lh) / 2;
    let fg = p.fg;
    let hi = if p.accent.contrast(bg) >= 4.5 { p.accent } else { fg };
    ui.painter().icon(Rect::new(row.x + pad, row.y + (row.h - icon_px) / 2, icon_px, icon_px), e.icon, if sel { fg } else { p.muted });
    // Right side first: the kind badge, then the shortcut left of it.
    let mut right = row.right() - pad;
    let bwid = badge_width(ui, e.kind.label());
    let tone = if e.kind == Kind::Tab { Tone::Accent } else { Tone::Neutral };
    badge(ui, Rect::new(right - bwid, row.y, bwid, row.h), e.kind.label(), tone);
    right -= bwid + pad;
    if let Some(s) = &e.shortcut {
        let sw = ui.measure(s, cfont, cpx).ceil() as i32;
        let clh = ui.line_height(cfont, cpx);
        ui.painter().text(Point::new(right - sw, row.y + (row.h - clh) / 2), s, cfont, cpx, p.muted);
        right -= sw + pad * 2;
    }
    // The label in runs of matched / unmatched chars.
    let x0 = row.x + pad + icon_px + pad + ui.px(tokens::SPACE_2XS);
    let room = right - x0;
    let shown = elide_mode(ui, &e.label, font, px, room, Elide::End);
    let elided = shown != e.label;
    let mut x = x0;
    let mut run = String::new();
    let mut run_hit = false;
    let chars: Vec<char> = shown.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let hit = !elided && e.matched.contains(&i);
        if hit != run_hit && !run.is_empty() {
            let (f, col) = if run_hit { (bold, hi) } else { (font, fg) };
            x += ui.painter().text(Point::new(x, ty), &run, f, px, col);
            run.clear();
        }
        run_hit = hit;
        run.push(*c);
    }
    if !run.is_empty() {
        let (f, col) = if run_hit { (bold, hi) } else { (font, fg) };
        x += ui.painter().text(Point::new(x, ty), &run, f, px, col);
    }
    // The detail, muted, in what is left.
    if !e.detail.is_empty() {
        let dx = x + ui.px(tokens::SPACE_SM);
        let mpx = ui.mono_px().min(px);
        let room = right - dx;
        if room > ui.px(40) {
            let d = elide_mode(ui, &e.detail, FontKind::Mono, mpx, room, Elide::Middle);
            // Bottom-aligned with the label, so the baselines roughly meet.
            let mlh = ui.line_height(FontKind::Mono, mpx);
            ui.painter().text(Point::new(dx, ty + lh - mlh), &d, FontKind::Mono, mpx, p.muted);
        }
    }
}

#[cfg(test)]
#[path = "palette_tests.rs"]
mod tests;
