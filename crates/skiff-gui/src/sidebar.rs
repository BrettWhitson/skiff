//! Host sidebar: a search field, sort and "⋯" menu; hosts grouped by source
//! (custom, then each ssh config file) under caption headers with counts;
//! rows with a status dot, the name in semibold, the address in muted mono
//! and tag chips; a context menu beside the pointer; a modal delete
//! confirmation; an empty state when there are no hosts at all; and an
//! "Open file transfer" button at the bottom.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use skiff_core::history::{self, History};
use skiff_core::hosts::{HostEntry, Source, fuzzy_score, visible};
use skiff_core::sshconfig::edit::display_path;
use skiff_core::state::SortMode;
use skiff_core::status::Status;
use skiff_ui::geom::{Point, Rect};
use skiff_ui::icons::Icon;
use skiff_ui::input::{Event, Key, Modifiers};
use skiff_ui::layout::Size as L;
use skiff_ui::style::{TextRole, tokens};
use skiff_ui::text::FontKind;
use skiff_ui::ui::{Id, Ui};
use skiff_ui::widgets::{
    ButtonKind, ButtonOpts, Dialog, DialogButton, DialogResult, Elide, EmptyState, MenuItem, TextInputOpts, Tone, badge, badge_width,
    button_with, context_menu, dialog, dialog_dismiss, dropdown_labeled, elide_mode, empty_state, icon_button, list_named,
    open_context_menu, text_input,
};

/// Height of one list row (group header or host), logical px.
const ROW_H: i32 = tokens::ROW_H;

/// One row of the flattened host list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    /// A group header: the group's name (`Custom` or a config path) and
    /// how many hosts it lists.
    Header(String, usize),
    /// A host, as an index into `SidebarCtx::hosts`.
    Host(usize),
}

/// What the user asked for. Hosts are named, not indexed: the host list
/// can be rebuilt (an outside edit, a save) between a click and the action,
/// or while a menu or confirmation is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    /// Open a terminal tab to the host.
    OpenTerminal(String),
    /// Open a file-transfer tab to the host.
    OpenFiles(String),
    /// Open a file-transfer tab between two local panes (no host selected).
    OpenLocalFiles,
    /// Open the Forwards tab with a new forward form for the host.
    Forward(String),
    /// Probe the host's status.
    Check(String),
    /// Probe every host's status.
    CheckAll,
    /// Open the host in the settings' connection editor.
    Edit(String),
    /// Add a copy of the host next to it.
    Duplicate(String),
    /// Move a host into ssh config file `.1`.
    MoveTo(String, PathBuf),
    /// Hide (`true`) or unhide (`false`) the host.
    SetHidden(String, bool),
    /// Delete the host (already confirmed in the sidebar).
    Delete(String),
    /// Change the host-list sort order.
    SetSort(SortMode),
    /// Flip whether hidden hosts are listed.
    ToggleShowHidden,
    /// Open the port-forward manager tab.
    OpenForwards,
    /// Start creating a new host.
    NewHost,
    /// Start listing the hosts of `~/.ssh/config` (turn `load_ssh_config` on).
    ImportSshConfig,
}

/// What the sidebar shows, borrowed from the app.
pub struct SidebarCtx<'a> {
    /// Every known host, custom and from ssh config.
    pub hosts: &'a [HostEntry],
    /// Last status probe result per host name.
    pub statuses: &'a HashMap<String, Status>,
    /// Connection history, for "Recent" sorting and "last connected".
    pub history: &'a History,
    /// Current sort order (used when no filter is typed).
    pub sort: SortMode,
    /// Whether hidden hosts are listed.
    pub show_hidden: bool,
    /// The ssh config files (for "Move to file…").
    pub files: &'a [PathBuf],
    /// Whether `~/.ssh/config` is read (`settings.load_ssh_config`); the
    /// empty state offers to import it when not.
    pub ssh_config_on: bool,
    /// The command palette's shortcut, for the empty state's hint.
    pub palette_key: Option<String>,
}

/// The sidebar's own state, kept across frames.
#[derive(Debug, Default)]
pub struct Sidebar {
    /// Text in the search box; non-empty switches to fuzzy-ranked results.
    pub filter: String,
    /// Selected row in the flattened list.
    selected: Option<usize>,
    /// The selected host's name, so the selection follows it when the
    /// rows change.
    selected_name: Option<String>,
    /// Names of collapsed groups (ignored while filtering).
    collapsed: HashSet<String>,
    /// The host the open context menu (and its "Move to file" submenu) acts on.
    menu_host: Option<String>,
    /// Host awaiting the delete confirmation dialog.
    pub confirm_delete: Option<String>,
    /// The host under the mouse (the status bar describes it).
    pub hovered: Option<String>,
}

/// The group a host is listed under: "Custom", or the ssh config file it
/// came from.
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
    /// Name of the selected host.
    pub fn selected_host(&self, ctx: &SidebarCtx) -> Option<String> {
        self.selected_name.clone().filter(|n| ctx.hosts.iter().any(|h| &h.name == n))
    }

    /// The flattened list: a header per non-empty group followed by its
    /// hosts (unless collapsed), sorted by the sort mode or by fuzzy score.
    fn rows(&self, ctx: &SidebarCtx) -> Vec<Row> {
        let q = self.filter.trim();
        let mut shown = visible(ctx.hosts, ctx.show_hidden);
        // No filter: the chosen sort. Filter: only matches, best score first.
        if q.is_empty() {
            history::order(ctx.hosts, &mut shown, ctx.sort, ctx.history);
        } else {
            let mut scored: Vec<(usize, i64)> = shown.into_iter().filter_map(|i| fuzzy_score(q, &ctx.hosts[i]).map(|s| (i, s))).collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.1));
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
            // Filtering always expands groups so no match is hidden.
            let collapsed = q.is_empty() && self.collapsed.contains(&g);
            out.push(Row::Header(g, members.len()));
            if !collapsed {
                out.extend(members.into_iter().map(Row::Host));
            }
        }
        out
    }
}

/// A host's status as a dot colour: green online, red offline, amber for
/// not-ssh or a jump host only, accent while checking, grey unchecked.
fn dot_color(ui: &Ui, s: Option<&Status>) -> skiff_ui::color::Color {
    let p = ui.style().palette;
    match s {
        Some(Status::Online { .. }) => p.success,
        Some(Status::Offline(_)) => p.danger,
        Some(Status::NotSsh) => p.warning,
        Some(Status::Via(_, inner)) if matches!(**inner, Status::Online { .. }) => p.success,
        Some(Status::Via(..)) => p.warning,
        Some(Status::Checking) => p.accent,
        _ => p.muted,
    }
}

/// Draws a host's status dot centred in `r`: filled when the host itself
/// was probed, a ring when only its jump host was (or it is unchecked).
fn status_dot(ui: &mut Ui, r: Rect, s: Option<&Status>) {
    let d = ui.px(tokens::DOT);
    let dot = Rect::new(r.x + (r.w - d) / 2, r.y + (r.h - d) / 2, d, d);
    let c = dot_color(ui, s);
    let ring = matches!(s, None | Some(Status::Unchecked | Status::Via(..) | Status::Unknown(_)));
    if ring {
        let bw = ui.px(if matches!(s, Some(Status::Via(..))) { 2 } else { 1 }).max(1) + 1;
        ui.painter().border(dot, c, bw.min(d / 2), d as f32 / 2.0);
    } else {
        ui.painter().rect(dot, c, d as f32 / 2.0);
    }
}

/// Draws one host row's contents in `r` (the list painted the background).
fn host_row(ui: &mut Ui, r: Rect, h: &HostEntry, status: Option<&Status>, selected: bool) {
    let p = ui.style().palette;
    let pad = ui.px(tokens::SPACE_SM);
    // A selected row is marked by shape too: an accent bar on its left.
    if selected {
        let w = ui.px(3).max(2);
        let bar = Rect::new(r.x, r.y + ui.px(4), w, r.h - ui.px(8));
        ui.painter().rect(bar, p.accent, w as f32 / 2.0);
    }
    status_dot(ui, Rect::new(r.x + pad, r.y, ui.px(tokens::DOT + 4), r.h), status);
    let (bold, px) = ui.text_style(TextRole::Strong);
    let lh = ui.line_height(bold, px);
    let x = r.x + pad + ui.px(tokens::DOT + 4) + ui.px(tokens::SPACE_SM);
    let y = r.y + (r.h - lh) / 2;
    let mut right = r.right() - pad;
    // Tag chips at the right edge, as many as leave the name room.
    let name_w = ui.measure(&h.name, bold, px).ceil() as i32;
    for tag in h.tags.iter().take(2) {
        let tw = badge_width(ui, tag);
        if right - tw < x + name_w.min(ui.px(96)) + ui.px(tokens::SPACE_LG) {
            break;
        }
        badge(ui, Rect::new(right - tw, r.y, tw, r.h), tag, Tone::Neutral);
        right -= tw + ui.px(tokens::SPACE_XS);
    }
    let color = if h.hidden { p.muted } else { p.fg };
    let name = elide_mode(ui, &h.name, bold, px, right - x, Elide::End);
    let drawn = ui.painter().text(Point::new(x, y), &name, bold, px, color);
    // The address, muted mono, in whatever room is left: whole, or elided
    // in the middle when there is room for a useful part of it (skipped
    // when it equals the name).
    let dx = x + drawn + ui.px(tokens::SPACE_SM);
    let room = right - ui.px(tokens::SPACE_XS) - dx;
    let (_, cpx) = ui.text_style(TextRole::Caption);
    let mpx = cpx + ui.scale();
    let fits = ui.measure(&h.hostname, FontKind::Mono, mpx).ceil() as i32 <= room;
    if (fits || room >= ui.px(72)) && h.hostname != h.name {
        let shown = elide_mode(ui, &h.hostname, FontKind::Mono, mpx, room, Elide::Middle);
        let mlh = ui.line_height(FontKind::Mono, mpx);
        ui.painter().text(Point::new(dx, y + lh - mlh), &shown, FontKind::Mono, mpx, p.muted);
    }
}

/// Draws a group header in `r`: a chevron, the caption (`CUSTOM`, or the
/// config path elided at its start, case kept) and the host count.
fn header_row(ui: &mut Ui, r: Rect, name: &str, count: usize, collapsed: bool) {
    let p = ui.style().palette;
    let (_, cpx) = ui.text_style(TextRole::Caption);
    let font = FontKind::UiSemibold;
    let lh = ui.line_height(font, cpx);
    let pad = ui.px(tokens::SPACE_SM);
    let icon = ui.px(tokens::ICON - 4);
    let chevron = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
    ui.painter().icon(Rect::new(r.x + pad, r.y + (r.h - icon) / 2, icon, icon), chevron, p.muted);
    let n = count.to_string();
    let nw = ui.measure(&n, FontKind::Ui, cpx).ceil() as i32;
    let y = r.y + (r.h - lh) / 2 + ui.px(1);
    ui.painter().text(Point::new(r.right() - pad - nw, y), &n, FontKind::Ui, cpx, p.muted);
    let x = r.x + pad + icon + ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
    let label = if name == "Custom" { "CUSTOM".to_string() } else { name.to_string() };
    let shown = elide_mode(ui, &label, font, cpx, r.right() - pad - nw - ui.px(tokens::SPACE_SM) - x, Elide::Start);
    ui.painter().text(Point::new(x, y), &shown, font, cpx, p.muted);
}

/// The context menu of a host: the actions in groups, Delete last in red
/// (separators count as indices).
fn host_menu(ssh: bool, files: usize, hidden: bool) -> [MenuItem<'static>; 11] {
    [
        MenuItem::new("Open terminal").icon(Icon::Terminal).shortcut("Enter"),
        MenuItem::new("Open file transfer").icon(Icon::Transfer),
        MenuItem::new("Port forward\u{2026}").icon(Icon::Forward),
        MenuItem::SEPARATOR,
        MenuItem::new("Check status").icon(Icon::Refresh),
        MenuItem::new("Edit").icon(Icon::Edit),
        MenuItem::new("Duplicate").icon(Icon::Copy),
        MenuItem::new("Move to file\u{2026}").icon(Icon::Folder).enabled(ssh && files > 1),
        if hidden { MenuItem::new("Unhide").icon(Icon::Eye) } else { MenuItem::new("Hide").icon(Icon::EyeOff) },
        MenuItem::SEPARATOR,
        MenuItem::new("Delete").icon(Icon::Trash).shortcut("Del").danger(),
    ]
}

/// Draws the sidebar into `rect` and handles its input; returns what the
/// user asked for this frame, if anything.
pub fn sidebar(ui: &mut Ui, rect: Rect, sb: &mut Sidebar, ctx: &SidebarCtx) -> Option<SidebarAction> {
    let hosts = ctx.hosts;
    let p = ui.style().palette;
    ui.painter().rect(rect, p.surface, 0.0);
    let pad = ui.px(tokens::SPACE_SM);
    let inner = rect.inset(pad);
    let parts = ui.split_v(inner, &[L::Px(tokens::CONTROL_H), L::Fill, L::Px(tokens::CONTROL_H)], tokens::SPACE_SM);
    let (head_r, list_r, button_r) = (parts[0], parts[1], parts[2]);
    let mut action = None;

    // Header: search field, sort dropdown and the "⋯" menu button.
    let head = ui.split_h(head_r, &[L::Fill, L::Px(84), L::Px(tokens::CONTROL_H)], tokens::SPACE_XS);
    let opts = TextInputOpts { placeholder: "Filter hosts", label: "Filter hosts", icon: Some(Icon::Search), ..Default::default() };
    let fr = text_input(ui, Id::new("sidebar-filter"), head[0], &mut sb.filter, opts);
    // A new filter changes the rows, so drop the selection.
    if fr.changed {
        sb.selected = None;
        sb.selected_name = None;
    }
    let sorts = [SortMode::Name, SortMode::Recent];
    let mut si = sorts.iter().position(|s| *s == ctx.sort).unwrap_or(0);
    if dropdown_labeled(ui, Id::new("sidebar-sort"), head[1], "Sort hosts by", &["Name", "Recent"], &mut si) {
        action = Some(SidebarAction::SetSort(sorts[si]));
    }
    let more = Id::new("sidebar-more-menu");
    if icon_button(ui, Id::new("sidebar-more"), head[2], Icon::Ellipsis, "More host actions", ButtonKind::Ghost).clicked {
        open_context_menu(ui, more, Point::new(head[2].x, head[2].bottom() + ui.px(tokens::SPACE_2XS)));
    }
    let more_items = [
        MenuItem::new("New host\u{2026}").icon(Icon::Plus),
        MenuItem::new("Check status of all").icon(Icon::Refresh).enabled(!hosts.is_empty()),
        if ctx.show_hidden {
            MenuItem::new("Hide hidden hosts").icon(Icon::EyeOff)
        } else {
            MenuItem::new("Show hidden hosts").icon(Icon::Eye)
        },
        MenuItem::SEPARATOR,
        MenuItem::new("Port forwards").icon(Icon::Forward),
    ];
    match context_menu(ui, more, &more_items) {
        Some(0) => action = Some(SidebarAction::NewHost),
        Some(1) => action = Some(SidebarAction::CheckAll),
        Some(2) => action = Some(SidebarAction::ToggleShowHidden),
        Some(4) => action = Some(SidebarAction::OpenForwards),
        _ => {}
    }

    // No hosts at all: say how to get some instead of an empty list.
    if hosts.is_empty() {
        let hint = ctx.palette_key.as_ref().map(|k| format!("or press {k} for the command palette"));
        let e = EmptyState {
            icon: Some(Icon::Host),
            title: "No hosts yet",
            body: if ctx.ssh_config_on {
                "Add a host, or add Host entries to ~/.ssh/config; they show up here."
            } else {
                "Add a host, or list the hosts of your ~/.ssh/config."
            },
            action: Some("Add host"),
            secondary: (!ctx.ssh_config_on).then_some("Import ~/.ssh/config"),
            hint: hint.as_deref(),
        };
        let r = empty_state(ui, Id::new("sidebar-empty"), list_r, &e);
        if r.action {
            action = Some(SidebarAction::NewHost);
        }
        if r.secondary {
            action = Some(SidebarAction::ImportSshConfig);
        }
        return action;
    }

    let rows = sb.rows(ctx);
    let host_row_at = |name: &str| rows.iter().position(|r| matches!(r, Row::Host(h) if hosts[*h].name == name));
    // Keep the selection on the same host when the rows changed.
    if let Some(name) = &sb.selected_name {
        sb.selected = host_row_at(name);
    }
    let before = sb.selected;
    let mouse = ui.input().mouse;
    let mut hovered = None;
    let mut drawn: Vec<(usize, Rect)> = vec![];
    let collapsed = &sb.collapsed;
    let filtering = !sb.filter.trim().is_empty();
    // Accessible name per row; the list widget draws each row with the closure below.
    let row_name = |i: usize| match &rows[i] {
        Row::Header(g, n) => {
            format!(
                "{g}, {n} host{}, {}",
                if *n == 1 { "" } else { "s" },
                if collapsed.contains(g) && !filtering { "collapsed" } else { "expanded" }
            )
        }
        Row::Host(hi) => {
            let h = &hosts[*hi];
            let st = ctx.statuses.get(&h.name).map_or_else(|| "not checked".to_string(), |s| s.label());
            format!("{}, {st}, {}", h.name, h.target())
        }
    };
    let lr =
        list_named(ui, Id::new("sidebar-list"), list_r, "Host list", rows.len(), ROW_H, &mut sb.selected, row_name, |ui, r, i, sel| {
            drawn.push((i, r));
            match &rows[i] {
                Row::Header(g, n) => header_row(ui, r, g, *n, collapsed.contains(g) && !filtering),
                Row::Host(hi) => {
                    let h = &hosts[*hi];
                    if r.contains(mouse) {
                        hovered = Some(h.name.clone());
                    }
                    host_row(ui, r, h, ctx.statuses.get(&h.name), sel);
                }
            }
        });
    sb.hovered = hovered;
    if rows.is_empty() {
        let (font, px) = ui.text_style(TextRole::Body);
        let msg = if filtering { "No hosts match" } else { "All hosts are hidden" };
        ui.painter().text(Point::new(list_r.x + pad, list_r.y + pad), msg, font, px, p.muted);
    }

    // A clicked header toggles its group instead of staying selected; with
    // the keyboard a header can be selected and Enter toggles it.
    let clicked = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(..)));
    if sb.selected != before
        && clicked
        && let Some(Row::Header(g, _)) = sb.selected.and_then(|i| rows.get(i))
    {
        if !sb.collapsed.remove(g) {
            sb.collapsed.insert(g.clone());
        }
        sb.selected = before.filter(|&b| b < rows.len() && matches!(rows[b], Row::Host(_)));
    }
    // Remember the selected host by name; a header selection has none.
    sb.selected_name = match sb.selected.and_then(|i| rows.get(i)) {
        Some(Row::Host(h)) => Some(hosts[*h].name.clone()),
        _ => None,
    };
    // Enter on a header toggles its group.
    if let Some(i) = lr.activated
        && let Some(Row::Header(g, _)) = rows.get(i)
        && !sb.collapsed.remove(g)
    {
        sb.collapsed.insert(g.clone());
    }
    // Delete on a focused host row asks for confirmation.
    if lr.response.focused
        && let Some(Row::Host(h)) = sb.selected.and_then(|i| rows.get(i))
        && ui.input_mut().take_key(&Key::Delete, Modifiers::NONE)
    {
        sb.confirm_delete = Some(hosts[*h].name.clone());
    }
    if let Some(i) = lr.activated
        && let Some(Row::Host(h)) = rows.get(i)
    {
        action = Some(SidebarAction::OpenTerminal(hosts[*h].name.clone()));
    }
    // Right-click on a host opens its menu beside the pointer, just below
    // the row so the row's text stays readable; the host is remembered by name.
    let menu = Id::new("sidebar-menu");
    let move_menu = Id::new("sidebar-move-menu");
    if let Some((i, at)) = lr.context
        && let Some(Row::Host(h)) = rows.get(i)
    {
        sb.menu_host = Some(hosts[*h].name.clone());
        let below = drawn.iter().find(|(k, _)| *k == i).map_or(at.y, |(_, r)| r.bottom());
        open_context_menu(ui, menu, Point::new(at.x + ui.px(tokens::SPACE_2XS), below.max(at.y)));
    }
    let host = sb.menu_host.as_ref().and_then(|n| hosts.iter().find(|h| &h.name == n));
    let ssh = host.is_some_and(|h| h.source == Source::SshConfig);
    let hidden = host.is_some_and(|h| h.hidden);
    let items = host_menu(ssh, ctx.files.len(), hidden);
    if let (Some(choice), Some(h)) = (context_menu(ui, menu, &items), host.map(|h| h.name.clone())) {
        match choice {
            0 => action = Some(SidebarAction::OpenTerminal(h)),
            1 => action = Some(SidebarAction::OpenFiles(h)),
            2 => action = Some(SidebarAction::Forward(h)),
            4 => action = Some(SidebarAction::Check(h)),
            5 => action = Some(SidebarAction::Edit(h)),
            6 => action = Some(SidebarAction::Duplicate(h)),
            7 => {
                let at = ui.input().mouse;
                open_context_menu(ui, move_menu, at);
            }
            8 => action = Some(SidebarAction::SetHidden(h, !hidden)),
            10 => sb.confirm_delete = Some(h),
            _ => {}
        }
    }
    // "Move to file" submenu: every config file except the host's current one.
    let current = host.and_then(|h| h.source_file.clone());
    let names: Vec<String> = ctx.files.iter().map(|f| display_path(f)).collect();
    let move_items: Vec<MenuItem> =
        names.iter().zip(ctx.files).map(|(n, f)| MenuItem::new(n).icon(Icon::File).enabled(current.as_ref() != Some(f))).collect();
    if let (Some(doc), Some(h)) = (context_menu(ui, move_menu, &move_items), host.map(|h| h.name.clone())) {
        action = Some(SidebarAction::MoveTo(h, ctx.files[doc].clone()));
    }

    // The delete confirmation, a modal dialog; dropped if the host
    // disappeared meanwhile.
    if let Some(h) = sb.confirm_delete.clone() {
        match hosts.iter().find(|x| x.name == h) {
            None => {
                sb.confirm_delete = None;
                dialog_dismiss(ui, Id::new("sidebar-delete-dialog"));
            }
            Some(host) => {
                let title = format!("Delete {}?", host.name);
                let body = match &host.source_file {
                    Some(f) => format!(
                        "Its Host block will be removed from {}. A backup of the file is kept in skiff's backups folder.",
                        display_path(f)
                    ),
                    None => "The connection will be removed from skiff's config.toml.".to_string(),
                };
                let buttons = [
                    DialogButton { label: "Cancel", kind: ButtonKind::Secondary },
                    DialogButton { label: "Delete", kind: ButtonKind::Danger },
                ];
                // Cancel has focus first: a reflexive Enter keeps the host.
                let d = Dialog { title: &title, body: &body, buttons: &buttons, initial: 0, icon: Some(Icon::Trash), ..Default::default() };
                match dialog(ui, Id::new("sidebar-delete-dialog"), &d) {
                    Some(DialogResult::Button(1)) => {
                        action = Some(SidebarAction::Delete(h));
                        sb.confirm_delete = None;
                    }
                    Some(_) => sb.confirm_delete = None,
                    None => {}
                }
            }
        }
    }

    // A ghost button: opens files for the selected host, or two local panes.
    let sel = sb.selected_host(ctx);
    let tip = match &sel {
        Some(h) => format!("Local \u{2194} {h}"),
        None => "Local \u{2194} Local (select a host to connect)".into(),
    };
    let opts = ButtonOpts { kind: ButtonKind::Ghost, icon: Some(Icon::Transfer), tooltip: Some(&tip), ..Default::default() };
    if button_with(ui, Id::new("sidebar-files"), button_r, "Open file transfer", opts).clicked {
        action = Some(match sel {
            Some(h) => SidebarAction::OpenFiles(h),
            None => SidebarAction::OpenLocalFiles,
        });
    }
    action
}
