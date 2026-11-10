//! A number field with − / + buttons (a "spin button"): type a number,
//! click the buttons, or use Up/Down (one step) and Page Up/Down (ten
//! steps) while it has focus. The value is clamped to its range; text that
//! does not parse is replaced by the value when focus leaves or on Enter.

use crate::a11y::{A11yAction, Role};
use crate::geom::Rect;
use crate::icons::Icon;
use crate::input::{CursorIcon, Key, Modifiers};
use crate::style::tokens;
use crate::ui::{Id, Sense, Ui};

use super::text_input::{TextInputOpts, text_input};

/// Range, step and naming of a `stepper`.
#[derive(Debug, Clone, Copy)]
pub struct StepperOpts<'a> {
    pub min: i64,
    pub max: i64,
    /// Change per click / arrow key (Page Up/Down move ten steps).
    pub step: i64,
    /// The accessible name (the setting it changes).
    pub label: &'a str,
}

impl Default for StepperOpts<'_> {
    /// 0..=100 in steps of 1, unnamed.
    fn default() -> Self {
        StepperOpts { min: 0, max: 100, step: 1, label: "" }
    }
}

/// `v` moved by `delta`, clamped to `min..=max` (saturating, never overflowing).
pub fn step_value(v: i64, delta: i64, min: i64, max: i64) -> i64 {
    v.saturating_add(delta).clamp(min, max.max(min))
}

/// The value typed in `s` (surrounding spaces allowed), clamped to the
/// range; `None` when it is not a number.
pub fn parse_value(s: &str, min: i64, max: i64) -> Option<i64> {
    s.trim().parse::<i64>().ok().map(|v| v.clamp(min, max.max(min)))
}

/// Text kept while the field is being edited.
#[derive(Debug, Clone, Default)]
struct StepperText {
    text: String,
    /// The field had focus last frame.
    editing: bool,
}

/// A number field in `rect` with − and + buttons at its right end.
/// Returns true when `value` changed this frame.
pub fn stepper(ui: &mut Ui, id: Id, rect: Rect, value: &mut i64, opts: StepperOpts) -> bool {
    let before = *value;
    let field = id.with("field");
    let focused = ui.has_focus(field);
    // Steps (in units of `opts.step`) from the keyboard and screen readers.
    let mut delta = 0i64;
    if focused && ui.keys_for(field) {
        for (k, d) in [(Key::Up, 1), (Key::Down, -1), (Key::PageUp, 10), (Key::PageDown, -10)] {
            while ui.input_mut().take_key(&k, Modifiers::NONE) {
                delta += d;
            }
        }
    }
    if ui.take_a11y_action(field, |a| matches!(a, A11yAction::Increment)).is_some() {
        delta += 1;
    }
    if ui.take_a11y_action(field, |a| matches!(a, A11yAction::Decrement)).is_some() {
        delta -= 1;
    }
    // The buttons sit on top of the field's right end, so they get the clicks.
    let bh = rect.h - 2 * ui.px(tokens::SPACE_2XS);
    let bw = bh.min(rect.w / 3);
    let plus = Rect::new(rect.right() - ui.px(tokens::SPACE_2XS) - bw, rect.y + ui.px(tokens::SPACE_2XS), bw, bh);
    let minus = plus.translate(-bw, 0);

    let mut st = ui.memory::<StepperText>(id).clone();
    if !st.editing || delta != 0 {
        st.text = value.to_string();
    }
    let field_rect = Rect::new(rect.x, rect.y, rect.w, rect.h);
    let r = text_input(ui, field, field_rect, &mut st.text, TextInputOpts { label: opts.label, ..Default::default() });
    if r.changed
        && let Some(v) = parse_value(&st.text, opts.min, opts.max)
    {
        *value = v;
    }
    let mut step = |ui: &mut Ui, rid: Id, rr: Rect, icon: Icon, d: i64, name: &str| {
        let can = if d > 0 { *value < opts.max } else { *value > opts.min };
        let resp = ui.interact(rid, rr, if can { Sense::click() } else { Sense::hover() });
        if let Some(n) = ui.a11y_node(rid, Role::Button, name) {
            n.disabled = !can;
        }
        if resp.clicked && can {
            delta += d;
        }
        let p = ui.style().palette;
        let fg = if !can {
            p.text_disabled
        } else if resp.hovered {
            p.fg
        } else {
            p.muted
        };
        if resp.hovered && can {
            ui.set_cursor_icon(CursorIcon::Pointer);
            let radius = tokens::RADIUS_SM * ui.scale();
            ui.painter().rect(rr, if resp.pressed { p.selection } else { p.hover }, radius);
        }
        let s = ui.px(tokens::ICON);
        ui.painter().icon(Rect::new(rr.x + (rr.w - s) / 2, rr.y + (rr.h - s) / 2, s, s), icon, fg);
    };
    step(ui, id.with("minus"), minus, Icon::Minus, -1, "Decrease");
    step(ui, id.with("plus"), plus, Icon::Plus, 1, "Increase");
    if delta != 0 {
        *value = step_value(*value, delta.saturating_mul(opts.step.max(1)), opts.min, opts.max);
        st.text = value.to_string();
    }
    // Leaving the field (or Enter) shows the value the text settled on.
    let now_focused = ui.has_focus(field);
    if (st.editing && !now_focused) || r.submitted {
        st.text = value.to_string();
    }
    st.editing = now_focused;
    *ui.memory::<StepperText>(id) = st;
    if let Some(n) = ui.a11y_node(field, Role::SpinButton, opts.label) {
        n.role = Role::SpinButton;
        n.value = Some(value.to_string());
    }
    *value != before
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steps clamp to the range without overflowing; typed text parses,
    /// clamps, and rejects non-numbers.
    #[test]
    fn stepping_and_parsing() {
        assert_eq!(step_value(5, 1, 0, 10), 6);
        assert_eq!(step_value(10, 1, 0, 10), 10);
        assert_eq!(step_value(0, -1, 0, 10), 0);
        assert_eq!(step_value(i64::MAX, 1, i64::MIN, i64::MAX), i64::MAX);
        assert_eq!(parse_value(" 42 ", 0, 100), Some(42));
        assert_eq!(parse_value("500", 0, 100), Some(100));
        assert_eq!(parse_value("4x", 0, 100), None);
    }
}
