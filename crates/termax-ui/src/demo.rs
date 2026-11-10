//! Demo UI shared by `examples/demo.rs` and the render-time test. It
//! exercises every widget: a filterable 100k-row list in a resizable
//! sidebar, tabs, a form, and a context menu.

use std::time::Instant;

use crate::geom::Rect;
use crate::layout::Size as L;
use crate::text::FontKind;
use crate::ui::{Id, Ui};
use crate::widgets::{
    MenuItem, TabTitle, TextInputOpts, button, checkbox, context_menu, label, label_with, list, open_context_menu,
    splitter_h, tabs, text_input,
};

pub struct Demo {
    items: Vec<String>,
    filter: String,
    filtered: Vec<usize>,
    selected: Option<usize>,
    tabs: Vec<String>,
    active: usize,
    next_tab: usize,
    sidebar: f32,
    host: String,
    user: String,
    password: String,
    remember: bool,
    status: String,
    menu_row: Option<usize>,
    build_ms: f64,
}

impl Default for Demo {
    fn default() -> Self {
        let items: Vec<String> = (0..100_000).map(|i| format!("host-{i:05}.example.test")).collect();
        let filtered = (0..items.len()).collect();
        Demo {
            items,
            filter: String::new(),
            filtered,
            selected: None,
            tabs: vec!["Welcome".into(), "Files".into(), "Settings".into()],
            active: 0,
            next_tab: 1,
            sidebar: 260.0,
            host: String::new(),
            user: String::new(),
            password: String::new(),
            remember: true,
            status: "ready".into(),
            menu_row: None,
            build_ms: 0.0,
        }
    }
}

const MENU: [MenuItem<'static>; 4] = [
    MenuItem { label: "Connect", shortcut: Some("Enter"), enabled: true },
    MenuItem { label: "Open files", shortcut: Some("Ctrl+Shift+F"), enabled: true },
    MenuItem { label: "Edit…", shortcut: None, enabled: false },
    MenuItem { label: "Copy name", shortcut: None, enabled: true },
];

impl Demo {
    pub fn ui(&mut self, ui: &mut Ui) {
        let t0 = Instant::now();
        let p = ui.style().palette;
        let win = ui.window_rect();
        let rows = ui.split_v(win, &[L::Fill, L::Px(26)], 0);
        let (side, main) = splitter_h(ui, Id::new("split"), rows[0], &mut self.sidebar, 160.0, 600.0);

        // Sidebar: filter + list.
        ui.painter().rect(side, p.surface, 0.0);
        let inner = side.inset(ui.px(8));
        let parts = ui.split_v(inner, &[L::Px(30), L::Fill], 8);
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
        let (items, filtered) = (&self.items, &self.filtered);
        let lr = list(ui, Id::new("hosts"), parts[1], filtered.len(), 24, &mut self.selected, |ui, row, i, _| {
            let pad = ui.px(8);
            let r = Rect::new(row.x + pad, row.y, row.w - 2 * pad, row.h);
            label(ui, r, &items[filtered[i]], None);
        });
        if let Some(i) = lr.activated {
            self.status = format!("activated {}", self.items[self.filtered[i]]);
        }
        if let Some((i, at)) = lr.context {
            self.menu_row = Some(i);
            open_context_menu(ui, Id::new("menu"), at);
        }

        // Main: tabs + content.
        let mparts = ui.split_v(main, &[L::Px(34), L::Fill], 0);
        let titles: Vec<TabTitle> = self.tabs.iter().map(|t| TabTitle { text: t, closable: true }).collect();
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
        self.form(ui, body.inset(ui.px(24)));

        // Status bar.
        let sb = rows[1];
        ui.painter().rect(sb, p.bg, 0.0);
        let text = format!(
            "{} · {} of {} hosts · frame build {:.2} ms",
            self.status,
            self.filtered.len(),
            self.items.len(),
            self.build_ms
        );
        let pad = ui.px(8);
        label_with(ui, Rect::new(sb.x + pad, sb.y, sb.w - 2 * pad, sb.h), &text, FontKind::Ui, p.muted);

        if let Some(i) = context_menu(ui, Id::new("menu"), &MENU) {
            let name = self.menu_row.and_then(|r| self.filtered.get(r)).map(|&k| self.items[k].clone());
            self.status = format!("{} → {}", MENU[i].label, name.unwrap_or_default());
            if i == 3 {
                if let Some(n) = self.menu_row.and_then(|r| self.filtered.get(r)).map(|&k| self.items[k].clone()) {
                    ui.clipboard().set(&n);
                }
            }
        }
        self.build_ms = t0.elapsed().as_secs_f64() * 1000.0;
    }

    fn form(&mut self, ui: &mut Ui, area: Rect) {
        let w = area.w.min(ui.px(420));
        let col = Rect::new(area.x, area.y, w, area.h);
        // title, (label, input) x3, checkbox, buttons, rest
        let sizes = [
            L::Px(24),
            L::Px(22),
            L::Px(30),
            L::Px(22),
            L::Px(30),
            L::Px(22),
            L::Px(30),
            L::Px(28),
            L::Px(34),
            L::Fill,
        ];
        let rows = ui.split_v(col, &sizes, 6);
        let title = self.tabs.get(self.active).cloned().unwrap_or_default();
        let accent = ui.style().palette.accent;
        label_with(ui, rows[0], &format!("{title} — new connection"), FontKind::Ui, accent);
        let field = |ui: &mut Ui, r: usize, name: &str, v: &mut String, password: bool| {
            label(ui, rows[r], name, None);
            text_input(ui, Id::new(name), rows[r + 1], v, TextInputOpts { password, placeholder: name, ..Default::default() })
        };
        field(ui, 1, "Host", &mut self.host, false);
        field(ui, 3, "User", &mut self.user, false);
        let pw = field(ui, 5, "Password", &mut self.password, true);
        checkbox(ui, Id::new("remember"), rows[7], &mut self.remember, "Remember this host");
        let b = ui.split_h(rows[8], &[L::Px(120), L::Px(120), L::Fill], 8);
        if button(ui, Id::new("connect"), b[0], "Connect").clicked || pw.submitted {
            self.status = format!("would connect to {}@{} (demo: no networking)", self.user, self.host);
        }
        if button(ui, Id::new("clear"), b[1], "Clear").clicked {
            self.host.clear();
            self.user.clear();
            self.password.clear();
        }
    }
}
