//! Tab strip (`tokens::TAB_H` tall): tabs with an optional status dot or
//! icon, a middle-elided title and a close button (shown on the active,
//! hovered or keyboard-focused tab), drag-to-reorder, drag-out, and a "+"
//! icon button after the last tab. The active tab is raised (window
//! background, joined to the content through the strip's bottom border)
//! with a 2 px accent bar on top. The caller owns the tab list and the
//! active index; `tabs` reports what the user did in a `TabsResponse`.

use crate::a11y::Role;
use crate::geom::{Point, Rect};
use crate::icons::Icon;
use crate::input::{CursorIcon, Event, Key, Modifiers, MouseButton};
use crate::style::{TextRole, tokens};
use crate::ui::{Id, Sense, Ui};

use super::badge::{Status, status_color};
use super::button::{ButtonKind, icon_button_tip};
use super::label::{Elide, elide_mode};

/// One tab as the caller describes it for this frame.
#[derive(Debug, Clone, Copy)]
pub struct TabTitle<'a> {
    /// Label shown on the tab (elided in the middle to fit) and used as its
    /// accessible name.
    pub text: &'a str,
    /// Whether the tab draws a close button.
    pub closable: bool,
    /// A status dot before the title, with its words (added to the tab's
    /// accessible name, so the state is not told by colour alone).
    pub status: Option<(Status, &'a str)>,
    /// An icon before the title (used when there is no status dot).
    pub icon: Option<Icon>,
}

impl TabTitle<'_> {
    /// A closable tab with no dot or icon (for `..` in literals).
    pub const DEFAULT: TabTitle<'static> = TabTitle { text: "", closable: true, status: None, icon: None };
}

/// Options for `tabs_with`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TabsOpts<'a> {
    /// Tooltip of the "+" button (e.g. with its shortcut); "New tab" if unset.
    pub plus_tooltip: Option<&'a str>,
}

/// What happened to the tab strip this frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabsResponse {
    /// The close button of this tab was clicked.
    pub closed: Option<usize>,
    /// A tab was dragged from `.0` to `.1`; the caller reorders its tabs.
    pub moved: Option<(usize, usize)>,
    /// The "+" (new tab) button was clicked.
    pub plus_clicked: bool,
    /// The dragged tab left the strip (reported once per drag): the caller
    /// may start a drag-and-drop with it. The tab that was active before
    /// the press is active again, so the content below is a drop target.
    pub dragged_out: Option<usize>,
    /// Where each tab was drawn.
    pub tab_rects: Vec<Rect>,
}

/// Per-strip drag state kept in widget memory between frames.
#[derive(Debug, Clone, Copy, Default)]
struct TabDrag {
    /// Current index of the tab being dragged (follows it as it moves).
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

/// Index of the tab under x-coordinate `x`: the first tab whose right edge is
/// past `x`, or the last tab when `x` is beyond the strip.
fn index_at(rects: &[Rect], x: i32) -> usize {
    rects.iter().position(|r| x < r.right()).unwrap_or(rects.len().saturating_sub(1))
}

/// Draws a tab strip in `rect` and handles clicks, Left/Right keys,
/// drag-to-reorder, drag-out, close buttons and the "+" button. `active` is
/// updated in place; everything else is reported in the returned response.
pub fn tabs(ui: &mut Ui, id: Id, rect: Rect, titles: &[TabTitle], active: &mut usize) -> TabsResponse {
    tabs_with(ui, id, rect, titles, active, TabsOpts::default())
}

/// `tabs` with options (the "+" button's tooltip).
pub fn tabs_with(ui: &mut Ui, id: Id, rect: Rect, titles: &[TabTitle], active: &mut usize, opts: TabsOpts) -> TabsResponse {
    let mut out = TabsResponse::default();
    let n = titles.len();
    let (font, px) = ui.text_style(TextRole::Body);
    let pad = ui.px(tokens::SPACE_MD);
    let close_side = ui.px(tokens::ICON + tokens::SPACE_XS);
    let icon_px = ui.px(tokens::ICON);
    let lead = icon_px + ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
    let gap = ui.px(tokens::SPACE_2XS);
    // Tabs hang from a small top margin, so the raised one reads as a tab.
    let top = ui.px(tokens::SPACE_XS + tokens::SPACE_2XS).min(rect.h / 4);
    let edge = ui.px(tokens::SPACE_XS);
    let plus_side = (rect.h - top).max(1);
    let avail = (rect.w - plus_side - 3 * edge).max(0);

    // Widths: natural size within [min, max], shrunk evenly when crowded.
    let (min_w, max_w) = (ui.px(96), ui.px(240));
    let mut widths: Vec<i32> = titles
        .iter()
        .map(|t| {
            let tw = ui.measure(t.text, font, px).ceil() as i32;
            let has_lead = t.status.is_some() || t.icon.is_some();
            (tw + 2 * pad + if has_lead { lead } else { 0 } + if t.closable { close_side } else { 0 }).clamp(min_w, max_w)
        })
        .collect();
    let total: i32 = widths.iter().sum::<i32>() + gap * n.saturating_sub(1) as i32;
    // Too wide: every tab gets the same share (this may go below `min_w`).
    if total > avail && n > 0 {
        let each = ((avail - gap * (n as i32 - 1)) / n as i32).max(0);
        widths.iter_mut().for_each(|w| *w = each);
    }
    let mut x = rect.x + edge;
    for w in &widths {
        out.tab_rects.push(Rect::new(x, rect.y, *w, rect.h));
        x += w + gap;
    }
    let strip_right = out.tab_rects.last().map_or(rect.x, |r| r.right());
    let strip = Rect::new(rect.x, rect.y, strip_right - rect.x, rect.h);

    // The whole strip is one widget, so a drag survives the reorder.
    let sr = ui.interact(id, strip, Sense { click: true, drag: true, focus: true });
    // Accessibility: a tab list with one selectable tab node per tab; an
    // a11y click activates that tab, and a11y focus goes to the active one.
    if ui.a11y_on() {
        ui.a11y_group(id, Role::TabList, "Tabs", |ui| {
            for (i, t) in titles.iter().enumerate() {
                let tid = id.with(("tab", i));
                if ui.a11y_clicked(tid) {
                    *active = i;
                }
                let name = match t.status {
                    Some((_, words)) if !words.is_empty() => format!("{}, {words}", t.text),
                    _ => t.text.to_string(),
                };
                if let Some(nd) = ui.a11y_node(tid, Role::Tab, &name) {
                    nd.bounds = out.tab_rects[i];
                    nd.selected = Some(i == *active);
                }
            }
            ui.a11y_focus_child(id, id.with(("tab", *active)));
        });
    }
    // Keyboard: Left/Right move the active tab while the strip has focus.
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
                // Held and moved above/below the strip: report the drag-out
                // once and restore the tab that was active before the press.
                Some(i) if !pressed_now && (mouse.y < rect.y || mouse.y >= rect.bottom()) => {
                    if !drag.out {
                        drag.out = true;
                        out.dragged_out = Some(i);
                        if let Some(b) = drag.before {
                            *active = b;
                        }
                    }
                }
                // Held within the strip over another tab: report a move and
                // re-index the dragged and "before" tabs to the new order.
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
                // Fresh press (or no drag yet): start dragging the tab under
                // the mouse and activate it, remembering the previous one.
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
    let radius = tokens::RADIUS_MD * ui.scale();
    let bw = ui.px(1).max(1);
    let accent_h = ui.px(2).max(1);
    let lh = ui.line_height(font, px);
    let dot = ui.px(tokens::DOT).max(4);
    // The strip's bottom border; the active tab covers it to join the content.
    ui.painter().rect(Rect::new(rect.x, rect.bottom() - bw, rect.w, bw), p.border, 0.0);
    let strip_focus = sr.focused && ui.focus_visible();
    for (i, t) in titles.iter().enumerate() {
        let tr = out.tab_rects[i];
        // The tab face, below the top margin; its rounded rect runs past the
        // strip's bottom and is clipped there, so only the top is rounded.
        let face = Rect::new(tr.x, tr.y + top, tr.w, tr.h - top);
        let long = Rect::new(face.x, face.y, face.w, face.h + radius.ceil() as i32 + bw);
        let hovered = sr.hovered && tr.contains(mouse);
        let is_active = i == *active;
        let close =
            Rect::new(face.right() - ui.px(tokens::SPACE_SM) - close_side, face.y + (face.h - close_side) / 2, close_side, close_side);
        let mut tx = face.x + pad;
        let text_right = if t.closable { close.x - ui.px(tokens::SPACE_XS) } else { face.right() - pad };
        {
            let mut painter = ui.painter();
            painter.push_clip(face);
            if is_active {
                painter.rect(long, p.border, radius);
                painter.rect(long.inset(bw), p.bg, (radius - bw as f32).max(0.0));
                painter.rect(Rect::new(face.x + bw, face.y, face.w - 2 * bw, accent_h), p.accent, 0.0);
            } else if hovered {
                painter.rect(long, p.hover, radius);
            }
            painter.pop_clip();
        }
        if let Some((status, _)) = t.status {
            let c = status_color(&p, status);
            let d = Rect::new(tx + (icon_px - dot) / 2, face.y + (face.h - dot) / 2, dot, dot);
            ui.painter().rect(d, c, dot as f32 / 2.0);
            tx += lead;
        } else if let Some(icon) = t.icon {
            let c = if is_active { p.fg } else { p.muted };
            ui.painter().icon(Rect::new(tx, face.y + (face.h - icon_px) / 2, icon_px, icon_px), icon, c);
            tx += lead;
        }
        let shown = elide_mode(ui, t.text, font, px, text_right - tx, Elide::Middle);
        let fg = if is_active || hovered { p.fg } else { p.muted };
        ui.painter().text(Point::new(tx, face.y + (face.h - lh) / 2), &shown, font, px, fg);
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
            // Shown on the active tab (so also while the strip has keyboard
            // focus) and on the hovered one.
            if cr.hovered || is_active || hovered || strip_focus {
                let s = ui.px(tokens::ICON - 4).max(8);
                let small_radius = tokens::RADIUS_SM * ui.scale();
                let mut painter = ui.painter();
                if cr.hovered {
                    painter.rect(close, p.hover.mix(p.fg, 0.08), small_radius);
                }
                let ic = Rect::new(close.x + (close.w - s) / 2, close.y + (close.h - s) / 2, s, s);
                painter.icon(ic, Icon::Close, if cr.hovered { p.fg } else { p.muted });
            }
        }
    }

    // The "+" icon button right after the last tab.
    let inset = ui.px(tokens::SPACE_2XS);
    let plus = Rect::new(strip_right + edge, rect.y + top, plus_side, plus_side).inset(inset);
    let tip = opts.plus_tooltip.unwrap_or("New tab");
    out.plus_clicked = icon_button_tip(ui, id.with("plus"), plus, Icon::Plus, "New tab", tip, ButtonKind::Ghost).clicked;
    out
}
