//! Vertical scroll area with wheel support and a draggable scrollbar.
//! The offset lives in widget memory, so callers only pass the area's id.

use crate::geom::Rect;
use crate::input::Event;
use crate::ui::{Id, Sense, Ui};

/// A scroll area's persisted state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollState {
    /// Pixels scrolled from the top of the content.
    pub offset: i32,
}

/// Sets the scroll offset of area `id` (clamped when it is next drawn).
pub fn scroll_to(ui: &mut Ui, id: Id, y: i32) {
    ui.memory::<ScrollState>(id).offset = y.max(0);
}

/// Scrolls area `id` the least amount that makes content rows
/// `top..bottom` visible in a view `view_h` pixels tall.
pub fn scroll_into_view(ui: &mut Ui, id: Id, top: i32, bottom: i32, view_h: i32) {
    let st = ui.memory::<ScrollState>(id);
    if top < st.offset {
        st.offset = top;
    } else if bottom > st.offset + view_h {
        st.offset = bottom - view_h;
    }
}

/// Lays out `content_height` pixels of content in `rect`. `f` receives the
/// visible area (excluding the scrollbar) and the current offset; content
/// at content-y `cy` belongs at screen-y `visible.y + cy - offset`.
pub fn scroll_area<R>(ui: &mut Ui, id: Id, rect: Rect, content_height: i32, f: impl FnOnce(&mut Ui, Rect, i32) -> R) -> R {
    ui.interact(id, rect, Sense::hover());
    let max = (content_height - rect.h).max(0);
    let mut off = ui.memory::<ScrollState>(id).offset;

    // The wheel scrolls the area under the mouse and is consumed, so an outer
    // area does not scroll as well.
    if max > 0 && rect.contains(ui.input().mouse) {
        let (_, dy) = ui.input().wheel();
        if dy != 0.0 {
            off -= dy.round() as i32;
            ui.input_mut().events.retain(|e| !matches!(e, Event::Wheel { .. }));
        }
    }
    off = off.clamp(0, max);

    // The scrollbar appears only when the content overflows.
    let bar_w = if max > 0 { ui.px(10).min(rect.w) } else { 0 };
    let visible = Rect::new(rect.x, rect.y, rect.w - bar_w, rect.h);
    let mut thumb = Rect::default();
    let mut thumb_hot = false;
    if bar_w > 0 && rect.h > 0 {
        let track = Rect::new(rect.right() - bar_w, rect.y, bar_w, rect.h);
        // The thumb's share of the track matches the visible share of the
        // content, but never shorter than 24px.
        let thumb_h = ((rect.h as i64 * rect.h as i64 / content_height.max(1) as i64) as i32).max(ui.px(24)).min(rect.h);
        let range = rect.h - thumb_h;
        let thumb_at = |off: i32| {
            let y = if max > 0 { (off as i64 * range as i64 / max as i64) as i32 } else { 0 };
            Rect::new(track.x, rect.y + y, bar_w, thumb_h)
        };
        // Track click pages; the thumb (registered after) wins its own area.
        let tr = ui.interact(id.with("track"), track, Sense::click());
        if tr.clicked {
            let my = ui.input().mouse.y;
            let t = thumb_at(off);
            if my < t.y {
                off -= rect.h;
            } else if my >= t.bottom() {
                off += rect.h;
            }
        }
        // Dragging the thumb by `d` moves the content `d * max / range`.
        let th = ui.interact(id.with("thumb"), thumb_at(off), Sense::drag());
        if let (Some(d), true) = (th.dragged, range > 0) {
            off += (d.y as i64 * max as i64 / range as i64) as i32;
        }
        off = off.clamp(0, max);
        thumb = thumb_at(off);
        thumb_hot = th.hovered || th.pressed;
    }
    ui.memory::<ScrollState>(id).offset = off;

    // Content is clipped to the visible area; the thumb is painted over it.
    let out = ui.with_clip(visible, |ui| f(ui, visible, off));

    if bar_w > 0 {
        let p = ui.style().palette;
        let c = if thumb_hot { p.muted } else { p.muted.with_alpha(140) };
        let inset = ui.px(2);
        let r = (bar_w - 2 * inset) as f32 / 2.0;
        ui.painter().rect(Rect::new(thumb.x + inset, thumb.y + inset, bar_w - 2 * inset, thumb.h - 2 * inset), c, r);
    }
    out
}
