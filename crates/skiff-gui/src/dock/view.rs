//! Drawing and input for the dock: the drop overlays shown while a tab is
//! dragged (a translucent preview of the area the tab would take, a
//! five-way dock guide in the middle of the group under the pointer, and an
//! insertion marker on tab strips) and the dividers between groups.
//!
//! Dividers drag with the mouse (snapping to 25 / 33 / 50 / 67 / 75%; Alt
//! drags freely; a double-click equalizes) and, focused (F6 reaches them),
//! move with the arrow keys from snap point to snap point. A divider is a
//! 1 px line in a `GAP`-wide hit zone that turns accent on hover and drag;
//! screen readers see a Splitter with its share as the value.

use skiff_ui::a11y::{A11yAction, Role};
use skiff_ui::color::Color;
use skiff_ui::geom::{Point, Rect};
use skiff_ui::input::{CursorIcon, Event, Key, Modifiers, MouseButton};
use skiff_ui::style::tokens;
use skiff_ui::ui::{Id, Sense, Ui};
use skiff_ui::widgets::tooltip;

use super::layout::{Dir, Divider, Layout, MIN_RATIO, SNAP_PX, Zone, ratio_at, snap, step_ratio};

/// Side of one square of the dock guide, logical px.
const GUIDE: i32 = 30;
/// Space between the guide's squares, logical px.
const GUIDE_GAP: i32 = 4;

/// The strip position a tab dropped at x-coordinate `x` takes: before the
/// first tab whose middle is right of `x` (the end past the last one).
pub fn insertion_index(tab_rects: &[Rect], x: i32) -> usize {
    tab_rects.iter().position(|r| x < r.center().x).unwrap_or(tab_rects.len())
}

/// The insertion marker on a tab strip: a 2 px accent bar in the gap before
/// tab `index` (after the last tab when `index` is past the end).
pub fn insertion_marker(ui: &mut Ui, tab_rects: &[Rect], strip: Rect, index: usize, accent: Color) {
    let w = ui.px(3).max(2);
    let x = match (tab_rects.get(index), tab_rects.last()) {
        (Some(r), _) => r.x,
        (None, Some(last)) => last.right() + ui.px(tokens::SPACE_2XS),
        (None, None) => strip.x + ui.px(tokens::SPACE_XS),
    };
    let inset = ui.px(tokens::SPACE_XS);
    let bar = Rect::new(x - w / 2 - ui.px(1), strip.y + inset, w, strip.h - 2 * inset);
    let radius = w as f32 / 2.0;
    ui.painter().rect(bar, accent, radius);
}

/// The area a drop would give the tab: a translucent accent fill with a
/// solid accent outline, inset a little from the group's edges.
pub fn drop_preview(ui: &mut Ui, r: Rect, accent: Color) {
    let r = r.inset(ui.px(tokens::SPACE_2XS));
    let radius = tokens::RADIUS_MD * ui.scale();
    let w = ui.px(2).max(1);
    let mut painter = ui.painter();
    painter.rect(r, accent.with_alpha(46), radius);
    painter.border(r, accent, w, radius);
}

/// The five squares of the dock guide in `content` (left, right, top,
/// bottom and centre, in a plus shape), or none when the group is too
/// small to show it.
pub fn guide_rects(ui: &Ui, content: Rect) -> Vec<(Zone, Rect)> {
    let (s, g) = (ui.px(GUIDE), ui.px(GUIDE_GAP));
    let span = 3 * s + 2 * g;
    if content.w < span + 2 * s || content.h < span + 2 * s {
        return vec![];
    }
    let c = content.center();
    let at = |dx: i32, dy: i32| Rect::new(c.x - s / 2 + dx * (s + g), c.y - s / 2 + dy * (s + g), s, s);
    vec![(Zone::Center, at(0, 0)), (Zone::Left, at(-1, 0)), (Zone::Right, at(1, 0)), (Zone::Top, at(0, -1)), (Zone::Bottom, at(0, 1))]
}

/// The zone under the pointer in a group's content: a guide square when it
/// is over one, else the position (`Zone::at`).
pub fn zone_under(guide: &[(Zone, Rect)], content: Rect, at: Point) -> Zone {
    guide.iter().find(|(_, r)| r.contains(at)).map_or_else(|| Zone::at(content, at), |(z, _)| *z)
}

/// Draws the dock guide: each square shows the part of the group its zone
/// fills, the zone under the pointer in accent. Zones `offered` refuses
/// keep a faint outline, so the guide keeps its shape.
pub fn draw_guide(ui: &mut Ui, guide: &[(Zone, Rect)], hot: Zone, offered: impl Fn(Zone) -> bool) {
    let p = ui.style().palette;
    let radius = tokens::RADIUS_SM * ui.scale();
    let bw = ui.px(1).max(1);
    let inner_pad = ui.px(tokens::SPACE_XS + 1);
    for &(zone, r) in guide {
        if !offered(zone) {
            ui.painter().border(r, p.border, bw, radius);
            continue;
        }
        let on = zone == hot;
        ui.elevate(r, radius);
        let mut painter = ui.painter();
        painter.rect(r, if on { p.accent } else { p.surface }, radius);
        painter.border(r, if on { p.accent } else { p.control }, bw, radius);
        // The miniature: an outline of the group with the zone's part filled.
        let mini = r.inset(inner_pad);
        let ink = if on { p.accent_fg } else { p.muted };
        painter.border(mini, ink, bw, 0.0);
        let fill = match zone.dir() {
            None => mini,
            Some(d) => {
                let (a, _, b) = super::layout::split_rect(mini, d, 0.5, 0);
                if zone.forward() { b } else { a }
            }
        };
        painter.rect(fill, ink.with_alpha(if on { 255 } else { 140 }), 0.0);
    }
}

/// One divider: drag it (snapping unless Alt is held; double-click
/// equalizes), or focus it and use the arrow keys (also the screen
/// reader's increment / decrement), which step through the snap points.
pub fn divider(ui: &mut Ui, id: Id, d: &Divider, gap: i32, layout: &mut Layout) {
    let r = ui.interact(id, d.handle, Sense { click: true, drag: true, focus: true });
    let mut ratio = d.ratio;
    let mut snapped = None;
    // Dragging (not on the press frame itself, so a click doesn't jump the
    // divider): follow the pointer, then snap when within SNAP_PX of a snap
    // point, measured in px of the split's room.
    let down_now = ui.input().events.iter().any(|e| matches!(e, Event::MouseDown(MouseButton::Left, _)));
    if r.pressed && !down_now && r.dragged.is_some() {
        ratio = ratio_at(d.area, d.dir, gap, ui.input().mouse);
        let room = if d.dir == Dir::Right { d.area.w } else { d.area.h } - gap;
        if !ui.input().mods.alt
            && let Some(s) = snap(ratio, room, ui.px(SNAP_PX))
        {
            ratio = s;
            snapped = Some(s);
        }
    }
    // Keyboard and screen-reader steps: to the next snap point, else KEY_STEP.
    if r.focused {
        let (less, more) = match d.dir {
            Dir::Right => (Key::Left, Key::Right),
            Dir::Down => (Key::Up, Key::Down),
        };
        if ui.input_mut().take_key(&less, Modifiers::NONE) {
            ratio = step_ratio(ratio, false);
        }
        if ui.input_mut().take_key(&more, Modifiers::NONE) {
            ratio = step_ratio(ratio, true);
        }
    }
    if ui.take_a11y_action(id, |a| matches!(a, A11yAction::Increment)).is_some() {
        ratio = step_ratio(ratio, true);
    }
    if ui.take_a11y_action(id, |a| matches!(a, A11yAction::Decrement)).is_some() {
        ratio = step_ratio(ratio, false);
    }
    // Write back to the stored layout (the drawn one may be min-clamped).
    if r.double_clicked {
        layout.equalize_at(&d.path);
        ratio = layout.ratio(&d.path).unwrap_or(ratio);
    } else if ratio != d.ratio {
        layout.set_ratio(&d.path, ratio);
    }
    let shown = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
    if r.hovered || r.pressed {
        ui.set_cursor_icon(match d.dir {
            Dir::Right => CursorIcon::ResizeH,
            Dir::Down => CursorIcon::ResizeV,
        });
    }
    let p = ui.style().palette;
    let c = if r.hovered || r.pressed || r.focused { p.accent } else { p.border };
    // Snapped: the line thickens and the share shows by the pointer.
    let line = if snapped.is_some() || r.focused { ui.px(3).max(2) } else { ui.px(1).max(1) };
    let h = d.handle;
    let mark = match d.dir {
        Dir::Right => Rect::new(h.x + (h.w - line) / 2, h.y, line, h.h),
        Dir::Down => Rect::new(h.x, h.y + (h.h - line) / 2, h.w, line),
    };
    ui.painter().rect(mark, c, 0.0);
    if let Some(s) = snapped {
        let m = ui.input().mouse;
        let at = Rect::new(m.x, m.y + ui.px(8), 1, 1);
        tooltip(ui, at, &format!("{}%", (s * 100.0).round()));
    }
    let name = match d.dir {
        Dir::Right => "Group divider, left and right",
        Dir::Down => "Group divider, top and bottom",
    };
    if let Some(node) = ui.a11y_node(id, Role::Splitter, name) {
        node.value = Some(format!("{}%", (shown * 100.0).round()));
        node.bounds = d.handle;
    }
}
