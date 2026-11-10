//! Quick switcher (Ctrl+Shift+K / Cmd+K): fuzzy search over hosts, open
//! tabs, themes and commands. Arrow keys + Enter; Esc or a click outside
//! closes it.

use termax_core::hosts::{HostEntry, fuzzy_score};
use termax_ui::geom::{Point, Rect};
use termax_ui::input::{Key, Modifiers};
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Sense, Ui};
use termax_ui::widgets::{TextInputOpts, text_input};

const MAX_ROWS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    NewConnection,
    Settings,
    ReloadConfig,
    Forwards,
    CheckStatus,
}

impl Cmd {
    const ALL: [Cmd; 5] = [Cmd::NewConnection, Cmd::Settings, Cmd::ReloadConfig, Cmd::Forwards, Cmd::CheckStatus];

    fn label(self) -> &'static str {
        match self {
            Cmd::NewConnection => "New host",
            Cmd::Settings => "Settings",
            Cmd::ReloadConfig => "Reload config",
            Cmd::Forwards => "Port forwards",
            Cmd::CheckStatus => "Check host status",
        }
    }
}

/// How many recent hosts an empty query lists.
pub const RECENT_HOSTS: usize = 8;

/// What ranking needs from the app besides the lists.
#[derive(Debug, Default)]
pub struct RankCtx {
    pub show_hidden: bool,
    /// Most recent first (see `History::recent`).
    pub recent: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Open a terminal to `hosts[i]`.
    Host(usize),
    /// Open a file transfer to `hosts[i]`.
    HostFiles(usize),
    Tab(u64),
    Theme(String),
    Command(Cmd),
}

/// Case-insensitive subsequence score (higher is better), `None` if no match.
fn score_text(query: &str, text: &str) -> Option<i64> {
    let q = query.to_lowercase();
    let hay = text.to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    if let Some(pos) = hay.find(&q) {
        return Some(1000 - pos as i64);
    }
    let mut it = hay.chars();
    let mut score = 0;
    for qc in q.chars() {
        it.find(|&c| c == qc)?;
        score += 1;
    }
    Some(score)
}

/// Everything matching `query`, best first, with display labels.
pub fn rank(query: &str, hosts: &[HostEntry], tabs: &[(u64, String)], themes: &[String], ctx: &RankCtx) -> Vec<(Item, String, i64)> {
    let q = query.trim();
    let mut out: Vec<(Item, String, i64)> = vec![];
    let empty = q.is_empty();
    for (id, title) in tabs {
        if let Some(s) = score_text(q, title) {
            out.push((Item::Tab(*id), format!("Go to {title}"), if empty { 3000 } else { s }));
        }
    }
    let n = hosts.len() as i64;
    for (i, h) in hosts.iter().enumerate() {
        if h.hidden && !ctx.show_hidden {
            continue;
        }
        // An empty query lists recent hosts only (all hosts when there is no history yet).
        let recent = ctx.recent.iter().position(|r| *r == h.name);
        if empty && !ctx.recent.is_empty() && recent.is_none() {
            continue;
        }
        if let Some(s) = fuzzy_score(q, h) {
            let base = match (empty, recent) {
                (true, Some(r)) => 2000 + n - r as i64,
                (true, None) => 2000 + n - i as i64,
                _ => s,
            };
            out.push((Item::Host(i), format!("{}  \u{2014} terminal", h.name), base));
            if !empty {
                out.push((Item::HostFiles(i), format!("{}  \u{2014} file transfer", h.name), base - 1));
            }
        }
    }
    for c in Cmd::ALL {
        if let Some(s) = score_text(q, c.label()) {
            out.push((Item::Command(c), c.label().to_string(), if empty { 1000 } else { s }));
        }
    }
    for t in themes {
        if let Some(s) = score_text(q, &format!("theme {t}")) {
            out.push((Item::Theme(t.clone()), format!("Theme: {t}"), if empty { 0 } else { s }));
        }
    }
    // Stable: equal scores keep the order above (tabs, hosts, commands, themes).
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out
}

#[derive(Debug, Default)]
pub struct SwitcherState {
    pub query: String,
    pub sel: usize,
    /// Set on the frame the switcher opens (focus moves into the input).
    pub just_opened: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub chosen: Option<Item>,
    pub close: bool,
}

/// Draws the switcher over everything; `items` comes from [`rank`].
pub fn show(ui: &mut Ui, st: &mut SwitcherState, items: &[(Item, String, i64)]) -> Outcome {
    let mut out = Outcome::default();
    let input_id = Id::new("switcher-input");
    if st.just_opened {
        ui.request_focus(input_id);
        st.just_opened = false;
    }
    let shown = items.len().min(MAX_ROWS);
    st.sel = st.sel.min(shown.saturating_sub(1));
    for (key, delta) in [(Key::Down, 1i32), (Key::Up, -1)] {
        if ui.input_mut().take_key(&key, Modifiers::NONE) && shown > 0 {
            st.sel = (st.sel as i32 + delta).rem_euclid(shown as i32) as usize;
        }
    }
    if ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) {
        out.close = true;
    }
    if ui.input_mut().take_key(&Key::Enter, Modifiers::NONE) {
        out.chosen = items.get(st.sel).map(|i| i.0.clone());
    }

    let p = ui.style().palette;
    let win = ui.window_rect();
    let w = ui.px(600).min(win.w - ui.px(40)).max(ui.px(200));
    let row_h = ui.px(30);
    let input_h = ui.px(34);
    let pad = ui.px(8);
    let h = input_h + 3 * pad + row_h * shown.max(1) as i32;
    let panel = Rect::new(win.x + (win.w - w) / 2, win.y + ui.px(80), w, h);
    ui.overlay(|ui| {
        let backdrop = ui.interact(Id::new("switcher-backdrop"), win, Sense::click());
        if backdrop.clicked {
            out.close = true;
        }
        let radius = ui.px(8) as f32;
        let bw = ui.px(1).max(1);
        {
            let mut painter = ui.painter();
            painter.rect(win, p.bg.with_alpha(110), 0.0);
            painter.rect(panel, p.surface_alt, radius);
            painter.border(panel, p.border, bw, radius);
        }
        ui.interact(Id::new("switcher-panel"), panel, Sense::hover());
        let input = Rect::new(panel.x + pad, panel.y + pad, panel.w - 2 * pad, input_h);
        let r = text_input(ui, input_id, input, &mut st.query, TextInputOpts { password: false, placeholder: "Search hosts, tabs, themes, commands", ..Default::default() });
        if r.changed {
            st.sel = 0;
        }
        ui.capture_keyboard(input_id);
        let px = ui.font_px();
        let lh = ui.line_height(FontKind::Ui, px);
        if items.is_empty() {
            let at = Point::new(input.x + pad, input.bottom() + pad + (row_h - lh) / 2);
            ui.painter().text(at, "No matches", FontKind::Ui, px, p.muted);
        }
        for (i, (item, label, _)) in items.iter().take(MAX_ROWS).enumerate() {
            let row = Rect::new(panel.x + pad, input.bottom() + pad + i as i32 * row_h, panel.w - 2 * pad, row_h);
            let rr = ui.interact(Id::new(("switcher-row", i)), row, Sense::click());
            if rr.hovered && ui.input().events.iter().any(|e| matches!(e, termax_ui::input::Event::MouseMove(_))) {
                st.sel = i;
            }
            if rr.clicked {
                out.chosen = Some(item.clone());
            }
            if i == st.sel {
                ui.painter().rect(row, p.selection, radius / 2.0);
            }
            let at = Point::new(row.x + pad, row.y + (row_h - lh) / 2);
            ui.painter().text(at, label, FontKind::Ui, px, p.fg);
        }
    });
    if out.chosen.is_some() {
        out.close = true;
    }
    out
}

#[cfg(test)]
#[path = "switcher_tests.rs"]
mod tests;
