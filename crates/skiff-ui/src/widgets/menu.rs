//! Context menus. Open one with `open_context_menu` (e.g. on right-click)
//! and call `context_menu` every frame; it closes itself on a choice,
//! Escape, or a click outside. Items can carry an icon, a right-aligned
//! shortcut hint and a danger style; `MenuItem::SEPARATOR` draws a divider.

use crate::a11y::Role;
use crate::geom::{Point, Rect};
use crate::icons::Icon;
use crate::input::{Event, Key, Modifiers};
use crate::style::{TextRole, tokens};
use crate::ui::{Id, Sense, Ui};

/// One entry in a context menu. Build with `MenuItem::new` and the
/// chainable setters, or a struct literal ending in `..MenuItem::DEFAULT`.
#[derive(Debug, Clone, Copy)]
pub struct MenuItem<'a> {
    pub label: &'a str,
    /// Shortcut text drawn right-aligned (display only; it is not bound here).
    pub shortcut: Option<&'a str>,
    /// Disabled items are drawn muted and cannot be hovered or chosen.
    pub enabled: bool,
    /// Icon drawn before the label.
    pub icon: Option<Icon>,
    /// Destructive action: drawn in the danger colour.
    pub danger: bool,
    /// A divider line instead of an item (never chosen; skipped by arrows).
    pub separator: bool,
}

impl<'a> MenuItem<'a> {
    /// An enabled item with no label, shortcut or icon (for `..` in literals).
    pub const DEFAULT: MenuItem<'static> =
        MenuItem { label: "", shortcut: None, enabled: true, icon: None, danger: false, separator: false };

    /// A divider between groups of items.
    pub const SEPARATOR: MenuItem<'static> = MenuItem { separator: true, enabled: false, ..MenuItem::DEFAULT };

    /// An enabled item labelled `label`.
    pub const fn new(label: &'a str) -> Self {
        MenuItem { label, shortcut: None, enabled: true, icon: None, danger: false, separator: false }
    }

    /// With a right-aligned shortcut hint.
    pub const fn shortcut(mut self, s: &'a str) -> Self {
        self.shortcut = Some(s);
        self
    }

    /// Enabled or not.
    pub const fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// With an icon before the label.
    pub const fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }

    /// Styled as a destructive action.
    pub const fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Can be highlighted and chosen.
    fn choosable(&self) -> bool {
        self.enabled && !self.separator
    }
}

/// A menu's state, kept in widget memory under the menu's id.
#[derive(Debug, Clone, Copy, Default)]
struct MenuState {
    open: bool,
    /// The requested top-left corner (the menu flips to stay on screen).
    at: Point,
    /// The highlighted item (mouse hover or arrow keys).
    hover: Option<usize>,
    /// Opened this frame: the click that opened it must not close it.
    fresh: bool,
    /// Where it was last drawn (kept after it closes, for submenus).
    rect: Rect,
}

/// Opens menu `id` at `at` and routes the keyboard to it.
pub fn open_context_menu(ui: &mut Ui, id: Id, at: Point) {
    *ui.memory::<MenuState>(id) = MenuState { open: true, at, hover: None, fresh: true, rect: Rect::default() };
    ui.capture_keyboard(id);
}

/// Closes menu `id` without a choice.
pub fn close_context_menu(ui: &mut Ui, id: Id) {
    ui.memory::<MenuState>(id).open = false;
}

/// Whether menu `id` is currently open.
pub fn menu_is_open(ui: &mut Ui, id: Id) -> bool {
    ui.memory::<MenuState>(id).open
}

/// Where item `i` of menu `id` (drawn with `items`) was last drawn, even
/// if the menu has closed since: anchor a submenu chosen from it there,
/// whether it was picked by mouse or keyboard. `None` if never drawn.
pub fn menu_item_rect(ui: &mut Ui, id: Id, items: &[MenuItem], i: usize) -> Option<Rect> {
    let menu = ui.memory::<MenuState>(id).rect;
    if menu.w <= 0 || i >= items.len() {
        return None;
    }
    let (row_h, sep_h, edge) = item_metrics(ui);
    let top: i32 = items[..i].iter().map(|it| if it.separator { sep_h } else { row_h }).sum();
    let h = if items[i].separator { sep_h } else { row_h };
    Some(Rect::new(menu.x + edge, menu.y + edge + top, menu.w - 2 * edge, h))
}

/// Row height, separator height and inner edge of a menu, in pixels.
fn item_metrics(ui: &Ui) -> (i32, i32, i32) {
    (ui.px(tokens::ROW_H), ui.px(tokens::SPACE_SM + 1), ui.px(tokens::SPACE_XS))
}

/// The next choosable item after `from` (or before it, going backwards),
/// wrapping around; `None` when no item can be chosen.
fn step(items: &[MenuItem], from: Option<usize>, forward: bool) -> Option<usize> {
    let n = items.len();
    if n == 0 {
        return None;
    }
    let mut i = from.unwrap_or(if forward { n - 1 } else { 0 });
    for _ in 0..n {
        i = if forward { (i + 1) % n } else { (i + n - 1) % n };
        if items[i].choosable() {
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
    let (font, px) = ui.text_style(TextRole::Body);
    let (sfont, spx) = ui.text_style(TextRole::Caption);
    let (row_h, sep_h, edge) = item_metrics(ui);
    let pad = ui.px(tokens::SPACE_SM);
    let icon_px = ui.px(tokens::ICON);
    let has_icons = items.iter().any(|it| it.icon.is_some());
    let text_x = pad + if has_icons { icon_px + pad } else { 0 };
    // Wide enough for the widest label plus its shortcut, at least 180px.
    let mut w = ui.px(180);
    for it in items {
        let lw = ui.measure(it.label, font, px).ceil() as i32;
        let sw = it.shortcut.map_or(0, |s| ui.measure(s, sfont, spx).ceil() as i32 + ui.px(tokens::SPACE_XL));
        w = w.max(lw + sw + text_x + pad + 2 * edge);
    }
    let heights: Vec<i32> = items.iter().map(|it| if it.separator { sep_h } else { row_h }).collect();
    let h = heights.iter().sum::<i32>() + 2 * edge;
    let win = ui.window_rect();
    // Flip left/up when the menu would run off the right/bottom of the window.
    let x = if st.at.x + w > win.right() { (st.at.x - w).max(0) } else { st.at.x };
    let y = if st.at.y + h > win.bottom() { (st.at.y - h).max(0) } else { st.at.y };
    let menu = Rect::new(x, y, w, h);
    st.rect = menu;

    if st.hover.is_some_and(|h| h >= items.len() || !items[h].choosable()) {
        st.hover = None;
    }
    let mut chosen = None;
    // Keyboard: Escape closes, Up/Down move the highlight, Enter picks it.
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
        chosen = st.hover.filter(|&i| items.get(i).is_some_and(|it| it.choosable()));
    }
    let mouse_moved = ui.input().events.iter().any(|e| matches!(e, Event::MouseMove(_)));
    // A click outside closes it, except the click that just opened it.
    if !st.fresh && ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(_, p) if !menu.contains(*p))) {
        close = true;
    }

    let p = ui.style().palette;
    let radius = tokens::RADIUS_LG * ui.scale();
    let item_radius = tokens::RADIUS_SM * ui.scale();
    let lh = ui.line_height(font, px);
    let slh = ui.line_height(sfont, spx);
    ui.overlay(|ui| {
        ui.a11y_group(id.with("a11y"), Role::Menu, "", |ui| {
            // The whole menu claims hover so widgets beneath it are not hot.
            ui.interact(id, menu, Sense::hover());
            let focus = st.hover.map_or(id.with("a11y"), |h| id.with(h));
            ui.a11y_focus(focus);
            let bw = ui.px(1).max(1);
            ui.elevate(menu, radius);
            {
                let mut painter = ui.painter();
                painter.rect(menu, p.overlay, radius);
                painter.border(menu, p.border, bw, radius);
            }
            let mut ry = menu.y + edge;
            for (i, it) in items.iter().enumerate() {
                let row = Rect::new(menu.x + edge, ry, menu.w - 2 * edge, heights[i]);
                ry += heights[i];
                if it.separator {
                    let line = Rect::new(row.x + pad / 2, row.y + row.h / 2, row.w - pad, bw);
                    ui.painter().rect(line, p.border, 0.0);
                    continue;
                }
                let sense = if it.enabled { Sense::click() } else { Sense::hover() };
                let rr = ui.interact(id.with(i), row, sense);
                // Hover follows the mouse only when it moves, so the keyboard highlight
                // is not stolen by a pointer resting over the menu.
                if it.enabled && rr.hovered && mouse_moved {
                    st.hover = Some(i);
                }
                if it.enabled && rr.clicked {
                    chosen = Some(i);
                }
                if let Some(n) = ui.a11y_node(id.with(i), Role::MenuItem, it.label) {
                    n.disabled = !it.enabled;
                }
                let lit = st.hover == Some(i);
                // A highlighted danger item fills with the danger colour.
                let (bg, fg) = match (lit, it.enabled, it.danger) {
                    (_, false, _) => (None, p.text_disabled),
                    (true, true, true) => (Some(p.danger), p.text_on(p.danger)),
                    (true, true, false) => (Some(p.selection), p.fg),
                    (false, true, true) => (None, p.danger),
                    (false, true, false) => (None, p.fg),
                };
                let sc = if lit && it.danger {
                    fg
                } else if it.enabled {
                    p.muted
                } else {
                    p.text_disabled
                };
                let sw = it.shortcut.map(|s| (s, ui.measure(s, sfont, spx).ceil() as i32));
                let mut painter = ui.painter();
                if let Some(bg) = bg {
                    painter.rect(row, bg, item_radius);
                }
                if let Some(icon) = it.icon {
                    painter.icon(Rect::new(row.x + pad, row.y + (row.h - icon_px) / 2, icon_px, icon_px), icon, fg);
                }
                painter.text(Point::new(row.x + text_x, row.y + (row.h - lh) / 2), it.label, font, px, fg);
                if let Some((s, w)) = sw {
                    painter.text(Point::new(row.right() - pad - w, row.y + (row.h - slh) / 2), s, sfont, spx, sc);
                }
            }
        })
    });

    // Keep the keyboard while open; closing releases it.
    st.fresh = false;
    if chosen.is_some() || close {
        st.open = false;
    } else {
        ui.capture_keyboard(id);
    }
    *ui.memory::<MenuState>(id) = st;
    chosen
}
