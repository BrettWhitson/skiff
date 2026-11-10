//! Push buttons in four variants (primary, secondary, ghost, danger),
//! square icon buttons with a tooltip and accessible name, and the
//! keyboard activation (Space/Enter) shared with other clickable widgets.

use std::time::Duration;

use crate::a11y::Role;
use crate::color::Color;
use crate::geom::{Point, Rect};
use crate::icons::Icon;
use crate::input::{CursorIcon, Key, Modifiers};
use crate::style::{Palette, TextRole, tokens};
use crate::ui::{Id, Response, Sense, Ui};

use super::label::elide;
use super::tooltip::tooltip;

/// Activates a focused widget on Space or Enter (consuming the key).
pub(crate) fn keyboard_activate(ui: &mut Ui, id: Id, r: &mut Response) {
    if r.focused && ui.keys_for(id) {
        let space = ui.input_mut().take_key(&Key::Space, Modifiers::NONE);
        let enter = ui.input_mut().take_key(&Key::Enter, Modifiers::NONE);
        if space || enter {
            r.clicked = true;
        }
    }
}

/// A button's look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// Accent fill: the one main action of a screen or dialog.
    Primary,
    /// Raised face with an outline: ordinary actions.
    #[default]
    Secondary,
    /// No face until hovered: toolbars, icon buttons, low-emphasis actions.
    Ghost,
    /// Danger fill: destructive actions (delete, discard).
    Danger,
}

/// Options for `button_with`; `Default` is an enabled secondary button.
#[derive(Debug, Clone, Copy)]
pub struct ButtonOpts<'a> {
    pub kind: ButtonKind,
    /// Icon before the label.
    pub icon: Option<Icon>,
    /// Icon after the label (a dropdown's chevron).
    pub trailing: Option<Icon>,
    /// Disabled buttons are drawn dimmed, skip Tab and ignore clicks.
    pub enabled: bool,
    /// Shown after a hover delay or on keyboard focus.
    pub tooltip: Option<&'a str>,
    /// The name screen readers announce, when the label alone is unclear.
    pub a11y_name: Option<&'a str>,
}

impl Default for ButtonOpts<'_> {
    /// An enabled secondary button without icons or tooltip.
    fn default() -> Self {
        ButtonOpts { kind: ButtonKind::Secondary, icon: None, trailing: None, enabled: true, tooltip: None, a11y_name: None }
    }
}

impl<'a> ButtonOpts<'a> {
    /// Default options with `kind`.
    pub fn kind(kind: ButtonKind) -> Self {
        ButtonOpts { kind, ..Default::default() }
    }
}

/// A secondary button's fill at rest, hovered and pressed (its label is `fg`).
pub fn button_backgrounds(p: &Palette) -> [(&'static str, Color); 3] {
    let [(_, rest, _), (_, hover, _), (_, pressed, _)] = button_colors(p, ButtonKind::Secondary);
    [("rest", rest), ("hover", hover), ("pressed", pressed)]
}

/// Fill and label colour of a `kind` button at rest, hovered and pressed
/// (a ghost's rest fill is transparent; its label reads on the surface below).
pub fn button_colors(p: &Palette, kind: ButtonKind) -> [(&'static str, Color, Color); 3] {
    let states = |rest: Color, hover: Color, pressed: Color, label: &dyn Fn(Color) -> Color| {
        [("rest", rest, label(rest)), ("hover", hover, label(hover)), ("pressed", pressed, label(pressed))]
    };
    match kind {
        ButtonKind::Primary => {
            // Hover and press move away from the label colour, keeping contrast.
            let away = if p.accent_fg.contrast(p.fg) > p.accent_fg.contrast(p.bg) { p.fg } else { p.bg };
            states(p.accent, p.accent.mix(away, 0.12), p.accent.mix(away, 0.24), &|_| p.accent_fg)
        }
        ButtonKind::Secondary => states(p.surface_alt, p.surface_alt.mix(p.fg, 0.07), p.selection, &|_| p.fg),
        ButtonKind::Ghost => states(Color::TRANSPARENT, p.hover, p.selection, &|_| p.fg),
        ButtonKind::Danger => {
            let label = p.text_on(p.danger);
            // Toward whichever of fg / bg is further from the label.
            let away = if label.contrast(p.fg) > label.contrast(p.bg) { p.fg } else { p.bg };
            states(p.danger, p.danger.mix(away, 0.12), p.danger.mix(away, 0.24), &|c| p.text_on(c))
        }
    }
}

/// How long the pointer rests on a widget before its tooltip shows.
const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

/// Shows `text` as `r`'s tooltip once it has been hovered for
/// `TOOLTIP_DELAY`, or at once while it has keyboard focus.
pub(crate) fn delayed_tooltip(ui: &mut Ui, id: Id, r: &Response, text: &str) {
    let since = ui.memory::<Option<std::time::Instant>>(id.with("tip"));
    let now_hover = r.hovered && !r.pressed;
    let start = match (*since, now_hover) {
        (Some(t), true) => Some(t),
        (None, true) => Some(ui.now()),
        _ => None,
    };
    *ui.memory::<Option<std::time::Instant>>(id.with("tip")) = start;
    let shown = r.focused && ui.focus_visible();
    let waited = start.map(|t| ui.now().saturating_duration_since(t));
    match waited {
        Some(w) if w >= TOOLTIP_DELAY => tooltip(ui, r.rect, text),
        Some(w) => {
            ui.request_repaint_in(TOOLTIP_DELAY - w);
            if shown {
                tooltip(ui, r.rect, text);
            }
        }
        None if shown => tooltip(ui, r.rect, text),
        None => {}
    }
}

/// The width a button needs for `text` (and an icon) at its padding.
pub fn button_width(ui: &mut Ui, text: &str, icons: usize) -> i32 {
    let (font, px) = ui.text_style(TextRole::Strong);
    let tw = ui.measure(text, font, px).ceil() as i32;
    let icon = ui.px(tokens::ICON) + ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
    tw + icons as i32 * icon + 2 * ui.px(tokens::SPACE_MD)
}

/// A push button with a centred label (secondary style). Focusable;
/// Space/Enter activate it.
pub fn button(ui: &mut Ui, id: Id, rect: Rect, text: &str) -> Response {
    button_with(ui, id, rect, text, ButtonOpts::default())
}

/// A push button with a centred label and optional icons, in the variant
/// `opts.kind`. Focusable while enabled; Space/Enter activate it; the
/// keyboard focus ring is drawn by `Ui`.
pub fn button_with(ui: &mut Ui, id: Id, rect: Rect, text: &str, opts: ButtonOpts) -> Response {
    let sense = if opts.enabled { Sense::focusable() } else { Sense::hover() };
    let mut r = ui.interact(id, rect, sense);
    if opts.enabled {
        keyboard_activate(ui, id, &mut r);
    } else {
        r.clicked = false;
    }
    if let Some(n) = ui.a11y_node(id, Role::Button, opts.a11y_name.unwrap_or(text)) {
        n.disabled = !opts.enabled;
    }
    if r.hovered && opts.enabled {
        ui.set_cursor_icon(CursorIcon::Pointer);
    }
    let p = ui.style().palette;
    let radius = ui.style().radius * ui.scale();
    let [(_, rest, rest_fg), (_, hover, hover_fg), (_, pressed, pressed_fg)] = button_colors(&p, opts.kind);
    // Pressed beats hovered beats resting.
    let (bg, mut fg) = if !opts.enabled {
        (rest, p.text_disabled)
    } else if r.pressed {
        (pressed, pressed_fg)
    } else if r.hovered {
        (hover, hover_fg)
    } else {
        (rest, rest_fg)
    };
    let disabled_fill = !opts.enabled && matches!(opts.kind, ButtonKind::Primary | ButtonKind::Danger);
    let bg = if disabled_fill { p.surface_alt } else { bg };
    if disabled_fill {
        fg = p.text_disabled;
    }
    let role = if matches!(opts.kind, ButtonKind::Primary | ButtonKind::Danger) { TextRole::Strong } else { TextRole::Body };
    let (font, px) = ui.text_style(role);
    // Labels elide at the style padding; `button_width` asks for more room.
    let pad = ui.px(ui.style().padding).min(rect.w / 4);
    let icon_px = ui.px(tokens::ICON);
    let gap = ui.px(tokens::SPACE_XS + tokens::SPACE_2XS);
    let icons = opts.icon.is_some() as i32 + opts.trailing.is_some() as i32;
    let room = rect.w - 2 * pad - icons * (icon_px + gap);
    let shown = if text.is_empty() { String::new() } else { elide(ui, text, font, px, room) };
    let tw = ui.measure(&shown, font, px).ceil() as i32;
    let lh = ui.line_height(font, px);
    // Icons and label as one centred group.
    let group = tw + icons * icon_px + if shown.is_empty() { (icons - 1).max(0) * gap } else { icons * gap };
    let mut x = rect.x + (rect.w - group) / 2;
    let iy = rect.y + (rect.h - icon_px) / 2;
    let bw = ui.px(1).max(1);
    let mut painter = ui.painter();
    painter.rect(rect, bg, radius);
    if opts.kind == ButtonKind::Secondary {
        painter.border(rect, if opts.enabled { p.control } else { p.border }, bw, radius);
    }
    if let Some(i) = opts.icon {
        painter.icon(Rect::new(x, iy, icon_px, icon_px), i, fg);
        x += icon_px + gap;
    }
    if !shown.is_empty() {
        painter.text(Point::new(x, rect.y + (rect.h - lh) / 2), &shown, font, px, fg);
        x += tw + gap;
    }
    if let Some(i) = opts.trailing {
        painter.icon(Rect::new(x, iy, icon_px, icon_px), i, fg);
    }
    if let Some(t) = opts.tooltip {
        delayed_tooltip(ui, id, &r, t);
    }
    r
}

/// A square button showing only `icon` (16 px, centred). `name` is both
/// its accessible name and its tooltip (after a hover delay, or at once on
/// keyboard focus). Ghost and secondary kinds draw the icon muted until
/// hovered.
pub fn icon_button(ui: &mut Ui, id: Id, rect: Rect, icon: Icon, name: &str, kind: ButtonKind) -> Response {
    icon_button_tip(ui, id, rect, icon, name, name, kind)
}

/// `icon_button` whose tooltip `tip` differs from its accessible `name`
/// (e.g. "Settings" with the tooltip "Settings (Ctrl+Shift+,)").
pub fn icon_button_tip(ui: &mut Ui, id: Id, rect: Rect, icon: Icon, name: &str, tip: &str, kind: ButtonKind) -> Response {
    let mut r = ui.interact(id, rect, Sense::focusable());
    keyboard_activate(ui, id, &mut r);
    ui.a11y_node(id, Role::Button, name);
    if r.hovered {
        ui.set_cursor_icon(CursorIcon::Pointer);
    }
    let p = ui.style().palette;
    let [(_, rest, rest_fg), (_, hover, hover_fg), (_, pressed, pressed_fg)] = button_colors(&p, kind);
    let quiet = matches!(kind, ButtonKind::Ghost | ButtonKind::Secondary);
    let (bg, fg) = if r.pressed {
        (pressed, pressed_fg)
    } else if r.hovered {
        (hover, hover_fg)
    } else {
        (rest, if quiet { p.muted } else { rest_fg })
    };
    let radius = ui.style().radius * ui.scale();
    let s = ui.px(tokens::ICON);
    let bw = ui.px(1).max(1);
    let mut painter = ui.painter();
    painter.rect(rect, bg, radius);
    if kind == ButtonKind::Secondary {
        painter.border(rect, p.control, bw, radius);
    }
    painter.icon(Rect::new(rect.x + (rect.w - s) / 2, rect.y + (rect.h - s) / 2, s, s), icon, fg);
    delayed_tooltip(ui, id, &r, tip);
    r
}
