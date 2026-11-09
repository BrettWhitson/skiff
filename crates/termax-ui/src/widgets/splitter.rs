//! Horizontal splitter: a draggable divider between two panes.

use crate::geom::Rect;
use crate::input::{CursorIcon, Event, MouseButton};
use crate::ui::{Id, Sense, Ui};

/// Splits `rect` into left and right panes. `first` is the left pane's
/// width in logical pixels (persist it as-is); dragging the divider
/// updates it, clamped to `min..=max` and to the available width.
pub fn splitter_h(ui: &mut Ui, id: Id, rect: Rect, first: &mut f32, min: f32, max: f32) -> (Rect, Rect) {
    let s = ui.scale();
    let gap = ui.px(5).max(1);
    let room = ((rect.w - gap) as f32 / s).max(0.0);
    let hi = max.min(room).max(min.min(room));
    let lo = min.min(hi);
    *first = first.clamp(lo, hi);
    let mut w = (*first * s).round() as i32;

    let handle = Rect::new(rect.x + w, rect.y, gap, rect.h);
    let r = ui.interact(id, handle, Sense::drag());
    let mouse = ui.input().mouse;
    let down_now = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(MouseButton::Left, _)));
    if r.pressed {
        if down_now {
            *ui.memory::<i32>(id) = mouse.x - handle.x;
        } else {
            let grab = *ui.memory::<i32>(id);
            let new_w = mouse.x - rect.x - grab;
            *first = (new_w as f32 / s).clamp(lo, hi);
            w = (*first * s).round() as i32;
        }
    }
    if r.hovered || r.pressed {
        ui.set_cursor_icon(CursorIcon::ResizeH);
    }
    let p = ui.style().palette;
    let line = ui.px(1).max(1);
    let c = if r.hovered || r.pressed { p.accent } else { p.border };
    ui.painter().rect(Rect::new(rect.x + w + (gap - line) / 2, rect.y, line, rect.h), c, 0.0);
    (Rect::new(rect.x, rect.y, w, rect.h), Rect::new(rect.x + w + gap, rect.y, (rect.w - w - gap).max(0), rect.h))
}
