//! Host sidebar: filter box, "Custom" and "~/.ssh/config" groups, context
//! menu, inline delete confirmation, "Open file transfer" button.

use termax_core::hosts::{HostEntry, Source, fuzzy_score};
use termax_ui::geom::{Point, Rect};
use termax_ui::layout::Size as L;
use termax_ui::text::FontKind;
use termax_ui::ui::{Id, Ui};
use termax_ui::widgets::{
    MenuItem, TextInputOpts, button, context_menu, elide, label, list, open_context_menu, text_input,
};

const ROW_H: i32 = 26;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Custom,
    SshConfig,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Group::Custom => "Custom",
            Group::SshConfig => "~/.ssh/config",
        }
    }

    fn of(h: &HostEntry) -> Group {
        match h.source {
            Source::Custom(_) => Group::Custom,
            Source::SshConfig => Group::SshConfig,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Header(Group),
    Host(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarAction {
    OpenTerminal(usize),
    OpenFiles(usize),
    Edit(usize),
    Duplicate(usize),
    Delete(usize),
}

#[derive(Debug, Default)]
pub struct Sidebar {
    pub filter: String,
    /// Selected row in the flattened list.
    selected: Option<usize>,
    collapsed: [bool; 2],
    menu_host: Option<usize>,
    confirm_delete: Option<usize>,
}

impl Sidebar {
    /// Host index of the selected row.
    pub fn selected_host(&self, hosts: &[HostEntry]) -> Option<usize> {
        let rows = self.rows(hosts);
        match self.selected.and_then(|i| rows.get(i)) {
            Some(Row::Host(h)) => Some(*h),
            _ => None,
        }
    }

    fn rows(&self, hosts: &[HostEntry]) -> Vec<Row> {
        let q = self.filter.trim();
        let mut out = vec![];
        for (gi, g) in [Group::Custom, Group::SshConfig].into_iter().enumerate() {
            let mut members: Vec<(usize, i64)> = hosts
                .iter()
                .enumerate()
                .filter(|(_, h)| Group::of(h) == g)
                .filter_map(|(i, h)| fuzzy_score(q, h).map(|s| (i, s)))
                .collect();
            if members.is_empty() {
                continue;
            }
            if !q.is_empty() {
                members.sort_by(|a, b| b.1.cmp(&a.1));
            }
            out.push(Row::Header(g));
            if q.is_empty() && self.collapsed[gi] {
                continue;
            }
            out.extend(members.into_iter().map(|(i, _)| Row::Host(i)));
        }
        out
    }
}

pub fn sidebar(ui: &mut Ui, rect: Rect, sb: &mut Sidebar, hosts: &[HostEntry]) -> Option<SidebarAction> {
    let p = ui.style().palette;
    ui.painter().rect(rect, p.surface, 0.0);
    let pad = ui.px(8);
    let inner = rect.inset(pad);
    let parts = ui.split_v(inner, &[L::Px(28), L::Fill, L::Px(if sb.confirm_delete.is_some() { 64 } else { 0 }), L::Px(30)], 8);
    let (filter_r, list_r, confirm_r, button_r) = (parts[0], parts[1], parts[2], parts[3]);
    let mut action = None;

    let fr = text_input(ui, Id::new("sidebar-filter"), filter_r, &mut sb.filter, TextInputOpts { password: false, placeholder: "Filter hosts" });
    if fr.changed {
        sb.selected = None;
    }

    let rows = sb.rows(hosts);
    let before = sb.selected;
    let px = ui.font_px();
    let lr = list(ui, Id::new("sidebar-list"), list_r, rows.len(), ROW_H, &mut sb.selected, |ui, r, i, _sel| {
        let lh = ui.line_height(FontKind::Ui, px);
        let y = r.y + (r.h - lh) / 2;
        match rows[i] {
            Row::Header(g) => {
                let gi = g as usize;
                let arrow = if sb.collapsed[gi] && sb.filter.trim().is_empty() { "\u{25B8}" } else { "\u{25BE}" };
                let t = format!("{arrow} {}", g.title());
                let at = Point::new(r.x + ui.px(4), y);
                ui.painter().text(at, &t, FontKind::Ui, px, p.muted);
            }
            Row::Host(hi) => {
                let h = &hosts[hi];
                let x = r.x + ui.px(18);
                let name_w = ui.measure(&h.name, FontKind::Ui, px).ceil() as i32;
                ui.painter().text(Point::new(x, y), &h.name, FontKind::Ui, px, p.fg);
                let detail = h.hostname.clone();
                let dx = x + name_w + ui.px(10);
                let room = r.right() - dx - ui.px(6);
                if room > ui.px(30) && detail != h.name {
                    let shown = elide(ui, &detail, FontKind::Ui, px, room);
                    ui.painter().text(Point::new(dx, y), &shown, FontKind::Ui, px, p.muted);
                }
            }
        }
    });

    // A header row toggles its group instead of staying selected.
    if sb.selected != before
        && let Some(Row::Header(g)) = sb.selected.and_then(|i| rows.get(i))
    {
        sb.collapsed[*g as usize] = !sb.collapsed[*g as usize];
        sb.selected = before.filter(|&b| b < rows.len() && matches!(rows[b], Row::Host(_)));
    }
    if let Some(i) = lr.activated
        && let Some(Row::Host(h)) = rows.get(i)
    {
        action = Some(SidebarAction::OpenTerminal(*h));
    }
    let menu = Id::new("sidebar-menu");
    if let Some((i, at)) = lr.context
        && let Some(Row::Host(h)) = rows.get(i)
    {
        sb.menu_host = Some(*h);
        open_context_menu(ui, menu, at);
    }
    let custom = sb.menu_host.and_then(|h| hosts.get(h)).is_some_and(|h| matches!(h.source, Source::Custom(_)));
    let items = [
        MenuItem { label: "Open terminal", shortcut: Some("Enter"), enabled: true },
        MenuItem { label: "Open file transfer", shortcut: None, enabled: true },
        MenuItem { label: "Edit", shortcut: None, enabled: custom },
        MenuItem { label: "Duplicate", shortcut: None, enabled: custom },
        MenuItem { label: "Delete", shortcut: None, enabled: custom },
    ];
    if let (Some(choice), Some(h)) = (context_menu(ui, menu, &items), sb.menu_host) {
        match choice {
            0 => action = Some(SidebarAction::OpenTerminal(h)),
            1 => action = Some(SidebarAction::OpenFiles(h)),
            2 => action = Some(SidebarAction::Edit(h)),
            3 => action = Some(SidebarAction::Duplicate(h)),
            4 => sb.confirm_delete = Some(h),
            _ => {}
        }
    }

    if let Some(h) = sb.confirm_delete {
        match hosts.get(h) {
            None => sb.confirm_delete = None,
            Some(host) => {
                let radius = ui.px(6) as f32;
                ui.painter().rect(confirm_r, p.surface_alt, radius);
                let rows = ui.split_v(confirm_r.inset(ui.px(4)), &[L::Fill, L::Fill], 4);
                label(ui, rows[0], &format!("Delete {}?", host.name), Some(p.danger));
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

    let sel = sb.selected_host(hosts);
    if button(ui, Id::new("sidebar-files"), button_r, "\u{21C5} Open file transfer").clicked
        && let Some(h) = sel
    {
        action = Some(SidebarAction::OpenFiles(h));
    }
    action
}
