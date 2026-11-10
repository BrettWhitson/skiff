//! Horizontal splitter: a draggable divider between two panes. It is also
//! focusable: the arrow keys (and a screen reader's increment / decrement)
//! move it by [`KEY_STEP`]. The stored width is in logical pixels so it
//! survives scale changes.

use crate::a11y::{A11yAction, Role};
use crate::geom::Rect;
use crate::input::{CursorIcon, Event, Key, Modifiers, MouseButton};
use crate::ui::{Id, Sense, Ui};

/// How far one arrow key press (or increment / decrement) moves the
/// divider, in logical pixels.
pub const KEY_STEP: f32 = 16.0;

/// Splits `rect` into left and right panes. `first` is the left pane's
/// width in logical pixels (persist it as-is); dragging the divider
/// updates it, clamped to `min..=max` and to the available width. `name`
/// is what screen readers call the divider; its value is the left pane's
/// share of the width.
pub fn splitter_h(ui: &mut Ui, id: Id, rect: Rect, first: &mut f32, min: f32, max: f32, name: &str) -> (Rect, Rect) {
    let s = ui.scale();
    let gap = ui.px(5).max(1);
    // The clamp range shrinks to fit the room; `min` gives way when even it does not fit.
    let room = ((rect.w - gap) as f32 / s).max(0.0);
    let hi = max.min(room).max(min.min(room));
    let lo = min.min(hi);
    // Clamp only what is laid out; `first` changes only when dragged, so a
    // narrow or minimized window never loses the user's width.
    let mut w = (first.clamp(lo, hi) * s).round() as i32;

    // The grab handle is a `gap`-wide strip at the divider.
    let handle = Rect::new(rect.x + w, rect.y, gap, rect.h);
    let r = ui.interact(id, handle, Sense { click: false, drag: true, focus: true });
    let mouse = ui.input().mouse;
    let down_now = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(MouseButton::Left, _)));
    if r.pressed {
        // On the press frame remember where in the handle the mouse grabbed it,
        // then keep that offset while dragging so the divider does not jump.
        if down_now {
            *ui.memory::<i32>(id) = mouse.x - handle.x;
        } else {
            let grab = *ui.memory::<i32>(id);
            let new_w = mouse.x - rect.x - grab;
            *first = (new_w as f32 / s).clamp(lo, hi);
            w = (*first * s).round() as i32;
        }
    }
    // Keyboard and screen-reader steps, from the laid-out width so a
    // clamped divider moves from where it is drawn.
    let mut step = 0.0;
    if r.focused {
        while ui.input_mut().take_key(&Key::Left, Modifiers::NONE) {
            step -= KEY_STEP;
        }
        while ui.input_mut().take_key(&Key::Right, Modifiers::NONE) {
            step += KEY_STEP;
        }
    }
    if ui.take_a11y_action(id, |a| matches!(a, A11yAction::Increment)).is_some() {
        step += KEY_STEP;
    }
    if ui.take_a11y_action(id, |a| matches!(a, A11yAction::Decrement)).is_some() {
        step -= KEY_STEP;
    }
    if step != 0.0 {
        *first = (w as f32 / s + step).clamp(lo, hi);
        w = (*first * s).round() as i32;
    }
    if r.hovered || r.pressed {
        ui.set_cursor_icon(CursorIcon::ResizeH);
    }
    let p = ui.style().palette;
    // Draw a 1px line centred in the handle, highlighted while hovered or dragged.
    let line = ui.px(1).max(1);
    let c = if r.hovered || r.pressed || r.focused { p.accent } else { p.border };
    ui.painter().rect(Rect::new(rect.x + w + (gap - line) / 2, rect.y, line, rect.h), c, 0.0);
    if let Some(node) = ui.a11y_node(id, Role::Splitter, name) {
        let share = if room > 0.0 { w as f32 / s / room } else { 0.0 };
        node.value = Some(format!("{}%", (share * 100.0).round()));
    }
    (Rect::new(rect.x, rect.y, w, rect.h), Rect::new(rect.x + w + gap, rect.y, (rect.w - w - gap).max(0), rect.h))
}
