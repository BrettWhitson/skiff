//! Context menus. Open one with `open_context_menu` (e.g. on right-click)
//! and call `context_menu` every frame; it closes itself on a choice,
//! Escape, or a click outside.

use crate::a11y::Role;
use crate::geom::{Point, Rect};
use crate::input::{Event, Key, Modifiers};
use crate::text::FontKind;
use crate::ui::{Id, Sense, Ui};

#[derive(Debug, Clone, Copy)]
pub struct MenuItem<'a> {
    pub label: &'a str,
    pub shortcut: Option<&'a str>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct MenuState {
    open: bool,
    at: Point,
    hover: Option<usize>,
    /// Opened this frame: the click that opened it must not close it.
    fresh: bool,
}

pub fn open_context_menu(ui: &mut Ui, id: Id, at: Point) {
    *ui.memory::<MenuState>(id) = MenuState { open: true, at, hover: None, fresh: true };
    ui.capture_keyboard(id);
}

pub fn close_context_menu(ui: &mut Ui, id: Id) {
    ui.memory::<MenuState>(id).open = false;
}

pub fn menu_is_open(ui: &mut Ui, id: Id) -> bool {
    ui.memory::<MenuState>(id).open
}

fn step(items: &[MenuItem], from: Option<usize>, forward: bool) -> Option<usize> {
    let n = items.len();
    if n == 0 {
        return None;
    }
    let mut i = from.unwrap_or(if forward { n - 1 } else { 0 });
    for _ in 0..n {
        i = if forward { (i + 1) % n } else { (i + n - 1) % n };
        if items[i].enabled {
            return Some(i);
        }
    }
    None
}

/// Draws menu `id` if open; returns the index of a chosen item.
pub fn context_menu(ui: &mut Ui, id: Id, items: &[MenuItem]) -> Option<usize> {
    let mut st = *ui.memory::<MenuState>(id);
    if !st.open {
        return None;
    }
    let px = ui.font_px();
    let row_h = ui.px(26);
    let pad = ui.px(10);
    let mut w = ui.px(160);
    for it in items {
        let lw = ui.measure(it.label, FontKind::Ui, px).ceil() as i32;
        let sw = it.shortcut.map_or(0, |s| ui.measure(s, FontKind::Ui, px).ceil() as i32 + pad);
        w = w.max(lw + sw + 2 * pad);
    }
    let h = row_h * items.len() as i32;
    let win = ui.window_rect();
    let x = if st.at.x + w > win.right() { (st.at.x - w).max(0) } else { st.at.x };
    let y = if st.at.y + h > win.bottom() { (st.at.y - h).max(0) } else { st.at.y };
    let menu = Rect::new(x, y, w, h);

    if st.hover.is_some_and(|h| h >= items.len()) {
        st.hover = None;
    }
    let mut chosen = None;
    let mut close = false;
    if ui.input_mut().take_key(&Key::Escape, Modifiers::NONE) {
        close = true;
    }
    if ui.input_mut().take_key(&Key::Down, Modifiers::NONE) {
        st.hover = step(items, st.hover, true);
    }
    if ui.input_mut().take_key(&Key::Up, Modifiers::NONE) {
        st.hover = step(items, st.hover, false);
    }
    if ui.input_mut().take_key(&Key::Enter, Modifiers::NONE) {
        chosen = st.hover.filter(|&i| items.get(i).is_some_and(|it| it.enabled));
    }
    let mouse_moved = ui.input().events.iter().any(|e| matches!(e, Event::MouseMove(_)));
    if !st.fresh && ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(_, p) if !menu.contains(*p))) {
        close = true;
    }

    let p = ui.style().palette;
    let radius = ui.px(6) as f32;
    let lh = ui.line_height(FontKind::Ui, px);
    ui.overlay(|ui| {
        ui.a11y_group(id.with("a11y"), Role::Menu, "", |ui| {
            ui.interact(id, menu, Sense::hover());
            let focus = st.hover.map_or(id.with("a11y"), |h| id.with(h));
            ui.a11y_focus(focus);
            let bw = ui.px(1).max(1);
            {
                let mut painter = ui.painter();
                let shadow = ui_shadow(menu);
                painter.rect(shadow, p.bg.with_alpha(120), radius);
                painter.rect(menu, p.surface_alt, radius);
                painter.border(menu, p.border, bw, radius);
            }
            for (i, it) in items.iter().enumerate() {
                let row = Rect::new(menu.x, menu.y + i as i32 * row_h, menu.w, row_h);
                let sense = if it.enabled { Sense::click() } else { Sense::hover() };
                let rr = ui.interact(id.with(i), row, sense);
                if it.enabled && rr.hovered && mouse_moved {
                    st.hover = Some(i);
                }
                if it.enabled && rr.clicked {
                    chosen = Some(i);
                }
                if let Some(n) = ui.a11y_node(id.with(i), Role::MenuItem, it.label) {
                    n.disabled = !it.enabled;
                }
                let ty = row.y + (row_h - lh) / 2;
                let fg = if it.enabled { p.fg } else { p.muted.with_alpha(160) };
                let sw = it.shortcut.map(|s| (s, ui.measure(s, FontKind::Ui, px).ceil() as i32));
                let mut painter = ui.painter();
                if st.hover == Some(i) {
                    painter.rect(row.inset(ui_inset(row)), p.selection, radius / 2.0);
                }
                painter.text(Point::new(row.x + pad, ty), it.label, FontKind::Ui, px, fg);
                if let Some((s, w)) = sw {
                    painter.text(Point::new(row.right() - pad - w, ty), s, FontKind::Ui, px, p.muted);
                }
            }
        })
    });

    st.fresh = false;
    if chosen.is_some() || close {
        st.open = false;
    } else {
        ui.capture_keyboard(id);
    }
    *ui.memory::<MenuState>(id) = st;
    chosen
}

fn ui_shadow(r: Rect) -> Rect {
    let d = (r.h / 40).clamp(2, 6);
    Rect::new(r.x + d, r.y + d, r.w, r.h)
}

fn ui_inset(r: Rect) -> i32 {
    (r.h / 12).max(1)
}
