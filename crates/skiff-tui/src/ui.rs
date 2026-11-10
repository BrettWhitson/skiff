//! Rendering for the TUI: draws the tab bar, the active tab (host list,
//! terminal with optional split, or SFTP panes), the footer, the which-key
//! list after the prefix and any open modal (over a dimmed backdrop) from
//! the `App` state each frame. Also holds the split-pane geometry that the
//! mouse handling in `app` shares.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ratatui::layout::{Alignment, Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Table, TableState, Wrap,
};

use ratatui::Frame;
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::app::{App, ConfirmAction, Modal, SplitDir, TabKind};
use crate::forms::{ForwardForm, FwdRow, Row as EdRow};
use crate::input::TextInput;
use crate::keys::Action;
use crate::sftp_tab::{Pane, SftpTab, Side, Status, TransferState, human_size};
use crate::tui_theme::{ASCII_BORDER, Theme};
use crate::widgets::{Hint, elide_middle, fit, fitted_line, hint, hint_spans, hints_width, width, wrap_items};
use skiff_core::askpass::AskKind;
use skiff_core::panes;
use skiff_core::status::Status as HostStatus;
use skiff_core::terminal::TerminalSession;

/// Draws one whole frame. Also records layout facts (tab label columns, pane
/// areas, terminal size) back into `app` for mouse handling and resizing.
pub fn draw(f: &mut Frame, app: &mut App) {
    app.theme.plain = app.plain;
    app.theme.mono = app.mono;
    app.theme.ascii = app.cfg.settings.tui_ascii_borders;
    let area = f.area();
    f.render_widget(Block::new().style(app.theme.base()), area);
    let [tabs_area, body, status_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(area);

    draw_tab_bar(f, app, tabs_area);
    let active = app.active;
    match &app.tabs[active].kind {
        TabKind::Hosts => draw_hosts(f, app, body),
        TabKind::Terminal(_) => draw_terminal(f, app, body),
        TabKind::Sftp(_) => draw_sftp(f, app, body),
    }
    draw_status(f, app, status_area);
    // Forward states are computed up front: `forward_state` borrows `app`
    // mutably, while the modal is drawn from a shared borrow.
    let states: Vec<String> = match app.modal {
        Some(Modal::Forwards { .. }) => (0..app.cfg.forwards.len()).map(|i| app.forward_state(i)).collect(),
        _ => vec![],
    };
    match &app.modal {
        // The find bar lives on the footer; the screen stays as it is.
        Some(Modal::Search { .. }) | None => {}
        Some(_) => backdrop(f, &app.theme, area),
    }
    if app.modal.is_none() && app.which_key_due(Instant::now()) {
        draw_which_key(f, app, body);
    }
    draw_modal(f, app, area, &states);
}

/// The standard panel: a rounded (or ASCII) border, accent when focused;
/// in plain mode no box at all, only its title line.
fn panel<'a>(t: &Theme, focused: bool) -> Block<'a> {
    if t.plain {
        return Block::new().style(t.base());
    }
    Block::bordered().border_set(t.border_set()).border_style(t.border(focused)).style(t.base())
}

/// A panel title's style: bold accent when focused, muted otherwise.
fn title_style(t: &Theme, focused: bool) -> Style {
    if focused { t.title() } else { t.muted() }
}

/// Dims everything already drawn in `area`, as the backdrop of a dialog.
fn backdrop(f: &mut Frame, t: &Theme, area: Rect) {
    let buf = f.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            let style = t.backdrop(cell.style());
            cell.set_style(style);
        }
    }
}

/// Draws the top row: the app name, then one label per tab with its number
/// and a status mark (`●` connected, `◌` connecting, `✗` failed or closed,
/// `?` waiting on an ssh prompt). Stores each label's columns in
/// `app.tab_cols` so clicks can pick a tab.
fn draw_tab_bar(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let brand = " Skiff ";
    let mut spans = vec![Span::styled(brand, t.title()), Span::raw(" ")];
    let mut x = area.x + width(brand) as u16 + 1;
    let mut cols = vec![];
    let waiting = app.waiting_tabs();
    let active = app.active;
    // One label per tab; `x` tracks the screen column for the click map.
    for (i, tab) in app.tabs.iter_mut().enumerate() {
        let asks = waiting.contains(&i);
        let alive = match &mut tab.kind {
            TabKind::Terminal(s) => s.is_alive(),
            _ => true,
        };
        let mark: Option<(&str, Style)> = match &tab.kind {
            _ if asks => Some(("?", t.warning())),
            TabKind::Hosts => None,
            TabKind::Terminal(_) if alive => (!t.plain).then(|| (t.glyph("●", "*"), t.success())),
            TabKind::Terminal(_) => Some((if t.plain { "closed" } else { t.glyph("✗", "x") }, t.error())),
            TabKind::Sftp(s) => match s.status {
                Status::Connecting => Some((if t.plain { "connecting" } else { t.glyph("◌", "~") }, t.muted())),
                Status::Connected => (!t.plain).then(|| (t.glyph("●", "*"), t.success())),
                Status::Failed(_) => Some((if t.plain { "failed" } else { t.glyph("✗", "x") }, t.error())),
            },
        };
        let num = format!(" {} ", i + 1);
        let name = format!("{} ", tab.title);
        let mark_text = mark.map(|(m, _)| format!("{m} ")).unwrap_or_default();
        let w = (width(&num) + width(&mark_text) + width(&name)) as u16;
        cols.push((x, x + w));
        x += w + 1;
        if i == active {
            spans.push(Span::styled(format!("{num}{mark_text}{name}"), t.badge()));
        } else {
            let text = if asks { t.warning() } else { t.muted() };
            spans.push(Span::styled(num, text));
            if let Some((m, st)) = mark {
                spans.push(Span::styled(format!("{m} "), st));
            }
            spans.push(Span::styled(name, text));
        }
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(t.base()), area);
    app.tab_cols = cols;
}

// ---------------------------------------------------------------- footer

/// A shortcut for the footer: a key after the prefix (the prefix group), or
/// a chord of its own (the tab's group).
enum Shortcut {
    AfterPrefix(Hint),
    Direct(Hint),
}

/// `action`'s footer hint labelled `label` at `rank`; `None` when unbound.
fn shortcut(app: &App, action: Action, label: &str, rank: u8) -> Option<Shortcut> {
    let k = &app.keys;
    if let Some(key) = k.after_prefix(action) {
        return Some(Shortcut::AfterPrefix(hint(key, label, rank)));
    }
    let shown = k.show(action);
    (shown != "unbound").then(|| Shortcut::Direct(hint(shown, label, rank)))
}

/// The tab-switching hint at `rank`.
fn tabs_shortcut(app: &App, rank: u8) -> Shortcut {
    match app.keys.tabs_after_prefix() {
        Some(k) => Shortcut::AfterPrefix(hint(k, "tab", rank)),
        None => Shortcut::Direct(hint(app.keys.show_tabs(), "tab", rank)),
    }
}

/// The active tab's footer hints: (keys of its own, keys after the prefix).
/// Rank 0 (`? help`) always stays; the rest drop by rank on narrow screens.
pub fn footer_hints(app: &App) -> (Vec<Hint>, Vec<Hint>) {
    let (mut local, mut after) = (vec![], vec![]);
    let mut add = |s: Option<Shortcut>| match s {
        Some(Shortcut::AfterPrefix(h)) => after.push(h),
        Some(Shortcut::Direct(h)) => local.push(h),
        None => {}
    };
    let tab = &app.tabs[app.active];
    match &tab.kind {
        TabKind::Hosts if app.filtering => {
            return (vec![hint("Enter", "connect", 1), hint("↑↓", "move", 2), hint("Esc", "clear", 0)], vec![]);
        }
        // Nothing listed: only the keys that can change that.
        TabKind::Hosts if app.hosts.is_empty() => {
            for h in [hint("n", "new host", 1), hint("r", "reload", 2), hint("q", "quit", 3), hint("?", "help", 0)] {
                add(Some(Shortcut::Direct(h)));
            }
            add(Some(tabs_shortcut(app, 4)));
        }
        TabKind::Hosts => {
            for (k, l, r) in [
                ("Enter", "ssh", 1),
                ("s", "sftp", 2),
                ("/", "filter", 4),
                ("n", "new", 5),
                ("e", "edit", 6),
                ("q", "quit", 7),
                ("f", "forward", 8),
                ("d", "delete", 9),
                ("p", "status", 10),
                ("o", "sort", 11),
                ("H", "hidden", 12),
                ("?", "help", 0),
            ] {
                add(Some(Shortcut::Direct(hint(k, l, r))));
            }
            add(Some(tabs_shortcut(app, 3)));
            add(shortcut(app, Action::Theme, "theme", 13));
        }
        TabKind::Terminal(_) => {
            add(Some(tabs_shortcut(app, 1)));
            add(shortcut(app, Action::CloseTab, "close", 2));
            add(shortcut(app, Action::Find, "find", 4));
            if tab.split.is_some() {
                add(shortcut(app, Action::FocusNextPane, "other pane", 5));
            } else {
                add(shortcut(app, Action::SplitRight, "split", 5));
            }
            add(shortcut(app, Action::Snippets, "snippets", 6));
            add(shortcut(app, Action::Help, "help", 0));
            add(Some(Shortcut::Direct(hint("Shift+PgUp", "scroll", 7))));
        }
        TabKind::Sftp(_) => {
            for (k, l, r) in [
                ("Enter", "open", 1),
                ("Tab", "pane", 2),
                ("c", "copy", 3),
                ("Bksp", "up", 5),
                ("d", "delete", 6),
                ("q", "close", 7),
                ("m", "mkdir", 8),
                ("R", "rename", 9),
                ("g", "go to", 10),
                ("r", "refresh", 11),
                (".", "hidden", 12),
                ("X", "cancel copy", 13),
                ("?", "help", 0),
            ] {
                add(Some(Shortcut::Direct(hint(k, l, r))));
            }
            add(Some(tabs_shortcut(app, 4)));
        }
    }
    (local, after)
}

/// The prefix group's badge text (` Alt+O ▸ `).
fn prefix_badge(t: &Theme, prefix: &str) -> String {
    format!(" {prefix} {} ", t.glyph("▸", ">"))
}

/// The key-hint footer in `max` cells: the tab's own keys on the left and,
/// on the right behind a badge naming the prefix, the keys pressed after it.
/// Low-ranked hints drop until both fit.
pub fn hint_footer(t: &Theme, prefix: Option<&str>, local: &[Hint], after: &[Hint], max: usize) -> Line<'static> {
    let badge = prefix.map(|p| prefix_badge(t, p)).unwrap_or_default();
    let all: Vec<Hint> = local.iter().chain(after).cloned().collect();
    let n = local.len();
    let widths = |shown: &[bool]| {
        let lw = hints_width(all[..n].iter().zip(&shown[..n]).filter(|x| *x.1).map(|x| x.0));
        let after: Vec<&Hint> = all[n..].iter().zip(&shown[n..]).filter(|x| *x.1).map(|x| x.0).collect();
        let aw = if after.is_empty() { 0 } else { width(&badge) + 1 + hints_width(after) };
        (lw, aw)
    };
    let total = |shown: &[bool]| {
        let (lw, aw) = widths(shown);
        1 + lw + if lw > 0 && aw > 0 { 3 } else { 0 } + aw + 1
    };
    let shown = fit(&all, max, total);
    let (lw, aw) = widths(&shown);
    let mut spans = vec![Span::raw(" ")];
    spans.extend(hint_spans(t, all[..n].iter().zip(&shown[..n]).filter(|x| *x.1).map(|x| x.0)));
    if aw > 0 {
        // Right-aligned after the tab's own keys; first when there are none.
        if lw > 0 {
            spans.push(Span::raw(" ".repeat(max.saturating_sub(1 + lw + aw + 1).max(3))));
        }
        spans.push(Span::styled(badge, t.badge()));
        spans.push(Span::raw(" "));
        spans.extend(hint_spans(t, all[n..].iter().zip(&shown[n..]).filter(|x| *x.1).map(|x| x.0)));
    }
    Line::from(spans)
}

/// The footer while the prefix waits for its key: a `PREFIX` badge, then
/// the keys that can follow (as many as fit) and `Esc cancel`.
fn prefix_footer(app: &App, max: usize) -> Line<'static> {
    let t = &app.theme;
    let badge = " PREFIX ";
    let (_, mut after) = footer_hints(app);
    after.retain(|h| h.label != "help" && h.label != "theme");
    for h in after.iter_mut().filter(|h| h.label == "tab") {
        h.rank = 1;
    }
    let close = (app.active > 0).then_some((Action::CloseTab, "close", 2));
    for (a, label, rank) in close.into_iter().chain([(Action::Help, "help", 3), (Action::Quit, "quit", 9)]) {
        if let (Some(key), false) = (app.keys.after_prefix(a), after.iter().any(|h| h.label == label)) {
            after.push(hint(key, label, rank));
        }
    }
    // Shown in rank order, which is also the order of importance.
    after.sort_by_key(|h| h.rank);
    after.push(hint("Esc", "cancel", 0));
    let head = format!(" {} {} ", app.keys.show(Action::Prefix), t.glyph("▸", ">"));
    let room = max.saturating_sub(1 + width(badge) + width(&head) + 1);
    let shown = fit(&after, room, |s| hints_width(after.iter().zip(s).filter(|x| *x.1).map(|x| x.0)));
    let mut spans = vec![Span::raw(" "), Span::styled(badge, t.alert_badge()), Span::styled(head, t.muted())];
    spans.extend(hint_spans(t, after.iter().zip(&shown).filter(|x| *x.1).map(|x| x.0)));
    Line::from(spans)
}

/// Draws the bottom line. In priority order: the find bar while searching,
/// the pending prefix, an error (plus any security-key notice), a notice, an
/// info message, a pointer to another tab waiting for an answer, else the
/// key hints for the tab.
fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    // The find bar replaces the status line and owns the cursor.
    if let Some(Modal::Search { query, .. }) = &app.modal {
        let count = app.search_status().unwrap_or_default();
        let head = " Find ";
        let mut spans = vec![Span::styled(head, t.badge()), Span::raw(" "), Span::styled(query.value.clone(), t.text())];
        spans.push(Span::styled(format!("  {count}  "), t.muted()));
        spans.extend(hint_spans(t, &[hint("Enter", "older", 1), hint("Shift+Enter", "newer", 2), hint("Esc", "close", 0)]));
        f.render_widget(Paragraph::new(Line::from(spans)).style(t.base()), area);
        set_cursor(f, area.x + width(head) as u16 + 1, area.y, query);
        return;
    }
    let waiting = app.waiting_tabs();
    let notice = app.notices.first().map(|(host, req)| Span::styled(format!(" {host}: {}", req.prompt.trim()), t.warning()));
    let line = if app.prefix_pending {
        prefix_footer(app, area.width as usize)
    } else if let Some((msg, true, _)) = &app.status {
        // An error first, with a pending security-key notice after it.
        let mut spans = vec![Span::styled(format!(" {} Error: {msg}", t.glyph("✗", "x")), t.error())];
        if let Some(n) = notice {
            spans.extend([Span::styled(" ·", t.muted()), n]);
        }
        Line::from(spans)
    } else if let Some(n) = notice {
        Line::from(n)
    } else if let Some((msg, _, _)) = &app.status {
        // Plain mode says what kind of message it is in words.
        let kind = if t.plain { "Info:" } else { t.glyph("✓", "+") };
        Line::from(Span::styled(format!(" {kind} {msg}"), t.success()))
    } else if let Some(&i) = waiting.iter().find(|&&i| i != app.active) {
        // Only tabs 1-9 have a shortcut.
        let key = if i < 9 { format!(" ({})", app.keys.show(Action::Tab(i as u8 + 1))) } else { String::new() };
        let text = format!(" {} is waiting for an answer: switch to tab {}{key}", app.tabs[i].title, i + 1);
        Line::from(Span::styled(text, t.warning()))
    } else {
        let (local, after) = footer_hints(app);
        hint_footer(t, app.keys.prefix().as_deref(), &local, &after, area.width as usize)
    };
    f.render_widget(Paragraph::new(line).style(t.base()), area);
}

/// One which-key group: its heading and (key, what it does) rows.
type KeyGroup = (&'static str, Vec<(String, &'static str)>);

/// The which-key list: a box at the bottom right of `body` with the keys
/// that can follow the prefix here, in two grouped columns.
fn draw_which_key(f: &mut Frame, app: &App, body: Rect) {
    let t = &app.theme;
    let k = &app.keys;
    let Some(prefix) = k.prefix() else { return };
    let tab = &app.tabs[app.active];
    let entry = |a: Action, label: &'static str| k.after_prefix(a).map(|key| (key, label));
    let collect = |list: Vec<Option<(String, &'static str)>>| list.into_iter().flatten().collect::<Vec<_>>();
    let mut tabs = collect(vec![
        k.tabs_after_prefix().map(|key| (key, "go to tab")),
        entry(Action::NextTab, "next tab"),
        entry(Action::PrevTab, "previous tab"),
    ]);
    if app.active > 0 {
        tabs.extend(entry(Action::CloseTab, "close"));
    }
    let app_keys = collect(vec![entry(Action::Theme, "theme"), entry(Action::Help, "help"), entry(Action::Quit, "quit")]);
    let term_keys = match (&tab.kind, &tab.split) {
        (TabKind::Terminal(_), Some(_)) => collect(vec![
            entry(Action::Find, "find"),
            entry(Action::Snippets, "snippets"),
            entry(Action::FocusNextPane, "other pane"),
            entry(Action::SwapPanes, "swap panes"),
            entry(Action::EqualizePanes, "equal sizes"),
        ]),
        (TabKind::Terminal(_), None) => collect(vec![
            entry(Action::Find, "find"),
            entry(Action::Snippets, "snippets"),
            entry(Action::SplitRight, "split right"),
            entry(Action::SplitDown, "split down"),
        ]),
        _ => vec![],
    };
    // Columns: tabs and app keys | terminal keys (or tabs | app keys).
    let (left, right): (Vec<KeyGroup>, Vec<KeyGroup>) = if term_keys.is_empty() {
        (vec![("Tabs", tabs)], vec![("App", app_keys)])
    } else {
        (vec![("Tabs", tabs), ("App", app_keys)], vec![("Terminal", term_keys)])
    };
    let key_w = left.iter().chain(&right).flat_map(|g| &g.1).map(|e| width(&e.0)).max().unwrap_or(1);
    let column = |groups: &[KeyGroup]| -> (Vec<Line<'static>>, usize) {
        let mut lines = vec![];
        let mut w = 0;
        for (head, entries) in groups.iter().filter(|g| !g.1.is_empty()) {
            if !lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(head.to_string(), t.muted().add_modifier(Modifier::BOLD)));
            w = w.max(width(head));
            for (key, label) in entries {
                w = w.max(key_w + 2 + width(label));
                lines.push(Line::from(vec![
                    Span::styled(key.clone(), t.key()),
                    Span::raw(" ".repeat(key_w - width(key) + 2)),
                    Span::styled(label.to_string(), t.text()),
                ]));
            }
        }
        (lines, w)
    };
    let (l_lines, lw) = column(&left);
    let (r_lines, rw) = column(&right);
    let gap = 4;
    let w = ((lw + gap + rw + 4) as u16).min(body.width);
    let h = ((l_lines.len().max(r_lines.len()) + 2) as u16).min(body.height);
    let r = Rect::new(body.right().saturating_sub(w + 1).max(body.x), body.bottom().saturating_sub(h), w, h);
    let title = Line::styled(prefix_badge(t, &prefix), t.title());
    let inner = dialog(f, t, r, title, &[hint("Esc", "cancel", 0)], false);
    let [a, _, b] = Layout::horizontal([Constraint::Length(lw as u16), Constraint::Length(gap as u16), Constraint::Min(0)]).areas(inner);
    f.render_widget(Paragraph::new(l_lines), a);
    f.render_widget(Paragraph::new(r_lines), b);
}

// ---------------------------------------------------------------- hosts

/// Draws the Hosts tab: the filterable host list with status marks on the
/// left and the selected host's details (including its ssh command) on the
/// right; with nothing to list, one full-width panel with what to do next.
fn draw_hosts(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let empty = app.filtered.is_empty();
    let (left, right) = if empty {
        (area, None)
    } else {
        let [l, r] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);
        (l, Some(r))
    };

    let count = if app.hosts.is_empty() {
        String::new()
    } else if app.filtered.len() == app.hosts.len() {
        format!("{} ", app.hosts.len())
    } else {
        format!("{}/{} ", app.filtered.len(), app.hosts.len())
    };
    let mut status = format!(" sort: {} ", app.ui_state.sort.label().to_lowercase());
    if app.ui_state.show_hidden {
        status.push_str("· hidden shown ");
    }
    let outer = panel(&t, true)
        .title(Line::from(vec![Span::styled(" Hosts ", t.title()), Span::styled(count, t.muted())]))
        .title(Line::styled(status, t.muted()).right_aligned());
    let inner = outer.inner(left);
    f.render_widget(outer, left);
    let [filter_area, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    let filter_line = if app.filtering || !app.filter.value.is_empty() {
        let style = if app.filtering { t.accent() } else { t.text() };
        Line::from(vec![Span::styled(" / ", t.accent()), Span::styled(app.filter.value.clone(), style)])
    } else {
        Line::styled(" / to filter", t.muted())
    };
    f.render_widget(Paragraph::new(filter_line), filter_area);
    if app.filtering {
        set_cursor(f, filter_area.x + 3, filter_area.y, &app.filter);
    }

    if empty {
        draw_hosts_empty(f, app, list_area);
        return;
    }
    // Names are padded to the longest shown (capped at 28) so targets line up.
    let name_w = app.filtered.iter().map(|&i| width(&app.hosts[i].name)).max().unwrap_or(0).min(28);
    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|&i| {
            let h = &app.hosts[i];
            let (sym, style) = status_symbol(&t, app.statuses.get(&h.name));
            let mut spans = vec![
                Span::styled(format!("{sym} "), style),
                Span::styled(format!("{:<name_w$}  ", h.name), t.text()),
                Span::styled(h.target(), t.muted()),
            ];
            if h.hidden {
                spans.push(Span::styled("  hidden", t.muted().add_modifier(Modifier::ITALIC)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let mut state = ListState::default().with_offset(app.host_offset).with_selected(Some(app.host_selected));
    let list = List::new(items).highlight_style(t.selected()).highlight_symbol(mark(&t)).highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(list, list_area, &mut state);
    app.host_offset = state.offset();
    // The terminal cursor on the selection lets screen readers follow it.
    if !app.filtering && app.modal.is_none() {
        row_cursor(f, list_area, app.host_selected, app.host_offset);
    }
    if let Some(right) = right {
        draw_details(f, app, &t, right);
    }
}

/// The selection mark in lists (shape, not only color).
fn mark(t: &Theme) -> &'static str {
    t.glyph("› ", "> ")
}

/// The empty host list: why it is empty and the keys that help, centred.
fn draw_hosts_empty(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let (head, body, keys) = if app.hosts.is_empty() {
        (
            "No hosts yet".to_string(),
            "Add a connection, or Host entries to ~/.ssh/config.".to_string(),
            vec![hint("n", "new host", 1), hint("r", "reload", 2), hint("?", "help", 0)],
        )
    } else if !app.filter.value.is_empty() {
        (format!("No hosts match \u{201c}{}\u{201d}", app.filter.value), "Try fewer letters.".into(), vec![hint("Esc", "clear filter", 0)])
    } else {
        (
            "Every host is hidden".into(),
            "Hidden hosts stay out of the list.".into(),
            vec![hint("H", "show hidden", 0), hint("?", "help", 1)],
        )
    };
    let max = area.width as usize;
    let mut lines = vec![Line::styled(head, t.strong()), Line::raw("")];
    let words: Vec<String> = body.split(' ').map(String::from).collect();
    lines.extend(wrap_items(&words, max.saturating_sub(4), " ", "…").into_iter().map(|l| Line::styled(l, t.muted())));
    lines.push(Line::raw(""));
    let shown = fit(&keys, max.saturating_sub(2), |s| hints_width(keys.iter().zip(s).filter(|x| *x.1).map(|x| x.0)));
    lines.push(Line::from(hint_spans(t, keys.iter().zip(&shown).filter(|x| *x.1).map(|x| x.0))));
    let top = (area.height.saturating_sub(lines.len() as u16) / 2).saturating_sub(1);
    let r = Rect { y: area.y + top, height: area.height - top, ..area };
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), r);
}

/// The details panel: the host's name and status in the border, one row per
/// setting (paths elided in the middle, lists wrapped between items), its
/// ssh command and the keys that act on it.
fn draw_details(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let Some(h) = app.selected_host() else { return };
    let status = app.statuses.get(&h.name).cloned().unwrap_or_default();
    let (sym, sym_style) = status_symbol(t, Some(&status));
    let ell = t.glyph("…", "~");
    let half = (area.width as usize / 2).max(8);
    let mut b = panel(t, false).title(Line::styled(format!(" {} ", elide_middle(&h.name, half, ell)), t.strong()));
    if !t.plain {
        b = b.title(
            Line::from(vec![
                Span::styled(format!(" {sym} "), sym_style),
                Span::styled(format!("{} ", elide_middle(&status.label(), half, ell)), t.muted()),
            ])
            .right_aligned(),
        );
    }
    let inner = b.inner(area);
    f.render_widget(b, area);

    const LABEL_W: usize = 11;
    let val_w = (inner.width as usize).saturating_sub(LABEL_W + 3).max(4);
    let indent = " ".repeat(LABEL_W + 3);
    let mut lines = vec![Line::raw("")];
    // One setting: its label, then the value lines (later ones indented).
    let row = |lines: &mut Vec<Line>, k: &str, vals: Vec<String>| {
        for (i, v) in vals.into_iter().enumerate() {
            let head = if i == 0 { format!("{k:>LABEL_W$}   ") } else { indent.clone() };
            lines.push(Line::from(vec![Span::styled(head, t.muted()), Span::styled(v, t.text())]));
        }
    };
    let one = |v: String| vec![elide_middle(&v, val_w, ell)];
    let dash = || t.glyph("—", "-").to_string();
    let mut cmd = vec![app.cfg.settings.ssh_binary.clone()];
    cmd.extend(h.shell_args());
    let words: Vec<String> = cmd.iter().map(|w| shell_words::quote(w).into_owned()).collect();
    row(&mut lines, "Host", one(h.hostname.clone()));
    row(&mut lines, "User", one(h.user.clone().unwrap_or_else(dash)));
    row(&mut lines, "Port", one(h.port.map(|p| p.to_string()).unwrap_or_else(|| "22".into())));
    row(&mut lines, "Identity", one(h.identity_file.clone().unwrap_or_else(dash)));
    row(&mut lines, "ProxyJump", one(h.proxy_jump.clone().unwrap_or_else(dash)));
    row(&mut lines, "Remote dir", one(h.remote_dir.clone().unwrap_or_else(dash)));
    row(&mut lines, "Tags", if h.tags.is_empty() { vec![dash()] } else { wrap_items(&h.tags, val_w, ", ", ell) });
    row(&mut lines, "Source", one(h.source_label.clone()));
    row(&mut lines, "Last used", one(app.history.ago(&h.name, skiff_core::history::now()).unwrap_or_else(|| "never".into())));
    if t.plain {
        row(&mut lines, "Status", one(status.label()));
    }
    lines.push(Line::raw(""));
    row(&mut lines, "Command", wrap_items(&words, val_w, " ", ell));
    lines.push(Line::raw(""));
    // The keys that act on this host, wrapped between hints.
    let acts = [hint("Enter", "terminal", 1), hint("s", "files", 2), hint("e", "edit", 3), hint("f", "forward", 4), hint("d", "delete", 5)];
    let room = (inner.width as usize).saturating_sub(4);
    let mut cur: Vec<&Hint> = vec![];
    for a in &acts {
        if !cur.is_empty() && hints_width(cur.iter().copied().chain([a])) > room {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(hint_spans(t, cur.drain(..)));
            lines.push(Line::from(spans));
        }
        cur.push(a);
    }
    let mut spans = vec![Span::raw("  ")];
    spans.extend(hint_spans(t, cur));
    lines.push(Line::from(spans));
    f.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------- terminal

/// The two pane areas of a split terminal: (first, second) = (left, right)
/// or (top, bottom). The first pane takes `ratio` of `area`, but each pane
/// keeps at least `MIN_COLS` x `MIN_ROWS` cells of content where there is
/// room. Side by side, the first pane's last column is the divider line;
/// stacked, each pane's first row is its title rule.
pub fn split_areas(area: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect) {
    let (total, min) = match dir {
        SplitDir::Right => (area.width, panes::MIN_COLS + 1),
        SplitDir::Down => (area.height, panes::MIN_ROWS + 1),
    };
    let first = (total as f32 * ratio).round() as u16;
    // `min` includes the divider or rule cell; too small for both: halve it.
    let first = if total >= 2 * min { first.clamp(min, total - min) } else { total / 2 };
    let sizes = [Constraint::Length(first), Constraint::Fill(1)];
    let [a, b] = match dir {
        SplitDir::Right => Layout::horizontal(sizes).areas(area),
        SplitDir::Down => Layout::vertical(sizes).areas(area),
    };
    (a, b)
}

/// The cell is the divider between the panes: the divider column side by
/// side, or the second pane's title rule when stacked.
pub fn on_divider(area: Rect, dir: SplitDir, ratio: f32, col: u16, row: u16) -> bool {
    if !area.contains(ratatui::layout::Position::new(col, row)) {
        return false;
    }
    let (a, b) = split_areas(area, dir, ratio);
    match dir {
        SplitDir::Right => col + 1 == a.right(),
        SplitDir::Down => row == b.y,
    }
}

/// The share that puts the divider at the pointer, snapped to the nearest
/// snap point within a cell when `snapping`.
pub fn divider_ratio(area: Rect, dir: SplitDir, col: u16, row: u16, snapping: bool) -> f32 {
    // Side by side the pointer's cell becomes the first pane's last (the
    // divider) column; stacked, the second pane's rule row.
    let (cells, total) = match dir {
        SplitDir::Right => (col.saturating_sub(area.x) + 1, area.width),
        SplitDir::Down => (row.saturating_sub(area.y), area.height),
    };
    let r = cells as f32 / total.max(1) as f32;
    let r = if snapping { panes::snap(r, total as i32, 1).unwrap_or(r) } else { r };
    r.clamp(panes::MIN_RATIO, 1.0 - panes::MIN_RATIO)
}

/// Draws one terminal pane in `area`, resizing its session to fit; returns
/// the content area. A pane of a split gets a one-row title rule (heavy
/// accent when focused); a lone pane has no frame at all. Disconnection or
/// scrollback shows in the rule, or as a badge at the top right.
fn draw_session(f: &mut Frame, t: &Theme, s: &mut TerminalSession, area: Rect, rule: Option<&str>, focused: bool, close: &str) -> Rect {
    let alive = s.is_alive();
    let scroll = s.scroll_offset();
    let state = if !alive {
        Some((format!("{} disconnected · Enter reconnect · {close} close", t.glyph("✗", "x")), t.error()))
    } else if scroll > 0 {
        Some((format!("scrollback +{scroll}"), t.warning()))
    } else {
        None
    };
    let content = match rule {
        Some(title) => {
            let set = match (t.ascii, focused) {
                (true, true) => border::Set { horizontal_top: "=", ..ASCII_BORDER },
                (true, false) => ASCII_BORDER,
                (false, true) => border::THICK,
                (false, false) => border::PLAIN,
            };
            let mut b =
                if t.plain { Block::new() } else { Block::new().borders(Borders::TOP).border_set(set).border_style(t.border(focused)) };
            b = b.title(Line::styled(format!(" {title} "), title_style(t, focused)));
            if let Some((text, style)) = &state {
                b = b.title(Line::styled(format!(" {text} "), *style).right_aligned());
            }
            let inner = b.inner(area);
            f.render_widget(b, area);
            inner
        }
        None => area,
    };
    s.resize(content.height, content.width);
    // A poisoned lock (the output thread panicked) shows an empty pane
    // instead of taking the whole app down.
    if let Ok(parser) = s.parser.lock() {
        // Only a live, focused pane at the bottom of its output shows the cursor.
        let mut cursor = Cursor::default();
        if !focused || !alive || scroll > 0 || parser.screen().hide_cursor() {
            cursor.hide();
        }
        f.render_widget(PseudoTerminal::new(parser.screen()).cursor(cursor), content);
    }
    if let (None, Some((text, style))) = (rule, state) {
        let text = format!(" {text} ");
        let w = (width(&text) as u16).min(content.width);
        let r = Rect::new(content.right() - w, content.y, w, content.height.min(1));
        f.render_widget(Paragraph::new(Span::styled(text, style.add_modifier(Modifier::REVERSED))), r);
    }
    content
}

/// Draws a terminal tab (both panes when split, with a divider between
/// them), records pane geometry in `app` for the mouse, then overlays the
/// mouse selection and search matches on the focused pane.
fn draw_terminal(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let active = app.active;
    let tab = &mut app.tabs[active];
    let title_text = tab.title.clone();
    app.split_area = tab.split.as_ref().map(|_| area);
    // `area` becomes the focused pane's rect; `other` the unfocused one's.
    let (area, other) = match &tab.split {
        None => (area, None),
        Some(sp) => {
            let (mut a, b) = split_areas(area, sp.dir, sp.ratio);
            if sp.dir == SplitDir::Right && a.width > 0 {
                // The first pane's last column is the divider line.
                a.width -= 1;
                let buf = f.buffer_mut();
                let line = if t.plain { "|" } else { t.glyph("│", "|") };
                for y in a.top()..a.bottom() {
                    buf[(a.right(), y)].set_symbol(line).set_style(t.border(false));
                }
            }
            if sp.focus_first { (a, Some(b)) } else { (b, Some(a)) }
        }
    };
    let close = app.keys.show(Action::CloseTab);
    let rule = other.map(|_| title_text.as_str());
    app.other_pane = match (&mut tab.split, other) {
        (Some(sp), Some(r)) => {
            draw_session(f, &t, &mut sp.other, r, rule, false, &close);
            Some(r)
        }
        _ => None,
    };
    let TabKind::Terminal(s) = &mut tab.kind else {
        return;
    };
    let inner = draw_session(f, &t, s, area, rule, true, &close);
    app_term_area(inner, &mut app.term_area);
    app.term_origin = (inner.x, inner.y);

    let TabKind::Terminal(s) = &app.tabs[active].kind else {
        return;
    };
    let Ok(parser) = s.parser.lock() else {
        return;
    };
    // Absolute line number of the pane's top row, to map selection and
    // search positions (absolute lines) onto screen rows.
    let top = s.scrolled().saturating_sub(parser.screen().scrollback() as u64);
    drop(parser);
    let buf = f.buffer_mut();
    // Restyles cells of one content row, clipped to the pane.
    let mut paint = |row: u16, cols: std::ops::Range<u16>, style: Style| {
        for c in cols.start..cols.end.min(inner.width) {
            if row < inner.height {
                buf[(inner.x + c, inner.y + row)].set_style(style);
            }
        }
    };
    // Mouse selection (`tui_mouse = "always"`).
    if let Some(sel) = app.selection.filter(|s| s.tab == app.tabs[active].id && s.dragged) {
        let (a, b) = sel.span();
        for row in 0..inner.height {
            let line = top + row as u64;
            if (a.0..=b.0).contains(&line) {
                let from = if line == a.0 { a.1 } else { 0 };
                let to = if line == b.0 { b.1 + 1 } else { inner.width };
                paint(row, from..to, t.selected());
            }
        }
    }
    // Search matches; the current one stands out.
    if let Some(Modal::Search { search, .. }) = &app.modal {
        for (row, col, len, current) in search.marks(top, inner.height as usize) {
            let style = if current {
                t.warning().add_modifier(Modifier::REVERSED | Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().add_modifier(Modifier::REVERSED)
            };
            paint(row as u16, col as u16..(col + len) as u16, style);
        }
    }
}

/// Stores the terminal content size as (rows, cols), the size new sessions start at.
fn app_term_area(inner: Rect, slot: &mut (u16, u16)) {
    *slot = (inner.height, inner.width);
}

// ---------------------------------------------------------------- sftp

/// Draws an SFTP tab: local and remote panes side by side (the remote one
/// shows connecting / failed states instead), with a one-line transfer
/// strip below them.
fn draw_sftp(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let close = app.keys.show(Action::CloseTab);
    let active = app.active;
    let TabKind::Sftp(s) = &mut app.tabs[active].kind else {
        return;
    };
    let s = s.as_mut();

    let [panes, strip] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
    let [left, right] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(panes);

    let focus = s.focus;
    let host = s.host_name.clone();
    draw_pane(f, &t, &mut s.local, left, "Local", focus == Side::Local);

    match &s.status {
        Status::Connected => draw_pane(f, &t, &mut s.remote, right, &host, focus == Side::Remote),
        Status::Connecting => {
            let b = panel(&t, focus == Side::Remote)
                .title(Line::styled(format!(" {host} "), title_style(&t, focus == Side::Remote)))
                .title(Line::styled(format!(" {} connecting ", t.glyph("◌", "~")), t.muted()).right_aligned());
            f.render_widget(
                Paragraph::new(format!("\nConnecting to {host}…")).alignment(Alignment::Center).style(t.muted()).block(b),
                right,
            );
        }
        Status::Failed(err) => {
            let b = panel(&t, focus == Side::Remote)
                .title(Line::styled(format!(" {host} "), title_style(&t, focus == Side::Remote)))
                .title(Line::styled(format!(" {} failed ", t.glyph("✗", "x")), t.error()).right_aligned())
                .padding(Padding::horizontal(1));
            let text = vec![
                Line::raw(""),
                Line::styled("Connection failed", t.error().add_modifier(Modifier::BOLD)),
                Line::raw(""),
                Line::styled(err.clone(), t.text()),
                Line::raw(""),
                Line::from(hint_spans(&t, &[hint("r", "retry", 1), hint(close, "close", 2)])),
            ];
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(b), right);
        }
    }
    draw_strip(f, &t, s, strip);
}

/// Draws one file pane: the side's name and its path (elided in the middle)
/// in the border, the item count on the right, then a name / size /
/// modified table with a header row (modified only when there is room). A
/// listing error takes two lines at the bottom.
fn draw_pane(f: &mut Frame, t: &Theme, pane: &mut Pane, area: Rect, label: &str, focused: bool) {
    let ell = t.glyph("…", "~");
    let items = pane.entries.len().saturating_sub(1);
    let right = if pane.loading { format!(" loading{ell} ") } else { format!(" {items} item{} ", if items == 1 { "" } else { "s" }) };
    let room = (area.width as usize).saturating_sub(width(label) + width(&right) + 6);
    let b = panel(t, focused)
        .title(Line::from(vec![
            Span::styled(format!(" {label} "), title_style(t, focused)),
            Span::styled(format!("{} ", elide_middle(&pane.path, room, ell)), if focused { t.text() } else { t.muted() }),
        ]))
        .title(Line::styled(right, t.muted()).right_aligned());
    let inner = b.inner(area);
    f.render_widget(b, area);
    let (list_area, err_area) = if pane.error.is_some() {
        let [a, e] = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(inner);
        (a, Some(e))
    } else {
        (inner, None)
    };

    let show_modified = inner.width >= 44;
    let fixed = 2 + 8 + 1 + if show_modified { 17 } else { 0 };
    let name_w = (inner.width as usize).saturating_sub(fixed).max(4);
    let rows: Vec<Row> = pane
        .entries
        .iter()
        .map(|e| {
            // Directories get a trailing `/` and accent; symlinks a `→`.
            let name_style = if e.is_dir { t.accent().add_modifier(Modifier::BOLD) } else { t.text() };
            let mut suffix = String::new();
            if e.is_dir && e.name != ".." {
                suffix.push('/');
            }
            if e.is_symlink {
                suffix.push_str(t.glyph(" →", " ->"));
            }
            let name = elide_middle(&e.name, name_w.saturating_sub(width(&suffix)), ell) + &suffix;
            let mut cells = vec![
                Cell::from(Span::styled(name, name_style)),
                Cell::from(Line::from(if e.is_dir { String::new() } else { human_size(e.size) }).alignment(Alignment::Right))
                    .style(t.muted()),
            ];
            if show_modified {
                cells.push(Cell::from(e.modified.map(format_time).unwrap_or_default()).style(t.muted()));
            }
            Row::new(cells)
        })
        .collect();
    let head_style = t.muted().add_modifier(Modifier::BOLD);
    let mut header = vec![Cell::from("Name"), Cell::from(Line::from("Size").alignment(Alignment::Right))];
    let mut widths = vec![Constraint::Min(4), Constraint::Length(8)];
    if show_modified {
        header.push(Cell::from("Modified"));
        widths.push(Constraint::Length(16));
    }
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(head_style))
        .column_spacing(1)
        .row_highlight_style(if focused { t.selected() } else { Style::new().add_modifier(Modifier::REVERSED) })
        .highlight_symbol(mark(t))
        .highlight_spacing(HighlightSpacing::Always);
    let mut state = TableState::default().with_offset(pane.offset).with_selected(Some(pane.selected));
    f.render_stateful_widget(table, list_area, &mut state);
    pane.offset = state.offset();
    if focused {
        // Below the header row.
        let rows = Rect { y: list_area.y + 1, height: list_area.height.saturating_sub(1), ..list_area };
        row_cursor(f, rows, pane.selected, pane.offset);
    }

    if let (Some(err), Some(a)) = (&pane.error, err_area) {
        f.render_widget(Paragraph::new(err.as_str()).style(t.error()).wrap(Wrap { trim: true }), a);
    }
}

/// The one-line transfer strip: the newest running copy with a progress bar
/// (else the newest finished one), counts per state on the right, and the
/// keys that act on them.
fn draw_strip(f: &mut Frame, t: &Theme, s: &SftpTab, area: Rect) {
    let count = |want: fn(&TransferState) -> bool| s.transfers.iter().filter(|tr| want(&tr.state)).count();
    let running = count(|st| *st == TransferState::Running);
    let done = count(|st| *st == TransferState::Done);
    let failed = count(|st| matches!(st, TransferState::Failed(_)));
    let shown = s.transfers.iter().rev().find(|tr| tr.state == TransferState::Running).or(s.transfers.last());
    let mut left: Vec<Span> = vec![Span::raw(" ")];
    match shown {
        None => left.push(Span::styled("No transfers · c copies the selection to the other pane", t.muted())),
        Some(tr) => {
            let (mark, style) = match &tr.state {
                TransferState::Running => (t.glyph("◌", "~"), t.accent()),
                TransferState::Done => (t.glyph("✓", "+"), t.success()),
                TransferState::Cancelled => (t.glyph("✗", "x"), t.warning()),
                TransferState::Failed(_) => (t.glyph("✗", "x"), t.error()),
            };
            left.push(Span::styled(format!("{mark} "), style));
            left.push(Span::styled(format!("{}  ", tr.label), t.text()));
            match tr.total {
                Some(total) if total > 0 && tr.state == TransferState::Running => {
                    const BAR: u64 = 12;
                    let filled = (tr.done.min(total) * BAR / total) as usize;
                    left.push(Span::styled(t.glyph("━", "#").repeat(filled), t.accent()));
                    left.push(Span::styled(t.glyph("─", "-").repeat(BAR as usize - filled), t.muted()));
                    left.push(Span::styled(
                        format!(" {:>3}%  {} / {}", tr.done * 100 / total, human_size(tr.done), human_size(total)),
                        t.muted(),
                    ));
                }
                _ => left.push(Span::styled(human_size(tr.done), t.muted())),
            }
            match &tr.state {
                TransferState::Failed(e) => left.push(Span::styled(format!("  {e}"), t.error())),
                TransferState::Cancelled => left.push(Span::styled("  cancelled", t.warning())),
                _ => {}
            }
        }
    }
    let mut right: Vec<Span> = vec![];
    for (n, what, style) in [(running, "running", t.accent()), (done, "done", t.muted()), (failed, "failed", t.error())] {
        if n > 0 {
            if !right.is_empty() {
                right.push(Span::styled(" · ", t.muted()));
            }
            right.push(Span::styled(format!("{n} {what}"), style));
        }
    }
    let mut keys = vec![];
    if running > 0 {
        keys.push(hint("X", "cancel", 1));
    }
    if s.transfers.len() > running {
        keys.push(hint("x", "clear", 2));
    }
    let sum = |spans: &[Span]| spans.iter().map(|s| s.width()).sum::<usize>();
    let max = area.width as usize;
    let lw = sum(&left);
    // Keys drop first, then the counts, when the line is too narrow.
    if !keys.is_empty() && lw + sum(&right) + hints_width(&keys) + 6 <= max {
        right.push(Span::raw("   "));
        right.extend(hint_spans(t, &keys));
    }
    if lw + sum(&right) + 2 > max {
        right.clear();
    }
    let rw = sum(&right);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(max.saturating_sub(lw + rw + 1))));
    spans.extend(right);
    f.render_widget(Paragraph::new(Line::from(spans)).style(t.base()), area);
}

/// Formats as `YYYY-MM-DD HH:MM` (UTC) without pulling in a date crate.
fn format_time(t: SystemTime) -> String {
    let Ok(d) = t.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rem / 3600, (rem % 3600) / 60)
}

// ---------------------------------------------------------------- modals

/// A `w` x `h` rect centred in `area`, shrunk to leave a one-cell margin.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// Clears `r` and draws a dialog frame there: `title` in the top border,
/// `hints` (as many as fit) on the bottom border, red when `destructive`.
/// Returns the area inside, with a column of padding on each side.
fn dialog(f: &mut Frame, t: &Theme, r: Rect, title: Line<'static>, hints: &[Hint], destructive: bool) -> Rect {
    f.render_widget(Clear, r);
    let mut b = if t.plain {
        Block::new().style(t.base())
    } else {
        let style = if destructive { t.danger_border() } else { t.border(true) };
        Block::bordered().border_set(t.border_set()).border_style(style).style(t.base()).padding(Padding::horizontal(1))
    };
    b = b.title(if destructive { title.style(t.error().add_modifier(Modifier::BOLD)) } else { title });
    if !hints.is_empty() {
        b = b.title_bottom(fitted_line(t, hints, (r.width as usize).saturating_sub(4)).right_aligned());
    }
    let inner = b.inner(r);
    f.render_widget(b, r);
    inner
}

/// A dialog title: the text padded with a space each side, bold accent.
fn dialog_title(t: &Theme, text: &str) -> Line<'static> {
    Line::styled(format!(" {} ", text.trim()), t.title())
}

/// Puts the terminal cursor at the start of row `selected` of a list drawn
/// in `area` and scrolled to `offset`.
fn row_cursor(f: &mut Frame, area: Rect, selected: usize, offset: usize) {
    let Some(row) = selected.checked_sub(offset) else {
        return;
    };
    if (row as u16) < area.height {
        f.set_cursor_position(Position::new(area.x, area.y + row as u16));
    }
}

/// Puts the terminal cursor at `input`'s caret, for a field drawn from (`x`, `y`).
fn set_cursor(f: &mut Frame, x: u16, y: u16, input: &TextInput) {
    let offset = width(input.before_cursor()) as u16;
    f.set_cursor_position(Position::new(x + offset, y));
}

/// The help's lines: key problems, how the prefix works, then each group
/// of keys under a bold heading with a blank line before it.
fn help_lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let k =
        |key: &str, desc: &str| Line::from(vec![Span::styled(format!("  {key:<22}"), t.key()), Span::styled(desc.to_string(), t.text())]);
    let a = |action: Action| k(&app.keys.show(action), action.label());
    let mut lines: Vec<Line> = app.key_errors.iter().map(|e| Line::styled(format!("  {e}"), t.error())).collect();
    if let Some(p) = app.keys.prefix() {
        // How to read "Alt+O, w" before the list uses it.
        lines.push(Line::styled(format!("  \"{p}, w\" means: press {p}, let go, then press w (Esc cancels)."), t.muted()));
    }
    let groups: Vec<(&str, Vec<Line>)> = vec![
        (
            "Everywhere  ·  [tui_keys] in the config remaps these",
            vec![
                a(Action::Prefix),
                k(&app.keys.show_tabs(), "switch to tab"),
                a(Action::PrevTab),
                a(Action::NextTab),
                a(Action::CloseTab),
                a(Action::Theme),
                a(Action::Help),
                a(Action::Quit),
            ],
        ),
        (
            "Hosts",
            vec![
                k("Enter", "open terminal (ssh)"),
                k("s", "open file transfer (sftp)"),
                k("/", "filter hosts"),
                k("n / e / d", "new / edit / delete host (ssh config too)"),
                k("p", "check which hosts are reachable"),
                k("o", "sort by name / recent"),
                k("H", "show / hide hidden hosts"),
                k("f / F", "forward a port / saved forwards"),
                k("r", "reload config files"),
            ],
        ),
        (
            "Host editor",
            vec![
                k("Tab / ↑↓ · Ctrl+S", "move between fields · save"),
                k("Space", "toggle Save to / Hidden"),
                k("Enter (Other options)", "add a line"),
            ],
        ),
        (
            "Terminal",
            vec![
                k("Shift+PgUp / PgDn", "scroll back"),
                a(Action::Find),
                a(Action::Snippets),
                a(Action::SplitRight),
                a(Action::SplitDown),
                a(Action::FocusNextPane),
                a(Action::SwapPanes),
                a(Action::ResizePaneLeft),
                a(Action::ResizePaneRight),
                a(Action::ResizePaneUp),
                a(Action::ResizePaneDown),
                a(Action::EqualizePanes),
                k("Enter (when closed)", "reconnect"),
            ],
        ),
        (
            "File transfer",
            vec![
                k("Tab / ← →", "switch pane"),
                k("Enter / Backspace", "open dir / go up"),
                k("c or F5", "copy to other pane"),
                k("m or F7", "make directory"),
                k("R or F2", "rename"),
                k("d, Del or F8", "delete"),
                k("g", "go to path"),
                k("X", "cancel the newest running transfer"),
                k(". / r / x", "hidden files / refresh / clear transfers"),
            ],
        ),
    ];
    for (head, rows) in groups {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(format!(" {head}"), t.title()));
        lines.extend(rows);
    }
    lines
}

/// Draws the open modal, if any, centred over `area` on a cleared background.
/// `states`: each saved forward's state, while the forwards list is open.
fn draw_modal(f: &mut Frame, app: &App, area: Rect, states: &[String]) {
    let t = &app.theme;
    let Some(modal) = &app.modal else { return };
    let ell = t.glyph("…", "~");
    match modal {
        Modal::Help { scroll } => {
            let r = centered(area, 80.min(area.width.saturating_sub(4)), area.height);
            let lines = help_lines(app);
            let total = lines.len() as u16;
            let mut b = if t.plain {
                Block::new().style(t.base())
            } else {
                Block::bordered().border_set(t.border_set()).border_style(t.border(true)).style(t.base())
            };
            let path = app.cfg_path.display().to_string();
            b = b
                .title(dialog_title(t, "Help"))
                .title(fitted_line(t, &[hint("↑↓ PgUp PgDn", "scroll", 1), hint("Esc", "close", 0)], r.width as usize / 2).right_aligned());
            // Clamp the scroll so the last line stays at the bottom.
            let visible = b.inner(r).height;
            let max_scroll = total.saturating_sub(visible);
            let scroll = (*scroll).min(max_scroll);
            let pos = format!(" {}–{} of {total} ", scroll + 1, (scroll + visible).min(total));
            let room = (r.width as usize).saturating_sub(width(&pos) + 14);
            b = b
                .title_bottom(Line::from(vec![
                    Span::styled(" config ", t.muted()),
                    Span::styled(format!("{} ", elide_middle(&path, room, ell)), t.text()),
                ]))
                .title_bottom(Line::styled(pos, t.muted()).right_aligned());
            f.render_widget(Clear, r);
            f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(b), r);
            if max_scroll > 0 && !t.plain {
                let mut st =
                    ScrollbarState::new(max_scroll as usize + 1).position(scroll as usize).viewport_content_length(visible as usize);
                let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None)
                    .track_symbol(Some(t.glyph("│", "|")))
                    .track_style(t.border(true))
                    .thumb_symbol(t.glyph("┃", "#"))
                    .thumb_style(t.title());
                f.render_stateful_widget(bar, r.inner(Margin { vertical: 1, horizontal: 0 }), &mut st);
            }
        }
        Modal::ThemePicker { selected, .. } => {
            let themes = app.all_themes();
            let r = centered(area, 44, themes.len() as u16 + 2);
            let inner = dialog(f, t, r, dialog_title(t, "Theme"), &[hint("Enter", "save", 1), hint("Esc", "cancel", 0)], false);
            let items: Vec<ListItem> = themes
                .iter()
                .map(|d| {
                    let swatch = |c: &str| Span::styled("■", Style::new().fg(std::str::FromStr::from_str(c).unwrap_or_default()));
                    let mut spans = vec![Span::raw(format!("{:<20}", d.name))];
                    if !t.mono {
                        spans.extend([
                            swatch(&d.bg),
                            swatch(&d.fg),
                            swatch(&d.accent),
                            swatch(&d.success),
                            swatch(&d.warning),
                            swatch(&d.error),
                        ]);
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(*selected));
            let list = List::new(items).highlight_style(t.selected()).highlight_symbol(mark(t));
            f.render_stateful_widget(list, inner, &mut state);
            row_cursor(f, inner, *selected, state.offset());
        }
        Modal::Editor(ed) => {
            let rows = ed.rows();
            let r = centered(area, 76, rows.len() as u16 + 4);
            let hints = [hint("Tab ↑↓", "move", 2), hint("Space", "toggle", 3), hint("Ctrl+S", "save", 0), hint("Esc", "cancel", 1)];
            let inner = dialog(f, t, r, dialog_title(t, ed.title()), &hints, false);
            // One line per row; the focused row's hint shows below them.
            let mut hint_text = "";
            for (i, row) in rows.iter().enumerate() {
                let focused = i == ed.focus;
                let (label, value, input) = match *row {
                    EdRow::Dest => ("Save to", format!("‹ {} ›", ed.dest_label()), None),
                    EdRow::Field(fl) => {
                        if focused {
                            hint_text = fl.hint();
                        }
                        let input = ed.input(fl);
                        (fl.label(), input.map(|i| i.value.clone()).unwrap_or_default(), input)
                    }
                    EdRow::Hidden => ("Hidden", if ed.form.hidden { "[x] left out of host lists" } else { "[ ]" }.to_string(), None),
                    EdRow::Other(j) => (if j == 0 { "Other options" } else { "" }, ed.other[j].value.clone(), Some(&ed.other[j])),
                };
                if matches!(row, EdRow::Other(_)) && focused {
                    hint_text = "one option per line, e.g. ForwardAgent yes · Enter adds a line";
                }
                field_row(f, t, inner, i as u16, label, &value, input, focused);
            }
            let footer = match &ed.error {
                Some(e) => Line::styled(format!("{} {e}", t.glyph("✗", "x")), t.error()),
                None => Line::styled(hint_text, t.muted()),
            };
            let y = inner.y + rows.len() as u16 + 1;
            if y < inner.bottom() {
                f.render_widget(Paragraph::new(footer).wrap(Wrap { trim: true }), Rect::new(inner.x, y, inner.width, inner.bottom() - y));
            }
        }
        Modal::Forward(form) => {
            let rows = form.rows();
            let r = centered(area, 76, rows.len() as u16 + 7);
            let hints = [hint("Tab ↑↓", "move", 2), hint("Ctrl+S", "save and start", 0), hint("Esc", "cancel", 1)];
            let inner = dialog(f, t, r, dialog_title(t, &format!("Forward a port · {}", form.host)), &hints, false);
            for (i, row) in rows.iter().enumerate() {
                let input = form.input(*row);
                let value = match row {
                    FwdRow::Kind => format!("‹ {} ›", form.kind.label()),
                    _ => input.map(|i| i.value.clone()).unwrap_or_default(),
                };
                field_row(f, t, inner, i as u16, ForwardForm::label(*row), &value, input, i == form.focus);
            }
            let y = inner.y + rows.len() as u16 + 1;
            let mut lines = vec![Line::styled(form.kind.explain(), t.muted())];
            if let Some(e) = &form.error {
                lines.insert(0, Line::styled(format!("{} {e}", t.glyph("✗", "x")), t.error()));
            } else {
                lines.push(Line::styled("Bind address defaults to 127.0.0.1.", t.muted()));
            }
            if y < inner.bottom() {
                f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), Rect::new(inner.x, y, inner.width, inner.bottom() - y));
            }
        }
        Modal::Forwards { selected } => {
            let list = &app.cfg.forwards;
            let r = centered(area, 76, list.len().max(1) as u16 + 2);
            let hints =
                [hint("Enter", "start", 1), hint("s", "stop", 2), hint("e", "edit", 3), hint("d", "delete", 4), hint("Esc", "close", 0)];
            let inner = dialog(f, t, r, dialog_title(t, "Saved forwards"), &hints, false);
            if list.is_empty() {
                f.render_widget(Paragraph::new("No saved forwards. Select a host and press f.").style(t.muted()), inner);
            } else {
                let items: Vec<ListItem> = list
                    .iter()
                    .zip(states)
                    .map(|(fw, st)| {
                        ListItem::new(Line::from(vec![Span::styled(fw.title(), t.text()), Span::styled(format!("  {st}"), t.muted())]))
                    })
                    .collect();
                let mut state = ListState::default().with_selected(Some(*selected));
                f.render_stateful_widget(List::new(items).highlight_style(t.selected()).highlight_symbol(mark(t)), inner, &mut state);
                row_cursor(f, inner, *selected, state.offset());
            }
        }
        Modal::Prompt(p) => {
            let r = centered(area, 60, 3);
            let inner = dialog(f, t, r, dialog_title(t, &p.title), &[hint("Enter", "ok", 1), hint("Esc", "cancel", 0)], false);
            f.render_widget(Paragraph::new(p.input.value.as_str()).style(t.text()), inner);
            set_cursor(f, inner.x, inner.y, &p.input);
        }
        Modal::Search { .. } => {} // drawn on the status line
        Modal::Ask { host, input, req } => {
            // ssh's own prompt text; secrets get a masked field, yes/no questions none.
            let secret = req.kind == AskKind::Secret;
            let prompt: Vec<Line> = req.prompt.trim_end().lines().map(|l| Line::styled(l.to_string(), t.text())).collect();
            let r = centered(area, 76, prompt.len() as u16 + if secret { 5 } else { 3 });
            let hints = if secret {
                vec![hint("Enter", "send", 1), hint("Esc", "cancel", 0)]
            } else {
                vec![hint("y", "yes", 1), hint("n", "no", 2), hint("Esc", "cancel", 0)]
            };
            let inner = dialog(f, t, r, dialog_title(t, &format!("{host} · ssh asks")), &hints, false);
            let [text, field] = Layout::vertical([Constraint::Min(1), Constraint::Length(if secret { 2 } else { 0 })]).areas(inner);
            let text = Rect { y: text.y + 1, height: text.height.saturating_sub(1), ..text };
            f.render_widget(Paragraph::new(prompt).wrap(Wrap { trim: false }), text);
            if secret {
                // Masked: one dot per character typed, never the text.
                let dots = "•".repeat(input.value.chars().count());
                f.render_widget(Paragraph::new(Span::styled(format!("> {dots}"), t.accent())), Rect { height: 1, ..field });
                let before = input.value.chars().take(input.cursor).count() as u16;
                f.set_cursor_position(Position::new(field.x + 2 + before, field.y));
            }
        }
        Modal::Overwrite { host, path, .. } => {
            let text = format!("{path} already exists ({host}). Replace it?");
            let w = (width(&text) as u16 + 6).clamp(48, 80);
            let r = centered(area, w, 5);
            let hints = [
                hint("o", "overwrite", 1),
                hint("s", "skip", 2),
                hint("a", "all", 4),
                hint("S", "skip all", 5),
                hint("c/Esc", "cancel", 0),
            ];
            let inner = dialog(f, t, r, dialog_title(t, "File exists"), &hints, false);
            f.render_widget(Paragraph::new(vec![Line::raw(""), Line::styled(text, t.text())]).wrap(Wrap { trim: true }), inner);
        }
        Modal::Snippets { selected } => {
            let list = app.tab_snippets();
            let r = centered(area, 76, list.len().max(1) as u16 + 2);
            let inner = dialog(f, t, r, dialog_title(t, "Snippets"), &[hint("Enter", "send", 1), hint("Esc", "close", 0)], false);
            if list.is_empty() {
                f.render_widget(Paragraph::new("No snippets. Add [[snippet]] entries to config.toml.").style(t.muted()), inner);
            } else {
                let items: Vec<ListItem> = list
                    .iter()
                    .map(|s| {
                        ListItem::new(Line::from(vec![
                            Span::styled(s.name.clone(), t.text()),
                            Span::styled(format!("  {}", s.command), t.muted()),
                        ]))
                    })
                    .collect();
                let mut state = ListState::default().with_selected(Some(*selected));
                f.render_stateful_widget(List::new(items).highlight_style(t.selected()).highlight_symbol(mark(t)), inner, &mut state);
                row_cursor(f, inner, *selected, state.offset());
            }
        }
        Modal::Confirm { text, action } => {
            let destructive = action.is_destructive();
            let w = (width(text) as u16 + 8).clamp(44, 64);
            let words: Vec<String> = text.split(' ').map(String::from).collect();
            let body = wrap_items(&words, (w as usize).saturating_sub(4).min(area.width.saturating_sub(6) as usize), " ", ell);
            let r = centered(area, w, body.len() as u16 + 4);
            let (title, yes) = match action {
                ConfirmAction::Quit => ("Quit", "quit"),
                ConfirmAction::Paste { .. } => ("Paste", "paste"),
                _ if destructive => ("Delete", "delete"),
                _ => ("Confirm", "yes"),
            };
            let mark = if destructive { format!("{} ", t.glyph("✗", "!")) } else { String::new() };
            let inner =
                dialog(f, t, r, dialog_title(t, &format!("{mark}{title}")), &[hint("y", yes, 0), hint("n/Esc", "cancel", 0)], destructive);
            // One line reads best centred; a wrapped question reads best left-aligned.
            let align = if body.len() == 1 { Alignment::Center } else { Alignment::Left };
            let mut lines = vec![Line::raw("")];
            lines.extend(body.into_iter().map(|l| Line::styled(l, t.text())));
            f.render_widget(Paragraph::new(lines).alignment(align), inner);
        }
    }
}

/// The list mark for a host's status: `●` up, `◌` checking, `✗` down or
/// failed, `○` unknown or not checked (shape differs, not only color); in
/// plain mode a word instead, for screen readers.
fn status_symbol(t: &Theme, s: Option<&HostStatus>) -> (&'static str, Style) {
    if t.plain {
        let word = match s {
            Some(HostStatus::Online { .. }) => "online",
            Some(HostStatus::Offline(_)) => "offline",
            Some(HostStatus::NotSsh) => "not ssh",
            Some(HostStatus::Checking) => "checking",
            Some(HostStatus::Unknown(_)) => "unknown",
            Some(HostStatus::Via(_, s)) if matches!(**s, HostStatus::Online { .. }) => "via jump",
            Some(HostStatus::Via(..)) => "jump down",
            Some(HostStatus::Unchecked) | None => "-",
        };
        return (word, t.muted());
    }
    let (up, busy, down, unknown) = (t.glyph("●", "*"), t.glyph("◌", "~"), t.glyph("✗", "x"), t.glyph("○", "o"));
    match s {
        Some(HostStatus::Online { .. }) => (up, t.success()),
        Some(HostStatus::Offline(_)) => (down, t.error()),
        Some(HostStatus::NotSsh) => (down, t.warning()),
        Some(HostStatus::Checking) => (busy, t.accent()),
        Some(HostStatus::Unknown(_)) => (unknown, t.muted()),
        // Only the jump host was checked: the hollow mark, in the up color.
        Some(HostStatus::Via(_, s)) if matches!(**s, HostStatus::Online { .. }) => (unknown, t.success()),
        Some(HostStatus::Via(..)) => (down, t.warning()),
        Some(HostStatus::Unchecked) | None => (unknown, t.muted()),
    }
}

/// One `label  value` line of a form; places the cursor in the focused input.
#[allow(clippy::too_many_arguments)]
fn field_row(f: &mut Frame, t: &Theme, inner: Rect, i: u16, label: &str, value: &str, input: Option<&TextInput>, focused: bool) {
    let y = inner.y + i;
    if y >= inner.bottom() {
        return;
    }
    let label_w = 17u16;
    let style = if focused { t.accent().add_modifier(Modifier::BOLD) } else { t.muted() };
    f.render_widget(Paragraph::new(Span::styled(format!("{label:>15}  "), style)), Rect::new(inner.x, y, label_w, 1));
    let area = Rect::new(inner.x + label_w, y, inner.width.saturating_sub(label_w), 1);
    let value_style = if focused { t.text().add_modifier(Modifier::UNDERLINED) } else { t.text() };
    f.render_widget(Paragraph::new(value.to_string()).style(value_style), area);
    if let (true, Some(input)) = (focused, input) {
        set_cursor(f, area.x, y, input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The epoch and a leap day render as the right UTC date and time.
    #[test]
    fn formats_time() {
        assert_eq!(format_time(UNIX_EPOCH), "1970-01-01 00:00");
        // 2024-02-29 12:34 UTC
        assert_eq!(format_time(UNIX_EPOCH + Duration::from_secs(1_709_210_040)), "2024-02-29 12:34");
    }
}
