//! Virtualized list: only rows inside the viewport are laid out and drawn,
//! so a 100k-row list costs the same per frame as a 10-row one.

use crate::geom::{Point, Rect};
use crate::input::{Event, Key};
use crate::ui::{Id, Response, Sense, Ui};

use super::scroll::{scroll_area, scroll_into_view};

#[derive(Debug, Clone, Copy, Default)]
pub struct ListResponse {
    pub response: Response,
    /// Enter or double-click on this row.
    pub activated: Option<usize>,
    /// Right-click on this row, at this point.
    pub context: Option<(usize, Point)>,
}

/// A selectable list of `rows` rows, each `row_h` logical pixels tall.
/// `f(ui, row_rect, index, selected)` draws one visible row's contents
/// (the selection highlight is already painted).
#[allow(clippy::too_many_arguments)]
pub fn list(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    rows: usize,
    row_h: i32,
    selected: &mut Option<usize>,
    mut f: impl FnMut(&mut Ui, Rect, usize, bool),
) -> ListResponse {
    let r = ui.interact(id, rect, Sense::focusable());
    let rh = ui.px(row_h).max(1);
    let sid = id.with("scroll");
    let mut out = ListResponse { response: r, ..Default::default() };
    if selected.is_some_and(|s| s >= rows) {
        *selected = rows.checked_sub(1);
    }

    if r.focused && rows > 0 {
        let page = (rect.h / rh).max(1) as usize;
        let events = ui.input().events.clone();
        let mut moved = false;
        for e in &events {
            let Event::KeyDown { key, mods, .. } = e else { continue };
            if mods.ctrl || mods.alt || mods.logo {
                continue;
            }
            let cur = *selected;
            let next = match key {
                Key::Up => Some(cur.map_or(0, |c| c.saturating_sub(1))),
                Key::Down => Some(cur.map_or(0, |c| (c + 1).min(rows - 1))),
                Key::PageUp => Some(cur.map_or(0, |c| c.saturating_sub(page))),
                Key::PageDown => Some(cur.map_or(0, |c| (c + page).min(rows - 1))),
                Key::Home => Some(0),
                Key::End => Some(rows - 1),
                Key::Enter => {
                    out.activated = *selected;
                    None
                }
                _ => None,
            };
            if let Some(n) = next {
                *selected = Some(n);
                moved = true;
            }
        }
        if moved && let Some(s) = *selected {
            let top = s as i32 * rh;
            scroll_into_view(ui, sid, top, top + rh, rect.h);
        }
    }

    let content_h = (rows as i64 * rh as i64).min(i32::MAX as i64) as i32;
    scroll_area(ui, sid, rect, content_h, |ui, vis, off| {
        let first = (off / rh) as usize;
        let last = (((off + vis.h + rh - 1) / rh) as usize).min(rows);
        let p = ui.style().palette;
        for i in first..last {
            let row = Rect::new(vis.x, vis.y + i as i32 * rh - off, vis.w, rh);
            let rr = ui.interact(id.with(i), row, Sense::click());
            if rr.pressed || rr.clicked {
                *selected = Some(i);
                ui.request_focus(id);
            }
            if rr.double_clicked {
                out.activated = Some(i);
            }
            if rr.right_clicked {
                *selected = Some(i);
                out.context = Some((i, ui.input().mouse));
            }
            let is_sel = *selected == Some(i);
            if is_sel {
                let c = if ui.has_focus(id) { p.selection } else { p.selection.mix(p.surface, 0.5) };
                ui.painter().rect(row, c, 0.0);
            } else if rr.hovered {
                ui.painter().rect(row, p.surface_alt, 0.0);
            }
            f(ui, row, i, is_sel);
        }
    });
    out
}
