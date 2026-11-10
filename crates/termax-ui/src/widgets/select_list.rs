//! Virtualized list with multi-selection (click, Ctrl-click, Shift-click,
//! Shift+arrows, Ctrl+A) and drag detection for drag-and-drop.

use std::collections::BTreeSet;

use crate::a11y::Role;
use crate::geom::{Point, Rect};
use crate::input::{Event, Key, MouseButton};
use crate::ui::{Id, Response, Sense, Ui};

use super::scroll::{scroll_area, scroll_into_view};

/// Logical pixels the pointer must travel with a row pressed to start a drag.
const DRAG_THRESHOLD: i32 = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub set: BTreeSet<usize>,
    /// Where Shift-extension starts.
    pub anchor: Option<usize>,
    /// The keyboard row (moved by arrows, activated by Enter).
    pub cursor: Option<usize>,
}

impl Selection {
    pub fn clear(&mut self) {
        *self = Selection::default();
    }

    pub fn select_only(&mut self, i: usize) {
        self.set = BTreeSet::from([i]);
        self.anchor = Some(i);
        self.cursor = Some(i);
    }

    pub fn toggle(&mut self, i: usize) {
        if !self.set.remove(&i) {
            self.set.insert(i);
        }
        self.anchor = Some(i);
        self.cursor = Some(i);
    }

    /// Selects the range from the anchor to `i` (the anchor stays).
    pub fn extend_to(&mut self, i: usize) {
        let a = *self.anchor.get_or_insert(i);
        self.set = (a.min(i)..=a.max(i)).collect();
        self.cursor = Some(i);
    }

    pub fn contains(&self, i: usize) -> bool {
        self.set.contains(&i)
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.set.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Drops indices at or past `rows`.
    pub fn clamp(&mut self, rows: usize) {
        self.set.retain(|&i| i < rows);
        self.anchor = self.anchor.filter(|&a| a < rows);
        self.cursor = self.cursor.filter(|&c| c < rows);
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SelectListResponse {
    pub response: Response,
    /// Enter (on the cursor row) or double-click.
    pub activated: Option<usize>,
    /// Right-click on this row, at this point.
    pub context: Option<(usize, Point)>,
    /// The pressed selection was dragged past the threshold (fires once);
    /// the caller should `ui.start_drag(..)`.
    pub drag_started: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct PressState {
    /// Row and point of the current left press.
    at: Option<(usize, Point)>,
    /// Plain press on an already-selected row: collapse on release, unless dragged.
    collapse: bool,
    dragging: bool,
}

/// A selectable list of `rows` rows, each `row_h` logical pixels tall.
/// `f(ui, row_rect, index, selected)` draws one visible row's contents.
#[allow(clippy::too_many_arguments)]
pub fn select_list(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    rows: usize,
    row_h: i32,
    sel: &mut Selection,
    f: impl FnMut(&mut Ui, Rect, usize, bool),
) -> SelectListResponse {
    select_list_named(ui, id, rect, "", rows, row_h, sel, |_| String::new(), f)
}

/// `select_list` with names for screen readers. A screen-reader Click
/// selects a row; a Click on the only selected row activates it.
#[allow(clippy::too_many_arguments)]
pub fn select_list_named(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    name: &str,
    rows: usize,
    row_h: i32,
    sel: &mut Selection,
    row_name: impl Fn(usize) -> String,
    mut f: impl FnMut(&mut Ui, Rect, usize, bool),
) -> SelectListResponse {
    let r = ui.interact(id, rect, Sense::focusable());
    let rh = ui.px(row_h).max(1);
    let sid = id.with("scroll");
    let mut out = SelectListResponse { response: r, ..Default::default() };
    sel.clamp(rows);

    if r.focused && rows > 0 && ui.keys_for(id) {
        let page = (rect.h / rh).max(1) as usize;
        let events = ui.input().events.clone();
        let mut moved = false;
        for e in &events {
            let Event::KeyDown { key, mods, .. } = e else { continue };
            let primary = mods.ctrl || mods.logo;
            if primary && !mods.shift && !mods.alt && *key == Key::Char('a') {
                sel.set = (0..rows).collect();
                continue;
            }
            if primary || mods.alt {
                continue;
            }
            let cur = sel.cursor;
            let next = match key {
                Key::Up => Some(cur.map_or(0, |c| c.saturating_sub(1))),
                Key::Down => Some(cur.map_or(0, |c| (c + 1).min(rows - 1))),
                Key::PageUp => Some(cur.map_or(0, |c| c.saturating_sub(page))),
                Key::PageDown => Some(cur.map_or(0, |c| (c + page).min(rows - 1))),
                Key::Home => Some(0),
                Key::End => Some(rows - 1),
                Key::Enter => {
                    out.activated = sel.cursor;
                    None
                }
                _ => None,
            };
            if let Some(n) = next {
                if mods.shift {
                    sel.extend_to(n)
                } else {
                    sel.select_only(n)
                }
                moved = true;
            }
        }
        if moved && let Some(c) = sel.cursor {
            let top = c as i32 * rh;
            scroll_into_view(ui, sid, top, top + rh, rect.h);
        }
    }

    let mut press = *ui.memory::<PressState>(id);
    let down_now = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(MouseButton::Left, _)));
    let mods = ui.input().mods;
    let mouse = ui.input().mouse;
    let left_down = ui.input().left_down;
    let content_h = (rows as i64 * rh as i64).min(i32::MAX as i64) as i32;
    ui.a11y_group(id, Role::List, name, |ui| {
        scroll_area(ui, sid, rect, content_h, |ui, vis, off| {
            let first = (off / rh) as usize;
            let last = (((off + vis.h + rh - 1) / rh) as usize).min(rows);
            let p = ui.style().palette;
            for i in first..last {
                let row = Rect::new(vis.x, vis.y + i as i32 * rh - off, vis.w, rh);
                let rid = id.with(i);
                let rr = ui.interact(rid, row, Sense::click());
                if ui.a11y_clicked(rid) {
                    if sel.contains(i) && sel.len() == 1 {
                        out.activated = Some(i);
                    } else {
                        sel.select_only(i);
                        ui.request_focus(id);
                    }
                }
                if ui.a11y_on() {
                    let nm = row_name(i);
                    let on = sel.contains(i);
                    if let Some(n) = ui.a11y_node(rid, Role::ListItem, &nm) {
                        n.selected = Some(on);
                    }
                    if sel.cursor == Some(i) {
                        ui.a11y_focus_child(id, rid);
                    }
                }
                // A press arrives as `pressed`, or as `clicked` when the button
                // went down and up within one frame.
                if down_now && (rr.pressed || rr.clicked) {
                    ui.request_focus(id);
                    press = PressState { at: Some((i, mouse)), collapse: false, dragging: false };
                    if mods.ctrl || mods.logo {
                        sel.toggle(i);
                    } else if mods.shift {
                        sel.extend_to(i);
                    } else if sel.contains(i) {
                        press.collapse = true;
                    } else {
                        sel.select_only(i);
                    }
                }
                if rr.clicked && press.collapse && !press.dragging {
                    sel.select_only(i);
                }
                if rr.double_clicked {
                    out.activated = Some(i);
                }
                if rr.right_clicked {
                    if !sel.contains(i) {
                        sel.select_only(i);
                    }
                    out.context = Some((i, ui.input().mouse));
                }
                let is_sel = sel.contains(i);
                if is_sel {
                    let c = if ui.has_focus(id) { p.selection } else { p.selection.mix(p.surface, 0.5) };
                    ui.painter().rect(row, c, 0.0);
                } else if rr.hovered {
                    ui.painter().rect(row, p.surface_alt, 0.0);
                }
                f(ui, row, i, is_sel);
                if sel.cursor == Some(i) && ui.has_focus(id) {
                    super::list::focus_bar(ui, row);
                }
            }
        })
    });

    if let Some((i, at)) = press.at {
        if !left_down {
            press = PressState::default();
        } else if !press.dragging && sel.contains(i) {
            let t = ui.px(DRAG_THRESHOLD);
            if (mouse.x - at.x).abs() >= t || (mouse.y - at.y).abs() >= t {
                press.dragging = true;
                press.collapse = false;
                out.drag_started = true;
            }
        }
    }
    *ui.memory::<PressState>(id) = press;
    out
}
