//! Host sidebar: filter, sort and "⋯" menu, hosts grouped by source (custom,
//! then each ssh config file) with a status dot and first tag, a context
//! menu, an inline delete confirmation and "Open file transfer".

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use termax_core::history::{self, History};
use termax_core::hosts::{HostEntry, Source, fuzzy_score, visible};
use termax_core::sshconfig::edit::display_path;
use termax_core::state::SortMode;
use termax_core::status::Status;
use termax_ui::geom::{Point, Rect};
use termax_ui::input::{Event, Key, Modifiers};
use termax_ui::layout::Size as L;
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Ui};
use termax_ui::widgets::{
    MenuItem, TextInputOpts, button, context_menu, dropdown, elide, label, list_named, open_context_menu, text_input,
};

const ROW_H: i32 = 26;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Header(String),
    Host(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarAction {
    OpenTerminal(usize),
    OpenFiles(usize),
    Forward(usize),
    Check(usize),
    CheckAll,
    Edit(usize),
    Duplicate(usize),
    /// Move a host into ssh config file `.1` (index into the app's file list).
    MoveTo(usize, usize),
    SetHidden(usize, bool),
    Delete(usize),
    SetSort(SortMode),
    ToggleShowHidden,
    OpenForwards,
    NewHost,
}

/// What the sidebar shows, borrowed from the app.
pub struct SidebarCtx<'a> {
    pub hosts: &'a [HostEntry],
    pub statuses: &'a HashMap<String, Status>,
    pub history: &'a History,
    pub sort: SortMode,
    pub show_hidden: bool,
    /// The ssh config files (for "Move to file…").
    pub files: &'a [PathBuf],
}

#[derive(Debug, Default)]
pub struct Sidebar {
    pub filter: String,
    /// Selected row in the flattened list.
    selected: Option<usize>,
    collapsed: HashSet<String>,
    menu_host: Option<usize>,
    confirm_delete: Option<usize>,
    /// The host under the mouse (the status bar describes it).
    pub hovered: Option<usize>,
}

fn group_of(h: &HostEntry) -> String {
    match (&h.source, &h.source_file) {
        (Source::Custom(_), _) => "Custom".into(),
        (Source::SshConfig, Some(f)) => display_path(f),
        (Source::SshConfig, None) => "~/.ssh/config".into(),
    }
}

/// One line about a host for the status bar and screen readers.
pub fn describe(h: &HostEntry, status: Option<&Status>, hist: &History) -> String {
    let ago = hist.ago(&h.name, history::now()).unwrap_or_else(|| "never".into());
    format!("{} \u{00B7} {} \u{00B7} last connected {ago}", h.target(), status.cloned().unwrap_or_default().label())
}

impl Sidebar {
    /// Host index of the selected row.
    pub fn selected_host(&self, ctx: &SidebarCtx) -> Option<usize> {
        match self.selected.and_then(|i| self.rows(ctx).get(i).cloned()) {
            Some(Row::Host(h)) => Some(h),
            _ => None,
        }
    }

    fn rows(&self, ctx: &SidebarCtx) -> Vec<Row> {
        let q = self.filter.trim();
        let mut shown = visible(ctx.hosts, ctx.show_hidden);
        if q.is_empty() {
            history::order(ctx.hosts, &mut shown, ctx.sort, ctx.history);
        } else {
            let mut scored: Vec<(usize, i64)> = shown.into_iter().filter_map(|i| fuzzy_score(q, &ctx.hosts[i]).map(|s| (i, s))).collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            shown = scored.into_iter().map(|(i, _)| i).collect();
        }
        // Groups in the order their first host appears in the host list.
        let mut groups: Vec<String> = Vec::new();
        for h in ctx.hosts {
            let g = group_of(h);
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
        let mut out = vec![];
        for g in groups {
            let members: Vec<usize> = shown.iter().copied().filter(|&i| group_of(&ctx.hosts[i]) == g).collect();
            if members.is_empty() {
                continue;
            }
            let collapsed = q.is_empty() && self.collapsed.contains(&g);
            out.push(Row::Header(g));
            if !collapsed {
                out.extend(members.into_iter().map(Row::Host));
            }
        }
        out
    }
}

fn status_dot(ui: &mut Ui, r: Rect, s: Option<&Status>) {
    let p = ui.style().palette;
    let d = ui.px(8);
    let dot = Rect::new(r.x, r.y + (r.h - d) / 2, d, d);
    let radius = d as f32 / 2.0;
    let fill = match s {
        Some(Status::Online { .. }) => Some(p.success),
        Some(Status::Offline(_)) => Some(p.danger),
        Some(Status::NotSsh) => Some(p.warning),
        _ => None,
    };
    // Only the jump host was checked: a ring in its color, not a solid dot.
    let ring = match s {
        Some(Status::Via(_, inner)) if matches!(**inner, Status::Online { .. }) => p.success,
        Some(Status::Via(..)) => p.warning,
        _ => p.muted,
    };
    match fill {
        Some(c) => ui.painter().rect(dot, c, radius),
        None => {
            let bw = ui.px(if ring == p.muted { 1 } else { 2 }).max(1);
            ui.painter().border(dot, ring, bw, radius)
        }
    }
}

pub fn sidebar(ui: &mut Ui, rect: Rect, sb: &mut Sidebar, ctx: &SidebarCtx) -> Option<SidebarAction> {
    let hosts = ctx.hosts;
    let p = ui.style().palette;
    ui.painter().rect(rect, p.surface, 0.0);
    let pad = ui.px(8);
    let inner = rect.inset(pad);
    let parts = ui.split_v(inner, &[L::Px(28), L::Fill, L::Px(if sb.confirm_delete.is_some() { 64 } else { 0 }), L::Px(30)], 8);
    let (head_r, list_r, confirm_r, button_r) = (parts[0], parts[1], parts[2], parts[3]);
    let mut action = None;

    let head = ui.split_h(head_r, &[L::Fill, L::Px(84), L::Px(30)], 4);
    let fr = text_input(ui, Id::new("sidebar-filter"), head[0], &mut sb.filter, TextInputOpts { password: false, placeholder: "Filter hosts", ..Default::default() });
    if fr.changed {
        sb.selected = None;
    }
    let sorts = [SortMode::Name, SortMode::Recent];
    let mut si = sorts.iter().position(|s| *s == ctx.sort).unwrap_or(0);
    if dropdown(ui, Id::new("sidebar-sort"), head[1], &["Name", "Recent"], &mut si) {
        action = Some(SidebarAction::SetSort(sorts[si]));
    }
    let more = Id::new("sidebar-more-menu");
    if button(ui, Id::new("sidebar-more"), head[2], "\u{22EF}").clicked {
        open_context_menu(ui, more, Point::new(head[2].x, head[2].bottom()));
    }
    let hidden_label = if ctx.show_hidden { "\u{2713} Show hidden hosts" } else { "Show hidden hosts" };
    let more_items = [
        MenuItem { label: "New host\u{2026}", shortcut: None, enabled: true },
        MenuItem { label: "Check status of all", shortcut: None, enabled: !hosts.is_empty() },
        MenuItem { label: hidden_label, shortcut: None, enabled: true },
        MenuItem { label: "Port forwards", shortcut: None, enabled: true },
    ];
    match context_menu(ui, more, &more_items) {
        Some(0) => action = Some(SidebarAction::NewHost),
        Some(1) => action = Some(SidebarAction::CheckAll),
        Some(2) => action = Some(SidebarAction::ToggleShowHidden),
        Some(3) => action = Some(SidebarAction::OpenForwards),
        _ => {}
    }

    let rows = sb.rows(ctx);
    let before = sb.selected;
    let px = ui.font_px();
    let small = px * 0.85;
    let mouse = ui.input().mouse;
    let mut hovered = None;
    let collapsed = &sb.collapsed;
    let filtering = !sb.filter.trim().is_empty();
    let row_name = |i: usize| match &rows[i] {
        Row::Header(g) => format!("{g}, {}", if collapsed.contains(g) && !filtering { "collapsed" } else { "expanded" }),
        Row::Host(hi) => {
            let h = &hosts[*hi];
            let st = ctx.statuses.get(&h.name).map_or_else(|| "not checked".to_string(), |s| s.label());
            format!("{}, {st}, {}", h.name, h.target())
        }
    };
    let lr = list_named(ui, Id::new("sidebar-list"), list_r, "Host list", rows.len(), ROW_H, &mut sb.selected, row_name, |ui, r, i, _sel| {
        let lh = ui.line_height(FontKind::Ui, px);
        let y = r.y + (r.h - lh) / 2;
        match &rows[i] {
            Row::Header(g) => {
                let arrow = if collapsed.contains(g) && !filtering { "\u{25B8}" } else { "\u{25BE}" };
                let t = format!("{arrow} {g}");
                let at = Point::new(r.x + ui.px(4), y);
                ui.painter().text(at, &t, FontKind::Ui, px, p.muted);
            }
            Row::Host(hi) => {
                let h = &hosts[*hi];
                if r.contains(mouse) {
                    hovered = Some(*hi);
                }
                status_dot(ui, Rect::new(r.x + ui.px(6), r.y, ui.px(8), r.h), ctx.statuses.get(&h.name));
                let x = r.x + ui.px(20);
                let name_w = ui.measure(&h.name, FontKind::Ui, px).ceil() as i32;
                let color = if h.hidden { p.muted } else { p.fg };
                ui.painter().text(Point::new(x, y), &h.name, FontKind::Ui, px, color);
                let mut right = r.right() - ui.px(6);
                if let Some(tag) = h.tags.first() {
                    let tw = ui.measure(tag, FontKind::Ui, small).ceil() as i32 + ui.px(10);
                    if right - tw > x + name_w + ui.px(40) {
                        let chip = Rect::new(right - tw, r.y + ui.px(5), tw, r.h - ui.px(10));
                        let radius = ui.px(4) as f32;
                        ui.painter().rect(chip, p.surface_alt, radius);
                        let sh = ui.line_height(FontKind::Ui, small);
                        let at = Point::new(chip.x + ui.px(5), chip.y + (chip.h - sh) / 2);
                        ui.painter().text(at, tag, FontKind::Ui, small, p.muted);
                        right = chip.x - ui.px(6);
                    }
                }
                let dx = x + name_w + ui.px(10);
                let room = right - dx;
                if room > ui.px(30) && h.hostname != h.name {
                    let shown = elide(ui, &h.hostname, FontKind::Ui, px, room);
                    ui.painter().text(Point::new(dx, y), &shown, FontKind::Ui, px, p.muted);
                }
            }
        }
    });
    sb.hovered = hovered;

    // A clicked header toggles its group instead of staying selected; with
    // the keyboard a header can be selected and Enter toggles it.
    let clicked = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(..)));
    if sb.selected != before
        && clicked
        && let Some(Row::Header(g)) = sb.selected.and_then(|i| rows.get(i))
    {
        if !sb.collapsed.remove(g) {
            sb.collapsed.insert(g.clone());
        }
        sb.selected = before.filter(|&b| b < rows.len() && matches!(rows[b], Row::Host(_)));
    }
    if let Some(i) = lr.activated
        && let Some(Row::Header(g)) = rows.get(i)
        && !sb.collapsed.remove(g)
    {
        sb.collapsed.insert(g.clone());
    }
    if lr.response.focused
        && let Some(Row::Host(h)) = sb.selected.and_then(|i| rows.get(i))
        && ui.input_mut().take_key(&Key::Delete, Modifiers::NONE)
    {
        sb.confirm_delete = Some(*h);
    }
    if let Some(i) = lr.activated
        && let Some(Row::Host(h)) = rows.get(i)
    {
        action = Some(SidebarAction::OpenTerminal(*h));
    }
    let menu = Id::new("sidebar-menu");
    let move_menu = Id::new("sidebar-move-menu");
    if let Some((i, at)) = lr.context
        && let Some(Row::Host(h)) = rows.get(i)
    {
        sb.menu_host = Some(*h);
        open_context_menu(ui, menu, at);
    }
    let host = sb.menu_host.and_then(|h| hosts.get(h));
    let ssh = host.is_some_and(|h| h.source == Source::SshConfig);
    let hidden = host.is_some_and(|h| h.hidden);
    let items = [
        MenuItem { label: "Open terminal", shortcut: Some("Enter"), enabled: true },
        MenuItem { label: "Open file transfer", shortcut: None, enabled: true },
        MenuItem { label: "Port forward\u{2026}", shortcut: None, enabled: true },
        MenuItem { label: "Check status", shortcut: None, enabled: true },
        MenuItem { label: "Edit", shortcut: None, enabled: true },
        MenuItem { label: "Duplicate", shortcut: None, enabled: true },
        MenuItem { label: "Move to file\u{2026}", shortcut: None, enabled: ssh && ctx.files.len() > 1 },
        MenuItem { label: if hidden { "Unhide" } else { "Hide" }, shortcut: None, enabled: true },
        MenuItem { label: "Delete", shortcut: None, enabled: true },
    ];
    if let (Some(choice), Some(h)) = (context_menu(ui, menu, &items), sb.menu_host) {
        match choice {
            0 => action = Some(SidebarAction::OpenTerminal(h)),
            1 => action = Some(SidebarAction::OpenFiles(h)),
            2 => action = Some(SidebarAction::Forward(h)),
            3 => action = Some(SidebarAction::Check(h)),
            4 => action = Some(SidebarAction::Edit(h)),
            5 => action = Some(SidebarAction::Duplicate(h)),
            6 => {
                let at = ui.input().mouse;
                open_context_menu(ui, move_menu, at);
            }
            7 => action = Some(SidebarAction::SetHidden(h, !hidden)),
            8 => sb.confirm_delete = Some(h),
            _ => {}
        }
    }
    let current = host.and_then(|h| h.source_file.clone());
    let names: Vec<String> = ctx.files.iter().map(|f| display_path(f)).collect();
    let move_items: Vec<MenuItem> = names
        .iter()
        .zip(ctx.files)
        .map(|(n, f)| MenuItem { label: n, shortcut: None, enabled: current.as_ref() != Some(f) })
        .collect();
    if let (Some(doc), Some(h)) = (context_menu(ui, move_menu, &move_items), sb.menu_host) {
        action = Some(SidebarAction::MoveTo(h, doc));
    }

    if let Some(h) = sb.confirm_delete {
        match hosts.get(h) {
            None => sb.confirm_delete = None,
            Some(host) => {
                let radius = ui.px(6) as f32;
                ui.painter().rect(confirm_r, p.surface_alt, radius);
                let rows = ui.split_v(confirm_r.inset(ui.px(4)), &[L::Fill, L::Fill], 4);
                let what = match &host.source_file {
                    Some(f) => format!("Delete {} from {}?", host.name, display_path(f)),
                    None => format!("Delete {}?", host.name),
                };
                label(ui, rows[0], &what, Some(p.danger));
                if let Some(n) = ui.a11y_node(Id::new("sidebar-delete-confirm"), termax_ui::a11y::Role::Dialog, &what) {
                    n.live = true;
                }
                let btns = ui.split_h(rows[1], &[L::Frac(0.5), L::Frac(0.5)], 6);
                if button(ui, Id::new("sidebar-del-yes"), btns[0], "Delete").clicked {
                    action = Some(SidebarAction::Delete(h));
                    sb.confirm_delete = None;
                }
                if button(ui, Id::new("sidebar-del-no"), btns[1], "Cancel").clicked {
                    sb.confirm_delete = None;
                }
            }
        }
    }

    let sel = sb.selected_host(ctx);
    if button(ui, Id::new("sidebar-files"), button_r, "\u{21C5} Open file transfer").clicked
        && let Some(h) = sel
    {
        action = Some(SidebarAction::OpenFiles(h));
    }
    action
}
