use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};

/// Marks the selected row in every list (not only by color).
const MARK: &str = "› ";
use ratatui::Frame;
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::app::{App, Modal, SplitDir, TabKind};
use crate::forms::{ForwardForm, FwdRow, Row as EdRow};
use crate::input::TextInput;
use crate::keys::Action;
use crate::sftp_tab::{Pane, SftpTab, Side, Status, TransferState, human_size};
use crate::tui_theme::Theme;
use termax_core::askpass::AskKind;
use termax_core::status::Status as HostStatus;
use termax_core::terminal::TerminalSession;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.theme.plain = app.plain;
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
    let states: Vec<String> = match app.modal {
        Some(Modal::Forwards { .. }) => (0..app.cfg.forwards.len()).map(|i| app.forward_state(i)).collect(),
        _ => vec![],
    };
    draw_modal(f, app, area, &states);
}

fn block<'a>(t: &Theme, title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    if t.plain {
        // The title stays as a text line; no box-drawing for readers to spell out.
        return Block::new().title(title.into().style(if focused { t.title() } else { t.muted() })).style(t.base());
    }
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.border(focused))
        .title(title.into().style(if focused { t.title() } else { t.muted() }))
        .style(t.base())
}

fn draw_tab_bar(f: &mut Frame, app: &mut App, area: Rect) {
    let t = &app.theme;
    let mut spans = vec![Span::styled(" termax ", t.title()), Span::styled("│", t.muted())];
    let mut x = area.x + unicode_width(" termax │");
    let mut cols = vec![];
    let waiting = app.waiting_tabs();
    for (i, tab) in app.tabs.iter().enumerate() {
        let asks = waiting.contains(&i);
        let marker = match &tab.kind {
            _ if asks => " ?",
            TabKind::Terminal(_) => "",
            TabKind::Sftp(s) if matches!(s.status, Status::Failed(_)) => " ✗",
            _ => "",
        };
        let label = format!(" {}:{}{marker} ", i + 1, tab.title);
        let w = unicode_width(&label);
        cols.push((x, x + w));
        x += w + 1;
        let style = match () {
            _ if i == app.active => t.selected(),
            _ if asks => Style::new().fg(t.warning),
            _ => t.muted(),
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::styled("│", t.muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(t.base()), area);
    app.tab_cols = cols;
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if let Some(Modal::Search { query, .. }) = &app.modal {
        let count = app.search_status().unwrap_or_default();
        let head = " Find: ";
        let line = Line::from(vec![
            Span::styled(head, t.title()),
            Span::raw(query.value.clone()),
            Span::styled(format!("  {count}  · Enter older · Shift+Enter newer · Esc close"), t.muted()),
        ]);
        f.render_widget(Paragraph::new(line).style(t.base()), area);
        set_cursor(f, area.x + head.len() as u16, area.y, query);
        return;
    }
    let waiting = app.waiting_tabs();
    let notice = app.notices.first().map(|(host, req)| Span::styled(format!(" {host}: {}", req.prompt.trim()), Style::new().fg(t.warning)));
    let line = if let Some((msg, true, _)) = &app.status {
        // An error first, with a pending security-key notice after it.
        let mut spans = vec![Span::styled(format!(" Error: {msg}"), Style::new().fg(t.error))];
        if let Some(n) = notice {
            spans.extend([Span::styled(" ·", t.muted()), n]);
        }
        Line::from(spans)
    } else if let Some(n) = notice {
        Line::from(n)
    } else if let Some((msg, is_err, _)) = &app.status {
        let kind = if *is_err { "Error" } else { "Info" };
        Line::from(Span::styled(format!(" {kind}: {msg}"), Style::new().fg(if *is_err { t.error } else { t.success })))
    } else if let Some(&i) = waiting.iter().find(|&&i| i != app.active) {
        // Only tabs 1-9 have a shortcut.
        let key = if i < 9 { format!(" ({})", app.keys.show(Action::Tab(i as u8 + 1))) } else { String::new() };
        let text = format!(" {} is waiting for an answer: switch to tab {}{key}", app.tabs[i].title, i + 1);
        Line::from(Span::styled(text, Style::new().fg(t.warning)))
    } else {
        let hints = match &app.tabs[app.active].kind {
            TabKind::Hosts if app.filtering => "type to filter · ↑↓ move · Enter connect · Esc clear".to_string(),
            TabKind::Hosts => {
                "Enter ssh · s sftp · / filter · n new · e edit · d delete · p status · o sort · H hidden · f forward · ? help".to_string()
            }
            TabKind::Terminal(_) => {
                let k = &app.keys;
                let split = if app.tabs[app.active].split.is_some() {
                    format!("{} other pane", k.show(Action::FocusNextPane))
                } else {
                    format!("{} split", k.show(Action::SplitRight))
                };
                format!(
                    "{} tabs · {} close · {} find · {} snippets · {split} · Shift+PgUp/PgDn scroll · {} help",
                    k.show_tabs(),
                    k.show(Action::CloseTab),
                    k.show(Action::Find),
                    k.show(Action::Snippets),
                    k.show(Action::Help)
                )
            }
            TabKind::Sftp(_) => {
                "Tab pane · Enter open · Bksp up · c copy · X cancel copy · m mkdir · R rename · d delete · g goto · . hidden · r refresh"
                    .to_string()
            }
        };
        Line::from(Span::styled(format!(" {hints}"), t.muted()))
    };
    f.render_widget(Paragraph::new(line).style(t.base()), area);
}

// ---------------------------------------------------------------- hosts

fn draw_hosts(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let [left, right] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);

    let mut title = format!(" Hosts ({}/{}) · sort: {} ", app.filtered.len(), app.hosts.len(), app.ui_state.sort.label());
    if app.ui_state.show_hidden {
        title.push_str("· hidden shown ");
    }
    let outer = block(&t, title, true);
    let inner = outer.inner(left);
    f.render_widget(outer, left);
    let [filter_area, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    let filter_style = if app.filtering { t.accent() } else { t.muted() };
    let filter_text = if app.filter.value.is_empty() && !app.filtering { "/ to filter".to_string() } else { app.filter.value.clone() };
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(" ", filter_style), Span::styled(filter_text, filter_style)])),
        filter_area,
    );
    if app.filtering {
        set_cursor(f, filter_area.x + 1, filter_area.y, &app.filter);
    }

    if app.hosts.is_empty() {
        f.render_widget(
            Paragraph::new("\nNo hosts yet.\n\nPress n to add a connection, or add\nHost entries to ~/.ssh/config.")
                .style(t.muted())
                .alignment(Alignment::Center),
            list_area,
        );
    } else {
        let name_w = app.filtered.iter().map(|&i| app.hosts[i].name.chars().count()).max().unwrap_or(0).min(28);
        let items: Vec<ListItem> = app
            .filtered
            .iter()
            .map(|&i| {
                let h = &app.hosts[i];
                let (sym, style) = status_symbol(&t, app.statuses.get(&h.name));
                let mut spans = vec![
                    Span::styled(format!(" {sym} "), style),
                    Span::raw(format!("{:<name_w$}  ", h.name)),
                    Span::styled(h.target(), t.muted()),
                ];
                if h.hidden {
                    spans.push(Span::styled("  (hidden)", t.muted()));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let mut state = ListState::default().with_offset(app.host_offset).with_selected(Some(app.host_selected));
        let list = List::new(items).highlight_style(t.selected()).highlight_symbol(MARK).highlight_spacing(HighlightSpacing::Always);
        f.render_stateful_widget(list, list_area, &mut state);
        app.host_offset = state.offset();
        // The terminal cursor on the selection lets screen readers follow it.
        if !app.filtering && app.modal.is_none() {
            row_cursor(f, list_area, app.host_selected, app.host_offset);
        }
    }

    // Details pane
    let details = block(&t, " Details ", false);
    let inner = details.inner(right);
    f.render_widget(details, right);
    let Some(h) = app.selected_host() else { return };
    let row = |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:>13}  "), t.muted()), Span::raw(v)]);
    let dash = || "—".to_string();
    let mut cmd = vec![app.cfg.settings.ssh_binary.clone()];
    cmd.extend(h.shell_args());
    let lines = vec![
        Line::from(Span::styled(format!("  {}", h.name), t.title())),
        Line::raw(""),
        row("Host", h.hostname.clone()),
        row("User", h.user.clone().unwrap_or_else(dash)),
        row("Port", h.port.map(|p| p.to_string()).unwrap_or_else(|| "22".into())),
        row("Identity", h.identity_file.clone().unwrap_or_else(dash)),
        row("ProxyJump", h.proxy_jump.clone().unwrap_or_else(dash)),
        row("Remote dir", h.remote_dir.clone().unwrap_or_else(dash)),
        row("Tags", if h.tags.is_empty() { dash() } else { h.tags.join(", ") }),
        row("Source", h.source_label.clone()),
        row("Status", app.statuses.get(&h.name).cloned().unwrap_or_default().label()),
        row("Last used", app.history.ago(&h.name, termax_core::history::now()).unwrap_or_else(|| "never".into())),
        Line::raw(""),
        row("Command", shell_words::join(&cmd)),
        Line::raw(""),
        Line::from(Span::styled("  Enter: terminal   s: file transfer   e: edit   f: forward a port", t.muted())),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ---------------------------------------------------------------- terminal

/// The two pane areas of a split terminal: (first, second) = (left, right)
/// or (top, bottom).
pub fn split_areas(area: Rect, dir: SplitDir) -> (Rect, Rect) {
    let halves = [Constraint::Percentage(50), Constraint::Percentage(50)];
    let [a, b] = match dir {
        SplitDir::Right => Layout::horizontal(halves).areas(area),
        SplitDir::Down => Layout::vertical(halves).areas(area),
    };
    (a, b)
}

/// Draws one terminal pane (sized to fit `area`); returns its content area.
fn draw_session(f: &mut Frame, t: &Theme, s: &mut TerminalSession, area: Rect, title_text: &str, focused: bool, close: &str) -> Rect {
    let alive = s.is_alive();
    let scroll = s.scroll_offset();
    let mut title = vec![Span::styled(format!(" {title_text} "), if focused { t.title() } else { t.muted() })];
    if !alive {
        title.push(Span::styled(format!(" disconnected — Enter to reconnect, {close} to close "), Style::new().fg(t.error)));
    } else if scroll > 0 {
        title.push(Span::styled(format!(" scrollback +{scroll} "), Style::new().fg(t.warning)));
    }
    let b = block(t, Line::from(title), focused);
    let inner = b.inner(area);
    s.resize(inner.height, inner.width);
    // A poisoned lock (the output thread panicked) shows an empty frame
    // instead of taking the whole app down.
    let Ok(parser) = s.parser.lock() else {
        f.render_widget(b, area);
        return inner;
    };
    let mut cursor = Cursor::default();
    if !focused || !alive || scroll > 0 || parser.screen().hide_cursor() {
        cursor.hide();
    }
    f.render_widget(PseudoTerminal::new(parser.screen()).block(b).cursor(cursor), area);
    inner
}

fn draw_terminal(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let active = app.active;
    let tab = &mut app.tabs[active];
    let title_text = tab.title.clone();
    let (area, other) = match &tab.split {
        None => (area, None),
        Some(sp) => {
            let (a, b) = split_areas(area, sp.dir);
            if sp.focus_first { (a, Some(b)) } else { (b, Some(a)) }
        }
    };
    let close = app.keys.show(Action::CloseTab);
    app.other_pane = match (&mut tab.split, other) {
        (Some(sp), Some(r)) => Some(draw_session(f, &t, &mut sp.other, r, &title_text, false, &close)),
        _ => None,
    };
    let TabKind::Terminal(s) = &mut tab.kind else {
        return;
    };
    let inner = draw_session(f, &t, s, area, &title_text, true, &close);
    app_term_area(inner, &mut app.term_area);
    app.term_origin = (inner.x, inner.y);

    let TabKind::Terminal(s) = &app.tabs[active].kind else {
        return;
    };
    let Ok(parser) = s.parser.lock() else {
        return;
    };
    let top = s.scrolled().saturating_sub(parser.screen().scrollback() as u64);
    drop(parser);
    let buf = f.buffer_mut();
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
                Style::new().fg(t.warning).add_modifier(Modifier::REVERSED | Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().add_modifier(Modifier::REVERSED)
            };
            paint(row as u16, col as u16..(col + len) as u16, style);
        }
    }
}

fn app_term_area(inner: Rect, slot: &mut (u16, u16)) {
    *slot = (inner.height, inner.width);
}

// ---------------------------------------------------------------- sftp

fn draw_sftp(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let active = app.active;
    let TabKind::Sftp(s) = &mut app.tabs[active].kind else {
        return;
    };
    let s = s.as_mut();

    let transfer_h = if s.transfers.is_empty() { 0 } else { (s.transfers.len() as u16).min(6) + 2 };
    let [panes, transfers] = Layout::vertical([Constraint::Min(5), Constraint::Length(transfer_h)]).areas(area);
    let [left, right] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(panes);

    let focus = s.focus;
    let host = s.host_name.clone();
    let local_title = format!(" Local: {} ", s_path(&s.local));
    draw_pane(f, &t, &mut s.local, left, local_title, focus == Side::Local);

    match &s.status {
        Status::Connected => {
            let title = format!(" {host}: {} ", s_path(&s.remote));
            draw_pane(f, &t, &mut s.remote, right, title, focus == Side::Remote);
        }
        Status::Connecting => {
            let b = block(&t, format!(" {host} "), focus == Side::Remote);
            f.render_widget(
                Paragraph::new(format!("\nConnecting to {host}…")).alignment(Alignment::Center).style(t.muted()).block(b),
                right,
            );
        }
        Status::Failed(err) => {
            let b = block(&t, format!(" {host} "), focus == Side::Remote);
            let text = vec![
                Line::styled("Connection failed", Style::new().fg(t.error).add_modifier(Modifier::BOLD)),
                Line::raw(""),
                Line::raw(err.clone()),
                Line::raw(""),
                Line::styled(format!("r: retry   {}: close", app.keys.show(Action::CloseTab)), t.muted()),
            ];
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(b), right);
        }
    }

    if !s.transfers.is_empty() {
        draw_transfers(f, &t, s, transfers);
    }
}

fn s_path(p: &Pane) -> String {
    if p.loading { format!("{} …", p.path) } else { p.path.clone() }
}

fn draw_pane(f: &mut Frame, t: &Theme, pane: &mut Pane, area: Rect, title: String, focused: bool) {
    let b = block(t, title, focused);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let (list_area, err_area) = if pane.error.is_some() {
        let [a, e] = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(inner);
        (a, Some(e))
    } else {
        (inner, None)
    };

    let rows: Vec<Row> = pane
        .entries
        .iter()
        .map(|e| {
            let name_style = if e.is_dir { Style::new().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::new().fg(t.fg) };
            let mut name = e.name.clone();
            if e.is_dir && name != ".." {
                name.push('/');
            }
            if e.is_symlink {
                name.push_str(" →");
            }
            Row::new(vec![
                Cell::from(Span::styled(name, name_style)),
                Cell::from(Line::from(if e.is_dir { String::new() } else { human_size(e.size) }).alignment(Alignment::Right))
                    .style(t.muted()),
                Cell::from(e.modified.map(format_time).unwrap_or_default()).style(t.muted()),
            ])
        })
        .collect();
    let table = Table::new(rows, [Constraint::Min(10), Constraint::Length(8), Constraint::Length(16)])
        .column_spacing(1)
        .row_highlight_style(if focused { t.selected() } else { Style::new().add_modifier(Modifier::REVERSED) })
        .highlight_symbol(MARK)
        .highlight_spacing(HighlightSpacing::Always);
    let mut state = TableState::default().with_offset(pane.offset).with_selected(Some(pane.selected));
    f.render_stateful_widget(table, list_area, &mut state);
    pane.offset = state.offset();
    if focused {
        row_cursor(f, list_area, pane.selected, pane.offset);
    }

    if let (Some(err), Some(a)) = (&pane.error, err_area) {
        f.render_widget(Paragraph::new(err.as_str()).style(Style::new().fg(t.error)).wrap(Wrap { trim: true }), a);
    }
}

fn draw_transfers(f: &mut Frame, t: &Theme, s: &SftpTab, area: Rect) {
    let b = block(t, " Transfers (X: cancel newest running · x: clear finished) ", false);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let lines: Vec<Line> = s
        .transfers
        .iter()
        .rev()
        .take(inner.height as usize)
        .map(|tr| {
            let progress = match tr.total {
                Some(total) if total > 0 => {
                    format!("{} / {} ({}%)", human_size(tr.done), human_size(total), tr.done * 100 / total)
                }
                _ => human_size(tr.done),
            };
            let (state, style) = match &tr.state {
                TransferState::Running => ("…".to_string(), Style::new().fg(t.warning)),
                TransferState::Done => ("✓".to_string(), Style::new().fg(t.success)),
                TransferState::Cancelled => ("✗ cancelled".to_string(), Style::new().fg(t.warning)),
                TransferState::Failed(e) => (format!("✗ {e}"), Style::new().fg(t.error)),
            };
            let skipped = if tr.skipped > 0 { format!("  ({} skipped)", tr.skipped) } else { String::new() };
            Line::from(vec![
                Span::styled(format!(" {state} "), style),
                Span::raw(format!("{}  ", tr.label)),
                Span::styled(progress + &skipped, t.muted()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
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

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
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

fn set_cursor(f: &mut Frame, x: u16, y: u16, input: &TextInput) {
    let offset = unicode_width(input.before_cursor());
    f.set_cursor_position(Position::new(x + offset, y));
}

fn unicode_width(s: &str) -> u16 {
    Line::raw(s).width() as u16
}

/// `states`: each saved forward's state, while the forwards list is open.
fn draw_modal(f: &mut Frame, app: &App, area: Rect, states: &[String]) {
    let t = &app.theme;
    let Some(modal) = &app.modal else { return };
    match modal {
        Modal::Help { scroll } => {
            let r = centered(area, 76, 50);
            f.render_widget(Clear, r);
            let k = |key: &str, desc: &str| Line::from(vec![Span::styled(format!("  {key:<22}"), t.accent()), Span::raw(desc.to_string())]);
            let a = |action: Action| k(&app.keys.show(action), action.label());
            let h = |s: &str| Line::styled(s.to_string(), t.title());
            let mut lines: Vec<Line> = app.key_errors.iter().map(|e| Line::styled(format!("  {e}"), Style::new().fg(t.error))).collect();
            lines.extend([
                h("Everywhere ([tui_keys] in the config remaps these)"),
                k(&app.keys.show_tabs(), "switch to tab"),
                a(Action::PrevTab),
                a(Action::NextTab),
                a(Action::CloseTab),
                a(Action::Theme),
                a(Action::Help),
                a(Action::Quit),
                h("Hosts"),
                k("Enter", "open terminal (ssh)"),
                k("s", "open file transfer (sftp)"),
                k("/", "filter hosts"),
                k("n / e / d", "new / edit / delete host (ssh config too)"),
                k("p", "check which hosts are reachable"),
                k("o", "sort by name / recent"),
                k("H", "show / hide hidden hosts"),
                k("f / F", "forward a port / saved forwards"),
                k("r", "reload config files"),
                h("Host editor"),
                k("Tab / ↑↓ · Ctrl+S", "move between fields · save"),
                k("Space", "toggle Save to / Hidden"),
                k("Enter (Other options)", "add a line"),
                h("Terminal"),
                k("Shift+PgUp / PgDn", "scroll back"),
                a(Action::Find),
                a(Action::Snippets),
                a(Action::SplitRight),
                a(Action::SplitDown),
                a(Action::FocusNextPane),
                k("Enter (when closed)", "reconnect"),
                h("File transfer"),
                k("Tab / ← →", "switch pane"),
                k("Enter / Backspace", "open dir / go up"),
                k("c or F5", "copy to other pane"),
                k("m or F7", "make directory"),
                k("R or F2", "rename"),
                k("d, Del or F8", "delete"),
                k("g", "go to path"),
                k("X", "cancel the newest running transfer"),
                k(". / r / x", "hidden files / refresh / clear transfers"),
            ]);
            let b = block(t, format!(" Help (↑↓ PgUp PgDn scroll · Esc close) — config: {} ", app.cfg_path.display()), true);
            let visible = b.inner(r).height;
            let scroll = (*scroll).min((lines.len() as u16).saturating_sub(visible));
            f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(b), r);
        }
        Modal::ThemePicker { selected, .. } => {
            let themes = app.all_themes();
            let r = centered(area, 40, themes.len() as u16 + 4);
            f.render_widget(Clear, r);
            let items: Vec<ListItem> = themes
                .iter()
                .map(|d| {
                    let swatch = |c: &str| Span::styled("■", Style::new().fg(std::str::FromStr::from_str(c).unwrap_or_default()));
                    ListItem::new(Line::from(vec![
                        Span::raw(format!(" {:<18}", d.name)),
                        swatch(&d.bg),
                        swatch(&d.fg),
                        swatch(&d.accent),
                        swatch(&d.success),
                        swatch(&d.warning),
                        swatch(&d.error),
                    ]))
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(*selected));
            let b = block(t, " Theme (Enter save · Esc cancel) ", true);
            let inner = b.inner(r);
            let list = List::new(items).highlight_style(t.selected()).highlight_symbol(MARK).block(b);
            f.render_stateful_widget(list, r, &mut state);
            row_cursor(f, inner, *selected, state.offset());
        }
        Modal::Editor(ed) => {
            let rows = ed.rows();
            let r = centered(area, 76, rows.len() as u16 + 6);
            f.render_widget(Clear, r);
            let b = block(t, ed.title(), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let mut hint = "";
            for (i, row) in rows.iter().enumerate() {
                let focused = i == ed.focus;
                let (label, value, input) = match *row {
                    EdRow::Dest => ("Save to", format!("‹ {} ›", ed.dest_label()), None),
                    EdRow::Field(fl) => {
                        if focused {
                            hint = fl.hint();
                        }
                        let input = ed.input(fl);
                        (fl.label(), input.map(|i| i.value.clone()).unwrap_or_default(), input)
                    }
                    EdRow::Hidden => ("Hidden", if ed.form.hidden { "[x] left out of host lists" } else { "[ ]" }.to_string(), None),
                    EdRow::Other(j) => (if j == 0 { "Other options" } else { "" }, ed.other[j].value.clone(), Some(&ed.other[j])),
                };
                if matches!(row, EdRow::Other(_)) && focused {
                    hint = "one option per line, e.g. ForwardAgent yes · Enter adds a line";
                }
                field_row(f, t, inner, i as u16, label, &value, input, focused);
            }
            let footer = match &ed.error {
                Some(e) => Line::styled(format!(" {e}"), Style::new().fg(t.error)),
                None if !hint.is_empty() => Line::styled(format!(" {hint}"), t.muted()),
                None => Line::styled(" Tab/↑↓ move · Space toggle · Ctrl+S save · Esc cancel", t.muted()),
            };
            let y = inner.y + rows.len() as u16 + 1;
            if y < inner.bottom() {
                f.render_widget(Paragraph::new(footer).wrap(Wrap { trim: true }), Rect::new(inner.x, y, inner.width, inner.bottom() - y));
            }
        }
        Modal::Forward(form) => {
            let rows = form.rows();
            let r = centered(area, 76, rows.len() as u16 + 8);
            f.render_widget(Clear, r);
            let b = block(t, format!(" Forward a port — {} ", form.host), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            for (i, row) in rows.iter().enumerate() {
                let input = form.input(*row);
                let value = match row {
                    FwdRow::Kind => format!("‹ {} ›", form.kind.label()),
                    _ => input.map(|i| i.value.clone()).unwrap_or_default(),
                };
                field_row(f, t, inner, i as u16, ForwardForm::label(*row), &value, input, i == form.focus);
            }
            let y = inner.y + rows.len() as u16 + 1;
            let mut lines = vec![Line::styled(format!(" {}", form.kind.explain()), t.muted())];
            if let Some(e) = &form.error {
                lines.insert(0, Line::styled(format!(" {e}"), Style::new().fg(t.error)));
            } else {
                lines.push(Line::styled(" Bind address defaults to 127.0.0.1 · Ctrl+S save and start · Esc cancel", t.muted()));
            }
            if y < inner.bottom() {
                f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), Rect::new(inner.x, y, inner.width, inner.bottom() - y));
            }
        }
        Modal::Forwards { selected } => {
            let list = &app.cfg.forwards;
            let r = centered(area, 76, list.len().max(1) as u16 + 4);
            f.render_widget(Clear, r);
            let b = block(t, " Saved forwards (Enter start · s stop · e edit · d delete · Esc close) ", true);
            if list.is_empty() {
                f.render_widget(Paragraph::new("No saved forwards. Select a host and press f.").style(t.muted()).block(b), r);
            } else {
                let items: Vec<ListItem> =
                    list.iter().zip(states).map(|(fw, st)| ListItem::new(format!(" {}  —  {st}", fw.title()))).collect();
                let mut state = ListState::default().with_selected(Some(*selected));
                let inner = b.inner(r);
                f.render_stateful_widget(List::new(items).highlight_style(t.selected()).highlight_symbol(MARK).block(b), r, &mut state);
                row_cursor(f, inner, *selected, state.offset());
            }
        }
        Modal::Prompt(p) => {
            let r = centered(area, 60, 3);
            f.render_widget(Clear, r);
            let b = block(t, format!(" {} (Enter ok · Esc cancel) ", p.title), true);
            let inner = b.inner(r);
            f.render_widget(Paragraph::new(p.input.value.as_str()).block(b), r);
            set_cursor(f, inner.x, inner.y, &p.input);
        }
        Modal::Search { .. } => {} // drawn on the status line
        Modal::Ask { host, input, req } => {
            let secret = req.kind == AskKind::Secret;
            let prompt: Vec<Line> = req.prompt.trim_end().lines().map(|l| Line::raw(l.to_string())).collect();
            let r = centered(area, 76, prompt.len() as u16 + 6);
            f.render_widget(Clear, r);
            let b = block(t, format!(" {host} — ssh asks "), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let hint = if secret { "Enter send · Esc cancel" } else { "y yes · n no · Esc cancel" };
            let [text, field, help] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(if secret { 2 } else { 0 }), Constraint::Length(1)]).areas(inner);
            f.render_widget(Paragraph::new(prompt).wrap(Wrap { trim: false }), text);
            if secret {
                // Masked: one dot per character typed, never the text.
                let dots = "•".repeat(input.value.chars().count());
                f.render_widget(Paragraph::new(Span::styled(format!("> {dots}"), t.accent())), Rect { y: field.y + 1, height: 1, ..field });
                let before = input.value.chars().take(input.cursor).count() as u16;
                f.set_cursor_position(Position::new(field.x + 2 + before, field.y + 1));
            }
            f.render_widget(Paragraph::new(Line::styled(hint, t.muted())), help);
        }
        Modal::Overwrite { host, path, .. } => {
            let text = format!("{path} already exists ({host}). Replace it?");
            let w = (unicode_width(&text) + 6).clamp(40, 80);
            let r = centered(area, w, 6);
            f.render_widget(Clear, r);
            let lines = vec![
                Line::raw(text),
                Line::raw(""),
                Line::styled("o overwrite · s skip · a overwrite all · S skip all · c/Esc cancel transfer", t.muted()),
            ];
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block(t, " File exists ", true)), r);
        }
        Modal::Snippets { selected } => {
            let list = app.tab_snippets();
            let r = centered(area, 76, list.len().max(1) as u16 + 4);
            f.render_widget(Clear, r);
            let b = block(t, " Snippets (Enter send · Esc close) ", true);
            if list.is_empty() {
                f.render_widget(Paragraph::new("No snippets. Add [[snippet]] entries to config.toml.").style(t.muted()).block(b), r);
            } else {
                let items: Vec<ListItem> = list.iter().map(|s| ListItem::new(format!(" {}  —  {}", s.name, s.command))).collect();
                let mut state = ListState::default().with_selected(Some(*selected));
                let inner = b.inner(r);
                f.render_stateful_widget(List::new(items).highlight_style(t.selected()).highlight_symbol(MARK).block(b), r, &mut state);
                row_cursor(f, inner, *selected, state.offset());
            }
        }
        Modal::Confirm { text, .. } => {
            let w = (unicode_width(text) + 6).clamp(30, 80);
            let r = centered(area, w, 5);
            f.render_widget(Clear, r);
            let lines = vec![Line::raw(text.clone()), Line::raw(""), Line::styled("y: yes   n: no", t.muted())];
            f.render_widget(
                Paragraph::new(lines).alignment(Alignment::Center).wrap(Wrap { trim: true }).block(block(t, " Confirm ", true)),
                r,
            );
        }
    }
}

/// The list symbol for a host's status (shape differs too, not only color).
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
    match s {
        Some(HostStatus::Online { .. }) => ("●", Style::new().fg(t.success)),
        Some(HostStatus::Offline(_)) => ("✕", Style::new().fg(t.error)),
        Some(HostStatus::NotSsh) => ("!", Style::new().fg(t.warning)),
        Some(HostStatus::Checking) => ("…", t.muted()),
        Some(HostStatus::Unknown(_)) => ("?", t.muted()),
        // Only the jump host was checked: a hollow mark, not the solid dot.
        Some(HostStatus::Via(_, s)) if matches!(**s, HostStatus::Online { .. }) => ("○", Style::new().fg(t.success)),
        Some(HostStatus::Via(..)) => ("✕", Style::new().fg(t.warning)),
        Some(HostStatus::Unchecked) | None => ("·", t.muted()),
    }
}

/// One `label: value` line of a form; places the cursor in the focused input.
#[allow(clippy::too_many_arguments)]
fn field_row(f: &mut Frame, t: &Theme, inner: Rect, i: u16, label: &str, value: &str, input: Option<&TextInput>, focused: bool) {
    let y = inner.y + i;
    if y >= inner.bottom() {
        return;
    }
    let label_w = 17u16;
    let style = if focused { t.accent() } else { t.muted() };
    let mark = if label.is_empty() { "  " } else { ": " };
    f.render_widget(Paragraph::new(Span::styled(format!("{label:>15}{mark}"), style)), Rect::new(inner.x, y, label_w, 1));
    let area = Rect::new(inner.x + label_w, y, inner.width.saturating_sub(label_w), 1);
    let value_style = if focused { Style::new().fg(t.fg).add_modifier(Modifier::UNDERLINED) } else { Style::new().fg(t.fg) };
    f.render_widget(Paragraph::new(value.to_string()).style(value_style), area);
    if let (true, Some(input)) = (focused, input) {
        set_cursor(f, area.x, y, input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_time() {
        assert_eq!(format_time(UNIX_EPOCH), "1970-01-01 00:00");
        // 2024-02-29 12:34 UTC
        assert_eq!(format_time(UNIX_EPOCH + Duration::from_secs(1_709_210_040)), "2024-02-29 12:34");
    }
}
