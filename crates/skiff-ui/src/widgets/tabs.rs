//! Tab strip with close buttons, drag-to-reorder and a "+" button.

use crate::a11y::Role;
use crate::geom::{Point, Rect};
use crate::input::{CursorIcon, Event, Key, Modifiers, MouseButton};
use crate::text::FontKind;
use crate::ui::{Id, Sense, Ui};

use super::label::elide;

#[derive(Debug, Clone, Copy)]
pub struct TabTitle<'a> {
    pub text: &'a str,
    pub closable: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabsResponse {
    /// The close button of this tab was clicked.
    pub closed: Option<usize>,
    /// A tab was dragged from `.0` to `.1`; the caller reorders its tabs.
    pub moved: Option<(usize, usize)>,
    pub plus_clicked: bool,
    /// The dragged tab left the strip (reported once per drag): the caller
    /// may start a drag-and-drop with it. The tab that was active before
    /// the press is active again, so the content below is a drop target.
    pub dragged_out: Option<usize>,
    /// Where each tab was drawn.
    pub tab_rects: Vec<Rect>,
}

#[derive(Debug, Clone, Copy, Default)]
struct TabDrag {
    dragging: Option<usize>,
    /// The tab active before the press.
    before: Option<usize>,
    /// The drag has left the strip once.
    out: bool,
}

/// Index `b` after the item at `from` moved to `to`.
fn index_after_move(b: usize, from: usize, to: usize) -> usize {
    if b == from {
        to
    } else if from < b && to >= b {
        b - 1
    } else if from > b && to <= b {
        b + 1
    } else {
        b
    }
}

fn index_at(rects: &[Rect], x: i32) -> usize {
    rects.iter().position(|r| x < r.right()).unwrap_or(rects.len().saturating_sub(1))
}

pub fn tabs(ui: &mut Ui, id: Id, rect: Rect, titles: &[TabTitle], active: &mut usize) -> TabsResponse {
    let mut out = TabsResponse::default();
    let n = titles.len();
    let px = ui.font_px();
    let pad = ui.px(12);
    let close_side = ui.px(16);
    let gap = ui.px(2);
    let plus_w = rect.h;
    let avail = (rect.w - plus_w - ui.px(4)).max(0);

    // Widths: natural size within [min, max], shrunk evenly when crowded.
    let (min_w, max_w) = (ui.px(72), ui.px(220));
    let mut widths: Vec<i32> = titles
        .iter()
        .map(|t| {
            let tw = ui.measure(t.text, FontKind::Ui, px).ceil() as i32;
            (tw + 2 * pad + if t.closable { close_side + pad / 2 } else { 0 }).clamp(min_w, max_w)
        })
        .collect();
    let total: i32 = widths.iter().sum::<i32>() + gap * n.saturating_sub(1) as i32;
    if total > avail && n > 0 {
        let each = ((avail - gap * (n as i32 - 1)) / n as i32).max(0);
        widths.iter_mut().for_each(|w| *w = each);
    }
    let mut x = rect.x;
    for w in &widths {
        out.tab_rects.push(Rect::new(x, rect.y, *w, rect.h));
        x += w + gap;
    }
    let strip_right = out.tab_rects.last().map_or(rect.x, |r| r.right());
    let strip = Rect::new(rect.x, rect.y, strip_right - rect.x, rect.h);

    // The whole strip is one widget, so a drag survives the reorder.
    let sr = ui.interact(id, strip, Sense { click: true, drag: true, focus: true });
    if ui.a11y_on() {
        ui.a11y_group(id, Role::TabList, "Tabs", |ui| {
            for (i, t) in titles.iter().enumerate() {
                let tid = id.with(("tab", i));
                if ui.a11y_clicked(tid) {
                    *active = i;
                }
                if let Some(nd) = ui.a11y_node(tid, Role::Tab, t.text) {
                    nd.bounds = out.tab_rects[i];
                    nd.selected = Some(i == *active);
                }
            }
            ui.a11y_focus_child(id, id.with(("tab", *active)));
        });
    }
    if sr.focused && n > 0 && ui.keys_for(id) {
        if ui.input_mut().take_key(&Key::Right, Modifiers::NONE) {
            *active = (*active + 1).min(n - 1);
        }
        if ui.input_mut().take_key(&Key::Left, Modifiers::NONE) {
            *active = active.saturating_sub(1);
        }
    }
    let mouse = ui.input().mouse;
    let pressed_now = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(MouseButton::Left, _)));
    let mut drag = *ui.memory::<TabDrag>(id);
    if n > 0 {
        if sr.pressed {
            match drag.dragging {
                Some(i) if !pressed_now && (mouse.y < rect.y || mouse.y >= rect.bottom()) => {
                    if !drag.out {
                        drag.out = true;
                        out.dragged_out = Some(i);
                        if let Some(b) = drag.before {
                            *active = b;
                        }
                    }
                }
                Some(i) if !pressed_now => {
                    let target = index_at(&out.tab_rects, mouse.x);
                    if target != i {
                        out.moved = Some((i, target));
                        if *active == i {
                            *active = target;
                        }
                        drag.before = drag.before.map(|b| index_after_move(b, i, target));
                        drag.dragging = Some(target);
                    }
                }
                _ => {
                    let i = index_at(&out.tab_rects, mouse.x);
                    drag = TabDrag { dragging: Some(i), before: Some(*active), out: false };
                    *active = i;
                }
            }
        } else {
            if sr.clicked {
                *active = index_at(&out.tab_rects, mouse.x);
            }
            drag = TabDrag::default();
        }
    }
    *ui.memory::<TabDrag>(id) = drag;
    if n > 0 {
        *active = (*active).min(n - 1);
    }

    let p = ui.style().palette;
    let radius = ui.px(4) as f32;
    let accent_h = ui.px(2).max(1);
    for (i, t) in titles.iter().enumerate() {
        let tr = out.tab_rects[i];
        let hovered = sr.hovered && tr.contains(mouse);
        let is_active = i == *active;
        let bg = if is_active {
            p.surface
        } else if hovered {
            p.surface_alt
        } else {
            p.bg
        };
        let close = Rect::new(tr.right() - pad / 2 - close_side, tr.y + (tr.h - close_side) / 2, close_side, close_side);
        let text_right = if t.closable { close.x - pad / 4 } else { tr.right() - pad };
        let shown = elide(ui, t.text, FontKind::Ui, px, text_right - tr.x - pad);
        let lh = ui.line_height(FontKind::Ui, px);
        {
            let mut painter = ui.painter();
            painter.rect(tr, bg, radius);
            if is_active {
                painter.rect(Rect::new(tr.x, tr.y, tr.w, accent_h), p.accent, 0.0);
            }
            let fg = if is_active { p.fg } else { p.muted };
            painter.text(Point::new(tr.x + pad, tr.y + (tr.h - lh) / 2), &shown, FontKind::Ui, px, fg);
        }
        if t.closable {
            let cr = ui.interact(id.with(("close", i)), close, Sense::click());
            if ui.a11y_on() {
                ui.a11y_node(id.with(("close", i)), Role::Button, &format!("Close {}", t.text));
            }
            if cr.clicked {
                out.closed = Some(i);
            }
            if cr.hovered {
                ui.set_cursor_icon(CursorIcon::Pointer);
            }
            let xw = ui.measure("×", FontKind::Ui, px).ceil() as i32;
            let mut painter = ui.painter();
            if cr.hovered {
                painter.rect(close, p.surface_alt.mix(p.fg, 0.1), radius);
            }
            if cr.hovered || is_active || hovered {
                painter.text(Point::new(close.x + (close.w - xw) / 2, close.y + (close.h - lh) / 2), "×", FontKind::Ui, px, p.muted);
            }
        }
    }

    let plus = Rect::new(strip_right + ui.px(4), rect.y, plus_w, rect.h);
    let pr = ui.interact(id.with("plus"), plus, Sense::click());
    out.plus_clicked = pr.clicked;
    ui.a11y_node(id.with("plus"), Role::Button, "New tab");
    let pw = ui.measure("+", FontKind::Ui, px).ceil() as i32;
    let lh = ui.line_height(FontKind::Ui, px);
    let mut painter = ui.painter();
    if pr.hovered {
        painter.rect(plus.inset(ui_px_inset(plus)), p.surface_alt, radius);
    }
    painter.text(Point::new(plus.x + (plus.w - pw) / 2, plus.y + (plus.h - lh) / 2), "+", FontKind::Ui, px, p.muted);
    out
}

fn ui_px_inset(r: Rect) -> i32 {
    (r.h / 8).max(1)
}
