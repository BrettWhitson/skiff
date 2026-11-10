//! Demo UI shared by `examples/demo.rs` and the render-time test. It
//! exercises every widget: a filterable 100k-row list in a resizable
//! sidebar, tabs, and two pages — "Components" (the design system: button
//! variants, icon buttons, badges, status dots and pills, the icon set,
//! the type scale, path elision, a sortable table, a stepper, an empty
//! state, a modal dialog and toasts) and "Form" (inputs, checkbox,
//! dropdown) — plus a context menu with icons, separators and a danger item.

use std::time::Instant;

use crate::a11y::SortDir;
use crate::geom::Rect;
use crate::icons::Icon;
use crate::layout::Size as L;
use crate::style::{TextRole, tokens};
use crate::text::FontKind;
use crate::ui::{Id, Ui};
use crate::widgets::{
    Align, ButtonKind, ButtonOpts, Column, Dialog, DialogButton, DialogResult, Elide, EmptyState, MenuItem, Sort, Status, StepperOpts,
    TabTitle, TextInputOpts, TextOpts, Toast, ToastEvent, ToastKind, Toasts, Tone, badge, button, button_with, checkbox, context_menu,
    dialog, dropdown_labeled, empty_state, icon_button, label, label_with, list, open_context_menu, section_header, splitter_h, status_dot,
    status_pill, stepper, table_header, tabs, text, text_input, toasts,
};

/// State of the demo screen, kept across frames.
pub struct Demo {
    /// 100,000 fake host names.
    items: Vec<String>,
    filter: String,
    /// Indexes into `items` that match `filter`.
    filtered: Vec<usize>,
    /// Selected row in the filtered list.
    selected: Option<usize>,
    tabs: Vec<String>,
    /// Index of the active tab (0 Components, 1 Form, others empty).
    pub active: usize,
    /// Number for the next "Session N" tab.
    next_tab: usize,
    /// Sidebar width in logical pixels (the splitter changes it).
    sidebar: f32,
    host: String,
    user: String,
    password: String,
    remember: bool,
    /// Index of the chosen auth method in the form's dropdown.
    auth: usize,
    /// Status-bar message.
    status: String,
    /// Filtered row the context menu was opened on.
    menu_row: Option<usize>,
    /// How long the previous frame took to build, shown in the status bar.
    build_ms: f64,
    /// The delete-confirmation dialog is showing.
    pub dialog_open: bool,
    /// Notifications on screen.
    pub toasts: Toasts,
    /// Sort of the components page's table.
    sort: Option<Sort>,
    /// The stepper's value.
    port: i64,
}

impl Default for Demo {
    /// 100k hosts, three tabs, an empty form.
    fn default() -> Self {
        let items: Vec<String> = (0..100_000).map(|i| format!("host-{i:05}.example.test")).collect();
        let filtered = (0..items.len()).collect();
        Demo {
            items,
            filter: String::new(),
            filtered,
            selected: None,
            tabs: vec!["Components".into(), "Form".into(), "Empty".into()],
            active: 0,
            next_tab: 1,
            sidebar: 260.0,
            host: String::new(),
            user: String::new(),
            password: String::new(),
            remember: true,
            auth: 0,
            status: "ready".into(),
            menu_row: None,
            build_ms: 0.0,
            dialog_open: false,
            toasts: Toasts::default(),
            sort: Some(Sort { column: 0, dir: SortDir::Ascending }),
            port: 22,
        }
    }
}

/// Context-menu entries: icons, a shortcut, a disabled item, separators
/// and a danger item.
const MENU: [MenuItem<'static>; 7] = [
    MenuItem::new("Connect").icon(Icon::Terminal).shortcut("Enter"),
    MenuItem::new("Open files").icon(Icon::Folder).shortcut("Ctrl+Shift+F"),
    MenuItem::new("Edit…").icon(Icon::Edit).enabled(false),
    MenuItem::SEPARATOR,
    MenuItem::new("Copy name").icon(Icon::Copy).shortcut("Ctrl+C"),
    MenuItem::SEPARATOR,
    MenuItem::new("Delete").icon(Icon::Trash).shortcut("Del").danger(),
];

/// Rows of the components page's table: (icon, name, size, path).
const FILES: [(Icon, &str, u64, &str); 5] = [
    (Icon::Folder, "projects", 0, "/home/demo/projects"),
    (Icon::File, "notes.md", 4_812, "/home/demo/documents/2026/notes.md"),
    (Icon::Link, "current", 0, "/srv/releases/2026-11-10/current"),
    (Icon::File, "backup.tar.gz", 734_003_200, "/var/backups/nightly/host-00042/backup.tar.gz"),
    (Icon::File, "id_ed25519.pub", 96, "/home/demo/.ssh/id_ed25519.pub"),
];

impl Demo {
    /// Builds the whole screen: sidebar list, tab strip, page, status bar
    /// and the overlays (menu, dialog, toasts).
    pub fn ui(&mut self, ui: &mut Ui) {
        let t0 = Instant::now();
        let p = ui.style().palette;
        let win = ui.window_rect();
        let rows = ui.split_v(win, &[L::Fill, L::Px(26)], 0);
        let (side, main) = splitter_h(ui, Id::new("split"), rows[0], &mut self.sidebar, 160.0, 600.0, "Sidebar width");

        // Sidebar: filter, section header, list.
        ui.painter().rect(side, p.surface, 0.0);
        let inner = side.inset(ui.px(tokens::SPACE_SM));
        let parts = ui.split_v(inner, &[L::Px(tokens::CONTROL_H), L::Px(tokens::HEADER_H), L::Fill], tokens::SPACE_XS);
        let f = text_input(
            ui,
            Id::new("filter"),
            parts[0],
            &mut self.filter,
            TextInputOpts { password: false, placeholder: "Filter hosts…", ..Default::default() },
        );
        if f.changed {
            let needle = self.filter.to_lowercase();
            self.filtered = (0..self.items.len()).filter(|&i| self.items[i].contains(&needle)).collect();
            self.selected = None;
        }
        section_header(ui, parts[1].inset(ui.px(tokens::SPACE_2XS)), "Hosts", Some(self.filtered.len()));
        // Double-click / Enter reports the row; right-click or the menu key opens the context menu.
        let (items, filtered) = (&self.items, &self.filtered);
        let lr = list(ui, Id::new("hosts"), parts[2], filtered.len(), tokens::ROW_H, &mut self.selected, |ui, row, i, _| {
            let pad = ui.px(tokens::SPACE_SM);
            let dot = ui.px(tokens::SPACE_MD);
            let k = filtered[i];
            let st = [Status::Ok, Status::Idle, Status::Idle, Status::Error, Status::Connecting][k % 5];
            status_dot(ui, Id::new(("dot", k)), Rect::new(row.x + pad, row.y, dot, row.h), st, "status");
            let r = Rect::new(row.x + 2 * pad + dot, row.y, row.w - 3 * pad - dot, row.h);
            text(ui, r, &items[k], TextOpts { role: Some(TextRole::Body), ..Default::default() });
        });
        if let Some(i) = lr.activated {
            self.status = format!("activated {}", self.items[self.filtered[i]]);
        }
        if let Some((i, at)) = lr.context {
            self.menu_row = Some(i);
            open_context_menu(ui, Id::new("menu"), at);
        }

        // Main: tabs + page.
        let mparts = ui.split_v(main, &[L::Px(tokens::TAB_H), L::Fill], 0);
        let titles: Vec<TabTitle> = self.tabs.iter().map(|t| TabTitle { text: t, ..TabTitle::DEFAULT }).collect();
        let tr = tabs(ui, Id::new("tabs"), mparts[0], &titles, &mut self.active);
        if let Some(i) = tr.closed {
            self.tabs.remove(i);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
        if let Some((a, b)) = tr.moved {
            let t = self.tabs.remove(a);
            self.tabs.insert(b, t);
        }
        if tr.plus_clicked {
            self.tabs.push(format!("Session {}", self.next_tab));
            self.next_tab += 1;
            self.active = self.tabs.len() - 1;
        }
        let body = mparts[1];
        ui.painter().rect(body, p.surface, 0.0);
        match self.tabs.get(self.active).map(String::as_str) {
            Some("Components") => self.components(ui, body.inset(ui.px(tokens::SPACE_XL))),
            Some("Form") => self.form(ui, body.inset(ui.px(tokens::SPACE_XL))),
            _ => {
                let e = EmptyState {
                    icon: Some(Icon::Host),
                    title: "No hosts yet",
                    body: "Add a host to connect to it, or import the hosts you already have in your SSH config.",
                    action: Some("Add host"),
                    secondary: Some("Import ~/.ssh/config"),
                    hint: Some("or press Ctrl+K for the command palette"),
                };
                let r = empty_state(ui, Id::new("empty"), body, &e);
                if r.action || r.secondary {
                    self.status = "empty state action".into();
                }
            }
        }

        // Status bar.
        let sb = rows[1];
        ui.painter().rect(sb, p.bg, 0.0);
        let line = Rect::new(sb.x, sb.y, sb.w, ui.px(1).max(1));
        ui.painter().rect(line, p.border, 0.0);
        let text_s =
            format!("{} · {} of {} hosts · frame build {:.2} ms", self.status, self.filtered.len(), self.items.len(), self.build_ms);
        let pad = ui.px(tokens::SPACE_SM);
        label_with(ui, Rect::new(sb.x + pad, sb.y, sb.w - 2 * pad, sb.h), &text_s, FontKind::Ui, p.muted);

        // Context menu: report the choice; "Copy name" also copies the host name.
        if let Some(i) = context_menu(ui, Id::new("menu"), &MENU) {
            let name = self.menu_row.and_then(|r| self.filtered.get(r)).map(|&k| self.items[k].clone()).unwrap_or_default();
            self.status = format!("{} → {name}", MENU[i].label);
            match i {
                4 => ui.clipboard().set(&name),
                6 => self.dialog_open = true,
                _ => {}
            }
        }
        if self.dialog_open {
            let buttons =
                [DialogButton { label: "Cancel", kind: ButtonKind::Secondary }, DialogButton { label: "Delete", kind: ButtonKind::Danger }];
            let d = Dialog {
                title: "Delete host-00042.example.test?",
                body: "The host and its saved settings are removed from ~/.ssh/config. Open sessions stay open. This cannot be undone.",
                buttons: &buttons,
                initial: 0,
                icon: Some(Icon::Warning),
                ..Default::default()
            };
            if let Some(r) = dialog(ui, Id::new("confirm"), &d) {
                self.dialog_open = false;
                if r == DialogResult::Button(1) {
                    self.toasts.push(Toast::new(ToastKind::Success, "Deleted host-00042.example.test").with_action("Undo"));
                }
            }
        }
        match toasts(ui, Id::new("toasts"), &mut self.toasts) {
            Some(ToastEvent::Action(_)) => self.status = "undo".into(),
            Some(ToastEvent::Dismissed(_)) | None => {}
        }
        self.build_ms = t0.elapsed().as_secs_f64() * 1000.0;
    }

    /// The design-system showcase in `area`, in two columns.
    fn components(&mut self, ui: &mut Ui, area: Rect) {
        let cols = ui.split_h(area, &[L::Fill, L::Fill], tokens::SPACE_2XL);
        ui.with_clip(cols[0], |ui| self.components_left(ui, cols[0]));
        ui.with_clip(cols[1], |ui| self.components_right(ui, cols[1]));
    }

    /// Left showcase column: buttons, badges and status, a sortable table
    /// and a stepper.
    fn components_left(&mut self, ui: &mut Ui, left: Rect) {
        let p = ui.style().palette;
        let hh = ui.px(tokens::HEADER_H);
        let ctl = ui.px(tokens::CONTROL_H);
        let gap = ui.px(tokens::SPACE_SM);
        let section = ui.px(tokens::SPACE_XL);

        // Buttons.
        let mut y = left.y;
        section_header(ui, Rect::new(left.x, y, left.w, hh), "Buttons", None);
        y += hh + gap;
        let kinds = [
            ("Connect", ButtonKind::Primary),
            ("Cancel", ButtonKind::Secondary),
            ("Skip", ButtonKind::Ghost),
            ("Delete", ButtonKind::Danger),
        ];
        let mut x = left.x;
        for (i, (t, k)) in kinds.iter().enumerate() {
            let w = ui.px(88);
            let r = button_with(ui, Id::new(("kind", i)), Rect::new(x, y, w, ctl), t, ButtonOpts::kind(*k));
            if r.clicked && *k == ButtonKind::Danger {
                self.dialog_open = true;
            }
            x += w + gap;
        }
        y += ctl + gap;
        let mut x = left.x;
        let w = ui.px(124);
        let add = ButtonOpts { icon: Some(Icon::Plus), ..ButtonOpts::kind(ButtonKind::Primary) };
        if button_with(ui, Id::new("add"), Rect::new(x, y, w, ctl), "Add host", add).clicked {
            self.toasts.push(Toast::new(ToastKind::Info, "Host added. Double-click it to connect."));
        }
        x += w + gap;
        let off = ButtonOpts { enabled: false, ..Default::default() };
        button_with(ui, Id::new("disabled"), Rect::new(x, y, ui.px(96), ctl), "Disabled", off);
        x += ui.px(96) + gap;
        for (i, (icon, name, k)) in [
            (Icon::Settings, "Settings", ButtonKind::Ghost),
            (Icon::Search, "Search", ButtonKind::Ghost),
            (Icon::Refresh, "Refresh", ButtonKind::Secondary),
            (Icon::Trash, "Delete", ButtonKind::Danger),
        ]
        .into_iter()
        .enumerate()
        {
            if icon_button(ui, Id::new(("ib", i)), Rect::new(x, y, ctl, ctl), icon, name, k).clicked && i == 1 {
                self.toasts.push(Toast::new(ToastKind::Error, "Connection to db-primary failed: host key changed.").with_action("Details"));
            }
            x += ctl + gap / 2;
        }
        y += ctl + section;

        // Badges and status.
        section_header(ui, Rect::new(left.x, y, left.w, hh), "Badges & status", None);
        y += hh + gap;
        let mut x = left.x;
        for (t, tone) in [
            ("prod", Tone::Neutral),
            ("ssh", Tone::Accent),
            ("info", Tone::Info),
            ("ok", Tone::Success),
            ("slow", Tone::Warning),
            ("down", Tone::Danger),
        ] {
            x = badge(ui, Rect::new(x, y, left.right() - x, ctl), t, tone).right() + gap;
        }
        y += ctl;
        let mut x = left.x;
        for (i, (s, t)) in
            [(Status::Ok, "Running"), (Status::Connecting, "Connecting"), (Status::Idle, "Stopped"), (Status::Error, "Failed")]
                .into_iter()
                .enumerate()
        {
            x = status_pill(ui, Id::new(("pill", i)), Rect::new(x, y, left.right() - x, ctl), s, t).right() + gap;
        }
        y += ctl + section;

        // Table with a sortable header.
        section_header(ui, Rect::new(left.x, y, left.w, hh), "Table", Some(FILES.len()));
        y += hh;
        let columns = [
            Column::new("Name", L::Fill),
            Column { align: Align::End, ..Column::new("Size", L::Px(90)) },
            Column { sortable: false, ..Column::new("Path", L::Fill) },
        ];
        let th = table_header(ui, Id::new("table"), Rect::new(left.x, y, left.w, hh), &columns, &mut self.sort);
        y += hh;
        let mut order: Vec<usize> = (0..FILES.len()).collect();
        if let Some(s) = self.sort {
            order.sort_by(|&a, &b| {
                let o = if s.column == 1 { FILES[a].2.cmp(&FILES[b].2) } else { FILES[a].1.cmp(FILES[b].1) };
                if s.dir == SortDir::Ascending { o } else { o.reverse() }
            });
        }
        let row_h = ui.px(tokens::ROW_H);
        let pad = ui.px(tokens::SPACE_SM);
        let icon = ui.px(tokens::ICON);
        for (n, &i) in order.iter().enumerate() {
            let (ic, name, size, path) = FILES[i];
            let row = |c: Rect| Rect::new(c.x, y, c.w, row_h);
            if n % 2 == 1 {
                ui.painter().rect(Rect::new(left.x, y, left.w, row_h), p.hover, 0.0);
            }
            let c0 = row(th.columns[0]);
            ui.painter().icon(
                Rect::new(c0.x + pad, y + (row_h - icon) / 2, icon, icon),
                ic,
                if ic == Icon::Folder { p.accent } else { p.muted },
            );
            let nr = Rect::new(c0.x + 2 * pad + icon, y, c0.w - 3 * pad - icon, row_h);
            text(ui, nr, name, TextOpts::default());
            let sz = if size == 0 { "—".to_string() } else { human(size) };
            let c1 = row(th.columns[1]).inset(0);
            text(
                ui,
                Rect::new(c1.x, y, c1.w - pad, row_h),
                &sz,
                TextOpts { color: Some(p.muted), align: Align::End, ..Default::default() },
            );
            let c2 = row(th.columns[2]);
            let opts = TextOpts { color: Some(p.muted), elide: Elide::Middle, ..Default::default() };
            text(ui, Rect::new(c2.x + pad, y, c2.w - pad, row_h), path, opts);
            y += row_h;
        }
        y += section;

        // Stepper.
        section_header(ui, Rect::new(left.x, y, left.w, hh), "Stepper", None);
        y += hh + gap;
        label(ui, Rect::new(left.x, y, ui.px(80), ctl), "Port", None);
        let opts = StepperOpts { min: 1, max: 65535, step: 1, label: "Port" };
        if stepper(ui, Id::new("port"), Rect::new(left.x + ui.px(80), y, ui.px(132), ctl), &mut self.port, opts) {
            self.status = format!("port {}", self.port);
        }
    }

    /// Right showcase column: the icon set, the type scale, path elision
    /// and an empty state.
    fn components_right(&mut self, ui: &mut Ui, right: Rect) {
        let p = ui.style().palette;
        let (hh, gap, section) = (ui.px(tokens::HEADER_H), ui.px(tokens::SPACE_SM), ui.px(tokens::SPACE_XL));
        let (icon, row_h) = (ui.px(tokens::ICON), ui.px(tokens::ROW_H));
        let mut y = right.y;
        section_header(ui, Rect::new(right.x, y, right.w, hh), "Icons", Some(crate::icons::Icon::ALL.len()));
        y += hh + gap;
        let cell = ui.px(32);
        let per_row = (right.w / cell).max(1) as usize;
        for (i, ic) in Icon::ALL.iter().enumerate() {
            let (cx, cy) = ((i % per_row) as i32 * cell, (i / per_row) as i32 * cell);
            let r = Rect::new(right.x + cx + (cell - icon) / 2, y + cy + (cell - icon) / 2, icon, icon);
            ui.painter().icon(r, *ic, p.fg);
        }
        y += Icon::ALL.len().div_ceil(per_row) as i32 * cell + section;

        section_header(ui, Rect::new(right.x, y, right.w, hh), "Type scale", None);
        y += hh;
        for (role, sample) in [
            (TextRole::Display, "Display 22 semibold"),
            (TextRole::Heading, "Heading 18 semibold"),
            (TextRole::Title, "Title 15 semibold"),
            (TextRole::Strong, "Strong 13 semibold"),
            (TextRole::Body, "Body 13 regular — the quick brown fox"),
            (TextRole::Caption, "Caption 12 regular · Ctrl+Shift+P"),
        ] {
            let h = crate::widgets::paragraph_line_h(ui, role);
            let color = if role == TextRole::Caption { p.muted } else { p.fg };
            text(ui, Rect::new(right.x, y, right.w, h), sample, TextOpts { role: Some(role), color: Some(color), ..Default::default() });
            y += h;
        }
        y += section;

        section_header(ui, Rect::new(right.x, y, right.w, hh), "Path elision", None);
        y += hh;
        let long = "/home/demo/projects/skiff/crates/skiff-ui/src/widgets/menu.rs";
        let w = right.w.min(ui.px(260));
        for (mode, name) in [(Elide::End, "end"), (Elide::Middle, "middle"), (Elide::Start, "start")] {
            let lw = ui.px(56);
            text(
                ui,
                Rect::new(right.x, y, lw, row_h),
                name,
                TextOpts { role: Some(TextRole::Caption), color: Some(p.muted), ..Default::default() },
            );
            text(ui, Rect::new(right.x + lw, y, w, row_h), long, TextOpts { elide: mode, ..Default::default() });
            y += row_h;
        }
        y += section;
        // An empty state in a bordered panel.
        let panel = Rect::new(right.x, y, right.w, (right.bottom() - y).max(0));
        if panel.h > ui.px(120) {
            let radius = tokens::RADIUS_MD * ui.scale();
            ui.painter().rect(panel, p.bg, radius);
            let bw = ui.px(1).max(1);
            ui.painter().border(panel, p.border, bw, radius);
            let e = EmptyState {
                icon: Some(Icon::Forward),
                title: "No port forwards",
                body: "Forward a local port to a host.",
                action: Some("New forward"),
                secondary: None,
                hint: Some("Ctrl+Shift+P → Forward"),
            };
            empty_state(ui, Id::new("fwd-empty"), panel, &e);
        }
    }

    /// The connection form in `area`: host, user, password, a dropdown, a
    /// checkbox and two buttons. Connecting only updates the status text.
    fn form(&mut self, ui: &mut Ui, area: Rect) {
        let w = area.w.min(ui.px(420));
        let col = Rect::new(area.x, area.y, w, area.h);
        let c = tokens::CONTROL_H;
        // title, (label, input) x3, auth, checkbox, buttons, rest
        let sizes = [L::Px(32), L::Px(20), L::Px(c), L::Px(20), L::Px(c), L::Px(20), L::Px(c), L::Px(c), L::Px(28), L::Px(c), L::Fill];
        let rows = ui.split_v(col, &sizes, 6);
        let title = self.tabs.get(self.active).cloned().unwrap_or_default();
        text(ui, rows[0], &format!("{title} — new connection"), TextOpts { role: Some(TextRole::Heading), ..Default::default() });
        let field = |ui: &mut Ui, r: usize, name: &str, v: &mut String, password: bool| {
            text(
                ui,
                rows[r],
                name,
                TextOpts { role: Some(TextRole::Caption), color: Some(ui.style().palette.muted), ..Default::default() },
            );
            text_input(ui, Id::new(name), rows[r + 1], v, TextInputOpts { password, placeholder: name, ..Default::default() })
        };
        field(ui, 1, "Host", &mut self.host, false);
        field(ui, 3, "User", &mut self.user, false);
        let pw = field(ui, 5, "Password", &mut self.password, true);
        let auth = Rect::new(rows[7].x, rows[7].y, ui.px(200), rows[7].h);
        dropdown_labeled(ui, Id::new("auth"), auth, "Authentication", &["Agent", "Key file", "Password"], &mut self.auth);
        checkbox(ui, Id::new("remember"), rows[8], &mut self.remember, "Remember this host");
        let b = ui.split_h(rows[9], &[L::Px(120), L::Px(120), L::Fill], 8);
        if button_with(ui, Id::new("connect"), b[0], "Connect", ButtonOpts::kind(ButtonKind::Primary)).clicked || pw.submitted {
            self.status = format!("would connect to {}@{} (demo: no networking)", self.user, self.host);
        }
        if button(ui, Id::new("clear"), b[1], "Clear").clicked {
            self.host.clear();
            self.user.clear();
            self.password.clear();
        }
    }
}

/// `n` bytes as a short human size ("4.7 KB").
fn human(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", units[u]) }
}
